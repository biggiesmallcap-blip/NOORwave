use super::*;

#[test]
fn watched_videos_add_capped_artist_affinity_for_library_artists() {
    let conn = Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute_batch(
        "INSERT INTO artists (id, name, tidal_id) VALUES (1, 'Watched', 100), (2, 'Skipped', 200);
         INSERT INTO video_history (tidal_video_id, artist_tidal_id, duration_watched_ms, video_duration_ms, completed) VALUES
            (1, 100, 200000, 200000, 1), (2, 100, 150000, 200000, 0),
            (3, 100, 200000, 200000, 1), (4, 100, 200000, 200000, 1),
            (5, 200, 5000, 200000, 0),
            (6, 999, 200000, 200000, 1)",
    )
    .expect("rows");
    let mut profile = SessionTasteProfile::default();
    add_video_watch_affinity(&conn, &mut profile).expect("affinity");
    let watched = profile.positive_artists.get(&1).copied().unwrap_or(0.0);
    assert!(
        (watched - VIDEO_TASTE_ARTIST_CAP).abs() < 1e-9,
        "four enjoyed watches hit the cap, got {watched}"
    );
    assert!(
        !profile.positive_artists.contains_key(&2),
        "a skip adds nothing"
    );
    assert_eq!(
        profile.positive_artists.len(),
        1,
        "artists outside the library are ignored"
    );
}
use crate::playback::automix::{
    automix_score, automix_scored_reason, build_automix_extension_with_reasons,
    evaluate_automix_for_seed,
};

#[test]
fn clamp_listened_ms_caps_runaway_sessions_at_track_length() {
    // Wall-clock accrual during a stalled stream must not outlive the
    // track (observed: 2795 s recorded on a 334 s track).
    assert_eq!(clamp_listened_ms(2_795_000, Some(334_000)), 334_000);
    assert_eq!(clamp_listened_ms(200_000, Some(334_000)), 200_000);
}

#[test]
fn classify_listen_separates_early_skips_from_partial_plays() {
    use ListenOutcome::*;
    for (completed, listened, duration, expected) in [
        (true, 200_000, Some(200_000), Completed),
        (false, 10_000, Some(200_000), EarlySkip),
        (false, 45_000, Some(200_000), EarlySkip),
        (false, 60_000, Some(200_000), Partial),
        (false, 150_000, Some(200_000), Partial),
        (false, 29_000, None, EarlySkip),
        (false, 31_000, None, Partial),
    ] {
        assert_eq!(
            classify_listen(completed, listened, duration),
            expected,
            "{completed} {listened} {duration:?}"
        );
    }
}

#[test]
fn session_taste_treats_partial_plays_as_neutral_and_ignores_old_listens() {
    let conn = conn();
    conn.execute_batch(
        "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed) VALUES
            (2, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-1 hours'), 10000, 0),
            (3, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-2 hours'), 120000, 0),
            (4, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-5 days'), 5000, 0);",
    )
    .unwrap();
    let current = queue::get_track_by_id(&conn, 1).unwrap().unwrap();

    let profile = build_session_taste_profile(&conn, &current).unwrap();

    assert!(profile.skipped_track_ids.contains(&2), "early skip");
    assert!(
        !profile.skipped_track_ids.contains(&3),
        "partial play is neutral"
    );
    assert!(profile.recent_track_ids.contains(&3));
    assert!(
        !profile.recent_track_ids.contains(&4),
        "older than the window"
    );
    assert!(!profile.skipped_track_ids.contains(&4));
}

#[test]
fn clamp_listened_ms_passes_through_unknown_durations() {
    assert_eq!(clamp_listened_ms(2_795_000, None), 2_795_000);
    assert_eq!(clamp_listened_ms(2_795_000, Some(0)), 2_795_000);
    assert_eq!(clamp_listened_ms(2_795_000, Some(-1)), 2_795_000);
}

fn conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
        CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, year INTEGER, artwork_url TEXT);
        CREATE TABLE tracks (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            artist_id INTEGER NOT NULL,
            album_id INTEGER,
            disc_number INTEGER,
            track_number INTEGER,
            duration_ms INTEGER,
            isrc TEXT,
            tidal_id INTEGER,
            ytmusic_id TEXT,
            soundcloud_id INTEGER,
            best_quality TEXT,
            best_source TEXT,
            fidelity_score INTEGER DEFAULT 0,
            is_favorite INTEGER DEFAULT 0,
            play_count INTEGER DEFAULT 0,
            last_played_at TEXT,
            date_added TEXT,
            source TEXT DEFAULT 'tidal'
        );
        CREATE TABLE queue (
            id               INTEGER PRIMARY KEY,
            track_id         INTEGER,
            position         INTEGER NOT NULL,
            source           TEXT    DEFAULT 'user',
            reason           TEXT,
            pending_artist   TEXT,
            pending_title    TEXT,
            pending_at       TIMESTAMP,
            resolving_at     TIMESTAMP,
            resolved_at      TIMESTAMP,
            tidal_match_score REAL,
            tidal_id_hint    INTEGER,
            ephemeral_album_title TEXT,
            ephemeral_artwork_url TEXT,
            ephemeral_duration_ms INTEGER,
            ephemeral_artist_tidal_id INTEGER,
            ephemeral_album_tidal_id INTEGER
        );
        CREATE TABLE genres (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            slug TEXT NOT NULL,
            parent_id INTEGER
        );
        CREATE TABLE track_genres (
            track_id INTEGER NOT NULL,
            genre_id INTEGER NOT NULL,
            source TEXT,
            confidence REAL DEFAULT 1.0
        );
        CREATE TABLE listen_history (
            id INTEGER PRIMARY KEY,
            track_id INTEGER NOT NULL,
            started_at TEXT NOT NULL,
            duration_listened_ms INTEGER DEFAULT 0,
            completed INTEGER DEFAULT 0
        );
        CREATE TABLE embedding_models (
            id INTEGER PRIMARY KEY,
            model_key TEXT NOT NULL UNIQUE,
            family TEXT NOT NULL,
            dimension INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            is_active INTEGER NOT NULL DEFAULT 0,
            trained_at TEXT,
            config_json TEXT,
            metrics_json TEXT,
            created_at TEXT DEFAULT (datetime('now'))
        );
        CREATE TABLE server_config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE external_track_candidates (
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
            resolved_track_id INTEGER,
            created_at TEXT DEFAULT (datetime('now')),
            updated_at TEXT DEFAULT (datetime('now'))
        );
        CREATE TABLE external_track_candidate_sightings (
            candidate_id INTEGER NOT NULL,
            seed_track_id INTEGER NOT NULL,
            source TEXT NOT NULL,
            source_payload_json TEXT,
            similarity REAL,
            seen_at TEXT DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            PRIMARY KEY (candidate_id, seed_track_id, source)
        );
        CREATE TABLE external_track_candidate_neighbors (
            library_track_id INTEGER NOT NULL,
            candidate_id INTEGER NOT NULL,
            model_id INTEGER NOT NULL,
            rank INTEGER NOT NULL,
            score REAL NOT NULL DEFAULT 0,
            audio_score REAL NOT NULL DEFAULT 0,
            metadata_score REAL NOT NULL DEFAULT 0,
            reason_json TEXT,
            computed_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (library_track_id, candidate_id, model_id)
        );
        CREATE TABLE track_similarity (
            track_a INTEGER NOT NULL,
            track_b INTEGER NOT NULL,
            similarity_score REAL NOT NULL DEFAULT 0,
            co_listen_score REAL DEFAULT 0,
            co_album_score REAL DEFAULT 0,
            co_artist_score REAL DEFAULT 0,
            genre_proximity REAL DEFAULT 0,
            duration_proximity REAL DEFAULT 0,
            era_proximity REAL DEFAULT 0,
            computed_at TEXT,
            PRIMARY KEY (track_a, track_b)
        );
        CREATE TABLE playback_state (
            id INTEGER PRIMARY KEY,
            current_track_id INTEGER,
            current_queue_item_id INTEGER,
            shuffle_seed INTEGER,
            position_ms INTEGER NOT NULL DEFAULT 0,
            is_playing INTEGER NOT NULL DEFAULT 0,
            volume REAL NOT NULL DEFAULT 1.0,
            shuffle_mode TEXT NOT NULL DEFAULT 'off',
            repeat_mode TEXT NOT NULL DEFAULT 'off',
            automix_enabled INTEGER NOT NULL DEFAULT 0,
            crossfade_ms INTEGER NOT NULL DEFAULT 0,
            automix_discover_new INTEGER NOT NULL DEFAULT 0,
            automix_use_learning INTEGER NOT NULL DEFAULT 1,
            automix_allow_external INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .unwrap();

    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])
        .unwrap();
    for id in 1..=6 {
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc,
                tidal_id, ytmusic_id, soundcloud_id, best_quality, best_source, fidelity_score,
                is_favorite, play_count, last_played_at, date_added, source
            ) VALUES (?1, ?2, 1, NULL, 1, ?1, 180000, NULL, ?1, NULL, NULL, 'LOSSLESS', 'tidal', 10, 0, 0, NULL, '2025-01-01', 'tidal')",
            params![id, format!("Track {id}")],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO playback_state (
            id, current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms
        ) VALUES (1, NULL, 0, 0, 1.0, 'off', 'off', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO track_similarity (track_a, track_b, similarity_score, co_artist_score)
         VALUES (2, 3, 0.95, 1.0)",
        [],
    )
    .unwrap();

    conn
}

fn load_tracks(conn: &Connection, ids: &[i64]) -> Vec<Track> {
    ids.iter()
        .map(|id| queue::get_track_by_id(conn, *id).unwrap().unwrap())
        .collect()
}

fn attach_test_dj_transition_plan(
    db: &Database,
    job: PlaybackPreparation,
    sample_rate: u32,
    channels: u16,
) -> Result<PlaybackPreparation> {
    let pair = db.with_conn(load_dj_lookahead_pair)?;
    attach_dj_transition_plan_for_pair(&DjEngine::new(db.clone()), job, pair, sample_rate, channels)
}

mod dj_lookahead {
    use super::*;

    const DEADLINE: u64 = 48_000 * 30;

    fn enable(conn: &Connection) {
        queries::set_dj_engine_enabled(conn, true).unwrap();
    }

    fn start(conn: &Connection) -> Option<DjLookaheadStart> {
        if !queries::is_dj_engine_enabled(conn).unwrap() {
            return None;
        }
        let pair = load_dj_lookahead_pair(conn).unwrap();
        dj_lookahead_start_from_pair(pair, DEADLINE)
    }

    fn seed_queue(conn: &Connection, ids: &[i64]) -> Vec<QueueItem> {
        let tracks = load_tracks(conn, ids);
        queue::replace_queue(conn, &tracks, "test").unwrap()
    }

    #[test]
    fn dj_disabled_queue_events_do_not_start_dj_lookahead() {
        let conn = conn();
        seed_queue(&conn, &[1, 2]);
        play_track_now(&conn, 1).unwrap();

        assert!(start(&conn).is_none());
    }

    #[test]
    fn dj_enable_starts_dj_lookahead_for_active_pair() {
        let conn = conn();
        enable(&conn);
        let queued = seed_queue(&conn, &[1, 2]);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![queued[0].id],
        )
        .unwrap();

        let start = start(&conn).expect("lookahead");
        assert_eq!(start.current_queue_item_id, Some(queued[0].id));
        assert_eq!(start.next_queue_item_id, Some(queued[1].id));
        assert_eq!(start.deadline_samples, DEADLINE);
    }

    #[test]
    fn manual_play_starts_dj_lookahead() {
        let conn = conn();
        enable(&conn);
        let queued = seed_queue(&conn, &[1, 2]);
        play_track_now(&conn, 1).unwrap();

        let start = start(&conn).expect("lookahead");
        assert_eq!(start.current_queue_item_id, Some(queued[0].id));
        assert_eq!(start.next_queue_item_id, Some(queued[1].id));
    }

    #[test]
    fn manual_queue_append_starts_dj_lookahead() {
        let conn = conn();
        enable(&conn);
        let queued = seed_queue(&conn, &[1]);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![queued[0].id],
        )
        .unwrap();
        enqueue_track(&conn, 2, "user").unwrap();

        let start = start(&conn).expect("lookahead");
        assert_eq!(start.current_queue_item_id, Some(queued[0].id));
        assert!(matches!(
            start.next,
            Some(DjMediaRef::TidalTrack { tidal_id: 2, .. })
        ));
    }

    #[test]
    fn play_next_starts_dj_lookahead() {
        let conn = conn();
        enable(&conn);
        let queued = seed_queue(&conn, &[1, 3]);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![queued[0].id],
        )
        .unwrap();
        let play_next = load_tracks(&conn, &[2]).remove(0);
        queue::append_tracks(&conn, &[play_next], "user_play_next").unwrap();
        let inserted = queue::load_queue(&conn).unwrap();
        let inserted_id = inserted.iter().find(|item| item.track.id == 2).unwrap().id;
        queue::move_queue_item(&conn, inserted_id, 1).unwrap();

        let start = start(&conn).expect("lookahead");
        assert_eq!(start.next_queue_item_id, Some(inserted_id));
    }

    #[test]
    fn queue_reorder_restarts_dj_lookahead() {
        let conn = conn();
        enable(&conn);
        let queued = seed_queue(&conn, &[1, 2, 3]);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![queued[0].id],
        )
        .unwrap();
        let before = start(&conn).expect("before");
        queue::move_queue_item(&conn, queued[2].id, 1).unwrap();

        let after = start(&conn).expect("after");
        assert_ne!(before.next_queue_item_id, after.next_queue_item_id);
        assert_eq!(after.next_queue_item_id, Some(queued[2].id));
    }

    #[test]
    fn pending_resolution_restarts_dj_lookahead_with_tidal_ref() {
        let conn = conn();
        enable(&conn);
        let queued = seed_queue(&conn, &[1]);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![queued[0].id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_artist, pending_title)
             VALUES (NULL, 1, 'radio_pending', 'Artist', 'Title')",
            [],
        )
        .unwrap();
        let pending_id = conn.last_insert_rowid();
        let before = start(&conn).expect("before");
        conn.execute(
            "UPDATE queue SET tidal_id_hint = 99 WHERE id = ?1",
            params![pending_id],
        )
        .unwrap();

        let after = start(&conn).expect("after");
        assert_ne!(before.queue_generation, after.queue_generation);
        assert!(matches!(
            after.next,
            Some(DjMediaRef::PendingQueueItem {
                tidal_id_hint: Some(99),
                ..
            })
        ));
    }
}

mod dj_prepare_next {
    use super::*;
    use crate::db::schema;

