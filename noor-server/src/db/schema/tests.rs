use super::*;
use rusqlite::Connection;

#[test]
fn migration_073_creates_the_station_lineup() {
    let conn = Connection::open_in_memory().unwrap();
    run_migrations(&conn).unwrap();
    conn.execute(
        "INSERT INTO video_station_lineup
             (day, station_id, position, grp, title, subtitle, unwatched_count, preview_json)
         VALUES ('2026-10-07', 'shuffle', 0, 'for_you', 'Pure shuffle', '', 40, '[]')",
        [],
    )
    .unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM video_station_lineup", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn migration_072_builds_the_video_discovery_ledger() {
    let conn = Connection::open_in_memory().unwrap();
    apply_migrations_up_to(&conn, 71).unwrap();
    conn.execute_batch(
        "INSERT INTO artists (id, tidal_id, name) VALUES (1, 10, 'Seed Artist');
         INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
             VALUES (900, 10, 'Seed Artist', '{\"tidal_id\":900,\"title\":\"Song\",\"duration_s\":200,\"artist_id\":10,\"artist_name\":\"Seed Artist\",\"album_tidal_id\":null,\"artwork_url\":null,\"release_year\":null}');
         INSERT INTO video_artist_scans (artist_tidal_id, scanned_at) VALUES
             (10, datetime('now', '-3 days')), (20, datetime('now', '-3 days'));
         INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source) VALUES
             (10, 20, 'Empty Neighbor', 'tidal'), (10, 30, 'Second', 'tidal'), (10, 40, 'Fm', 'lastfm');
         INSERT INTO video_related_scans (seed_tidal_id, scanned_at) VALUES (10, datetime('now', '-1 day'));",
    )
    .unwrap();
    run_migrations(&conn).unwrap();

    type Row = (String, bool, i64, bool, i64, Option<i64>);
    let row = |id: i64| -> Row {
        conn.query_row(
            "SELECT name, seen_main, fetched_count, last_checked_at IS NOT NULL, empty_streak,
                    CAST(ROUND(julianday(next_check_at) - julianday('now')) AS INTEGER)
               FROM video_artist_state WHERE artist_tidal_id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap()
    };
    assert_eq!(row(10), ("Seed Artist".into(), true, 1, true, 0, Some(27)));
    assert_eq!(
        row(20),
        ("Empty Neighbor".into(), false, 0, true, 1, Some(57))
    );
    assert_eq!(row(30).0, "Second");

    let mut stmt = conn
        .prepare("SELECT related_tidal_id, rank, weight FROM video_related_artists ORDER BY related_tidal_id")
        .unwrap();
    let edges: Vec<(i64, i64, f64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        edges.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(),
        vec![(20, 0), (30, 1), (40, 0)]
    );
    assert!((edges[0].2 - 1.0).abs() < 1e-9);
    assert!((edges[1].2 - 0.96).abs() < 1e-9);
    assert!((edges[2].2 - 1.0).abs() < 1e-9);

    let expanded: (bool, i64) = conn
        .query_row(
            "SELECT last_expanded_at IS NOT NULL,
                    CAST(ROUND(julianday(next_expand_at) - julianday('now')) AS INTEGER)
               FROM video_artist_state WHERE artist_tidal_id = 10",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(expanded, (true, 29));
    let has_duration: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('video_history') WHERE name = 'video_duration_ms')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_duration);
}

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

