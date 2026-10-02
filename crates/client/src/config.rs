//! A configured server endpoint.
//!
//! The [`id`](ServerEndpoint::id) is a stable hash of the normalized URL, so
//! credentials can be stored per server without the caller inventing ids.

use serde::{Deserialize, Serialize};

use crate::error::{ClientError, Result};
use crate::util::sha256_hex;

/// A netvideo server the client can talk to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEndpoint {
    /// Stable id derived from the URL.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Normalized base URL, without a trailing slash.
    pub url: String,
}

impl ServerEndpoint {
    /// Builds an endpoint, normalizing and validating the URL.
    pub fn new(name: impl Into<String>, url: &str) -> Result<Self> {
        let url = normalize_url(url)?;
        let id = endpoint_id(&url);
        Ok(Self {
            id,
            name: name.into(),
            url,
        })
    }
}

/// Normalizes a base URL: trims whitespace, requires an `http`/`https` scheme
/// and removes any trailing slash.
pub fn normalize_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(ClientError::Url("empty URL".into()));
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(ClientError::Url(format!(
            "{trimmed:?} must start with http:// or https://"
        )));
    }
    let without_trailing = trimmed.trim_end_matches('/').to_string();
    if without_trailing.len() <= "https://".len() {
        return Err(ClientError::Url(format!("{trimmed:?} has no host")));
    }
    if trimmed.contains(['?', '#']) {
        return Err(ClientError::Url(format!(
            "{trimmed:?} must not have a query or fragment"
        )));
    }
    if trimmed.contains(char::is_whitespace) {
        return Err(ClientError::Url(format!("{trimmed:?} contains whitespace")));
    }
    Ok(without_trailing)
}

/// The stable id for a normalized URL.
fn endpoint_id(url: &str) -> String {
    sha256_hex(b"netvideo/endpoint/v1:", url.as_bytes())[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_strips_trailing_slash() {
        assert_eq!(
            normalize_url("https://video.example.com/").unwrap(),
            "https://video.example.com"
        );
        assert_eq!(
            normalize_url("  http://10.0.0.2:8080  ").unwrap(),
            "http://10.0.0.2:8080"
        );
    }

    #[test]
    fn rejects_scheme_less_and_empty_urls() {
        assert!(normalize_url("video.example.com").is_err());
        assert!(normalize_url("").is_err());
        assert!(normalize_url("https://").is_err());
        assert!(normalize_url("https://bad host").is_err());
        assert!(normalize_url("https://video.example.com/?x=1").is_err());
        assert!(normalize_url("https://video.example.com/#top").is_err());
    }

    #[test]
    fn endpoint_ids_are_stable_and_url_specific() {
        let a = ServerEndpoint::new("Home", "https://video.example.com/").unwrap();
        let b = ServerEndpoint::new("Home", "https://video.example.com").unwrap();
        assert_eq!(a.id, b.id);
        let c = ServerEndpoint::new("Other", "https://other.example.com").unwrap();
        assert_ne!(a.id, c.id);
        assert_eq!(a.id.len(), 16);
    }
}
