//! Read-only server facts for the admin Settings → System / Integrations tabs.
//! These come from `SDRD_*` environment variables, so they're shown, not edited.

use std::sync::atomic::Ordering;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use super::{AdminUser, AppState};

pub async fn get(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    let c = &state.cfg;
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "source": state.radio.state().source,
        "event_retention_days": c.event_retention_days,
        "ism_viewers": c.ism_viewers,
        "audio_viewers": c.audio_viewers,
        "audio_max_listeners": c.audio_max_listeners,
        "audio_listeners": state.audio_listeners.load(Ordering::Relaxed),
        "session_hours": c.session_hours,
        "trust_proxy": c.trust_proxy,
        "map_tiles": c.map_tile_url,
        "mqtt": c.mqtt.as_ref().map(|m| json!({
            "prefix": m.prefix,
            "discovery_prefix": m.discovery_prefix,
            "events": m.events,
        })),
        "mqtt_status": state.mqtt.as_ref().map(|m| m.status()),
    }))
}
