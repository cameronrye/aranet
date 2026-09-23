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
//!
//! The agent is registered as BlueZ's default agent, so it receives pairing and
//! authorization requests for every device near the host. It approves pairing
//! only for devices that `aranet-core` is connecting to and rejects everything
//! else. It always rejects `AuthorizeService`, since aranet never needs a remote
//! device to connect to the host's own profiles.

use std::collections::BTreeSet;
use std::sync::Mutex;
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

/// Devices this process is connecting to, and may therefore pair with.
///
/// The agent is registered as BlueZ's default agent, so it receives pairing
/// requests for *every* device near the host. Only addresses added through
/// [`allow_pairing`] are approved; everything else is rejected.
static PAIRING_ALLOWED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Allow the agent to complete pairing for `address` (e.g. `AA:BB:CC:DD:EE:FF`).
pub(crate) fn allow_pairing(address: &str) {
    PAIRING_ALLOWED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(address.to_ascii_uppercase());
}

/// `AA:BB:CC:DD:EE:FF` from a BlueZ device path such as
/// `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`.
fn device_path_to_address(path: &str) -> Option<String> {
    let segment = path.rsplit('/').next()?.strip_prefix("dev_")?;
    let octets: Vec<&str> = segment.split('_').collect();
    let valid = octets.len() == 6
        && octets
            .iter()
            .all(|o| o.len() == 2 && o.chars().all(|c| c.is_ascii_hexdigit()));
    valid.then(|| octets.join(":").to_ascii_uppercase())
}

fn is_pairing_allowed(device_path: &str) -> bool {
    let Some(address) = device_path_to_address(device_path) else {
        return false;
    };
    PAIRING_ALLOWED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&address)
}

/// Reject agent requests for devices we didn't ask to pair with.
fn check_allowed(device: &dbus::Path) -> Result<(), dbus_crossroads::MethodErr> {
    if is_pairing_allowed(device) {
        Ok(())
    } else {
        warn!("BlueZ agent: rejecting request for {device} (not a device aranet is connecting to)");
        Err((
            "org.bluez.Error.Rejected",
            "Device is not managed by aranet",
        )
            .into())
    }
}

/// Answer BlueZ's `AuthorizeService` request: always reject.
///
/// BlueZ asks this when a remote device connects to one of the host's own
/// profiles (HID input, audio, PAN, ...). aranet is only ever a GATT client of
/// the sensor, so the Aranet flow never needs it. The pairing allow-list is
/// keyed by an address that Aranet devices broadcast in the clear, so approving
/// here would let anyone spoofing that address reach those profiles, for
/// example to inject keystrokes over HID.
fn authorize_service(device: &dbus::Path, uuid: &str) -> Result<(), dbus_crossroads::MethodErr> {
    warn!(
        "BlueZ agent: rejecting AuthorizeService {uuid} for {device} (aranet never accepts host profile connections)"
    );
    Err((
        "org.bluez.Error.Rejected",
        "aranet does not authorize host profile connections",
    )
        .into())
}

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
                check_allowed(&device)?;
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
                check_allowed(&device)
            },
        );

        b.method(
            "RequestAuthorization",
            ("device",),
            (),
            |_, _, (device,): (dbus::Path,)| {
                debug!("BlueZ agent: RequestAuthorization for {device}");
                check_allowed(&device)
            },
        );

        b.method(
            "AuthorizeService",
            ("device", "uuid"),
            (),
            |_, _, (device, uuid): (dbus::Path, String)| authorize_service(&device, &uuid),
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

    #[test]
    fn test_device_path_to_address() {
        assert_eq!(
            device_path_to_address("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").as_deref(),
            Some("AA:BB:CC:DD:EE:FF")
        );
        assert_eq!(
            device_path_to_address("/org/bluez/hci1/dev_aa_bb_cc_dd_ee_0f").as_deref(),
            Some("AA:BB:CC:DD:EE:0F")
        );
        assert_eq!(device_path_to_address("/org/bluez/hci0"), None);
        assert_eq!(device_path_to_address("/org/bluez/hci0/dev_AA_BB"), None);
        assert_eq!(
            device_path_to_address("/org/bluez/hci0/dev_ZZ_BB_CC_DD_EE_FF"),
            None
        );
    }

    #[test]
    fn test_pairing_only_allowed_for_registered_devices() {
        // Unique addresses: the allow-list is process-global.
        allow_pairing("aa:bb:cc:dd:ee:01");
        assert!(is_pairing_allowed("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01"));
        assert!(!is_pairing_allowed("/org/bluez/hci0/dev_11_22_33_44_55_66"));
        assert!(!is_pairing_allowed("/org/bluez/hci0"));
    }

    fn device(path: &str) -> dbus::Path<'static> {
        dbus::Path::new(path.to_owned()).unwrap()
    }

    fn assert_rejected(result: Result<(), dbus_crossroads::MethodErr>) {
        let err = result.expect_err("request should be rejected");
        assert_eq!(&**err.errorname(), "org.bluez.Error.Rejected");
    }

    #[test]
    fn test_check_allowed_rejects_unknown_device() {
        assert_rejected(check_allowed(&device(
            "/org/bluez/hci0/dev_11_22_33_44_55_02",
        )));
    }

    #[test]
    fn test_check_allowed_accepts_allowed_device() {
        allow_pairing("AA:BB:CC:DD:EE:03");
        assert!(check_allowed(&device("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_03")).is_ok());
    }

    #[test]
    fn test_authorize_service_rejected_even_for_allowed_device() {
        // aranet is only a GATT client, so it never needs a remote device to
        // connect to the host's own profiles (HID, audio, PAN). An allowed
        // address can be spoofed from its advertisements, so allowing it here
        // would open those profiles to anyone nearby.
        allow_pairing("AA:BB:CC:DD:EE:04");
        assert_rejected(authorize_service(
            &device("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_04"),
            "00001124-0000-1000-8000-00805f9b34fb",
        ));
        assert_rejected(authorize_service(
            &device("/org/bluez/hci0/dev_11_22_33_44_55_05"),
            "00001124-0000-1000-8000-00805f9b34fb",
        ));
    }
}
