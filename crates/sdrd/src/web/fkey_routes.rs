//! Each user's function keys on the receiver's front panel.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

use super::{ApiError, AppState, AuthUser};
use crate::fkeys::{self, FKey, FKeyIn};

pub async fn list(State(state): State<AppState>, AuthUser(s): AuthUser) -> Result<Json<Vec<FKey>>, ApiError> {
    Ok(Json(fkeys::list(&state.db, s.user.id).await.map_err(ApiError::internal)?))
}

pub async fn set(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Path(slot): Path<u8>,
    Json(req): Json<FKeyIn>,
) -> Result<Json<Value>, ApiError> {
    fkeys::check_slot(slot).map_err(ApiError::bad_request)?;
    let k = req.validate().map_err(ApiError::bad_request)?;
    fkeys::set(&state.db, s.user.id, slot, &k).await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn clear(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Path(slot): Path<u8>,
) -> Result<Json<Value>, ApiError> {
    fkeys::check_slot(slot).map_err(ApiError::bad_request)?;
    fkeys::clear(&state.db, s.user.id, slot).await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true })))
}
