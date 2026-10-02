//! Runs scans in the background, one at a time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::{ScanStats, Scanner};
use crate::audit;
use crate::db::Db;
use crate::error::Result;

/// Serializes scans: at most one runs at any time.
#[derive(Clone)]
pub struct ScanCoordinator {
    scanner: Arc<Scanner>,
    running: Arc<Mutex<()>>,
}

impl ScanCoordinator {
    /// Wraps `scanner`.
    pub fn new(scanner: Scanner) -> Self {
        Self {
            scanner: Arc::new(scanner),
            running: Arc::new(Mutex::new(())),
        }
    }

    /// Runs a scan now and waits for it. Returns `None` when another scan is
    /// already running.
    pub async fn scan_once(&self, db: &Db) -> Result<Option<ScanStats>> {
        let Ok(_guard) = self.running.try_lock() else {
            return Ok(None);
        };
        self.scan_locked(db).await.map(Some)
    }

    /// Runs a scan; the caller holds the `running` lock.
    async fn scan_locked(&self, db: &Db) -> Result<ScanStats> {
        let started = Instant::now();
        let stats = self.scanner.scan(db).await?;
        audit::scan_finished(&stats, started.elapsed().as_millis() as u64);
        Ok(stats)
    }

    /// Starts a scan in the background. Returns `false` when one is already
    /// running.
    pub fn trigger(&self, db: Db) -> bool {
        // The guard moves into the task, so no other scan can slip in between
        // this check and the scan itself.
        let Ok(guard) = Arc::clone(&self.running).try_lock_owned() else {
            return false;
        };
        let this = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            if let Err(error) = this.scan_locked(&db).await {
                tracing::error!(%error, "library scan failed");
            }
        });
        true
    }

    /// Scans every `interval` in the background; a zero interval disables it.
    pub fn start_periodic(&self, db: Db, interval: Duration) {
        if interval.is_zero() {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // The first tick fires immediately; the startup scan covers it.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(error) = this.scan_once(&db).await {
                    tracing::error!(%error, "periodic library scan failed");
                }
            }
        });
    }
}
