//! Extra receivers for Mesh, Sensors and APRS (Settings → Receiver).

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::radio_routes::TCP_SERVERS_KEY;
use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::db;
use crate::radio::{covers, OFF};
use crate::receivers::{AuxReceiver, ReceiverSettings, Use};

fn pick<'a>(state: &'a AppState, u: &str) -> Result<&'a AuxReceiver, ApiError> {
    let u = Use::parse(u).ok_or_else(|| ApiError::bad_request("unknown receiver"))?;
    Ok(state.receivers.get(u))
}

fn describe(r: &AuxReceiver) -> Value {
    json!({
        "use": r.use_,
        "label": r.use_.label(),
        "selected": r.radio.state().device_id,
        // Its decoder is fed from it rather than from the main receiver.
        "dedicated": r.is_dedicated(),
        "state": r.radio.state(),
        "settings": r.settings(),
        "tuning": r.tuning(),
    })
}

pub async fn list(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    let list: Vec<Value> = Use::ALL.into_iter().map(|u| describe(state.receivers.get(u))).collect();
    Json(json!({ "receivers": list }))
}

/// Receivers that could be this one, and whether each reaches its band.
pub async fn devices(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(u): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let r = pick(&state, &u)?;
    let hz = r.tuning().center_hz;
    let list: Vec<Value> = r
        .radio
        .devices()
        .await
        .into_iter()
        .map(|d| {
            let ok = covers(&d.ranges, hz);
            let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
            if let Value::Object(m) = &mut v {
                m.insert("covers".into(), json!(ok));
            }
            v
        })
        .collect();
    Ok(Json(json!({ "selected": r.radio.state().device_id, "devices": list })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectReq {
    id: String,
}

pub async fn select_device(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(u): Path<String>,
    Json(req): Json<SelectReq>,
) -> Result<Json<Value>, ApiError> {
    let r = pick(&state, &u)?;
    if req.id != OFF {
        let hz = r.tuning().center_hz;
        if let Some(d) = r.radio.devices().await.into_iter().find(|d| d.id == req.id) {
            if !covers(&d.ranges, hz) {
                return Err(ApiError::bad_request(format!(
                    "{} can't receive {:.3} MHz",
                    d.name,
                    hz as f64 / 1e6
                )));
            }
        }
    }
    r.radio.select(&req.id).map_err(ApiError::bad_request)?;
    db::set_setting(&state.db, &r.use_.device_key(), &req.id).await.map_err(ApiError::internal)?;
    if let Some(addr) = req.id.strip_prefix("tcp:") {
        let list = r.radio.add_tcp_server(addr);
        db::set_setting(&state.db, TCP_SERVERS_KEY, &list).await.map_err(ApiError::internal)?;
    }
    tracing::info!(by = %admin.user.username, receiver = r.use_.key(), device = %req.id, "extra receiver selected");
    Ok(Json(json!({ "ok": true })))
}

/// Gain, ppm and bias-T (saved even while no device is selected).
pub async fn set_settings(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(u): Path<String>,
    Json(s): Json<ReceiverSettings>,
) -> Result<Json<Value>, ApiError> {
    let r = pick(&state, &u)?;
    r.set_settings(s.clone()).map_err(ApiError::bad_request)?;
    db::set_setting(&state.db, &r.use_.settings_key(), &s).await.map_err(ApiError::internal)?;
    tracing::info!(
        by = %admin.user.username,
        receiver = r.use_.key(),
        gain = ?s.gain_db,
        ppm = s.ppm,
        bias_tee = s.bias_tee,
        "extra receiver configured"
    );
    Ok(Json(json!({ "ok": true })))
}
