use anyhow::Result;
use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    MIGRATION_001,
    MIGRATION_002,
    MIGRATION_003,
    MIGRATION_004,
    MIGRATION_005,
    MIGRATION_006,
];

const MIGRATION_001: &str = r#"
-- =============================================
-- NOOR Database Schema v1
-- =============================================

-- Migration tracking
CREATE TABLE IF NOT EXISTS _migrations (
    id INTEGER PRIMARY KEY,
    applied_at TEXT DEFAULT (datetime('now'))
);

-- Artists
CREATE TABLE artists (
    id INTEGER PRIMARY KEY,
    tidal_id INTEGER UNIQUE,
    ytmusic_id TEXT UNIQUE,
    soundcloud_id INTEGER UNIQUE,
    name TEXT NOT NULL,
    name_sort TEXT,
    biography TEXT,
    photo_url TEXT,
    created_at TEXT DEFAULT (datetime('now'))
);

-- Albums
CREATE TABLE albums (
    id INTEGER PRIMARY KEY,
    tidal_id INTEGER UNIQUE,
    ytmusic_id TEXT UNIQUE,
    title TEXT NOT NULL,
    artist_id INTEGER REFERENCES artists(id),
    year INTEGER,
    artwork_url TEXT,
    artwork_cached_path TEXT,
    release_type TEXT DEFAULT 'album',
    label TEXT,
    track_count INTEGER,
    source TEXT NOT NULL DEFAULT 'tidal',
    created_at TEXT DEFAULT (datetime('now'))
);

-- Tracks
CREATE TABLE tracks (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    title_sort TEXT,
    artist_id INTEGER NOT NULL REFERENCES artists(id),
    album_id INTEGER REFERENCES albums(id),
    disc_number INTEGER DEFAULT 1,
    track_number INTEGER,
    duration_ms INTEGER,
    isrc TEXT,
    -- Service IDs
    tidal_id INTEGER UNIQUE,
    ytmusic_id TEXT UNIQUE,
    soundcloud_id INTEGER UNIQUE,
    -- Local file
    file_path TEXT,
    file_format TEXT,
    sample_rate INTEGER,
    bit_depth INTEGER,
    -- Quality
    best_quality TEXT,
    best_source TEXT,
    fidelity_score INTEGER DEFAULT 0,
    -- Library state
    is_favorite INTEGER DEFAULT 0,
    play_count INTEGER DEFAULT 0,
    last_played_at TEXT,
    date_added TEXT DEFAULT (datetime('now')),
    source TEXT NOT NULL DEFAULT 'tidal',
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

-- Genres (hierarchical)
CREATE TABLE genres (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    slug TEXT NOT NULL UNIQUE,
    parent_id INTEGER REFERENCES genres(id),
    tidal_genre_id TEXT
);

-- Track <-> Genre (many-to-many)
CREATE TABLE track_genres (
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    genre_id INTEGER NOT NULL REFERENCES genres(id),
    source TEXT DEFAULT 'tidal',
    confidence REAL DEFAULT 1.0,
    PRIMARY KEY (track_id, genre_id)
);

-- Playlists
CREATE TABLE playlists (
    id INTEGER PRIMARY KEY,
    tidal_uuid TEXT UNIQUE,
    name TEXT NOT NULL,
    description TEXT,
    is_smart INTEGER DEFAULT 0,
    smart_rules TEXT,
    is_synced INTEGER DEFAULT 1,
    track_count INTEGER DEFAULT 0,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    position INTEGER NOT NULL,
    added_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (playlist_id, position)
);

-- Playback state
CREATE TABLE playback_state (
    id INTEGER PRIMARY KEY DEFAULT 1,
    current_track_id INTEGER REFERENCES tracks(id),
    position_ms INTEGER DEFAULT 0,
    is_playing INTEGER DEFAULT 0,
    volume REAL DEFAULT 1.0,
    shuffle_mode TEXT DEFAULT 'off',
    repeat_mode TEXT DEFAULT 'off',
    automix_enabled INTEGER DEFAULT 0,
    crossfade_ms INTEGER DEFAULT 0
);

-- Insert default playback state
INSERT INTO playback_state (id) VALUES (1);

-- Queue
CREATE TABLE queue (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    position INTEGER NOT NULL,
    source TEXT DEFAULT 'user'
);

-- Shuffle state
CREATE TABLE shuffle_state (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id),
    played_at TEXT DEFAULT (datetime('now'))
);

-- Listen history
CREATE TABLE listen_history (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    started_at TEXT NOT NULL,
    duration_listened_ms INTEGER,
    completed INTEGER DEFAULT 0
);

-- Duplicate groups
CREATE TABLE duplicate_groups (
    id INTEGER PRIMARY KEY,
    status TEXT DEFAULT 'pending'
);

CREATE TABLE duplicate_members (
    group_id INTEGER NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    is_preferred INTEGER DEFAULT 0,
    PRIMARY KEY (group_id, track_id)
);

-- Service auth (one row per service)
CREATE TABLE service_auth (
    service TEXT PRIMARY KEY,
    access_token_enc BLOB,
    refresh_token_enc BLOB,
    token_expiry TEXT,
    user_id TEXT,
    subscription_type TEXT,
    extra_data TEXT,
    connected_at TEXT DEFAULT (datetime('now'))
);

-- Discovery Cloud presets
CREATE TABLE discovery_presets (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    words TEXT NOT NULL,
    services TEXT DEFAULT '["tidal"]',
    created_at TEXT DEFAULT (datetime('now'))
);