    fn db_with_pair(next_source: &str) -> Database {
        let db = Database::open_in_memory().expect("db");
        db.with_conn(|conn| {
            schema::run_migrations(conn)?;
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            for id in 1..=4 {
                conn.execute(
                    "INSERT INTO tracks (
                        id, title, artist_id, tidal_id, source, best_quality, best_source, duration_ms
                    ) VALUES (?1, ?2, 1, ?1, 'tidal', 'LOSSLESS', 'tidal', 180000)",
                    params![id, format!("Track {id}")],
                )?;
            }
            conn.execute(
                "INSERT INTO queue (id, track_id, position, source)
                 VALUES (11, 1, 0, 'manual'), (12, 2, 1, ?1)",
                params![next_source],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = 11, is_playing = 1
                 WHERE id = 1",
                [],
            )?;
            Ok(())
        })
        .expect("seed db");
        db
    }

    fn enable(db: &Database) {
        db.with_conn(|conn| queries::set_dj_engine_enabled(conn, true))
            .expect("enable");
    }

    fn next_job(db: &Database) -> PlaybackPreparation {
        db.with_conn(|conn| {
            let track = queue::get_track_by_id(conn, 2)?.expect("track");
            Ok(build_playback_preparation(&track, None, 0, None))
        })
        .expect("next job")
    }

    fn planned_job_for_source(source: &str) -> PlaybackPreparation {
        let db = db_with_pair(source);
        enable(&db);
        attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan")
    }

    #[test]
    fn prepare_next_attaches_program_when_enabled() {
        let job = planned_job_for_source("manual");

        assert!(job.prepared_transition.is_some());
    }

    #[test]
    fn prepared_dj_program_supplies_runtime_overlap() {
        let job = planned_job_for_source("manual");

        assert!(job.prepared_transition.is_some());
        assert!(job.gapless.enabled);
        assert!(job.gapless.overlap_ms > 0);
    }

    #[test]
    fn prepare_next_omits_program_when_disabled() {
        let db = db_with_pair("manual");
        let job = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");

        assert!(job.prepared_transition.is_none());
    }

    #[test]
    fn dj_planning_does_not_reorder_queue() {
        let db = db_with_pair("manual");
        enable(&db);
        let before = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position, id")?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .expect("before");

        let _ = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");
        let after = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position, id")?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .expect("after");

        assert_eq!(after, before);
    }

    #[test]
    fn dj_planning_does_not_replace_next_queue_item() {
        let db = db_with_pair("manual");
        enable(&db);

        let _ = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");
        let next_queue_track = db
            .with_conn(|conn| {
                conn.query_row("SELECT track_id FROM queue WHERE id = 12", [], |row| {
                    row.get::<_, Option<i64>>(0)
                })
                .map_err(anyhow::Error::from)
            })
            .expect("next queue item");

        assert_eq!(next_queue_track, Some(2));
    }

    #[test]
    fn manual_queue_next_uses_plan_transition_path() {
        assert!(
            planned_job_for_source("manual")
                .prepared_transition
                .is_some()
        );
    }

    #[test]
    fn radio_queue_next_uses_plan_transition_path() {
        assert!(
            planned_job_for_source("radio")
                .prepared_transition
                .is_some()
        );
    }

    #[test]
    fn automix_queue_next_uses_plan_transition_path() {
        assert!(
            planned_job_for_source("automix-new")
                .prepared_transition
                .is_some()
        );
    }

    #[test]
    fn external_next_track_uses_same_plan_transition_path() {
        assert!(
            planned_job_for_source("radio_pending")
                .prepared_transition
                .is_some()
        );
    }

    #[test]
    fn pending_next_without_profile_falls_back_to_safe_crossfade() {
        let db = Database::open_in_memory().expect("db");
        db.with_conn(|conn| {
            schema::run_migrations(conn)?;
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, tidal_id, source, best_quality, best_source, duration_ms
                 ) VALUES (1, 'Track 1', 1, 1, 'tidal', 'LOSSLESS', 'tidal', 180000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (id, track_id, position, source, pending_artist, pending_title)
                 VALUES (11, 1, 0, 'manual', NULL, NULL),
                        (12, NULL, 1, 'radio_pending', 'External A', 'External B')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = 11, is_playing = 1
                 WHERE id = 1",
                [],
            )?;
            queries::set_dj_engine_enabled(conn, true)?;
            Ok(())
        })
        .expect("seed pending");
        let pair = db.with_conn(load_dj_lookahead_pair).expect("pair");
        let engine = DjEngine::new(db.clone());
        let job = attach_dj_transition_plan_for_pair(
            &engine,
            PreparedPlaybackJob::test_fixture(99, 1),
            pair,
            48_000,
            2,
        )
        .expect("plan");

        let program = job.prepared_transition.expect("transition").program;
        assert_eq!(program.template, "SafeCrossfade");
    }
}

mod dj_transition_logging {
    use super::*;
    use crate::db::models::{
        AudioDjProfileCorrectionRow, AudioDjProfileKey, AudioDjProfileRow, AudioDspFeatures,
    };
    use crate::db::schema;
    use crate::services::audio_analysis::dj_profile::{
        DJ_PROFILE_VERSION, encode_f32_blob, encode_u32_blob,
    };
    use serde_json::Value;

