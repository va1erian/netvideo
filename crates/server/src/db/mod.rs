//! SQLite-backed state: connection pool, migrations and typed queries.

pub mod devices;
pub mod models;
pub mod schema;

use std::path::Path;

use r2d2_sqlite::SqliteConnectionManager;

use crate::error::Result;

/// The database file name inside the data directory.
pub const DB_FILE: &str = "netvideo.db";

/// A pooled connection handle. Cloning is cheap (the pool is reference
/// counted) and every clone is `Send + Sync`.
#[derive(Clone)]
pub struct Db {
    pool: r2d2::Pool<SqliteConnectionManager>,
}

impl Db {
    /// Opens (creating if needed) the database under `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let manager =
            SqliteConnectionManager::file(data_dir.join(DB_FILE)).with_init(configure_connection);
        let pool = r2d2::Pool::builder().max_size(8).build(manager)?;
        let db = Self { pool };
        db.migrate()?;
        Ok(db)
    }

    /// Opens an isolated database at an explicit file path (used by tests).
    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let manager = SqliteConnectionManager::file(path).with_init(configure_connection);
        let pool = r2d2::Pool::builder().max_size(4).build(manager)?;
        let db = Self { pool };
        db.migrate()?;
        Ok(db)
    }

    /// Borrows a pooled connection.
    pub fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>> {
        Ok(self.pool.get()?)
    }
    /// Brings the schema up to [`schema::CURRENT_VERSION`].
    pub fn migrate(&self) -> Result<()> {
        let mut conn = self.conn()?;
        let mut version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        // A negative value indicates local tampering; treat it as empty.
        version = version.max(0);
        if version > schema::CURRENT_VERSION {
            return Err(crate::error::ServerError::Conflict(format!(
                "database schema version {version} is newer than this binary supports ({})",
                schema::CURRENT_VERSION
            )));
        }
        while version < schema::CURRENT_VERSION {
            let statements = schema::MIGRATIONS[version as usize];
            // IMMEDIATE serializes concurrent migrators (e.g. a CLI run while
            // the server boots) instead of racing to create the same table.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute_batch(statements)?;
            tx.pragma_update(None, "user_version", version + 1)?;
            tx.commit()?;
            version += 1;
        }
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES (?1, ?2)",
            rusqlite::params![
                schema::META_LIBRARY_VERSION,
                schema::INITIAL_LIBRARY_VERSION.to_string()
            ],
        )?;
        Ok(())
    }
}

/// Per-connection pragmas: WAL for concurrent readers, foreign keys on,
/// a busy timeout so writers queue instead of failing spuriously.
fn configure_connection(conn: &mut rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA temp_store = MEMORY;",
    )
}
