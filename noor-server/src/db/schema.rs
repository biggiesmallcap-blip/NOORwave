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
    MIGRATION_017,
    MIGRATION_018,
    MIGRATION_019,
    MIGRATION_020,
    MIGRATION_021,
    MIGRATION_022,
    MIGRATION_023,
    MIGRATION_024,
    MIGRATION_025,
    MIGRATION_026,
    MIGRATION_027,
    MIGRATION_028,
    MIGRATION_029,
    MIGRATION_030,
    MIGRATION_031,
    MIGRATION_032,
    MIGRATION_033,
    MIGRATION_034,
    MIGRATION_035,
    MIGRATION_036,
    MIGRATION_037,
    MIGRATION_038,
    MIGRATION_039,
    MIGRATION_040,
    MIGRATION_041,
    MIGRATION_042,
    MIGRATION_043,
    MIGRATION_044,
    MIGRATION_045,
    MIGRATION_046,
    MIGRATION_047,
    MIGRATION_048,
    MIGRATION_049,
    MIGRATION_050,
    MIGRATION_051,
    MIGRATION_052,
    MIGRATION_053,
    MIGRATION_054,
    MIGRATION_055,
    MIGRATION_056,
    MIGRATION_057,
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
    -- Reserved-but-empty: Tidal v1 API doesn't expose per-genre IDs on the
    -- endpoints we use. See docs/tidal-genre-source-investigation.md.
    tidal_genre_id TEXT
);

-- Track <-> Genre (many-to-many)
-- Note: source='tidal' is reserved-but-unpopulated in practice; lastfm and
-- musicbrainz are the active sources. See docs/tidal-genre-source-investigation.md.
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

const MIGRATION_017: &str = r#"
ALTER TABLE playlists ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_playlists_favorite ON playlists(is_favorite);
"#;

// Phase 2b: per-queue-item provenance string. Radio writes a structured
// "why is this here" reason on insert; automix-extended items carry NULL
// until automix migrates in a later phase. Frontend renders the reason
// in a queue-row tooltip and tolerates NULL gracefully.
const MIGRATION_018: &str = r#"
ALTER TABLE queue ADD COLUMN reason TEXT;
"#;

// Sample track context for unresolved last.fm tags, so taxonomy curation
// can spot-check what kind of music produces a given un-mappable tag.
// Nullable because pre-existing rows from migration 015 don't have it.
const MIGRATION_019: &str = r#"
ALTER TABLE lastfm_unresolved_tags ADD COLUMN last_track_id INTEGER;
"#;

// Phase 2c-ii-a: track current queue item ID separately from current_track_id
// so pending rows (track_id = NULL) can be the "current" item without a FK violation.
const MIGRATION_021: &str = r#"
ALTER TABLE playback_state ADD COLUMN current_queue_item_id INTEGER;
"#;

// Phase 2c-ii-a: allow non-library (pending last.fm) entries in the queue.
// Rebuilds queue to make track_id nullable and adds pending-row metadata.
// SQLite requires a full table rebuild to drop NOT NULL on track_id.
// Existing library rows are preserved; new columns are NULL for them.
const MIGRATION_020: &str = r#"
ALTER TABLE queue RENAME TO _queue_v019;

CREATE TABLE queue (
    id                  INTEGER PRIMARY KEY,
    track_id            INTEGER REFERENCES tracks(id),
    position            INTEGER NOT NULL,
    source              TEXT    DEFAULT 'user',
    reason              TEXT,
    pending_artist      TEXT,
    pending_title       TEXT,
    pending_at          TIMESTAMP,
    resolving_at        TIMESTAMP,
    resolved_at         TIMESTAMP,
    tidal_match_score   REAL
);

INSERT INTO queue (id, track_id, position, source, reason)
SELECT id, track_id, position, source, reason
FROM _queue_v019;

DROP TABLE _queue_v019;

CREATE INDEX idx_queue_position ON queue(position);
CREATE INDEX idx_queue_pending  ON queue(track_id, pending_at);
"#;

// Discovery + Radio Tier 1: confidence, support, in-degree, play-count columns on neighbor edges.
// Defaults are 0/0.0 so existing rows remain valid until next training run repopulates them.
const MIGRATION_022: &str = r#"
ALTER TABLE track_neighbors ADD COLUMN confidence REAL NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN support_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN candidate_in_degree INTEGER NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN candidate_in_degree_percentile REAL NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN play_count_seed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN play_count_candidate INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_track_neighbors_candidate_in_degree_pct
    ON track_neighbors(candidate_in_degree_percentile);
CREATE INDEX IF NOT EXISTS idx_track_neighbors_confidence
    ON track_neighbors(confidence);
"#;

// Discovery + Radio Tier 1: session/source/position/transition context on listen_history.
// All columns nullable: backfill populates historical rows; live writes populate new rows.
const MIGRATION_023: &str = r#"
ALTER TABLE listen_history ADD COLUMN session_id TEXT;
ALTER TABLE listen_history ADD COLUMN source TEXT;
ALTER TABLE listen_history ADD COLUMN position_in_session INTEGER;
ALTER TABLE listen_history ADD COLUMN transition_from_track_id INTEGER;

CREATE INDEX IF NOT EXISTS idx_listen_history_session_position
    ON listen_history(session_id, position_in_session);
CREATE INDEX IF NOT EXISTS idx_listen_history_source_started
    ON listen_history(source, started_at);
CREATE INDEX IF NOT EXISTS idx_listen_history_transition_from
    ON listen_history(transition_from_track_id);
"#;

// Discovery + Radio Tier 1: primary_reason on track_neighbors (argmax of reason_json weights).
// Null until the next training run promotes existing reason_json arrays.
const MIGRATION_024: &str = r#"
ALTER TABLE track_neighbors ADD COLUMN primary_reason TEXT;

CREATE INDEX IF NOT EXISTS idx_track_neighbors_primary_reason
    ON track_neighbors(primary_reason);
"#;

