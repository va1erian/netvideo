//! `netvideo-mobile`: the Android-facing bindings over [`netvideo_client`].
//!
//! A thin [uniffi] wrapper, so pairing, token refresh and browsing cannot
//! drift from the desktop client. It builds on the host as well as for the
//! Android NDK, which keeps `cargo test` usable without a device.
//!
//! Kotlin owns the UI, playback (Media3) and key storage: it implements
//! [`SecretVault`] on top of an Android Keystore key and hands it to
//! [`MobileSession`], which keeps the device credentials there.

#![forbid(unsafe_code)]

uniffi::setup_scaffolding!();

mod error;
mod session;
mod types;
mod vault;

pub use error::MobileError;
pub use session::MobileSession;
pub use types::{FolderPage, FolderRef, Progress, StreamInfo, VideoDetail, VideoSummary};
pub use vault::SecretVault;
