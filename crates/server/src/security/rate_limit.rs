//! In-memory sliding-window rate limiting.
//!
//! Used to blunt brute-force attempts against the pairing endpoint. Keys are
//! opaque strings (typically `scope:client-ip`); the limiter never stores the
//! client's secrets, only timestamps. The map is bounded so a flood of unique
//! source addresses cannot grow server memory without limit; when it is full
//! of live entries, new keys are refused (fail closed) rather than resetting
//! everyone's history.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Maximum number of distinct keys tracked; beyond it new keys are refused.
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

    /// Returns `true` if `key` still has room under `limit` per `window`,
    /// without recording an attempt. Pair with [`RateLimiter::check`] to count
    /// only some outcomes (for example failures).
    pub fn has_capacity(&self, key: &str, limit: u32, window: Duration) -> bool {
        self.has_capacity_at(key, limit, window, Instant::now())
    }

    /// Clock-injectable variant used by tests.
    pub fn check_at(&self, key: &str, limit: u32, window: Duration, now: Instant) -> bool {
        let mut map = self.lock();
        if map.len() >= MAX_KEYS && !map.contains_key(key) {
            prune_stale(&mut map, now, window);
            if map.len() >= MAX_KEYS {
                // Fail closed: a flood of unique keys must not reset others.
                return false;
            }
        }

        let entries = map.entry(key.to_owned()).or_default();
        expire(entries, now, window);
        if entries.len() as u32 >= limit {
            return false;
        }
        entries.push_back(now);
        true
    }

    /// Clock-injectable variant of [`RateLimiter::has_capacity`].
    pub fn has_capacity_at(&self, key: &str, limit: u32, window: Duration, now: Instant) -> bool {
        let mut map = self.lock();
        match map.get_mut(key) {
            Some(entries) => {
                expire(entries, now, window);
                (entries.len() as u32) < limit
            }
            None => limit > 0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, VecDeque<Instant>>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The rate-limit identity of a client address. IPv6 clients are keyed by
/// their /64, since one host typically controls a whole /64; IPv4-mapped
/// IPv6 addresses are keyed as the IPv4 address.
pub fn client_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
    }
}

fn expire(entries: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    if let Some(cutoff) = now.checked_sub(window) {
        while entries.front().is_some_and(|at| *at <= cutoff) {
            entries.pop_front();
        }
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

    #[test]
    fn a_full_map_refuses_new_keys_but_keeps_existing_history() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        assert!(limiter.check_at("victim", 2, window, now));
        assert!(limiter.check_at("victim", 2, window, now));
        for index in 0..MAX_KEYS {
            limiter.check_at(&format!("flood-{index}"), 1, window, now);
        }
        assert!(
            !limiter.check_at("victim", 2, window, now),
            "a flood must not reset an existing key's history"
        );
        assert!(!limiter.check_at("newcomer", 2, window, now));
        let later = now + Duration::from_secs(61);
        assert!(limiter.check_at("newcomer", 2, window, later));
    }

    #[test]
    fn has_capacity_does_not_record() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        assert!(limiter.has_capacity_at("k", 1, window, now));
        assert!(limiter.has_capacity_at("k", 1, window, now));
        assert!(limiter.check_at("k", 1, window, now));
        assert!(!limiter.has_capacity_at("k", 1, window, now));
    }

    #[test]
    fn ipv6_clients_share_their_slash_64() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(client_key(a), client_key(b));
        assert_ne!(client_key(a), client_key(c));
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(client_key(mapped), "192.0.2.7");
    }
}
