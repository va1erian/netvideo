//! Library writes: what the scanner knows about a root, and applying a scan.

use std::collections::{HashMap, HashSet};

use rand::RngCore;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::db::Db;
use crate::db::models::ProbeInfo;
use crate::error::Result;

/// A video row as the scanner needs it to decide whether to re-probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownVideo {
    /// File size at the last scan.
    pub size: i64,
    /// Modification time at the last scan.
    pub mtime: i64,
    /// Whether ffprobe metadata is stored.
    pub probed: bool,
}

/// One video to insert or update.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoUpsert {
    /// Path relative to the root.
    pub rel_path: String,
    /// File size in bytes.
    pub size: i64,
    /// Modification time (Unix seconds).
    pub mtime: i64,
    /// Probe result; `None` stores the video without metadata.
    pub metadata: Option<ProbeInfo>,
}

/// Everything one scan learned about one root.
#[derive(Debug, Clone, Default)]
pub struct RootUpdate {
    /// Index of the root in the configuration.
    pub root_index: i64,
    /// Display name of the root folder.
    pub root_name: String,
    /// Every directory found (relative paths).
    pub dirs: Vec<String>,
    /// Every video found (relative paths), changed or not.
    pub seen_videos: HashSet<String>,
    /// Videos whose row must be written.
    pub upserts: Vec<VideoUpsert>,
    /// Whether rows not seen by this scan may be deleted.
    pub prune: bool,
}

/// Row counts changed by [`Db::apply_root`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RootChanges {
    /// Videos inserted or updated.
    pub written: u64,
    /// Videos deleted.
    pub removed: u64,
}

