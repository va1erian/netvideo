//! Device keys, refresh proofs and token fingerprints.
//!
//! The formulas here must match `netvideo-server` exactly; both sides use
//! PASETO v4.public with the same claim names.

use std::convert::TryFrom;
use std::time::Duration;

use pasetors::claims::Claims;
use pasetors::keys::{AsymmetricKeyPair, AsymmetricPublicKey, AsymmetricSecretKey, Generate};
use pasetors::paserk::FormatAsPaserk;
use pasetors::public;
use pasetors::version4::V4;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::{ClientError, Result};
use crate::util::sha256_hex;

/// Claim binding a refresh proof to the token it renews.
pub const REFRESH_FOR_CLAIM: &str = "refresh_for";

/// Generates a fresh device keypair.
pub fn generate_keypair() -> Result<(AsymmetricSecretKey<V4>, AsymmetricPublicKey<V4>)> {
    let pair = AsymmetricKeyPair::<V4>::generate()
        .map_err(|error| ClientError::Token(error.to_string()))?;
    Ok((pair.secret, pair.public))
}

/// Serializes a public key to PASERK `k4.public.…`.
pub fn public_key_paserk(key: &AsymmetricPublicKey<V4>) -> Result<String> {
    let mut out = String::new();
    key.fmt(&mut out)
        .map_err(|error| ClientError::Token(error.to_string()))?;
    Ok(out)
}

/// Serializes a secret key to PASERK `k4.secret.…`.
pub fn secret_key_paserk(key: &AsymmetricSecretKey<V4>) -> Result<String> {
    let mut out = String::new();
    key.fmt(&mut out)
        .map_err(|error| ClientError::Token(error.to_string()))?;
    Ok(out)
}

/// Parses a stored PASERK secret key.
pub fn parse_secret_key(encoded: &str) -> Result<AsymmetricSecretKey<V4>> {
    AsymmetricSecretKey::<V4>::try_from(encoded.trim())
        .map_err(|error| ClientError::Token(error.to_string()))
}

/// The fingerprint a refresh proof must be bound to.
pub fn token_fingerprint(token: &str) -> String {
    sha256_hex(b"netvideo/refresh-token/v1:", token.as_bytes())
}

/// Signs a short-lived proof binding the device to `token_fingerprint`.
///
/// `iat`/`nbf` are backdated by `skew` so a modest clock difference from
/// the server does not invalidate the proof. Each proof carries a random
/// `jti`: the server accepts a given proof only once.
pub fn issue_refresh_proof(
    secret: &AsymmetricSecretKey<V4>,
    token_fingerprint: &str,
    ttl: Duration,
    skew: Duration,
) -> Result<String> {
    let mut claims =
        Claims::new_expires_in(&ttl).map_err(|error| ClientError::Token(error.to_string()))?;
    let back = OffsetDateTime::now_utc() - time::Duration::seconds(skew.as_secs() as i64);
    let back = back
        .max(OffsetDateTime::UNIX_EPOCH)
        .format(&Rfc3339)
        .map_err(|error| ClientError::Token(error.to_string()))?;
    claims
        .issued_at(&back)
        .map_err(|error| ClientError::Token(error.to_string()))?;
    claims
        .not_before(&back)
        .map_err(|error| ClientError::Token(error.to_string()))?;
    claims
        .add_additional(REFRESH_FOR_CLAIM, token_fingerprint)
        .map_err(|error| ClientError::Token(error.to_string()))?;
    claims
        .token_identifier(&uuid::Uuid::new_v4().to_string())
        .map_err(|error| ClientError::Token(error.to_string()))?;
    public::sign(secret, &claims, None, None).map_err(|error| ClientError::Token(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_paserk_round_trips() {
        let (secret, public) = generate_keypair().unwrap();
        let encoded_public = public_key_paserk(&public).unwrap();
        assert!(encoded_public.starts_with("k4.public."));
        let encoded_secret = secret_key_paserk(&secret).unwrap();
        assert!(encoded_secret.starts_with("k4.secret."));
        assert!(parse_secret_key(&encoded_secret).is_ok());
    }

    #[test]
    fn fingerprint_is_deterministic() {
        assert_eq!(token_fingerprint("abc"), token_fingerprint("abc"));
        assert_ne!(token_fingerprint("abc"), token_fingerprint("abcd"));
        assert_eq!(token_fingerprint("abc").len(), 64);
    }
}
