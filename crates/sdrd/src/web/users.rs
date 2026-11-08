use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::{AdminUser, ApiError, AppState};
use crate::auth::{self, Role};

pub async fn list(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<auth::UserRow>>, ApiError> {
    Ok(Json(auth::list_users(&state.db).await.map_err(ApiError::internal)?))
}

#[derive(Deserialize)]
pub struct CreateReq {
    username: String,
    password: String,
    role: Role,
}

pub async fn create(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(req): Json<CreateReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = auth::create_user(&state.db, &req.username, &req.password, req.role)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    tracing::info!(by = %admin.user.username, user = %req.username, role = ?req.role, "user created");
    Ok(Json(json!({ "id": id })))
}

pub async fn remove(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if id == admin.user.id {
        return Err(ApiError::bad_request("you cannot delete your own account"));
    }
    let target = auth::get_user(&state.db, id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::new(axum::http::StatusCode::NOT_FOUND, "no such user"))?;
    if target.role == Role::Admin
        && auth::admin_count(&state.db).await.map_err(ApiError::internal)? <= 1
    {
        return Err(ApiError::bad_request("cannot delete the last admin"));
    }
    auth::delete_user(&state.db, id).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, user = %target.username, "user deleted");
    Ok(Json(json!({ "ok": true })))
}
