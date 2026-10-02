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
use walk::{FoundVideo, RootWalk, walk_root};

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
        let healthy = probe::healthy(&self.ffprobe).await;
        if !healthy {
            tracing::warn!(program = %self.ffprobe.display(), "ffprobe does not run; indexing without metadata");
        }
        let ffprobe_available = Arc::new(AtomicBool::new(healthy));
        let mut stats = ScanStats::default();
        for (index, root) in self.roots.iter().enumerate() {
            // One failing root must not stop the others from being scanned.
            if let Err(error) = self
                .scan_root(db, index as i64, root, &ffprobe_available, &mut stats)
                .await
            {
                tracing::error!(root = %root.display(), %error, "library root scan failed");
                stats.partial = true;
            }
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
        stats.videos_found += walk.videos.len() as u64;

        let known = blocking({
            let db = db.clone();
            move || db.known_videos(root_index)
        })
        .await?;
        let shielded = shielded_videos(&walk, &known);
        if !shielded.is_empty() {
            tracing::warn!(
                root = %root.display(),
                videos = shielded.len(),
                "folders that held videos are now empty (unmounted?); keeping their rows"
            );
        }
        stats.partial |= walk.partial || !shielded.is_empty();
        let pending: Vec<&FoundVideo> = walk
            .videos
            .iter()
            .filter(|video| needs_write(video, known.get(&video.rel_path)))
            .collect();
        let (probes, transient) = self
            .probe_all(root, &pending, &known, ffprobe_available)
            .await;

        let mut upserts = Vec::new();
        for video in pending {
            let metadata = probes.get(&video.rel_path).cloned();
            let known_video = known.get(&video.rel_path);
            let probe_failed = metadata.is_none()
                && ffprobe_available.load(Ordering::Relaxed)
                && !transient.contains(&video.rel_path);
            if probe_failed {
                stats.probe_failures += 1;
            } else if metadata.is_none() && known_video.is_some_and(|k| is_unchanged(video, k)) {
                // No verdict (ffprobe missing or a transient error) and the
                // file is unchanged: nothing new to store.
                continue;
            }
            upserts.push(VideoUpsert {
                rel_path: video.rel_path.clone(),
                size: video.size as i64,
                mtime_ns: video.mtime_ns,
                metadata,
                probe_failed,
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
                .chain(shielded)
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
    /// Returns the metadata found, and the files that hit a transient error
    /// and must not be marked as failed.
    async fn probe_all(
        &self,
        root: &Path,
        pending: &[&FoundVideo],
        known: &HashMap<String, KnownVideo>,
        ffprobe_available: &Arc<AtomicBool>,
    ) -> (HashMap<String, ProbeInfo>, HashSet<String>) {
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
                    Ok(info) => Some((rel, Some(info))),
                    Err(probe::ProbeError::Transient(error)) => {
                        tracing::warn!(file = %rel, %error, "probe deferred to the next scan");
                        Some((rel, None))
                    }
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
        let (mut results, mut transient) = (HashMap::new(), HashSet::new());
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Some((rel, Some(info)))) => {
                    results.insert(rel, info);
                }
                Ok(Some((rel, None))) => {
                    transient.insert(rel);
                }
                _ => {}
            }
        }
        (results, transient)
    }
}

/// Known videos the walk did not see, under a folder that still exists but
/// has no entries at all. An unmounted share or a missing bind mount looks
/// exactly like that, at the root or deeper, and pruning would delete every
/// row and id beneath it. A folder that still holds anything (subtitles,
/// posters, hidden files) is not shielded: its missing videos were deleted.
fn shielded_videos(walk: &RootWalk, known: &HashMap<String, KnownVideo>) -> Vec<String> {
    let seen: HashSet<&str> = walk.videos.iter().map(|v| v.rel_path.as_str()).collect();
    known
        .keys()
        .filter(|rel| !seen.contains(rel.as_str()))
        .filter(|rel| ancestors(rel).any(|dir| walk.empty_dirs.contains(dir)))
        .cloned()
        .collect()
}

/// Proper ancestors of a relative path, nearest first, ending with `""`
/// (the root). The root itself has none.
pub(crate) fn ancestors(rel: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(rel);
    std::iter::from_fn(move || {
        let path = rest.filter(|path| !path.is_empty())?;
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        rest = Some(parent);
        Some(parent)
    })
}

/// Whether a found video needs its row written or its metadata read: new or
/// changed files always, unchanged ones only when they were never probed. A file ffprobe already failed on waits until it changes.
fn needs_write(video: &FoundVideo, known: Option<&KnownVideo>) -> bool {
    known.is_none_or(|known| !is_unchanged(video, known) || (!known.probed && !known.probe_failed))
}

fn is_unchanged(video: &FoundVideo, known: &KnownVideo) -> bool {
    known.size == video.size as i64 && known.mtime_ns == video.mtime_ns
}

/// The root folder's display name: its last component. Never the absolute
/// path, which would reveal the server's layout to clients.
fn root_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "library".to_owned())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ServerError::Io(std::io::Error::other(error)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_walk_up_to_the_root() {
        assert_eq!(ancestors("a/b/c.mkv").collect::<Vec<_>>(), ["a/b", "a", ""]);
        assert_eq!(ancestors("c.mkv").collect::<Vec<_>>(), [""]);
        assert_eq!(ancestors("").count(), 0);
    }
}
