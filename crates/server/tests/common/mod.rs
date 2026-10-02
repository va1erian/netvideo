//! Shared test harness: a real router over a temp data dir and library root.

#![allow(dead_code)]

pub mod library;

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use netvideo_server::auth::paseto::{generate_device_keypair, public_key_paserk};
use netvideo_server::config::{Config, LibraryConfig, SecurityConfig, ServerConfig};
use netvideo_server::state::AppState;
use netvideo_server::util::unix_now;
use netvideo_server::{api, auth};
use pasetors::keys::AsymmetricSecretKey;
use pasetors::version4::V4;
use tempfile::TempDir;
use tower::ServiceExt;

pub struct Harness {
    pub state: AppState,
    pub app: Router,
    pub library: PathBuf,
    _dir: TempDir,
}

/// A paired test device.
pub struct Paired {
    pub token: String,
    pub device_id: String,
    pub secret: AsymmetricSecretKey<V4>,
}

impl Harness {
    pub fn new() -> Self {
        Self::build(SecurityConfig::default(), "ffprobe".into())
    }

    pub fn with_security(security: SecurityConfig) -> Self {
        Self::build(security, "ffprobe".into())
    }

    /// A harness whose scanner runs `ffprobe` (a missing program leaves
    /// videos indexed but unprobed).
    pub fn with_ffprobe(ffprobe: &Path) -> Self {
        Self::build(SecurityConfig::default(), ffprobe.to_path_buf())
    }

    fn build(security: SecurityConfig, ffprobe_path: PathBuf) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        std::fs::create_dir_all(&library).unwrap();
        let config = Config {
            server: ServerConfig {
                host: "127.0.0.1".into(),
                port: 0,
                data_dir: dir.path().join("data"),
                trusted_proxies: vec![],
            },
            security,
            library: LibraryConfig {
                paths: vec![library.clone()],
                scan_interval_secs: 0,
                ffprobe_path,
            },
        };
        let state = netvideo_server::build_state(config).unwrap();
        // Canonical form, so tests can compare against resolved paths.
        let library = library.canonicalize().unwrap();
        Self {
            app: api::router(state.clone()),
            state,
            library,
            _dir: dir,
        }
    }

    pub async fn request(&self, request: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
        let response = self.app.clone().oneshot(request).await.expect("request");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, body)
    }

    pub fn code(&self, grants_admin: bool) -> String {
        auth::pairing::generate_pairing_code(
            &self.state.db,
            &self.state.keys,
            600,
            grants_admin,
            unix_now(),
        )
        .unwrap()
    }

    /// Redeems `code` with a fresh device key and returns the device.
    pub async fn redeem(&self, code: &str) -> Paired {
        let (secret, public) = generate_device_keypair().unwrap();
        let body = serde_json::json!({
            "pairing_code": code,
            "device_name": "test device",
            "public_key": public_key_paserk(&public).unwrap(),
        });
        let (status, _, response) = self
            .request(json_request("POST", "/api/v1/auth/pair", &body))
            .await;
        assert_eq!(status, StatusCode::OK, "pairing should succeed");
        let value: serde_json::Value = serde_json::from_slice(&response).unwrap();
        Paired {
            token: value["auth_token"].as_str().unwrap().to_string(),
            device_id: value["device_id"].as_str().unwrap().to_string(),
            secret,
        }
    }

    pub async fn pair_admin(&self) -> Paired {
        self.redeem(&self.code(true)).await
    }

    pub async fn pair_viewer(&self) -> Paired {
        self.redeem(&self.code(false)).await
    }

    pub fn authed(&self, method: &str, uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    pub fn authed_json(
        &self,
        method: &str,
        uri: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }
}

pub fn json_request(method: &str, uri: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

pub fn json(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body).unwrap()
}
