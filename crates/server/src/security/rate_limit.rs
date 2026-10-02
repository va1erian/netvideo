//! In-memory sliding-window rate limiting.
//!
//! Used to blunt brute-force attempts against the pairing endpoint. Keys are
//! opaque strings (typically `scope:client-ip`); the limiter never stores the
//! client's secrets, only timestamps. The map is bounded so a flood of unique
//! source addresses cannot grow server memory without limit.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Maximum number of distinct keys tracked before the oldest are pruned.
const MAX_KEYS: usize = 4096;

/// A per-key sliding-window limiter.
#[derive(Debug, Default)]
pub struct RateLimiter {
    inner: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    /// Creates an empty limiter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an attempt for `key` at the current time. Returns `true` if it
    /// is within `limit` per `window`.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> bool {
        self.check_at(key, limit, window, Instant::now())
    }

    /// Clock-injectable variant used by tests.
    pub fn check_at(&self, key: &str, limit: u32, window: Duration, now: Instant) -> bool {
        let mut map = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if map.len() >= MAX_KEYS {
            prune_stale(&mut map, now, window);
            if map.len() >= MAX_KEYS {
                map.clear();
            }
        }

        let cutoff = now.checked_sub(window);
        let entries = map.entry(key.to_owned()).or_default();
        if let Some(cutoff) = cutoff {
            while entries.front().is_some_and(|at| *at <= cutoff) {
                entries.pop_front();
            }
        }
        if entries.len() as u32 >= limit {
            return false;
        }
        entries.push_back(now);
        true
    }
}

fn prune_stale(map: &mut HashMap<String, VecDeque<Instant>>, now: Instant, window: Duration) {
    let Some(cutoff) = now.checked_sub(window) else {
        return;
    };
    map.retain(|_, entries| {
        while entries.front().is_some_and(|at| *at <= cutoff) {
            entries.pop_front();
        }
        !entries.is_empty()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_limit_then_blocks() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        assert!(limiter.check_at("ip", 3, window, now));
        assert!(limiter.check_at("ip", 3, window, now));
        assert!(limiter.check_at("ip", 3, window, now));
        assert!(!limiter.check_at("ip", 3, window, now));
    }

    #[test]
    fn window_slides_forward() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        let window = Duration::from_secs(60);
        assert!(limiter.check_at("ip", 1, window, start));
        assert!(!limiter.check_at("ip", 1, window, start + Duration::from_secs(30)));
        assert!(limiter.check_at("ip", 1, window, start + Duration::from_secs(61)));
    }

    #[test]
    fn keys_are_independent() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        assert!(limiter.check_at("a", 1, window, now));
        assert!(limiter.check_at("b", 1, window, now));
    }
}
