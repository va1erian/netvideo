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
    /// The token's lifetime when it was issued, in seconds (0 if unknown).
    #[serde(default)]
    pub lifetime_secs: i64,
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

    /// The file for a server id. Ids are [`ServerEndpoint`] ids (lowercase
    /// hex); anything else is refused so it cannot name another path.
    ///
    /// [`ServerEndpoint`]: crate::ServerEndpoint
    pub fn path(&self, server_id: &str) -> Result<PathBuf> {
        let valid = !server_id.is_empty()
            && server_id.len() <= 64
            && server_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !valid {
            return Err(ClientError::Store(format!(
                "invalid server id {server_id:?}"
            )));
        }
        Ok(self.dir.join(format!("{server_id}.json")))
    }
}

impl CredentialStore for FileStore {
    /// Loads credentials, or `None` when the server is not paired.
    fn load(&self, server_id: &str) -> Result<Option<Credentials>> {
        let path = self.path(server_id)?;
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
        create_private_dir(&self.dir)?;
        let path = self.path(server_id)?;
        let temporary = unique_temp(&path);
        let encoded = serde_json::to_vec_pretty(credentials)
            .map_err(|error| ClientError::Store(error.to_string()))?;
        if let Err(error) = write_private(&temporary, &encoded) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        // `rename` replaces the old file atomically on Unix and Windows.
        if let Err(error) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(())
    }

    /// Removes stored credentials. Missing files are not an error.
    fn remove(&self, server_id: &str) -> Result<()> {
        match std::fs::remove_file(self.path(server_id)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// Creates `dir` (owner-only on Unix, so file names are private too).
fn create_private_dir(dir: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    Ok(())
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
            lifetime_secs: 1_000,
        }
    }

    #[test]
    fn save_load_and_remove_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::with_dir(dir.path().join("servers"));
        assert!(store.load("ab12").unwrap().is_none());
        store.save("ab12", &sample()).unwrap();
        let mut updated = sample();
        updated.token = "v4.public.new".into();
        store.save("ab12", &updated).unwrap();
        assert_eq!(store.load("ab12").unwrap().unwrap(), updated);
        let names: Vec<_> = std::fs::read_dir(dir.path().join("servers"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["ab12.json"], "no temporary file left behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&store.path("ab12").unwrap()), 0o600);
            assert_eq!(mode(&dir.path().join("servers")), 0o700);
        }
        store.remove("ab12").unwrap();
        assert!(store.load("ab12").unwrap().is_none());
        store.remove("ab12").unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::with_dir(dir.path().to_path_buf());
        std::fs::write(store.path("ab12").unwrap(), b"{ not json").unwrap();
        assert!(store.load("ab12").is_err());
    }

    #[test]
    fn server_ids_cannot_name_other_paths() {
        let store = FileStore::with_dir(PathBuf::from("/nonexistent"));
        for id in ["", "../x", "AB12", "a/b", "srv"] {
            assert!(store.path(id).is_err(), "{id:?}");
        }
    }
}