    fn db_with_pair() -> Database {
        let db = Database::open_in_memory().expect("db");
        db.with_conn(|conn| {
            schema::run_migrations(conn)?;
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, tidal_id, source, best_quality, best_source, duration_ms
                ) VALUES (1, 'Track 1', 1, 1, 'tidal', 'LOSSLESS', 'tidal', 180000),
                         (2, 'Track 2', 1, 2, 'tidal', 'LOSSLESS', 'tidal', 180000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (id, track_id, position, source)
                 VALUES (11, 1, 0, 'manual'), (12, 2, 1, 'manual')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = 11, is_playing = 1
                 WHERE id = 1",
                [],
            )?;
            queries::set_dj_engine_enabled(conn, true)?;
            seed_dsp(conn, 1, "8A")?;
            seed_dsp(conn, 2, "8A")?;
            seed_profile(conn, "tidal_track", "1", Some(1))?;
            seed_profile(conn, "tidal_track", "2", Some(2))?;
            Ok(())
        })
        .expect("seed");
        db
    }

    fn seed_profile(conn: &Connection, kind: &str, id: &str, track_id: Option<i64>) -> Result<()> {
        let row = AudioDjProfileRow {
            media_ref_kind: kind.to_string(),
            media_ref_id: id.to_string(),
            track_id,
            queue_item_id: None,
            tidal_id: id.parse().ok(),
            profile_version: DJ_PROFILE_VERSION.to_string(),
            beat_grid_blob: encode_f32_blob(&(0..64).map(|i| i as f32 * 0.5).collect::<Vec<_>>()),
            downbeats_blob: encode_f32_blob(&(0..16).map(|i| i as f32 * 2.0).collect::<Vec<_>>()),
            phrase_boundaries_blob: encode_u32_blob(&(0..2).collect::<Vec<_>>()),
            mix_in_blob: encode_f32_blob(&[0.0]),
            mix_out_blob: encode_f32_blob(&[90.0]),
            intro_end_seconds: Some(16.0),
            outro_start_seconds: Some(120.0),
            breakdown_blob: encode_f32_blob(&[]),
            drop_blob: encode_f32_blob(&[]),
            safe_transition_windows_blob: encode_f32_blob(&[0.0, 8.0, 1.0]),
            energy_contour_blob: encode_f32_blob(&[]),
            vocal_presence_blob: encode_f32_blob(&[0.0; 2]),
            vocal_density_blob: encode_f32_blob(&[0.0; 2]),
            waveform_peaks_blob: encode_f32_blob(&[0.0, 0.5, 1.0, 0.5]),
            lufs_loud_body: Some(-12.0),
            true_peak_dbtp: Some(-1.0),
            beat_confidence: Some(0.9),
            profile_confidence: 0.9,
            analysis_scope_ms: 90_000,
            is_temporary: false,
            source: "test".to_string(),
            computed_at: "now".to_string(),
        };
        queries::upsert_audio_dj_profile(conn, &row)
    }

    fn seed_dsp(conn: &Connection, track_id: i64, camelot_key: &str) -> Result<()> {
        queries::upsert_audio_dsp_features(
            conn,
            &AudioDspFeatures {
                track_id,
                bpm: Some(120.0),
                key_signature: None,
                camelot_key: Some(camelot_key.to_string()),
                loudness_lufs: Some(-12.0),
                energy: Some(0.5),
                danceability: None,
                beat_strength: None,
                spectral_centroid: None,
                stereo_width: None,
                is_instrumental: false,
                analysis_source: "test".to_string(),
                analysis_offset_ms: 0,
                samples_analyzed: None,
                analyzed_at: "now".to_string(),
                analysis_version: "test".to_string(),
            },
        )
    }

    fn next_job(db: &Database) -> PlaybackPreparation {
        db.with_conn(|conn| {
            let track = queue::get_track_by_id(conn, 2)?.expect("track");
            Ok(build_playback_preparation(&track, None, 0, None))
        })
        .expect("job")
    }

    fn plan(db: &Database) -> PreparedTransitionProgram {
        attach_test_dj_transition_plan(db, next_job(db), 48_000, 2)
            .expect("plan")
            .prepared_transition
            .expect("transition")
    }

    fn event_count(db: &Database) -> i64 {
        db.with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM dj_transition_events", [], |row| {
                row.get(0)
            })
            .map_err(Into::into)
        })
        .expect("count")
    }

    fn insert_timing_sample(
        conn: &Connection,
        delta_ms: i64,
        runtime_rendered_dj_mixer: bool,
        runtime_renderer_status: &str,
        runtime_renderer_reason: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, timing_delta_ms, timing_status,
                timing_source,
                runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                ?1, 'fired', 'downbeat_sync', ?2, ?3, ?4
             )",
            params![
                delta_ms,
                if runtime_rendered_dj_mixer { 1 } else { 0 },
                runtime_renderer_status,
                runtime_renderer_reason,
            ],
        )?;
        Ok(())
    }

    fn make_pair_drop_tease_ready(db: &Database) {
        db.with_conn(|conn| {
            queries::set_dj_global_policy(conn, "bold", "neutral")?;
            let phrase_blob = encode_u32_blob(&(0..4).collect::<Vec<_>>());
            let drop_blob = encode_f32_blob(&[32.0]);
            let vocal_blob = encode_f32_blob(&[0.0; 4]);
            conn.execute(
                "UPDATE audio_dj_profiles
                 SET phrase_boundaries_blob = ?1,
                     vocal_presence_blob = ?2,
                     vocal_density_blob = ?2
                 WHERE media_ref_id = '1'",
                params![&phrase_blob, &vocal_blob],
            )?;
            conn.execute(
                "UPDATE audio_dj_profiles
                 SET phrase_boundaries_blob = ?1,
                     drop_blob = ?2,
                     vocal_presence_blob = ?3,
                     vocal_density_blob = ?3
                 WHERE media_ref_id = '2'",
                params![&phrase_blob, &drop_blob, &vocal_blob],
            )?;
            Ok(())
        })
        .expect("drop tease profile fixture");
    }

    #[test]
    fn latest_open_dj_transition_event_for_pair_prefers_fired_event() {
        let db = db_with_pair();
        let fired = plan(&db);
        let fired_id = fired.transition_event_id.expect("fired event");
        db.with_conn(|conn| {
            queries::update_dj_transition_fire_timing(
                conn,
                fired_id,
                172_040,
                "fired",
                true,
                "rendered_handoff",
                "none",
            )
        })
        .expect("mark fired");
        let duplicate = plan(&db);
        assert_ne!(duplicate.transition_event_id, Some(fired_id));

        let selected = db
            .with_conn(|conn| latest_open_dj_transition_event_for_pair(conn, Some(1), 2))
            .expect("selected");

        assert_eq!(selected, Some(fired_id));
        // A new late execution is still this session's mix. An older
        // tight fire must not replace its cue, renderer or listen outcome.
        let late_id = duplicate.transition_event_id.unwrap();
        db.with_conn(|conn| {
            queries::update_dj_transition_fire_timing(
                conn,
                late_id,
                172_400,
                "late",
                true,
                "rendered_handoff",
                "next_decode_late_at_fire",
            )
        })
        .expect("mark newer late execution");
        assert_eq!(
            db.with_conn(|conn| latest_open_dj_transition_event_for_pair(conn, Some(1), 2))
                .expect("latest execution"),
            Some(late_id)
        );
    }

    #[test]
    fn repeated_planning_reuses_existing_armed_event_for_pair() {
        let db = db_with_pair();
        let first = plan(&db);
        let second = plan(&db);

        assert_eq!(second.transition_event_id, first.transition_event_id);
        assert_eq!(event_count(&db), 1);
    }

    #[test]
    fn future_policy_replan_retains_event_and_refuses_to_rewrite_fired_audio() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            conn.execute("DELETE FROM audio_dj_profiles", [])?;
            Ok(())
        })
        .expect("missing analysis");
        let first = plan(&db);
        let first_id = first.transition_event_id.unwrap();
        db.with_conn(|conn| queries::set_dj_global_policy(conn, "balanced", "slower"))
            .expect("slower policy");
        let engine = DjEngine::new(db.clone());
        let pair = db.with_conn(load_dj_lookahead_pair).unwrap();
        let update = plan_prepared_dj_transition_update(&engine, pair, 48_000, 2, 90_000)
            .unwrap()
            .expect("future update");
        assert_eq!(update.transition.transition_event_id, Some(first_id));
        assert_eq!(update.gapless.overlap_ms, 9000);
        persist_prepared_dj_transition_update(&engine, &update).unwrap();
        assert_eq!(event_count(&db), 1);
        let pair = db.with_conn(load_dj_lookahead_pair).unwrap();
        assert!(
            plan_prepared_dj_transition_update(&engine, pair, 48_000, 2, 90_000)
                .unwrap()
                .is_none()
        );
        db.with_conn(|conn| {
            queries::update_dj_transition_fire_timing(
                conn,
                first_id,
                171_006,
                "fired",
                true,
                "rendered_handoff",
                "none",
            )
        })
        .unwrap();
        db.with_conn(|conn| queries::set_dj_global_policy(conn, "balanced", "faster"))
            .unwrap();
        let pair = db.with_conn(load_dj_lookahead_pair).unwrap();
        assert!(
            plan_prepared_dj_transition_update(&engine, pair, 48_000, 2, 90_000)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn armed_event_reuse_restores_its_audio_and_structural_windows() {
        for rhythmic in [false, true] {
            let db = db_with_pair();
            db.with_conn(|conn| {
                queries::set_dj_global_policy(conn, "safe", "neutral")?;
                if rhythmic {
                    // Safe audio is 6s, while its measured grid alignment
                    // reserves 8s of the outgoing tail.
                    conn.execute("UPDATE audio_dj_profiles SET profile_confidence = .65", [])?;
                    // Conservative personality still admits a good smooth
                    // blend. Use the real safe-only control for this fixture.
                    conn.execute(
                        "INSERT INTO audio_dj_profile_corrections
                        (media_ref_kind, media_ref_id, safe_crossfade_only)
                        VALUES ('tidal_track', '1', 1)",
                        [],
                    )?;
                } else {
                    conn.execute("UPDATE audio_dj_profiles SET beat_confidence = .05", [])?;
                }
                Ok(())
            })
            .unwrap();
            let prepare = || {
                let mut job = next_job(&db);
                job.gapless = GaplessPlan {
                    enabled: true,
                    overlap_ms: 5_000,
                    prebuffer_ms: 500,
                    requires_stream_metadata: true,
                };
                attach_test_dj_transition_plan(&db, job, 48_000, 2).unwrap()
            };
            let first = prepare();
            let repeated = prepare();
            assert_eq!(
                first.gapless.overlap_ms,
                if rhythmic { 8_000 } else { 6_000 }
            );
            assert_eq!(
                repeated.gapless, first.gapless,
                "a reused event must not inherit the configured 5s crossfade"
            );
            assert_eq!(
                repeated
                    .prepared_transition
                    .as_ref()
                    .unwrap()
                    .transition_event_id,
                first
                    .prepared_transition
                    .as_ref()
                    .unwrap()
                    .transition_event_id
            );
            assert_eq!(
                repeated
                    .prepared_transition
                    .as_ref()
                    .unwrap()
                    .anchor_start_ms,
                first.prepared_transition.as_ref().unwrap().anchor_start_ms
            );
            assert_eq!(event_count(&db), 1);
        }
    }

    #[test]
    fn partial_scope_keeps_measured_rhythm_but_cannot_certify_synthetic_phase() {
        let db = db_with_pair();
        let current = DjMediaRef::TidalTrack {
            track_id: Some(1),
            tidal_id: 1,
        };
        let program = noor_mix::planner::bass_swap_16_program(48_000, 2, 16_000);
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE audio_dj_profiles SET profile_confidence = .65, beat_confidence = .9",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let measured = db
            .with_conn(|conn| synced_dj_overlap_ms(conn, &current, Some(180_000), &program, 16_000))
            .unwrap()
            .unwrap();
        assert_eq!(measured.overlap_ms, 16_000);
        assert_eq!(measured.timing_source, "downbeat_sync");
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE audio_dj_profiles SET source = 'dj_playback', beat_confidence = 1.0",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            db.with_conn(|conn| synced_dj_overlap_ms(
                conn,
                &current,
                Some(180_000),
                &program,
                16_000
            ))
            .unwrap()
            .is_none(),
            "uniform zero-origin tempo projections cannot certify phase"
        );
    }

    #[test]
    fn missing_profile_armed_event_replans_when_profile_arrives() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            conn.execute(
                "DELETE FROM audio_dj_profiles
                 WHERE media_ref_kind = 'tidal_track' AND media_ref_id = '2'",
                [],
            )?;
            Ok(())
        })
        .expect("remove next profile");

        let fallback = plan(&db);
        let fallback_id = fallback.transition_event_id.expect("fallback event");
        assert_eq!(fallback.program.template, "SafeCrossfade");
        let fallback_reason: Option<String> = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT fallback_reason FROM dj_transition_events WHERE id = ?1",
                    params![fallback_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("fallback reason");
        assert_eq!(fallback_reason.as_deref(), Some("next_profile_missing"));

        db.with_conn(|conn| seed_profile(conn, "tidal_track", "2", Some(2)))
            .expect("seed next profile");
        let replanned = plan(&db);

        assert_eq!(replanned.transition_event_id, Some(fallback_id));
        assert_eq!(replanned.program.template, "LongHarmonicBlend");
        assert_eq!(event_count(&db), 1);
        let row: (String, Option<String>) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT template, fallback_reason
                     FROM dj_transition_events WHERE id = ?1",
                    params![fallback_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(Into::into)
            })
            .expect("replanned row");
        assert_eq!(row.0, "LongHarmonicBlend");
        assert_eq!(row.1, None);
    }

    #[test]
    fn dj_disabled_does_not_write_dj_transition_events() {
        let db = db_with_pair();
        db.with_conn(|conn| queries::set_dj_engine_enabled(conn, false))
            .expect("disable");

        let job = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");

        assert!(job.prepared_transition.is_none());
        assert_eq!(event_count(&db), 0);
    }

    #[test]
    fn dj_transition_logging_does_not_replace_playback_transitions() {
        let db = db_with_pair();
        let _ = plan(&db);
        db.with_conn(|conn| queries::record_playback_transition(conn, 1, 2, "queue", true, 8000))
            .expect("legacy transition");

        let counts = db
            .with_conn(|conn| {
                let dj: i64 =
                    conn.query_row("SELECT COUNT(*) FROM dj_transition_events", [], |row| {
                        row.get(0)
                    })?;
                let legacy: i64 =
                    conn.query_row("SELECT COUNT(*) FROM playback_transitions", [], |row| {
                        row.get(0)
                    })?;
                Ok((dj, legacy))
            })
            .expect("counts");

        assert_eq!(counts, (1, 1));
    }

    #[test]
    fn dj_transition_logging_stores_no_fabricated_rejected_alternatives() {
        let db = db_with_pair();
        let transition = plan(&db);
        let rejected: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT rejected_alternatives_json FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("rejected");
        let parsed: Vec<Value> = serde_json::from_str(&rejected).expect("json");

        // Admitted candidate scores live in program.decision. This legacy
        // rejection field must not fabricate scores for discarded choices.
        assert!(parsed.is_empty());
    }

    #[test]
    fn dj_transition_logging_uses_planner_version_not_profile_version() {
        let db = db_with_pair();
        let transition = plan(&db);
        let planner_version: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT planner_version FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("planner version");

        assert_eq!(planner_version, noor_mix::planner::DJ_PLANNER_VERSION);
        assert_ne!(planner_version, DJ_PROFILE_VERSION);
    }

    #[test]
    fn v1_planner_logs_and_prepares_bass_swap_16_when_renderable() {
        let db = db_with_pair();
        db.with_conn(|conn| queries::set_dj_preferred_strategy(conn, "bass_swap"))
            .expect("prefer bass swap for the renderer contract");
        let transition = plan(&db);

        assert_eq!(transition.program.template, "BassSwap16");
        let row: (String, String, Option<String>) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT template, program_json, fallback_reason
                     FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(Into::into)
            })
            .expect("event");
        let renderer_program: noor_mix::TransitionProgram =
            serde_json::from_str(&row.1).expect("program");

        assert_eq!(row.0, "BassSwap16");
        assert_eq!(renderer_program.template, "BassSwap16");
        assert_eq!(renderer_program.resolve_at, 768_000);
        assert_eq!(row.2, None);
    }

    #[test]
    fn v1_planner_keeps_drop_tease_overlay_out_of_end_transition() {
        let db = db_with_pair();
        make_pair_drop_tease_ready(&db);
        db.with_conn(|conn| queries::set_dj_preferred_strategy(conn, "bass_swap"))
            .expect("prefer bass swap for the end transition");
        let transition = plan(&db);

        assert_eq!(transition.program.template, "BassSwap32");
        let row: (String, String, Option<String>) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT template, program_json, fallback_reason
                     FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(Into::into)
            })
            .expect("event");
        let renderer_program: noor_mix::TransitionProgram =
            serde_json::from_str(&row.1).expect("program");

        assert_eq!(row.0, "BassSwap32");
        assert_eq!(renderer_program.template, "BassSwap32");
        assert_eq!(row.2, None);
    }

    #[test]
    fn v1_planner_logs_and_prepares_filter_sweep_when_renderable() {
        let db = db_with_pair();
        // An unsyncable 5% delta only stays a FilterSweep under bold
        // intent; balanced intent now degrades to SafeCrossfade.
        db.with_conn(|conn| {
            queries::set_dj_global_policy(conn, "bold", "neutral")?;
            // Independent tempo corroborates the corrected measured
            // grid; this renderer test must not depend on contradictory
            // analysis being admitted as beat-accurate evidence.
            conn.execute(
                "UPDATE audio_dsp_features SET bpm = 126.0 WHERE track_id = 2",
                [],
            )?;
            queries::upsert_audio_dj_profile_correction(
                conn,
                &AudioDjProfileCorrectionRow {
                    media_ref_kind: "tidal_track".to_string(),
                    media_ref_id: "2".to_string(),
                    bpm_multiplier: Some(1.05),
                    downbeat_offset_beats: None,
                    phrase_offset_bars: None,
                    safe_crossfade_only: false,
                    transition_speed_bias: None,
                    manual_drop_blob: Vec::new(),
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .expect("seed correction");
        let transition = plan(&db);

        assert_eq!(transition.program.template, "FilterSweep");
        let row: (String, String, Option<String>) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT template, program_json, fallback_reason
                     FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(Into::into)
            })
            .expect("event");
        let renderer_program: noor_mix::TransitionProgram =
            serde_json::from_str(&row.1).expect("program");

        assert_eq!(row.0, "FilterSweep");
        assert_eq!(renderer_program.template, "FilterSweep");
        assert_eq!(renderer_program.resolve_at, 384_000);
        assert_eq!(row.2, None);
    }

    #[test]
    fn v1_renderable_program_preserves_filter_sweep_duration() {
        let input = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 20_000);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

        assert_eq!(program.template, "FilterSweep");
        assert_eq!(program.resolve_at, 960_000);
        assert_eq!(reason, None);
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == noor_mix::Param::HighGain(noor_mix::DeckId::A))
        );
    }

    #[test]
    fn v1_renderable_program_passes_bass_swap_16_with_low_handoff() {
        let mut input = noor_mix::planner::bass_swap_16_program(48_000, 2, 16_000);
        input.deck_b_start_frame = 384_000;
        input.automation.push(noor_mix::AutomationEvent {
            param: noor_mix::Param::PlaybackRate(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: input.resolve_at,
            from: 0.985,
            to: 0.985,
            curve: noor_mix::Curve::Linear,
        });

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

        assert_eq!(program.template, "BassSwap16");
        assert_eq!(program.resolve_at, 768_000);
        assert_eq!(program.deck_b_start_frame, 384_000);
        assert_eq!(reason, None);
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
            .expect("rate automation");
        assert_eq!(rate.from, 0.985);
        assert_eq!(rate.to, 0.985);
        assert!(program.automation.iter().any(|event| event.param
            == noor_mix::Param::LowGain(noor_mix::DeckId::B)
            && event.start_sample == program.swap_start
            && event.to == 1.0));
    }

    #[test]
    fn v1_renderable_program_passes_bass_swap_32_with_low_handoff() {
        let input = noor_mix::planner::bass_swap_32_program(48_000, 2, 16_000);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

        assert_eq!(program.template, "BassSwap32");
        assert_eq!(program.resolve_at, 768_000);
        assert_eq!(reason, None);
        assert!(program.automation.iter().any(|event| event.param
            == noor_mix::Param::LowGain(noor_mix::DeckId::B)
            && event.start_sample == program.swap_start
            && event.to == 1.0));
    }

    #[test]
    fn v1_renderable_program_passes_slam_cut_as_short_gain_cut() {
        let input = noor_mix::planner::slam_cut_program(48_000, 2, 40);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

        assert_eq!(program.template, "SlamCut");
        assert_eq!(program.resolve_at, 1_920);
        assert_eq!(reason, None);
        assert!(program.automation.iter().all(|event| !matches!(
            event.param,
            noor_mix::Param::LowGain(_)
                | noor_mix::Param::MidGain(_)
                | noor_mix::Param::HighGain(_)
        )));
    }

    #[test]
    fn v1_renderable_program_passes_long_harmonic_blend_rate() {
        let input = noor_mix::planner::long_harmonic_blend_program(48_000, 2, 16_000, 0.985);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

        assert_eq!(program.template, "LongHarmonicBlend");
        assert_eq!(program.resolve_at, 768_000);
        assert_eq!(reason, None);
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
            .expect("rate automation")
            .to;
        assert_eq!(rate, 0.985);
    }

    #[test]
    fn v1_renderable_program_defaults_long_harmonic_blend_rate() {
        let input = noor_mix::planner::long_harmonic_blend_program(48_000, 2, 16_000, 1.0);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

        assert_eq!(program.template, "LongHarmonicBlend");
        assert_eq!(reason, None);
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
            .expect("rate automation")
            .to;
        assert_eq!(rate, 1.0);
    }

    #[test]
    fn v1_renderable_program_preserves_drop_tease_as_overlay() {
        let mut input = noor_mix::planner::drop_tease_16_program(48_000, 2, 10_000);
        input.drop_source = Some("manual_drop_cue".to_string());
        input.deck_b_start_frame = 384_000;

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

        assert_eq!(program.template, "DropTease16");
        assert_eq!(program.deck_b_start_frame, 384_000);
        assert_eq!(reason, None);
        assert!(program.automation.iter().any(|event| event.param
            == noor_mix::Param::DeckGain(noor_mix::DeckId::A)
            && event.from == 0.0
            && event.to == 0.0));
    }

    #[test]
    fn renderer_preserves_strategy_automation_phase_and_drop_provenance() {
        let mut input = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
        input.template = "EnergyLift".to_string();
        input.intro_start = 48_000;
        input.fade_start = 400_000;
        input.deck_b_start_frame = 96_000;
        input.drop_source = Some("profile_drop_candidate".to_string());
        input.automation[0].curve = noor_mix::Curve::Cosine;
        let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);
        assert_eq!(reason, None);
        assert_eq!(program, input);
    }

    #[test]
    fn renderer_rescales_every_planner_marker_without_rebuilding_envelope() {
        let input = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
        let (program, reason) = v1_renderable_program(&input, 44_100, 2, false);
        assert_eq!(reason, None);
        assert_eq!(program, input.rescaled_to(44_100));
    }

    #[test]
    fn renderer_accepts_new_styles_without_collapsing_their_programs() {
        for template in [
            "ClubMix",
            "QuickMix",
            "EnergyLift",
            "EnergyReset",
            "DropSwap",
        ] {
            let mut input = noor_mix::planner::bass_swap_16_program(48_000, 2, 4_000);
            input.template = template.to_string();
            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);
            assert_eq!(reason, None, "{template}");
            assert_eq!(program, input, "{template}");
        }
    }

    #[test]
    fn renderer_rejects_oversized_invalid_and_dynamic_rate_programs_to_safety() {
        let oversized = noor_mix::planner::bass_swap_16_program(48_000, 2, 32_000);
        let mut invalid = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
        invalid.swap_start = invalid.resolve_at + 1;
        let mut dynamic = noor_mix::planner::long_harmonic_blend_program(48_000, 2, 12_000, 1.0);
        let rate = dynamic
            .automation
            .iter_mut()
            .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
            .unwrap();
        rate.to = 1.02;
        for input in [oversized, invalid, dynamic] {
            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);
            assert_eq!(program.template, "SafeCrossfade");
            assert_eq!(reason, Some("audio_safety_rejected"));
            assert_eq!(program.resolve_at, 288_000);
            assert!(
                !program
                    .automation
                    .iter()
                    .any(|event| matches!(event.param, noor_mix::Param::PlaybackRate(_)))
            );
        }
    }

    #[test]
    fn short_cut_schedules_its_own_tail_window_and_honours_downbeat_correction() {
        let db = db_with_pair();
        let program = noor_mix::planner::slam_cut_program(48_000, 2, 40);
        let current = DjMediaRef::TidalTrack {
            track_id: Some(1),
            tidal_id: 1,
        };
        let overlap = db
            .with_conn(|conn| synced_dj_overlap_ms(conn, &current, Some(180_000), &program, 250))
            .unwrap()
            .unwrap();
        assert_eq!(overlap.overlap_ms, 2_000);
        db.with_conn(|conn| {
            queries::upsert_audio_dj_profile_correction(
                conn,
                &AudioDjProfileCorrectionRow {
                    media_ref_kind: "tidal_track".to_string(),
                    media_ref_id: "1".to_string(),
                    bpm_multiplier: None,
                    downbeat_offset_beats: Some(1),
                    phrase_offset_bars: None,
                    safe_crossfade_only: false,
                    transition_speed_bias: None,
                    manual_drop_blob: vec![],
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .unwrap();
        let corrected = db
            .with_conn(|conn| synced_dj_overlap_ms(conn, &current, Some(180_000), &program, 250))
            .unwrap()
            .unwrap();
        assert_eq!(corrected.overlap_ms, 1_500);
    }

    #[test]
    fn incomplete_analysis_never_earns_a_false_tail_phrase_and_weak_beats_are_not_projected() {
        let db = db_with_pair();
        let program = noor_mix::planner::bass_swap_16_program(48_000, 2, 20_000);
        let current = DjMediaRef::TidalTrack {
            track_id: Some(1),
            tidal_id: 1,
        };
        db.with_conn(|conn| {
            let key = current.profile_key();
            let mut profile = queries::get_audio_dj_profile(conn, &key)?.unwrap();
            // A malformed/out-of-scope structural cue must not make a
            // partial 90s analysis claim to know the true 180s tail.
            profile.mix_out_blob = encode_f32_blob(&[156.0]);
            profile.outro_start_seconds = Some(156.0);
            queries::upsert_audio_dj_profile(conn, &profile)
        })
        .unwrap();
        let overlap = db
            .with_conn(|conn| synced_dj_overlap_ms(conn, &current, Some(180_000), &program, 20_000))
            .unwrap()
            .unwrap();
        assert_eq!(overlap.overlap_ms, 20_000);
        assert_eq!(overlap.timing_source, "downbeat_sync");
        db.with_conn(|conn| {
            let key = current.profile_key();
            let mut profile = queries::get_audio_dj_profile(conn, &key)?.unwrap();
            profile.beat_confidence = Some(0.4);
            queries::upsert_audio_dj_profile(conn, &profile)
        })
        .unwrap();
        let weak = db
            .with_conn(|conn| synced_dj_overlap_ms(conn, &current, Some(180_000), &program, 20_000))
            .unwrap();
        assert!(weak.is_none());
    }

    #[test]
    fn unstable_timing_downgrades_filter_sweep_to_safe_crossfade() {
        let input = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 10_000);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, true);

        assert_eq!(program.template, "SafeCrossfade");
        assert_eq!(reason, Some("timing_unstable"));
    }

    #[test]
    fn short_phase_uncertain_energy_handoffs_keep_their_envelopes_with_old_phase_jitter() {
        for template in ["EnergyLift", "EnergyReset"] {
            let mut input = noor_mix::planner::bass_swap_16_program(48_000, 2, 4_000);
            input.template = template.into();
            input.decision = Some(noor_mix::program::TransitionDecision {
                strategy: template.into(),
                confidence: 0.65,
                score: 0.7,
                reason: "Compatible tempo; phase is unverified".into(),
                energy_direction: "lift".into(),
                incoming_entry_seconds: 0.0,
                incoming_drop_seconds: None,
                outgoing_window: "tempo_informed_short_overlap".into(),
                duration_beats: 8.0,
                candidates: vec![],
            });
            let (rendered, reason) = v1_renderable_program(&input, 48_000, 2, true);
            assert_eq!(reason, None);
            assert_eq!(rendered, input);
            input.decision.as_mut().unwrap().outgoing_window = "phrase_end".into();
            assert_eq!(
                v1_renderable_program(&input, 48_000, 2, true).1,
                Some("timing_unstable")
            );
        }
    }

    #[test]
    fn unstable_timing_downgrades_bass_swap_16_to_safe_crossfade() {
        let input = noor_mix::planner::bass_swap_16_program(48_000, 2, 16_000);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, true);

        assert_eq!(program.template, "SafeCrossfade");
        assert_eq!(reason, Some("timing_unstable"));
    }

    #[test]
    fn unstable_timing_downgrades_bass_swap_32_to_safe_crossfade() {
        let input = noor_mix::planner::bass_swap_32_program(48_000, 2, 32_000);

        let (program, reason) = v1_renderable_program(&input, 48_000, 2, true);

        assert_eq!(program.template, "SafeCrossfade");
        assert_eq!(reason, Some("timing_unstable"));
    }

    #[test]
    fn dj_renderers_use_wider_overlap_than_safe_crossfade() {
        let safe = crate::playback::dj_engine::safe_crossfade_program(
            48_000,
            2,
            noor_mix::Policy::default(),
        );
        let filter = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 18_000);
        let bass_swap = noor_mix::planner::bass_swap_16_program(48_000, 2, 24_000);
        let bass_swap_32 = noor_mix::planner::bass_swap_32_program(48_000, 2, 28_000);

        // SafeCrossfade dropped from 12s to 6s: two full-spectrum tracks
        // fighting for 12 seconds read as mud next to the beat-matched
        // FullBlend renders, which keep their longer windows.
        assert_eq!(dj_gapless_plan_from_program(&safe).overlap_ms, 6_000);
        assert_eq!(dj_gapless_plan_from_program(&filter).overlap_ms, 18_000);
        assert_eq!(dj_gapless_plan_from_program(&bass_swap).overlap_ms, 24_000);
        assert_eq!(
            dj_gapless_plan_from_program(&bass_swap_32).overlap_ms,
            28_000
        );
    }

    #[test]
    fn fire_ahead_requires_latest_twenty_positive_evidence() {
        let passing = vec![
            412, 709, 270, 475, 8, 827, 258, 738, 8, 35, 252, 141, -375, 210, 73, 529, -53, 48,
            481, 300,
        ];
        let mixed = vec![
            412, 709, 270, 475, -8, -827, -258, -738, -8, -35, 252, 141, -375, 210, 73, 529, -53,
            48, 481, 300,
        ];
        let low_median = vec![
            151, 150, 149, 148, 147, 146, 145, 144, 143, 142, 141, 140, 139, 138, 137, 136, 135,
            134, -20, -40,
        ];

        assert_eq!(fire_ahead_ms_from_deltas(&passing), 127);
        assert_eq!(fire_ahead_ms_from_deltas(&mixed), 0);
        assert_eq!(fire_ahead_ms_from_deltas(&low_median), 0);
        assert_eq!(fire_ahead_ms_from_deltas(&passing[..19]), 0);
    }

    #[test]
    fn renderer_timing_gate_uses_abs_error_not_signed_bias() {
        assert!(render_timing_unstable_from_deltas(&[549, -399, 303, -375]));
        assert!(!render_timing_unstable_from_deltas(&[140, -130, 75, -90]));
        assert!(!render_timing_unstable_from_deltas(&[549, -399, 303]));
    }

    #[test]
    fn timing_calibration_ignores_decode_fallback_rows() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            conn.execute("DELETE FROM dj_transition_events", [])?;
            for delta_ms in [549, -399, 303, -375] {
                insert_timing_sample(
                    conn,
                    delta_ms,
                    false,
                    "legacy_overlap",
                    "active_deck_not_decoded",
                )?;
            }
            Ok(())
        })
        .expect("seed fallback timing");

        let unstable = db.with_conn(render_timing_unstable).expect("gate");

        assert!(!unstable);
    }

    #[test]
    fn timing_calibration_uses_successful_dj_mixer_rows() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            conn.execute("DELETE FROM dj_transition_events", [])?;
            for delta_ms in [549, -399, 303, -375] {
                insert_timing_sample(conn, delta_ms, true, "rendered_handoff", "none")?;
            }
            Ok(())
        })
        .expect("seed rendered timing");

        let unstable = db.with_conn(render_timing_unstable).expect("gate");

        assert!(unstable);
    }

    #[test]
    fn old_session_timing_cannot_permanently_veto_current_mixes() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            for _ in 0..20 {
                insert_timing_sample(conn, 2_000, true, "rendered_handoff", "none")?;
            }
            assert!(render_timing_unstable(conn)?);
            conn.execute(
                "UPDATE dj_transition_events SET started_at = datetime('now', '-2 days')",
                [],
            )?;
            for delta in [3, 7, 1, 6] {
                insert_timing_sample(conn, delta, true, "rendered_handoff", "none")?;
            }
            assert!(!render_timing_unstable(conn)?);
            assert_eq!(dj_transition_fire_ahead_ms(conn)?, 0);
            assert_eq!(
                conn.query_row("SELECT count(*) FROM dj_transition_events", [], |row| row
                    .get::<_, i64>(
                    0
                ))?,
                24
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn rendered_fallback_duration_errors_do_not_calibrate_creative_timing() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            for _ in 0..20 {
                insert_timing_sample(conn, 2_000, true, "rendered_handoff", "none")?;
            }
            conn.execute(
                "UPDATE dj_transition_events SET timing_source = 'fallback_overlap'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.with_conn(dj_transition_fire_ahead_ms).unwrap(), 0);
        assert!(!db.with_conn(render_timing_unstable).unwrap());
    }

    #[test]
    fn fire_ahead_ignores_fallback_timing_rows() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            conn.execute("DELETE FROM dj_transition_events", [])?;
            for _ in 0..20 {
                insert_timing_sample(conn, 500, false, "legacy_overlap", "next_deck_not_decoded")?;
            }
            Ok(())
        })
        .expect("seed fallback timing");

        let fire_ahead_ms = db
            .with_conn(dj_transition_fire_ahead_ms)
            .expect("fire ahead");

        assert_eq!(fire_ahead_ms, 0);
    }

    #[test]
    fn fire_ahead_caps_large_median() {
        assert_eq!(fire_ahead_ms_from_deltas(&[500; 20]), 150);
    }

    #[test]
    fn external_dj_transition_logging_does_not_require_library_track_id() {
        let db = db_with_pair();
        db.with_conn(|conn| {
            conn.execute("DELETE FROM queue WHERE id = 12", [])?;
            conn.execute(
                "INSERT INTO queue (id, track_id, position, source, pending_artist, pending_title)
                 VALUES (12, NULL, 1, 'radio_pending', 'External A', 'External B')",
                [],
            )?;
            seed_profile(conn, "queue_item", "12", None)?;
            Ok(())
        })
        .expect("external");
        let pair = db.with_conn(load_dj_lookahead_pair).expect("pair");
        let engine = DjEngine::new(db.clone());
        let job = attach_dj_transition_plan_for_pair(
            &engine,
            PreparedPlaybackJob::test_fixture(99, 1),
            pair,
            48_000,
            2,
        )
        .expect("plan");

        let event_id = job
            .prepared_transition
            .and_then(|transition| transition.transition_event_id)
            .expect("event");
        let row = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT to_track_id, to_media_ref_kind, to_media_ref_id
                     FROM dj_transition_events WHERE id = ?1",
                    params![event_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(Into::into)
            })
            .expect("event");

        assert_eq!(row.0, None);
        assert_eq!(row.1, "queue_item");
        assert_eq!(row.2, "12");
    }

    #[test]
    fn skip_mid_transition_updates_dj_event() {
        let db = db_with_pair();
        let transition = plan(&db);

        db.with_conn(|conn| {
            record_dj_transition_listen_outcome(conn, transition.transition_event_id, 12_000, false)
        })
        .expect("outcome");

        let outcome: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT outcome FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("outcome");
        assert_eq!(outcome, "skip_within_30s");
    }

    #[test]
    fn skip_within_30s_counts_as_negative_transition_outcome() {
        let db = db_with_pair();
        let transition = plan(&db);

        db.with_conn(|conn| {
            record_dj_transition_listen_outcome(conn, transition.transition_event_id, 29_999, false)
        })
        .expect("outcome");

        let skip_flag: i64 = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT skip_within_30s FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("flag");
        assert_eq!(skip_flag, 1);
    }

    #[test]
    fn skip_after_30s_does_not_count_as_bad_transition_feedback() {
        let db = db_with_pair();
        let transition = plan(&db);

        db.with_conn(|conn| {
            record_dj_transition_listen_outcome(conn, transition.transition_event_id, 30_000, false)
        })
        .expect("outcome");

        let row = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT outcome, skip_within_30s FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(Into::into)
            })
            .expect("row");
        assert_eq!(row, (None, 0));
    }

    #[test]
    fn manual_bad_feedback_counts_stronger_than_skip() {
        let db = db_with_pair();
        let transition = plan(&db);

        db.with_conn(|conn| {
            record_dj_transition_listen_outcome(
                conn,
                transition.transition_event_id,
                12_000,
                false,
            )?;
            conn.execute(
                "UPDATE dj_transition_events SET user_rating = -1 WHERE id = ?1",
                params![transition.transition_event_id],
            )?;
            queries::count_recent_bad_dj_feedback_for_ref(
                conn,
                &AudioDjProfileKey {
                    media_ref_kind: "tidal_track".to_string(),
                    media_ref_id: "2".to_string(),
                },
                3,
            )
        })
        .map(|count| assert_eq!(count, 1))
        .expect("feedback count");
    }

    #[test]
    fn finished_transition_updates_dj_event() {
        let db = db_with_pair();
        let transition = plan(&db);

        db.with_conn(|conn| {
            record_dj_transition_listen_outcome(conn, transition.transition_event_id, 170_000, true)
        })
        .expect("outcome");

        let row = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT outcome, skip_within_30s FROM dj_transition_events WHERE id = ?1",
                    params![transition.transition_event_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(Into::into)
            })
            .expect("row");
        assert_eq!(row, ("finished".to_string(), 0));
    }
}

