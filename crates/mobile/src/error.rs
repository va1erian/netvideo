//! The error crossing the uniffi boundary.

use netvideo_client::ClientError;

/// Errors surfaced to Kotlin. The variants the UI acts on are kept apart:
/// [`MobileError::NotPaired`] and [`MobileError::Unauthorized`] lead back to
/// pairing, [`MobileError::Network`] to a retry.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    /// No device credentials are stored for this server.
    #[error("not paired with this server")]
    NotPaired,
    /// The pairing code was wrong, expired or already used.
    #[error("invalid or expired pairing code")]
    PairingCode,
    /// The device was revoked or its token expired: pair again.
    #[error("this device must pair again")]
    Unauthorized,
    /// The server could not be reached.
    #[error("{detail}")]
    Network {
        /// Human-readable description.
        detail: String,
    },
    /// Any other failure: a server error, a bad URL, the vault.
    #[error("{detail}")]
    Other {
        /// Human-readable description.
        detail: String,
    },
}

impl From<ClientError> for MobileError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::NotPaired => Self::NotPaired,
            ClientError::PairingCode => Self::PairingCode,
            ClientError::Unauthorized => Self::Unauthorized,
            ClientError::Network(_) => Self::Network {
                detail: error.to_string(),
            },
            other => Self::Other {
                detail: other.to_string(),
            },
        }
    }
}

impl From<uniffi::UnexpectedUniFFICallbackError> for MobileError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Other {
            detail: format!("secret vault failed: {}", error.reason),
        }
    }
}
