//! SQLite schema migrations for the server store.
//!
//! Migrations are applied in order against `PRAGMA user_version`: entry `i`
//! takes the database from version `i - 1` to `i`. Once released an entry
//! must never be edited — add a new one instead.

/// The schema version this build expects.
pub const CURRENT_VERSION: i64 = MIGRATIONS.len() as i64;

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
    // v2: the library, mirroring the filesystem.
    r"
    CREATE TABLE library_roots (
        root_index  INTEGER PRIMARY KEY,
        path        TEXT NOT NULL UNIQUE
    );

    CREATE TABLE folders (
        id          TEXT PRIMARY KEY,
        root_index  INTEGER NOT NULL,
        rel_path    TEXT NOT NULL,
        parent_id   TEXT REFERENCES folders(id) ON DELETE CASCADE,
        name        TEXT NOT NULL
    );
    CREATE UNIQUE INDEX idx_folders_path ON folders(root_index, rel_path);
    CREATE INDEX idx_folders_parent ON folders(parent_id, name COLLATE NOCASE);

    CREATE TABLE videos (
        id          TEXT PRIMARY KEY,
        folder_id   TEXT NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
        root_index  INTEGER NOT NULL,
        rel_path    TEXT NOT NULL,
        name        TEXT NOT NULL,
        size        INTEGER NOT NULL,
        mtime_ns    INTEGER NOT NULL,
        probed      INTEGER NOT NULL DEFAULT 0,
        probe_failed INTEGER NOT NULL DEFAULT 0,
        container   TEXT,
        duration_ms INTEGER,
        bitrate     INTEGER
    );
    CREATE UNIQUE INDEX idx_videos_path ON videos(root_index, rel_path);
    CREATE INDEX idx_videos_folder ON videos(folder_id, name COLLATE NOCASE);

    CREATE TABLE streams (
        video_id    TEXT NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
        idx         INTEGER NOT NULL,
        kind        TEXT NOT NULL,
        codec       TEXT,
        profile     TEXT,
        width       INTEGER,
        height      INTEGER,
        fps         REAL,
        channels    INTEGER,
        language    TEXT,
        title       TEXT,
        is_default  INTEGER NOT NULL DEFAULT 0,
        is_forced   INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (video_id, idx)
    );
    ",
    // v3: playback progress, per device.
    r"
    CREATE TABLE progress (
        device_id   TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
        video_id    TEXT NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
        position_ms INTEGER NOT NULL,
        watched     INTEGER NOT NULL DEFAULT 0,
        updated_at  INTEGER NOT NULL,
        PRIMARY KEY (device_id, video_id)
    );
    CREATE INDEX idx_progress_video ON progress(video_id);
    ",
];

/// Seed the monotonic library version used by delta sync.
pub const INITIAL_LIBRARY_VERSION: i64 = 0;

/// Key under which the library version is stored in `meta`.
pub const META_LIBRARY_VERSION: &str = "library_version";
