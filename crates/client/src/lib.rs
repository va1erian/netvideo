//! `netvideo-client`: the cross-platform client for `netvideo-server`.
//!
//! It holds everything the Android and desktop apps share:
//!
//! - [`ServerEndpoint`] describing a server, with a stable id;
//! - [`CredentialStore`] persisting each server's device key and token,
//!   with [`FileStore`] as the plain-file implementation (platforms wrap
//!   theirs in an OS keystore);
//! - [`RemoteClient`] speaking the REST API;
//! - [`Session`] tying them together: pairing, automatic token refresh and
//!   the browse calls.
//!
//! All network calls are blocking (`ureq`); callers run them on worker
//! threads.

#![forbid(unsafe_code)]

pub mod auth;
pub mod client;
pub mod config;
pub mod credentials;
pub mod error;
pub mod link;
pub mod session;
pub mod types;
mod util;

pub use client::RemoteClient;
pub use config::ServerEndpoint;
pub use credentials::{CredentialStore, Credentials, FileStore};
pub use error::{ClientError, Result};
pub use link::PairingLink;
pub use session::Session;
pub use types::{FolderPage, FolderRef, Progress, StreamInfo, VideoDetail, VideoSummary};
pub use util::unix_now;
