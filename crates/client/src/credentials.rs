//! Per-server credentials: the device keypair and the current access token.
//!
//! [`CredentialStore`] is the persistence seam: platforms with an OS keystore
//! implement it themselves (Android wraps the file in a Keystore key).
//! [`FileStore`] keeps one JSON file per server with owner-only permissions
//! on Unix.

use std::path::{Path, PathBuf};

use pasetors::keys::AsymmetricSecretKey;
use pasetors::version4::V4;
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::error::{ClientError, Result};

/// Stored credentials for one paired server.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// Server-assigned device id.
    pub device_id: String,
    /// Device name supplied at pairing.
    pub device_name: String,
    /// PASERK `k4.secret.…` device private key.
    pub secret: String,
    /// Current PASETO access token.
    pub token: String,
    /// Token expiry (Unix seconds).
    pub expires_at: i64,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// Parses the stored device private key.
    pub fn secret_key(&self) -> Result<AsymmetricSecretKey<V4>> {
        auth::parse_secret_key(&self.secret)
    }

    /// Whether the token expires within `within_secs` of `now`.
    pub fn expiring_within(&self, within_secs: i64, now: i64) -> bool {
        self.expires_at.saturating_sub(now) <= within_secs
    }
}

/// Persists [`Credentials`] per server id.
pub trait CredentialStore: Send + Sync {
    /// Loads credentials, or `None` when the server is not paired.
    fn load(&self, server_id: &str) -> Result<Option<Credentials>>;
    /// Saves credentials, replacing any previous ones.
    fn save(&self, server_id: &str, credentials: &Credentials) -> Result<()>;
    /// Removes stored credentials. Missing ones are not an error.
    fn remove(&self, server_id: &str) -> Result<()>;
}

/// Reads and writes [`Credentials`] as files under a directory.
#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// The default store: `<config>/netvideo/servers`, or
    /// `<NETVIDEO_CLIENT_DIR>/servers` when that variable is set.
    pub fn new() -> Result<Self> {
        if let Some(dir) = std::env::var_os("NETVIDEO_CLIENT_DIR") {
            return Ok(Self {
                dir: PathBuf::from(dir).join("servers"),
            });
        }
        let base = dirs::config_dir().ok_or_else(|| {
            ClientError::Store("no user configuration directory available".into())
        })?;
        Ok(Self {
            dir: base.join("netvideo").join("servers"),
        })
    }

    /// A store rooted at an explicit directory (used by tests and the CLI).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The directory holding the credential files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file for a server id.
    pub fn path(&self, server_id: &str) -> PathBuf {
        self.dir.join(format!("{server_id}.json"))
    }
}

impl CredentialStore for FileStore {
    /// Loads credentials, or `None` when the server is not paired.
    fn load(&self, server_id: &str) -> Result<Option<Credentials>> {
        let path = self.path(server_id);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
                ClientError::Store(format!("cannot parse {}: {error}", path.display()))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes credentials atomically with owner-only permissions.
    fn save(&self, server_id: &str, credentials: &Credentials) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(server_id);
        let temporary = unique_temp(&path);
        let encoded = serde_json::to_vec_pretty(credentials)
            .map_err(|error| ClientError::Store(error.to_string()))?;
        if let Err(error) = write_private(&temporary, &encoded) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        std::fs::rename(&temporary, &path)?;
        Ok(())
    }

    /// Removes stored credentials. Missing files are not an error.
    fn remove(&self, server_id: &str) -> Result<()> {
        match std::fs::remove_file(self.path(server_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// A unique sibling path for an in-progress credential write.
fn unique_temp(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "credentials".to_string());
    path.with_file_name(format!("{name}.{}.{sequence}.tmp", std::process::id()))
}

/// Writes bytes, creating the file exclusively and restricting it to the owner
/// on Unix.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Credentials {
        Credentials {
            device_id: "device-1".into(),
            device_name: "test".into(),
            secret: "k4.secret.abc".into(),
            token: "v4.public.xyz".into(),
            expires_at: 2_000,
        }
    }

    #[test]
    fn save_load_and_remove_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "netvideo-client-creds-{}-{}",
            std::process::id(),
            crate::util::unix_now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileStore::with_dir(dir.clone());
        assert!(store.load("srv").unwrap().is_none());
        store.save("srv", &sample()).unwrap();
        assert_eq!(store.load("srv").unwrap().unwrap(), sample());
        store.remove("srv").unwrap();
        assert!(store.load("srv").unwrap().is_none());
        store.remove("srv").unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn debug_hides_secrets() {
        let shown = format!("{:?}", sample());
        assert!(!shown.contains("k4.secret") && !shown.contains("v4.public"));
    }

    #[test]
    fn expiring_within_uses_a_margin() {
        let credentials = sample();
        assert!(credentials.expiring_within(1_000, 1_500));
        assert!(!credentials.expiring_within(100, 1_500));
    }

    #[test]
    fn corrupt_credentials_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "netvideo-client-corrupt-{}-{}",
            std::process::id(),
            crate::util::unix_now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = FileStore::with_dir(dir.clone());
        std::fs::write(store.path("srv"), b"{ not json").unwrap();
        assert!(store.load("srv").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
