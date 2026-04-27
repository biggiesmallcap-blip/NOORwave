use anyhow::Result;
use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    MIGRATION_001,
    MIGRATION_002,
    MIGRATION_003,
    MIGRATION_004,
    MIGRATION_005,
    MIGRATION_006,
    MIGRATION_007,
    MIGRATION_008,
    MIGRATION_009,
    MIGRATION_010,
    MIGRATION_011,
    MIGRATION_012,
    MIGRATION_013,
    MIGRATION_014,
    MIGRATION_015,
    MIGRATION_016,
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

const MIGRATION_007: &str = r#"
-- Track similarity: pre-computed co-listen + metadata similarity pairs
-- Built from: playlist co-occurrence, album co-occurrence, listen session overlap,
--             shared genre branches, shared artist appearances
CREATE TABLE IF NOT EXISTS track_similarity (
    track_a INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    track_b INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    similarity_score REAL NOT NULL DEFAULT 0,
    -- Component scores (each 0-1, for debugging/tuning):
    co_listen_score REAL DEFAULT 0,      -- from shared playlist/session appearance
    co_album_score REAL DEFAULT 0,       -- from same album (tracks on same album are similar)
    co_artist_score REAL DEFAULT 0,      -- same artist or frequent collaborator
    genre_proximity REAL DEFAULT 0,       -- shared genre taxonomy branches
    duration_proximity REAL DEFAULT 0,   -- similar length
    era_proximity REAL DEFAULT 0,        -- similar year/era
    -- Metadata:
    computed_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (track_a, track_b),
    CHECK (track_a < track_b)  -- canonical ordering, no duplicates
);
CREATE INDEX idx_track_similarity_a ON track_similarity(track_a, similarity_score DESC);
CREATE INDEX idx_track_similarity_b ON track_similarity(track_b, similarity_score DESC);
CREATE INDEX idx_track_similarity_score ON track_similarity(similarity_score DESC);
"#;

