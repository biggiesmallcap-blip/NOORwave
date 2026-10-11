use super::*;

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open test db");
    conn.execute_batch(
        "
        CREATE TABLE artists (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL
        );

        CREATE TABLE albums (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            year INTEGER,
            artwork_url TEXT,
            is_favorite INTEGER NOT NULL DEFAULT 0
        );

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
            source TEXT NOT NULL DEFAULT 'tidal',
            sample_rate INTEGER,
            bit_depth INTEGER,
            file_path TEXT,
            is_library INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE audio_dsp_features (
            track_id INTEGER PRIMARY KEY,
            bpm REAL
        );

        CREATE TABLE track_embeddings (
            track_id INTEGER NOT NULL,
            model_id INTEGER NOT NULL,
            vector_blob BLOB,
            PRIMARY KEY (track_id, model_id)
        );

        CREATE TABLE track_neighbors (
            track_id INTEGER NOT NULL,
            neighbor_track_id INTEGER NOT NULL,
            model_id INTEGER NOT NULL,
            rank INTEGER NOT NULL DEFAULT 0,
            score REAL NOT NULL DEFAULT 0
        );

        CREATE TABLE track_similarity (
            track_a INTEGER NOT NULL,
            track_b INTEGER NOT NULL,
            similarity_score REAL NOT NULL DEFAULT 0
        );

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

        CREATE TABLE listen_history (
            id INTEGER PRIMARY KEY,
            track_id INTEGER NOT NULL,
            started_at TEXT NOT NULL,
            duration_listened_ms INTEGER DEFAULT 0,
            completed INTEGER DEFAULT 0
        );

        CREATE TABLE playlist_tracks (
            playlist_id INTEGER NOT NULL,
            track_id INTEGER NOT NULL,
            position INTEGER NOT NULL
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
            tidal_id_hint    INTEGER
        );

        CREATE TABLE shuffle_state (
            track_id INTEGER PRIMARY KEY,
            position INTEGER NOT NULL
        );

        CREATE TABLE track_genres (
            track_id INTEGER NOT NULL,
            genre_id INTEGER NOT NULL,
            source TEXT,
            confidence REAL DEFAULT 1.0
        );

        CREATE TABLE playback_state (
            id INTEGER PRIMARY KEY,
            current_track_id INTEGER,
            current_queue_item_id INTEGER,
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

        CREATE TABLE dj_transition_events (
            id INTEGER PRIMARY KEY,
            from_track_id INTEGER REFERENCES tracks(id),
            to_track_id INTEGER REFERENCES tracks(id)
        );
        ",
    )
    .expect("create schema");

    conn.execute(
        "INSERT INTO artists (id, name) VALUES (1, 'Test Artist')",
        [],
    )
    .expect("insert artist");
    conn.execute(
        "INSERT INTO playback_state (
            id, current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms
        ) VALUES (1, NULL, 0, 0, 1.0, 'off', 'off', 0, 0)",
        [],
    )
    .expect("seed playback_state");
    conn
}

fn insert_album(conn: &Connection, id: i64, title: &str, year: Option<i32>) {
    conn.execute(
        "INSERT INTO albums (id, title, year) VALUES (?1, ?2, ?3)",
        params![id, title, year],
    )
    .expect("insert album");
}

#[allow(clippy::too_many_arguments)]
fn insert_track_full(
    conn: &Connection,
    id: i64,
    title: &str,
    duration_ms: i64,
    isrc: Option<&str>,
    album_id: Option<i64>,
    best_quality: Option<&str>,
    sample_rate: Option<i64>,
    file_path: Option<&str>,
    fidelity_score: i32,
) {
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, isrc,
            best_quality, sample_rate, file_path, fidelity_score, source
         ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'tidal')",
        params![
            id,
            title,
            album_id,
            duration_ms,
            isrc,
            best_quality,
            sample_rate,
            file_path,
            fidelity_score
        ],
    )
    .expect("insert track");
}

fn insert_track(conn: &Connection, id: i64, title: &str, duration_ms: i64, isrc: Option<&str>) {
    insert_track_full(
        conn,
        id,
        title,
        duration_ms,
        isrc,
        None,
        None,
        None,
        None,
        100,
    );
}

fn group_relationships(conn: &Connection) -> Vec<String> {
    let groups = load_groups(conn, 100, 0).expect("load groups");
    groups.into_iter().map(|g| g.relationship).collect()
}

