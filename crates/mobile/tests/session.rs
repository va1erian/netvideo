//! `MobileSession` over an in-memory vault. The pairing and browse flow
//! itself is covered end to end by `netvideo-client`'s tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use netvideo_mobile::{MobileError, MobileSession, SecretVault};

#[derive(Default)]
struct MemoryVault(Mutex<HashMap<String, String>>);

impl SecretVault for MemoryVault {
    fn read(&self, key: String) -> Result<Option<String>, MobileError> {
        Ok(self.0.lock().unwrap().get(&key).cloned())
    }
    fn write(&self, key: String, value: String) -> Result<(), MobileError> {
        self.0.lock().unwrap().insert(key, value);
        Ok(())
    }
    fn delete(&self, key: String) -> Result<(), MobileError> {
        self.0.lock().unwrap().remove(&key);
        Ok(())
    }
}

struct BrokenVault;

impl SecretVault for BrokenVault {
    fn read(&self, _: String) -> Result<Option<String>, MobileError> {
        Err(MobileError::Other {
            detail: "keystore locked".into(),
        })
    }
    fn write(&self, _: String, _: String) -> Result<(), MobileError> {
        unreachable!()
    }
    fn delete(&self, _: String) -> Result<(), MobileError> {
        unreachable!()
    }
}

const URL: &str = "http://127.0.0.1:9";

#[test]
fn an_unpaired_session_never_touches_the_network() {
    let session = MobileSession::new(URL.into(), Arc::new(MemoryVault::default())).unwrap();
    assert!(!session.is_paired());
    assert!(matches!(session.roots(), Err(MobileError::NotPaired)));
    assert!(matches!(
        session.authorization(),
        Err(MobileError::NotPaired)
    ));
    assert!(
        session
            .file_url("abc".into())
            .ends_with("/api/v1/videos/abc/file")
    );
}

#[test]
fn stored_credentials_are_read_from_the_vault_and_forgotten() {
    let vault = Arc::new(MemoryVault::default());
    let id = key();
    let credentials = format!(
        r#"{{"device_id":"d1","device_name":"tv","secret":"k4.secret.x","token":"t","expires_at":{}}}"#,
        i64::MAX / 2
    );
    vault.write(id.clone(), credentials).unwrap();

    let session = MobileSession::new(URL.into(), vault.clone()).unwrap();
    assert_eq!(session.device_id().as_deref(), Some("d1"));
    assert_eq!(session.authorization().unwrap(), "Bearer t");
    // The server is unreachable: revocation fails, the local forget does not.
    session.forget().unwrap();
    assert!(vault.read(id).unwrap().is_none());
    assert!(!session.is_paired());
}

#[test]
fn vault_failures_surface_as_errors() {
    let error = MobileSession::new(URL.into(), Arc::new(BrokenVault))
        .err()
        .expect("vault error");
    assert!(error.to_string().contains("keystore locked"), "{error}");
}

#[test]
fn corrupt_credentials_are_reported() {
    let vault = Arc::new(MemoryVault::default());
    vault.write(key(), "{".into()).unwrap();
    let error = MobileSession::new(URL.into(), vault)
        .err()
        .expect("corrupt");
    assert!(error.to_string().contains("corrupt credentials"), "{error}");
}

/// The vault key a session uses: its endpoint id.
fn key() -> String {
    netvideo_client::ServerEndpoint::new("server", URL)
        .unwrap()
        .id
}
