//! HTTP API: router, health check and shared error type.

pub mod auth_routes;
pub mod error;
pub mod library_routes;
pub mod range;
pub mod stream_routes;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::{delete, get, post};
use serde_json::{Value, json};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Builds the complete application router.
pub fn router(state: AppState) -> Router {
    let max_body = state.config.security.max_body_bytes;
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/auth/pair", post(auth_routes::pair))
        .route("/api/v1/auth/refresh", post(auth_routes::refresh))
        .route("/api/v1/devices", get(auth_routes::list_devices))
        .route(
            "/api/v1/devices/pairing-codes",
            post(auth_routes::create_pairing_code),
        )
        .route("/api/v1/devices/{id}", delete(auth_routes::revoke))
        .route("/api/v1/roots", get(library_routes::roots))
        .route("/api/v1/folders/{id}", get(library_routes::folder))
        .route("/api/v1/videos/{id}", get(library_routes::video))
        .route("/api/v1/videos/{id}/file", get(stream_routes::file))
        .route("/api/v1/library/scan", post(library_routes::scan))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(max_body))
        .with_state(state)
}

/// `GET /api/v1/health` — unauthenticated liveness probe. Deliberately
/// exposes no library state: it only confirms the process is serving.
async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "started_at": state.started_at,
    }))
}