#[test]
fn scan_rejects_shared_isrc_with_large_duration_gap() {
    let conn = test_conn();
    insert_track(&conn, 1, "My Barn My Rules", 127_000, Some("DEU672200178"));
    insert_track(&conn, 2, "My Barn My Rules", 266_000, Some("DEU672200178"));

    let stats = scan(&conn).expect("scan duplicates");

    assert_eq!(stats.groups_found, 0);
    assert_eq!(stats.tracks_affected, 0);
}

#[test]
fn classifies_alt_version_remix() {
    let conn = test_conn();
    // Variant marker mismatch with shared ISRC — must group, classified as alt_version.
    insert_track(
        &conn,
        1,
        "Tarlabasi (Be Svendsen Remix)",
        546_000,
        Some("DEHM81600158"),
    );
    insert_track(&conn, 2, "Tarlabasi", 545_000, Some("DEHM81600158"));

    let stats = scan(&conn).expect("scan duplicates");
    assert_eq!(stats.groups_found, 1);

    let groups = load_groups(&conn, 10, 0).expect("load groups");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].relationship, "alt_version");
    // No is_preferred for non-exact relationships.
    assert!(groups[0].members.iter().all(|m| !m.is_preferred));
}

#[test]
fn scan_keeps_feature_credit_variants_as_duplicates() {
    let conn = test_conn();
    insert_track(
        &conn,
        1,
        "Cachaca (feat. Tom Scott)",
        291_000,
        Some("AUI441600195"),
    );
    insert_track(&conn, 2, "Cachaca", 290_000, Some("AUI441600195"));

    let stats = scan(&conn).expect("scan duplicates");
    let groups = load_groups(&conn, 10, 0).expect("load duplicate groups");

    assert_eq!(stats.groups_found, 1);
    assert_eq!(stats.tracks_affected, 2);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members.len(), 2);
    assert_eq!(groups[0].relationship, "exact_duplicate");
    assert!(groups[0].members.iter().any(|m| m.is_preferred));
}

#[test]
fn scan_counts_only_pending_tracks_in_stats() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO duplicate_groups (id, status) VALUES (99, 'resolved')",
        [],
    )
    .expect("insert resolved group");
    insert_track(&conn, 1, "Only Old Group", 180_000, Some("OLDISRC"));
    conn.execute(
        "INSERT INTO duplicate_members (group_id, track_id, is_preferred) VALUES (99, 1, 1)",
        [],
    )
    .expect("insert resolved membership");

    let stats = scan(&conn).expect("scan duplicates");

    assert_eq!(stats.groups_found, 0);
    assert_eq!(stats.tracks_affected, 0);
}

#[test]
fn classifies_remaster_with_year_drift() {
    let conn = test_conn();
    insert_album(&conn, 1, "Original Album", Some(1991));
    insert_album(&conn, 2, "Remaster Reissue", Some(2011));
    insert_track_full(
        &conn,
        1,
        "Memory Lane",
        240_000,
        None,
        Some(1),
        Some("HI_RES"),
        None,
        None,
        150,
    );
    insert_track_full(
        &conn,
        2,
        "Memory Lane",
        240_500,
        None,
        Some(2),
        Some("HI_RES"),
        None,
        None,
        150,
    );

    scan(&conn).expect("scan");
    let groups = load_groups(&conn, 10, 0).expect("load");

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].relationship, "remaster");
    assert!(groups[0].differences.iter().any(|d| d.kind == "year"));
    assert!(groups[0].members.iter().all(|m| !m.is_preferred));
}

#[test]
fn classifies_mono_vs_stereo_as_remaster() {
    let conn = test_conn();
    insert_album(&conn, 1, "Sgt. Pepper", None);
    insert_track_full(
        &conn,
        1,
        "Sgt. Pepper (Mono)",
        150_000,
        None,
        Some(1),
        Some("LOSSLESS"),
        None,
        None,
        120,
    );
    insert_track_full(
        &conn,
        2,
        "Sgt. Pepper (Stereo)",
        150_500,
        None,
        Some(1),
        Some("LOSSLESS"),
        None,
        None,
        120,
    );

    scan(&conn).expect("scan");
    let groups = load_groups(&conn, 10, 0).expect("load");

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].relationship, "remaster");
    assert!(
        groups[0]
            .differences
            .iter()
            .any(|d| d.kind == "version_marker")
    );
    assert!(groups[0].members.iter().all(|m| !m.is_preferred));
}

