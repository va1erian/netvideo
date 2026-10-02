//! Caps concurrent file streams, per device and in total.
//!
//! Each open stream holds a file descriptor and a socket until the client
//! finishes reading, and a client that stops reading holds both forever. One
//! device (or a stolen token) must not be able to exhaust the server's file
//! descriptors and lock everyone else out.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Streams one device may have open at once.
pub const PER_DEVICE_STREAMS: usize = 4;

/// Streams the server serves at once, across all devices.
pub const TOTAL_STREAMS: usize = 32;

/// Hands out stream slots.
#[derive(Debug, Clone)]
pub struct StreamLimiter {
    total: Arc<Semaphore>,
    per_device: Arc<Mutex<HashMap<String, usize>>>,
    device_limit: usize,
}

impl Default for StreamLimiter {
    fn default() -> Self {
        Self::new(PER_DEVICE_STREAMS, TOTAL_STREAMS)
    }
}

impl StreamLimiter {
    /// A limiter with explicit limits.
    pub fn new(device_limit: usize, total: usize) -> Self {
        Self {
            total: Arc::new(Semaphore::new(total)),
            per_device: Arc::new(Mutex::new(HashMap::new())),
            device_limit,
        }
    }

    /// Takes a slot for `device_id`, or `None` when the device or the server
    /// is at its limit. The slot is released when the guard drops.
    pub fn acquire(&self, device_id: &str) -> Option<StreamSlot> {
        let permit = Arc::clone(&self.total).try_acquire_owned().ok()?;
        let mut counts = self.per_device.lock().unwrap_or_else(|e| e.into_inner());
        let count = counts.entry(device_id.to_owned()).or_insert(0);
        if *count >= self.device_limit {
            return None;
        }
        *count += 1;
        Some(StreamSlot {
            _permit: permit,
            device_id: device_id.to_owned(),
            per_device: Arc::clone(&self.per_device),
        })
    }
}

/// One held stream slot.
#[derive(Debug)]
pub struct StreamSlot {
    _permit: OwnedSemaphorePermit,
    device_id: String,
    per_device: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut counts = self.per_device.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = counts.get_mut(&self.device_id) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.device_id);
            }
        }
    }
}

/// A reader that keeps its stream slot until the body is dropped.
pub struct SlotReader<R> {
    inner: R,
    _slot: StreamSlot,
}

impl<R> SlotReader<R> {
    /// Ties `slot` to the lifetime of `inner`.
    pub fn new(inner: R, slot: StreamSlot) -> Self {
        Self { inner, _slot: slot }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for SlotReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_device_and_in_total() {
        let limiter = StreamLimiter::new(2, 3);
        let a1 = limiter.acquire("a").expect("a1");
        let _a2 = limiter.acquire("a").expect("a2");
        assert!(limiter.acquire("a").is_none(), "device limit");
        let _b1 = limiter.acquire("b").expect("b1");
        assert!(limiter.acquire("c").is_none(), "total limit");
        drop(a1);
        assert!(limiter.acquire("c").is_some(), "slot released on drop");
    }

    #[test]
    fn a_refused_device_does_not_leak_a_total_slot() {
        let limiter = StreamLimiter::new(1, 2);
        let _a = limiter.acquire("a").expect("a");
        assert!(limiter.acquire("a").is_none());
        assert!(
            limiter.acquire("b").is_some(),
            "the refused attempt freed its permit"
        );
    }
}
