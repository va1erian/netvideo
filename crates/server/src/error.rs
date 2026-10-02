//! Error types shared across the server.

use std::path::PathBuf;

/// Errors produced by the server's library, database, auth and I/O layers.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Configuration is missing, unreadable or internally inconsistent.
    #[error("configuration error: {0}")]
    Config(String),

    /// An I/O operation failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The bundled SQLite store returned an error.
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    /// The connection pool could not hand out a connection.
    #[error("database pool error: {0}")]
    Pool(#[from] r2d2::Error),

    /// The supplied credentials are invalid, expired or revoked.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// The request was well-formed but conflicts with current state.
    #[error("conflict: {0}")]
    Conflict(String),

    /// The requested file could not be resolved inside any library root.
    #[error("path rejected: {0}")]
    PathRejected(String),

    /// A filesystem path escaped its authorized root.
    #[error("path {path} escapes root {root}")]
    PathEscape {
        /// The offending path.
        path: PathBuf,
        /// The root it was supposed to stay under.
        root: PathBuf,
    },

    /// A pairing code was wrong, expired or already used.
    #[error("invalid pairing code")]
    InvalidPairingCode,

    /// The token subsystem failed.
    #[error("token error: {0}")]
    Token(String),
}

/// Convenience alias for fallible server operations.
pub type Result<T> = std::result::Result<T, ServerError>;
