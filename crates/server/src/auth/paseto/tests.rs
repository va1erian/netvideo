//! Unit tests for token issuing and verification.

use std::time::Duration;

use pasetors::claims::Claims;
use pasetors::public;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::*;

fn server() -> ServerKey {
    ServerKey::generate().expect("server key")
}

#[test]
fn access_token_round_trips() {
    let key = server();
    let issued = issue_access_token(&key, "device-1", Duration::from_secs(3600)).unwrap();
    let verified = verify_access_token(&key, &issued.token).unwrap();
    assert_eq!(verified.device_id, "device-1");
    assert!(!verified.token_id.is_empty());
    assert!(verified.expires_at > crate::util::unix_now());
}

#[test]
fn access_token_signed_by_another_server_is_rejected() {
    let key = server();
    let other = server();
    let issued = issue_access_token(&key, "device-1", Duration::from_secs(3600)).unwrap();
    assert!(verify_access_token(&other, &issued.token).is_err());
}

#[test]
fn tampered_token_is_rejected() {
    let key = server();
    let issued = issue_access_token(&key, "device-1", Duration::from_secs(3600)).unwrap();
    let mut bytes = issued.token.into_bytes();
    let last = bytes.len() - 1;
    bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(bytes).unwrap();
    assert!(verify_access_token(&key, &tampered).is_err());
}

#[test]
fn expired_access_token_is_rejected() {
    let key = server();
    let past = (OffsetDateTime::now_utc() - time::Duration::hours(2))
        .format(&Rfc3339)
        .unwrap();
    let mut claims = Claims::new().unwrap();
    claims.subject("device-1").unwrap();
    claims.token_identifier("jti").unwrap();
    claims.expiration(&past).unwrap();
    let token = public::sign(key.secret(), &claims, None, None).unwrap();
    assert!(verify_access_token(&key, &token).is_err());
}

#[test]
fn refresh_proof_round_trips_and_is_bound_to_the_token() {
    let (device_secret, device_public) = generate_device_keypair().unwrap();
    let fingerprint = token_fingerprint("the-access-token");
    let proof = issue_refresh_proof(
        &device_secret,
        &fingerprint,
        Duration::from_secs(120),
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(verify_refresh_proof(&device_public, &proof, &fingerprint).is_ok());
    assert!(verify_refresh_proof(&device_public, &proof, "other-fingerprint").is_err());
}

#[test]
fn refresh_proof_from_another_device_is_rejected() {
    let (device_secret, _) = generate_device_keypair().unwrap();
    let (_, other_public) = generate_device_keypair().unwrap();
    let fingerprint = token_fingerprint("token");
    let proof = issue_refresh_proof(
        &device_secret,
        &fingerprint,
        Duration::from_secs(120),
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(verify_refresh_proof(&other_public, &proof, &fingerprint).is_err());
}

#[test]
fn public_key_paserk_round_trips_through_type() {
    use std::convert::TryFrom;
    let (_, public) = generate_device_keypair().unwrap();
    let encoded = public_key_paserk(&public).unwrap();
    assert!(encoded.starts_with("k4.public."));
    assert!(AsymmetricPublicKey::<V4>::try_from(encoded.as_str()).is_ok());
}
