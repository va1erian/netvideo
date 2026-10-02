//! The library scanner: walks each root, probes new or changed videos with
//! ffprobe and writes the result, mirroring the filesystem as folders and
//! videos.

pub mod coordinator;
pub mod formats;
pub mod probe;
pub mod walk;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::db::Db;
use crate::db::library::{KnownVideo, RootUpdate, VideoUpsert};
use crate::db::models::ProbeInfo;
use crate::error::{Result, ServerError};
use walk::{FoundVideo, walk_root};

pub use coordinator::ScanCoordinator;

/// Concurrent ffprobe processes. Kept low: the target host is a small CPU.
const PROBE_CONCURRENCY: usize = 2;

/// Totals for one scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Video files found across all reachable roots.
    pub videos_found: u64,
    /// Video rows inserted or updated.
    pub written: u64,
    /// Video rows deleted.
    pub removed: u64,
    /// Videos ffprobe could not read.
    pub probe_failures: u64,
    /// Whether some root was unreachable or only partly readable.
    pub partial: bool,
}

/// Scans the configured library roots.
#[derive(Debug, Clone)]
pub struct Scanner {
    roots: Vec<PathBuf>,
    ffprobe: PathBuf,
}

impl Scanner {
    /// A scanner over `roots`, probing with the ffprobe at `ffprobe`.
    pub fn new(roots: Vec<PathBuf>, ffprobe: PathBuf) -> Self {
        Self { roots, ffprobe }
    }

    /// Runs one full scan.
    pub async fn scan(&self, db: &Db) -> Result<ScanStats> {
        let root_count = self.roots.len() as i64;
        blocking({
            let db = db.clone();
            move || db.drop_roots_from(root_count)
        })
        .await?;

        let ffprobe_available = Arc::new(AtomicBool::new(true));
        let mut stats = ScanStats::default();
        for (index, root) in self.roots.iter().enumerate() {
            self.scan_root(db, index as i64, root, &ffprobe_available, &mut stats)
                .await?;
        }
        Ok(stats)
    }

    async fn scan_root(
        &self,
        db: &Db,
        root_index: i64,
        root: &Path,
        ffprobe_available: &Arc<AtomicBool>,
        stats: &mut ScanStats,
    ) -> Result<()> {
        let walk = blocking({
            let root = root.to_path_buf();
            move || Ok(walk_root(&root))
        })
        .await?;
        if walk.unreachable {
            tracing::warn!(root = %root.display(), "library root is unreachable; keeping its rows");
            stats.partial = true;
            return Ok(());
        }
        stats.partial |= walk.partial;
        stats.videos_found += walk.videos.len() as u64;

        let known = blocking({
            let db = db.clone();
            move || db.known_videos(root_index)
        })
        .await?;
        let pending: Vec<&FoundVideo> = walk
            .videos
            .iter()
            .filter(|video| needs_write(video, known.get(&video.rel_path)))
            .collect();
        let probes = self
            .probe_all(root, &pending, &known, ffprobe_available)
            .await;

        let mut upserts = Vec::new();
        for video in pending {
            let metadata = probes.get(&video.rel_path).cloned();
            let known_video = known.get(&video.rel_path);
            if metadata.is_none() && known_video.is_some_and(|k| is_unchanged(video, k)) {
                // Still unprobed and unchanged: nothing new to store.
                continue;
            }
            if metadata.is_none() && ffprobe_available.load(Ordering::Relaxed) {
                stats.probe_failures += 1;
            }
            upserts.push(VideoUpsert {
                rel_path: video.rel_path.clone(),
                size: video.size as i64,
                mtime: video.mtime,
                metadata,
            });
        }

        let update = RootUpdate {
            root_index,
            root_name: root_name(root),
            dirs: walk.dirs.into_iter().map(|dir| dir.rel_path).collect(),
            seen_videos: walk
                .videos
                .into_iter()
                .map(|video| video.rel_path)
                .collect(),
            upserts,
            prune: !walk.partial,
        };
        let changes = blocking({
            let db = db.clone();
            move || db.apply_root(&update)
        })
        .await?;
        stats.written += changes.written;
        stats.removed += changes.removed;
        Ok(())
    }

    /// Probes the pending videos that need metadata, a few at a time.
    async fn probe_all(
        &self,
        root: &Path,
        pending: &[&FoundVideo],
        known: &HashMap<String, KnownVideo>,
        ffprobe_available: &Arc<AtomicBool>,
    ) -> HashMap<String, ProbeInfo> {
        let semaphore = Arc::new(Semaphore::new(PROBE_CONCURRENCY));
        let mut tasks = JoinSet::new();
        let mut queued = HashSet::new();
        for video in pending {
            let unchanged_and_probed = known
                .get(&video.rel_path)
                .is_some_and(|k| k.probed && is_unchanged(video, k));
            if unchanged_and_probed || !queued.insert(video.rel_path.clone()) {
                continue;
            }
            let (semaphore, available) = (Arc::clone(&semaphore), Arc::clone(ffprobe_available));
            let program = self.ffprobe.clone();
            let path = root.join(&video.rel_path);
            let rel = video.rel_path.clone();
            tasks.spawn(async move {
                let _permit = semaphore.acquire_owned().await.ok()?;
                if !available.load(Ordering::Relaxed) {
                    return None;
                }
                match probe::probe(&program, &path).await {
                    Ok(info) => Some((rel, info)),
                    Err(probe::ProbeError::Unavailable(error)) => {
                        if available.swap(false, Ordering::Relaxed) {
                            tracing::warn!(%error, "ffprobe unavailable; indexing without metadata");
                        }
                        None
                    }
                    Err(error) => {
                        tracing::warn!(file = %rel, %error, "cannot probe video");
                        None
                    }
                }
            });
        }
        let mut results = HashMap::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(Some((rel, info))) = joined {
                results.insert(rel, info);
            }
        }
        results
    }
}

/// Whether a found video needs its row written or its metadata read.
fn needs_write(video: &FoundVideo, known: Option<&KnownVideo>) -> bool {
    known.is_none_or(|known| !known.probed || !is_unchanged(video, known))
}

fn is_unchanged(video: &FoundVideo, known: &KnownVideo) -> bool {
    known.size == video.size as i64 && known.mtime == video.mtime
}

fn root_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| root.display().to_string(), str::to_owned)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ServerError::Io(std::io::Error::other(error)))?
}
