//! A JSON body extractor whose rejections use the API's error format.

use axum::Json;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

use crate::api::error::ApiError;

/// Like [`axum::Json`], but a malformed or mistyped body is answered with
/// a JSON 400 (`{"error": ...}`) instead of axum's plain-text rejection.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            // An oversized body keeps its 413; everything else is a 400.
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                Err(ApiError::payload_too_large())
            }
            Err(_) => Err(ApiError::bad_request("invalid request body")),
        }
    }
}
