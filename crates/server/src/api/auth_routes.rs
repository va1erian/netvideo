//! Authentication, pairing and device-management endpoints.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::json::ApiJson;
use crate::audit;
use crate::auth::middleware::{AdminDevice, AuthDevice, ClientIp, bearer_token};
use crate::auth::pairing;
use crate::auth::paseto::{issue_access_token, token_fingerprint, verify_refresh_proof};
use crate::db::devices::parse_device_public_key;
use crate::db::models::Device;
use crate::security::{Claim, client_key};
use crate::state::AppState;
use crate::util::unix_now;

/// Pairing-code minting attempts allowed per minute, per client IP.
const PAIRING_CODE_ATTEMPT_LIMIT: u32 = 10;

/// Failed pairing attempts allowed per minute across all clients. Bounds a
/// distributed guesser: over a code's 600 s lifetime that is at most 300 of
/// the 1,000,000 possible codes.
const GLOBAL_PAIR_FAILURE_LIMIT: u32 = 30;

/// Rate-limiter key for the global failed-pairing budget.
const GLOBAL_PAIR_FAILURE_KEY: &str = "pair-failures:global";

/// Pairing request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    /// The one-time code shown by the server CLI.
    pub pairing_code: String,
    /// A human-readable name for the device.
    pub device_name: String,
    /// The device's PASERK-encoded Ed25519 public key.
    pub public_key: String,
}

/// Successful pairing response.
#[derive(Debug, Serialize)]
pub struct PairResponse {
    /// Server-assigned device id.
    pub device_id: String,
    /// Device name as registered.
    pub device_name: String,
    /// First access token.
    pub auth_token: String,
    /// Token expiry (Unix seconds).
    pub expires_at: i64,
    /// The server's PASERK public key: clients pin it and check that every
    /// token they receive is signed by it.
    pub server_key: String,
}

/// Refresh request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    /// A device-signed PASETO bound to the presented access token.
    pub proof: String,
}

/// Token response shared by pairing and refresh.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// The access token.
    pub auth_token: String,
    /// Expiry (Unix seconds).
    pub expires_at: i64,
}

/// A device as returned by the management endpoints (no public key).
#[derive(Debug, Serialize)]
pub struct DeviceView {
    /// Device id.
    pub id: String,
    /// Device name.
    pub name: String,
    /// Pairing time (Unix seconds).
    pub paired_at: i64,
    /// Last authenticated request (Unix seconds).
    pub last_seen: Option<i64>,
    /// Whether the device is revoked.
    pub is_revoked: bool,
    /// Whether the device is an administrator.
    pub is_admin: bool,
}

impl From<Device> for DeviceView {
    fn from(device: Device) -> Self {
        Self {
            id: device.id,
            name: device.name,
            paired_at: device.paired_at,
            last_seen: device.last_seen,
            is_revoked: device.is_revoked,
            is_admin: device.is_admin,
        }
    }
}

/// Optional body of a pairing-code request.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingCodeRequest {
    /// Whether the device redeeming the code becomes an administrator.
    #[serde(default)]
    pub admin: bool,
}

/// Response for a freshly generated pairing code.
#[derive(Debug, Serialize)]
pub struct PairingCodeResponse {
    /// The six-digit code.
    pub pairing_code: String,
    /// How long it stays valid, in seconds.
    pub expires_in_secs: u64,
}

/// `POST /api/v1/auth/pair`
pub async fn pair(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(request): ApiJson<PairRequest>,
) -> Result<Json<PairResponse>, ApiError> {
    let ip_text = ip.to_string();
    let window = Duration::from_secs(60);
    // Every attempt is charged to the global budget before it runs and
    // refunded if it succeeds or never runs, so concurrent guesses cannot all
    // slip past a budget that still looks open.
    let Some(charged) =
        state
            .rate
            .charge(GLOBAL_PAIR_FAILURE_KEY, GLOBAL_PAIR_FAILURE_LIMIT, window)
    else {
        audit::rate_limited(&ip_text, "pair-global");
        return Err(ApiError::too_many_requests());
    };
    if !state.rate.check(
        &format!("pair:{}", client_key(ip)),
        state.pairing_attempt_limit(),
        window,
    ) {
        state.rate.refund(GLOBAL_PAIR_FAILURE_KEY, charged);
        audit::rate_limited(&ip_text, "pair");
        return Err(ApiError::too_many_requests());
    }

    let server_key = state.keys.public_paserk().map_err(ApiError::from)?;
    let db = state.db.clone();
    let keys = Arc::clone(&state.keys);
    let ttl = state.token_ttl();
    let outcome = tokio::task::spawn_blocking(move || {
        pairing::pair(
            &db,
            &keys,
            ttl,
            &request.pairing_code,
            &request.device_name,
            &request.public_key,
            unix_now(),
        )
    })
    .await
    .map_err(|_| ApiError::internal())?;

    match outcome {
        Ok(outcome) => {
            state.rate.refund(GLOBAL_PAIR_FAILURE_KEY, charged);
            audit::device_paired(&ip_text, &outcome.device.id, &outcome.device.name);
            Ok(Json(PairResponse {
                device_id: outcome.device.id,
                device_name: outcome.device.name,
                auth_token: outcome.token.token,
                expires_at: outcome.token.expires_at,
                server_key,
            }))
        }
        Err(error) => {
            audit::pair_failed(&ip_text);
            Err(error.into())
        }
    }
}

