//! WebSocket stream: binary waterfall frames plus JSON status messages.

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use super::{check_origin, ApiError, AppState, AuthUser};
use crate::auth;
use crate::radio::RadioState;

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    AuthUser(session): AuthUser,
) -> Result<Response, ApiError> {
    // Browsers send cookies on cross-site WebSocket upgrades, so check Origin.
    check_origin(&state.cfg, &headers)?;
    let token_hash = session.token_hash;
    Ok(ws
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| client(socket, state, token_hash)))
}

fn radio_msg(s: &RadioState) -> String {
    json!({ "type": "radio", "state": s }).to_string()
}

async fn client(mut socket: WebSocket, state: AppState, token_hash: String) {
    let mut wf = state.waterfall_tx.subscribe();
    let mut radio = state.radio.subscribe();
    let initial = radio_msg(&radio.borrow_and_update());
    if socket.send(Message::Text(initial.into())).await.is_err() {
        return;
    }

    // Re-check the session periodically so logout / user deletion / password
    // change also ends open streams.
    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    recheck.tick().await;

    loop {
        tokio::select! {
            frame = wf.recv() => match frame {
                Ok(f) => {
                    if socket.send(Message::Binary(f)).await.is_err() { break; }
                }
                Err(RecvError::Lagged(_)) => continue, // slow client: drop frames
                Err(RecvError::Closed) => break,
            },
            changed = radio.changed() => {
                if changed.is_err() { break; }
                let msg = radio_msg(&radio.borrow_and_update());
                if socket.send(Message::Text(msg.into())).await.is_err() { break; }
            }
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {} // control goes via REST; ignore client messages
            },
            _ = recheck.tick() => {
                if !auth::session_valid(&state.db, &token_hash).await {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}
