//! BlueZ D-Bus agent for handling BLE authentication on Linux.
//!
//! Aranet devices expose a Battery Level characteristic that requires authentication.
//! When BlueZ discovers services, it reads this characteristic and gets an
//! "Insufficient Authentication" ATT error. BlueZ then initiates SMP pairing.
//!
//! Without a registered Bluetooth agent, the pairing request has no handler, causing
//! BlueZ to wait indefinitely and never resolve services. This blocks all subsequent
//! GATT operations including reads on characteristics that don't require authentication.
//!
//! This module registers a minimal `NoInputNoOutput` agent that allows BlueZ to complete
//! "Just Works" pairing, unblocking service discovery and characteristic reads.

use std::sync::atomic::{AtomicU8, Ordering};

use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus_crossroads::{Crossroads, IfaceBuilder};
use tracing::{debug, info, warn};

const STATE_IDLE: u8 = 0;
const STATE_STARTING: u8 = 1;
const STATE_REGISTERED: u8 = 2;
const STATE_FAILED_PERMANENTLY: u8 = 3;

/// Maximum agent registration attempts before giving up.
/// Each failed attempt leaks a D-Bus connection (the spawned resource task
/// is not abortable), so we cap retries to bound the leak.
const MAX_AGENT_ATTEMPTS: u8 = 3;

static AGENT_STATE: AtomicU8 = AtomicU8::new(STATE_IDLE);
static AGENT_ATTEMPTS: AtomicU8 = AtomicU8::new(0);
static AGENT_PATH: &str = "/dev/rye/aranet/agent";
const AGENT_CAPABILITY: &str = "NoInputNoOutput";

/// Ensure a BlueZ agent is registered for this process.
///
/// This is safe to call multiple times — the agent is only registered once.
/// If registration fails, subsequent calls will retry.
/// The agent runs in a background tokio task for the lifetime of the process.
pub fn ensure_agent() {
    // Only transition from IDLE → STARTING; all other states are no-ops.
    // REGISTERED and FAILED_PERMANENTLY are terminal. STARTING transitions
    // back to IDLE on failure (allowing retry on the next call).
    if AGENT_STATE
        .compare_exchange(
            STATE_IDLE,
            STATE_STARTING,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return;
    }
    tokio::spawn(async {
        match run_agent().await {
            Ok(()) => {
                AGENT_STATE.store(STATE_REGISTERED, Ordering::SeqCst);
            }
            Err(e) => {
                let attempt = AGENT_ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt >= MAX_AGENT_ATTEMPTS {
                    warn!(
                        "Failed to register BlueZ agent after {attempt} attempts: {e} — \
                         giving up (BLE scans may hang if pairing is required)"
                    );
                    AGENT_STATE.store(STATE_FAILED_PERMANENTLY, Ordering::SeqCst);
                } else {
                    warn!(
                        "Failed to register BlueZ agent (attempt {attempt}/{MAX_AGENT_ATTEMPTS}): \
                         {e} — will retry on next BLE operation"
                    );
                    // Reset to IDLE so a subsequent call can retry
                    AGENT_STATE.store(STATE_IDLE, Ordering::SeqCst);
                }
            }
        }
    });
}

async fn run_agent() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Connect to the system D-Bus
    let (resource, conn) = dbus_tokio::connection::new_system_sync()?;

    // Spawn the D-Bus event loop
    let _handle = tokio::spawn(async move {
        let err = resource.await;
        warn!("BlueZ agent D-Bus connection lost: {err}");
    });

    // Build the agent object using crossroads
    let mut cr = Crossroads::new();

    let iface_token = cr.register("org.bluez.Agent1", |b: &mut IfaceBuilder<()>| {
        b.method("Release", (), (), |_, _, ()| {
            debug!("BlueZ agent: Release");
            Ok(())
        });

        b.method(
            "RequestPasskey",
            ("device",),
            ("passkey",),
            |_, _, (device,): (dbus::Path,)| {
                debug!("BlueZ agent: RequestPasskey for {device}");
                // Return 0 for "Just Works" pairing
                Ok((0u32,))
            },
        );

        b.method(
            "RequestConfirmation",
            ("device", "passkey"),
            (),
            |_, _, (device, passkey): (dbus::Path, u32)| {
                debug!("BlueZ agent: RequestConfirmation for {device}, passkey {passkey}");
                Ok(())
            },
        );

        b.method(
            "RequestAuthorization",
            ("device",),
            (),
            |_, _, (device,): (dbus::Path,)| {
                debug!("BlueZ agent: RequestAuthorization for {device}");
                Ok(())
            },
        );

        b.method(
            "AuthorizeService",
            ("device", "uuid"),
            (),
            |_, _, (device, uuid): (dbus::Path, String)| {
                debug!("BlueZ agent: AuthorizeService {uuid} for {device}");
                Ok(())
            },
        );

        b.method("Cancel", (), (), |_, _, ()| {
            debug!("BlueZ agent: Cancel");
            Ok(())
        });
    });

    cr.insert(AGENT_PATH, &[iface_token], ());

    // Start handling incoming D-Bus messages
    conn.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |msg, conn| {
            if let Err(()) = cr.handle_message(msg, conn) {
                warn!("BlueZ agent: failed to handle D-Bus message");
            }
            true
        }),
    );

    // Register with BlueZ as the default agent so all pairing requests
    // (including passkey confirmation for public-address devices like Aranet4)
    // are routed to us. Without being the default agent, BlueZ has no handler
    // for pairing callbacks and pairing fails with "No agent available".
    let proxy = dbus::nonblock::Proxy::new(
        "org.bluez",
        "/org/bluez",
        std::time::Duration::from_secs(5),
        conn.clone(),
    );

    let () = proxy
        .method_call(
            "org.bluez.AgentManager1",
            "RegisterAgent",
            (dbus::Path::from(AGENT_PATH), AGENT_CAPABILITY),
        )
        .await?;

    let () = proxy
        .method_call(
            "org.bluez.AgentManager1",
            "RequestDefaultAgent",
            (dbus::Path::from(AGENT_PATH),),
        )
        .await?;

    info!("BlueZ agent registered as default ({AGENT_CAPABILITY})");

    // Keep the task alive — the agent needs to stay registered.
    // When the process exits, BlueZ automatically cleans up the agent
    // since the D-Bus connection drops.
    std::future::pending::<()>().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_state_constants_are_distinct() {
        let states = [
            STATE_IDLE,
            STATE_STARTING,
            STATE_REGISTERED,
            STATE_FAILED_PERMANENTLY,
        ];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "States at index {i} and {j} must differ");
                }
            }
        }
    }
}
