//! WebSocket stream: binary waterfall / SSTV / audio frames plus JSON status
//! messages. Clients send JSON only to control live audio:
//!
//! - `{"type":"audio","freq_hz":145800000,"mode":"nfm","squelch_db":-60}`
//!   starts listening, or retunes the listener (`squelch_db` may be null;
//!   add `"nr":true` for noise reduction)
//! - `{"type":"audio_stop"}`

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use super::{check_origin, ApiError, AppState, AuthUser};
use crate::audio::demod::{Mode, Params};
use crate::audio::{self, Listener};
use crate::auth::{self, Role};
use crate::radio::{RadioState, MAX_FREQ_HZ, MIN_FREQ_HZ};

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    AuthUser(session): AuthUser,
) -> Result<Response, ApiError> {
    // Browsers send cookies on cross-site WebSocket upgrades, so check Origin.
    check_origin(&state.cfg, &headers)?;
    let token_hash = session.token_hash.clone();
    let ism = super::sensors::can_see_ism(&state, &session);
    let can_listen = session.user.role == Role::Admin || state.cfg.audio_viewers;
    Ok(ws
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| client(socket, state, token_hash, ism, can_listen)))
}

fn radio_msg(s: &RadioState) -> String {
    json!({ "type": "radio", "state": s }).to_string()
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientMsg {
    Audio {
        freq_hz: u64,
        mode: Mode,
        squelch_db: Option<f32>,
        #[serde(default)]
        nr: bool,
    },
    AudioStop,
}

fn audio_msg(kind: &str, message: Option<&str>) -> Message {
    Message::Text(json!({ "type": kind, "message": message }).to_string().into())
}

/// Apply an audio control message. Returns a reply for the client, if any.
fn handle_audio(state: &AppState, listener: &mut Option<Listener>, can_listen: bool, text: &str) -> Option<Message> {
    match serde_json::from_str::<ClientMsg>(text) {
        Ok(ClientMsg::AudioStop) => {
            *listener = None;
            Some(audio_msg("audio_stopped", None))
        }
        Ok(ClientMsg::Audio { freq_hz, mode, squelch_db, nr }) => {
            if !can_listen {
                return Some(audio_msg("audio_error", Some("live audio is limited to admins on this server")));
            }
            if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&freq_hz)
                || squelch_db.is_some_and(|s| !(-140.0..=0.0).contains(&s) || !s.is_finite())
            {
                return Some(audio_msg("audio_error", Some("bad frequency or squelch")));
            }
            let params = Params { freq_hz, mode, squelch_db, nr };
            if let Some(l) = listener.as_ref() {
                l.params.send_replace(params);
                return None;
            }
            match audio::start(&state.audio_listeners, state.cfg.audio_max_listeners, state.iq_tx.subscribe(), params) {
                Ok(l) => {
                    *listener = Some(l);
                    Some(audio_msg("audio_started", None))
                }
                Err(e) => Some(audio_msg("audio_error", Some(&e))),
            }
        }
        Err(_) => None, // ignore anything we don't understand
    }
}

async fn client(mut socket: WebSocket, state: AppState, token_hash: String, ism: bool, can_listen: bool) {
    let mut wf = state.waterfall_tx.subscribe();
    let mut sstv = state.sstv_rows_tx.subscribe();
    let mut events = state.event_feed.subscribe();
    let mut radio = state.radio.subscribe();
    let initial = radio_msg(&radio.borrow_and_update());
    if socket.send(Message::Text(initial.into())).await.is_err() {
        return;
    }
    let mut listener: Option<Listener> = None;

    // Re-check the session periodically so logout / user deletion / password
    // change also ends open streams.
    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    recheck.tick().await;

    loop {
        let audio_frame = async {
            match listener.as_mut() {
                Some(l) => l.frames.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            frame = wf.recv() => match frame {
                Ok(f) => {
                    if socket.send(Message::Binary(f)).await.is_err() { break; }
                }
                Err(RecvError::Lagged(_)) => continue, // slow client: drop frames
                Err(RecvError::Closed) => break,
            },
            rows = sstv.recv() => match rows {
                Ok(f) => {
                    if socket.send(Message::Binary(f)).await.is_err() { break; }
                }
                // Missed rows only leave a gap in the live view; the saved image is whole.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            f = audio_frame => match f {
                Some(f) => {
                    if socket.send(Message::Binary(f)).await.is_err() { break; }
                }
                None => listener = None,
            },
            ev = events.recv() => match ev {
                Ok(msg) if ism || msg.public => {
                    if socket.send(Message::Text((&*msg.json).into())).await.is_err() { break; }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => tracing::debug!("ws client missed {n} events"),
                Err(RecvError::Closed) => break,
            },
            changed = radio.changed() => {
                if changed.is_err() { break; }
                let msg = radio_msg(&radio.borrow_and_update());
                if socket.send(Message::Text(msg.into())).await.is_err() { break; }
            }
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(t))) => {
                    if let Some(reply) = handle_audio(&state, &mut listener, can_listen, t.as_str()) {
                        if socket.send(reply).await.is_err() { break; }
                    }
                }
                Some(Ok(_)) => {}
            },
            _ = recheck.tick() => {
                if !auth::session_valid(&state.db, &token_hash).await {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
    // Dropping the listener frees its slot and stops its thread.
}
