//! Persisted row models shared between the database and the REST API.

use serde::{Deserialize, Serialize};

/// A paired client device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Server-assigned opaque identifier (UUID v4).
    pub id: String,
    /// Human-readable device name supplied at pairing time.
    pub name: String,
    /// PASERK-encoded Ed25519 public key proving possession at refresh.
    pub public_key: String,
    /// Unix timestamp (seconds) when the device paired.
    pub paired_at: i64,
    /// Unix timestamp (seconds) of the device's last authenticated request.
    pub last_seen: Option<i64>,
    /// Whether administrator revocation has invalidated the device.
    pub is_revoked: bool,
    /// Whether the device may manage devices and mint pairing codes.
    pub is_admin: bool,
}
