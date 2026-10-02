//! PASETO v4.public token issuing and verification.
//!
//! The server signs short-lived access tokens with its own Ed25519 key. A
//! device proves possession of its private key by signing a *refresh proof*
//! over the token it wants to renew, which the server verifies with the
//! device's stored public key. A stolen access token alone therefore cannot be
//! refreshed indefinitely.

use std::convert::TryFrom;
use std::time::Duration;

use pasetors::claims::{Claims, ClaimsValidationRules};
use pasetors::keys::{AsymmetricPublicKey, AsymmetricSecretKey};
use pasetors::token::UntrustedToken;
use pasetors::version4::V4;
use pasetors::{Public, public};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::auth::keys::ServerKey;
use crate::error::{Result, ServerError};

/// Custom claim carrying the SHA-256 of the token being refreshed.
pub const REFRESH_FOR_CLAIM: &str = "refresh_for";

/// Longest a refresh proof may stay valid, counted from when the server
/// checks it. Bounds how long a captured proof matters at all.
pub const MAX_REFRESH_PROOF_LIFETIME: Duration = Duration::from_secs(600);

/// Clock skew allowed on top of [`MAX_REFRESH_PROOF_LIFETIME`] between a
/// proof's `iat` and `exp`, since clients backdate `iat`.
pub const MAX_REFRESH_PROOF_SKEW: Duration = Duration::from_secs(300);

/// Custom claim carrying the fingerprint of the device key a token was
/// issued for, so a client can tell a token minted for its own key from one
/// a man-in-the-middle obtained for a substituted key.
pub const DEVICE_KEY_CLAIM: &str = "device_key";

/// Access-token scope claim value.
pub const SCOPE_LIBRARY: &str = "library:read stream:read";

/// An issued access token and its absolute expiry (Unix seconds).
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedToken {
    /// The PASETO token string.
    pub token: String,
    /// Expiry as a Unix timestamp in seconds.
    pub expires_at: i64,
}

impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Verified properties of an access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAccess {
    /// The device (`sub`) the token was issued to.
    pub device_id: String,
    /// Unique token identifier (`jti`).
    pub token_id: String,
    /// Expiry as a Unix timestamp in seconds.
    pub expires_at: i64,
}

/// The fingerprint of a device's PASERK `k4.public` key, as carried in
/// [`DEVICE_KEY_CLAIM`]. Clients compute the same value.
pub fn device_key_fingerprint(public_paserk: &str) -> String {
    crate::util::sha256_hex(b"netvideo/device-key/v1:", public_paserk.as_bytes())
}

/// Issues an access token for `device_id`, whose stored PASERK public key is
/// `device_key`, valid for `ttl`.
pub fn issue_access_token(
    key: &ServerKey,
    device_id: &str,
    device_key: &str,
    ttl: Duration,
) -> Result<IssuedToken> {
    let mut claims =
        Claims::new_expires_in(&ttl).map_err(|error| ServerError::Token(error.to_string()))?;
    claims
        .subject(device_id)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    claims
        .token_identifier(&uuid::Uuid::new_v4().to_string())
        .map_err(|error| ServerError::Token(error.to_string()))?;
    claims
        .add_additional("scope", SCOPE_LIBRARY)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    claims
        .add_additional(DEVICE_KEY_CLAIM, device_key_fingerprint(device_key))
        .map_err(|error| ServerError::Token(error.to_string()))?;

    let token = public::sign(key.secret(), &claims, None, None)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    let expires_at = unix_now().saturating_add(ttl.as_secs() as i64);
    Ok(IssuedToken { token, expires_at })
}

/// Verifies an access token's signature, expiry and required claims.
pub fn verify_access_token(key: &ServerKey, token: &str) -> Result<VerifiedAccess> {
    let untrusted = UntrustedToken::<Public, V4>::try_from(token)
        .map_err(|_| ServerError::Unauthorized("malformed token".into()))?;
    let rules = ClaimsValidationRules::new();
    let trusted = public::verify(key.public(), &untrusted, &rules, None, None)
        .map_err(|_| ServerError::Unauthorized("invalid or expired token".into()))?;
    let claims = trusted
        .payload_claims()
        .ok_or_else(|| ServerError::Unauthorized("token has no claims".into()))?;

    let device_id = string_claim(claims, "sub")?;
    let token_id = string_claim(claims, "jti")?;
    let expires_at = unix_claim(claims, "exp")?;
    Ok(VerifiedAccess {
        device_id,
        token_id,
        expires_at,
    })
}

