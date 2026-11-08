//! APRS channel settings and station tracks (APRS is public: any signed-in user).

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::decoders::aprs::{AprsConfig, NAME};
use crate::events;

pub async fn get_config(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    Json(json!({
        "config": state.aprs.config(),
        "status": state.aprs.status(),
        "map_tiles": state.cfg.map_tile_url,
    }))
}

pub async fn set_config(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(cfg): Json<AprsConfig>,
) -> Result<Json<Value>, ApiError> {
    cfg.validate().map_err(ApiError::bad_request)?;
    state
        .aprs
        .set_config(&state.db, cfg.clone())
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, channels = cfg.channels.len(), "APRS configured");
    Ok(Json(json!({ "config": cfg })))
}

#[derive(Deserialize)]
pub struct TrackQuery {
    since: Option<i64>,
}

/// `[{ device_id, points: [[ts, lat, lon], ...] }]` for stations with 2+ positions.
pub async fn tracks(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(q): Query<TrackQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let since = q.since.unwrap_or_else(|| crate::db::now_ms() - 6 * 3_600_000);
    let rows = events::tracks(&state.db, NAME, since).await.map_err(ApiError::internal)?;
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let id = rows[i].0;
        let pts: Vec<Value> = rows[i..]
            .iter()
            .take_while(|r| r.0 == id)
            .map(|(_, ts, lat, lon)| json!([ts, lat, lon]))
            .collect();
        i += pts.len();
        if pts.len() > 1 {
            out.push(json!({ "device_id": id, "points": pts }));
        }
    }
    Ok(Json(out))
}