// Discovery + Radio Tier 2: radio_diagnostics table + initial feature-flag values.
// All five behavior flags + the kill-switch default to "false". orchestrate_song
// runs the new pipeline with all behavior gated off, producing legacy-equivalent output
// until an operator flips a flag in server_config.
const MIGRATION_025: &str = r#"
CREATE TABLE IF NOT EXISTS radio_diagnostics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    seed_track_id INTEGER,
    profile_name TEXT NOT NULL,
    creativity REAL NOT NULL,
    queue_size INTEGER NOT NULL,
    target_library_weight REAL,
    target_lastfm_weight REAL,
    target_engine_weight REAL,
    actual_library_count INTEGER NOT NULL DEFAULT 0,
    actual_lastfm_count INTEGER NOT NULL DEFAULT 0,
    actual_engine_count INTEGER NOT NULL DEFAULT 0,
    avg_confidence REAL,
    avg_candidate_in_degree_pct REAL,
    same_artist_penalties INTEGER NOT NULL DEFAULT 0,
    same_album_penalties INTEGER NOT NULL DEFAULT 0,
    genre_saturation_penalties INTEGER NOT NULL DEFAULT 0,
    repetition_skips INTEGER NOT NULL DEFAULT 0,
    penalty_relaxations INTEGER NOT NULL DEFAULT 0,
    hub_penalty_total REAL NOT NULL DEFAULT 0,
    normalization_enabled INTEGER NOT NULL,
    confidence_penalty_enabled INTEGER NOT NULL,
    hub_penalty_enabled INTEGER NOT NULL,
    diversity_rerank_enabled INTEGER NOT NULL,
    source_quota_bonus_enabled INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_radio_diagnostics_created_at
    ON radio_diagnostics(created_at);
CREATE INDEX IF NOT EXISTS idx_radio_diagnostics_seed
    ON radio_diagnostics(seed_track_id);

INSERT OR IGNORE INTO server_config (key, value) VALUES ('radio_use_legacy_pipeline', 'false');
INSERT OR IGNORE INTO server_config (key, value) VALUES ('radio_score_normalization_enabled', 'false');
INSERT OR IGNORE INTO server_config (key, value) VALUES ('radio_confidence_penalty_enabled', 'false');
INSERT OR IGNORE INTO server_config (key, value) VALUES ('radio_hub_penalty_enabled', 'false');
INSERT OR IGNORE INTO server_config (key, value) VALUES ('radio_diversity_rerank_enabled', 'false');
INSERT OR IGNORE INTO server_config (key, value) VALUES ('radio_source_quota_bonus_enabled', 'false');
"#;

// Discovery training intensity. Three preset tiers + an explicit kill-switch
// for the audio-proxy stage. The intensity key drives trainer params at
// runtime; the audio-proxy flag lets `low` skip a stage entirely without
// requiring a separate config row read.
const MIGRATION_027: &str = r#"
INSERT OR IGNORE INTO server_config (key, value) VALUES ('discovery_intensity', 'medium');
"#;

// Discovery + Radio Tier 1: per-reason held-out hit-rate diagnostics.
// One row per (model_id, primary_reason) emitted by the trainer's eval block.
// Tags below the impressions threshold are still recorded so we can see the
// "insufficient data" tail rather than only the headline numbers.
const MIGRATION_026: &str = r#"
CREATE TABLE IF NOT EXISTS discovery_diagnostics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id INTEGER NOT NULL REFERENCES embedding_models(id) ON DELETE CASCADE,
    primary_reason TEXT NOT NULL,
    impressions INTEGER NOT NULL,
    hits INTEGER NOT NULL,
    hit_rate REAL NOT NULL,
    mean_rank REAL,
    mrr_contribution REAL NOT NULL DEFAULT 0,
    insufficient_data INTEGER NOT NULL DEFAULT 0,
    computed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_discovery_diagnostics_model
    ON discovery_diagnostics(model_id, primary_reason);
"#;

