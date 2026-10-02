//! The error crossing the uniffi boundary.

use netvideo_client::ClientError;

/// Errors surfaced to Kotlin. The variants the UI acts on are kept apart:
/// [`MobileError::NotPaired`] and [`MobileError::Unauthorized`] lead back to
/// pairing, [`MobileError::Network`] to a retry, [`MobileError::Server`]
/// with 404 to dropping a stale item.
///
/// uniffi does not carry the `Display` text to Kotlin (`message` is empty or
/// `detail=…`), so Kotlin branches on the exception type and shows `detail`.
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
    /// The server URL is unusable (no `https://`, a path or query, ...).
    #[error("{detail}")]
    InvalidUrl {
        /// Human-readable description.
        detail: String,
    },
    /// The server answered with an error status.
    #[error("{detail}")]
    Server {
        /// HTTP status code.
        status: u16,
        /// The server's message.
        detail: String,
    },
    /// Any other failure: an unexpected response, keys, the vault.
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
            ClientError::Url(_) => Self::InvalidUrl {
                detail: error.to_string(),
            },
            ClientError::Http { status, message } => Self::Server {
                status,
                detail: message,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_cases_the_ui_acts_on_apart() {
        assert!(matches!(
            ClientError::Unauthorized.into(),
            MobileError::Unauthorized
        ));
        assert!(matches!(
            ClientError::PairingCode.into(),
            MobileError::PairingCode
        ));
        assert!(matches!(
            ClientError::Network("down".into()).into(),
            MobileError::Network { .. }
        ));
        assert!(matches!(
            ClientError::Url("no scheme".into()).into(),
            MobileError::InvalidUrl { .. }
        ));
        let not_found: MobileError = ClientError::Http {
            status: 404,
            message: "video not found".into(),
        }
        .into();
        assert!(matches!(
            not_found,
            MobileError::Server { status: 404, ref detail } if detail == "video not found"
        ));
        assert!(matches!(
            ClientError::Store("locked".into()).into(),
            MobileError::Other { .. }
        ));
    }
}