fn track_with_tidal_id(id: i64, tidal_id: Option<i64>, quality: Option<&str>) -> Track {
    Track {
        id,
        title: format!("Track {id}"),
        artist_id: 1,
        artist_name: Some("A".to_string()),
        album_id: None,
        album_title: None,
        disc_number: Some(1),
        track_number: Some(id as i32),
        duration_ms: Some(180_000),
        isrc: None,
        tidal_id,
        artist_tidal_id: None,
        album_tidal_id: None,
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: quality.map(|s| s.to_string()),
        best_source: tidal_id.map(|_| "tidal".to_string()),
        fidelity_score: 10,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: Some("2025-01-01".to_string()),
        source: if tidal_id.is_some() {
            "tidal".to_string()
        } else {
            "local".to_string()
        },
        artwork_url: None,
    }
}

fn blank_dsp_features() -> AudioDspFeatures {
    AudioDspFeatures {
        track_id: 0,
        bpm: None,
        key_signature: None,
        camelot_key: None,
        loudness_lufs: None,
        energy: None,
        danceability: None,
        beat_strength: None,
        spectral_centroid: None,
        stereo_width: None,
        is_instrumental: false,
        analysis_source: "test".to_string(),
        analysis_offset_ms: 0,
        samples_analyzed: None,
        analyzed_at: "2026-01-01T00:00:00Z".to_string(),
        analysis_version: "test".to_string(),
    }
}

#[test]
fn automix_reason_does_not_claim_harmonic_match_without_key_or_bpm() {
    let profile = SessionTasteProfile {
        current_source: Some("tidal".to_string()),
        ..SessionTasteProfile::default()
    };
    let (taste, seed) = from_session_profile(&profile);
    let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));
    let seed_features = blank_dsp_features();
    let candidate_features = blank_dsp_features();

    let score = automix_score(
        &track,
        &[],
        &taste,
        &seed,
        Some(&seed_features),
        Some(&candidate_features),
    );
    let reason = automix_scored_reason(&score);

    assert!(reason.contains("same source"), "got: {reason}");
    assert!(
        !reason.contains("harmonic")
            && !reason.contains("key clash")
            && !reason.contains("adjacent key"),
        "missing key and BPM must produce no harmonic signal at all: {reason}"
    );
}

#[test]
fn automix_reason_marks_recent_skip_as_penalty_not_cause() {
    // The candidate's artist carries negative session affinity, so
    // automix_score penalizes it. The reason must render that as a
    // "despite" clause, never as a cause the track was picked.
    let profile = SessionTasteProfile {
        current_source: Some("tidal".to_string()),
        negative_artists: HashMap::from([(1, 1.0)]),
        ..SessionTasteProfile::default()
    };
    let (taste, seed) = from_session_profile(&profile);
    let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));

    let score = automix_score(&track, &[], &taste, &seed, None, None);
    let reason = automix_scored_reason(&score);

    assert!(
        reason.contains("despite") && reason.contains("recent skip penalty"),
        "a penalizing signal must be rendered as a penalty: {reason}"
    );
    let lead = reason.split(" despite ").next().unwrap_or("");
    assert!(
        !lead.contains("recent skip penalty"),
        "a penalty must never appear in the selection-cause lead: {reason}"
    );
}

#[test]
fn automix_reason_marks_energy_whiplash_as_penalty() {
    // A large energy jump multiplies the score *down*; the old reason
    // builder mislabeled it "energy contrast" as if it were a cause.
    let profile = SessionTasteProfile {
        current_source: Some("tidal".to_string()),
        ..SessionTasteProfile::default()
    };
    let (taste, seed) = from_session_profile(&profile);
    let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));
    let seed_features = AudioDspFeatures {
        energy: Some(0.2),
        ..blank_dsp_features()
    };
    let candidate_features = AudioDspFeatures {
        energy: Some(0.9),
        ..blank_dsp_features()
    };

    let score = automix_score(
        &track,
        &[],
        &taste,
        &seed,
        Some(&seed_features),
        Some(&candidate_features),
    );
    let reason = automix_scored_reason(&score);

    assert!(
        reason.contains("despite") && reason.contains("energy whiplash"),
        "a large energy jump is a penalty, not a selection cause: {reason}"
    );
}

