//! Browse endpoints: roots, folder pages, video details and scan triggers.

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::json::ApiJson;
use crate::auth::middleware::{AdminDevice, AuthDevice};
use crate::db::models::{FolderPage, FolderRef, VideoDetail};
use crate::state::AppState;
use crate::util::unix_now;

/// Default number of entries per folder page.
const DEFAULT_PAGE: u64 = 200;

/// Largest accepted folder page.
const MAX_PAGE: u64 = 1000;

/// Query parameters of a folder page.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    /// Opaque cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Entries per page (1..=1000).
    pub limit: Option<u64>,
}

/// `GET /api/v1/roots`
pub async fn roots(
    State(state): State<AppState>,
    AuthDevice(_): AuthDevice,
) -> Result<Json<Vec<FolderRef>>, ApiError> {
    let db = state.db.clone();
    let roots = tokio::task::spawn_blocking(move || db.roots())
        .await
        .map_err(|_| ApiError::internal())??;
    Ok(Json(roots))
}

/// `GET /api/v1/folders/{id}?cursor=&limit=`
pub async fn folder(
    State(state): State<AppState>,
    AuthDevice(device): AuthDevice,
    Path(id): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<FolderPage>, ApiError> {
    let Query(query) = query.map_err(|_| ApiError::bad_request("invalid query"))?;
    let offset = match query.cursor.as_deref() {
        None => 0,
        Some(cursor) => cursor
            .parse::<u64>()
            .map_err(|_| ApiError::bad_request("invalid cursor"))?,
    };
    let limit = query.limit.unwrap_or(DEFAULT_PAGE);
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 1000"));
    }
    let db = state.db.clone();
    let page = tokio::task::spawn_blocking(move || db.folder_page(&id, &device.id, offset, limit))
        .await
        .map_err(|_| ApiError::internal())??;
    page.map(Json).ok_or_else(ApiError::not_found)
}

/// `GET /api/v1/videos/{id}`
pub async fn video(
    State(state): State<AppState>,
    AuthDevice(device): AuthDevice,
    Path(id): Path<String>,
) -> Result<Json<VideoDetail>, ApiError> {
    let db = state.db.clone();
    let detail = tokio::task::spawn_blocking(move || db.video_detail(&id, &device.id))
        .await
        .map_err(|_| ApiError::internal())??;
    detail.map(Json).ok_or_else(ApiError::not_found)
}

/// Longest resume position accepted (one week), to keep junk out of the
/// database.
const MAX_POSITION_MS: i64 = 7 * 24 * 3600 * 1000;

/// Body of a progress update.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressRequest {
    /// Resume position in milliseconds.
    pub position_ms: i64,
    /// Whether the video counts as watched; left unchanged when absent.
    pub watched: Option<bool>,
}

/// `PUT /api/v1/videos/{id}/progress`: saves this device's position.
pub async fn save_progress(
    State(state): State<AppState>,
    AuthDevice(device): AuthDevice,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<ProgressRequest>,
) -> Result<StatusCode, ApiError> {
    if !(0..=MAX_POSITION_MS).contains(&request.position_ms) {
        return Err(ApiError::bad_request("position_ms is out of range"));
    }
    let db = state.db.clone();
    let saved = tokio::task::spawn_blocking(move || {
        db.save_progress(
            &device.id,
            &id,
            request.position_ms,
            request.watched,
            unix_now(),
        )
    })
    .await
    .map_err(|_| ApiError::internal())??;
    if saved {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

/// `POST /api/v1/library/scan` (admin only): starts a background scan.
/// Returns 202, or 409 when a scan is already running.
pub async fn scan(
    State(state): State<AppState>,
    AdminDevice(_): AdminDevice,
) -> Result<StatusCode, ApiError> {
    if state.scan.trigger(state.db.clone()) {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError::conflict("a scan is already running"))
    }
}
