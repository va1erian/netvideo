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

/// Refresh the token once it expires within this many seconds (or within
/// half its lifetime, for servers issuing short tokens). Tokens live a week
/// by default and an expired token cannot be refreshed, so a device used
/// every few days never has to pair again.
pub const REFRESH_MARGIN_SECS: i64 = 4 * 24 * 3600;

/// Lifetime of a refresh proof. With [`PROOF_SKEW`] it spans 10 minutes,
/// the most the server accepts, and tolerates clocks 5 minutes off.
pub const PROOF_TTL: Duration = Duration::from_secs(300);

/// How far a proof's `iat` is backdated, for device clocks running fast.
pub const PROOF_SKEW: Duration = Duration::from_secs(300);

/// One server's session. Cheap to share across threads behind an `Arc`.
pub struct Session {
    endpoint: ServerEndpoint,
    client: RemoteClient,
    store: Arc<dyn CredentialStore>,
    /// Cached credentials. Held only briefly, never across network calls.
    credentials: Mutex<Option<Credentials>>,
    /// Serializes refreshes, so concurrent calls spend one proof.
    refreshing: Mutex<()>,
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
            refreshing: Mutex::new(()),
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
        lock(&self.credentials)
            .as_ref()
            .map(|c| c.device_id.clone())
    }

    /// Pairs with a typed one-time code; see [`Session::pair_pinned`].
    pub fn pair(&self, pairing_code: &str, device_name: &str) -> Result<String> {
        self.pair_pinned(pairing_code, device_name, None)
    }

    /// Pairs with a one-time code, generating a fresh device key, and
    /// stores the credentials. Returns the new device id.
    ///
    /// The server's key is pinned: with `key_fingerprint` (from a pairing QR
    /// code) it must match, and either way every later token must be signed
    /// by it. A server that does not send its key is accepted only without a
    /// fingerprint, for servers that predate pinning.
    pub fn pair_pinned(
        &self,
        pairing_code: &str,
        device_name: &str,
        key_fingerprint: Option<&str>,
    ) -> Result<String> {
        let (secret, public) = auth::generate_keypair()?;
        let public = auth::public_key_paserk(&public)?;
        let paired = self
            .client
            .pair(pairing_code.trim(), device_name, &public)?;
        match (&paired.server_key, key_fingerprint) {
            (Some(key), expected) => {
                if expected.is_some_and(|fp| auth::server_key_fingerprint(key) != fp) {
                    return Err(ClientError::ServerKey);
                }
                auth::verify_server_token(key, &paired.auth_token)?;
            }
            (None, Some(_)) => return Err(ClientError::ServerKey),
            (None, None) => {}
        }
        let credentials = Credentials {
            device_id: paired.device_id.clone(),
            device_name: paired.device_name,
            secret: auth::secret_key_paserk(&secret)?,
            token: paired.auth_token,
            lifetime_secs: paired.expires_at - unix_now(),
            expires_at: paired.expires_at,
            server_key: paired.server_key,
        };
        let mut guard = lock(&self.credentials);
        self.store.save(&self.endpoint.id, &credentials)?;
        *guard = Some(credentials);
        Ok(paired.device_id)
    }

    /// A valid access token, refreshed first when it nears expiry.
    ///
    /// A failed refresh is not fatal while the current token is still
    /// valid: it is returned and the refresh is retried on a later call.
    /// Only an expired token (which the server would reject anyway) or a
    /// missing pairing is an error.
    pub fn token(&self) -> Result<String> {
        let current = self.current()?;
        if !needs_refresh(&current, unix_now()) {
            return Ok(current.token);
        }
        let _refreshing = lock(&self.refreshing);
        // Another caller may have refreshed while we waited.
        let current = self.current()?;
        let now = unix_now();
        if !needs_refresh(&current, now) {
            return Ok(current.token);
        }
        if current.expires_at <= now {
            return Err(ClientError::Unauthorized);
        }
        match self.refresh(&current) {
            Ok(token) => Ok(token),
            Err(ClientError::NotPaired) => Err(ClientError::NotPaired),
            Err(_) => Ok(current.token),
        }
    }

    /// Renews `current`'s token and stores it, unless the session was
    /// re-paired or forgotten meanwhile.
    fn refresh(&self, current: &Credentials) -> Result<String> {
        let fingerprint = auth::token_fingerprint(&current.token);
        let proof =
            auth::issue_refresh_proof(&current.secret_key()?, &fingerprint, PROOF_TTL, PROOF_SKEW)?;
        let renewed = self.client.refresh(&current.token, &proof)?;
        if let Some(key) = &current.server_key {
            auth::verify_server_token(key, &renewed.auth_token)?;
        }
        let mut guard = lock(&self.credentials);
        let still_current = guard
            .as_ref()
            .is_some_and(|c| c.device_id == current.device_id && c.token == current.token);
        if !still_current {
            return Err(ClientError::NotPaired);
        }
        let mut updated = current.clone();
        updated.lifetime_secs = renewed.expires_at - unix_now();
        updated.token = renewed.auth_token;
        updated.expires_at = renewed.expires_at;
        self.store.save(&self.endpoint.id, &updated)?;
        let token = updated.token.clone();
        *guard = Some(updated);
        Ok(token)
    }

    fn current(&self) -> Result<Credentials> {
        lock(&self.credentials)
            .clone()
            .ok_or(ClientError::NotPaired)
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

    /// Unpairs: asks the server to revoke this device, then deletes the
    /// stored credentials. Revoking is best effort, so an unreachable server
    /// does not trap the user, and it is skipped once the token expired.
    /// Only admin devices may revoke, so a viewer is forgotten locally and
    /// stays listed on the server until an admin revokes it.
    pub fn forget(&self) -> Result<()> {
        let _refreshing = lock(&self.refreshing);
        if let Ok(current) = self.current()
            && current.expires_at > unix_now()
        {
            let _ = self
                .client
                .revoke_device(&current.token, &current.device_id);
        }
        let mut guard = lock(&self.credentials);
        self.store.remove(&self.endpoint.id)?;
        *guard = None;
        Ok(())
    }
}

/// Whether `credentials` should be refreshed at `now`.
fn needs_refresh(credentials: &Credentials, now: i64) -> bool {
    let margin = match credentials.lifetime_secs {
        lifetime if lifetime > 0 => REFRESH_MARGIN_SECS.min(lifetime / 2),
        _ => REFRESH_MARGIN_SECS,
    };
    credentials.expiring_within(margin, now)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
