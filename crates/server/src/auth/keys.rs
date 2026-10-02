//! The server's long-lived Ed25519 signing key.
//!
//! The key is generated once on first start and persisted as a PASERK
//! `k4.secret` string inside the data directory. On Unix the file is created
//! with mode `0600`; on Windows the data directory's ACL is the protection
//! boundary (the file must not be readable by other users).

use std::convert::TryFrom;
use std::path::{Path, PathBuf};

use pasetors::keys::{AsymmetricKeyPair, AsymmetricPublicKey, AsymmetricSecretKey, Generate};
use pasetors::paserk::FormatAsPaserk;
use pasetors::version4::V4;

use crate::error::{Result, ServerError};

/// File name of the persisted server secret key.
pub const KEY_FILE: &str = "server.key";

/// The fingerprint of a PASERK `k4.public` server key. Clients compute the
/// same value (`netvideo-client`'s `server_key_fingerprint`).
pub fn server_key_fingerprint(public_paserk: &str) -> String {
    crate::util::sha256_hex(b"netvideo/server-key/v1:", public_paserk.as_bytes())
}

/// The server keypair, kept in memory for the process lifetime.
#[derive(Clone)]
pub struct ServerKey {
    secret: AsymmetricSecretKey<V4>,
    public: AsymmetricPublicKey<V4>,
}

impl ServerKey {
    /// Generates a fresh server key.
    pub fn generate() -> Result<Self> {
        let pair = AsymmetricKeyPair::<V4>::generate()
            .map_err(|error| ServerError::Token(error.to_string()))?;
        Ok(Self {
            secret: pair.secret,
            public: pair.public,
        })
    }

    /// Loads the server key from `path`, generating and persisting one if it
    /// does not exist yet.
    ///
    /// When another process wins the creation race, its key is loaded instead
    /// of returning the losing in-memory key, so all processes agree.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let key = Self::generate()?;
        if key.persist(path)? {
            Ok(key)
        } else {
            Self::load(path)
        }
    }

    /// Loads an existing key from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let encoded = std::fs::read_to_string(path).map_err(|error| {
            ServerError::Token(format!(
                "cannot read server key {}: {error}",
                path.display()
            ))
        })?;
        if encoded.trim().is_empty() {
            return Err(ServerError::Token(format!(
                "server key {} is empty; remove it to generate a new one",
                path.display()
            )));
        }
        let secret = AsymmetricSecretKey::<V4>::try_from(encoded.trim()).map_err(|error| {
            ServerError::Token(format!("server key {} is invalid: {error}", path.display()))
        })?;
        Self::from_secret(secret)
    }

    /// Writes the key to disk with restrictive permissions. Returns `true`
    /// when this process created the file, `false` when it already existed.
    pub fn persist(&self, path: &Path) -> Result<bool> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut encoded = String::new();
        self.secret
            .fmt(&mut encoded)
            .map_err(|error| ServerError::Token(error.to_string()))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        restrict_permissions(&mut options);
        match options.open(path) {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(encoded.as_bytes())?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                Ok(true)
            }
            // A concurrent process created it first: it owns the key.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// The server's public key, used to verify issued tokens.
    pub fn public(&self) -> &AsymmetricPublicKey<V4> {
        &self.public
    }

    /// The public key as a PASERK `k4.public` string, sent to clients at
    /// pairing so they can check every token they receive.
    pub fn public_paserk(&self) -> Result<String> {
        crate::auth::paseto::public_key_paserk(&self.public)
    }

    /// The fingerprint a pairing QR code carries, so a client can tell the
    /// real server's key from an impostor's.
    pub fn fingerprint(&self) -> Result<String> {
        Ok(server_key_fingerprint(&self.public_paserk()?))
    }

    /// The server's secret key, used to sign tokens.
    pub fn secret(&self) -> &AsymmetricSecretKey<V4> {
        &self.secret
    }
}

impl std::fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerKey(..)")
    }
}

impl ServerKey {
    fn from_secret(secret: AsymmetricSecretKey<V4>) -> Result<Self> {
        let public = AsymmetricPublicKey::<V4>::try_from(&secret)
            .map_err(|error| ServerError::Token(error.to_string()))?;
        Ok(Self { secret, public })
    }
}

/// Default location of the server key inside a data directory.
pub fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join(KEY_FILE)
}

#[cfg(unix)]
fn restrict_permissions(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn restrict_permissions(_options: &mut std::fs::OpenOptions) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "netvideo-key-{name}-{}-{}",
            std::process::id(),
            crate::util::unix_now()
        ))
    }

    #[test]
    fn generate_produces_a_verifiable_public_key() {
        let key = ServerKey::generate().expect("generate");
        let text = {
            use pasetors::paserk::FormatAsPaserk;
            let mut out = String::new();
            key.public().fmt(&mut out).unwrap();
            out
        };
        assert!(text.starts_with("k4.public."));
    }

    #[test]
    fn load_or_create_persists_and_reloads_identical_key() {
        let path = temp_path("persist");
        let _ = std::fs::remove_file(&path);
        let first = ServerKey::load_or_create(&path).expect("create");
        let second = ServerKey::load_or_create(&path).expect("reload");
        assert_eq!(first.secret(), second.secret());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn empty_key_file_is_reported_clearly() {
        let path = temp_path("empty");
        std::fs::write(&path, b"").unwrap();
        let error = ServerKey::load(&path).expect_err("empty key must fail");
        assert!(error.to_string().contains("is empty"));
        std::fs::remove_file(&path).ok();
    }
}
