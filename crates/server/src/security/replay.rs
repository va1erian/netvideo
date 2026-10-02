//! Remembers refresh proofs already used, so a captured proof cannot be
//! replayed while it is still valid.
//!
//! Proofs live at most [`crate::auth::paseto::MAX_REFRESH_PROOF_LIFETIME`],
//! so entries are dropped once they expire. The log is bounded per device,
//! so one device cannot crowd out the others, and in total; past either
//! bound new proofs are refused (fail closed). A restart forgets the log,
//! which the short proof lifetime makes acceptable.

use std::collections::HashMap;
use std::sync::Mutex;

/// Maximum number of live proofs remembered.
const MAX_PROOFS: usize = 4096;

/// Maximum number of live proofs one device may have in the log. A client
/// refreshes about once a week, so this only bites a device that floods.
const MAX_PROOFS_PER_DEVICE: usize = 8;

/// Outcome of [`ProofLog::claim`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The proof is new and is now recorded.
    Accepted,
    /// The proof was already used.
    Replayed,
    /// The device, or the log, holds too many live proofs.
    Full,
}

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
    /// `expires_at` (Unix seconds), unless it was already used or the log
    /// is full.
    pub fn claim(&self, device_id: &str, proof_id: &str, expires_at: i64, now: i64) -> Claim {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|_, exp| *exp >= now);
        let key = (device_id.to_owned(), proof_id.to_owned());
        if seen.contains_key(&key) {
            return Claim::Replayed;
        }
        let device_live = seen.keys().filter(|(id, _)| id == device_id).count();
        if seen.len() >= MAX_PROOFS || device_live >= MAX_PROOFS_PER_DEVICE {
            return Claim::Full;
        }
        seen.insert(key, expires_at);
        Claim::Accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_is_accepted_once() {
        let log = ProofLog::new();
        assert_eq!(log.claim("dev", "p1", 100, 50), Claim::Accepted);
        assert_eq!(log.claim("dev", "p1", 100, 60), Claim::Replayed);
        assert_eq!(log.claim("dev", "p2", 100, 60), Claim::Accepted);
        assert_eq!(log.claim("other", "p1", 100, 60), Claim::Accepted);
    }

    #[test]
    fn one_device_cannot_fill_the_log() {
        let log = ProofLog::new();
        for index in 0..MAX_PROOFS_PER_DEVICE {
            assert_eq!(
                log.claim("dev", &index.to_string(), 100, 0),
                Claim::Accepted
            );
        }
        assert_eq!(log.claim("dev", "late", 100, 0), Claim::Full);
        assert_eq!(log.claim("other", "p", 100, 0), Claim::Accepted);
        assert_eq!(log.claim("dev", "late", 200, 101), Claim::Accepted);
    }

    #[test]
    fn a_full_log_refuses_until_entries_expire() {
        let log = ProofLog::new();
        for index in 0..MAX_PROOFS {
            let device = format!("dev-{index}");
            assert_eq!(log.claim(&device, "p", 100, 0), Claim::Accepted);
        }
        assert_eq!(log.claim("new", "p", 100, 0), Claim::Full);
        assert_eq!(log.claim("new", "p", 200, 101), Claim::Accepted);
    }
}
