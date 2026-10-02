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
use crate::api::stream_limit::{SlotReader, StreamSlot};
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
    if if_none_match(&headers, &etag) {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, CACHE_CONTROL.to_owned()),
            ],
            Body::empty(),
        )
            .into_response());
    }
    let range = match usable_range(&headers, &etag, len) {
        Ok(range) => range,
        Err(()) => return Ok(unsatisfiable(len)),
    };
    let slot = state
        .streams
        .acquire(&device.id)
        .ok_or_else(ApiError::too_many_requests)?;
    let file = tokio::fs::File::from_std(file);
    serve(file, slot, range, len, mime, &etag).await
}

/// Cache policy for video bytes: per user, never re-encoded by proxies.
const CACHE_CONTROL: &str = "private, no-transform";

/// The byte range to serve, `None` for the whole file, or `Err` when the
/// requested range lies outside the file (416). Per RFC 9110 §14.2 a Range
/// the server cannot use (unknown unit, several ranges, bad syntax) is
/// ignored, as is one whose `If-Range` no longer matches the current ETag.
fn usable_range(headers: &HeaderMap, etag: &str, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(raw) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    if let Some(if_range) = headers.get(header::IF_RANGE) {
        // Only a strong ETag can validate a range; dates and weak tags fail.
        if if_range.to_str().ok().map(str::trim) != Some(etag) {
            return Ok(None);
        }
    }
    match parse_range(raw, len) {
        Ok(range) => Ok(Some(range)),
        Err(RangeError::Unsatisfiable) => Err(()),
        Err(RangeError::Invalid) => Ok(None),
    }
}

/// Whether `If-None-Match` matches the current ETag, using the weak
/// comparison RFC 9110 §13.1.2 requires. `*` matches any existing file.
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    let Some(value) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let opaque = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
    let current = opaque(etag);
    value.trim() == "*"
        || value
            .split(',')
            .any(|candidate| opaque(candidate) == current)
}

/// Streams `range` (or the whole file) of `file`; `slot` is held until the
/// body is dropped.
async fn serve(
    mut file: tokio::fs::File,
    slot: StreamSlot,
    range: Option<(u64, u64)>,
    len: u64,
    mime: &str,
    etag: &str,
) -> Result<Response, ApiError> {
    let (start, end) = range.unwrap_or((0, len.saturating_sub(1)));
    let length = if len == 0 { 0 } else { end - start + 1 };
    let body = if length == 0 {
        Body::empty()
    } else {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|_| ApiError::internal())?;
        Body::from_stream(ReaderStream::new(SlotReader::new(file.take(length), slot)))
    };
    let mut builder = Response::builder()
        .status(if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, CACHE_CONTROL);
    if range.is_some() {
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
