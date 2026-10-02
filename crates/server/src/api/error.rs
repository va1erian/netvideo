//! HTTP error mapping.
//!
//! Internal failures return a generic message so implementation details are
//! never leaked; security-relevant rejections are audited where they happen.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::ServerError;

/// An error rendered as a JSON HTTP response.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    /// Builds an unauthorized (401) response.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    /// Builds a bad-request (400) response.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    /// Builds a forbidden (403) response.
    pub fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: "forbidden".into(),
        }
    }

    /// Builds a not-found (404) response.
    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: "not found".into(),
        }
    }

    /// Builds a conflict (409) response.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    /// Builds a too-many-requests (429) response.
    pub fn too_many_requests() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "too many requests".into(),
        }
    }

    /// Builds an internal (500) response without exposing details.
    pub fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal error".into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<ServerError> for ApiError {
    fn from(error: ServerError) -> Self {
        match error {
            ServerError::Unauthorized(message) => Self::unauthorized(message),
            ServerError::InvalidPairingCode => Self::unauthorized("invalid pairing code"),
            // Path violations are never disclosed to the client; the real
            // event is written to the audit log by the caller.
            ServerError::PathRejected(_) | ServerError::PathEscape { .. } => Self::not_found(),
            ServerError::Conflict(_) => Self::conflict("conflict"),
            other => {
                tracing::error!(%other, "request failed");
                Self::internal()
            }
        }
    }
}
