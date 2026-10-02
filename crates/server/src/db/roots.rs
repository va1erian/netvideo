//! Keeps library rows attached to the right root when the configuration
//! changes.
//!
//! Rows store a root's position in `library.paths`, so reordering, inserting
//! or removing a root would otherwise make an old video id resolve to the same
//! relative path under a different root. Reconciliation runs before the
//! server serves anything: rows follow their root's path to its new position,
//! and rows of roots no longer configured are deleted.

use std::path::{Path, PathBuf};

use rusqlite::{TransactionBehavior, params};

use crate::db::Db;
use crate::error::Result;

/// The stable identity of a configured root: its normalized path.
pub fn root_identity(path: &Path) -> String {
    path.components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

impl Db {
    /// Re-keys library rows to the configured root order. Returns how many
    /// previously known roots were dropped.
    pub fn reconcile_roots(&self, paths: &[PathBuf]) -> Result<usize> {
        let wanted: Vec<String> = paths.iter().map(|path| root_identity(path)).collect();
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: Vec<(i64, String)> = {
            let mut stmt = tx.prepare("SELECT root_index, path FROM library_roots")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let unchanged = stored.len() == wanted.len()
            && stored
                .iter()
                .all(|(index, path)| wanted.get(*index as usize) == Some(path));
        if unchanged {
            return Ok(0);
        }

        // Park every row at a negative index, move the ones whose root is
        // still configured, and delete what is left.
        for table in ["folders", "videos"] {
            tx.execute(
                &format!("UPDATE {table} SET root_index = -1 - root_index"),
                [],
            )?;
        }
        let mut dropped = 0;
        for (old, path) in &stored {
            match wanted.iter().position(|candidate| candidate == path) {
                Some(new) => {
                    for table in ["folders", "videos"] {
                        tx.execute(
                            &format!("UPDATE {table} SET root_index = ?1 WHERE root_index = ?2"),
                            params![new as i64, -1 - old],
                        )?;
                    }
                }
                None => dropped += 1,
            }
        }
        // Folders cascade to their videos; rows of roots never recorded (a
        // database from before this table) are dropped too.
        tx.execute("DELETE FROM videos WHERE root_index < 0", [])?;
        tx.execute("DELETE FROM folders WHERE root_index < 0", [])?;
        tx.execute("DELETE FROM library_roots", [])?;
        for (index, path) in wanted.iter().enumerate() {
            tx.execute(
                "INSERT INTO library_roots (root_index, path) VALUES (?1, ?2)",
                params![index as i64, path],
            )?;
        }
        tx.commit()?;
        Ok(dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::library::{RootUpdate, VideoUpsert};

    fn scan_one(db: &Db, root_index: i64, name: &str, video: &str) {
        db.apply_root(&RootUpdate {
            root_index,
            root_name: name.into(),
            seen_videos: [video.to_owned()].into(),
            upserts: vec![VideoUpsert {
                rel_path: video.into(),
                size: 1,
                mtime_ns: 1,
                metadata: None,
                probe_failed: false,
            }],
            prune: true,
            ..RootUpdate::default()
        })
        .unwrap();
    }

    fn video_root(db: &Db, rel: &str) -> Option<i64> {
        use rusqlite::OptionalExtension;
        db.conn()
            .unwrap()
            .query_row(
                "SELECT root_index FROM videos WHERE rel_path = ?1",
                [rel],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    }

    #[test]
    fn rows_follow_their_root_when_the_configuration_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_at(&dir.path().join("t.db")).unwrap();
        let films = PathBuf::from("/media/films");
        let shows = PathBuf::from("/media/shows");
        let extra = PathBuf::from("/media/extra");
        assert_eq!(
            db.reconcile_roots(&[films.clone(), shows.clone()]).unwrap(),
            0
        );
        scan_one(&db, 0, "films", "heat.mkv");
        scan_one(&db, 1, "shows", "e01.mkv");

        // Reordered, with a new root first: rows move with their path.
        let dropped = db
            .reconcile_roots(&[extra.clone(), shows.clone(), films.clone()])
            .unwrap();
        assert_eq!(dropped, 0);
        assert_eq!(video_root(&db, "heat.mkv"), Some(2));
        assert_eq!(video_root(&db, "e01.mkv"), Some(1));

        // A removed root takes its rows with it.
        assert_eq!(db.reconcile_roots(&[films]).unwrap(), 2);
        assert_eq!(video_root(&db, "heat.mkv"), Some(0));
        assert_eq!(video_root(&db, "e01.mkv"), None);
    }

    #[test]
    fn identity_ignores_trailing_slashes_and_dots() {
        assert_eq!(root_identity(Path::new("/media/films/")), "/media/films");
        assert_eq!(root_identity(Path::new("/media/./films")), "/media/films");
    }
}
