//! Scheduler, station location, TLEs and pass recordings.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::events;
use crate::orbit::Observer;
use crate::scheduler::{ScheduleConfig, PASSES};

pub async fn get(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    Json(json!({ "config": state.schedule.config(), "status": state.schedule.status() }))
}

pub async fn set_config(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(cfg): Json<ScheduleConfig>,
) -> Result<Json<Value>, ApiError> {
    cfg.validate().map_err(ApiError::bad_request)?;
    state
        .schedule
        .set_config(&state.db, cfg.clone())
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, rotation = cfg.rotation, "schedule configured");
    Ok(Json(json!({ "config": cfg })))
}

pub async fn set_location(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(obs): Json<Observer>,
) -> Result<Json<Value>, ApiError> {
    obs.validate().map_err(ApiError::bad_request)?;
    state
        .schedule
        .set_location(&state.db, obs)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, "station location set");
    Ok(Json(json!({ "location": obs })))
}

pub async fn refresh_tle(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    state.schedule.refresh_tle().await;
    Json(json!({ "ok": true }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualTle {
    name: String,
    line1: String,
    line2: String,
}

pub async fn set_tle(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(t): Json<ManualTle>,
) -> Result<Json<Value>, ApiError> {
    if t.name.chars().count() > 32 || t.line1.len() > 80 || t.line2.len() > 80 {
        return Err(ApiError::bad_request("TLE lines must be at most 80 characters"));
    }
    let id = state
        .schedule
        .set_manual_tle(&state.db, t.name.trim(), &t.line1, &t.line2)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    tracing::info!(by = %admin.user.username, norad = id, "TLE entered by hand");
    Ok(Json(json!({ "norad_id": id })))
}

#[derive(Deserialize)]
pub struct TestReq {
    seconds: u32,
}

/// Run a pretend pass now: tunes, records and logs it like a real one.
pub async fn test_pass(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(r): Json<TestReq>,
) -> Result<Json<Value>, ApiError> {
    if !(10..=900).contains(&r.seconds) {
        return Err(ApiError::bad_request("test pass must be 10–900 s"));
    }
    if state.schedule.status().active.is_some() {
        return Err(ApiError::bad_request("a pass is already in progress"));
    }
    state.schedule.test_pass(r.seconds).await;
    tracing::info!(by = %admin.user.username, seconds = r.seconds, "test pass started");
    Ok(Json(json!({ "ok": true })))
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such recording")
}

/// The recording's file name, if the event is a pass with one we wrote.
async fn audio_file(state: &AppState, id: i64) -> Result<Option<String>, ApiError> {
    let (ev, decoder) = events::get_event(&state.db, id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(not_found)?;
    if decoder != PASSES {
        return Err(not_found());
    }
    let data: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
    Ok(data
        .get("audio")
        .and_then(Value::as_str)
        .filter(|s| {
            s.ends_with(".wav") && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        })
        .map(String::from))
}

/// WAV download/stream (ServeFile handles Range requests for seeking).
pub async fn audio(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
    req: Request,
) -> Result<Response, ApiError> {
    let name = audio_file(&state, id).await?.ok_or_else(not_found)?;
    let res = ServeFile::new(state.schedule.passes_dir.join(name))
        .oneshot(req)
        .await
        .map_err(ApiError::internal)?;
    Ok(res.map(Body::new).into_response())
}

pub async fn delete_pass(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    if let Some(name) = audio_file(&state, id).await? {
        let _ = tokio::fs::remove_file(state.schedule.passes_dir.join(name)).await;
    }
    events::delete_event(&state.db, id).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, id, "pass recording deleted");
    Ok(Json(json!({ "ok": true })))
}
