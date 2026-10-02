//! SQLite schema migrations for the server store.
//!
//! Migrations are applied in order against `PRAGMA user_version`: entry `i`
//! takes the database from version `i - 1` to `i`. Once released an entry
//! must never be edited — add a new one instead.

/// The schema version this build expects.
pub const CURRENT_VERSION: i64 = 1;

/// Ordered migration statements.
pub const MIGRATIONS: &[&str] = &[
    // v1: devices and pairing codes.
    r"
    CREATE TABLE meta (
        key     TEXT PRIMARY KEY,
        value   TEXT NOT NULL
    );

    CREATE TABLE devices (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        public_key  TEXT NOT NULL,
        paired_at   INTEGER NOT NULL,
        last_seen   INTEGER,
        is_revoked  INTEGER NOT NULL DEFAULT 0,
        is_admin    INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE pairing_codes (
        id           INTEGER PRIMARY KEY,
        code_hash    TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL,
        used         INTEGER NOT NULL DEFAULT 0,
        grants_admin INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_pairing_codes_expires ON pairing_codes(expires_at);
    ",
];

/// Seed the monotonic library version used by delta sync.
pub const INITIAL_LIBRARY_VERSION: i64 = 0;

/// Key under which the library version is stored in `meta`.
pub const META_LIBRARY_VERSION: &str = "library_version";
