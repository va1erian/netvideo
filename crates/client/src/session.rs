//! A paired session with one server: pairing, token refresh and the browse
//! calls, with credentials persisted through a [`CredentialStore`].

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::auth;
use crate::client::RemoteClient;
use crate::config::ServerEndpoint;
use crate::credentials::{CredentialStore, Credentials};
use crate::error::{ClientError, Result};
use crate::types::{FolderPage, FolderRef, VideoDetail};
use crate::util::unix_now;

/// Refresh the token once it expires within this many seconds. Tokens live a
/// week by default and an expired token cannot be refreshed, so a device
/// that is used every few days never has to pair again.
pub const REFRESH_MARGIN_SECS: i64 = 4 * 24 * 3600;

/// Lifetime of a refresh proof; the server accepts at most 10 minutes.
const PROOF_TTL: Duration = Duration::from_secs(120);

/// Clock skew tolerated between the device and the server.
const PROOF_SKEW: Duration = Duration::from_secs(60);

/// One server's session. Cheap to share across threads behind an `Arc`.
pub struct Session {
    endpoint: ServerEndpoint,
    client: RemoteClient,
    store: Arc<dyn CredentialStore>,
    /// Cached credentials; the lock also serializes refreshes.
    credentials: Mutex<Option<Credentials>>,
}

impl Session {
    /// A session for `endpoint`, loading any stored credentials.
    pub fn new(endpoint: ServerEndpoint, store: Arc<dyn CredentialStore>) -> Result<Self> {
        let client = RemoteClient::from_endpoint(&endpoint)?;
        let credentials = store.load(&endpoint.id)?;
        Ok(Self {
            endpoint,
            client,
            store,
            credentials: Mutex::new(credentials),
        })
    }

    /// The server this session talks to.
    pub fn endpoint(&self) -> &ServerEndpoint {
        &self.endpoint
    }

    /// The underlying API client.
    pub fn client(&self) -> &RemoteClient {
        &self.client
    }

    /// This device's id on the server, when paired.
    pub fn device_id(&self) -> Option<String> {
        self.lock().as_ref().map(|c| c.device_id.clone())
    }

    /// Pairs with a one-time code, generating a fresh device key, and
    /// stores the credentials. Returns the new device id.
    pub fn pair(&self, pairing_code: &str, device_name: &str) -> Result<String> {
        let (secret, public) = auth::generate_keypair()?;
        let public = auth::public_key_paserk(&public)?;
        let paired = self
            .client
            .pair(pairing_code.trim(), device_name, &public)?;
        let credentials = Credentials {
            device_id: paired.device_id.clone(),
            device_name: paired.device_name,
            secret: auth::secret_key_paserk(&secret)?,
            token: paired.auth_token,
            expires_at: paired.expires_at,
        };
        self.store.save(&self.endpoint.id, &credentials)?;
        *self.lock() = Some(credentials);
        Ok(paired.device_id)
    }

    /// A valid access token, refreshed first when it nears expiry.
    pub fn token(&self) -> Result<String> {
        let mut guard = self.lock();
        let credentials = guard.as_mut().ok_or(ClientError::NotPaired)?;
        if credentials.expiring_within(REFRESH_MARGIN_SECS, unix_now()) {
            let fingerprint = auth::token_fingerprint(&credentials.token);
            let proof = auth::issue_refresh_proof(
                &credentials.secret_key()?,
                &fingerprint,
                PROOF_TTL,
                PROOF_SKEW,
            )?;
            let renewed = self.client.refresh(&credentials.token, &proof)?;
            let mut updated = credentials.clone();
            updated.token = renewed.auth_token;
            updated.expires_at = renewed.expires_at;
            self.store.save(&self.endpoint.id, &updated)?;
            *credentials = updated;
        }
        Ok(credentials.token.clone())
    }

    /// The library roots.
    pub fn roots(&self) -> Result<Vec<FolderRef>> {
        self.client.roots(&self.token()?)
    }

    /// One page of a folder.
    pub fn folder(&self, folder_id: &str, cursor: Option<&str>, limit: u32) -> Result<FolderPage> {
        self.client.folder(&self.token()?, folder_id, cursor, limit)
    }

    /// Full details of a video.
    pub fn video(&self, video_id: &str) -> Result<VideoDetail> {
        self.client.video(&self.token()?, video_id)
    }

    /// Saves this device's playback position.
    pub fn save_progress(&self, video_id: &str, position_ms: i64, watched: bool) -> Result<()> {
        self.client
            .save_progress(&self.token()?, video_id, position_ms, watched)
    }

    /// The direct-play URL of a video; see [`RemoteClient::file_url`].
    pub fn file_url(&self, video_id: &str) -> String {
        self.client.file_url(video_id)
    }

    /// Unpairs: asks the server to revoke this device and deletes the stored
    /// credentials. Revoking is best effort, so an unreachable server does
    /// not trap the user (and only admin devices may revoke today).
    pub fn forget(&self) -> Result<()> {
        let device = self.device_id();
        if let (Some(device), Ok(token)) = (device, self.token()) {
            let _ = self.client.revoke_device(&token, &device);
        }
        self.store.remove(&self.endpoint.id)?;
        *self.lock() = None;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Option<Credentials>> {
        self.credentials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
