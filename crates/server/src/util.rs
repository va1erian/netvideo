//! Small shared helpers with no dependencies on other server modules.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current time as a Unix timestamp in seconds, or `0` if the clock is
/// before the epoch (which never happens on a correctly configured host).
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Hex-encoded SHA-256 of `bytes`, with an optional domain-separation prefix.
pub fn sha256_hex(prefix: &[u8], bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_now_is_sane() {
        assert!(unix_now() > 1_700_000_000);
    }

    #[test]
    fn sha256_hex_is_deterministic_and_prefix_separated() {
        let a = sha256_hex(b"a:", b"payload");
        assert_eq!(a, sha256_hex(b"a:", b"payload"));
        assert_ne!(a, sha256_hex(b"b:", b"payload"));
        assert_eq!(a.len(), 64);
    }
}