#[test]
fn classifies_cross_album_reissue() {
    let conn = test_conn();
    insert_album(&conn, 1, "Studio Album", Some(2005));
    insert_album(&conn, 2, "Greatest Hits", Some(2007));
    insert_track_full(
        &conn,
        1,
        "Anthem",
        200_000,
        Some("ISRC123"),
        Some(1),
        Some("LOSSLESS"),
        None,
        None,
        100,
    );
    insert_track_full(
        &conn,
        2,
        "Anthem",
        200_300,
        Some("ISRC123"),
        Some(2),
        Some("LOSSLESS"),
        None,
        None,
        100,
    );

    scan(&conn).expect("scan");
    let groups = load_groups(&conn, 10, 0).expect("load");

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].relationship, "cross_album_reissue");
    assert!(groups[0].differences.iter().any(|d| d.kind == "album"));
    // Cross-album reissues are not auto-preferred — same-recording but
    // different release; the user picks.
    assert!(groups[0].members.iter().all(|m| !m.is_preferred));
}

#[test]
fn classifies_quality_variant() {
    let conn = test_conn();
    insert_album(&conn, 1, "Studio Album", Some(2020));
    insert_track_full(
        &conn,
        1,
        "Brightside",
        210_000,
        Some("QV12345"),
        Some(1),
        Some("HI_RES_LOSSLESS"),
        None,
        None,
        200,
    );
    insert_track_full(
        &conn,
        2,
        "Brightside",
        210_200,
        Some("QV12345"),
        Some(1),
        Some("LOW"),
        None,
        None,
        50,
    );

    scan(&conn).expect("scan");
    let groups = load_groups(&conn, 10, 0).expect("load");

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].relationship, "quality_variant");
    assert!(groups[0].differences.iter().any(|d| d.kind == "quality"));
    assert!(groups[0].members.iter().all(|m| !m.is_preferred));
}

#[test]
fn classifies_exact_duplicate_and_marks_preferred() {
    let conn = test_conn();
    insert_album(&conn, 1, "Studio Album", Some(2020));
    insert_track_full(
        &conn,
        1,
        "Same Recording",
        180_000,
        Some("EXD0001"),
        Some(1),
        Some("LOSSLESS"),
        None,
        None,
        200,
    );
    insert_track_full(
        &conn,
        2,
        "Same Recording",
        180_500,
        Some("EXD0001"),
        Some(1),
        Some("LOSSLESS"),
        None,
        None,
        150,
    );

    scan(&conn).expect("scan");
    let groups = load_groups(&conn, 10, 0).expect("load");

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].relationship, "exact_duplicate");
    // The higher-fidelity row should be marked preferred.
    let preferred: Vec<i64> = groups[0]
        .members
        .iter()
        .filter(|m| m.is_preferred)
        .map(|m| m.track.id)
        .collect();
    assert_eq!(preferred, vec![1]);
}

#[test]
fn bucket_groups_alt_versions_together() {
    let conn = test_conn();
    // No shared ISRC and different canonical titles — these would NOT have
    // bucketed under the old logic. base_title strips "(Remix)" so they do.
    insert_track(&conn, 1, "Wavelength", 200_000, None);
    insert_track(&conn, 2, "Wavelength (Remix)", 200_500, None);

    scan(&conn).expect("scan");
    let rels = group_relationships(&conn);

    assert_eq!(rels, vec!["alt_version".to_string()]);
}

#[test]
fn tightened_duration_rejects_loose_match() {
    let conn = test_conn();
    // Same canonical title, no ISRC, but durations differ by 2.5s — over
    // the tightened ±2000ms tolerance for non-ISRC matches.
    insert_track(&conn, 1, "Drift", 180_000, None);
    insert_track(&conn, 2, "Drift", 182_500, None);

    let stats = scan(&conn).expect("scan");
    assert_eq!(stats.groups_found, 0);
}

#[test]
fn local_sample_rate_difference_classifies_quality_variant() {
    let conn = test_conn();
    insert_album(&conn, 1, "Master Tape", Some(2018));
    insert_track_full(
        &conn,
        1,
        "Origin",
        240_000,
        Some("SR0001"),
        Some(1),
        Some("LOSSLESS"),
        Some(44_100),
        Some("/music/origin.flac"),
        150,
    );
    insert_track_full(
        &conn,
        2,
        "Origin",
        240_400,
        Some("SR0001"),
        Some(1),
        Some("LOSSLESS"),
        Some(96_000),
        Some("/music/origin-hires.flac"),
        150,
    );

    scan(&conn).expect("scan");
    let groups = load_groups(&conn, 10, 0).expect("load");

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].relationship, "quality_variant");
    assert!(
        groups[0]
            .differences
            .iter()
            .any(|d| d.kind == "sample_rate")
    );
}

