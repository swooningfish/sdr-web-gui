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
use crate::orbit::{self, Observer, Satellite};
use crate::radio::{covers, describe_ranges};
use crate::scheduler::{ScheduleConfig, TleRecord, PASSES};

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotationReq {
    enabled: bool,
}

/// Turn profile rotation on or off without touching the rest of the schedule.
pub async fn set_rotation(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(r): Json<RotationReq>,
) -> Result<Json<Value>, ApiError> {
    let mut cfg = state.schedule.config();
    if r.enabled && cfg.profiles.is_empty() {
        return Err(ApiError::bad_request("add a rotation profile in Settings → Rotation first"));
    }
    cfg.rotation = r.enabled;
    state
        .schedule
        .set_config(&state.db, cfg)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, enabled = r.enabled, "rotation switched");
    Ok(Json(json!({ "rotation": r.enabled })))
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

#[derive(Deserialize)]
pub struct TrackQuery {
    /// Start of the window (default: 10 minutes ago).
    from_ms: Option<i64>,
    /// Length of the window (default 110, at most 240).
    minutes: Option<u32>,
}

/// Ground tracks of the configured satellites, for the map: where each one
/// is over the Earth every 20 s across the window.
pub async fn tracks(
    State(state): State<AppState>,
    _user: AuthUser,
    axum::extract::Query(q): axum::extract::Query<TrackQuery>,
) -> Result<Json<Value>, ApiError> {
    const STEP_MS: i64 = 20_000;
    const MAX_FROM_MS: i64 = 4_102_444_800_000;
    let minutes = q.minutes.unwrap_or(110);
    if !(1..=240).contains(&minutes) {
        return Err(ApiError::bad_request("minutes must be 1–240"));
    }
    let from = q.from_ms.unwrap_or_else(|| crate::db::now_ms() - 10 * 60_000);
    // 1970–2100: keeps the arithmetic below far away from overflow.
    if !(0..=MAX_FROM_MS).contains(&from) {
        return Err(ApiError::bad_request("from_ms is out of range"));
    }
    let steps = minutes as i64 * 60_000 / STEP_MS;
    let to = from + steps * STEP_MS;
    let mut out = Vec::new();
    for s in state.schedule.config().satellites.iter().filter(|s| s.enabled) {
        let Ok(Some(rec)) =
            crate::db::get_setting::<TleRecord>(&state.db, &format!("tle.{}", s.norad_id)).await
        else {
            continue;
        };
        let Ok(sat) = Satellite::from_tle(&s.name, &rec.line1, &rec.line2) else { continue };
        let mut points = Vec::new();
        // A fixed number of steps, so the loop ends whatever the times are.
        for i in 0..=steps {
            let t = from + i * STEP_MS;
            if let Some((lat, lon, alt)) = orbit::ground(&sat, t) {
                points.push((t, (lat * 1e3).round() / 1e3, (lon * 1e3).round() / 1e3, alt.round()));
            }
        }
        out.push(json!({ "satellite": s.name, "norad_id": s.norad_id, "points": points }));
    }
    Ok(Json(json!({ "from_ms": from, "to_ms": to, "tracks": out })))
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
/// End the pass in progress now (a real pass is skipped, not restarted).
pub async fn stop_pass(State(state): State<AppState>, AdminUser(admin): AdminUser) -> Result<Json<Value>, ApiError> {
    if state.schedule.status().active.is_none() {
        return Err(ApiError::bad_request("no pass in progress"));
    }
    state.schedule.stop_pass().await;
    tracing::info!(by = %admin.user.username, "pass stopped");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkipReq {
    aos_ms: i64,
    #[serde(default = "yes")]
    skip: bool,
}

fn yes() -> bool {
    true
}

/// Stay on the current tuning for an upcoming pass (or change your mind).
pub async fn skip_pass(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(r): Json<SkipReq>,
) -> Result<Json<Value>, ApiError> {
    let st = state.schedule.status();
    if !st.passes.iter().any(|p| p.aos_ms == r.aos_ms) {
        return Err(ApiError::bad_request("no such pass"));
    }
    if st.active.as_ref().is_some_and(|a| a.aos_ms == r.aos_ms && !a.test) {
        return Err(ApiError::bad_request("that pass has started; stop it on the Satellites page"));
    }
    state.schedule.skip_pass(r.aos_ms, r.skip).await;
    tracing::info!(by = %admin.user.username, aos_ms = r.aos_ms, skip = r.skip, "pass skip changed");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct GeoQuery {
    q: String,
}

/// Place / postcode → coordinates via OpenStreetMap Nominatim, for setting
/// the station location when the browser can't share it. Nominatim's usage
/// policy allows at most one request a second, so we enforce that here.
pub async fn geocode(
    _admin: AdminUser,
    axum::extract::Query(q): axum::extract::Query<GeoQuery>,
) -> Result<Json<Value>, ApiError> {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let q = q.q.trim().to_string();
    if q.is_empty() || q.chars().count() > 200 {
        return Err(ApiError::bad_request("search must be 1–200 characters"));
    }
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1)) {
            return Err(ApiError::too_many(1));
        }
        *last = Some(std::time::Instant::now());
    }
    let body = tokio::task::spawn_blocking(move || -> Result<String, String> {
        ureq::get("https://nominatim.openstreetmap.org/search")
            .query("format", "jsonv2")
            .query("limit", "5")
            .query("q", &q)
            .set("User-Agent", "sdrd (sdr-web-gui)")
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("place search failed: {e}")))?;
    let found: Vec<Value> = serde_json::from_str(&body).unwrap_or_default();
    let places: Vec<Value> = found
        .iter()
        .filter_map(|p| {
            let lat: f64 = p.get("lat")?.as_str()?.parse().ok()?;
            let lon: f64 = p.get("lon")?.as_str()?.parse().ok()?;
            Some(json!({ "name": p.get("display_name")?.as_str()?, "lat_deg": lat, "lon_deg": lon }))
        })
        .collect();
    Ok(Json(json!({ "places": places })))
}

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
    let ranges = state.radio.state().ranges;
    if let Some(sat) = state.schedule.config().satellites.first() {
        if !covers(&ranges, sat.center_hz) {
            return Err(ApiError::bad_request(format!(
                "{} ({:.3} MHz) is outside this SDR's range ({})",
                sat.name,
                sat.center_hz as f64 / 1e6,
                describe_ranges(&ranges)
            )));
        }
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
