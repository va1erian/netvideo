//! Credential storage backed by a vault the platform implements.

use std::sync::Arc;

use netvideo_client::{ClientError, CredentialStore, Credentials};

use crate::error::MobileError;

/// A small string store that encrypts what it keeps, implemented in Kotlin
/// on top of an Android Keystore key. Values are device private keys and
/// tokens, so they must never be written anywhere in clear.
///
/// Calls arrive on whichever thread called into [`crate::MobileSession`],
/// possibly several at once, so implementations must be thread-safe. They
/// may block (Keystore operations do).
#[uniffi::export(with_foreign)]
pub trait SecretVault: Send + Sync {
    /// The value stored under `key`, or `None`.
    fn read(&self, key: String) -> Result<Option<String>, MobileError>;
    /// Stores `value` under `key`, replacing any previous value.
    fn write(&self, key: String, value: String) -> Result<(), MobileError>;
    /// Deletes `key`. A missing key is not an error.
    fn delete(&self, key: String) -> Result<(), MobileError>;
}

/// Adapts a [`SecretVault`] to the client's [`CredentialStore`], keeping
/// each server's credentials as one JSON value keyed by the server id.
pub(crate) struct VaultStore(pub Arc<dyn SecretVault>);

fn store_error(error: MobileError) -> ClientError {
    ClientError::Store(error.to_string())
}

impl CredentialStore for VaultStore {
    fn load(&self, server_id: &str) -> netvideo_client::Result<Option<Credentials>> {
        let Some(text) = self.0.read(server_id.to_owned()).map_err(store_error)? else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            // serde's message can quote a stored value (a key or token).
            .map_err(|error| {
                ClientError::Store(format!(
                    "corrupt credentials (line {}, column {})",
                    error.line(),
                    error.column()
                ))
            })
    }

    fn save(&self, server_id: &str, credentials: &Credentials) -> netvideo_client::Result<()> {
        let text = serde_json::to_string(credentials)
            .map_err(|error| ClientError::Store(error.to_string()))?;
        self.0
            .write(server_id.to_owned(), text)
            .map_err(store_error)
    }

    fn remove(&self, server_id: &str) -> netvideo_client::Result<()> {
        self.0.delete(server_id.to_owned()).map_err(store_error)
    }
}