const MIGRATION_028: &str = r#"
CREATE TABLE IF NOT EXISTS track_context_tags (
    track_id       INTEGER NOT NULL,
    tag            TEXT    NOT NULL,
    normalized_tag TEXT    NOT NULL,
    context        TEXT    NOT NULL,
    source         TEXT    NOT NULL,
    confidence     REAL    NOT NULL DEFAULT 0.5,
    PRIMARY KEY (track_id, normalized_tag, context, source),
    FOREIGN KEY (track_id) REFERENCES tracks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_context_tags_track
    ON track_context_tags(track_id);

CREATE INDEX IF NOT EXISTS idx_context_tags_context
    ON track_context_tags(context);

CREATE INDEX IF NOT EXISTS idx_context_tags_lookup
    ON track_context_tags(normalized_tag, context, confidence DESC);
"#;

// Public Spotify stats cache. Three positive caches + one negative cache. All
// `*_at` columns hold unix epoch seconds. Stats tables have a 7-day TTL
// (handler logic), the ISRC→track mapping is permanent (Spotify track IDs
// don't change), and the negative cache rate-limits repeated misses for 24h.
const MIGRATION_029: &str = r#"
CREATE TABLE IF NOT EXISTS spotify_isrc_map (
    isrc             TEXT PRIMARY KEY,
    spotify_track_id TEXT NOT NULL,
    resolved_at      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS spotify_track_stats (
    spotify_track_id TEXT PRIMARY KEY,
    playcount        INTEGER NOT NULL,
    fetched_at       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_spotify_track_stats_fetched_at
    ON spotify_track_stats(fetched_at);

CREATE TABLE IF NOT EXISTS spotify_artist_stats (
    spotify_artist_id TEXT PRIMARY KEY,
    monthly_listeners INTEGER NOT NULL,
    fetched_at        INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_spotify_artist_stats_fetched_at
    ON spotify_artist_stats(fetched_at);

CREATE TABLE IF NOT EXISTS spotify_null_cache (
    isrc      TEXT PRIMARY KEY,
    cached_at INTEGER NOT NULL
);
"#;

// User-queued external tracks (TIDAL search rows the user clicked Add to queue
// or Play next on) carry a known tidal_id at insert time. The pending-row
// resolver uses this hint to fetch the track directly instead of searching
// Tidal by artist+title — same row schema, faster + more accurate resolution.
// Existing rows have NULL; resolver falls back to artist+title search.
const MIGRATION_030: &str = r#"
ALTER TABLE queue ADD COLUMN tidal_id_hint INTEGER;
"#;

// Sportify (anonymous Spotify metadata proxy) discovery layer.
// Four metadata caches keyed on Spotify IDs, plus the Spotify->TIDAL
// resolution map and its negative-cache twin, plus a search-result cache.
// World playcounts and monthly listeners go into the existing
// spotify_track_stats / spotify_artist_stats tables (MIGRATION_029) — no
// parallel sportify_*_stats tables, so the legacy artist page benefits too.
const MIGRATION_031: &str = r#"
CREATE TABLE IF NOT EXISTS sportify_track_meta (
    spotify_track_id TEXT PRIMARY KEY,
    payload          TEXT NOT NULL,
    fetched_at       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sportify_track_meta_fetched_at
    ON sportify_track_meta(fetched_at);

CREATE TABLE IF NOT EXISTS sportify_album_meta (
    spotify_album_id TEXT PRIMARY KEY,
    payload          TEXT NOT NULL,
    fetched_at       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sportify_album_meta_fetched_at
    ON sportify_album_meta(fetched_at);

CREATE TABLE IF NOT EXISTS sportify_artist_meta (
    spotify_artist_id TEXT PRIMARY KEY,
    payload           TEXT NOT NULL,
    fetched_at        INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sportify_artist_meta_fetched_at
    ON sportify_artist_meta(fetched_at);

CREATE TABLE IF NOT EXISTS sportify_playlist_meta (
    spotify_playlist_id TEXT PRIMARY KEY,
    payload             TEXT NOT NULL,
    snapshot_id         TEXT,
    fetched_at          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sportify_playlist_meta_fetched_at
    ON sportify_playlist_meta(fetched_at);

CREATE TABLE IF NOT EXISTS sportify_track_map (
    spotify_track_id TEXT PRIMARY KEY,
    tidal_track_id   INTEGER NOT NULL,
    confidence       REAL NOT NULL,
    match_reason     TEXT,
    resolved_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sportify_track_map_tidal
    ON sportify_track_map(tidal_track_id);

CREATE TABLE IF NOT EXISTS sportify_unresolved (
    spotify_track_id TEXT PRIMARY KEY,
    last_attempt_at  INTEGER NOT NULL,
    attempts         INTEGER NOT NULL DEFAULT 1,
    reason           TEXT
);

CREATE INDEX IF NOT EXISTS idx_sportify_unresolved_last_attempt
    ON sportify_unresolved(last_attempt_at);

CREATE TABLE IF NOT EXISTS sportify_search_cache (
    query_hash TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sportify_search_cache_fetched_at
    ON sportify_search_cache(fetched_at);
"#;

// Clear Sportify metadata + search caches. Initial release shipped with a
// model that mismatched the upstream Sportify shape (track `title`/`artist`,
// playlist `owner` string, flat `results` array). All four meta tables and
// the search cache may hold rows that deserialized into all-None defaults.
// Wipe them once so live re-fetches populate with the corrected models.
// Resolution map (`sportify_track_map`) is preserved — those rows are TIDAL
// ids and aren't affected by Sportify shape changes.
const MIGRATION_032: &str = r#"
DELETE FROM sportify_track_meta;
DELETE FROM sportify_album_meta;
DELETE FROM sportify_artist_meta;
DELETE FROM sportify_playlist_meta;
DELETE FROM sportify_search_cache;
"#;

// Single-strongest-tag-per-track view. Used by radio diversity reranking and
// any future consumer that wants ONE trustworthy signal instead of the
// multi-tag set. Pick rule:
//   1. confidence DESC
//   2. source priority: musicbrainz > spotify > lastfm > anything else
//   3. genre_id ASC (deterministic final tiebreak)
//
// Not materialized — recomputed on every read. At ~40k assignments the cost is
// negligible and we avoid the synchronization burden of a real column. Galaxy
// queries that want a top-1 view can `JOIN track_primary_genre` instead of
// re-implementing the pick.
const MIGRATION_033: &str = r#"
CREATE VIEW IF NOT EXISTS track_primary_genre AS
SELECT track_id, genre_id AS primary_genre_id, source, confidence
FROM (
    SELECT
        track_id,
        genre_id,
        source,
        confidence,
        ROW_NUMBER() OVER (
            PARTITION BY track_id
            ORDER BY
                confidence DESC,
                CASE source
                    WHEN 'musicbrainz' THEN 1
                    WHEN 'spotify'     THEN 2
                    WHEN 'lastfm'      THEN 3
                    ELSE                    9
                END,
                genre_id
        ) AS rn
    FROM track_genres
)
WHERE rn = 1;
"#;

// Mirror of `sportify_search_cache` for upstream TIDAL catalog search.
// Keyed by sha256(normalized query + limit + offset). The wrapper in
// `services/tidal/cache.rs` writes the parsed `TidalSearchCatalog` JSON,
// not the enriched response — `in_library` flags re-derive on every read.
const MIGRATION_034: &str = r#"
CREATE TABLE IF NOT EXISTS tidal_search_cache (
    query_hash TEXT PRIMARY KEY,
    payload    TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tidal_search_cache_fetched_at
    ON tidal_search_cache(fetched_at);
"#;

// Analytics signals: cohort first-listen detection runs `MIN(started_at) GROUP BY track_id`
// over the full listen_history table. Without this covering index the cohort query scans
// every row; with it, the planner can use the index alone.
const MIGRATION_035: &str = r#"
CREATE INDEX IF NOT EXISTS idx_listen_history_track_started
    ON listen_history(track_id, started_at);
"#;

// Backfill tracks.last_played_at from the most recent listen_history row for any
// track that has listens but a NULL last_played_at. Pre-fix code only stamped
// last_played_at on completed listens, so partial-only tracks were invisible to
// the freshness weighting in shuffle.rs even after we'd heard them.
const MIGRATION_036: &str = r#"
UPDATE tracks
SET last_played_at = (
    SELECT MAX(started_at)
    FROM listen_history
    WHERE listen_history.track_id = tracks.id
)
WHERE last_played_at IS NULL
  AND id IN (SELECT DISTINCT track_id FROM listen_history);
"#;

const MIGRATION_037: &str = r#"
ALTER TABLE track_neighbors ADD COLUMN support_transition REAL NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN support_colisten REAL NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN support_structure REAL NOT NULL DEFAULT 0;
ALTER TABLE track_neighbors ADD COLUMN support_metadata REAL NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS external_track_candidates (
    id INTEGER PRIMARY KEY,
    tidal_id INTEGER,
    mbid TEXT,
    dedupe_key TEXT NOT NULL UNIQUE,
    normalized_artist_name TEXT NOT NULL DEFAULT '',
    normalized_title TEXT NOT NULL DEFAULT '',
    duration_bucket INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    artist_name TEXT NOT NULL,
    genre_tags_json TEXT,
    duration_ms INTEGER,
    expires_at TEXT NOT NULL,
    resolved_track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_external_candidates_tidal_id
    ON external_track_candidates(tidal_id)
    WHERE tidal_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_external_candidates_mbid
    ON external_track_candidates(mbid)
    WHERE mbid IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_external_candidates_dedupe_key
    ON external_track_candidates(dedupe_key);
CREATE UNIQUE INDEX IF NOT EXISTS idx_external_candidates_fallback_identity
    ON external_track_candidates(normalized_artist_name, normalized_title, duration_bucket)
    WHERE tidal_id IS NULL
      AND mbid IS NULL
      AND normalized_artist_name <> ''
      AND normalized_title <> '';
CREATE INDEX IF NOT EXISTS idx_external_candidates_expires_at
    ON external_track_candidates(expires_at);

CREATE TABLE IF NOT EXISTS external_track_candidate_sightings (
    candidate_id INTEGER NOT NULL REFERENCES external_track_candidates(id) ON DELETE CASCADE,
    seed_track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    source_payload_json TEXT,
    similarity REAL,
    seen_at TEXT DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    PRIMARY KEY (candidate_id, seed_track_id, source)
);

CREATE INDEX IF NOT EXISTS idx_external_sightings_seed_source
    ON external_track_candidate_sightings(seed_track_id, source);
CREATE INDEX IF NOT EXISTS idx_external_sightings_expires_at
    ON external_track_candidate_sightings(expires_at);

CREATE TABLE IF NOT EXISTS external_track_candidate_audio_features (
    candidate_id INTEGER PRIMARY KEY REFERENCES external_track_candidates(id) ON DELETE CASCADE,
    feature_version TEXT NOT NULL,
    vector_blob BLOB NOT NULL,
    clip_start_ms INTEGER NOT NULL DEFAULT 0,
    clip_duration_ms INTEGER NOT NULL DEFAULT 0,
    computed_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS external_track_candidate_embeddings (
    candidate_id INTEGER NOT NULL REFERENCES external_track_candidates(id) ON DELETE CASCADE,
    model_id INTEGER NOT NULL REFERENCES embedding_models(id) ON DELETE CASCADE,
    vector_blob BLOB NOT NULL,
    l2_norm REAL NOT NULL DEFAULT 0,
    generated_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (candidate_id, model_id)
);

CREATE TABLE IF NOT EXISTS external_track_candidate_neighbors (
    library_track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    candidate_id INTEGER NOT NULL REFERENCES external_track_candidates(id) ON DELETE CASCADE,
    model_id INTEGER NOT NULL REFERENCES embedding_models(id) ON DELETE CASCADE,
    rank INTEGER NOT NULL,
    score REAL NOT NULL DEFAULT 0,
    audio_score REAL NOT NULL DEFAULT 0,
    metadata_score REAL NOT NULL DEFAULT 0,
    reason_json TEXT,
    computed_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (library_track_id, candidate_id, model_id)
);

CREATE INDEX IF NOT EXISTS idx_external_neighbors_library_model_rank
    ON external_track_candidate_neighbors(library_track_id, model_id, rank);
"#;

const MIGRATION_038: &str = r#"
ALTER TABLE sync_metadata ADD COLUMN last_full_sync_at TEXT;
ALTER TABLE sync_metadata ADD COLUMN last_sync_kind TEXT;
ALTER TABLE sync_metadata ADD COLUMN tidal_favorite_artist_cursor TEXT;
ALTER TABLE sync_metadata ADD COLUMN tidal_favorite_album_cursor TEXT;
ALTER TABLE sync_metadata ADD COLUMN tidal_favorite_track_cursor TEXT;
"#;

/// Manual BPM override flag. When set, the row is treated as authoritative —
/// automatic analysis (passive actor, queue prescanner, bulk scanner) skips
/// it even after CURRENT_ANALYSIS_VERSION bumps. Lets users halve / double
/// slow reggae / folk where every tempo detector (mine, aubio, the madmom-
/// port BLSTM) doubles the result.
const MIGRATION_039: &str = r#"
ALTER TABLE audio_dsp_features ADD COLUMN manual_override INTEGER NOT NULL DEFAULT 0;
"#;

// Health signal for the radio Engine lane. `actual_engine_count = 0` alone is
// ambiguous: it can mean "no track_similarity match for this seed" or "the
// track_similarity index was never built". This flag disambiguates so an empty
// index is an obvious signal in radio_diagnostics rather than a silent zero.
const MIGRATION_040: &str = r#"
ALTER TABLE radio_diagnostics ADD COLUMN engine_index_empty INTEGER NOT NULL DEFAULT 0;
"#;

// Spotify public stats: drop the NOT NULL on monthly_listeners (Spotify omits
// the field for some artists), add followers / world_rank / top_cities_json
// columns, and introduce spotify_artist_map for the Tidal->Spotify artist
// resolution + negative-cache layer. The whole block is wrapped in BEGIN/COMMIT
// because run_migrations does not wrap individual migrations in a transaction,
// and the table-rebuild step (DROP + RENAME) leaves the schema wedged if it
// half-applies.
const MIGRATION_041: &str = r#"
BEGIN;

ALTER TABLE spotify_artist_stats ADD COLUMN followers INTEGER;
ALTER TABLE spotify_artist_stats ADD COLUMN world_rank INTEGER;
ALTER TABLE spotify_artist_stats ADD COLUMN top_cities_json TEXT;

CREATE TABLE spotify_artist_stats_new (
    spotify_artist_id TEXT PRIMARY KEY,
    monthly_listeners INTEGER,
    followers         INTEGER,
    world_rank        INTEGER,
    top_cities_json   TEXT,
    fetched_at        INTEGER NOT NULL
);

INSERT INTO spotify_artist_stats_new
    SELECT spotify_artist_id, monthly_listeners, followers, world_rank,
           top_cities_json, fetched_at
    FROM spotify_artist_stats;

DROP TABLE spotify_artist_stats;
ALTER TABLE spotify_artist_stats_new RENAME TO spotify_artist_stats;
CREATE INDEX idx_spotify_artist_stats_fetched_at
    ON spotify_artist_stats(fetched_at);

CREATE TABLE IF NOT EXISTS spotify_artist_map (
    tidal_artist_id   TEXT PRIMARY KEY,
    spotify_artist_id TEXT,
    resolved_at       INTEGER NOT NULL
);

COMMIT;
"#;

const MIGRATION_042: &str = r#"
ALTER TABLE playback_state ADD COLUMN shuffle_seed INTEGER;
"#;

// Discovery / favorites / radio-seed ordering indexes. Before these, the three
// hottest non-library track queries each did a full `SCAN tracks` plus a temp
// B-tree sort over all ~35k rows on every call:
//   - get_favorite_track_ids:        ORDER BY is_favorite, play_count DESC
//   - get_discovery_candidate_tracks ORDER BY is_favorite DESC, play_count ASC,
//                                    fidelity_score DESC, date_added DESC, title
//   - get_tidal_similar_seed_rows:   ORDER BY play_count DESC, last_played_at DESC, id
// Each index lets SQLite satisfy the ORDER BY directly (no temp B-tree) and stop
// after LIMIT rows. Measured on the dev library (35.5k tracks): favorites 17x,
// discovery 95x, seed 51x faster. The default library listing already rides
// idx_tracks_date_added, so it is unchanged. Cost is three extra indexes to
// maintain on track insert/update, paid during bulk sync transactions.
const MIGRATION_043: &str = r#"
CREATE INDEX IF NOT EXISTS idx_tracks_fav_play
    ON tracks(is_favorite DESC, play_count DESC);
CREATE INDEX IF NOT EXISTS idx_tracks_discovery
    ON tracks(is_favorite DESC, play_count ASC, fidelity_score DESC, date_added DESC, title ASC);
CREATE INDEX IF NOT EXISTS idx_tracks_play_last
    ON tracks(play_count DESC, last_played_at DESC, id DESC);
"#;

const MIGRATION_044: &str = r#"
CREATE TABLE IF NOT EXISTS audio_dj_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    media_ref_kind TEXT NOT NULL,
    media_ref_id TEXT NOT NULL,
    track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    queue_item_id INTEGER REFERENCES queue(id) ON DELETE SET NULL,
    tidal_id INTEGER,
    profile_version TEXT NOT NULL,
    beat_grid_blob BLOB NOT NULL,
    downbeats_blob BLOB NOT NULL,
    phrase_boundaries_blob BLOB NOT NULL,
    mix_in_blob BLOB NOT NULL,
    mix_out_blob BLOB NOT NULL,
    intro_end_seconds REAL,
    outro_start_seconds REAL,
    breakdown_blob BLOB NOT NULL,
    drop_blob BLOB NOT NULL,
    safe_transition_windows_blob BLOB NOT NULL,
    energy_contour_blob BLOB NOT NULL,
    vocal_presence_blob BLOB NOT NULL,
    vocal_density_blob BLOB NOT NULL,
    lufs_loud_body REAL,
    true_peak_dbtp REAL,
    beat_confidence REAL,
    profile_confidence REAL NOT NULL DEFAULT 0,
    analysis_scope_ms INTEGER NOT NULL DEFAULT 0,
    is_temporary INTEGER NOT NULL DEFAULT 0,
    source TEXT NOT NULL DEFAULT 'noor_dj_v1',
    computed_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(media_ref_kind, media_ref_id)
);

CREATE INDEX IF NOT EXISTS idx_audio_dj_profiles_version
    ON audio_dj_profiles(profile_version);
CREATE INDEX IF NOT EXISTS idx_audio_dj_profiles_track
    ON audio_dj_profiles(track_id)
    WHERE track_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_audio_dj_profiles_tidal
    ON audio_dj_profiles(tidal_id)
    WHERE tidal_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS audio_dj_profile_corrections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    media_ref_kind TEXT NOT NULL,
    media_ref_id TEXT NOT NULL,
    bpm_multiplier REAL,
    downbeat_offset_beats INTEGER,
    phrase_offset_bars INTEGER,
    safe_crossfade_only INTEGER NOT NULL DEFAULT 0,
    transition_speed_bias TEXT,
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(media_ref_kind, media_ref_id),
    CHECK (transition_speed_bias IS NULL OR transition_speed_bias IN ('slower', 'neutral', 'faster'))
);

CREATE INDEX IF NOT EXISTS idx_audio_dj_profile_corrections_ref
    ON audio_dj_profile_corrections(media_ref_kind, media_ref_id);

CREATE TABLE IF NOT EXISTS dj_transition_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    from_track_id INTEGER REFERENCES tracks(id),
    to_track_id INTEGER REFERENCES tracks(id),
    from_media_ref_kind TEXT,
    from_media_ref_id TEXT,
    to_media_ref_kind TEXT,
    to_media_ref_id TEXT,
    template TEXT NOT NULL,
    program_json TEXT NOT NULL,
    rejected_alternatives_json TEXT,
    planner_version TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    outcome TEXT,
    outcome_at TEXT,
    fallback_reason TEXT,
    user_rating INTEGER,
    skip_within_30s INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_dj_transition_events_tracks
    ON dj_transition_events(from_track_id, to_track_id, started_at);
CREATE INDEX IF NOT EXISTS idx_dj_transition_events_outcome
    ON dj_transition_events(outcome)
    WHERE outcome IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_dj_transition_events_feedback_from
    ON dj_transition_events(from_media_ref_kind, from_media_ref_id, started_at)
    WHERE user_rating IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_dj_transition_events_feedback_to
    ON dj_transition_events(to_media_ref_kind, to_media_ref_id, started_at)
    WHERE user_rating IS NOT NULL;

INSERT OR IGNORE INTO server_config (key, value)
VALUES ('dj_engine_enabled', '0');
INSERT OR IGNORE INTO server_config (key, value)
VALUES ('dj_mix_intent', 'balanced');
INSERT OR IGNORE INTO server_config (key, value)
VALUES ('dj_transition_speed_bias', 'neutral');
"#;

const MIGRATION_045: &str = r#"
ALTER TABLE dj_transition_events ADD COLUMN planned_start_ms INTEGER;
ALTER TABLE dj_transition_events ADD COLUMN actual_start_ms INTEGER;
ALTER TABLE dj_transition_events ADD COLUMN timing_delta_ms INTEGER;
ALTER TABLE dj_transition_events ADD COLUMN timing_source TEXT;
ALTER TABLE dj_transition_events ADD COLUMN timing_status TEXT;
"#;

const MIGRATION_046: &str = r#"
ALTER TABLE dj_transition_events ADD COLUMN runtime_rendered_dj_mixer INTEGER;
ALTER TABLE dj_transition_events ADD COLUMN runtime_renderer_status TEXT;
ALTER TABLE dj_transition_events ADD COLUMN runtime_renderer_reason TEXT;
"#;

const MIGRATION_047: &str = r#"
ALTER TABLE audio_dj_profiles ADD COLUMN waveform_peaks_blob BLOB NOT NULL DEFAULT X'';
"#;

const MIGRATION_048: &str = r#"
CREATE TABLE IF NOT EXISTS chart_sources (
    id INTEGER PRIMARY KEY,
    source_key TEXT UNIQUE NOT NULL,
    display_name TEXT NOT NULL,
    provider TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    default_region TEXT,
    refresh_interval_hours INTEGER NOT NULL DEFAULT 24,
    last_success_at INTEGER,
    last_error TEXT
);

CREATE TABLE IF NOT EXISTS chart_snapshots (
    id INTEGER PRIMARY KEY,
    source_key TEXT NOT NULL,
    region TEXT NOT NULL,
    period TEXT NOT NULL,
    chart_date TEXT NOT NULL,
    fetched_at INTEGER NOT NULL,
    etag TEXT,
    content_hash TEXT,
    status TEXT NOT NULL,
    UNIQUE (source_key, region, period, chart_date)
);

CREATE INDEX IF NOT EXISTS idx_chart_snapshots_latest
    ON chart_snapshots(source_key, region, period, chart_date DESC, fetched_at DESC);

CREATE TABLE IF NOT EXISTS chart_entries (
    id INTEGER PRIMARY KEY,
    snapshot_id INTEGER NOT NULL REFERENCES chart_snapshots(id) ON DELETE CASCADE,
    rank INTEGER NOT NULL,
    rank_delta INTEGER,
    artist TEXT NOT NULL,
    title TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'track',
    album TEXT,
    artwork_url TEXT,
    external_track_id TEXT,
    external_artist_id TEXT,
    external_video_id TEXT,
    external_url TEXT,
    streams INTEGER,
    stream_delta INTEGER,
    views INTEGER,
    likes INTEGER,
    audience REAL,
    audience_delta REAL,
    points REAL,
    points_delta REAL,
    seven_day_streams INTEGER,
    total_streams INTEGER,
    days_on_chart INTEGER,
    peak_rank INTEGER,
    provider_positions_json TEXT,
    raw_json TEXT,
    UNIQUE (snapshot_id, rank)
);

CREATE INDEX IF NOT EXISTS idx_chart_entries_snapshot_rank
    ON chart_entries(snapshot_id, rank);

CREATE TABLE IF NOT EXISTS chart_entry_resolutions (
    entry_id INTEGER PRIMARY KEY REFERENCES chart_entries(id) ON DELETE CASCADE,
    external_candidate_id INTEGER REFERENCES external_track_candidates(id) ON DELETE SET NULL,
    local_track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    tidal_id INTEGER,
    status TEXT NOT NULL,
    score REAL,
    resolved_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_chart_entry_resolutions_status
    ON chart_entry_resolutions(status, resolved_at);
"#;

const MIGRATION_049: &str = r#"
CREATE TABLE IF NOT EXISTS scrobble_outbox (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    kind TEXT NOT NULL,
    track_id INTEGER,
    artist TEXT NOT NULL,
    title TEXT NOT NULL,
    album TEXT,
    duration_ms INTEGER,
    listened_ms INTEGER,
    started_at_unix INTEGER,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(provider, kind, track_id, started_at_unix)
);

CREATE INDEX IF NOT EXISTS idx_scrobble_outbox_due
    ON scrobble_outbox(status, next_attempt_at, id);

CREATE TABLE IF NOT EXISTS provider_feedback_outbox (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    action TEXT NOT NULL,
    track_id INTEGER NOT NULL,
    artist TEXT NOT NULL,
    title TEXT NOT NULL,
    mbid TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(provider, action, track_id)
);

CREATE INDEX IF NOT EXISTS idx_provider_feedback_outbox_due
    ON provider_feedback_outbox(status, next_attempt_at, id);

CREATE TABLE IF NOT EXISTS provider_recommendation_cache (
    provider TEXT NOT NULL,
    cache_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    fetched_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY(provider, cache_key)
);

CREATE INDEX IF NOT EXISTS idx_provider_recommendation_cache_expiry
    ON provider_recommendation_cache(provider, expires_at);
"#;

const MIGRATION_050: &str = r#"
ALTER TABLE audio_dj_profile_corrections
    ADD COLUMN manual_drop_blob BLOB NOT NULL DEFAULT X'';
"#;

// Ephemeral TIDAL queue rows. TIDAL mix/album/playlist tracks now live in the
// `queue` table as real, mutable rows (positive ids) that stream ephemerally
// instead of being imported. They keep `track_id` NULL forever and carry their
// TIDAL metadata here so `load_queue` can hydrate a synthetic playable track
// without a `tracks` row. `tidal_id_hint` (migration 030) holds the TIDAL id;
// these columns hold the rest the streaming + display paths need. Distinct from
// Last.fm pending rows, which DO get imported. See the queue ephemeral-row
// helpers in playback/queue.rs.
const MIGRATION_051: &str = r#"
ALTER TABLE queue ADD COLUMN ephemeral_album_title TEXT;
ALTER TABLE queue ADD COLUMN ephemeral_artwork_url TEXT;
ALTER TABLE queue ADD COLUMN ephemeral_duration_ms INTEGER;
"#;

// `is_library` separates genuinely-curated library tracks from transient TIDAL
// rows that resolvers/discovery import for playback. Both populations carry
// `source = 'tidal'` and `is_favorite = 0`, so no existing column could tell
// them apart at read time. The bug this fixes: a discovery/resolver import
// (inject_discovery_tracks, import_track_from_metadata) that lands in an
// already-favorited album would surface in the Library Tracks tab via the
// `favorite_predicate` album-favorite branch, even though the user never added
// it (and it was often a dead, non-streamable edition).
//
// Going forward the flag is set only by genuine write paths (insert_tidal_track
// favorites/album/playlist sync, the favorite-toggle); transient paths leave it
// at DEFAULT 0 (fail-safe: a forgotten path stays hidden, never wrongly shown).
//
// The backfill PRESERVES current visibility, then demotes only true post-favorite
// injections, so nothing genuine disappears before the next sync:
//   (1) explicit likes are unambiguously library.
//   (2) local files are user-owned.
//   (3) every track sitting in a favorited album (the whole-album-favorite
//       population) is preserved -- no "fully populated" test, so partial
//       region-incomplete favorited albums keep their tracks.
//   (4) demote un-liked tracks added to a favorited album either via the
//       resolver source ('tidal_stream') or well after that album row was
//       created. Genuine favorites-sync lands a whole album at once (gap ~0);
//       an injected track shows a multi-day gap. NULL created_at/date_added
//       yield NULL comparisons (never demoted), which is the safe direction.
const MIGRATION_052: &str = r#"
ALTER TABLE tracks ADD COLUMN is_library INTEGER NOT NULL DEFAULT 0;

UPDATE tracks SET is_library = 1 WHERE is_favorite = 1;
UPDATE tracks SET is_library = 1 WHERE source = 'local';
UPDATE tracks SET is_library = 1
 WHERE album_id IN (SELECT id FROM albums WHERE is_favorite = 1);

UPDATE tracks SET is_library = 0
 WHERE is_favorite = 0
   AND album_id IN (SELECT id FROM albums WHERE is_favorite = 1)
   AND (
        source = 'tidal_stream'
        OR julianday(date_added) - julianday(
             (SELECT created_at FROM albums WHERE albums.id = tracks.album_id)
           ) > 7
   );

CREATE INDEX IF NOT EXISTS idx_tracks_is_library ON tracks(is_library);
"#;

// Carry the TIDAL artist/album ids on ephemeral queue rows so a mix/playlist/album
// track keeps its clickable artist/album identity through the whole player (now
// playing + Up Next), independent of the frontend metadata cache. Nullable: only
// ephemeral TIDAL rows populate them; library/pending rows leave them NULL and
// link via their local ids instead.
const MIGRATION_053: &str = r#"
ALTER TABLE queue ADD COLUMN ephemeral_artist_tidal_id INTEGER;
ALTER TABLE queue ADD COLUMN ephemeral_album_tidal_id INTEGER;
"#;

// Sync rework: favorited albums become bookmarks and their tracks arrive as
// hidden discovery fill (is_library=0) via the enrichment pass, gated by
// sync_metadata.enrich_from_favorite_albums (default on). The two indexes
// back the per-track import-dedupe candidate lookups
// (library::duplicates::fetch_import_candidates); artist_id had no dedicated
// index before. Deliberately additive only - demoting existing album-fill is
// the user-triggered Reclean action, never a silent migration.
const MIGRATION_054: &str = r#"
ALTER TABLE sync_metadata ADD COLUMN enrich_from_favorite_albums INTEGER NOT NULL DEFAULT 1;
ALTER TABLE albums ADD COLUMN enrich_completed_at TEXT;
CREATE INDEX IF NOT EXISTS idx_tracks_isrc ON tracks(isrc);
CREATE INDEX IF NOT EXISTS idx_tracks_artist_id ON tracks(artist_id);
"#;

// Editorial video sets for the /videos browse state. A row is one built
// snapshot of one set (slug) for one rotation bucket (bucket_key, e.g.
// "2026-07-17" for daily or "2026-W29" for weekly). Items are stored as an
// opaque JSON array: sets are ephemeral editorial output, fetched whole and
// never joined, so normalized rows would buy nothing.
const MIGRATION_055: &str = r#"
CREATE TABLE IF NOT EXISTS video_sets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    slug TEXT NOT NULL,
    bucket_key TEXT NOT NULL,
    title TEXT NOT NULL,
    blurb TEXT NOT NULL,
    built_at TEXT NOT NULL DEFAULT (datetime('now')),
    items_json TEXT NOT NULL,
    UNIQUE(slug, bucket_key)
);
CREATE INDEX IF NOT EXISTS idx_video_sets_slug_built ON video_sets(slug, built_at);
"#;

// Watch history for editorial videos. Separate from listen_history on purpose:
// that table is track_id-FK and every reader assumes track semantics, whereas
// videos are TIDAL-id-only and never imported. The builder reads this to drop
// recently-watched picks so the shelves move on instead of re-serving what you
// just watched. completed/duration are recorded for a later taste loop.
const MIGRATION_056: &str = r#"
CREATE TABLE IF NOT EXISTS video_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tidal_video_id INTEGER NOT NULL,
    title TEXT,
    artist_tidal_id INTEGER,
    artist_name TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    duration_watched_ms INTEGER,
    completed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_video_history_started ON video_history(started_at);
CREATE INDEX IF NOT EXISTS idx_video_history_video ON video_history(tidal_video_id);
"#;

// track_neighbors had no index with model_id as its leading column: the primary
// key is (track_id, neighbor_track_id, model_id) and the two secondary indexes
// lead with track_id and neighbor_track_id. Anything selecting by model alone
// therefore scanned the whole table.
//
// That is exactly what pruning retired models does, and it made the cleanup
// unusable: measured on an 18.4M-row table it managed ~4k rows/sec, about an
// hour to clear 14.7M dead rows, because every 20k-row batch re-scanned the
// table. With this index each batch is a seek.
const MIGRATION_057: &str = r#"
CREATE INDEX IF NOT EXISTS idx_track_neighbors_model ON track_neighbors(model_id);
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

#[cfg(test)]
pub(super) fn apply_migrations_up_to(conn: &Connection, n: usize) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            applied_at TEXT DEFAULT (datetime('now'))
        );",
    )?;

    let applied: i64 = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
        .unwrap_or(0);

    let limit = n.min(MIGRATIONS.len());
    for (i, migration) in MIGRATIONS[..limit].iter().enumerate() {
        let migration_id = (i + 1) as i64;
        if migration_id > applied {
            conn.execute_batch(migration)?;
            conn.execute("INSERT INTO _migrations (id) VALUES (?1)", [migration_id])?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migration_041_preserves_existing_artist_stats() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        // Apply through 040 first.
        apply_migrations_up_to(&conn, 40).unwrap();

        // Seed one row in spotify_artist_stats with monthly_listeners set
        // (this column was NOT NULL pre-041).
        conn.execute(
            "INSERT INTO spotify_artist_stats (spotify_artist_id, monthly_listeners, fetched_at) \
             VALUES ('abc123', 12345, 1700000000)",
            [],
        )
        .unwrap();

        // Now apply 041 (table rebuild + new columns + new map table).
        apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

        // Row survives the rebuild with original values.
        let (ml, fa): (Option<i64>, i64) = conn
            .query_row(
                "SELECT monthly_listeners, fetched_at FROM spotify_artist_stats WHERE spotify_artist_id = 'abc123'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(ml, Some(12345));
        assert_eq!(fa, 1700000000);

        // New columns exist and default to NULL.
        let (followers, world_rank, top_cities): (Option<i64>, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT followers, world_rank, top_cities_json FROM spotify_artist_stats \
                 WHERE spotify_artist_id = 'abc123'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(followers, None);
        assert_eq!(world_rank, None);
        assert_eq!(top_cities, None);

        // monthly_listeners is now nullable: insert with NULL succeeds.
        conn.execute(
            "INSERT INTO spotify_artist_stats (spotify_artist_id, monthly_listeners, fetched_at) \
             VALUES ('null_ml', NULL, 1700000001)",
            [],
        )
        .unwrap();

        // spotify_artist_map exists and accepts both positive and negative rows.
        conn.execute(
            "INSERT INTO spotify_artist_map (tidal_artist_id, spotify_artist_id, resolved_at) \
             VALUES ('42', 'spotify_xyz', 1700000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO spotify_artist_map (tidal_artist_id, spotify_artist_id, resolved_at) \
             VALUES ('99', NULL, 1700000000)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn migration_054_adds_enrichment_toggle_and_dedupe_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

        // The seeded tidal row (migration 006) got the new column backfilled
        // with the default: enrichment on.
        let enabled: i64 = conn
            .query_row(
                "SELECT enrich_from_favorite_albums FROM sync_metadata WHERE service = 'tidal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 1, "enrichment defaults on");

        // albums.enrich_completed_at exists and defaults NULL (never enriched).
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'A')
             ",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (1, 'Al', 1, 'tidal')",
            [],
        )
        .unwrap();
        let enriched_at: Option<String> = conn
            .query_row(
                "SELECT enrich_completed_at FROM albums WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enriched_at, None);

        // Import-dedupe candidate indexes exist.
        for idx in ["idx_tracks_isrc", "idx_tracks_artist_id"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected index {idx} to exist");
        }
    }

    #[test]
    fn migration_043_adds_ordering_indexes_and_avoids_temp_sort() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

        for idx in [
            "idx_tracks_fav_play",
            "idx_tracks_discovery",
            "idx_tracks_play_last",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing index {idx}");
        }

        // The discovery ordering must be satisfiable straight from the index:
        // the planner should not fall back to a temp B-tree sort. EXPLAIN QUERY
        // PLAN decides this from the schema alone, so it is stable on an empty
        // in-memory table.
        let plan: String = {
            let mut stmt = conn
                .prepare(
                    "EXPLAIN QUERY PLAN \
                     SELECT t.id FROM tracks t \
                     ORDER BY t.is_favorite DESC, t.play_count ASC, \
                              t.fidelity_score DESC, t.date_added DESC, t.title ASC \
                     LIMIT 200",
                )
                .unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            rows.join(" | ")
        };
        assert!(
            plan.contains("idx_tracks_discovery"),
            "discovery query should use idx_tracks_discovery, plan was: {plan}"
        );
        assert!(
            !plan.contains("TEMP B-TREE"),
            "discovery query should not need a temp sort, plan was: {plan}"
        );
    }

    #[test]
    fn migration_045_adds_dj_transition_timing_fields() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

        for column in [
            "planned_start_ms",
            "actual_start_ms",
            "timing_delta_ms",
            "timing_source",
            "timing_status",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM pragma_table_info('dj_transition_events')
                     WHERE name = ?1",
                    [column],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing {column}");
        }
    }

    #[test]
    fn migration_048_adds_manual_drop_correction_blob() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM pragma_table_info('audio_dj_profile_corrections')
                 WHERE name = 'manual_drop_blob'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }
}
