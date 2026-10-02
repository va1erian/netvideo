//! Directory walking for one library root.
//!
//! The walk only stats entries; nothing is opened. Hidden entries (names
//! starting with `.`) and symlinks are skipped, so a link can never pull a
//! file from outside the root into the library. A root that cannot be read,
//! or a subtree that cannot be listed, marks the walk partial so existing rows
//! are never deleted on the strength of a scan that could not see everything.

use std::collections::HashSet;
use std::path::Path;
use std::time::UNIX_EPOCH;

use walkdir::WalkDir;

use super::formats::video_mime;

/// A directory found under a root (the root itself is implied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundDir {
    /// Path relative to the root, using forward slashes.
    pub rel_path: String,
}

/// A video file found under a root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundVideo {
    /// Path relative to the root, using forward slashes.
    pub rel_path: String,
    /// File size in bytes.
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
}

/// Result of walking a single root.
#[derive(Debug, Default)]
pub struct RootWalk {
    /// Directories found, parents before children.
    pub dirs: Vec<FoundDir>,
    /// Video files found.
    pub videos: Vec<FoundVideo>,
    /// Directories with no entries at all, not even hidden or non-video
    /// files (`""` is the root). An unmounted share looks like this.
    pub empty_dirs: HashSet<String>,
    /// Whether the root itself could not be read.
    pub unreachable: bool,
    /// Whether some entry could not be read.
    pub partial: bool,
}

impl RootWalk {
    /// Whether deletions may be derived from this walk.
    pub fn can_delete(&self) -> bool {
        !self.unreachable && !self.partial
    }
}

/// Walks `root`, collecting directories and video files.
pub fn walk_root(root: &Path) -> RootWalk {
    let mut outcome = RootWalk::default();
    if !std::fs::metadata(root).is_ok_and(|meta| meta.is_dir()) {
        outcome.unreachable = true;
        return outcome;
    }

    if is_empty_dir(root) {
        outcome.empty_dirs.insert(String::new());
    }
    let walker = WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| !is_hidden(entry.file_name()));
    for entry in walker {
        let Ok(entry) = entry else {
            outcome.partial = true;
            continue;
        };
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            continue;
        }
        let Some(rel_path) = relative_slash_path(root, entry.path()) else {
            // Non-UTF-8 names cannot be represented in the API.
            tracing::warn!(path = %entry.path().display(), "skipping non-UTF-8 path");
            continue;
        };
        if file_type.is_dir() {
            if is_empty_dir(entry.path()) {
                outcome.empty_dirs.insert(rel_path.clone());
            }
            outcome.dirs.push(FoundDir { rel_path });
        } else if file_type.is_file() && video_mime(&rel_path).is_some() {
            let Ok(metadata) = entry.metadata() else {
                outcome.partial = true;
                continue;
            };
            outcome.videos.push(FoundVideo {
                rel_path,
                size: metadata.len(),
                mtime_ns: unix_mtime_ns(&metadata).unwrap_or(0),
            });
        }
    }
    outcome
}

/// Whether `dir` lists no entries. An unreadable directory is not empty: it
/// already makes the walk partial.
fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

/// Dot-files and dot-directories, including names that are not UTF-8.
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

fn relative_slash_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

/// A file's modification time in nanoseconds since the Unix epoch, when the
/// platform reports one. Sub-second precision catches same-size replacements
/// made within one second.
pub fn unix_mtime_ns(metadata: &std::fs::Metadata) -> Option<i64> {
    let since_epoch = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since_epoch.as_nanos()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn finds_videos_and_dirs_and_skips_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Films/Heat.mkv"));
        touch(&root.join("Films/Heat.nfo"));
        touch(&root.join("clip.MP4"));
        touch(&root.join(".hidden/secret.mkv"));
        touch(&root.join("Films/.part.mkv"));

        let walk = walk_root(root);
        assert!(walk.can_delete());
        let dirs: Vec<_> = walk.dirs.iter().map(|d| d.rel_path.as_str()).collect();
        assert_eq!(dirs, ["Films"]);
        let videos: Vec<_> = walk.videos.iter().map(|v| v.rel_path.as_str()).collect();
        assert_eq!(videos, ["Films/Heat.mkv", "clip.MP4"]);
        assert_eq!(walk.videos[0].size, 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        let outside = tempfile::tempdir().unwrap();
        touch(&outside.path().join("private.mkv"));
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("private.mkv"),
            dir.path().join("link.mkv"),
        )
        .unwrap();

        let walk = walk_root(dir.path());
        assert!(walk.dirs.is_empty());
        assert!(walk.videos.is_empty());
    }

    #[test]
    fn a_missing_root_is_unreachable() {
        let walk = walk_root(Path::new("/definitely/not/here"));
        assert!(walk.unreachable);
        assert!(!walk.can_delete());
    }
}