/// Builds the client side of a refresh proof: a short-lived device-signed
/// token bound to `token_fingerprint`.
///
/// The client clock is assumed to differ by at most `skew`, so `iat`/`nbf`
/// are backdated by that amount.
pub fn issue_refresh_proof(
    device_secret: &AsymmetricSecretKey<V4>,
    token_fingerprint: &str,
    ttl: Duration,
    skew: Duration,
) -> Result<String> {
    let mut claims =
        Claims::new_expires_in(&ttl).map_err(|error| ServerError::Token(error.to_string()))?;
    backdate_claims(&mut claims, skew)?;
    claims
        .add_additional(REFRESH_FOR_CLAIM, token_fingerprint)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    claims
        .token_identifier(&uuid::Uuid::new_v4().to_string())
        .map_err(|error| ServerError::Token(error.to_string()))?;
    public::sign(device_secret, &claims, None, None)
        .map_err(|error| ServerError::Token(error.to_string()))
}

/// Identity of a verified refresh proof, for replay tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProof {
    /// Unique proof identifier (`jti`).
    pub proof_id: String,
    /// Expiry as a Unix timestamp in seconds.
    pub expires_at: i64,
}

/// Verifies a device's refresh proof, that it is bound to the presented
/// token, and that it expires within [`MAX_REFRESH_PROOF_LIFETIME`].
pub fn verify_refresh_proof(
    device_public: &AsymmetricPublicKey<V4>,
    proof: &str,
    expected_fingerprint: &str,
) -> Result<VerifiedProof> {
    let untrusted = UntrustedToken::<Public, V4>::try_from(proof)
        .map_err(|_| ServerError::Unauthorized("malformed refresh proof".into()))?;
    let rules = ClaimsValidationRules::new();
    let trusted = public::verify(device_public, &untrusted, &rules, None, None)
        .map_err(|_| ServerError::Unauthorized("invalid or expired refresh proof".into()))?;
    let claims = trusted
        .payload_claims()
        .ok_or_else(|| ServerError::Unauthorized("refresh proof has no claims".into()))?;
    let bound = string_claim(claims, REFRESH_FOR_CLAIM)?;
    if !constant_time_eq(bound.as_bytes(), expected_fingerprint.as_bytes()) {
        return Err(ServerError::Unauthorized(
            "refresh proof is not bound to this token".into(),
        ));
    }
    let expires_at = unix_claim(claims, "exp")?;
    let issued_at = unix_claim(claims, "iat")?;
    let max = MAX_REFRESH_PROOF_LIFETIME.as_secs() as i64;
    let skew = MAX_REFRESH_PROOF_SKEW.as_secs() as i64;
    if expires_at > unix_now() + max || expires_at - issued_at > max + skew {
        return Err(ServerError::Unauthorized(
            "refresh proof lives too long".into(),
        ));
    }
    let proof_id = string_claim(claims, "jti")?;
    Ok(VerifiedProof {
        proof_id,
        expires_at,
    })
}

/// Builds the fingerprint a refresh proof must be bound to.
pub fn token_fingerprint(token: &str) -> String {
    crate::util::sha256_hex(b"netvideo/refresh-token/v1:", token.as_bytes())
}

/// Generates a device keypair for pairing or tests.
pub fn generate_device_keypair() -> Result<(AsymmetricSecretKey<V4>, AsymmetricPublicKey<V4>)> {
    use pasetors::keys::{AsymmetricKeyPair, Generate};
    let pair = AsymmetricKeyPair::<V4>::generate()
        .map_err(|error| ServerError::Token(error.to_string()))?;
    Ok((pair.secret, pair.public))
}

/// Serializes a key to its PASERK string form.
pub fn public_key_paserk(key: &AsymmetricPublicKey<V4>) -> Result<String> {
    use pasetors::paserk::FormatAsPaserk;
    let mut out = String::new();
    key.fmt(&mut out)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    Ok(out)
}

fn backdate_claims(claims: &mut Claims, skew: Duration) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let back = now - time::Duration::seconds(skew.as_secs() as i64);
    let back = back
        .max(OffsetDateTime::UNIX_EPOCH)
        .format(&Rfc3339)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    claims
        .issued_at(&back)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    claims
        .not_before(&back)
        .map_err(|error| ServerError::Token(error.to_string()))?;
    Ok(())
}

fn string_claim(claims: &Claims, key: &str) -> Result<String> {
    claims
        .get_claim(key)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ServerError::Unauthorized(format!("token is missing the {key:?} claim")))
}

fn unix_claim(claims: &Claims, key: &str) -> Result<i64> {
    let raw = string_claim(claims, key)?;
    let parsed = OffsetDateTime::parse(&raw, &Rfc3339)
        .map_err(|_| ServerError::Unauthorized(format!("token claim {key:?} is malformed")))?;
    Ok(parsed.unix_timestamp())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Current time as Unix seconds.
fn unix_now() -> i64 {
    crate::util::unix_now()
}

#[cfg(test)]
mod tests;
