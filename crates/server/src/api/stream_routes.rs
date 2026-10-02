//! Direct play: the original file, served with HTTP Range support.
//!
//! A client only ever names a video id. The stored relative path is resolved
//! through the path jail, and bytes are read only through the handle the jail
//! opened and verified.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use crate::api::error::ApiError;
use crate::api::range::{RangeError, parse_range};
use crate::audit;
use crate::auth::middleware::{AuthDevice, ClientIp};
use crate::error::ServerError;
use crate::scanner::formats::video_mime;
use crate::scanner::walk::unix_mtime_ns;
use crate::state::AppState;

/// `GET /api/v1/videos/{id}/file`
pub async fn file(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AuthDevice(device): AuthDevice,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let db = state.db.clone();
    let lookup = id.clone();
    let location = tokio::task::spawn_blocking(move || db.video_location(&lookup))
        .await
        .map_err(|_| ApiError::internal())??
        .ok_or_else(ApiError::not_found)?;

    let roots = state.roots.clone();
    let (root_index, rel_path) = (location.root_index, location.rel_path.clone());
    let opened = tokio::task::spawn_blocking(move || {
        let file = roots.open(root_index, &rel_path)?;
        let metadata = file.metadata()?;
        Ok::<_, ServerError>((file, metadata))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let (file, metadata) = match opened {
        Ok(opened) => opened,
        // Only a genuine escape is a security event; a missing or stale file
        // is ordinary and must not pollute the audit log.
        Err(error @ ServerError::PathEscape { .. }) => {
            audit::path_violation(&ip.to_string(), &device.id, &id);
            return Err(error.into());
        }
        Err(_) => return Err(ApiError::not_found()),
    };

    let len = metadata.len();
    let mime = video_mime(&location.rel_path).unwrap_or("application/octet-stream");
    // Live size and nanosecond mtime, so a file replaced since the last scan
    // never matches a stale ETag.
    let mtime_ns = unix_mtime_ns(&metadata).unwrap_or(location.mtime_ns);
    let etag = format!("\"{len:x}-{mtime_ns:x}\"");
    let file = tokio::fs::File::from_std(file);
    serve_with_range(file, &headers, len, mime, &etag).await
}

/// Serves `file` honouring `Range` and `If-None-Match`.
async fn serve_with_range(
    file: tokio::fs::File,
    headers: &HeaderMap,
    len: u64,
    mime: &str,
    etag: &str,
) -> Result<Response, ApiError> {
    if headers.get(header::RANGE).is_none() && matches_etag(headers, etag) {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, etag.to_string())],
            Body::empty(),
        )
            .into_response());
    }
    let requested = headers
        .get(header::RANGE)
        .map(|value| value.to_str().unwrap_or_default());
    match requested {
        None => serve(file, 0, len.saturating_sub(1), len, mime, etag, false).await,
        Some(raw) => match parse_range(raw, len) {
            Ok((start, end)) => serve(file, start, end, len, mime, etag, true).await,
            Err(RangeError::Unsatisfiable) => Ok(unsatisfiable(len)),
            Err(RangeError::Invalid) => Err(ApiError::bad_request("invalid range")),
        },
    }
}

async fn serve(
    mut file: tokio::fs::File,
    start: u64,
    end: u64,
    len: u64,
    mime: &str,
    etag: &str,
    partial: bool,
) -> Result<Response, ApiError> {
    let length = if len == 0 { 0 } else { end - start + 1 };
    let body = if length == 0 {
        Body::empty()
    } else {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|_| ApiError::internal())?;
        Body::from_stream(ReaderStream::new(file.take(length)))
    };
    let mut builder = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "private, no-transform");
    if partial {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    builder.body(body).map_err(|_| ApiError::internal())
}

fn unsatisfiable(len: u64) -> Response {
    (
        StatusCode::RANGE_NOT_SATISFIABLE,
        [(header::CONTENT_RANGE, format!("bytes */{len}"))],
        Body::empty(),
    )
        .into_response()
}

fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag))
}
