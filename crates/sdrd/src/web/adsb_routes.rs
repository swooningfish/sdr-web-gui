use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::radio_routes::TCP_SERVERS_KEY;
use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::adsb::{self, acdb, AdsbConfig, ReceiverSettings, ADSB_HZ};
use crate::db;
use crate::radio::{covers, OFF};

/// Status, settings and counts for the Aircraft tab.
pub async fn get(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    let a = &state.adsb;
    let receiver = db::get_setting::<ReceiverSettings>(&state.db, adsb::RADIO_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    Json(json!({
        "config": a.config(),
        "status": a.status().await,
        "receiver_settings": receiver,
        "seen_24h": a.seen_24h().await,
        "main_tuned": adsb::main_in_band(
            a.main_radio().state().settings.center_hz,
            a.main_radio().state().settings.sample_rate,
        ),
        "tile_url": state.cfg.map_tile_url,
        "database_url": acdb::SOURCE_URL,
    }))
}

pub async fn set_config(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(cfg): Json<AdsbConfig>,
) -> Result<Json<Value>, ApiError> {
    state.adsb.set_config(cfg.clone()).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, enabled = cfg.enabled, "ADS-B settings changed");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct Since {
    #[serde(default)]
    since: i64,
}

/// Live aircraft, with trail points newer than `since` (ms; 0 = all).
pub async fn aircraft(State(state): State<AppState>, _user: AuthUser, Query(q): Query<Since>) -> Json<Value> {
    Json(state.adsb.aircraft(q.since))
}

pub async fn range(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    Json(state.adsb.range())
}

pub async fn reset_range(State(state): State<AppState>, AdminUser(admin): AdminUser) -> Result<Json<Value>, ApiError> {
    state.adsb.reset_range().await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, "ADS-B range stats reset");
    Ok(Json(json!({ "ok": true })))
}

/// Receivers that could be the ADS-B one, and whether each reaches 1090 MHz.
pub async fn devices(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    let list: Vec<Value> = state
        .adsb
        .radio
        .devices()
        .await
        .into_iter()
        .map(|d| {
            let ok = covers(&d.ranges, ADSB_HZ);
            let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
            if let Value::Object(m) = &mut v {
                m.insert("covers_1090".into(), json!(ok));
            }
            v
        })
        .collect();
    Json(json!({
        "usb_supported": cfg!(feature = "rtlsdr"),
        "selected": state.adsb.radio.state().device_id,
        "devices": list,
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
    let radio = &state.adsb.radio;
    if r.id != OFF {
        if let Some(d) = radio.devices().await.into_iter().find(|d| d.id == r.id) {
            if !covers(&d.ranges, ADSB_HZ) {
                return Err(ApiError::bad_request(format!("{} can't receive 1090 MHz", d.name)));
            }
        }
    }
    radio.select(&r.id).map_err(ApiError::bad_request)?;
    db::set_setting(&state.db, adsb::DEVICE_KEY, &r.id).await.map_err(ApiError::internal)?;
    if let Some(addr) = r.id.strip_prefix("tcp:") {
        let list = radio.add_tcp_server(addr);
        db::set_setting(&state.db, TCP_SERVERS_KEY, &list).await.map_err(ApiError::internal)?;
    }
    tracing::info!(by = %admin.user.username, device = %r.id, "ADS-B receiver selected");
    Ok(Json(json!({ "ok": true })))
}

/// Gain, ppm and bias-T of the ADS-B receiver (saved even while it's off).
pub async fn set_receiver(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(s): Json<ReceiverSettings>,
) -> Result<Json<Value>, ApiError> {
    if s.gain_db.is_some_and(|g| !(0.0..=60.0).contains(&g) || !g.is_finite()) {
        return Err(ApiError::bad_request("gain must be between 0 and 60 dB"));
    }
    if !(-200..=200).contains(&s.ppm) {
        return Err(ApiError::bad_request("ppm must be between -200 and 200"));
    }
    state.adsb.radio.force_settings(s.radio()).map_err(ApiError::bad_request)?;
    db::set_setting(&state.db, adsb::RADIO_KEY, &s).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, gain = ?s.gain_db, ppm = s.ppm, bias_tee = s.bias_tee, "ADS-B receiver configured");
    Ok(Json(json!({ "ok": true })))
}

/// Start downloading the aircraft database (runs in the background).
pub async fn download_db(State(state): State<AppState>, AdminUser(admin): AdminUser) -> Result<Json<Value>, ApiError> {
    if acdb::status(&state.db).await.downloading {
        return Err(ApiError::new(axum::http::StatusCode::CONFLICT, "already downloading"));
    }
    tracing::info!(by = %admin.user.username, "aircraft database download started");
    let pool = state.db.clone();
    let adsb = state.adsb.clone();
    tokio::spawn(async move {
        match acdb::download(&pool).await {
            Ok(_) => adsb.refresh_db(),
            Err(e) => tracing::warn!("aircraft database: {e}"),
        }
    });
    Ok(Json(json!({ "ok": true })))
}