const MIGRATION_008: &str = r#"
CREATE TABLE IF NOT EXISTS embedding_models (
    id INTEGER PRIMARY KEY,
    model_key TEXT NOT NULL UNIQUE,
    family TEXT NOT NULL,
    dimension INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'idle',
    is_active INTEGER NOT NULL DEFAULT 0,
    trained_at TEXT,
    config_json TEXT,
    metrics_json TEXT,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS training_runs (
    id INTEGER PRIMARY KEY,
    model_id INTEGER REFERENCES embedding_models(id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    progress REAL NOT NULL DEFAULT 0,
    items_total INTEGER,
    items_done INTEGER NOT NULL DEFAULT 0,
    started_at TEXT DEFAULT (datetime('now')),
    finished_at TEXT,
    error_text TEXT
);

CREATE TABLE IF NOT EXISTS track_embeddings (
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    model_id INTEGER NOT NULL REFERENCES embedding_models(id) ON DELETE CASCADE,
    vector_blob BLOB NOT NULL,
    l2_norm REAL NOT NULL DEFAULT 0,
    generated_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (track_id, model_id)
);

CREATE TABLE IF NOT EXISTS track_audio_features (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    feature_version TEXT NOT NULL,
    vector_blob BLOB NOT NULL,
    clip_start_ms INTEGER NOT NULL DEFAULT 30000,
    clip_duration_ms INTEGER NOT NULL DEFAULT 20000,
    computed_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS track_neighbors (
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    neighbor_track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    model_id INTEGER NOT NULL REFERENCES embedding_models(id) ON DELETE CASCADE,
    rank INTEGER NOT NULL,
    score REAL NOT NULL DEFAULT 0,
    behavioral_score REAL DEFAULT 0,
    audio_score REAL DEFAULT 0,
    metadata_score REAL DEFAULT 0,
    reason_json TEXT,
    computed_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (track_id, neighbor_track_id, model_id)
);

CREATE TABLE IF NOT EXISTS playback_transitions (
    id INTEGER PRIMARY KEY,
    from_track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    to_track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    transition_source TEXT NOT NULL,
    completed_prev INTEGER NOT NULL DEFAULT 0,
    gap_ms INTEGER NOT NULL DEFAULT 0,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS discovery_feedback (
    id INTEGER PRIMARY KEY,
    seed_track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    candidate_track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    action TEXT NOT NULL,
    surface TEXT NOT NULL,
    context_json TEXT,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_training_runs_model ON training_runs(model_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_track_embeddings_model ON track_embeddings(model_id);
CREATE INDEX IF NOT EXISTS idx_track_neighbors_track ON track_neighbors(track_id, model_id, rank);
CREATE INDEX IF NOT EXISTS idx_track_neighbors_neighbor ON track_neighbors(neighbor_track_id, model_id);
CREATE INDEX IF NOT EXISTS idx_playback_transitions_from ON playback_transitions(from_track_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_playback_transitions_to ON playback_transitions(to_track_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_discovery_feedback_seed ON discovery_feedback(seed_track_id, created_at DESC);

ALTER TABLE playback_state ADD COLUMN automix_use_learning INTEGER NOT NULL DEFAULT 1;
ALTER TABLE playback_state ADD COLUMN automix_allow_external INTEGER NOT NULL DEFAULT 0;
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

const MIGRATION_009: &str = r#"
-- DSP feature store: extracted audio features per track
CREATE TABLE IF NOT EXISTS audio_dsp_features (
    track_id          INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    bpm               REAL,
    key_signature     TEXT,        -- "Am", "Cmaj"
    camelot_key       TEXT,        -- "8A", "8B"
    loudness_lufs     REAL,
    energy            REAL,        -- 0.0 - 1.0
    danceability      REAL,        -- 0.0 - 1.0
    beat_strength     REAL,        -- 0.0 - 1.0
    spectral_centroid REAL,
    stereo_width      REAL,        -- 0.0 (mono) - 1.0 (wide)
    is_instrumental   INTEGER DEFAULT 0,
    analysis_source   TEXT NOT NULL DEFAULT 'noor_dsp',
    analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
    samples_analyzed  INTEGER,
    analyzed_at       TEXT DEFAULT (datetime('now')),
    analysis_version   TEXT NOT NULL DEFAULT '1.0'
);

CREATE INDEX IF NOT EXISTS idx_dsp_bpm ON audio_dsp_features(bpm);
CREATE INDEX IF NOT EXISTS idx_dsp_key ON audio_dsp_features(key_signature);
CREATE INDEX IF NOT EXISTS idx_dsp_energy ON audio_dsp_features(energy);

-- Audio fingerprint storage (for duplicate detection via fingerprint matching)
CREATE TABLE IF NOT EXISTS audio_fingerprints (
    track_id    INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    hashes_blob BLOB,          -- compact binary fingerprint
    peak_count  INTEGER
);

-- Individual fingerprint hash entries for fast lookup
CREATE TABLE IF NOT EXISTS fingerprint_hashes (
    hash         INTEGER NOT NULL,
    track_id     INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    time_offset  INTEGER NOT NULL,
    PRIMARY KEY (hash, track_id, time_offset)
);

CREATE INDEX IF NOT EXISTS idx_fingerprint_hash ON fingerprint_hashes(hash);

-- ACRCloud recognition results cache
CREATE TABLE IF NOT EXISTS acrcloud_results (
    id                INTEGER PRIMARY KEY,
    track_id          INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    original_title    TEXT,
    original_artist   TEXT,
    original_album    TEXT,
    original_year     INTEGER,
    confidence_score  REAL,
    sample_start_ms   INTEGER,
    sample_end_ms     INTEGER,
    isrc              TEXT,
    matched_at        TEXT DEFAULT (datetime('now')),
    api_response_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_acrcloud_track ON acrcloud_results(track_id);
"#;

const MIGRATION_010: &str = r#"
ALTER TABLE duplicate_groups ADD COLUMN source TEXT;
ALTER TABLE duplicate_groups ADD COLUMN confidence REAL;
"#;

const MIGRATION_011: &str = r#"
CREATE TABLE IF NOT EXISTS server_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

const MIGRATION_012: &str = r#"
CREATE TABLE IF NOT EXISTS spotify_checked (
    track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    checked_at TEXT DEFAULT (datetime('now'))
);
-- Backfill: tracks already tagged by spotify don't need re-querying.
INSERT OR IGNORE INTO spotify_checked (track_id)
SELECT DISTINCT track_id FROM track_genres WHERE source = 'spotify';
"#;

const MIGRATION_013: &str = r#"
CREATE TABLE IF NOT EXISTS lastfm_checked (
    track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    checked_at TEXT DEFAULT (datetime('now'))
);
-- Backfill: tracks already tagged by lastfm don't need re-querying.
INSERT OR IGNORE INTO lastfm_checked (track_id)
SELECT DISTINCT track_id FROM track_genres WHERE source = 'lastfm';
"#;

const MIGRATION_014: &str = r#"
CREATE TABLE IF NOT EXISTS lastfm_artist_cache (
    artist_name TEXT PRIMARY KEY,
    tags_json   TEXT NOT NULL,
    fetched_at  TEXT DEFAULT (datetime('now'))
);
"#;

const MIGRATION_015: &str = r#"
CREATE TABLE IF NOT EXISTS lastfm_unresolved_tags (
    tag        TEXT PRIMARY KEY,
    seen_count INTEGER NOT NULL DEFAULT 1,
    last_seen  TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

const MIGRATION_016: &str = r#"
ALTER TABLE discovery_feedback ADD COLUMN session_id TEXT;
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
