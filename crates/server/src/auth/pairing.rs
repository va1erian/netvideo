//! Device pairing: one-time codes and key registration.

use std::convert::TryFrom;
use std::time::Duration;

use hmac::{Hmac, Mac};
use pasetors::keys::AsymmetricPublicKey;
use pasetors::version4::V4;
use rand::Rng;
use rand::distributions::Uniform;
use sha2::Sha256;

use crate::auth::keys::ServerKey;
use crate::auth::paseto::{IssuedToken, issue_access_token};
use crate::db::Db;
use crate::db::devices::is_valid_pairing_code_format;
use crate::db::models::Device;
use crate::error::{Result, ServerError};

/// Successful pairing result.
#[derive(Debug, Clone)]
pub struct PairOutcome {
    /// The newly registered device.
    pub device: Device,
    /// Its first access token.
    pub token: IssuedToken,
}

/// Generates, stores and returns a fresh one-time pairing code. The device
/// that redeems it becomes an administrator when `grants_admin` is set.
///
/// Only a keyed HMAC of the code is persisted. The HMAC key is the server
/// secret, which lives in `server.key` rather than the database, so a leak of
/// the database alone does not expose a brute-forceable code. Codes are also
/// short-lived and single-use.
pub fn generate_pairing_code(
    db: &Db,
    key: &ServerKey,
    ttl_secs: u64,
    grants_admin: bool,
    now: i64,
) -> Result<String> {
    let code = random_pairing_code();
    let hash = pairing_code_hash(key, &code);
    db.purge_expired_pairing_codes(now)?;
    db.insert_pairing_code(
        &hash,
        now,
        now.saturating_add(ttl_secs as i64),
        grants_admin,
    )?;
    Ok(code)
}

/// Validates a pairing request and, on success, registers the device.
///
/// A wrong, expired or already-used code all yield [`ServerError::InvalidPairingCode`]
/// so callers cannot distinguish them.
pub fn pair(
    db: &Db,
    key: &ServerKey,
    token_ttl: Duration,
    pairing_code: &str,
    device_name: &str,
    public_key: &str,
    now: i64,
) -> Result<PairOutcome> {
    let name = validate_device_name(device_name)?;
    if !is_valid_pairing_code_format(pairing_code) {
        return Err(ServerError::InvalidPairingCode);
    }
    let parsed_key = parse_public_key(public_key)?;
    let canonical_key = crate::auth::paseto::public_key_paserk(&parsed_key)?;
    let device = Device {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        public_key: canonical_key,
        paired_at: now,
        last_seen: None,
        is_revoked: false,
        is_admin: false,
    };

    // Consumption and device registration are one transaction, so a code
    // cannot be burned without creating its device.
    let expected = pairing_code_hash(key, pairing_code);
    let Some(device) = db.pair_device_with_code(&expected, now, &device)? else {
        return Err(ServerError::InvalidPairingCode);
    };

    let token = issue_access_token(key, &device.id, &device.public_key, token_ttl)?;
    Ok(PairOutcome { device, token })
}

/// Ensures the device name is a short, printable, non-empty string.
pub fn validate_device_name(name: &str) -> Result<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ServerError::Unauthorized("device name is empty".into()));
    }
    if trimmed.chars().count() > 64 {
        return Err(ServerError::Unauthorized(
            "device name is longer than 64 characters".into(),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ServerError::Unauthorized(
            "device name contains control characters".into(),
        ));
    }
    Ok(trimmed)
}

/// A uniformly random six-digit code.
fn random_pairing_code() -> String {
    let range = Uniform::new(0u32, 1_000_000);
    let value = rand::rngs::OsRng.sample(range);
    format!("{value:06}")
}

/// Keyed hash of a pairing code, stored instead of the code itself.
pub fn pairing_code_hash(key: &ServerKey, code: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.secret().as_bytes())
        .expect("HMAC-SHA256 accepts keys of any length");
    mac.update(b"netvideo/pairing-code/v1:");
    mac.update(code.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn parse_public_key(public_key: &str) -> Result<AsymmetricPublicKey<V4>> {
    AsymmetricPublicKey::<V4>::try_from(public_key.trim()).map_err(|_| {
        ServerError::Unauthorized("device public key is not a valid PASERK key".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_codes_are_six_digits() {
        for _ in 0..200 {
            let code = random_pairing_code();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        }
    }

    #[test]
    fn code_hash_is_deterministic_and_key_dependent() {
        let a = ServerKey::generate().unwrap();
        let b = ServerKey::generate().unwrap();
        assert_eq!(
            pairing_code_hash(&a, "123456"),
            pairing_code_hash(&a, "123456")
        );
        assert_ne!(
            pairing_code_hash(&a, "123456"),
            pairing_code_hash(&b, "123456")
        );
        assert_ne!(
            pairing_code_hash(&a, "123456"),
            pairing_code_hash(&a, "654321")
        );
    }

    #[test]
    fn device_name_validation() {
        assert!(validate_device_name("Living Room").is_ok());
        assert!(validate_device_name("  ").is_err());
        assert!(validate_device_name(&"x".repeat(65)).is_err());
        assert!(validate_device_name("bad\u{7}name").is_err());
    }
}
