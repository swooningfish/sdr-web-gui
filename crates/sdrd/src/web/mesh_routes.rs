use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::auth::Role;
use crate::mesh::observer::ObserverConfig;
use crate::mesh::{MeshConfig, PRESETS};

/// Status and settings for the Mesh tab (channel keys for admins only).
pub async fn get(State(state): State<AppState>, AuthUser(s): AuthUser) -> Json<Value> {
    let m = &state.mesh;
    let cfg = m.config();
    let admin = s.user.role == Role::Admin;
    // Whether the receiver in use (main, or Mesh's own) hears any preset.
    let st = m.radio_settings();
    let tuned = PRESETS.iter().any(|p| crate::mesh::in_band(st.center_hz, st.sample_rate, p));
    Json(json!({
        "config": if admin { serde_json::to_value(&cfg).unwrap_or(Value::Null) } else { cfg.redacted() },
        "status": m.status(),
        "presets": PRESETS,
        "main_tuned": tuned,
        "own_receiver": state.receivers.mesh.is_dedicated(),
        "tile_url": state.cfg.map_tile_url,
    }))
}

pub async fn set_config(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(cfg): Json<MeshConfig>,
) -> Result<Json<Value>, ApiError> {
    state.mesh.set_config(cfg.clone()).await.map_err(ApiError::bad_request)?;
    // Names only: the keys stay out of the log.
    tracing::info!(
        by = %admin.user.username,
        enabled = cfg.enabled,
        presets = ?cfg.presets,
        meshtastic_channels = cfg.meshtastic_channels.len(),
        meshcore_channels = cfg.meshcore_channels.len(),
        "mesh settings changed"
    );
    Ok(Json(json!({ "ok": true })))
}

pub async fn nodes(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    Json(json!({ "now": crate::db::now_ms(), "nodes": state.mesh.nodes() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteReq {
    keys: Vec<String>,
}

pub async fn delete_nodes(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(r): Json<DeleteReq>,
) -> Result<Json<Value>, ApiError> {
    if r.keys.len() > 10_000 {
        return Err(ApiError::bad_request("too many nodes"));
    }
    state.mesh.delete_nodes(&r.keys).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, count = r.keys.len(), "mesh nodes deleted");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct Limit {
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    300
}

/// Channel messages, newest first (private channels for admins only).
pub async fn messages(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Query(q): Query<Limit>,
) -> Result<Json<Value>, ApiError> {
    let admin = s.user.role == Role::Admin;
    let list = state.mesh.messages(admin, q.limit).await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "messages": list })))
}

/// The recent packet log, newest first.
pub async fn packets(State(state): State<AppState>, AuthUser(s): AuthUser) -> Json<Value> {
    let admin = s.user.role == Role::Admin;
    Json(json!({ "packets": state.mesh.packets(admin) }))
}

/// MeshCore observer (LetsMesh) settings, identity and connection state.
pub async fn get_observer(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    Json(state.mesh.observer.info())
}

pub async fn set_observer(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(cfg): Json<ObserverConfig>,
) -> Result<Json<Value>, ApiError> {
    let cfg = state.mesh.observer.set_config(cfg).await.map_err(ApiError::bad_request)?;
    tracing::info!(
        by = %admin.user.username,
        enabled = cfg.enabled,
        broker = %cfg.broker,
        iata = %cfg.iata,
        name = %cfg.name,
        "mesh observer settings changed"
    );
    Ok(Json(state.mesh.observer.info()))
}
