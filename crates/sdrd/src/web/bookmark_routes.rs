//! Bookmarks: everyone sees the shared station list and their own; admins
//! edit the shared list, each user their own.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::sensors::{IdList, MAX_BULK};
use super::{ApiError, AppState, AuthUser};
use crate::auth::{Role, Session};
use crate::bookmarks::{self, Bookmark, BookmarkIn, MAX_PER_LIST};

#[derive(Serialize)]
pub struct BookmarkOut {
    #[serde(flatten)]
    b: Bookmark,
    can_edit: bool,
}

fn is_admin(s: &Session) -> bool {
    s.user.role == Role::Admin
}

/// Owner column for a bookmark in this list (None = shared).
fn owner_for(s: &Session, shared: bool) -> Result<Option<i64>, ApiError> {
    if shared {
        if !is_admin(s) {
            return Err(ApiError::forbidden("only admins can change the station's shared bookmarks"));
        }
        Ok(None)
    } else {
        Ok(Some(s.user.id))
    }
}

fn may_edit(s: &Session, owner: Option<i64>) -> bool {
    match owner {
        None => is_admin(s),
        Some(o) => o == s.user.id,
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such bookmark")
}

pub async fn list(State(state): State<AppState>, AuthUser(s): AuthUser) -> Result<Json<Vec<BookmarkOut>>, ApiError> {
    let list = bookmarks::list(&state.db, s.user.id).await.map_err(ApiError::internal)?;
    let admin = is_admin(&s);
    Ok(Json(list.into_iter().map(|b| BookmarkOut { can_edit: !b.shared || admin, b }).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Json(req): Json<BookmarkIn>,
) -> Result<Json<Value>, ApiError> {
    let b = req.validate().map_err(ApiError::bad_request)?;
    let owner = owner_for(&s, b.shared)?;
    if bookmarks::count(&state.db, owner).await.map_err(ApiError::internal)? >= MAX_PER_LIST {
        return Err(ApiError::bad_request(format!("a list holds at most {MAX_PER_LIST} bookmarks")));
    }
    let id = bookmarks::insert(&state.db, owner, &b).await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn update(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<BookmarkIn>,
) -> Result<Json<Value>, ApiError> {
    let b = req.validate().map_err(ApiError::bad_request)?;
    let (_, owner) = bookmarks::get(&state.db, id).await.map_err(ApiError::internal)?.ok_or_else(not_found)?;
    if !may_edit(&s, owner) {
        return Err(ApiError::forbidden("only admins can change the station's shared bookmarks"));
    }
    // Moving between the shared list and your own needs rights on both.
    let new_owner = owner_for(&s, b.shared)?;
    bookmarks::update(&state.db, id, new_owner, &b).await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let (_, owner) = bookmarks::get(&state.db, id).await.map_err(ApiError::internal)?.ok_or_else(not_found)?;
    if !may_edit(&s, owner) {
        return Err(ApiError::forbidden("only admins can change the station's shared bookmarks"));
    }
    bookmarks::delete(&state.db, id).await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true })))
}

/// Delete several; ones the user may not change are skipped.
pub async fn remove_many(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Json(req): Json<IdList>,
) -> Result<Json<Value>, ApiError> {
    if req.ids.is_empty() || req.ids.len() > MAX_BULK {
        return Err(ApiError::bad_request(format!("give 1–{MAX_BULK} ids")));
    }
    let (mut deleted, mut skipped) = (0, 0);
    for id in req.ids {
        match bookmarks::get(&state.db, id).await.map_err(ApiError::internal)? {
            Some((_, owner)) if may_edit(&s, owner) => {
                bookmarks::delete(&state.db, id).await.map_err(ApiError::internal)?;
                deleted += 1;
            }
            _ => skipped += 1,
        }
    }
    Ok(Json(json!({ "deleted": deleted, "skipped": skipped })))
}

#[derive(Deserialize)]
pub struct ExportQuery {
    /// all (default), shared or mine
    #[serde(default)]
    scope: Option<String>,
}

pub async fn export(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let list = bookmarks::list(&state.db, s.user.id).await.map_err(ApiError::internal)?;
    let scope = q.scope.as_deref().unwrap_or("all");
    let list: Vec<Bookmark> = match scope {
        "shared" => list.into_iter().filter(|b| b.shared).collect(),
        "mine" => list.into_iter().filter(|b| !b.shared).collect(),
        "all" => list,
        _ => return Err(ApiError::bad_request("scope is all, shared or mine")),
    };
    let name = format!("bookmarks-{scope}.csv");
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        bookmarks::to_csv(&list),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct ImportReq {
    csv: String,
    /// Into the shared station list (admins) rather than your own.
    #[serde(default)]
    shared: bool,
}

pub async fn import(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Json(req): Json<ImportReq>,
) -> Result<Json<Value>, ApiError> {
    let owner = owner_for(&s, req.shared)?;
    if req.csv.len() > 1_000_000 {
        return Err(ApiError::bad_request("file too big (1 MB at most)"));
    }
    let parsed = bookmarks::from_csv(&req.csv).map_err(ApiError::bad_request)?;
    let mut have = bookmarks::count(&state.db, owner).await.map_err(ApiError::internal)?;
    let (mut added, mut duplicates) = (0, 0);
    let mut errors = parsed.errors;
    for b in parsed.rows {
        if bookmarks::exists(&state.db, owner, &b.name, b.freq_hz).await.map_err(ApiError::internal)? {
            duplicates += 1;
            continue;
        }
        if have >= MAX_PER_LIST {
            errors.push(format!("stopped at {MAX_PER_LIST} bookmarks"));
            break;
        }
        bookmarks::insert(&state.db, owner, &b).await.map_err(ApiError::internal)?;
        have += 1;
        added += 1;
    }
    let more = errors.len().saturating_sub(20);
    errors.truncate(20);
    if more > 0 {
        errors.push(format!("…and {more} more"));
    }
    tracing::info!(user = %s.user.username, shared = req.shared, added, "bookmarks imported");
    Ok(Json(json!({ "added": added, "duplicates": duplicates, "errors": errors })))
}