// ── decide_import ────────────────────────────────────────────────────────

fn incoming<'a>(
    tidal_id: i64,
    title: &'a str,
    isrc: Option<&'a str>,
    duration_ms: i64,
) -> IncomingTrack<'a> {
    IncomingTrack {
        tidal_id,
        title,
        artist_name: "Test Artist",
        isrc,
        duration_ms,
        version: None,
        explicit: None,
    }
}

fn candidate(
    track_id: i64,
    tidal_id: Option<i64>,
    title: &str,
    isrc: Option<&str>,
    duration_ms: i64,
) -> ExistingCandidate {
    ExistingCandidate {
        track_id,
        tidal_id,
        title: title.to_string(),
        artist_name: "Test Artist".to_string(),
        isrc: isrc.map(str::to_string),
        duration_ms,
        version: None,
        explicit: None,
    }
}

#[test]
fn decide_import_same_tidal_id_is_a_resync_not_a_duplicate() {
    // Rule 0: the upsert must run so re-syncs keep refreshing metadata.
    let inc = incoming(42, "Song", Some("ISRC1"), 200_000);
    let cands = vec![candidate(1, Some(42), "Song", Some("ISRC1"), 200_000)];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
}

#[test]
fn decide_import_exact_isrc_dup_skips() {
    let inc = incoming(42, "Song", Some("isrc1 "), 200_000);
    let cands = vec![candidate(7, Some(99), "Song", Some("ISRC1"), 201_000)];
    assert_eq!(
        decide_import(&inc, &cands),
        ImportDecision::LinkAlias {
            existing_track_id: 7,
            existing_tidal_id: Some(99),
        }
    );
}

#[test]
fn decide_import_isrc_with_large_duration_gap_inserts() {
    // Known upstream bug: one ISRC reused across different-length cuts.
    let inc = incoming(42, "My Barn My Rules", Some("DEU672200178"), 127_000);
    let cands = vec![candidate(
        7,
        Some(99),
        "My Barn My Rules",
        Some("DEU672200178"),
        266_000,
    )];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
}

#[test]
fn decide_import_title_only_match_remains_for_review() {
    // Same recording released on a single and an album: no ISRC on the
    // incoming copy, title+artist+duration collapse it.
    let inc = incoming(42, "Song", None, 200_000);
    let cands = vec![candidate(7, Some(99), "Song", None, 200_500)];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
}

#[test]
fn decide_import_keeps_live_variant() {
    // Variants are never duplicates: differing alt fingerprints.
    let inc = incoming(42, "Song (Live)", None, 200_000);
    let cands = vec![candidate(7, Some(99), "Song", None, 200_000)];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);

    // And the mirror: incoming base, only the live cut exists.
    let inc = incoming(43, "Song", None, 200_000);
    let cands = vec![candidate(8, Some(98), "Song (Live)", None, 200_000)];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
}

#[test]
fn decide_import_keeps_remix_variant() {
    let inc = incoming(42, "Song (Remix)", None, 200_000);
    let cands = vec![candidate(7, Some(99), "Song", None, 200_000)];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
}

#[test]
fn decide_import_unverified_remaster_remains_for_review() {
    // Master markers are not part of the variant fingerprint: a remaster
    // with matching duration is the same recording.
    let inc = incoming(42, "Song (2011 Remaster)", None, 200_000);
    let cands = vec![candidate(7, Some(99), "Song", None, 200_500)];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
}

#[test]
fn decide_import_extended_cut_survives_via_duration() {
    // "Extended" is a master token, so the duration gate is what keeps a
    // genuinely longer cut alive.
    let inc = incoming(42, "Song (Extended Mix)", None, 260_000);
    let cands = vec![candidate(7, Some(99), "Song", None, 200_000)];
    assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
}

#[test]
fn decide_import_no_candidates_inserts() {
    let inc = incoming(42, "Song", Some("ISRC1"), 200_000);
    assert_eq!(decide_import(&inc, &[]), ImportDecision::Insert);
}

// ── merge_group / auto_merge_pending ────────────────────────────────────

