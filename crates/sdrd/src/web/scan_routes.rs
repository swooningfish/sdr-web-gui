//! Band scan: admins start and stop it (it retunes the receiver); anyone
//! can see what it found and add stations to their bookmarks (admins also
//! to the station list).

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::auth::Role;
use crate::bookmarks::{self, BookmarkIn, MAX_PER_LIST};
use crate::scan::Band;

/// Most stations saved in one request.
const MAX_SAVE: usize = 500;

/// The scan's state and results; each station says whether it's already
/// among your bookmarks (same frequency and name).
pub async fn status(State(state): State<AppState>, AuthUser(s): AuthUser) -> Result<Json<Value>, ApiError> {
    let st = state.scan.status();
    let have = bookmarks::list(&state.db, s.user.id).await.map_err(ApiError::internal)?;
    let mut v = serde_json::to_value(&st).map_err(ApiError::internal)?;
    if let Some(found) = v.get_mut("found").and_then(Value::as_array_mut) {
        for (f, src) in found.iter_mut().zip(&st.found) {
            let saved = have.iter().any(|b| b.freq_hz == src.freq_hz && b.name.eq_ignore_ascii_case(&src.name));
            f["saved"] = json!(saved);
        }
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartReq {
    band: Band,
}

pub async fn start(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(r): Json<StartReq>,
) -> Result<Json<Value>, ApiError> {
    state
        .scan
        .start(r.band, state.radio.clone(), state.iq_tx.clone(), state.schedule.clone(), admin.user.username.clone())
        .map_err(ApiError::bad_request)?;
    tracing::info!(by = %admin.user.username, band = ?r.band, "band scan requested");
    Ok(Json(json!({ "ok": true })))
}

pub async fn stop(State(state): State<AppState>, AdminUser(_): AdminUser) -> Json<Value> {
    state.scan.stop();
    Json(json!({ "ok": true }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveReq {
    /// Ids of found stations.
    ids: Vec<String>,
    /// Into the shared station list (admins) rather than your own.
    #[serde(default)]
    shared: bool,
}

/// Add found stations as bookmarks; ones already there are skipped.
pub async fn save(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Json(r): Json<SaveReq>,
) -> Result<Json<Value>, ApiError> {
    if r.ids.is_empty() || r.ids.len() > MAX_SAVE {
        return Err(ApiError::bad_request(format!("pick 1–{MAX_SAVE} stations")));
    }
    let owner = if r.shared {
        if s.user.role != Role::Admin {
            return Err(ApiError::forbidden("only admins can change the station's shared bookmarks"));
        }
        None
    } else {
        Some(s.user.id)
    };
    let found = state.scan.status().found;
    let mut have = bookmarks::count(&state.db, owner).await.map_err(ApiError::internal)?;
    let (mut added, mut duplicates, mut missing) = (0, 0, 0);
    for id in &r.ids {
        let Some(f) = found.iter().find(|f| &f.id == id) else {
            missing += 1;
            continue;
        };
        let b = BookmarkIn {
            name: f.name.clone(),
            freq_hz: f.freq_hz,
            mode: f.mode,
            group: Some(f.group.clone()),
            colour: None,
            notes: Some(f.notes.clone()),
            sample_rate: f.sample_rate,
            shared: r.shared,
        }
        .validate()
        .map_err(ApiError::bad_request)?;
        if bookmarks::exists(&state.db, owner, &b.name, b.freq_hz).await.map_err(ApiError::internal)? {
            duplicates += 1;
            continue;
        }
        if have >= MAX_PER_LIST {
            return Err(ApiError::bad_request(format!(
                "added {added}, then the list was full ({MAX_PER_LIST} bookmarks)"
            )));
        }
        bookmarks::insert(&state.db, owner, &b).await.map_err(ApiError::internal)?;
        have += 1;
        added += 1;
    }
    tracing::info!(user = %s.user.username, shared = r.shared, added, "scanned stations bookmarked");
    Ok(Json(json!({ "added": added, "duplicates": duplicates, "missing": missing })))
}