#[test]
fn automix_reason_does_not_claim_harmonic_match_on_key_clash_with_close_bpm() {
    // 8A vs 11A is a Camelot clash, but a near-identical BPM pushes the
    // *combined* harmonic multiplier above 1.0. The reason must still call
    // it a key clash - deriving the signal from the Camelot relationship,
    // not the blended multiplier.
    let profile = SessionTasteProfile {
        current_source: Some("tidal".to_string()),
        ..SessionTasteProfile::default()
    };
    let (taste, seed) = from_session_profile(&profile);
    let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));
    let seed_features = AudioDspFeatures {
        camelot_key: Some("8A".to_string()),
        bpm: Some(120.0),
        ..blank_dsp_features()
    };
    let candidate_features = AudioDspFeatures {
        camelot_key: Some("11A".to_string()),
        bpm: Some(122.0),
        ..blank_dsp_features()
    };

    let score = automix_score(
        &track,
        &[],
        &taste,
        &seed,
        Some(&seed_features),
        Some(&candidate_features),
    );
    let reason = automix_scored_reason(&score);

    assert!(
        reason.contains("key clash"),
        "a Camelot clash must be labeled a key clash: {reason}"
    );
    assert!(
        !reason.contains("harmonic match") && !reason.contains("adjacent key"),
        "a key clash must never be shown as a harmonic match: {reason}"
    );
}

#[test]
fn previous_track_moves_back_when_under_threshold() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state SET current_track_id = 2, position_ms = 0, is_playing = 1 WHERE id = 1",
        [],
    )
    .unwrap();

    let outcome = previous_track(&conn, 2_500, None).unwrap();

    assert!(!outcome.restart_in_place);
    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
    assert_eq!(outcome.snapshot.state.position_ms, 0);
    assert!(outcome.snapshot.state.is_playing);
}

#[test]
fn previous_track_restarts_current_track_when_over_threshold() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    // DB position_ms stays 0 during playback (nothing persists the live
    // playhead); the threshold must key on the caller-provided live
    // position, never this column.
    conn.execute(
        "UPDATE playback_state SET current_track_id = 2, position_ms = 0, is_playing = 1 WHERE id = 1",
        [],
    )
    .unwrap();

    let outcome = previous_track(&conn, PREVIOUS_RESTART_THRESHOLD_MS, None).unwrap();

    assert!(outcome.restart_in_place);
    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 2);
    assert_eq!(outcome.snapshot.state.position_ms, 0);
}

#[test]
fn previous_track_restarts_first_track_when_no_previous_exists() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state SET current_track_id = 1, position_ms = 0, is_playing = 1 WHERE id = 1",
        [],
    )
    .unwrap();

    let outcome = previous_track(&conn, 1_000, None).unwrap();

    assert!(outcome.restart_in_place);
    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
    assert_eq!(outcome.snapshot.state.position_ms, 0);
}

#[test]
fn previous_track_prefers_valid_history_anchor() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    // Simulate a shuffled/jumped session: current is the FIRST row, but
    // history says the third row actually played before it. Queue-order
    // stepping would restart-in-place; history must win.
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
         WHERE id = 1",
        params![queue_items[0].id],
    )
    .unwrap();

    let anchor = HistoryAnchor {
        queue_item_id: queue_items[2].id,
        track_id: Some(queue_items[2].track.id),
    };
    let outcome = previous_track(&conn, 500, Some(&anchor)).unwrap();

    assert!(!outcome.restart_in_place);
    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 3);
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[2].id)
    );
}

#[test]
fn previous_track_falls_back_when_history_anchor_row_is_gone() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
         WHERE id = 1",
        params![queue_items[1].id],
    )
    .unwrap();

    let anchor = HistoryAnchor {
        queue_item_id: 999_999,
        track_id: Some(3),
    };
    let outcome = previous_track(&conn, 500, Some(&anchor)).unwrap();

    // Stale anchor: fall back to queue-order stepping (row above).
    assert!(!outcome.restart_in_place);
    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
}

#[test]
fn previous_track_rejects_history_anchor_with_changed_track() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
         WHERE id = 1",
        params![queue_items[1].id],
    )
    .unwrap();

    // Anchor row exists but now holds a different track (re-resolved /
    // edited): must not navigate onto the wrong track.
    let anchor = HistoryAnchor {
        queue_item_id: queue_items[2].id,
        track_id: Some(999),
    };
    let outcome = previous_track(&conn, 500, Some(&anchor)).unwrap();

    assert!(!outcome.restart_in_place);
    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
}

#[test]
fn previous_track_ignores_mismatched_current_queue_item_id() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
         WHERE id = 1",
        params![queue_items[0].id],
    )
    .unwrap();

    let outcome = previous_track(&conn, 1_000, None).unwrap();

    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[0].id)
    );
    assert_eq!(outcome.snapshot.state.position_ms, 0);
}

#[test]
fn previous_track_accepts_pending_current_queue_item_id() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "INSERT INTO queue (track_id, position, source, pending_artist, pending_title)
         VALUES (NULL, 1, 'radio_pending', 'Pending Artist', 'Pending Title')",
        [],
    )
    .unwrap();
    let pending_qid = conn.last_insert_rowid();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = NULL, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
         WHERE id = 1",
        params![pending_qid],
    )
    .unwrap();

    let outcome = previous_track(&conn, 1_000, None).unwrap();

    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[0].id)
    );
    assert_eq!(outcome.snapshot.state.position_ms, 0);
}

#[test]
fn previous_track_selects_first_queue_item_when_nothing_is_playing() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();

    let outcome = previous_track(&conn, 0, None).unwrap();

    assert!(!outcome.restart_in_place);
    assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
    assert_eq!(outcome.snapshot.state.position_ms, 0);
    assert!(outcome.snapshot.state.is_playing);
}

#[test]
fn previous_track_clears_state_when_queue_is_empty() {
    let conn = conn();
    conn.execute(
        "UPDATE playback_state SET current_track_id = 1, position_ms = 1500, is_playing = 1 WHERE id = 1",
        [],
    )
    .unwrap();

    let outcome = previous_track(&conn, 5_000, None).unwrap();

    assert!(!outcome.restart_in_place);
    assert!(outcome.snapshot.state.current_track.is_none());
    assert_eq!(outcome.snapshot.state.position_ms, 0);
    assert!(!outcome.snapshot.state.is_playing);
    assert!(outcome.snapshot.queue.is_empty());
}

#[test]
fn build_tidal_stream_request_uses_track_quality_or_defaults() {
    let track = track_with_tidal_id(42, Some(88), Some("HI_RES_LOSSLESS"));

    let request = build_tidal_stream_request(&track, None).unwrap();

    assert_eq!(request.track_id, 88);
    assert_eq!(request.audio_quality, "HI_RES_LOSSLESS");
    assert_eq!(request.playback_mode, "STREAM");
    assert_eq!(request.asset_presentation, "FULL");
}

#[test]
fn build_playback_preparation_marks_local_tracks_as_local_library() {
    let track = track_with_tidal_id(7, None, None);

    let prep = build_playback_preparation(&track, None, 1500, None);

    assert!(prep.is_local());
    assert_eq!(prep.source_kind(), PlaybackSourceKind::LocalLibrary);
    assert!(prep.stream_request().is_none());
    assert!(prep.dj_media_ref.is_none());
    assert!(!prep.gapless.enabled);
    assert_eq!(prep.track.id, 7);
}

#[test]
fn build_playback_preparation_includes_tidal_stream_request() {
    let track = track_with_tidal_id(7, Some(77), Some("LOSSLESS"));
    let stream = StreamInfo {
        url: "https://example.com/stream.flac".to_string(),
        segment_urls: vec![],
        segment_offsets_ms: vec![],
        track_id: 77,
        audio_quality: "LOSSLESS".to_string(),
        codec: "audio/flac".to_string(),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
    };

    let prep = build_playback_preparation(&track, Some(&stream), 1500, None);

    assert!(prep.is_tidal());
    assert_eq!(prep.source_kind(), PlaybackSourceKind::TidalStream);
    let request = prep.stream_request().expect("expected a tidal request");
    assert_eq!(request.track_id, 77);
    assert_eq!(request.audio_quality, "LOSSLESS");
    assert!(prep.gapless.enabled);
    assert_eq!(prep.gapless.overlap_ms, 1500);
    assert_eq!(prep.output_sample_rate, Some(44_100));
    assert_eq!(
        prep.dj_media_ref
            .as_ref()
            .map(|media_ref| media_ref.profile_key()),
        Some(crate::db::models::AudioDjProfileKey {
            media_ref_kind: "tidal_track".to_string(),
            media_ref_id: "77".to_string(),
        })
    );
}

#[test]
fn phrase_candidate_can_win_within_a_bounded_tail_without_a_mid_song_skip() {
    let candidate = tail_transition_candidate_ms(
        120_000,
        &[92.0, 94.0, 96.0, 98.0, 100.0],
        20_000,
        20_000,
        "ClubMix",
        false,
        &[96_000],
        &[],
        None,
        "downbeat_sync",
    )
    .unwrap();
    assert_eq!(candidate, (24_000, "phrase_sync"));
    let early_phrase = tail_transition_candidate_ms(
        120_000,
        &[20.0, 94.0, 96.0, 98.0, 100.0],
        20_000,
        20_000,
        "ClubMix",
        false,
        &[20_000],
        &[],
        None,
        "downbeat_sync",
    )
    .unwrap();
    assert_eq!(early_phrase, (20_000, "downbeat_sync"));
}

#[test]
fn quick_mix_uses_a_shorter_grid_window_and_unstable_grid_cannot_project() {
    let candidate = tail_transition_candidate_ms(
        120_000,
        &[114.0, 116.0, 118.0],
        3_000,
        3_000,
        "QuickMix",
        false,
        &[],
        &[],
        None,
        "downbeat_sync",
    )
    .unwrap();
    assert_eq!(candidate.0, 4_000);
    assert!(stable_grid(&[0.0, 2.0, 4.0, 6.0, 8.0]));
    assert!(!stable_grid(&[0.0, 0.1, 4.0, 4.3, 10.0]));
}

#[test]
fn extrapolated_grid_preserves_fractional_period_and_fitted_phase() {
    let bpm = 174.0_f64;
    let beats = (0..250)
        .map(|beat| ((0.13 + beat as f64 * 60.0 / bpm) * 100.0).round() as f32 / 100.0)
        .collect::<Vec<_>>();
    let projected = extrapolated_grid_ms(&beats, 270_000).unwrap();
    let last = *projected.last().unwrap() as f64 / 1000.0;
    let index = ((last - 0.13) * bpm / 60.0).round();
    let actual = 0.13 + index * 60.0 / bpm;
    assert!(
        (last - actual).abs() < 0.02,
        "tail phase error={}s",
        last - actual
    );
}

#[test]
fn synced_overlap_uses_projected_downbeat_before_track_end() {
    let overlap_ms =
        synced_overlap_from_grid_ms(180_000, &[0.0, 2.0, 4.0], 8_000, Some(8_000 * 48), 48_000)
            .expect("overlap");

    assert_eq!(overlap_ms, 8_000);
}

#[test]
fn synced_overlap_keeps_preferred_minimum_when_last_grid_is_too_close() {
    let overlap_ms =
        synced_overlap_from_grid_ms(181_000, &[0.0, 2.0, 4.0], 8_000, Some(8_000 * 48), 48_000)
            .expect("overlap");

    assert_eq!(overlap_ms, 9_000);
}

#[test]
fn synced_overlap_allows_longer_dj_handoff_windows() {
    let overlap_ms =
        synced_overlap_from_grid_ms(180_000, &[0.0, 2.0, 4.0], 24_000, Some(24_000 * 48), 48_000)
            .expect("overlap");

    assert_eq!(overlap_ms, 24_000);
}

#[test]
fn synced_overlap_extrapolation_ignores_near_duplicate_grid_noise() {
    // A 10 ms near-duplicate pair must not become the extrapolation
    // interval; the median keeps the projected grid on the real 2 s bar
    // spacing. With a min-interval flood every 10 ms this would return
    // exactly 8_000 from a fake marker instead of 9_000 from a real one.
    let overlap_ms = synced_overlap_from_grid_ms(
        181_000,
        &[0.0, 2.0, 2.01, 4.0, 6.0, 8.0],
        8_000,
        Some(8_000 * 48),
        48_000,
    )
    .expect("overlap");

    assert_eq!(overlap_ms, 9_000);
}

#[test]
fn armed_event_anchor_requires_grid_timing_source() {
    let event = |timing_source: Option<&str>| ArmedDjTransitionEvent {
        id: 1,
        program: noor_mix::TransitionProgram {
            tier: noor_mix::program::Tier::SafeCrossfade,
            template: "SafeCrossfade".to_string(),
            drop_source: None,
            decision: None,
            sample_rate: 48_000,
            channels: 2,
            deck_a_start_frame: 0,
            deck_b_start_frame: 0,
            sync_start: 0,
            intro_start: 0,
            swap_start: 1,
            fade_start: 1,
            resolve_at: 2,
            loops: vec![],
            automation: vec![],
        },
        fallback_reason: None,
        planned_start_ms: Some(200_000),
        timing_source: timing_source.map(str::to_string),
    };

    // Grid-derived plans fire against the planned start directly.
    assert_eq!(
        event(Some("downbeat_sync")).anchor_start_ms(),
        Some(200_000)
    );
    assert_eq!(event(Some("beat_sync")).anchor_start_ms(), Some(200_000));
    assert_eq!(event(Some("phrase_sync")).anchor_start_ms(), Some(200_000));
    assert_eq!(event(Some("mix_out_sync")).anchor_start_ms(), Some(200_000));
    // A fallback overlap's planned start is metadata arithmetic; firing
    // against it would reintroduce the duration-mismatch error.
    assert_eq!(event(Some("fallback_overlap")).anchor_start_ms(), None);
    assert_eq!(event(None).anchor_start_ms(), None);
}

#[test]
fn completed_listen_uses_ninety_percent_or_four_minute_cap() {
    let short_track = track_with_tidal_id(1, Some(42), Some("LOSSLESS"));
    assert!(is_completed_listen(&short_track, 162_000));
    assert!(!is_completed_listen(&short_track, 161_999));

    let long_track = Track {
        duration_ms: Some(600_000),
        ..track_with_tidal_id(2, Some(99), Some("LOSSLESS"))
    };
    assert!(is_completed_listen(&long_track, 240_000));
    assert!(!is_completed_listen(&long_track, 239_999));
}

#[test]
fn next_track_extends_queue_when_automix_is_enabled() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
         WHERE id = 1",
        [],
    )
    .unwrap();

    let snapshot = next_track(&conn, false).unwrap();

    assert_eq!(snapshot.state.current_track.unwrap().id, 3);
    assert!(snapshot.queue.len() > 2);
    assert!(snapshot.queue.iter().any(|item| item.source == "automix"));
}

#[test]
fn ensure_automix_queue_depth_records_reasons_for_generated_rows() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
         WHERE id = 1",
        [],
    )
    .unwrap();

    let queue = ensure_automix_queue_depth(&conn, AUTOMIX_MIN_UPCOMING, false).unwrap();
    let generated = queue
        .iter()
        .filter(|item| item.source == "automix")
        .collect::<Vec<_>>();

    assert!(!generated.is_empty(), "expected generated automix rows");
    assert!(
        generated.iter().all(|item| item
            .reason
            .as_deref()
            .is_some_and(|reason| !reason.trim().is_empty())),
        "generated automix rows should persist selection reasons: {generated:?}"
    );
}

#[test]
fn peek_next_track_can_see_generated_automix_track() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
         WHERE id = 1",
        [],
    )
    .unwrap();

    let next = peek_next_track(&conn, false)
        .unwrap()
        .expect("generated automix track");

    assert_eq!(next.id, 3);
    let queue_items = queue::load_queue(&conn).unwrap();
    assert!(queue_items.len() > 2);
}

#[test]
fn peek_next_track_uses_current_queue_item_id_for_duplicate_tracks() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 1, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    let second_track_one = queue_items
        .iter()
        .find(|item| item.position == 2 && item.track.id == 1)
        .expect("second copy of track 1");
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![second_track_one.id],
    )
    .unwrap();

    let next = peek_next_track(&conn, false).unwrap().expect("next track");

    assert_eq!(next.id, 3);
}

