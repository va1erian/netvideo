//! Library reads for the browse API.

use rusqlite::{OptionalExtension, params};

use crate::db::Db;
use crate::db::models::{
    FolderPage, FolderRef, Progress, StreamInfo, StreamKind, VideoDetail, VideoLocation,
    VideoSummary,
};
use crate::error::Result;
use crate::scanner::formats::video_mime;

/// Deepest folder chain walked when building a breadcrumb.
const MAX_DEPTH: usize = 256;

impl Db {
    /// The root folders, in configuration order.
    pub fn roots(&self) -> Result<Vec<FolderRef>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT id, name FROM folders WHERE parent_id IS NULL ORDER BY root_index")?;
        let rows = stmt.query_map([], folder_ref)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One page of a folder: subfolders, then videos, each sorted by name.
    /// `offset` counts entries across both lists. `None` for an unknown id.
    pub fn folder_page(
        &self,
        id: &str,
        device_id: &str,
        offset: u64,
        limit: u64,
    ) -> Result<Option<FolderPage>> {
        let conn = self.conn()?;
        let Some(folder) = conn
            .query_row(
                "SELECT id, name FROM folders WHERE id = ?1",
                [id],
                folder_ref,
            )
            .optional()?
        else {
            return Ok(None);
        };

        let folder_count: u64 = conn.query_row(
            "SELECT COUNT(*) FROM folders WHERE parent_id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let video_count: u64 = conn.query_row(
            "SELECT COUNT(*) FROM videos WHERE folder_id = ?1",
            [id],
            |row| row.get(0),
        )?;

        let mut folders = Vec::new();
        if offset < folder_count {
            let mut stmt = conn.prepare(
                "SELECT id, name FROM folders WHERE parent_id = ?1
                 ORDER BY name COLLATE NOCASE, name LIMIT ?2 OFFSET ?3",
            )?;
            let rows = stmt.query_map(params![id, limit, offset], folder_ref)?;
            folders = rows.collect::<rusqlite::Result<_>>()?;
        }
        let remaining = limit - folders.len() as u64;
        let video_offset = offset.saturating_sub(folder_count);
        let mut videos = Vec::new();
        if remaining > 0 && video_offset < video_count {
            let mut stmt = conn.prepare(
                "SELECT v.id, v.name, v.size, v.duration_ms, s.codec, s.width, s.height,
                 p.position_ms, p.watched, p.updated_at
                 FROM videos v
                 LEFT JOIN streams s ON s.video_id = v.id AND s.idx = (
                     SELECT MIN(idx) FROM streams WHERE video_id = v.id AND kind = 'video')
                 LEFT JOIN progress p ON p.video_id = v.id AND p.device_id = ?4
                 WHERE v.folder_id = ?1
                 ORDER BY v.name COLLATE NOCASE, v.name LIMIT ?2 OFFSET ?3",
            )?;
            let args = params![id, remaining, video_offset, device_id];
            let rows = stmt.query_map(args, |row| {
                Ok(VideoSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    size: row.get(2)?,
                    duration_ms: row.get(3)?,
                    video_codec: row.get(4)?,
                    width: row.get(5)?,
                    height: row.get(6)?,
                    progress: progress_at(row, 7)?,
                })
            })?;
            videos = rows.collect::<rusqlite::Result<_>>()?;
        }

        let end = offset + folders.len() as u64 + videos.len() as u64;
        let next_cursor = (end < folder_count + video_count).then(|| end.to_string());
        Ok(Some(FolderPage {
            path: ancestors(&conn, id)?,
            folder,
            folders,
            videos,
            next_cursor,
        }))
    }

    /// Full details of a video, with `device_id`'s progress, or `None` for an
    /// unknown id.
    pub fn video_detail(&self, id: &str, device_id: &str) -> Result<Option<VideoDetail>> {
        let conn = self.conn()?;
        let detail = conn
            .query_row(
                "SELECT v.id, v.name, v.folder_id, v.size, v.mtime_ns, v.probed, v.container,
                 v.duration_ms, v.bitrate, p.position_ms, p.watched, p.updated_at
                 FROM videos v
                 LEFT JOIN progress p ON p.video_id = v.id AND p.device_id = ?2
                 WHERE v.id = ?1",
                [id, device_id],
                |row| {
                    let name: String = row.get(1)?;
                    Ok(VideoDetail {
                        id: row.get(0)?,
                        mime: video_mime(&name)
                            .unwrap_or("application/octet-stream")
                            .to_owned(),
                        name,
                        folder_id: row.get(2)?,
                        size: row.get(3)?,
                        mtime_ns: row.get(4)?,
                        probed: row.get::<_, i64>(5)? != 0,
                        container: row.get(6)?,
                        duration_ms: row.get(7)?,
                        bitrate: row.get(8)?,
                        streams: Vec::new(),
                        progress: progress_at(row, 9)?,
                    })
                },
            )
            .optional()?;
        let Some(mut detail) = detail else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT idx, kind, codec, profile, width, height, fps, channels, language, title,
             is_default, is_forced FROM streams WHERE video_id = ?1 ORDER BY idx",
        )?;
        let rows = stmt.query_map([id], |row| {
            let kind: String = row.get(1)?;
            let Some(kind) = StreamKind::parse(&kind) else {
                return Ok(None);
            };
            Ok(Some(StreamInfo {
                index: row.get(0)?,
                kind,
                codec: row.get(2)?,
                profile: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                fps: row.get(6)?,
                channels: row.get(7)?,
                language: row.get(8)?,
                title: row.get(9)?,
                is_default: row.get::<_, i64>(10)? != 0,
                is_forced: row.get::<_, i64>(11)? != 0,
            }))
        })?;
        for stream in rows {
            detail.streams.extend(stream?);
        }
        Ok(Some(detail))
    }

    /// Where a video lives on disk, or `None` for an unknown id.
    pub fn video_location(&self, id: &str) -> Result<Option<VideoLocation>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                "SELECT root_index, rel_path, size, mtime_ns FROM videos WHERE id = ?1",
                [id],
                |row| {
                    Ok(VideoLocation {
                        root_index: row.get(0)?,
                        rel_path: row.get(1)?,
                        size: row.get(2)?,
                        mtime_ns: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }
}

fn folder_ref(row: &rusqlite::Row<'_>) -> rusqlite::Result<FolderRef> {
    Ok(FolderRef {
        id: row.get(0)?,
        name: row.get(1)?,
    })
}

/// The folder's ancestors, root first, excluding the folder itself.
fn ancestors(conn: &rusqlite::Connection, id: &str) -> Result<Vec<FolderRef>> {
    let mut path = Vec::new();
    let mut current: Option<String> = conn
        .query_row("SELECT parent_id FROM folders WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?
        .flatten();
    while let Some(parent) = current {
        if path.len() >= MAX_DEPTH {
            break;
        }
        let (folder, next): (FolderRef, Option<String>) = conn.query_row(
            "SELECT id, name, parent_id FROM folders WHERE id = ?1",
            [&parent],
            |row| Ok((folder_ref(row)?, row.get(2)?)),
        )?;
        path.push(folder);
        current = next;
    }
    path.reverse();
    Ok(path)
}

/// Reads the `position_ms, watched, updated_at` columns starting at `first`,
/// which are all NULL when the device has no progress.
fn progress_at(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Option<Progress>> {
    let Some(position_ms) = row.get::<_, Option<i64>>(first)? else {
        return Ok(None);
    };
    Ok(Some(Progress {
        position_ms,
        watched: row.get::<_, i64>(first + 1)? != 0,
        updated_at: row.get(first + 2)?,
    }))
}
