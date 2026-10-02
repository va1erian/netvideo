//! Small internal helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current time as a Unix timestamp in seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Hex-encoded SHA-256 of `bytes` with an optional domain-separation prefix.
pub fn sha256_hex(prefix: &[u8], bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