#[test]
fn peek_next_track_ignores_mismatched_current_queue_item_id() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_items[0].id],
    )
    .unwrap();

    let next = peek_next_track(&conn, false).unwrap().expect("next track");

    assert_eq!(next.id, 3);
}

#[test]
fn peek_next_track_returns_first_queue_item_when_unanchored() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 1
         WHERE id = 1",
        [],
    )
    .unwrap();

    let next = peek_next_track(&conn, false).unwrap().expect("first track");

    assert_eq!(next.id, 1);
}

#[test]
fn ensure_automix_queue_depth_suppresses_refill_when_recently_cleared() {
    // Same setup as `next_track_extends_queue_when_automix_is_enabled`:
    // two tracks queued, automix on, current = 2. The non-suppressed
    // refill path is already covered by that sibling test; here we
    // verify the new gate alone - that with `recently_cleared = true`
    // the helper short-circuits before any extension work and returns
    // the existing queue unmodified.
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
         WHERE id = 1",
        [],
    )
    .unwrap();

    // Suppression on - must return the 2 existing items, never call the
    // extension path (which would otherwise touch tables not present in
    // this minimal test schema and panic).
    let suppressed = ensure_automix_queue_depth(&conn, AUTOMIX_MIN_UPCOMING, true).unwrap();
    assert_eq!(
        suppressed.len(),
        2,
        "suppressed call must not extend the queue"
    );
    assert!(
        !suppressed.iter().any(|item| item.source == "automix"),
        "suppressed call must not append automix rows"
    );
    let stored = queue::load_queue(&conn).unwrap();
    assert_eq!(
        stored.len(),
        2,
        "DB queue must be untouched while suppressed"
    );
}

/// Last.fm linked the candidate to track 1 (the playing seed).
fn sight_for_seed_one(conn: &Connection, candidate_id: i64, similarity: f64) {
    queries::upsert_external_candidate_sighting(
        conn,
        &queries::ExternalCandidateSightingUpsert {
            candidate_id,
            seed_track_id: 1,
            source: "lastfm_similar".to_string(),
            source_payload_json: None,
            similarity: Some(similarity),
            expires_at: "2099-01-01 00:00:00".to_string(),
        },
    )
    .unwrap();
}

#[test]
fn include_new_uses_the_lastfm_lane_and_skips_weak_links() {
    let conn = conn();
    let current = queue::get_tracks_by_ids(&conn, &[1]).unwrap().remove(0);
    queue::append_tracks(&conn, std::slice::from_ref(&current), "user").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, position_ms = 0, is_playing = 1, automix_enabled = 1,
             automix_allow_external = 0, automix_discover_new = 1, automix_use_learning = 0",
        [],
    )
    .unwrap();
    let candidate = |tidal_id: i64, title: &str| {
        queries::upsert_external_track_candidate(
            &conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(tidal_id),
                mbid: None,
                dedupe_key: format!("tidal:{tidal_id}"),
                title: title.to_string(),
                artist_name: "Outside Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )
        .unwrap()
    };
    let strong = candidate(99101, "Strong Link");
    let weak = candidate(99102, "Weak Link");
    sight_for_seed_one(&conn, strong.id, 0.7);
    sight_for_seed_one(&conn, weak.id, 0.05);

    let queue = ensure_automix_queue_depth(&conn, 1, false).unwrap();

    let external: Vec<&str> = queue
        .iter()
        .filter(|item| item.source == "automix-new")
        .map(|item| item.track.title.as_str())
        .collect();
    assert_eq!(external, vec!["Strong Link"]);
}

#[test]
fn ensure_automix_external_enabled_appends_pending_sidecar_rows() {
    let conn = conn();
    let current = queue::get_tracks_by_ids(&conn, &[1]).unwrap().remove(0);
    queue::append_tracks(&conn, std::slice::from_ref(&current), "user").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, position_ms = 0, is_playing = 1,
             automix_enabled = 1, automix_allow_external = 1, automix_use_learning = 0",
        [],
    )
    .unwrap();
    let model = queries::create_embedding_model(
        &conn,
        "discovery-fusion-v2:test-external",
        "discovery-fusion-v2",
        32,
        "ready",
        None,
    )
    .unwrap();
    queries::activate_embedding_model(&conn, model.id).unwrap();
    let candidate = queries::upsert_external_track_candidate(
        &conn,
        &queries::ExternalTrackCandidateUpsert {
            tidal_id: Some(99001),
            mbid: None,
            dedupe_key: "tidal:99001".to_string(),
            title: "Outside Track".to_string(),
            artist_name: "Outside Artist".to_string(),
            genre_tags_json: None,
            duration_ms: Some(180_000),
            expires_at: "2099-01-01 00:00:00".to_string(),
        },
    )
    .unwrap();
    sight_for_seed_one(&conn, candidate.id, 0.9);

    let queue = ensure_automix_queue_depth(&conn, 1, false).unwrap();

    let pending = queue
        .iter()
        .find(|item| item.source == "automix-new")
        .expect("pending external automix row");
    assert!(pending.is_pending);
    assert_eq!(pending.track.title, "Outside Track");
    let tidal_hint: Option<i64> = conn
        .query_row(
            "SELECT tidal_id_hint FROM queue WHERE id = ?1",
            params![pending.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tidal_hint, Some(99001));
}

#[test]
fn ensure_automix_external_overfetches_past_already_queued_candidates() {
    let conn = conn();
    let current = queue::get_tracks_by_ids(&conn, &[1]).unwrap().remove(0);
    queue::append_tracks(&conn, std::slice::from_ref(&current), "user").unwrap();
    queue::append_external_track(
        &conn,
        &queue::ExternalTrackInsert {
            artist: "Outside Artist",
            title: "Already Queued",
            source: "automix-new",
            reason: Some("external similarity"),
            tidal_id_hint: Some(99001),
            ..Default::default()
        },
    )
    .unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, position_ms = 0, is_playing = 1,
             automix_enabled = 1, automix_allow_external = 1, automix_use_learning = 0",
        [],
    )
    .unwrap();
    let model = queries::create_embedding_model(
        &conn,
        "discovery-fusion-v2:test-external-overfetch",
        "discovery-fusion-v2",
        32,
        "ready",
        None,
    )
    .unwrap();
    queries::activate_embedding_model(&conn, model.id).unwrap();
    let first = queries::upsert_external_track_candidate(
        &conn,
        &queries::ExternalTrackCandidateUpsert {
            tidal_id: Some(99001),
            mbid: None,
            dedupe_key: "tidal:99001".to_string(),
            title: "Already Queued".to_string(),
            artist_name: "Outside Artist".to_string(),
            genre_tags_json: None,
            duration_ms: Some(180_000),
            expires_at: "2099-01-01 00:00:00".to_string(),
        },
    )
    .unwrap();
    let second = queries::upsert_external_track_candidate(
        &conn,
        &queries::ExternalTrackCandidateUpsert {
            tidal_id: Some(99002),
            mbid: None,
            dedupe_key: "tidal:99002".to_string(),
            title: "Fresh External".to_string(),
            artist_name: "Outside Artist".to_string(),
            genre_tags_json: None,
            duration_ms: Some(181_000),
            expires_at: "2099-01-01 00:00:00".to_string(),
        },
    )
    .unwrap();
    sight_for_seed_one(&conn, first.id, 0.95);
    sight_for_seed_one(&conn, second.id, 0.9);

    ensure_automix_queue_depth(&conn, 2, false).unwrap();

    let hints = conn
        .prepare(
            "SELECT tidal_id_hint
             FROM queue
             WHERE source = 'automix-new'
             ORDER BY position",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, Option<i64>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(hints.iter().filter(|hint| **hint == Some(99001)).count(), 1);
    assert!(hints.contains(&Some(99002)));
}

#[test]
fn ensure_automix_external_skips_hidden_ai_candidates_without_failing() {
    let conn = conn();
    conn.execute_batch(
        "ALTER TABLE tracks ADD COLUMN is_library INTEGER DEFAULT 0;
         CREATE TABLE tidal_track_labels (tidal_id INTEGER PRIMARY KEY, ai INTEGER);
         INSERT INTO server_config (key, value) VALUES ('tidal_hide_ai', '1');
         INSERT INTO tidal_track_labels (tidal_id, ai) VALUES (99001, 1);",
    )
    .unwrap();
    let current = queue::get_tracks_by_ids(&conn, &[1]).unwrap().remove(0);
    queue::append_tracks(&conn, std::slice::from_ref(&current), "user").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, position_ms = 0, is_playing = 1,
             automix_enabled = 1, automix_allow_external = 1, automix_use_learning = 0",
        [],
    )
    .unwrap();
    let model = queries::create_embedding_model(
        &conn,
        "discovery-fusion-v2:test-external-ai",
        "discovery-fusion-v2",
        32,
        "ready",
        None,
    )
    .unwrap();
    queries::activate_embedding_model(&conn, model.id).unwrap();
    let candidate = |tidal_id: i64, title: &str| {
        queries::upsert_external_track_candidate(
            &conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(tidal_id),
                mbid: None,
                dedupe_key: format!("tidal:{tidal_id}"),
                title: title.to_string(),
                artist_name: "Outside Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )
        .unwrap()
    };
    let hidden = candidate(99001, "Hidden AI Track");
    let fresh = candidate(99002, "Fresh External");
    sight_for_seed_one(&conn, hidden.id, 0.9);
    sight_for_seed_one(&conn, fresh.id, 0.8);

    let queue = ensure_automix_queue_depth(&conn, 1, false)
        .expect("hidden content must not fail the refill");

    let external: Vec<&str> = queue
        .iter()
        .filter(|item| item.source == "automix-new")
        .map(|item| item.track.title.as_str())
        .collect();
    assert_eq!(external, vec!["Fresh External"]);
}

#[test]
fn ensure_automix_queue_depth_anchors_to_duplicate_current_queue_item() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 1]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    let second_track_one = queue_items
        .iter()
        .find(|item| item.position == 2 && item.track.id == 1)
        .expect("second copy of track 1");
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1,
             automix_enabled = 1, shuffle_mode = 'off'
         WHERE id = 1",
        params![second_track_one.id],
    )
    .unwrap();

    let queue = ensure_automix_queue_depth(&conn, 1, false).unwrap();

    assert!(
        queue.len() > queue_items.len(),
        "automix should refill from the active duplicate row"
    );
    assert!(queue.iter().any(|item| item.source == "automix"));
}

#[test]
fn play_track_now_sets_current_queue_item_id() {
    let conn = conn();
    // Seed two queue rows pointing at the same track so the "lowest
    // position" tiebreak is testable.
    let tracks = load_tracks(&conn, &[1, 1, 2]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    let q = queue::load_queue(&conn).unwrap();

    play_track_now(&conn, 1).unwrap();
    let state = load_state(&conn).unwrap();
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
    // Of the two rows pointing at track 1, the lowest-position one wins.
    let expected_qid = q.iter().find(|i| i.track.id == 1).unwrap().id;
    assert_eq!(state.current_queue_item_id, Some(expected_qid));
    assert!(state.is_playing);
}

#[test]
fn play_queue_item_anchor_jumps_to_the_exact_row() {
    let conn = conn();
    // Two rows share track 1; anchoring by queue-item id must pick the
    // exact clicked row, which play-by-track-id cannot do.
    let tracks = load_tracks(&conn, &[1, 1, 2]);
    let items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    let second_row_of_track1 = items.iter().filter(|i| i.track.id == 1).nth(1).unwrap().id;

    let snap = play_queue_item_anchor(&conn, second_row_of_track1)
        .unwrap()
        .unwrap();
    assert_eq!(snap.state.current_queue_item_id, Some(second_row_of_track1));
    assert_eq!(snap.state.current_track.as_ref().map(|t| t.id), Some(1));
    assert!(snap.state.is_playing);
}

#[test]
fn play_queue_item_anchor_pending_row_sets_null_track() {
    let conn = conn();
    conn.execute(
        "INSERT INTO queue (track_id, position, source, pending_artist, pending_title,
                            pending_at, tidal_id_hint)
         VALUES (NULL, 0, 'radio_pending', 'Artist', 'Title', datetime('now'), 555)",
        [],
    )
    .unwrap();
    let qid: i64 = conn
        .query_row("SELECT id FROM queue", [], |r| r.get(0))
        .unwrap();

    let snap = play_queue_item_anchor(&conn, qid).unwrap().unwrap();
    // Pending row: anchored by queue item with a NULL track, exactly the
    // shape resolve_or_skip_pending_current expects to pick up.
    assert_eq!(snap.state.current_queue_item_id, Some(qid));
    assert!(snap.state.current_track.is_none());
}

#[test]
fn play_queue_item_anchor_missing_row_returns_none() {
    let conn = conn();
    assert!(play_queue_item_anchor(&conn, 12345).unwrap().is_none());
}

#[test]
fn remove_current_queue_item_advances_to_next_survivor() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_items[1].id],
    )
    .unwrap();

    let outcome = remove_queue_item_and_reconcile(&conn, queue_items[1].id).unwrap();

    assert!(outcome.removed_current);
    assert_eq!(
        outcome
            .snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.id),
        Some(3)
    );
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[2].id)
    );
    assert!(outcome.snapshot.state.is_playing);
    assert_eq!(outcome.snapshot.queue.len(), 2);
}

#[test]
fn remove_current_queue_item_repairs_stale_anchor_before_reconcile() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = 999999, is_playing = 1
         WHERE id = 1",
        [],
    )
    .unwrap();

    let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

    assert!(outcome.removed_current);
    assert_eq!(
        outcome
            .snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.id),
        Some(2)
    );
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[1].id)
    );
    assert!(outcome.snapshot.state.is_playing);
}

#[test]
fn remove_current_queue_item_repairs_mismatched_anchor_before_reconcile() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_items[1].id],
    )
    .unwrap();

    let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

    assert!(outcome.removed_current);
    assert_eq!(
        outcome
            .snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.id),
        Some(2)
    );
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[1].id)
    );
    assert!(outcome.snapshot.state.is_playing);
}

#[test]
fn remove_current_queue_item_repairs_missing_anchor_before_reconcile() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = NULL, is_playing = 0
         WHERE id = 1",
        [],
    )
    .unwrap();

    let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

    assert!(outcome.removed_current);
    assert!(!outcome.was_playing);
    assert_eq!(
        outcome
            .snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.id),
        Some(2)
    );
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[1].id)
    );
    assert!(!outcome.snapshot.state.is_playing);
}

#[test]
fn remove_current_queue_item_stops_when_no_survivor_exists() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_items[0].id],
    )
    .unwrap();

    let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

    assert!(outcome.removed_current);
    assert!(outcome.snapshot.state.current_track.is_none());
    assert_eq!(outcome.snapshot.state.current_queue_item_id, None);
    assert!(!outcome.snapshot.state.is_playing);
    assert!(outcome.snapshot.queue.is_empty());
}

#[test]
fn remove_paused_current_queue_item_preserves_paused_state() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 0
         WHERE id = 1",
        params![queue_items[0].id],
    )
    .unwrap();

    let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

    assert!(outcome.removed_current);
    assert!(!outcome.was_playing);
    assert_eq!(
        outcome
            .snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.id),
        Some(2)
    );
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[1].id)
    );
    assert!(!outcome.snapshot.state.is_playing);
}

#[test]
fn remove_previous_queue_item_preserves_current_queue_item_anchor() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 3, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_items[2].id],
    )
    .unwrap();

    let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

    assert!(!outcome.removed_current);
    assert_eq!(
        outcome
            .snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.id),
        Some(3)
    );
    assert_eq!(
        outcome.snapshot.state.current_queue_item_id,
        Some(queue_items[2].id)
    );
    assert_eq!(outcome.snapshot.queue.len(), 2);
    assert_eq!(outcome.snapshot.queue[0].position, 0);
    assert_eq!(outcome.snapshot.queue[1].position, 1);
}