fn seed_pending_group(conn: &Connection, track_ids: &[i64]) -> i64 {
    conn.execute(
        "INSERT INTO duplicate_groups (status) VALUES ('pending')",
        [],
    )
    .expect("insert group");
    let gid = conn.last_insert_rowid();
    for &tid in track_ids {
        conn.execute(
            "INSERT INTO duplicate_members (group_id, track_id, is_preferred) VALUES (?1, ?2, 0)",
            params![gid, tid],
        )
        .expect("insert member");
    }
    gid
}

#[test]
fn merge_group_repoints_history_and_transfers_like() {
    let conn = test_conn();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    insert_track(&conn, 1, "Song", 200_000, Some("ISRC1"));
    insert_track(&conn, 2, "Song", 200_500, Some("ISRC1"));
    // Loser 2 carries the like, plays, history, playlist membership, DSP.
    conn.execute(
        "UPDATE tracks SET is_favorite = 1, play_count = 5,
             date_added = '2024-07-22T03:55:51.120+0000' WHERE id = 2",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE tracks SET date_added = '2026-09-18 07:37:31' WHERE id = 1",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO listen_history (track_id, started_at) VALUES (2, '2026-01-01')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (1, 2, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (2, 128.0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO dj_transition_events (id, from_track_id, to_track_id)
         VALUES (1, 2, 1), (2, 1, 2)",
        [],
    )
    .unwrap();
    conn.execute("UPDATE tracks SET tidal_id = 900 WHERE id = 1", [])
        .unwrap();
    conn.execute("UPDATE tracks SET tidal_id = 901 WHERE id = 2", [])
        .unwrap();

    let gid = seed_pending_group(&conn, &[1, 2]);
    let outcome = merge_group(&conn, gid, 1).expect("merge");

    assert_eq!(outcome.removed_track_ids, vec![2]);
    assert_eq!(outcome.favorited_loser_tidal_ids, vec![901]);
    assert_eq!(outcome.kept_tidal_id, Some(900));

    // Like, plays, history, playlist, DSP all moved to the kept row.
    let (fav, plays, date_added): (i32, i64, String) = conn
        .query_row(
            "SELECT is_favorite, play_count, date_added FROM tracks WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(fav, 1);
    assert_eq!(plays, 5);
    assert_eq!(date_added, "2024-07-22T03:55:51.120+0000");
    let history_target: i64 = conn
        .query_row("SELECT track_id FROM listen_history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(history_target, 1);
    let playlist_target: i64 = conn
        .query_row("SELECT track_id FROM playlist_tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(playlist_target, 1);
    let dsp_target: i64 = conn
        .query_row("SELECT track_id FROM audio_dsp_features", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dsp_target, 1);
    let dj_targets: Vec<(i64, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT from_track_id, to_track_id
                 FROM dj_transition_events ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(dj_targets, vec![(1, 1), (1, 1)]);

    // Loser row gone, group resolved.
    let loser_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks WHERE id = 2", [], |r| r.get(0))
        .unwrap();
    assert_eq!(loser_count, 0);
    let status: String = conn
        .query_row(
            "SELECT status FROM duplicate_groups WHERE id = ?1",
            [gid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "resolved");
}

#[test]
fn merge_group_dedupes_playlist_holding_both_copies() {
    let conn = test_conn();
    insert_track(&conn, 1, "Song", 200_000, Some("ISRC1"));
    insert_track(&conn, 2, "Song", 200_500, Some("ISRC1"));
    conn.execute(
        "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (1, 1, 0), (1, 2, 1)",
        [],
    )
    .unwrap();

    let gid = seed_pending_group(&conn, &[1, 2]);
    merge_group(&conn, gid, 1).expect("merge");

    // One membership survives, pointing at the kept row.
    let rows: Vec<(i64, i64)> = {
        let mut stmt = conn
            .prepare("SELECT track_id, position FROM playlist_tracks ORDER BY position")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(rows, vec![(1, 0)]);
}

#[test]
fn merge_group_rolls_back_every_change_when_loser_delete_fails() {
    let conn = test_conn();
    insert_track(&conn, 1, "Song", 200_000, Some("ISRC1"));
    insert_track(&conn, 2, "Song", 200_500, Some("ISRC1"));
    conn.execute(
        "UPDATE tracks SET is_favorite = 1, play_count = 5 WHERE id = 2",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO listen_history (track_id, started_at) VALUES (2, '2026-01-01')",
        [],
    )
    .unwrap();
    let gid = seed_pending_group(&conn, &[1, 2]);
    conn.execute_batch(
        "CREATE TRIGGER force_merge_failure BEFORE DELETE ON tracks
         WHEN OLD.id = 2 BEGIN SELECT RAISE(ABORT, 'forced merge failure'); END;",
    )
    .unwrap();

    let error = match merge_group(&conn, gid, 1) {
        Ok(_) => panic!("trigger must abort merge"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("forced merge failure"));

    let kept: (i32, i64) = conn
        .query_row(
            "SELECT is_favorite, play_count FROM tracks WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kept, (0, 0));
    let loser: (i32, i64) = conn
        .query_row(
            "SELECT is_favorite, play_count FROM tracks WHERE id = 2",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(loser, (1, 5));
    let history_target: i64 = conn
        .query_row("SELECT track_id FROM listen_history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(history_target, 2);
    let status: String = conn
        .query_row(
            "SELECT status FROM duplicate_groups WHERE id = ?1",
            [gid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "pending");
}

#[test]
fn auto_merge_keeps_liked_row_and_skips_alt_versions() {
    let conn = test_conn();
    // Group A: exact ISRC dup; the LIKED row must be the survivor even
    // with lower fidelity.
    insert_track_full(
        &conn,
        1,
        "Song",
        200_000,
        Some("ISRC1"),
        None,
        None,
        None,
        None,
        100,
    );
    insert_track_full(
        &conn,
        2,
        "Song",
        200_400,
        Some("ISRC1"),
        None,
        None,
        None,
        None,
        900,
    );
    conn.execute("UPDATE tracks SET is_favorite = 1 WHERE id = 1", [])
        .unwrap();
    // Group B: alt_version (remix) - must survive untouched.
    insert_track(&conn, 3, "Wavelength", 210_000, None);
    insert_track(&conn, 4, "Wavelength (Remix)", 210_200, None);

    scan(&conn).expect("scan");
    let stats = auto_merge_pending(&conn).expect("auto merge");

    assert_eq!(stats.merged_groups, 1);
    assert_eq!(stats.removed_tracks, 1);
    assert_eq!(stats.skipped_groups, 1);

    // Liked low-fidelity row survived; high-fidelity loser removed.
    let survivor: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE id = 1 AND is_favorite = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(survivor, 1);
    let loser: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks WHERE id = 2", [], |r| r.get(0))
        .unwrap();
    assert_eq!(loser, 0);
    // Both remix-group rows still present.
    let variants: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks WHERE id IN (3, 4)", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(variants, 2);

    // Idempotent: nothing left to merge on a second pass.
    scan(&conn).expect("rescan");
    let stats2 = auto_merge_pending(&conn).expect("second pass");
    assert_eq!(stats2.merged_groups, 0);
    assert_eq!(stats2.removed_tracks, 0);
}

#[test]
fn auto_merge_never_touches_local_files() {
    let conn = test_conn();
    insert_track_full(
        &conn,
        1,
        "Origin",
        240_000,
        Some("SR0001"),
        None,
        Some("LOSSLESS"),
        None,
        Some("/music/origin.flac"),
        150,
    );
    insert_track_full(
        &conn,
        2,
        "Origin",
        240_200,
        Some("SR0001"),
        None,
        Some("HI_RES_LOSSLESS"),
        None,
        None,
        900,
    );

    scan(&conn).expect("scan");
    let stats = auto_merge_pending(&conn).expect("auto merge");

    assert_eq!(stats.merged_groups, 0);
    assert_eq!(stats.skipped_groups, 1);
    let both: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(both, 2);
}

#[test]
fn auto_merge_all_liked_group_keeps_recording_liked() {
    let conn = test_conn();
    insert_track_full(
        &conn,
        1,
        "Song",
        200_000,
        Some("ISRC1"),
        None,
        None,
        None,
        None,
        700,
    );
    insert_track_full(
        &conn,
        2,
        "Song",
        200_400,
        Some("ISRC1"),
        None,
        None,
        None,
        None,
        900,
    );
    conn.execute("UPDATE tracks SET is_favorite = 1 WHERE id IN (1, 2)", [])
        .unwrap();

    scan(&conn).expect("scan");
    let stats = auto_merge_pending(&conn).expect("auto merge");

    assert_eq!(stats.merged_groups, 1);
    // Higher fidelity liked row won; the recording is still liked.
    let (kept, fav): (i64, i32) = conn
        .query_row("SELECT id, is_favorite FROM tracks", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(kept, 2);
    assert_eq!(fav, 1);
}
