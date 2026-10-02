//! The blocking HTTP client for the server REST API.
//!
//! One [`RemoteClient`] per server; it owns a `ureq` agent with bounded
//! timeouts, so a stalled server fails fast. Non-2xx responses are parsed
//! for the server's `{"error": ...}` body, and response bodies are capped
//! to bound memory. Video bytes are not fetched here: players stream
//! [`RemoteClient::file_url`] themselves, with the bearer token as a header.

use std::io::Read;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::json;

use crate::config::{ServerEndpoint, normalize_url};
use crate::error::{ClientError, Result};
use crate::types::{
    ApiErrorBody, FolderPage, FolderRef, Health, PairResponse, TokenResponse, VideoDetail,
};

/// Maximum bytes buffered for a JSON response (a 1000-entry folder page).
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
/// Maximum bytes buffered for an error body.
const MAX_ERROR_BYTES: usize = 64 * 1024;

/// A client bound to one server base URL.
#[derive(Debug, Clone)]
pub struct RemoteClient {
    base: String,
    agent: ureq::Agent,
}

impl RemoteClient {
    /// Builds a client for a base URL such as `https://video.example.com`.
    pub fn new(base: &str) -> Result<Self> {
        let base = normalize_url(base)?;
        let config = ureq::Agent::config_builder()
            // Read the status ourselves so we can surface the JSON error body.
            .http_status_as_error(false)
            .timeout_resolve(Some(Duration::from_secs(10)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .build();
        Ok(Self {
            base,
            agent: ureq::Agent::new_with_config(config),
        })
    }

    /// Builds a client from a configured endpoint.
    pub fn from_endpoint(endpoint: &ServerEndpoint) -> Result<Self> {
        Self::new(&endpoint.url)
    }

    /// The normalized base URL.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn bearer<B>(request: ureq::RequestBuilder<B>, token: &str) -> ureq::RequestBuilder<B> {
        request.header("Authorization", format!("Bearer {token}"))
    }

    fn decode<T: DeserializeOwned>(response: http::Response<ureq::Body>) -> Result<T> {
        let status = response.status().as_u16();
        if status == 401 {
            return Err(ClientError::Unauthorized);
        }
        if !(200..300).contains(&status) {
            return Err(ClientError::Http {
                status,
                message: error_message(response),
            });
        }
        let bytes = read_capped(response, MAX_JSON_BYTES)?;
        serde_json::from_slice(&bytes).map_err(|error| ClientError::Protocol(error.to_string()))
    }

    /// `GET /api/v1/health`.
    pub fn health(&self) -> Result<Health> {
        let response = self
            .agent
            .get(self.url("/api/v1/health"))
            .call()
            .map_err(map_transport)?;
        Self::decode(response)
    }

    /// `POST /api/v1/auth/pair`.
    pub fn pair(
        &self,
        pairing_code: &str,
        device_name: &str,
        public_key: &str,
    ) -> Result<PairResponse> {
        let body = json!({
            "pairing_code": pairing_code,
            "device_name": device_name,
            "public_key": public_key,
        });
        let response = self
            .agent
            .post(self.url("/api/v1/auth/pair"))
            .send_json(&body)
            .map_err(map_transport)?;
        // A 401 here means the pairing code was wrong or expired, not a
        // revoked device.
        if response.status().as_u16() == 401 {
            return Err(ClientError::PairingCode);
        }
        Self::decode(response)
    }

    /// `POST /api/v1/auth/refresh`.
    pub fn refresh(&self, token: &str, proof: &str) -> Result<TokenResponse> {
        let body = json!({ "proof": proof });
        let response = Self::bearer(self.agent.post(self.url("/api/v1/auth/refresh")), token)
            .send_json(&body)
            .map_err(map_transport)?;
        Self::decode(response)
    }

    /// `GET /api/v1/roots`.
    pub fn roots(&self, token: &str) -> Result<Vec<FolderRef>> {
        let response = Self::bearer(self.agent.get(self.url("/api/v1/roots")), token)
            .call()
            .map_err(map_transport)?;
        Self::decode(response)
    }

    /// `GET /api/v1/folders/{id}`, one page from `cursor` (the start when
    /// `None`) of at most `limit` entries.
    pub fn folder(
        &self,
        token: &str,
        folder_id: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<FolderPage> {
        let mut url = self.url(&format!(
            "/api/v1/folders/{}?limit={limit}",
            encode_segment(folder_id)
        ));
        if let Some(cursor) = cursor {
            url.push_str("&cursor=");
            url.push_str(&encode_segment(cursor));
        }
        let response = Self::bearer(self.agent.get(url), token)
            .call()
            .map_err(map_transport)?;
        Self::decode(response)
    }

    /// `GET /api/v1/videos/{id}`.
    pub fn video(&self, token: &str, video_id: &str) -> Result<VideoDetail> {
        let url = self.url(&format!("/api/v1/videos/{}", encode_segment(video_id)));
        let response = Self::bearer(self.agent.get(url), token)
            .call()
            .map_err(map_transport)?;
        Self::decode(response)
    }

    /// `PUT /api/v1/videos/{id}/progress`.
    pub fn save_progress(
        &self,
        token: &str,
        video_id: &str,
        position_ms: i64,
        watched: bool,
    ) -> Result<()> {
        let url = self.url(&format!(
            "/api/v1/videos/{}/progress",
            encode_segment(video_id)
        ));
        let body = json!({ "position_ms": position_ms, "watched": watched });
        let response = Self::bearer(self.agent.put(url), token)
            .send_json(&body)
            .map_err(map_transport)?;
        Self::expect_success(response)
    }

    /// The direct-play URL of a video. Requests to it need the
    /// `Authorization: Bearer <token>` header; tokens never go in URLs.
    pub fn file_url(&self, video_id: &str) -> String {
        self.url(&format!("/api/v1/videos/{}/file", encode_segment(video_id)))
    }

    /// `DELETE /api/v1/devices/{id}`: revoke a device (typically this one).
    pub fn revoke_device(&self, token: &str, device_id: &str) -> Result<()> {
        let url = self.url(&format!("/api/v1/devices/{}", encode_segment(device_id)));
        let response = Self::bearer(self.agent.delete(url), token)
            .call()
            .map_err(map_transport)?;
        Self::expect_success(response)
    }

    fn expect_success(response: http::Response<ureq::Body>) -> Result<()> {
        let status = response.status().as_u16();
        if status == 401 {
            return Err(ClientError::Unauthorized);
        }
        if !(200..300).contains(&status) {
            return Err(ClientError::Http {
                status,
                message: error_message(response),
            });
        }
        Ok(())
    }
}

/// Reads at most `max` bytes of a response body.
fn read_capped(response: http::Response<ureq::Body>, max: usize) -> Result<Vec<u8>> {
    let reader = response.into_body().into_reader();
    let mut buffer = Vec::new();
    let read = reader
        .take(max as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|error| ClientError::Network(error.to_string()))?;
    if read > max {
        return Err(ClientError::Protocol(format!(
            "server response exceeds {max} bytes"
        )));
    }
    Ok(buffer)
}

/// Extracts the server's error message, falling back to the status text.
fn error_message(response: http::Response<ureq::Body>) -> String {
    let status = response.status();
    match read_capped(response, MAX_ERROR_BYTES)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ApiErrorBody>(&bytes).ok())
    {
        Some(body) => body.error,
        None => format!("HTTP {status}"),
    }
}

/// Percent-encodes a value for use as a single URL path segment.
fn encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

/// Maps a transport-level `ureq` error.
fn map_transport(error: ureq::Error) -> ClientError {
    match error {
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
            ClientError::Network("cannot reach the server".into())
        }
        other => ClientError::Network(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_path_segments() {
        assert_eq!(encode_segment("abc123"), "abc123");
        assert_eq!(encode_segment("a/b"), "a%2Fb");
        assert_eq!(encode_segment("a b#c"), "a%20b%23c");
    }
}
