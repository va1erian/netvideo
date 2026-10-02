//! Remembers refresh proofs already used, so a captured proof cannot be
//! replayed while it is still valid.
//!
//! Proofs live at most [`crate::auth::paseto::MAX_REFRESH_PROOF_LIFETIME`],
//! so entries are dropped once they expire. The log is bounded; when it is
//! full of live entries new proofs are refused (fail closed). A restart
//! forgets the log, which the short proof lifetime makes acceptable.

use std::collections::HashMap;
use std::sync::Mutex;

/// Maximum number of live proofs remembered.
const MAX_PROOFS: usize = 4096;

/// Used refresh proofs, keyed by device and proof id, with their expiry.
#[derive(Debug, Default)]
pub struct ProofLog {
    seen: Mutex<HashMap<(String, String), i64>>,
}

impl ProofLog {
    /// Creates an empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the proof `proof_id` of `device_id`, which expires at
    /// `expires_at` (Unix seconds). Returns `false` if it was already used,
    /// or if the log is full of live proofs.
    pub fn claim(&self, device_id: &str, proof_id: &str, expires_at: i64, now: i64) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let key = (device_id.to_owned(), proof_id.to_owned());
        if seen.get(&key).is_some_and(|exp| *exp >= now) {
            return false;
        }
        if seen.len() >= MAX_PROOFS {
            seen.retain(|_, exp| *exp >= now);
            if seen.len() >= MAX_PROOFS {
                return false;
            }
        }
        seen.insert(key, expires_at);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_is_accepted_once() {
        let log = ProofLog::new();
        assert!(log.claim("dev", "p1", 100, 50));
        assert!(!log.claim("dev", "p1", 100, 60));
        assert!(log.claim("dev", "p2", 100, 60));
        assert!(log.claim("other", "p1", 100, 60), "ids are per device");
    }

    #[test]
    fn a_full_log_refuses_until_entries_expire() {
        let log = ProofLog::new();
        for index in 0..MAX_PROOFS {
            assert!(log.claim("dev", &index.to_string(), 100, 0));
        }
        assert!(!log.claim("dev", "late", 100, 0));
        assert!(log.claim("dev", "late", 200, 101));
    }
}
