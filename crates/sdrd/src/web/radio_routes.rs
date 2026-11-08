use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::db;
use crate::radio::{covers, DeviceInfo, RadioPatch, RadioSettings, RadioState};

/// Settings key for the receiver chosen in the UI.
pub const DEVICE_KEY: &str = "radio.device";

pub async fn get_state(State(state): State<AppState>, _user: AuthUser) -> Json<RadioState> {
    Json(state.radio.state())
}

pub async fn configure(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(patch): Json<RadioPatch>,
) -> Result<Json<RadioSettings>, ApiError> {
    let s = state.radio.configure(&patch).map_err(ApiError::bad_request)?;
    tracing::info!(
        by = %admin.user.username,
        center_hz = s.center_hz,
        sample_rate = s.sample_rate,
        gain = ?s.gain_db,
        "radio configured"
    );
    Ok(Json(s))
}

/// Something the station listens to on a fixed frequency.
#[derive(Serialize)]
pub struct Feature {
    /// Which part of the app it belongs to.
    group: &'static str,
    label: String,
    freq_hz: u64,
    /// Switched on in its settings.
    enabled: bool,
}

fn features(state: &AppState) -> Vec<Feature> {
    let mut out = Vec::new();
    let r = state.rtl433.config();
    out.push(Feature { group: "Sensors", label: "rtl_433".into(), freq_hz: r.frequency_hz, enabled: r.enabled });
    let a = state.aprs.config();
    for c in a.channels {
        out.push(Feature { group: "APRS", label: c.label, freq_hz: c.frequency_hz, enabled: a.enabled && c.enabled });
    }
    let s = state.sstv.config();
    for c in s.channels {
        out.push(Feature { group: "SSTV", label: c.label, freq_hz: c.frequency_hz, enabled: s.enabled && c.enabled });
    }
    let sched = state.schedule.config();
    for sat in sched.satellites {
        out.push(Feature { group: "Passes", label: sat.name, freq_hz: sat.center_hz, enabled: sat.enabled });
    }
    for p in sched.profiles {
        out.push(Feature { group: "Rotation", label: p.name, freq_hz: p.center_hz, enabled: sched.rotation });
    }
    out
}

#[derive(Serialize)]
struct DeviceRow {
    #[serde(flatten)]
    device: DeviceInfo,
    /// Per entry of `features`: whether this device can receive it.
    supported: Vec<bool>,
}

/// Receivers to choose from, and which features each one can receive.
pub async fn devices(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    let features = features(&state);
    let devices: Vec<DeviceRow> = state
        .radio
        .devices()
        .await
        .into_iter()
        .map(|d| DeviceRow {
            supported: features.iter().map(|f| covers(&d.ranges, f.freq_hz)).collect(),
            device: d,
        })
        .collect();
    Json(json!({
        "can_select": state.radio.state().can_select,
        "devices": devices,
        "features": features,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectReq {
    id: String,
}

pub async fn select_device(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(r): Json<SelectReq>,
) -> Result<Json<Value>, ApiError> {
    state.radio.select(&r.id).map_err(ApiError::bad_request)?;
    db::set_setting(&state.db, DEVICE_KEY, &r.id).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, device = %r.id, "receiver selected");
    Ok(Json(json!({ "ok": true })))
}
