//! Client error type.

/// Errors produced by the remote client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server URL is missing a scheme or otherwise unusable.
    #[error("invalid server URL: {0}")]
    Url(String),

    /// No credentials are stored for this server.
    #[error("not paired with this server")]
    NotPaired,

    /// The pairing code was wrong, expired or already used.
    #[error("invalid or expired pairing code")]
    PairingCode,

    /// The server rejected a request.
    #[error("server error ({status}): {message}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Message from the server's JSON error body, when present.
        message: String,
    },

    /// The device's token is expired or revoked and cannot be refreshed.
    #[error("unauthorized; the device may have been revoked or must pair again")]
    Unauthorized,

    /// A transport-level failure.
    #[error("network error: {0}")]
    Network(String),

    /// The server returned a response we could not parse.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The token/key subsystem failed.
    #[error("token error: {0}")]
    Token(String),

    /// Reading or writing local files failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The credential store could not be read or written.
    #[error("credential store error: {0}")]
    Store(String),
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, ClientError>;