#[test]
fn setting_shuffle_off_clears_shuffle_seed() {
    let conn = conn();
    conn.execute(
        "UPDATE playback_state SET shuffle_mode = 'genre', shuffle_seed = 12345 WHERE id = 1",
        [],
    )
    .unwrap();

    let update = set_shuffle_mode(&conn, ShuffleMode::Off).unwrap();
    let stored_seed: Option<i64> = conn
        .query_row(
            "SELECT shuffle_seed FROM playback_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(update.snapshot.state.shuffle_mode, "off");
    assert!(update.debug.is_none());
    assert_eq!(stored_seed, None);
}

#[test]
fn setting_shuffle_repairs_stale_current_queue_anchor() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3, 4]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    let current_qid = queue_items[1].id;
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2,
             current_queue_item_id = 999999,
             is_playing = 1
         WHERE id = 1",
        [],
    )
    .unwrap();

    let update = set_shuffle_mode(&conn, ShuffleMode::True).unwrap();
    let debug = update.debug.expect("shuffle debug");
    let stored_current_qid: Option<i64> = conn
        .query_row(
            "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(stored_current_qid, Some(current_qid));
    assert_eq!(
        update.snapshot.state.current_queue_item_id,
        Some(current_qid)
    );
    assert_eq!(debug.locked_count, 2);
    assert_eq!(debug.candidate_count, 2);
    assert_eq!(
        update
            .snapshot
            .queue
            .iter()
            .position(|item| item.id == current_qid),
        Some(1)
    );
}

#[test]
fn setting_shuffle_repairs_mismatched_current_queue_anchor() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3, 4]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    let mismatched_qid = queue_items[0].id;
    let current_qid = queue_items[1].id;
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2,
             current_queue_item_id = ?1,
             is_playing = 1
         WHERE id = 1",
        params![mismatched_qid],
    )
    .unwrap();

    let update = set_shuffle_mode(&conn, ShuffleMode::True).unwrap();
    let debug = update.debug.expect("shuffle debug");
    let stored_current_qid: Option<i64> = conn
        .query_row(
            "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(stored_current_qid, Some(current_qid));
    assert_eq!(
        update.snapshot.state.current_queue_item_id,
        Some(current_qid)
    );
    assert_eq!(debug.locked_count, 2);
    assert_eq!(
        update
            .snapshot
            .queue
            .iter()
            .position(|item| item.id == current_qid),
        Some(1)
    );
}

#[test]
fn setting_shuffle_repairs_missing_anchor_for_duplicate_current_track() {
    let conn = conn();
    let repeated = queue::get_track_by_id(&conn, 1).unwrap().unwrap();
    let other_tracks = load_tracks(&conn, &[2, 3]);
    let queue_items = queue::replace_queue(
        &conn,
        &[
            repeated.clone(),
            repeated,
            other_tracks[0].clone(),
            other_tracks[1].clone(),
        ],
        "test",
    )
    .unwrap();
    let repaired_qid = queue_items[0].id;
    let duplicate_qid = queue_items[1].id;
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1,
             current_queue_item_id = NULL,
             is_playing = 1
         WHERE id = 1",
        [],
    )
    .unwrap();

    let update = set_shuffle_mode(&conn, ShuffleMode::True).unwrap();
    let debug = update.debug.expect("shuffle debug");

    assert_eq!(
        update.snapshot.state.current_queue_item_id,
        Some(repaired_qid)
    );
    assert_eq!(debug.locked_count, 1);
    assert_eq!(debug.candidate_count, 3);
    assert_eq!(update.snapshot.queue[0].id, repaired_qid);
    assert!(
        update
            .snapshot
            .queue
            .iter()
            .any(|item| item.id == duplicate_qid)
    );
}

#[test]
fn lookup_listen_source_maps_radio_pending_and_automix_new() {
    let conn = conn();
    for (source, expected) in [
        ("radio_pending", crate::db::models::ListenSource::Radio),
        ("automix-new", crate::db::models::ListenSource::Automix),
    ] {
        conn.execute("DELETE FROM queue", []).unwrap();
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, ?1)",
            params![source],
        )
        .unwrap();
        let qid = conn.last_insert_rowid();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1
             WHERE id = 1",
            params![qid],
        )
        .unwrap();

        assert_eq!(lookup_current_listen_source(&conn), expected);
    }
}

#[test]
fn next_track_starts_first_queue_item_when_no_current_anchor() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();

    conn.execute(
        "UPDATE playback_state
         SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 1
         WHERE id = 1",
        [],
    )
    .unwrap();

    let snapshot = next_track(&conn, false).unwrap();

    assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(2));
    assert_eq!(
        snapshot.state.current_queue_item_id,
        snapshot.queue.first().map(|item| item.id)
    );
    assert!(snapshot.state.is_playing);
}

#[test]
fn next_track_stops_when_current_anchor_is_stale() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();

    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 9999, current_queue_item_id = NULL, is_playing = 1
         WHERE id = 1",
        [],
    )
    .unwrap();

    let snapshot = next_track(&conn, false).unwrap();

    assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), None);
    assert_eq!(snapshot.state.current_queue_item_id, None);
    assert!(!snapshot.state.is_playing);
}

#[test]
fn peek_next_track_returns_none_when_current_anchor_is_stale() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();

    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 9999, current_queue_item_id = NULL, is_playing = 1
         WHERE id = 1",
        [],
    )
    .unwrap();

    let next = peek_next_track(&conn, false).unwrap();

    assert!(next.is_none());
}

#[test]
fn next_track_uses_current_queue_item_id_for_duplicate_tracks() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 1, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    let second_track_one = queue_items
        .iter()
        .find(|item| item.position == 2 && item.track.id == 1)
        .expect("second copy of track 1");
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![second_track_one.id],
    )
    .unwrap();

    let snapshot = next_track(&conn, false).unwrap();

    assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(3));
    assert_eq!(
        snapshot.state.current_queue_item_id,
        queue_items
            .iter()
            .find(|item| item.track.id == 3)
            .map(|item| item.id)
    );
}

#[test]
fn next_track_ignores_mismatched_current_queue_item_id() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 2, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_items[0].id],
    )
    .unwrap();

    let snapshot = next_track(&conn, false).unwrap();

    assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(3));
    assert_eq!(
        snapshot.state.current_queue_item_id,
        queue_items
            .iter()
            .find(|item| item.track.id == 3)
            .map(|item| item.id)
    );
}

#[test]
fn start_queue_from_beginning_ignores_previous_current_anchor() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2]);
    let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_items[0].id],
    )
    .unwrap();

    let snapshot = start_queue_from_beginning(&conn, false).unwrap();

    assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(1));
    assert_eq!(
        snapshot.state.current_queue_item_id,
        Some(queue_items[0].id)
    );
    assert!(snapshot.state.is_playing);
}

#[test]
fn reconcile_advances_current_when_deleted() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    let queue_after_seed = queue::load_queue(&conn).unwrap();
    // Mark t1 as current.
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![queue_after_seed[0].id],
    )
    .unwrap();

    let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
    assert!(outcome.queue_changed);
    assert!(outcome.current_changed);
    assert!(!outcome.stopped_playback);
    assert_eq!(outcome.new_current_track_id, Some(2));

    let state = load_state(&conn).unwrap();
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
    assert!(state.is_playing);

    let remaining = queue::load_queue(&conn).unwrap();
    assert_eq!(remaining.len(), 2);
    assert_eq!(remaining[0].position, 0);
    assert_eq!(remaining[1].position, 1);
}

#[test]
fn reconcile_stops_playback_when_no_survivors() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    let q = queue::load_queue(&conn).unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![q[0].id],
    )
    .unwrap();

    let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
    assert!(outcome.queue_changed);
    assert!(outcome.current_changed);
    assert!(outcome.stopped_playback);
    assert_eq!(outcome.new_current_track_id, None);

    let state = load_state(&conn).unwrap();
    assert!(state.current_track.is_none());
    assert!(!state.is_playing);
}

#[test]
fn reconcile_skips_pending_rows() {
    let conn = conn();
    // Seed: one library row (track 1, current) followed by a pending row.
    conn.execute(
        "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
         VALUES (NULL, 1, 'radio_pending', 'Pending Artist', 'Pending Title', datetime('now'))",
        [],
    )
    .unwrap();
    let q = queue::load_queue(&conn).unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![q[0].id],
    )
    .unwrap();

    let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
    assert!(outcome.current_changed);
    assert!(!outcome.stopped_playback);
    // Pending rows always survive - they don't reference local track IDs.
    assert_eq!(outcome.new_current_track_id, None);

    let remaining = queue::load_queue(&conn).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].is_pending);
}

#[test]
fn reconcile_noop_when_current_not_in_deleted_set() {
    let conn = conn();
    let tracks = load_tracks(&conn, &[1, 2, 3]);
    queue::replace_queue(&conn, &tracks, "test").unwrap();
    let q = queue::load_queue(&conn).unwrap();
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
         WHERE id = 1",
        params![q[0].id],
    )
    .unwrap();

    // Delete track 3 - current (track 1) is unaffected.
    let outcome = reconcile_after_track_delete(&conn, &[3]).unwrap();
    assert!(outcome.queue_changed);
    assert!(!outcome.current_changed);
    assert!(!outcome.stopped_playback);
    assert_eq!(outcome.new_current_track_id, Some(1));

    let state = load_state(&conn).unwrap();
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
}

/// Test-only convenience: run the automix extension builder and discard
/// the per-row reasons, returning just the tracks. Keeps the extension
/// tests focused on selection behaviour without threading through
/// `AutomixSelection` at every call site.
fn extension_tracks(
    conn: &Connection,
    current_track: &Track,
    queue_items: &[QueueItem],
    mode: ShuffleMode,
    needed: usize,
    use_learning: bool,
) -> Result<Vec<Track>> {
    Ok(build_automix_extension_with_reasons(
        conn,
        current_track,
        queue_items,
        mode,
        None,
        needed,
        use_learning,
    )?
    .into_iter()
    .map(|selection| selection.track)
    .collect())
}

/// Build an isolated DB fixture with the full surface
/// `build_automix_extension_with_reasons` needs (the standard `conn()`
/// helper above lacks `embedding_models`, `track_embeddings`, and
/// `track_similarity`). Returns a connection with one seed track
/// inserted but **no** embedding row and **no** similarity rows -
/// the "no recommendation signal" case the guard targets.
fn empty_signal_conn() -> (Connection, Track) {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE server_config (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE tidal_track_labels (tidal_id INTEGER PRIMARY KEY, ai INTEGER);
        CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
        CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, artwork_url TEXT, year INTEGER);
        CREATE TABLE tracks (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            artist_id INTEGER NOT NULL,
            album_id INTEGER,
            disc_number INTEGER,
            track_number INTEGER,
            duration_ms INTEGER,
            isrc TEXT,
            tidal_id INTEGER,
            ytmusic_id TEXT,
            soundcloud_id INTEGER,
            best_quality TEXT,
            best_source TEXT,
            fidelity_score INTEGER DEFAULT 0,
            is_favorite INTEGER DEFAULT 0,
            play_count INTEGER DEFAULT 0,
            last_played_at TEXT,
            date_added TEXT,
            source TEXT DEFAULT 'tidal'
        );
        CREATE TABLE queue (
            id               INTEGER PRIMARY KEY,
            track_id         INTEGER,
            position         INTEGER NOT NULL,
            source           TEXT    DEFAULT 'user',
            reason           TEXT,
            pending_artist   TEXT,
            pending_title    TEXT,
            pending_at       TIMESTAMP,
            resolving_at     TIMESTAMP,
            resolved_at      TIMESTAMP,
            tidal_match_score REAL,
            tidal_id_hint    INTEGER,
            ephemeral_album_title TEXT,
            ephemeral_artwork_url TEXT,
            ephemeral_duration_ms INTEGER,
            ephemeral_artist_tidal_id INTEGER,
            ephemeral_album_tidal_id INTEGER
        );
        CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL, parent_id INTEGER);
        CREATE TABLE track_genres (
            track_id INTEGER NOT NULL,
            genre_id INTEGER NOT NULL,
            source TEXT,
            confidence REAL DEFAULT 1.0
        );
        CREATE TABLE listen_history (
            id INTEGER PRIMARY KEY,
            track_id INTEGER NOT NULL,
            started_at TEXT NOT NULL,
            duration_listened_ms INTEGER DEFAULT 0,
            completed INTEGER DEFAULT 0
        );
        CREATE TABLE embedding_models (
            id INTEGER PRIMARY KEY,
            model_key TEXT NOT NULL UNIQUE,
            family TEXT NOT NULL,
            dimension INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            is_active INTEGER NOT NULL DEFAULT 0,
            trained_at TEXT,
            config_json TEXT,
            metrics_json TEXT,
            created_at TEXT
        );
        CREATE TABLE track_embeddings (
            model_id INTEGER NOT NULL,
            track_id INTEGER NOT NULL,
            vector_blob BLOB NOT NULL,
            PRIMARY KEY (model_id, track_id)
        );
        CREATE TABLE track_similarity (
            track_a INTEGER NOT NULL,
            track_b INTEGER NOT NULL,
            similarity_score REAL NOT NULL DEFAULT 0,
            co_listen_score REAL DEFAULT 0,
            co_album_score REAL DEFAULT 0,
            co_artist_score REAL DEFAULT 0,
            genre_proximity REAL DEFAULT 0,
            duration_proximity REAL DEFAULT 0,
            era_proximity REAL DEFAULT 0,
            computed_at TEXT,
            PRIMARY KEY (track_a, track_b)
        );
        ",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO artists (id, name) VALUES (1, 'Unenriched Artist')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, source, fidelity_score)
         VALUES (1, 'Fresh Tidal Import', 1, 'tidal_stream', 0)",
        [],
    )
    .unwrap();

    let track = queue::get_track_by_id(&conn, 1).unwrap().unwrap();
    (conn, track)
}

/// Phase 2c hotfix: when both the embedding fast-path AND the
/// precomputed similarity table are empty for a seed,
/// `build_automix_extension` must return `Vec::new()` rather than
/// falling back to a 500-track random library pool.
///
/// Reproduces the Amy Shark "I Said Hi" symptom from the
/// diagnostic: fresh `tidal_stream` import, no enrichment, no
/// similarity rows, no embedding. Pre-fix behaviour was a queue
/// full of unrelated library tracks (Mac Miller, Bob Marley,
/// James Brown, etc.); post-fix the queue ends gracefully.
#[test]
fn build_automix_extension_returns_empty_when_seed_has_no_signal() {
    let (conn, seed) = empty_signal_conn();
    let extension = extension_tracks(
        &conn,
        &seed,
        &[], // empty queue
        ShuffleMode::Off,
        12,   // typical needed
        true, // use_learning enabled - fast-path will run, find no model, fall through
    )
    .expect("extension call");
    assert!(
        extension.is_empty(),
        "expected empty extension for seed with no signal, got {} tracks",
        extension.len()
    );
}

/// Same fixture but with one similarity row for the seed → the
/// guard should NOT fire. We deliberately don't assert on the
/// extension's exact contents (that depends on scoring), only on
/// the fact that the guard's early-return path didn't engage.
/// Sparse-but-non-empty signal is the documented "still falls
/// through to random pool below" case.
#[test]
fn build_automix_extension_does_not_skip_when_seed_has_some_signal() {
    let (conn, seed) = empty_signal_conn();
    // Add a second track + one similarity row from seed (id=1) to it.
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, source, fidelity_score)
         VALUES (2, 'Other Track', 1, 'tidal_stream', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO track_similarity (track_a, track_b, similarity_score)
         VALUES (1, 2, 0.5)",
        [],
    )
    .unwrap();

    let extension =
        extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 12, true).expect("extension call");

    // We don't pin contents - only that the empty-signal guard
    // didn't bail. Sparse-signal seeds still walk through the
    // random-pool path which is intended legacy behaviour.
    assert!(
        !extension.is_empty(),
        "expected non-empty extension once a similarity row exists"
    );
}

