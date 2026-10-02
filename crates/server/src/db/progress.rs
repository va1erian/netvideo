//! Playback progress, kept per device.

use rusqlite::{OptionalExtension, params};

use crate::db::Db;
use crate::db::models::Progress;
use crate::error::Result;

impl Db {
    /// Saves `device_id`'s progress on `video_id`. `watched` keeps its
    /// stored value when `None` (false for a new row). Returns `false` when
    /// the video does not exist.
    pub fn save_progress(
        &self,
        device_id: &str,
        video_id: &str,
        position_ms: i64,
        watched: Option<bool>,
        now: i64,
    ) -> Result<bool> {
        let conn = self.conn()?;
        // One statement, so a video pruned concurrently cannot leave an
        // orphan row (the foreign key would reject it anyway).
        let written = conn.execute(
            "INSERT INTO progress (device_id, video_id, position_ms, watched, updated_at)
             SELECT ?1, id, ?3, COALESCE(?4, 0), ?5 FROM videos WHERE id = ?2
             ON CONFLICT (device_id, video_id) DO UPDATE SET
                 position_ms = excluded.position_ms,
                 watched = COALESCE(?4, watched),
                 updated_at = excluded.updated_at",
            params![device_id, video_id, position_ms, watched, now],
        )?;
        Ok(written > 0)
    }

    /// `device_id`'s progress on `video_id`, if any.
    pub fn progress(&self, device_id: &str, video_id: &str) -> Result<Option<Progress>> {
        let conn = self.conn()?;
        let progress = conn
            .query_row(
                "SELECT position_ms, watched, updated_at FROM progress
                 WHERE device_id = ?1 AND video_id = ?2",
                [device_id, video_id],
                |row| {
                    Ok(Progress {
                        position_ms: row.get(0)?,
                        watched: row.get::<_, i64>(1)? != 0,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(progress)
    }
}