#[test]
fn migration_057_adds_liked_video_tables() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

    conn.execute_batch(
        "INSERT INTO artists (id, name) VALUES (1, 'Anchor');
         INSERT INTO tracks (id, title, artist_id, is_favorite)
         VALUES (10, 'Song', 1, 1);",
    )
    .unwrap();

    // A liked track can carry several videos: live takes, covers and
    // alternates each get their own card.
    conn.execute_batch(
        "INSERT INTO library_videos (track_id, tidal_video_id, video_title, match_score)
         VALUES (10, 900, 'Song', 1.0), (10, 901, 'Song (Live)', 0.93);",
    )
    .unwrap();
    let kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM library_videos WHERE track_id = 10",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 2, "duplicates per track are kept on purpose");

    // The same video cannot be attached to the same track twice.
    let dupe = conn.execute(
        "INSERT INTO library_videos (track_id, tidal_video_id, video_title, match_score)
         VALUES (10, 900, 'Song', 1.0)",
        [],
    );
    assert!(dupe.is_err(), "(track_id, tidal_video_id) is the key");

    // suppressed defaults to visible.
    let suppressed: i64 = conn
        .query_row(
            "SELECT suppressed FROM library_videos WHERE tidal_video_id = 900",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(suppressed, 0);

    // Hits follow the track out.
    conn.execute("DELETE FROM tracks WHERE id = 10", [])
        .unwrap();
    let orphans: i64 = conn
        .query_row("SELECT COUNT(*) FROM library_videos", [], |row| row.get(0))
        .unwrap();
    assert_eq!(orphans, 0, "hits cascade with the track");

    // The scan ledger is keyed by artist, and follows the artist out.
    conn.execute("INSERT INTO artists (id, name) VALUES (2, 'Gone')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO library_video_scans (artist_id, video_count) VALUES (2, 0)",
        [],
    )
    .unwrap();
    conn.execute("DELETE FROM artists WHERE id = 2", [])
        .unwrap();
    let scans: i64 = conn
        .query_row("SELECT COUNT(*) FROM library_video_scans", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(scans, 0, "scan rows cascade with the artist");
}

#[test]
fn migration_062_caps_genre_confidence_idempotently() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    apply_migrations_up_to(&conn, 61).unwrap();
    conn.execute_batch(
        "INSERT INTO artists (id, name) VALUES (62001, 'Migration 062 Artist');
         INSERT INTO tracks (id, title, artist_id)
         VALUES (62001, 'Migration 062 Track', 62001);
         INSERT INTO genres (id, name, slug) VALUES
            (62001, 'Migration 062 Hot', 'migration-062-hot'),
            (62002, 'Migration 062 Stable', 'migration-062-stable');
         INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
            (62001, 62001, 'musicbrainz', 2.27),
            (62001, 62002, 'lastfm', 0.73);",
    )
    .unwrap();

    apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

    let confidences = || {
        let over_limit: f64 = conn
            .query_row(
                "SELECT confidence FROM track_genres WHERE genre_id = 62001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let already_valid: f64 = conn
            .query_row(
                "SELECT confidence FROM track_genres WHERE genre_id = 62002",
                [],
                |row| row.get(0),
            )
            .unwrap();
        (over_limit, already_valid)
    };

    assert_eq!(confidences(), (1.0, 0.73));

    conn.execute_batch(MIGRATION_062).unwrap();
    assert_eq!(confidences(), (1.0, 0.73));
}

#[test]
fn migration_063_is_reentrant_and_preserves_existing_data() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    apply_migrations_up_to(&conn, 62).unwrap();
    conn.execute_batch(
        "INSERT INTO server_config (key, value) VALUES ('server_token', '123456');
         INSERT INTO service_auth (service, user_id) VALUES ('tidal', 'existing-user');",
    )
    .unwrap();

    // Simulate interruption after DDL committed but before migration 063's
    // completion row was recorded.
    conn.execute_batch(MIGRATION_063).unwrap();
    apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();
    apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();

    let migration_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM _migrations WHERE id = 63",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let token: String = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'server_token'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let user_id: String = conn
        .query_row(
            "SELECT user_id FROM service_auth WHERE service = 'tidal'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let hash_is_unique: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_index_list('remote_devices') WHERE name = 'idx_remote_devices_token_hash' AND \"unique\" = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(migration_rows, 1);
    assert_eq!(token, "123456");
    assert_eq!(user_id, "existing-user");
    assert_eq!(hash_is_unique, 1);
}

#[test]
fn migration_071_upgrades_master_and_existing_dj_test_databases() {
    for preview_build in [false, true] {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations_up_to(&conn, if preview_build { 68 } else { 70 }).unwrap();
        conn.execute_batch("INSERT INTO artists(id,name) VALUES(1,'Artist');
            INSERT INTO tracks(id,tidal_id,title,artist_id,is_library,date_added) VALUES(1,10,'Saved',1,1,'2020-01-01');").unwrap();
        if preview_build {
            conn.execute_batch(MIGRATION_071).unwrap();
            conn.execute("INSERT INTO _migrations(id) VALUES(69)", [])
                .unwrap();
            conn.execute_batch("CREATE TABLE tidal_track_aliases(conflicting_column INTEGER);")
                .unwrap();
            assert!(run_migrations(&conn).is_err());
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name='library_added_at'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM _migrations WHERE id=69", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            conn.execute_batch("DROP TABLE tidal_track_aliases;")
                .unwrap();
        }
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM _migrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            MIGRATIONS.len() as i64
        );
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_table_info('dj_transition_events') WHERE name='runtime_planned_start_ms'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(
            conn.query_row(
                "SELECT track_id FROM tidal_track_aliases WHERE tidal_id=10",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT title FROM tracks WHERE id=1", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "Saved"
        );
    }
}

#[test]
fn migration_070_date_normalization_is_atomic_and_restart_safe() {
    let conn = Connection::open_in_memory().unwrap();
    apply_migrations_up_to(&conn, 69).unwrap();
    conn.execute_batch("INSERT INTO artists(id,name) VALUES(1,'Artist');
        INSERT INTO tracks(id,tidal_id,title,artist_id,is_library,date_added,library_added_at) VALUES(1,10,'Saved',1,1,'2020-06-10 07:10:44','2020-06-10 07:10:44');
        CREATE TRIGGER force_format_failure BEFORE UPDATE OF date_added ON tracks BEGIN SELECT RAISE(ABORT,'forced'); END;").unwrap();
    assert!(run_migrations(&conn).is_err());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name='date_choice_at'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM _migrations WHERE id=70", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER force_format_failure;")
        .unwrap();
    run_migrations(&conn).unwrap();
    run_migrations(&conn).unwrap();
    assert_eq!(
        conn.query_row("SELECT date_added FROM tracks", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "2020-06-10T07:10:44Z"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM catalogue_merge_audit WHERE entity='date_format'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn migration_069_rolls_back_failed_backfill_and_can_restart() {
    let conn = Connection::open_in_memory().unwrap();
    apply_migrations_up_to(&conn, 68).unwrap();
    conn.execute_batch("INSERT INTO artists(id,name) VALUES(1,'Artist');
        INSERT INTO tracks(id,tidal_id,title,artist_id,is_library,date_added) VALUES(1,10,'Saved',1,1,'2020-01-01');
        CREATE TABLE tidal_track_aliases(conflicting_column INTEGER);").unwrap();
    assert!(run_migrations(&conn).is_err());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name='library_added_at'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TABLE tidal_track_aliases;")
        .unwrap();
    run_migrations(&conn).unwrap();
    run_migrations(&conn).unwrap();
    assert_eq!(
        conn.query_row("SELECT library_added_at FROM tracks WHERE id=1", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "2020-01-01"
    );
    assert_eq!(
        conn.query_row(
            "SELECT track_id FROM tidal_track_aliases WHERE tidal_id=10",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn migration_065_upgrades_an_existing_video_radio_cache() {
    let conn = Connection::open_in_memory().unwrap();
    apply_migrations_up_to(&conn, 64).unwrap();
    conn.execute(
        "INSERT INTO video_seed_genres (seed_tidal_id, genre_name) VALUES (42, 'Electronic')",
        [],
    )
    .unwrap();

    apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();
    let rank: i64 = conn
        .query_row(
            "SELECT rank FROM video_seed_genres WHERE seed_tidal_id = 42",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO video_genre_scans (genre_name) VALUES ('Electronic')",
        [],
    )
    .unwrap();
    assert_eq!(rank, 0);
}

#[test]
fn migration_066_adds_exact_saved_video_cuts() {
    let conn = Connection::open_in_memory().unwrap();
    apply_migrations_up_to(&conn, 65).unwrap();
    apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();
    conn.execute(
        "INSERT INTO saved_videos (tidal_video_id, item_json) VALUES (91, '{\"tidal_id\":91,\"title\":\"Live cut\"}')",
        [],
    ).unwrap();
    let saved: String = conn
        .query_row(
            "SELECT item_json FROM saved_videos WHERE tidal_video_id = 91",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(saved.contains("Live cut"));
}

#[test]
fn migration_067_preserves_existing_related_artists_and_separates_sources() {
    let conn = Connection::open_in_memory().unwrap();
    apply_migrations_up_to(&conn, 66).unwrap();
    conn.execute(
        "INSERT INTO video_related_artists
        (seed_tidal_id, related_tidal_id, name, source)
        VALUES (10, 20, 'Neighbor', 'tidal')",
        [],
    )
    .unwrap();
    apply_migrations_up_to(&conn, MIGRATIONS.len()).unwrap();
    conn.execute(
        "INSERT INTO video_related_artists
        (seed_tidal_id, related_tidal_id, name, source)
        VALUES (10, 20, 'Neighbor', 'lastfm')",
        [],
    )
    .unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM video_related_artists
        WHERE seed_tidal_id = 10 AND related_tidal_id = 20",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    run_migrations(&conn).unwrap();
}