#[test]
fn learned_automix_refill_stays_anchored_to_the_listeners_pick() {
    let conn = conn();
    conn.execute_batch(
        "
        CREATE TABLE track_neighbors (
            track_id INTEGER NOT NULL,
            neighbor_track_id INTEGER NOT NULL,
            model_id INTEGER NOT NULL,
            rank INTEGER NOT NULL,
            score REAL NOT NULL DEFAULT 0,
            behavioral_score REAL DEFAULT 0,
            audio_score REAL DEFAULT 0,
            metadata_score REAL DEFAULT 0,
            reason_json TEXT,
            computed_at TEXT DEFAULT (datetime('now')),
            primary_reason TEXT,
            confidence REAL NOT NULL DEFAULT 0,
            support_count INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree_percentile REAL NOT NULL DEFAULT 0,
            play_count_seed INTEGER NOT NULL DEFAULT 0,
            play_count_candidate INTEGER NOT NULL DEFAULT 0,
            support_transition REAL NOT NULL DEFAULT 0,
            support_colisten REAL NOT NULL DEFAULT 0,
            support_structure REAL NOT NULL DEFAULT 0,
            support_metadata REAL NOT NULL DEFAULT 0,
            PRIMARY KEY (track_id, neighbor_track_id, model_id)
        );
        INSERT INTO server_config (key, value) VALUES ('discovery_engine', 'v2');
        INSERT INTO embedding_models (
            id, model_key, family, dimension, status, is_active, trained_at, created_at
        ) VALUES (
            1, 'test-anchor', 'discovery-fusion-v2', 3, 'ready', 1,
            '2026-01-01 00:00:00', '2026-01-01 00:00:00'
        );
        -- Track 1 is what the listener picked; track 2 is an automix pick
        -- now playing. Each has one learned neighbor.
        INSERT INTO track_neighbors (
            track_id, neighbor_track_id, model_id, rank, score, behavioral_score, primary_reason
        ) VALUES
            (1, 5, 1, 1, 0.90, 0.90, 'behavioral'),
            (2, 6, 1, 1, 0.90, 0.90, 'behavioral');
        ",
    )
    .expect("schema");
    let picked = queue::get_tracks_by_ids(&conn, &[1]).unwrap();
    queue::append_tracks(&conn, &picked, "user").unwrap();
    let playing = queue::get_tracks_by_ids(&conn, &[2]).unwrap();
    queue::append_tracks(&conn, &playing, "automix").unwrap();
    let queue_items = queue::load_queue(&conn).unwrap();
    let current = playing[0].clone();

    let extension = extension_tracks(&conn, &current, &queue_items, ShuffleMode::Off, 2, true)
        .expect("extension call");

    // The anchor's neighbor leads; the playing pick's neighbor follows.
    assert_eq!(
        extension.iter().map(|track| track.id).collect::<Vec<_>>(),
        vec![5, 6]
    );
}

#[test]
fn learned_automix_builds_chain_aware_order_from_overfetched_neighbors() {
    let conn = conn();
    conn.execute_batch(
        "
        CREATE TABLE track_neighbors (
            track_id INTEGER NOT NULL,
            neighbor_track_id INTEGER NOT NULL,
            model_id INTEGER NOT NULL,
            rank INTEGER NOT NULL,
            score REAL NOT NULL DEFAULT 0,
            behavioral_score REAL DEFAULT 0,
            audio_score REAL DEFAULT 0,
            metadata_score REAL DEFAULT 0,
            reason_json TEXT,
            computed_at TEXT DEFAULT (datetime('now')),
            primary_reason TEXT,
            confidence REAL NOT NULL DEFAULT 0,
            support_count INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree_percentile REAL NOT NULL DEFAULT 0,
            play_count_seed INTEGER NOT NULL DEFAULT 0,
            play_count_candidate INTEGER NOT NULL DEFAULT 0,
            support_transition REAL NOT NULL DEFAULT 0,
            support_colisten REAL NOT NULL DEFAULT 0,
            support_structure REAL NOT NULL DEFAULT 0,
            support_metadata REAL NOT NULL DEFAULT 0,
            PRIMARY KEY (track_id, neighbor_track_id, model_id)
        );
        CREATE TABLE audio_dsp_features (
            track_id INTEGER PRIMARY KEY,
            bpm REAL,
            key_signature TEXT,
            camelot_key TEXT,
            loudness_lufs REAL,
            energy REAL,
            danceability REAL,
            beat_strength REAL,
            spectral_centroid REAL,
            stereo_width REAL,
            is_instrumental INTEGER NOT NULL DEFAULT 0,
            analysis_source TEXT NOT NULL DEFAULT 'test',
            analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
            samples_analyzed INTEGER,
            analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
            analysis_version TEXT NOT NULL DEFAULT 'test'
        );
        INSERT INTO server_config (key, value) VALUES ('discovery_engine', 'v2');
        UPDATE playback_state SET crossfade_ms = 4000 WHERE id = 1;
        INSERT INTO embedding_models (
            id, model_key, family, dimension, status, is_active, trained_at, created_at
        ) VALUES (
            1, 'test-chain', 'discovery-fusion-v2', 3, 'ready', 1,
            '2026-01-01 00:00:00', '2026-01-01 00:00:00'
        );
        INSERT INTO track_neighbors (
            track_id, neighbor_track_id, model_id, rank, score, audio_score, primary_reason
        ) VALUES
            (1, 2, 1, 1, 0.90, 0.90, 'audio_texture'),
            (1, 3, 1, 2, 0.89, 0.89, 'audio_texture'),
            (1, 4, 1, 3, 0.88, 0.88, 'audio_texture');
        INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES
            (1, 120.0, '1A'),
            (2, 120.0, '2A'),
            (3, 120.0, '12B'),
            (4, 120.0, '3A');
        ",
    )
    .expect("schema");
    let seed = queue::get_track_by_id(&conn, 1).unwrap().unwrap();

    let extension =
        extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 3, true).expect("extension call");

    assert_eq!(
        extension.iter().map(|track| track.id).collect::<Vec<_>>(),
        vec![2, 4, 3]
    );
}

#[test]
fn learned_automix_keeps_relevance_and_lets_fit_nudge_only_while_mixing() {
    let conn = conn();
    conn.execute_batch(
        "
        CREATE TABLE track_neighbors (
            track_id INTEGER NOT NULL,
            neighbor_track_id INTEGER NOT NULL,
            model_id INTEGER NOT NULL,
            rank INTEGER NOT NULL,
            score REAL NOT NULL DEFAULT 0,
            behavioral_score REAL DEFAULT 0,
            audio_score REAL DEFAULT 0,
            metadata_score REAL DEFAULT 0,
            reason_json TEXT,
            computed_at TEXT DEFAULT (datetime('now')),
            primary_reason TEXT,
            confidence REAL NOT NULL DEFAULT 0,
            support_count INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree_percentile REAL NOT NULL DEFAULT 0,
            play_count_seed INTEGER NOT NULL DEFAULT 0,
            play_count_candidate INTEGER NOT NULL DEFAULT 0,
            support_transition REAL NOT NULL DEFAULT 0,
            support_colisten REAL NOT NULL DEFAULT 0,
            support_structure REAL NOT NULL DEFAULT 0,
            support_metadata REAL NOT NULL DEFAULT 0,
            PRIMARY KEY (track_id, neighbor_track_id, model_id)
        );
        CREATE TABLE audio_dsp_features (
            track_id INTEGER PRIMARY KEY,
            bpm REAL,
            key_signature TEXT,
            camelot_key TEXT,
            loudness_lufs REAL,
            energy REAL,
            danceability REAL,
            beat_strength REAL,
            spectral_centroid REAL,
            stereo_width REAL,
            is_instrumental INTEGER NOT NULL DEFAULT 0,
            analysis_source TEXT NOT NULL DEFAULT 'test',
            analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
            samples_analyzed INTEGER,
            analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
            analysis_version TEXT NOT NULL DEFAULT 'test'
        );
        INSERT INTO server_config (key, value) VALUES ('discovery_engine', 'v2');
        INSERT INTO embedding_models (
            id, model_key, family, dimension, status, is_active, trained_at, created_at
        ) VALUES (
            1, 'test-smoke', 'discovery-fusion-v2', 3, 'ready', 1,
            '2026-01-01 00:00:00', '2026-01-01 00:00:00'
        );
        INSERT INTO track_neighbors (
            track_id, neighbor_track_id, model_id, rank, score, behavioral_score,
            audio_score, metadata_score, reason_json, primary_reason, support_colisten
        ) VALUES
            (1, 2, 1, 1, 0.99, 0.80, 0.10, 0.10, '[{\"key\":\"behavioral\"}]', 'behavioral', 1.0),
            (1, 3, 1, 2, 0.98, 0.00, 0.98, 0.00, '[{\"key\":\"audio_texture\"}]', 'audio_texture', 0.0),
            (1, 4, 1, 3, 0.97, 0.00, 0.10, 0.00, '[{\"key\":\"lastfm_direct\"}]', 'lastfm_direct', 0.0),
            (1, 5, 1, 4, 0.96, 0.00, 0.10, 0.00, '[{\"key\":\"lastfm_branch\"}]', 'lastfm_branch', 0.0),
            (1, 6, 1, 5, 0.70, 0.00, 0.20, 0.30, '[{\"key\":\"bpm_match\"},{\"key\":\"harmonic_match\"}]', 'bpm_match', 0.0);
        INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES
            (1, 120.0, '1A'),
            (2, 150.0, '6B'),
            (3, 120.0, '1A'),
            (4, 145.0, '6B'),
            (5, 120.0, '1A'),
            (6, 120.0, '1A');
        ",
    )
    .expect("schema");
    let seed = queue::get_track_by_id(&conn, 1).unwrap().unwrap();

    // Not mixing: relevance with lane policy decides, so the co-listened
    // rank-1 neighbor leads even though it clashes on key and tempo.
    let plain =
        extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 5, true).expect("extension call");
    assert_eq!(plain.first().map(|track| track.id), Some(2));

    // Mixing: the clash drops behind close, well-fitting ranks but is not
    // buried, and the far rank-5 fit does not jump to the top.
    conn.execute(
        "UPDATE playback_state SET crossfade_ms = 4000 WHERE id = 1",
        [],
    )
    .unwrap();
    let mixed =
        extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 5, true).expect("extension call");
    let position = |id: i64| mixed.iter().position(|track| track.id == id).unwrap();
    assert_eq!(mixed.first().map(|track| track.id), Some(3));
    assert!(
        (1..=3).contains(&position(2)),
        "order: {:?}",
        mixed.iter().map(|t| t.id).collect::<Vec<_>>()
    );
}

#[test]
fn automix_evaluator_reports_before_after_without_queue_insert() {
    let conn = conn();
    conn.execute_batch(
        "
        CREATE TABLE track_neighbors (
            track_id INTEGER NOT NULL,
            neighbor_track_id INTEGER NOT NULL,
            model_id INTEGER NOT NULL,
            rank INTEGER NOT NULL,
            score REAL NOT NULL DEFAULT 0,
            behavioral_score REAL DEFAULT 0,
            audio_score REAL DEFAULT 0,
            metadata_score REAL DEFAULT 0,
            reason_json TEXT,
            computed_at TEXT DEFAULT (datetime('now')),
            primary_reason TEXT,
            confidence REAL NOT NULL DEFAULT 0,
            support_count INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree INTEGER NOT NULL DEFAULT 0,
            candidate_in_degree_percentile REAL NOT NULL DEFAULT 0,
            play_count_seed INTEGER NOT NULL DEFAULT 0,
            play_count_candidate INTEGER NOT NULL DEFAULT 0,
            support_transition REAL NOT NULL DEFAULT 0,
            support_colisten REAL NOT NULL DEFAULT 0,
            support_structure REAL NOT NULL DEFAULT 0,
            support_metadata REAL NOT NULL DEFAULT 0,
            PRIMARY KEY (track_id, neighbor_track_id, model_id)
        );
        CREATE TABLE audio_dsp_features (
            track_id INTEGER PRIMARY KEY,
            bpm REAL,
            key_signature TEXT,
            camelot_key TEXT,
            loudness_lufs REAL,
            energy REAL,
            danceability REAL,
            beat_strength REAL,
            spectral_centroid REAL,
            stereo_width REAL,
            is_instrumental INTEGER NOT NULL DEFAULT 0,
            analysis_source TEXT NOT NULL DEFAULT 'test',
            analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
            samples_analyzed INTEGER,
            analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
            analysis_version TEXT NOT NULL DEFAULT 'test'
        );
        INSERT INTO server_config (key, value) VALUES ('discovery_engine', 'v2');
        INSERT INTO embedding_models (
            id, model_key, family, dimension, status, is_active, trained_at, created_at
        ) VALUES (
            1, 'test-evaluator', 'discovery-fusion-v2', 3, 'ready', 1,
            '2026-01-01 00:00:00', '2026-01-01 00:00:00'
        );
        INSERT INTO track_neighbors (
            track_id, neighbor_track_id, model_id, rank, score, audio_score,
            metadata_score, reason_json, primary_reason
        ) VALUES
            (1, 2, 1, 1, 0.90, 0.40, 0.20, '[{\"key\":\"bpm_match\"}]', 'bpm_match');
        INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES
            (1, 120.0, '1A'),
            (2, 120.5, '1A');
        ",
    )
    .expect("schema");

    let report = evaluate_automix_for_seed(&conn, 1, 1).expect("evaluate");

    assert_eq!(report.queue_len_before, report.queue_len_after);
    assert_eq!(report.before.len(), 1);
    assert_eq!(report.after.len(), 1);
    assert_eq!(report.before[0].track_id, 2);
    assert_eq!(report.after[0].track_id, 2);
    assert_eq!(report.after[0].bpm, Some(120.5));
    assert_eq!(report.after[0].camelot_key.as_deref(), Some("1A"));
    assert!(report.after[0].final_score.is_some());
}

/// Metadata fallback: seed has no embedding/similarity signal but the
/// artist has other tracks in the library. The cascade should return
/// same-artist tracks rather than ending the queue.
#[test]
fn build_automix_extension_falls_back_to_same_artist_when_no_signal() {
    let (conn, seed) = empty_signal_conn();
    // Add four more tracks by the same artist (id=1).
    for i in 2..=5 {
        conn.execute(
            &format!(
                "INSERT INTO tracks (id, title, artist_id, source, fidelity_score) \
                 VALUES ({i}, 'Track {i}', 1, 'tidal_stream', 0)"
            ),
            [],
        )
        .unwrap();
    }

    let extension =
        extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 12, true).expect("extension call");

    assert!(
        !extension.is_empty(),
        "expected same-artist tracks in fallback, got empty extension"
    );
    assert!(
        extension.iter().all(|t| t.artist_id == seed.artist_id),
        "expected all fallback tracks to share the seed's artist_id"
    );
    // Seed itself must not appear.
    assert!(
        !extension.iter().any(|t| t.id == seed.id),
        "seed track must not appear in its own extension"
    );
}

/// Metadata fallback: seed has no signal AND is the only track by its
/// artist/album, with no genre tags - the truly-orphan path. The
/// extension must stay empty (no random kitchen-sink fill).
#[test]
fn build_automix_extension_returns_empty_when_no_artist_album_or_genre() {
    let (conn, seed) = empty_signal_conn();
    // No additional tracks, no genre rows - seed is completely isolated.
    let extension =
        extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 12, true).expect("extension call");

    assert!(
        extension.is_empty(),
        "expected empty extension for a fully isolated seed, got {} tracks",
        extension.len()
    );
}