/// `POST /api/v1/auth/refresh`
pub async fn refresh(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AuthDevice(device): AuthDevice,
    headers: HeaderMap,
    ApiJson(request): ApiJson<RefreshRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let ip_text = ip.to_string();
    let token =
        bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
    let fingerprint = token_fingerprint(token);

    let public = parse_device_public_key(&device.public_key)?;
    let proof = match verify_refresh_proof(&public, &request.proof, &fingerprint) {
        Ok(proof) => proof,
        Err(error) => {
            audit::auth_failed(&ip_text, "invalid refresh proof");
            return Err(error.into());
        }
    };
    match state
        .proofs
        .claim(&device.id, &proof.proof_id, proof.expires_at, unix_now())
    {
        Claim::Accepted => {}
        Claim::Replayed => {
            audit::auth_failed(&ip_text, "replayed refresh proof");
            return Err(ApiError::unauthorized("refresh proof already used"));
        }
        Claim::Full => {
            audit::rate_limited(&ip_text, "refresh");
            return Err(ApiError::too_many_requests());
        }
    }

    let issued = issue_access_token(&state.keys, &device.id, state.token_ttl())?;
    audit::token_refreshed(&ip_text, &device.id);
    Ok(Json(TokenResponse {
        auth_token: issued.token,
        expires_at: issued.expires_at,
    }))
}

/// `GET /api/v1/devices` (admin only)
pub async fn list_devices(
    State(state): State<AppState>,
    AdminDevice(_): AdminDevice,
) -> Result<Json<Vec<DeviceView>>, ApiError> {
    let db = state.db.clone();
    let devices = tokio::task::spawn_blocking(move || db.list_devices())
        .await
        .map_err(|_| ApiError::internal())??;
    Ok(Json(devices.into_iter().map(DeviceView::from).collect()))
}

/// `POST /api/v1/devices/pairing-codes`
///
/// Admin only. Codes mint viewer devices unless the body asks for
/// `{"admin": true}`.
pub async fn create_pairing_code(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AdminDevice(admin): AdminDevice,
    request: Option<Json<PairingCodeRequest>>,
) -> Result<Json<PairingCodeResponse>, ApiError> {
    let grants_admin = request.is_some_and(|Json(request)| request.admin);
    let ip_text = ip.to_string();
    if !state.rate.check(
        &format!("paircode:{}", client_key(ip)),
        PAIRING_CODE_ATTEMPT_LIMIT,
        Duration::from_secs(60),
    ) {
        audit::rate_limited(&ip_text, "pairing-code");
        return Err(ApiError::too_many_requests());
    }

    let db = state.db.clone();
    let keys = Arc::clone(&state.keys);
    let ttl = state.config.security.pairing_code_ttl_secs;
    let code = tokio::task::spawn_blocking(move || {
        pairing::generate_pairing_code(&db, &keys, ttl, grants_admin, unix_now())
    })
    .await
    .map_err(|_| ApiError::internal())??;
    audit::pairing_code_created(&ip_text, Some(&admin.id), grants_admin);
    Ok(Json(PairingCodeResponse {
        pairing_code: code,
        expires_in_secs: ttl,
    }))
}

/// `DELETE /api/v1/devices/{id}` (admin only)
pub async fn revoke(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AdminDevice(admin): AdminDevice,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let db = state.db.clone();
    let target = id.clone();
    let revoked = tokio::task::spawn_blocking(move || db.revoke_device(&target))
        .await
        .map_err(|_| ApiError::internal())??;
    if !revoked {
        return Err(ApiError::not_found());
    }
    audit::device_revoked(&ip.to_string(), Some(&admin.id), &id);
    Ok(StatusCode::NO_CONTENT)
}