impl Db {
    /// Known videos of a root, keyed by relative path.
    pub fn known_videos(&self, root_index: i64) -> Result<HashMap<String, KnownVideo>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT rel_path, size, mtime, probed FROM videos WHERE root_index = ?1")?;
        let rows = stmt.query_map([root_index], |row| {
            Ok((
                row.get::<_, String>(0)?,
                KnownVideo {
                    size: row.get(1)?,
                    mtime: row.get(2)?,
                    probed: row.get::<_, i64>(3)? != 0,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Applies one root's scan in a single transaction.
    pub fn apply_root(&self, update: &RootUpdate) -> Result<RootChanges> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mut folders = folder_ids(&tx, update.root_index)?;
        ensure_folder(&tx, update, "", &mut folders)?;
        let mut dirs: Vec<&str> = update.dirs.iter().map(String::as_str).collect();
        // Parents sort before their children.
        dirs.sort_by_key(|rel| rel.matches('/').count());
        for rel in &dirs {
            ensure_folder(&tx, update, rel, &mut folders)?;
        }

        let mut changes = RootChanges::default();
        for video in &update.upserts {
            let parent = parent_of(&video.rel_path);
            ensure_folder(&tx, update, parent, &mut folders)?;
            upsert_video(&tx, update.root_index, &folders[parent], video)?;
            changes.written += 1;
        }

        if update.prune {
            changes.removed = prune(&tx, update, &dirs)?;
        }
        tx.commit()?;
        Ok(changes)
    }

    /// Deletes every row of roots at or beyond `root_count` (removed from the
    /// configuration).
    pub fn drop_roots_from(&self, root_count: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM videos WHERE root_index >= ?1", [root_count])?;
        conn.execute("DELETE FROM folders WHERE root_index >= ?1", [root_count])?;
        Ok(())
    }
}

fn folder_ids(tx: &Transaction<'_>, root_index: i64) -> Result<HashMap<String, String>> {
    let mut stmt = tx.prepare("SELECT rel_path, id FROM folders WHERE root_index = ?1")?;
    let rows = stmt.query_map([root_index], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn ensure_folder(
    tx: &Transaction<'_>,
    update: &RootUpdate,
    rel: &str,
    folders: &mut HashMap<String, String>,
) -> Result<()> {
    if folders.contains_key(rel) {
        return Ok(());
    }
    let (parent_id, name) = if rel.is_empty() {
        (None, update.root_name.as_str())
    } else {
        let parent = parent_of(rel);
        ensure_folder(tx, update, parent, folders)?;
        (Some(folders[parent].clone()), file_name(rel))
    };
    let id = new_id();
    tx.execute(
        "INSERT INTO folders (id, root_index, rel_path, parent_id, name) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, update.root_index, rel, parent_id, name],
    )?;
    folders.insert(rel.to_owned(), id);
    Ok(())
}

fn upsert_video(
    tx: &Transaction<'_>,
    root_index: i64,
    folder_id: &str,
    video: &VideoUpsert,
) -> Result<()> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM videos WHERE root_index = ?1 AND rel_path = ?2",
            params![root_index, video.rel_path],
            |row| row.get(0),
        )
        .optional()?;
    let meta = video.metadata.as_ref();
    let probed = meta.is_some() as i64;
    let container = meta.and_then(|m| m.container.clone());
    let duration = meta.and_then(|m| m.duration_ms);
    let bitrate = meta.and_then(|m| m.bitrate);
    let id = match existing {
        Some(id) => {
            tx.execute(
                "UPDATE videos SET folder_id = ?2, size = ?3, mtime = ?4, probed = ?5,
                 container = ?6, duration_ms = ?7, bitrate = ?8 WHERE id = ?1",
                params![
                    id,
                    folder_id,
                    video.size,
                    video.mtime,
                    probed,
                    container,
                    duration,
                    bitrate
                ],
            )?;
            tx.execute("DELETE FROM streams WHERE video_id = ?1", [&id])?;
            id
        }
        None => {
            let id = new_id();
            tx.execute(
                "INSERT INTO videos (id, folder_id, root_index, rel_path, name, size, mtime,
                 probed, container, duration_ms, bitrate)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id,
                    folder_id,
                    root_index,
                    video.rel_path,
                    file_name(&video.rel_path),
                    video.size,
                    video.mtime,
                    probed,
                    container,
                    duration,
                    bitrate
                ],
            )?;
            id
        }
    };
    for stream in meta.map(|m| m.streams.as_slice()).unwrap_or_default() {
        tx.execute(
            "INSERT OR REPLACE INTO streams (video_id, idx, kind, codec, profile, width, height,
             fps, channels, language, title, is_default, is_forced)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                stream.index,
                stream.kind.as_str(),
                stream.codec,
                stream.profile,
                stream.width,
                stream.height,
                stream.fps,
                stream.channels,
                stream.language,
                stream.title,
                stream.is_default as i64,
                stream.is_forced as i64
            ],
        )?;
    }
    Ok(())
}

/// Deletes videos and folders of the root that the scan did not see.
fn prune(tx: &Transaction<'_>, update: &RootUpdate, dirs: &[&str]) -> Result<u64> {
    let mut removed = 0;
    let videos: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT id, rel_path FROM videos WHERE root_index = ?1")?;
        let rows = stmt.query_map([update.root_index], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (id, rel) in videos {
        if !update.seen_videos.contains(&rel) {
            tx.execute("DELETE FROM videos WHERE id = ?1", [id])?;
            removed += 1;
        }
    }
    let keep: HashSet<&str> = dirs
        .iter()
        .copied()
        .chain(std::iter::once(""))
        .chain(update.seen_videos.iter().map(|rel| parent_of(rel)))
        .collect();
    for (rel, id) in folder_ids(tx, update.root_index)? {
        if !keep.contains(rel.as_str()) {
            tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        }
    }
    Ok(removed)
}

/// The parent of a relative path (`""` for top-level entries).
fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn file_name(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, name)| name)
}

/// A random 128-bit id, hex-encoded. Ids reveal nothing about order or size.
fn new_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers() {
        assert_eq!(parent_of("a/b/c.mkv"), "a/b");
        assert_eq!(parent_of("c.mkv"), "");
        assert_eq!(file_name("a/b/c.mkv"), "c.mkv");
        assert_eq!(new_id().len(), 32);
        assert_ne!(new_id(), new_id());
    }
}