-- Discovery results cache
CREATE TABLE discovery_results (
    id INTEGER PRIMARY KEY,
    preset_id INTEGER REFERENCES discovery_presets(id),
    track_title TEXT,
    artist_name TEXT,
    service TEXT,
    service_track_id TEXT,
    relevance_score REAL,
    preview_url TEXT,
    added_to_library INTEGER DEFAULT 0,
    discovered_at TEXT DEFAULT (datetime('now'))
);

-- Full-text search indexes
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title,
    content='tracks',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE VIRTUAL TABLE artists_fts USING fts5(
    name,
    content='artists',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE VIRTUAL TABLE albums_fts USING fts5(
    title,
    content='albums',
    content_rowid='id',
    tokenize='unicode61'
);

-- FTS triggers: keep FTS in sync with content tables
CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title) VALUES (new.id, new.title);
END;
CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title) VALUES('delete', old.id, old.title);
END;
CREATE TRIGGER tracks_au AFTER UPDATE OF title ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title) VALUES('delete', old.id, old.title);
    INSERT INTO tracks_fts(rowid, title) VALUES (new.id, new.title);
END;

CREATE TRIGGER artists_ai AFTER INSERT ON artists BEGIN
    INSERT INTO artists_fts(rowid, name) VALUES (new.id, new.name);
END;
CREATE TRIGGER artists_ad AFTER DELETE ON artists BEGIN
    INSERT INTO artists_fts(artists_fts, rowid, name) VALUES('delete', old.id, old.name);
END;
CREATE TRIGGER artists_au AFTER UPDATE OF name ON artists BEGIN
    INSERT INTO artists_fts(artists_fts, rowid, name) VALUES('delete', old.id, old.name);
    INSERT INTO artists_fts(rowid, name) VALUES (new.id, new.name);
END;

CREATE TRIGGER albums_ai AFTER INSERT ON albums BEGIN
    INSERT INTO albums_fts(rowid, title) VALUES (new.id, new.title);
END;
CREATE TRIGGER albums_ad AFTER DELETE ON albums BEGIN
    INSERT INTO albums_fts(albums_fts, rowid, title) VALUES('delete', old.id, old.title);
END;
CREATE TRIGGER albums_au AFTER UPDATE OF title ON albums BEGIN
    INSERT INTO albums_fts(albums_fts, rowid, title) VALUES('delete', old.id, old.title);
    INSERT INTO albums_fts(rowid, title) VALUES (new.id, new.title);
END;

-- Performance indexes
CREATE INDEX idx_tracks_tidal ON tracks(tidal_id);
CREATE INDEX idx_tracks_artist ON tracks(artist_id);
CREATE INDEX idx_tracks_album ON tracks(album_id);
CREATE INDEX idx_tracks_fidelity ON tracks(fidelity_score DESC);
CREATE INDEX idx_tracks_source ON tracks(source);
CREATE INDEX idx_tracks_date_added ON tracks(date_added);
CREATE INDEX idx_albums_artist ON albums(artist_id);
CREATE INDEX idx_albums_year ON albums(year);
CREATE INDEX idx_queue_position ON queue(position);
CREATE INDEX idx_listen_history_track ON listen_history(track_id);
CREATE INDEX idx_listen_history_time ON listen_history(started_at);
CREATE INDEX idx_track_genres_genre ON track_genres(genre_id);
CREATE INDEX idx_genres_parent ON genres(parent_id);
CREATE INDEX idx_playlist_tracks_track ON playlist_tracks(track_id);
"#;

const MIGRATION_002: &str = r#"
ALTER TABLE discovery_presets ADD COLUMN mode TEXT NOT NULL DEFAULT 'mood';
"#;

const MIGRATION_003: &str = r#"
ALTER TABLE albums ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_albums_favorite ON albums(is_favorite);
"#;

const MIGRATION_005: &str = r#"
ALTER TABLE playback_state ADD COLUMN automix_discover_new INTEGER NOT NULL DEFAULT 0;
"#;

const MIGRATION_006: &str = r#"
CREATE TABLE IF NOT EXISTS sync_metadata (
    service TEXT PRIMARY KEY,
    last_sync_at TEXT DEFAULT (datetime('now')),
    auto_sync_daily INTEGER NOT NULL DEFAULT 0,
    last_sync_track_count INTEGER DEFAULT 0,
    last_sync_album_count INTEGER DEFAULT 0
);
-- Seed TIDAL sync row
INSERT OR IGNORE INTO sync_metadata (service, last_sync_at, auto_sync_daily)
VALUES ('tidal', datetime('now'), 0);
"#;

const MIGRATION_004: &str = r#"
CREATE TABLE IF NOT EXISTS musicbrainz_checked (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    checked_at TEXT DEFAULT (datetime('now'))
);
-- Backfill: tracks that already have musicbrainz genre data don't need re-querying.
INSERT OR IGNORE INTO musicbrainz_checked (track_id)
SELECT DISTINCT track_id FROM track_genres WHERE source = 'musicbrainz';
"#;

pub fn run_migrations(conn: &Connection) -> Result<()> {
    // Create migrations table if not exists
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            applied_at TEXT DEFAULT (datetime('now'))
        );",
    )?;

    let applied: i64 = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
        .unwrap_or(0);

    for (i, migration) in MIGRATIONS.iter().enumerate() {
        let migration_id = (i + 1) as i64;
        if migration_id > applied {
            conn.execute_batch(migration)?;
            conn.execute("INSERT INTO _migrations (id) VALUES (?1)", [migration_id])?;
            tracing::info!("Applied migration {}", migration_id);
        }
    }

    Ok(())
}
