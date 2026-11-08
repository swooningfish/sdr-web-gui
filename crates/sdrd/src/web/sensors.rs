//! ISM / TPMS devices, their events, and the rtl_433 decoder settings.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::auth::{Role, Session};
use crate::decoders::rtl433::{Rtl433Config, SpecError};
use crate::events::{self, Device, EventRow};

/// Other people's sensor data (TPMS IDs especially) is only shown to admins
/// unless SDRD_ISM_VIEWERS is set.
pub fn can_see_ism(state: &AppState, s: &Session) -> bool {
    s.user.role == Role::Admin || state.cfg.ism_viewers
}

fn ism_user(state: &AppState, s: &Session) -> Result<(), ApiError> {
    if can_see_ism(state, s) {
        Ok(())
    } else {
        Err(ApiError::forbidden("sensor data is limited to admins"))
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such device")
}

#[derive(Deserialize)]
pub struct ListQuery {
    limit: Option<i64>,
    before: Option<i64>,
    since: Option<i64>,
}

pub async fn list_devices(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Device>>, ApiError> {
    ism_user(&state, &s)?;
    let limit = q.limit.unwrap_or(500).clamp(1, 2000);
    Ok(Json(events::list_devices(&state.db, limit).await.map_err(ApiError::internal)?))
}

pub async fn device_events(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<EventRow>>, ApiError> {
    ism_user(&state, &s)?;
    // Enough for a week of a sensor that reports every couple of minutes.
    let limit = q.limit.unwrap_or(200).clamp(1, 10_000);
    Ok(Json(
        events::device_events(&state.db, id, q.since, q.before, limit)
            .await
            .map_err(ApiError::internal)?,
    ))
}

pub async fn recent_events(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<EventRow>>, ApiError> {
    ism_user(&state, &s)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    Ok(Json(events::recent_events(&state.db, limit).await.map_err(ApiError::internal)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePatch {
    /// Empty string clears the name.
    name: Option<String>,
    tracked: Option<bool>,
}

pub async fn update_device(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    Json(p): Json<DevicePatch>,
) -> Result<Json<Device>, ApiError> {
    let before = events::get_device(&state.db, id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(not_found)?;
    let name = match p.name {
        None => None,
        Some(n) => {
            let n = n.trim().to_string();
            if n.chars().count() > 64 || n.chars().any(char::is_control) {
                return Err(ApiError::bad_request("name must be at most 64 characters"));
            }
            Some(if n.is_empty() { None } else { Some(n) })
        }
    };
    events::update_device(&state.db, id, name, p.tracked)
        .await
        .map_err(ApiError::internal)?;
    let after = events::get_device(&state.db, id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(not_found)?;
    if let Some(m) = &state.mqtt {
        if after.tracked {
            m.announce(&after);
        } else if before.tracked {
            m.retract(&before);
        }
    }
    tracing::info!(by = %admin.user.username, device = %after.key, tracked = after.tracked, "device updated");
    Ok(Json(after))
}

pub async fn delete_device(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let d = events::get_device(&state.db, id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(not_found)?;
    events::delete_device(&state.db, id).await.map_err(ApiError::internal)?;
    if let (Some(m), true) = (&state.mqtt, d.tracked) {
        m.retract(&d);
    }
    tracing::info!(by = %admin.user.username, device = %d.key, "device deleted");
    Ok(Json(json!({ "ok": true })))
}

pub async fn get_rtl433(State(state): State<AppState>, AuthUser(s): AuthUser) -> Result<Json<Value>, ApiError> {
    ism_user(&state, &s)?;
    Ok(Json(json!({
        "config": state.rtl433.config(),
        "status": state.rtl433.status(),
        "mqtt": state.mqtt.as_ref().map(|m| m.status()),
    })))
}

pub async fn set_rtl433(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(cfg): Json<Rtl433Config>,
) -> Result<Json<Value>, ApiError> {
    cfg.validate().map_err(ApiError::bad_request)?;
    for f in cfg.flex.iter().filter(|f| f.enabled) {
        match state.rtl433.check_spec(&f.spec).await {
            Ok(()) => {}
            Err(SpecError::Invalid(m)) => return Err(ApiError::bad_request(format!("{}: {m}", f.spec))),
            // Can't verify without the binary; the decoder status will show the problem.
            Err(SpecError::Unavailable(m)) => tracing::warn!("{m}"),
        }
    }
    state
        .rtl433
        .set_config(&state.db, cfg.clone())
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, enabled = cfg.enabled, freq = cfg.frequency_hz, "rtl_433 configured");
    Ok(Json(json!({ "config": cfg })))
}

#[derive(Deserialize)]
pub struct CheckReq {
    spec: String,
}

pub async fn check_spec(
    State(state): State<AppState>,
    _admin: AdminUser,
    Json(req): Json<CheckReq>,
) -> Json<Value> {
    Json(match state.rtl433.check_spec(&req.spec).await {
        Ok(()) => json!({ "ok": true }),
        Err(SpecError::Invalid(m)) | Err(SpecError::Unavailable(m)) => json!({ "ok": false, "message": m }),
    })
}
