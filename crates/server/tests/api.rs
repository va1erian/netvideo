//! End-to-end API tests for pairing, refresh, revocation, device roles and
//! request limits, all exercised through the real Axum router.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use netvideo_server::auth::paseto::{
    generate_device_keypair, issue_refresh_proof, public_key_paserk, token_fingerprint,
};
use netvideo_server::config::{Config, LibraryConfig, SecurityConfig, ServerConfig};
use netvideo_server::state::AppState;
use netvideo_server::util::unix_now;
use netvideo_server::{api, auth};
use pasetors::keys::AsymmetricSecretKey;
use pasetors::version4::V4;
use tempfile::TempDir;
use tower::ServiceExt;

struct Harness {
    state: AppState,
    app: Router,
    _dir: TempDir,
}

/// A paired test device.
struct Paired {
    token: String,
    device_id: String,
    secret: AsymmetricSecretKey<V4>,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            server: ServerConfig {
                host: "127.0.0.1".into(),
                port: 0,
                data_dir: dir.path().join("data"),
                trusted_proxies: vec![],
            },
            security: SecurityConfig::default(),
            library: LibraryConfig {
                paths: vec![root],
                scan_interval_secs: 0,
            },
        };
        let state = netvideo_server::build_state(config).unwrap();
        Self {
            app: api::router(state.clone()),
            state,
            _dir: dir,
        }
    }

    async fn request(&self, request: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
        let response = self.app.clone().oneshot(request).await.expect("request");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, body)
    }

    fn code(&self, grants_admin: bool) -> String {
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
    async fn redeem(&self, code: &str) -> Paired {
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

    async fn pair_admin(&self) -> Paired {
        self.redeem(&self.code(true)).await
    }

    async fn pair_viewer(&self) -> Paired {
        self.redeem(&self.code(false)).await
    }

    fn authed(&self, method: &str, uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    fn authed_json(
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

fn json_request(method: &str, uri: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn json(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body).unwrap()
}

#[tokio::test]
async fn health_is_public() {
    let harness = Harness::new();
    let (status, _, body) = harness
        .request(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["status"], "ok");
}

#[tokio::test]
async fn protected_endpoints_require_a_token() {
    let harness = Harness::new();
    let (status, _, _) = harness
        .request(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pairing_rejects_a_wrong_code() {
    let harness = Harness::new();
    let real = harness.code(true);
    let wrong = if real == "111111" { "222222" } else { "111111" };
    let (_, public) = generate_device_keypair().unwrap();
    let body = serde_json::json!({
        "pairing_code": wrong,
        "device_name": "intruder",
        "public_key": public_key_paserk(&public).unwrap(),
    });
    let (status, _, _) = harness
        .request(json_request("POST", "/api/v1/auth/pair", &body))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pairing_is_rate_limited_per_ip() {
    let harness = Harness::new();
    for attempt in 0..4 {
        let code = harness.code(false);
        let (_, public) = generate_device_keypair().unwrap();
        let body = serde_json::json!({
            "pairing_code": code,
            "device_name": "device",
            "public_key": public_key_paserk(&public).unwrap(),
        });
        let (status, _, _) = harness
            .request(json_request("POST", "/api/v1/auth/pair", &body))
            .await;
        if attempt < 3 {
            assert_eq!(status, StatusCode::OK, "attempt {attempt}");
        } else {
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "attempt {attempt}");
        }
    }
}

#[tokio::test]
async fn a_pairing_code_cannot_be_reused() {
    let harness = Harness::new();
    let code = harness.code(false);
    let (_, public) = generate_device_keypair().unwrap();
    let public = public_key_paserk(&public).unwrap();

    let first = serde_json::json!({
        "pairing_code": code, "device_name": "one", "public_key": public,
    });
    let (status, _, _) = harness
        .request(json_request("POST", "/api/v1/auth/pair", &first))
        .await;
    assert_eq!(status, StatusCode::OK);

    let second = serde_json::json!({
        "pairing_code": code, "device_name": "two", "public_key": public,
    });
    let (status, _, _) = harness
        .request(json_request("POST", "/api/v1/auth/pair", &second))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_expired_pairing_code_is_rejected() {
    let harness = Harness::new();
    let now = unix_now();
    let hash = auth::pairing::pairing_code_hash(&harness.state.keys, "123456");
    harness
        .state
        .db
        .insert_pairing_code(&hash, now - 700, now - 100, true)
        .unwrap();
    let (_, public) = generate_device_keypair().unwrap();
    let body = serde_json::json!({
        "pairing_code": "123456",
        "device_name": "late",
        "public_key": public_key_paserk(&public).unwrap(),
    });
    let (status, _, _) = harness
        .request(json_request("POST", "/api/v1/auth/pair", &body))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refresh_requires_a_valid_device_proof() {
    let harness = Harness::new();
    let device = harness.pair_viewer().await;
    let fingerprint = token_fingerprint(&device.token);
    let proof = issue_refresh_proof(
        &device.secret,
        &fingerprint,
        std::time::Duration::from_secs(120),
        std::time::Duration::from_secs(30),
    )
    .unwrap();

    let body = serde_json::json!({ "proof": proof });
    let request = harness.authed_json("POST", "/api/v1/auth/refresh", &device.token, &body);
    let (status, _, response) = harness.request(request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        json(&response)["auth_token"]
            .as_str()
            .unwrap()
            .starts_with("v4.public.")
    );

    let bad = serde_json::json!({ "proof": "not-a-token" });
    let request = harness.authed_json("POST", "/api/v1/auth/refresh", &device.token, &bad);
    let (status, _, _) = harness.request(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_devices_are_locked_out() {
    let harness = Harness::new();
    let admin = harness.pair_admin().await;
    assert!(harness.state.db.revoke_device(&admin.device_id).unwrap());
    let (status, _, _) = harness
        .request(harness.authed("GET", "/api/v1/devices", &admin.token))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn viewers_cannot_manage_devices() {
    let harness = Harness::new();
    let admin = harness.pair_admin().await;
    let viewer = harness.pair_viewer().await;

    let (status, _, _) = harness
        .request(harness.authed("GET", "/api/v1/devices", &viewer.token))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = harness
        .request(harness.authed("POST", "/api/v1/devices/pairing-codes", &viewer.token))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let uri = format!("/api/v1/devices/{}", admin.device_id);
    let (status, _, _) = harness
        .request(harness.authed("DELETE", &uri, &viewer.token))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        harness
            .state
            .db
            .device_by_id(&admin.device_id)
            .unwrap()
            .is_some_and(|device| !device.is_revoked),
        "a viewer must not be able to revoke the admin"
    );
}

#[tokio::test]
async fn admins_list_and_revoke_devices() {
    let harness = Harness::new();
    let admin = harness.pair_admin().await;
    let viewer = harness.pair_viewer().await;

    let (status, _, body) = harness
        .request(harness.authed("GET", "/api/v1/devices", &admin.token))
        .await;
    assert_eq!(status, StatusCode::OK);
    let devices = json(&body);
    let devices = devices.as_array().unwrap();
    assert_eq!(devices.len(), 2);
    assert!(
        devices
            .iter()
            .all(|device| device.get("public_key").is_none()),
        "public keys are never listed"
    );
    let viewer_row = devices
        .iter()
        .find(|device| device["id"] == viewer.device_id.as_str())
        .unwrap();
    assert_eq!(viewer_row["is_admin"], false);

    let uri = format!("/api/v1/devices/{}", viewer.device_id);
    let (status, _, _) = harness
        .request(harness.authed("DELETE", &uri, &admin.token))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = harness
        .request(harness.authed("GET", "/api/v1/devices", &viewer.token))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn device_minted_codes_pair_viewers_unless_admin_is_requested() {
    let harness = Harness::new();
    let admin = harness.pair_admin().await;

    let (status, _, body) = harness
        .request(harness.authed("POST", "/api/v1/devices/pairing-codes", &admin.token))
        .await;
    assert_eq!(status, StatusCode::OK);
    let code = json(&body)["pairing_code"].as_str().unwrap().to_string();
    let viewer = harness.redeem(&code).await;
    let (status, _, _) = harness
        .request(harness.authed("GET", "/api/v1/devices", &viewer.token))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let request = harness.authed_json(
        "POST",
        "/api/v1/devices/pairing-codes",
        &admin.token,
        &serde_json::json!({ "admin": true }),
    );
    let (status, _, body) = harness.request(request).await;
    assert_eq!(status, StatusCode::OK);
    let code = json(&body)["pairing_code"].as_str().unwrap().to_string();
    let second_admin = harness.redeem(&code).await;
    let (status, _, _) = harness
        .request(harness.authed("GET", "/api/v1/devices", &second_admin.token))
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_client_cannot_claim_admin_while_pairing() {
    let harness = Harness::new();
    let code = harness.code(false);
    let (_, public) = generate_device_keypair().unwrap();
    let body = serde_json::json!({
        "pairing_code": code,
        "device_name": "sneaky",
        "public_key": public_key_paserk(&public).unwrap(),
        "is_admin": true,
    });
    let (status, _, response) = harness
        .request(json_request("POST", "/api/v1/auth/pair", &body))
        .await;
    // Unknown fields are ignored or refused; either way no admin is created.
    if status == StatusCode::OK {
        let id = json(&response)["device_id"].as_str().unwrap().to_string();
        let device = harness.state.db.device_by_id(&id).unwrap().unwrap();
        assert!(!device.is_admin);
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn oversized_bodies_are_rejected() {
    let harness = Harness::new();
    let huge = "x".repeat(200 * 1024);
    let body = serde_json::json!({
        "pairing_code": "123456",
        "device_name": huge,
        "public_key": "k4.public.AAAA",
    });
    let request = json_request("POST", "/api/v1/auth/pair", &body);
    let (status, _, _) = harness.request(request).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
