//! SSTV channel settings and received images. SSTV from the ISS and other
//! amateurs is public, so any signed-in user may view; admins change things.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::decoders::sstv::{SstvConfig, NAME};
use crate::events;

pub async fn get_config(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    Json(json!({ "config": state.sstv.config(), "status": state.sstv.status() }))
}

pub async fn set_config(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(cfg): Json<SstvConfig>,
) -> Result<Json<Value>, ApiError> {
    cfg.validate().map_err(ApiError::bad_request)?;
    state
        .sstv
        .set_config(&state.db, cfg.clone())
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, channels = cfg.channels.len(), "SSTV configured");
    Ok(Json(json!({ "config": cfg })))
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such image")
}

/// File names we wrote ourselves; anything else is refused.
fn safe_name(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?;
    (s.ends_with(".png") && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'))
        .then(|| s.to_string())
}

async fn image_files(state: &AppState, id: i64) -> Result<(Option<String>, Option<String>), ApiError> {
    let (ev, decoder) = events::get_event(&state.db, id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(not_found)?;
    if decoder != NAME {
        return Err(not_found());
    }
    let data: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
    Ok((safe_name(data.get("image")), safe_name(data.get("thumb"))))
}

#[derive(Deserialize)]
pub struct ImageQuery {
    thumb: Option<bool>,
}

pub async fn image(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<ImageQuery>,
) -> Result<Response, ApiError> {
    let (full, thumb) = image_files(&state, id).await?;
    let name = if q.thumb.unwrap_or(false) { thumb.or(full) } else { full }.ok_or_else(not_found)?;
    let bytes = tokio::fs::read(state.sstv.images_dir.join(&name))
        .await
        .map_err(|_| not_found())?;
    Response::builder()
        .header(header::CONTENT_TYPE, "image/png")
        // Images never change once written.
        .header(header::CACHE_CONTROL, "private, max-age=604800, immutable")
        .header(header::CONTENT_DISPOSITION, format!("inline; filename=\"{name}\""))
        .body(Body::from(bytes))
        .map_err(ApiError::internal)
}

pub async fn delete_image(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let (full, thumb) = image_files(&state, id).await?;
    for name in [full, thumb].into_iter().flatten() {
        let _ = tokio::fs::remove_file(state.sstv.images_dir.join(name)).await;
    }
    events::delete_event(&state.db, id).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, id, "SSTV image deleted");
    Ok(Json(json!({ "ok": true })))
}

/// Delete several images at once.
pub async fn delete_images(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(req): Json<super::sensors::IdList>,
) -> Result<Json<Value>, ApiError> {
    if req.ids.is_empty() || req.ids.len() > super::sensors::MAX_BULK {
        return Err(ApiError::bad_request("too many or no ids"));
    }
    let mut deleted = 0;
    for id in req.ids {
        // Ids that aren't SSTV images (or are already gone) are skipped.
        let Ok((full, thumb)) = image_files(&state, id).await else { continue };
        for name in [full, thumb].into_iter().flatten() {
            let _ = tokio::fs::remove_file(state.sstv.images_dir.join(name)).await;
        }
        events::delete_event(&state.db, id).await.map_err(ApiError::internal)?;
        deleted += 1;
    }
    tracing::info!(by = %admin.user.username, deleted, "SSTV images deleted in bulk");
    Ok(Json(json!({ "deleted": deleted })))
}

#[derive(Deserialize)]
pub struct ZipQuery {
    /// Comma-separated event ids.
    ids: String,
}

/// Download several images as one ZIP (stored, not compressed: PNGs already are).
pub async fn zip(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(q): Query<ZipQuery>,
) -> Result<Response, ApiError> {
    let ids: Vec<i64> = q.ids.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if ids.is_empty() || ids.len() > 500 {
        return Err(ApiError::bad_request("give 1–500 image ids"));
    }
    let mut files = Vec::new();
    for id in ids {
        if let Ok((Some(name), _)) = image_files(&state, id).await {
            if let Ok(bytes) = tokio::fs::read(state.sstv.images_dir.join(&name)).await {
                files.push((name, bytes));
            }
        }
    }
    if files.is_empty() {
        return Err(not_found());
    }
    let zipped = tokio::task::spawn_blocking(move || -> zip::result::ZipResult<Vec<u8>> {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in files {
            z.start_file(name, opts)?;
            z.write_all(&bytes)?;
        }
        Ok(z.finish()?.into_inner())
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"sstv-images.zip\"")
        .body(Body::from(zipped))
        .map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_file_names() {
        assert!(safe_name(Some(&json!("1794-iss-sstv-robot-36.png"))).is_some());
        assert!(safe_name(Some(&json!("../sdrd.db"))).is_none());
        assert!(safe_name(Some(&json!("a/b.png"))).is_none());
        assert!(safe_name(Some(&json!(".png"))).is_none());
        assert!(safe_name(None).is_none());
    }
}
