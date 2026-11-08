use axum::extract::State;
use axum::Json;

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::radio::{RadioPatch, RadioSettings, RadioState};

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
