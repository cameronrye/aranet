//! WebSocket handler for real-time updates.

use std::sync::Arc;

use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use futures::{SinkExt, StreamExt};
use tracing::{debug, info, warn};

use crate::state::{AppState, ReadingEvent};

/// Create the WebSocket router.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/ws", get(ws_handler))
}

/// WebSocket upgrade handler.
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle a WebSocket connection.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to reading events FIRST (before sending snapshot)
    // This ensures we don't miss any readings published while sending the snapshot
    let mut rx = state.readings_tx.subscribe();

    info!("WebSocket client connected");

    // Send initial snapshot of latest readings for all devices
    // This ensures clients immediately see current state without waiting for next poll
    {
        let store = state.store.lock().await;
        if let Ok(devices) = store.list_devices() {
            for device in devices {
                if let Ok(Some(reading)) = store.get_latest_reading(&device.id) {
                    let event = ReadingEvent {
                        device_id: device.id.clone(),
                        reading,
                    };
                    if let Ok(json) = serde_json::to_string(&event) {
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            info!("WebSocket client disconnected during initial snapshot");
                            return;
                        }
                    }
                }
            }
        }
    }

    debug!("Sent initial snapshot to WebSocket client");

    // Spawn a task to send reading events to the client
    let mut send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let json = match serde_json::to_string(&event) {
                Ok(j) => j,
                Err(e) => {
                    warn!("Failed to serialize event: {}", e);
                    continue;
                }
            };

            if sender.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Spawn a task to receive messages from the client (for keep-alive pings)
    let mut recv_task = tokio::spawn(async move {
        while let Some(result) = receiver.next().await {
            match result {
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(data)) => {
                    debug!("Received ping");
                    // Pong is handled automatically by axum
                    let _ = data;
                }
                Ok(_) => {
                    // Ignore other messages
                }
                Err(e) => {
                    warn!("WebSocket receive error: {}", e);
                    break;
                }
            }
        }
    });

    // Wait for either task to finish, then abort the other
    tokio::select! {
        _ = &mut send_task => {
            recv_task.abort();
        },
        _ = &mut recv_task => {
            send_task.abort();
        },
    }

    info!("WebSocket client disconnected");
}
