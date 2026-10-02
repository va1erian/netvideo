//! The [`MobileSession`] object exported to Kotlin.
//!
//! Every method that talks to the server blocks; Kotlin calls them from a
//! background dispatcher.

use std::sync::Arc;

use netvideo_client::{ServerEndpoint, Session};

use crate::error::MobileError;
use crate::types::{FolderPage, FolderRef, PairingLink, VideoDetail};
use crate::vault::{SecretVault, VaultStore};

/// Parses a scanned pairing QR code (`netvideo-server pair --qr`).
#[uniffi::export]
pub fn parse_pairing_link(text: String) -> Result<PairingLink, MobileError> {
    Ok(netvideo_client::PairingLink::parse(&text)?.into())
}

/// One server's session: pairing, automatic token refresh and browsing.
#[derive(uniffi::Object)]
pub struct MobileSession {
    session: Session,
}

#[uniffi::export]
impl MobileSession {
    /// A session for the server at `url` (`https://…`; `http://` only for a
    /// LAN debug build), keeping credentials in `vault`. Blocks while the
    /// vault is read, so call it off the main thread.
    #[uniffi::constructor]
    pub fn new(url: String, vault: Arc<dyn SecretVault>) -> Result<Arc<Self>, MobileError> {
        let endpoint = ServerEndpoint::new("server", &url)?;
        let session = Session::new(endpoint, Arc::new(VaultStore(vault)))?;
        Ok(Arc::new(Self { session }))
    }

    /// The normalized server URL.
    pub fn url(&self) -> String {
        self.session.endpoint().url.clone()
    }

    /// Checks that a netvideo server answers at this URL, before the user
    /// spends a one-time pairing code on it.
    pub fn check_server(&self) -> Result<(), MobileError> {
        self.session.client().health()?;
        Ok(())
    }

    /// This device's id on the server, when paired.
    pub fn device_id(&self) -> Option<String> {
        self.session.device_id()
    }

    /// Whether this device has credentials for the server.
    pub fn is_paired(&self) -> bool {
        self.session.device_id().is_some()
    }

    /// Pairs with a one-time code and returns the new device id. With the
    /// `key_fingerprint` of a scanned [`PairingLink`], a server holding any
    /// other key is refused ([`MobileError::ServerMismatch`]).
    pub fn pair(
        &self,
        code: String,
        device_name: String,
        key_fingerprint: Option<String>,
    ) -> Result<String, MobileError> {
        Ok(self
            .session
            .pair_pinned(&code, &device_name, key_fingerprint.as_deref())?)
    }

    /// The `Authorization` header value for requests Kotlin makes itself
    /// (the player's file requests), refreshing the token when due.
    pub fn authorization(&self) -> Result<String, MobileError> {
        Ok(format!("Bearer {}", self.session.token()?))
    }

    /// The library roots.
    pub fn roots(&self) -> Result<Vec<FolderRef>, MobileError> {
        let roots = self.session.roots()?;
        Ok(roots.into_iter().map(FolderRef::from).collect())
    }

    /// One page of a folder, from `cursor` (the start when `None`).
    pub fn folder(
        &self,
        folder_id: String,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<FolderPage, MobileError> {
        let page = self.session.folder(&folder_id, cursor.as_deref(), limit)?;
        Ok(page.into())
    }

    /// Full details of a video, including this device's progress.
    pub fn video(&self, video_id: String) -> Result<VideoDetail, MobileError> {
        Ok(self.session.video(&video_id)?.into())
    }

    /// Saves this device's playback position.
    pub fn save_progress(
        &self,
        video_id: String,
        position_ms: i64,
        watched: bool,
    ) -> Result<(), MobileError> {
        Ok(self
            .session
            .save_progress(&video_id, position_ms, watched)?)
    }

    /// The direct-play URL of a video. Requests need [`Self::authorization`]
    /// as their `Authorization` header.
    pub fn file_url(&self, video_id: String) -> String {
        self.session.file_url(&video_id)
    }

    /// Unpairs: asks the server to revoke this device (best effort; only
    /// admin devices may) and deletes the stored credentials.
    pub fn forget(&self) -> Result<(), MobileError> {
        Ok(self.session.forget()?)
    }
}
