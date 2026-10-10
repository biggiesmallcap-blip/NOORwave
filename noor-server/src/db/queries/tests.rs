use super::*;
use crate::db::schema;
use rusqlite::Connection;

#[test]
fn bare_artist_photo_ids_become_urls() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO artists (id, name, photo_url) VALUES
            (1, 'Bare', '3a503460-3914-4d4a-b4de-aa93f4020d08'),
            (2, 'Url', 'https://resources.tidal.com/images/a/b/c/d/e/640x640.jpg'),
            (3, 'None', NULL)",
        [],
    )
    .expect("artists");
    assert_eq!(repair_bare_artist_photos(&conn).expect("repair"), 1);
    let url: String = conn
        .query_row("SELECT photo_url FROM artists WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("photo");
    assert_eq!(
        url,
        "https://resources.tidal.com/images/3a503460/3914/4d4a/b4de/aa93f4020d08/750x750.jpg"
    );
    assert_eq!(repair_bare_artist_photos(&conn).expect("again"), 0);
}

#[test]
fn label_from_copyright_keeps_the_name() {
    assert_eq!(
        label_from_copyright(Some("(P) 2003 Parlophone Records Ltd")),
        "Parlophone Records Ltd"
    );
    assert_eq!(
        label_from_copyright(Some("\u{2117} 2026 Mediaphon")),
        "Mediaphon"
    );
    assert_eq!(label_from_copyright(Some("Warp Records")), "Warp Records");
    assert_eq!(label_from_copyright(None), "");
}

#[test]
fn album_label_round_trips() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, tidal_id, year) VALUES (7, 'Album', 1, 99, 2001)",
        [],
    )
    .expect("album");
    assert_eq!(
        get_album_credits(&conn, 7).expect("credits"),
        Some((Some(99), None, Some(2001)))
    );
    set_album_label(&conn, 7, "Warp").expect("label");
    assert_eq!(
        get_album_credits(&conn, 7).expect("credits"),
        Some((Some(99), Some("Warp".to_string()), Some(2001)))
    );
    assert_eq!(get_album_credits(&conn, 8).expect("missing"), None);
}

#[test]
fn artists_sort_letters_first_ignoring_the_and_index_by_initial() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    for (id, name) in [
        (1, "*NSYNC"),
        (2, "070 Shake"),
        (3, "The Beatles"),
        (4, "abba"),
        (5, "Radiohead"),
    ] {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (?1, ?2)",
            params![id, name],
        )
        .expect("artist");
    }
    let names: Vec<String> = get_artists(&conn, "name", "asc", 10, 0)
        .expect("artists")
        .into_iter()
        .map(|artist| artist.name)
        .collect();
    assert_eq!(
        names,
        vec!["abba", "The Beatles", "Radiohead", "*NSYNC", "070 Shake"]
    );

    let index = get_artist_letter_index(&conn).expect("index");
    assert_eq!(index.total, 5);
    let letters: Vec<(String, i64)> = index
        .letters
        .into_iter()
        .map(|entry| (entry.letter, entry.offset))
        .collect();
    assert_eq!(
        letters,
        vec![
            ("A".into(), 0),
            ("B".into(), 1),
            ("R".into(), 2),
            ("#".into(), 3)
        ]
    );
}

fn read_onboarding_value(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM server_config WHERE key='onboarding_complete'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .expect("query server_config")
}

/// A library with `count` favourite tracks on one favourite album, plus one
/// non-favourite track and one non-favourite album that the shuffle sample
/// must never reach for.
fn shuffle_fixture(count: i64) -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, is_favorite) VALUES (1, 'Album', 1, 1)",
        [],
    )
    .expect("album");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, is_favorite) VALUES (2, 'Stranger', 1, 0)",
        [],
    )
    .expect("other album");
    for id in 1..=count {
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, is_favorite, is_library)
             VALUES (?1, 'Track', 1, 1, 1, 1)",
            params![id],
        )
        .expect("track");
    }
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, is_favorite, is_library)
         VALUES (9001, 'Outsider', 1, 2, 0, 0)",
        [],
    )
    .expect("non-favourite track");
    conn
}

#[test]
fn shuffled_sample_is_stable_for_one_salt_and_moves_with_the_next() {
    // The home murals repaint on every remount; a sample that reshuffled per
    // request would churn the panel under the user. Same salt must mean the
    // same picks, and the next time bucket must actually move them.
    let conn = shuffle_fixture(60);

    let first = get_shuffled_tracks(&conn, 100, 12, true).expect("sample");
    let again = get_shuffled_tracks(&conn, 100, 12, true).expect("sample");
    assert_eq!(first.len(), 12, "the sample fills the requested limit");
    assert_eq!(
        first.iter().map(|t| t.id).collect::<Vec<_>>(),
        again.iter().map(|t| t.id).collect::<Vec<_>>(),
        "the same salt must return the same picks"
    );

    let next = get_shuffled_tracks(&conn, 101, 12, true).expect("sample");
    assert_ne!(
        first.iter().map(|t| t.id).collect::<Vec<_>>(),
        next.iter().map(|t| t.id).collect::<Vec<_>>(),
        "the next bucket must reshuffle"
    );
}

#[test]
fn shuffled_sample_respects_the_favourite_filter() {
    let conn = shuffle_fixture(4);

    let favourites = get_shuffled_tracks(&conn, 7, 50, true).expect("sample");
    assert_eq!(favourites.len(), 4);
    assert!(
        favourites.iter().all(|t| t.id != 9001),
        "a non-favourite track must not reach the library murals"
    );
    assert_eq!(
        get_shuffled_tracks(&conn, 7, 50, false)
            .expect("sample")
            .len(),
        5,
        "without the filter every track is eligible"
    );

    let albums = get_shuffled_albums(&conn, 7, 50, true).expect("albums");
    assert_eq!(albums.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1]);
    assert_eq!(
        get_shuffled_albums(&conn, 7, 50, false)
            .expect("albums")
            .len(),
        2
    );
}

#[test]
fn compute_track_similarity_swaps_from_temp_build_table() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, year) VALUES (1, 'Album', 1, 1999)",
        [],
    )
    .expect("album");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms)
         VALUES
            (1, 'One', 1, 1, 180000),
            (2, 'Two', 1, 1, 181000),
            (3, 'Three', 1, 1, 220000)",
        [],
    )
    .expect("tracks");
    conn.execute(
        "INSERT INTO track_similarity (track_a, track_b, similarity_score)
         VALUES (1, 2, 0.01)",
        [],
    )
    .expect("stale similarity");

    let count = compute_track_similarity(&conn).expect("compute similarity");
    assert_eq!(count, 3);

    let persisted_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM track_similarity", [], |row| {
            row.get(0)
        })
        .expect("persisted count");
    assert_eq!(persisted_count, 3);
    let score: f64 = conn
        .query_row(
            "SELECT similarity_score FROM track_similarity WHERE track_a = 1 AND track_b = 2",
            [],
            |row| row.get(0),
        )
        .expect("score");
    assert!(score > 0.01, "rebuild should replace stale similarity rows");
    assert!(
        conn.query_row("SELECT COUNT(*) FROM _track_similarity_build", [], |row| {
            row.get::<_, i64>(0)
        },)
            .is_err(),
        "temporary build table should be dropped after swap"
    );
}

#[test]
fn compute_track_similarity_weights_rare_genres_above_broad_ones() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    schema::run_migrations(&conn).expect("migrations");
    // Distinct artists, no albums or listens, so genre_proximity is the only
    // non-zero signal and the test isolates the IDF weighting.
    for id in 1..=12 {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (?1, ?2)",
            params![id, format!("A{id}")],
        )
        .expect("artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, ?2, ?1, 180000)",
            params![id, format!("T{id}")],
        )
        .expect("track");
    }
    conn.execute(
        "INSERT INTO genres (id, name, slug) VALUES (1,'Broad','broad'),(2,'Rare','rare')",
        [],
    )
    .expect("genres");
    // Broad genre covers 10 of 12 tracks; rare genre covers 2.
    for id in 1..=10 {
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id) VALUES (?1, 1)",
            params![id],
        )
        .expect("broad genre");
    }
    conn.execute(
        "INSERT INTO track_genres (track_id, genre_id) VALUES (11,2),(12,2)",
        [],
    )
    .expect("rare genre");

    compute_track_similarity(&conn).expect("compute similarity");

    let broad: f64 = conn
        .query_row(
            "SELECT genre_proximity FROM track_similarity WHERE track_a=1 AND track_b=2",
            [],
            |row| row.get(0),
        )
        .expect("broad pair");
    let rare: f64 = conn
        .query_row(
            "SELECT genre_proximity FROM track_similarity WHERE track_a=11 AND track_b=12",
            [],
            |row| row.get(0),
        )
        .expect("rare pair");

    assert!(
        rare > broad,
        "rare-genre pair ({rare}) should outscore broad-genre pair ({broad})"
    );
    assert!(
        broad > 0.0,
        "a broad-genre pair still carries some proximity"
    );
    assert!(
        (rare - 1.0).abs() < 1e-9,
        "the rarest shared-genre pair normalizes to 1.0"
    );
}

#[test]
fn compute_track_similarity_weights_genre_bridges_by_confidence() {
    // A weakly-attested genre tag (a single-vote MusicBrainz "jazz" bleeding
    // onto a track, stored at low confidence after the scorer fix) must bridge
    // to genuine holders of that genre far more weakly than two confident
    // holders bridge to each other - the consumer side of the data-layer fix.
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    schema::run_migrations(&conn).expect("migrations");
    // Distinct artists, no albums or listens, so genre_proximity is isolated.
    for id in 1..=30 {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (?1, ?2)",
            params![id, format!("A{id}")],
        )
        .expect("artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, ?2, ?1, 180000)",
            params![id, format!("T{id}")],
        )
        .expect("track");
    }
    conn.execute(
        "INSERT INTO genres (id, name, slug) VALUES (1,'Niche','niche')",
        [],
    )
    .expect("genre");
    // Tracks 16-18 genuinely hold the niche genre at full confidence; track 3
    // carries it only as a low-confidence bleed.
    conn.execute(
        "INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
            (16,1,'lastfm',1.0),(17,1,'lastfm',1.0),(18,1,'lastfm',1.0),
            (3,1,'musicbrainz',0.2)",
        [],
    )
    .expect("niche genre tags");

    compute_track_similarity(&conn).expect("compute similarity");

    let gp = |a: i64, b: i64| -> f64 {
        conn.query_row(
            "SELECT genre_proximity FROM track_similarity WHERE track_a=?1 AND track_b=?2",
            params![a, b],
            |row| row.get(0),
        )
        .unwrap_or(0.0)
    };
    let genuine = gp(16, 17); // two confident holders
    let bleed = gp(3, 16); // low-confidence tag bridging to a confident holder

    assert!(
        bleed < genuine,
        "a low-confidence tag must bridge weaker ({bleed}) than two confident holders ({genuine})"
    );
    assert!(
        (genuine - 1.0).abs() < 1e-9,
        "the strongest genuine pair normalizes to 1.0"
    );
    // MIN() picks the weaker believer, so the bleed bridge is exactly the tag's
    // confidence fraction (0.2) of the genuine bridge.
    assert!(
        (bleed / genuine - 0.2).abs() < 1e-9,
        "the low-confidence bridge should scale with the tag's confidence, got {bleed}"
    );
}

#[test]
fn compute_track_similarity_scores_co_listens_across_artists() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    schema::run_migrations(&conn).expect("migrations");
    for id in 1..=4 {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (?1, ?2)",
            params![id, format!("A{id}")],
        )
        .expect("artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, ?2, ?1, 180000)",
            params![id, format!("T{id}")],
        )
        .expect("track");
    }
    // Stored the way the player writes them: RFC 3339 with a T separator.
    let listen = |track: i64, hours_ago: i64, minutes: i64| {
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
             VALUES (?1, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now',
                     printf('-%d hours', ?2), printf('+%d minutes', ?3)), 180000, 1)",
            params![track, hours_ago, minutes],
        )
        .expect("listen");
    };
    // 1 and 2 (different artists) back to back in two sessions; 3 and 4 in eight.
    for hours_ago in [30, 20] {
        listen(1, hours_ago, 0);
        listen(2, hours_ago, 5);
    }
    for hours_ago in [80, 70, 60, 50, 45, 40, 35, 25] {
        listen(3, hours_ago, 0);
        listen(4, hours_ago, 5);
    }

    compute_track_similarity(&conn).expect("compute similarity");

    let co = |a: i64, b: i64| -> f64 {
        conn.query_row(
            "SELECT co_listen_score FROM track_similarity WHERE track_a=?1 AND track_b=?2",
            params![a, b],
            |row| row.get(0),
        )
        .expect("co-listened pair is a candidate")
    };
    // Both cross-artist pairs are scored; the strongest normalizes to 1.0.
    let (a, b) = (co(1, 2), co(3, 4));
    assert!(a > 0.0 && b > 0.0, "co(1, 2) = {a}, co(3, 4) = {b}");
    assert!((a.max(b) - 1.0).abs() < 1e-9);
}

#[test]
fn compute_track_similarity_co_listen_prefers_pairs_beyond_chance() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    schema::run_migrations(&conn).expect("migrations");
    for id in 1..=8 {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (?1, ?2)",
            params![id, format!("A{id}")],
        )
        .expect("artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, ?2, ?1, 180000)",
            params![id, format!("T{id}")],
        )
        .expect("track");
    }
    let listen = |track: i64, hours_ago: i64, minutes: i64| {
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
             VALUES (?1, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now',
                     printf('-%d hours', ?2), printf('+%d minutes', ?3)), 180000, 1)",
            params![track, hours_ago, minutes],
        )
        .expect("listen");
    };
    // 1 and 2 are only ever heard together (twice).
    for hours_ago in [10, 20] {
        listen(1, hours_ago, 0);
        listen(2, hours_ago, 5);
    }
    // 3 is popular: twice with 4, and once each with 5..8.
    for hours_ago in [30, 40] {
        listen(3, hours_ago, 0);
        listen(4, hours_ago, 5);
    }
    for (partner, hours_ago) in [(5, 50), (6, 60), (7, 70), (8, 80)] {
        listen(3, hours_ago, 0);
        listen(partner, hours_ago, 5);
    }

    compute_track_similarity(&conn).expect("compute similarity");

    let co = |a: i64, b: i64| -> f64 {
        conn.query_row(
            "SELECT co_listen_score FROM track_similarity WHERE track_a=?1 AND track_b=?2",
            params![a, b],
            |row| row.get(0),
        )
        .expect("co-listened pair")
    };
    assert!(
        co(1, 2) > co(3, 4),
        "a pair heard only together ({}) beats a popular track's pair ({})",
        co(1, 2),
        co(3, 4)
    );
}

#[test]
fn compute_track_similarity_genre_candidates_skip_broad_genres_and_reach_high_ids() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    schema::run_migrations(&conn).expect("migrations");
    // One artist with 1,001 tracks: over the 100-track same-artist cap, so
    // genre is the only candidate source.
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Prolific')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO genres (id, name, slug) VALUES
            (1,'Broad','broad'),(2,'Niche Low','niche-low'),(3,'Niche High','niche-high')",
        [],
    )
    .expect("genres");
    let tx = conn.unchecked_transaction().expect("tx");
    for id in 1..=1001 {
        tx.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, ?2, 1, 180000)",
            params![id, format!("T{id}")],
        )
        .expect("track");
        tx.execute(
            "INSERT INTO track_genres (track_id, genre_id) VALUES (?1, 1)",
            params![id],
        )
        .expect("broad genre");
    }
    tx.execute(
        "INSERT INTO track_genres (track_id, genre_id) VALUES (1,2),(2,2),(1000,3),(1001,3)",
        [],
    )
    .expect("niche genres");
    tx.commit().expect("commit");

    compute_track_similarity(&conn).expect("compute similarity");

    let exists = |a: i64, b: i64| -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM track_similarity WHERE track_a=?1 AND track_b=?2)",
            params![a, b],
            |row| row.get(0),
        )
        .expect("exists")
    };
    assert!(exists(1, 2), "niche pair at low ids is a candidate");
    assert!(
        exists(1000, 1001),
        "niche pair at high ids is a candidate too"
    );
    assert!(
        !exists(1, 1001),
        "sharing only a 1,001-track genre does not make a candidate"
    );
}

#[test]
fn get_genre_diverse_candidates_samples_one_track_per_artist() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO artists (id, name) VALUES (1,'A'),(2,'B'),(3,'C'),(9,'Seed')",
        [],
    )
    .expect("artists");
    conn.execute(
        "INSERT INTO genres (id, name, slug) VALUES (1,'Shared','shared'),(2,'Other','other')",
        [],
    )
    .expect("genres");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES
            (10,'seed',9,1000),(11,'other-seed',9,1000),
            (201,'b-low',2,1000),(202,'b-high',2,1000),
            (301,'c-only',3,1000),
            (401,'off-genre',9,1000)",
        [],
    )
    .expect("tracks");
    // Artist 1 has twenty tracks in the shared genre.
    for id in 101..=120 {
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, 'a', 1, 1000)",
            params![id],
        )
        .expect("artist 1 track");
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id) VALUES (?1, 1)",
            params![id],
        )
        .expect("artist 1 genre");
    }
    conn.execute(
        "INSERT INTO track_genres (track_id, genre_id) VALUES
            (10,1),(11,1),(201,1),(202,1),(301,1),(401,2)",
        [],
    )
    .expect("track_genres");

    let out = get_genre_diverse_candidates(&conn, 10, 100).expect("query");
    let artist_ids: HashSet<i64> = out.iter().map(|t| t.artist_id).collect();
    // One representative per artist that shares genre 1 - never two from one.
    assert_eq!(out.len(), artist_ids.len(), "no artist appears twice");
    assert!(artist_ids.contains(&1) && artist_ids.contains(&2) && artist_ids.contains(&3));
    // The seed never widens to itself, and off-genre tracks never leak in.
    assert!(!out.iter().any(|t| t.id == 10));
    assert!(!out.iter().any(|t| t.id == 401));
    // Stable for a seed, but a different seed samples differently.
    let again = get_genre_diverse_candidates(&conn, 10, 100).expect("query");
    assert_eq!(
        out.iter().map(|t| t.id).collect::<Vec<_>>(),
        again.iter().map(|t| t.id).collect::<Vec<_>>()
    );
    let artist_one = |tracks: &[Track]| tracks.iter().find(|t| t.artist_id == 1).map(|t| t.id);
    let other = get_genre_diverse_candidates(&conn, 11, 100).expect("query");
    assert_ne!(
        artist_one(&out),
        artist_one(&other),
        "seeds should sample differently"
    );
}

mod dj_transition_event {
    use super::*;

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id)
             VALUES (1, 'Track 1', 1, 1001), (2, 'Track 2', 1, 1002)",
            [],
        )
        .expect("tracks");
        conn
    }

    fn insert_event(conn: &Connection) -> i64 {
        insert_dj_transition_event(
            conn,
            Some(1),
            Some(2),
            Some("library_track"),
            Some("1"),
            Some("tidal_track"),
            Some("200"),
            "SafeCrossfade",
            r#"{"template":"SafeCrossfade"}"#,
            None,
            "dj-v1",
            None,
            Some(172_000),
            Some("downbeat_sync"),
            Some("armed"),
        )
        .expect("insert event")
    }

    #[test]
    fn insert_dj_transition_event_round_trips() {
        let conn = setup_conn();
        let id = insert_event(&conn);

        let row = conn
            .query_row(
                "SELECT from_track_id, to_track_id, template, program_json, planner_version,
                        planned_start_ms, timing_source, timing_status
                 FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .expect("event");

        assert_eq!(row.0, Some(1));
        assert_eq!(row.1, Some(2));
        assert_eq!(row.2, "SafeCrossfade");
        assert_eq!(row.3, r#"{"template":"SafeCrossfade"}"#);
        assert_eq!(row.4, "dj-v1");
        assert_eq!(row.5, Some(172_000));
        assert_eq!(row.6.as_deref(), Some("downbeat_sync"));
        assert_eq!(row.7.as_deref(), Some("armed"));
    }

    #[test]
    fn musical_policy_and_feedback_round_trip_without_changing_playback_outcomes() {
        let conn = setup_conn();
        assert_eq!(get_dj_preferred_strategy(&conn).unwrap(), "adaptive");
        set_dj_preferred_strategy(&conn, "club_mix").unwrap();
        assert_eq!(get_dj_preferred_strategy(&conn).unwrap(), "club_mix");
        assert!(set_dj_preferred_strategy(&conn, "unknown").is_err());
        assert_eq!(get_dj_preferred_strategy(&conn).unwrap(), "club_mix");
        let id = insert_event(&conn);
        assert!(!record_dj_feedback(&conn, id, "good", None).unwrap());
        update_dj_transition_fire_timing(
            &conn,
            id,
            172_000,
            "fired",
            true,
            "rendered_handoff",
            "none",
        )
        .unwrap();
        record_dj_feedback(&conn, id, "too_safe", Some("More variation")).unwrap();
        let (rating, outcome, category): (i64, Option<String>, String) = conn
            .query_row(
                "SELECT e.user_rating, e.outcome, c.value FROM dj_transition_events e
             JOIN server_config c ON c.key = 'dj_feedback:' || e.id WHERE e.id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(rating, 0);
        assert!(outcome.is_none());
        assert_eq!(category, "too_safe");
        record_dj_feedback(&conn, id, "good", None).unwrap();
        let category: String = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = ?1",
                [format!("dj_feedback:{id}")],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(category, "good", "a repeated vote replaces the prior vote");
        assert!(!record_dj_feedback(&conn, 999_999, "bad", None).unwrap());
    }

    #[test]
    fn insert_dj_transition_event_round_trips_external_refs() {
        let conn = setup_conn();
        let id = insert_dj_transition_event(
            &conn,
            None,
            None,
            Some("queue_item"),
            Some("44"),
            Some("tidal_track"),
            Some("555"),
            "SafeCrossfade",
            "{}",
            None,
            "dj-v1",
            None,
            None,
            None,
            None,
        )
        .expect("insert external refs");

        let refs = conn
            .query_row(
                "SELECT from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id
                 FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .expect("refs");

        assert_eq!(refs.0.as_deref(), Some("queue_item"));
        assert_eq!(refs.1.as_deref(), Some("44"));
        assert_eq!(refs.2.as_deref(), Some("tidal_track"));
        assert_eq!(refs.3.as_deref(), Some("555"));
    }

    #[test]
    fn insert_dj_transition_event_round_trips_rejected_alternatives() {
        let conn = setup_conn();
        let rejected = r#"[{"template":"SlamCut","score":0.2,"reason":"low_confidence"}]"#;
        let id = insert_dj_transition_event(
            &conn,
            Some(1),
            Some(2),
            Some("library_track"),
            Some("1"),
            Some("library_track"),
            Some("2"),
            "SafeCrossfade",
            "{}",
            Some(rejected),
            "dj-v1",
            None,
            None,
            None,
            None,
        )
        .expect("insert rejected");

        let loaded: String = conn
            .query_row(
                "SELECT rejected_alternatives_json FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("rejected");
        assert_eq!(loaded, rejected);
    }

    #[test]
    fn insert_dj_transition_event_accepts_known_fallback_reasons() {
        let conn = setup_conn();
        let reasons = [
            "disabled",
            "current_profile_missing",
            "next_profile_missing",
            "profile_low_confidence",
            "next_not_resolved",
            "fetch_failed",
            "decode_late",
            "analysis_late",
            "program_invalid",
            "queue_changed",
            "safety_override_safe",
        ];

        for reason in reasons {
            insert_dj_transition_event(
                &conn,
                None,
                None,
                None,
                None,
                None,
                None,
                "SafeCrossfade",
                "{}",
                None,
                "dj-v1",
                Some(reason),
                None,
                None,
                None,
            )
            .unwrap_or_else(|error| panic!("reason {reason} rejected: {error}"));
        }
        assert!(
            insert_dj_transition_event(
                &conn,
                None,
                None,
                None,
                None,
                None,
                None,
                "SafeCrossfade",
                "{}",
                None,
                "dj-v1",
                Some("unknown"),
                None,
                None,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn update_dj_transition_outcome_sets_timestamp() {
        let conn = setup_conn();
        let id = insert_event(&conn);

        update_dj_transition_outcome(&conn, id, "bad", true).expect("update");

        let row = conn
            .query_row(
                "SELECT outcome, outcome_at, skip_within_30s
                 FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .expect("outcome");
        assert_eq!(row.0.as_deref(), Some("bad"));
        assert!(row.1.is_some());
        assert_eq!(row.2, 1);
    }

    #[test]
    fn manual_skip_outcome_does_not_write_actual_timing() {
        let conn = setup_conn();
        let id = insert_event(&conn);

        update_dj_transition_outcome(&conn, id, "skip_within_30s", true).expect("update");

        let actual_start_ms: Option<i64> = conn
            .query_row(
                "SELECT actual_start_ms FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("actual");
        assert_eq!(actual_start_ms, None);
    }

    #[test]
    fn update_dj_transition_fire_timing_sets_delta_and_status() {
        let conn = setup_conn();
        let id = insert_event(&conn);

        update_dj_transition_fire_timing(
            &conn,
            id,
            172_144,
            "fired",
            true,
            "rendered_handoff",
            "none",
        )
        .expect("timing");

        let row = conn
            .query_row(
                "SELECT actual_start_ms, timing_delta_ms, timing_status,
                        runtime_rendered_dj_mixer, runtime_renderer_status,
                        runtime_renderer_reason
                 FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .expect("timing row");
        assert_eq!(row.0, Some(172_144));
        assert_eq!(row.1, Some(144));
        assert_eq!(row.2.as_deref(), Some("fired"));
        assert_eq!(row.3, Some(1));
        assert_eq!(row.4.as_deref(), Some("rendered_handoff"));
        assert_eq!(row.5.as_deref(), Some("none"));
    }

    #[test]
    fn decoded_countdown_target_preserves_metadata_error_separately_from_fire_delta() {
        let conn = setup_conn();
        let id = insert_event(&conn);
        conn.execute(
            "UPDATE dj_transition_events SET timing_source = 'fallback_overlap',
            planned_start_ms = 401000 WHERE id = ?1",
            [id],
        )
        .unwrap();
        update_dj_transition_fire_timing_with_runtime_target(
            &conn,
            id,
            403_006,
            Some(403_000),
            "fired",
            true,
            "rendered_handoff",
            "none",
        )
        .unwrap();
        let timing: (i64, i64, i64, i64) = conn.query_row(
            "SELECT planned_start_ms, runtime_planned_start_ms, actual_start_ms, timing_delta_ms
             FROM dj_transition_events WHERE id = ?1", [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(timing, (401_000, 403_000, 403_006, 6));
        // The visible original estimate still exposes the 2s duration
        // mismatch; the scheduler's 6ms precision is measured honestly.
        assert_eq!(timing.1 - timing.0, 2_000);
        update_dj_transition_fire_timing_with_runtime_target(
            &conn,
            id,
            409_000,
            Some(403_000),
            "missed",
            false,
            "boundary_fallback",
            "manual_seek_suppressed",
        )
        .unwrap();
        let cleared: (i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT planned_start_ms, runtime_planned_start_ms, timing_delta_ms
             FROM dj_transition_events WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(cleared, (401_000, None, None));
    }

    #[test]
    fn update_dj_transition_fire_timing_leaves_missed_fire_timing_empty() {
        let conn = setup_conn();
        let id = insert_event(&conn);

        update_dj_transition_fire_timing(
            &conn,
            id,
            199_465,
            "missed",
            false,
            "boundary_fallback",
            "prepared_mixer_missing",
        )
        .expect("timing");

        let row = conn
            .query_row(
                "SELECT actual_start_ms, timing_delta_ms, timing_status,
                        runtime_rendered_dj_mixer, runtime_renderer_status,
                        runtime_renderer_reason
                 FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .expect("timing row");

        assert_eq!(row.0, None);
        assert_eq!(row.1, None);
        assert_eq!(row.2.as_deref(), Some("missed"));
        assert_eq!(row.3, Some(0));
        assert_eq!(row.4.as_deref(), Some("boundary_fallback"));
        assert_eq!(row.5.as_deref(), Some("prepared_mixer_missing"));
    }

    #[test]
    fn update_dj_transition_fire_timing_accepts_precise_miss_reasons() {
        let conn = setup_conn();

        for reason in [
            "next_decode_late_at_fire",
            "next_deck_missing_at_fire",
            "transition_plan_missing_at_fire",
            "sync_window_not_signaled",
        ] {
            let id = insert_event(&conn);
            update_dj_transition_fire_timing(
                &conn,
                id,
                199_465,
                "missed",
                false,
                "boundary_fallback",
                reason,
            )
            .expect("timing");

            let stored_reason: Option<String> = conn
                .query_row(
                    "SELECT runtime_renderer_reason FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .expect("stored reason");
            assert_eq!(stored_reason.as_deref(), Some(reason));
        }
    }

    #[test]
    fn update_dj_transition_fire_timing_accepts_manual_seek_reason() {
        let conn = setup_conn();
        let id = insert_event(&conn);

        update_dj_transition_fire_timing(
            &conn,
            id,
            180_000,
            "late",
            false,
            "boundary_fallback",
            "manual_seek_suppressed",
        )
        .expect("manual seek timing");

        let reason: Option<String> = conn
            .query_row(
                "SELECT runtime_renderer_reason FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("reason");
        assert_eq!(reason.as_deref(), Some("manual_seek_suppressed"));
    }

    #[test]
    fn update_dj_transition_fire_timing_closes_duplicate_armed_pair_rows() {
        let conn = setup_conn();
        let older_id = insert_event(&conn);
        let fired_id = insert_event(&conn);

        update_dj_transition_fire_timing(
            &conn,
            fired_id,
            172_144,
            "fired",
            false,
            "legacy_overlap",
            "next_deck_not_decoded",
        )
        .expect("timing");

        let rows: Vec<(i64, Option<String>)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, timing_status
                     FROM dj_transition_events
                     WHERE id IN (?1, ?2)
                     ORDER BY id",
                )
                .expect("prepare");
            stmt.query_map(params![older_id, fired_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows")
        };

        assert_eq!(rows[0], (older_id, Some("missed".to_string())));
        assert_eq!(rows[1], (fired_id, Some("fired".to_string())));
    }

    #[test]
    fn mark_dj_transition_timing_status_for_pair_updates_only_armed_pair() {
        let conn = setup_conn();
        let updated = mark_dj_transition_timing_status_for_pair(
            &conn,
            "library_track",
            "1",
            "tidal_track",
            "200",
            "missed",
        )
        .expect("mark before insert");
        assert_eq!(updated, 0);

        let id = insert_event(&conn);
        let updated = mark_dj_transition_timing_status_for_pair(
            &conn,
            "library_track",
            "1",
            "tidal_track",
            "200",
            "missed",
        )
        .expect("mark");
        assert_eq!(updated, 1);

        let status: Option<String> = conn
            .query_row(
                "SELECT timing_status FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("status");
        assert_eq!(status.as_deref(), Some("missed"));
    }

    #[test]
    fn mark_dj_transition_manual_seek_suppressed_closes_armed_pair() {
        let conn = setup_conn();
        let id = insert_event(&conn);

        let updated = mark_dj_transition_manual_seek_suppressed_for_pair(
            &conn,
            "library_track",
            "1",
            "tidal_track",
            "200",
        )
        .expect("mark manual seek suppressed");
        assert_eq!(updated, 1);

        let row = conn
            .query_row(
                "SELECT timing_status, outcome, runtime_rendered_dj_mixer,
                        runtime_renderer_status, runtime_renderer_reason
                 FROM dj_transition_events WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .expect("manual seek row");

        assert_eq!(row.0.as_deref(), Some("missed"));
        assert_eq!(row.1.as_deref(), Some("manual_seek_suppressed"));
        assert_eq!(row.2, Some(0));
        assert_eq!(row.3.as_deref(), Some("boundary_fallback"));
        assert_eq!(row.4.as_deref(), Some("manual_seek_suppressed"));

        let second = mark_dj_transition_manual_seek_suppressed_for_pair(
            &conn,
            "library_track",
            "1",
            "tidal_track",
            "200",
        )
        .expect("mark manual seek suppressed again");
        assert_eq!(second, 0);
    }

    #[test]
    fn mark_dj_transition_timing_status_for_pair_updates_new_attempt_after_old_fired_pair() {
        let conn = setup_conn();
        let fired_id = insert_event(&conn);
        update_dj_transition_fire_timing(
            &conn,
            fired_id,
            172_040,
            "fired",
            false,
            "legacy_overlap",
            "prepared_mixer_missing",
        )
        .expect("mark fired");
        let armed_id = insert_event(&conn);

        let updated = mark_dj_transition_timing_status_for_pair(
            &conn,
            "library_track",
            "1",
            "tidal_track",
            "200",
            "missed",
        )
        .expect("mark");

        assert_eq!(updated, 1);
        let status: Option<String> = conn
            .query_row(
                "SELECT timing_status FROM dj_transition_events WHERE id = ?1",
                params![armed_id],
                |row| row.get(0),
            )
            .expect("status");
        assert_eq!(status.as_deref(), Some("missed"));
    }
}

mod audio_dj_profile {
    use super::*;

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        schema::run_migrations(&conn).expect("migrations");
        conn
    }

    fn key(kind: &str, id: &str) -> AudioDjProfileKey {
        AudioDjProfileKey {
            media_ref_kind: kind.to_string(),
            media_ref_id: id.to_string(),
        }
    }

    fn seed_track(conn: &Connection) -> i64 {
        conn.execute("INSERT INTO artists (name) VALUES ('Artist')", [])
            .expect("artist");
        let artist_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO tracks (title, artist_id, tidal_id) VALUES ('Track', ?1, 12345)",
            params![artist_id],
        )
        .expect("track");
        conn.last_insert_rowid()
    }

    fn profile_row(kind: &str, id: &str) -> AudioDjProfileRow {
        AudioDjProfileRow {
            media_ref_kind: kind.to_string(),
            media_ref_id: id.to_string(),
            track_id: None,
            queue_item_id: None,
            tidal_id: None,
            profile_version: "dj_profile_v1".to_string(),
            beat_grid_blob: vec![1, 2, 3],
            downbeats_blob: vec![4, 5],
            phrase_boundaries_blob: vec![6],
            mix_in_blob: vec![7],
            mix_out_blob: vec![8],
            intro_end_seconds: Some(16.0),
            outro_start_seconds: Some(180.0),
            breakdown_blob: vec![9],
            drop_blob: vec![10],
            safe_transition_windows_blob: vec![11],
            energy_contour_blob: vec![12],
            vocal_presence_blob: vec![13],
            vocal_density_blob: vec![14],
            waveform_peaks_blob: vec![15],
            lufs_loud_body: Some(-12.0),
            true_peak_dbtp: Some(-1.0),
            beat_confidence: Some(0.9),
            profile_confidence: 0.85,
            analysis_scope_ms: 90_000,
            is_temporary: false,
            source: "test".to_string(),
            computed_at: "2026-05-21T00:00:00Z".to_string(),
        }
    }

    fn correction_row(kind: &str, id: &str) -> AudioDjProfileCorrectionRow {
        AudioDjProfileCorrectionRow {
            media_ref_kind: kind.to_string(),
            media_ref_id: id.to_string(),
            bpm_multiplier: Some(2.0),
            downbeat_offset_beats: Some(1),
            phrase_offset_bars: Some(-2),
            safe_crossfade_only: true,
            transition_speed_bias: Some("faster".to_string()),
            manual_drop_blob: vec![20, 21, 22],
            notes: Some("user correction".to_string()),
            created_at: "2026-05-21T00:00:00Z".to_string(),
            updated_at: "2026-05-21T00:00:01Z".to_string(),
        }
    }

    fn table_exists(conn: &Connection, table: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |_| Ok(()),
        )
        .optional()
        .expect("table lookup")
        .is_some()
    }

    #[test]
    fn migration_043_creates_audio_dj_profiles() {
        let conn = setup_conn();
        assert!(table_exists(&conn, "audio_dj_profiles"));
        assert!(table_exists(&conn, "dj_transition_events"));
    }

    #[test]
    fn migration_043_creates_audio_dj_profile_corrections() {
        let conn = setup_conn();
        assert!(table_exists(&conn, "audio_dj_profile_corrections"));
    }

    #[test]
    fn upsert_audio_dj_profile_round_trips_library_key() {
        let conn = setup_conn();
        let track_id = seed_track(&conn);
        let mut row = profile_row("library_track", &track_id.to_string());
        row.track_id = Some(track_id);

        upsert_audio_dj_profile(&conn, &row).expect("upsert");
        let loaded = get_audio_dj_profile(&conn, &key("library_track", &track_id.to_string()))
            .expect("get")
            .expect("profile");
        assert_eq!(loaded.track_id, Some(track_id));
        assert_eq!(loaded.media_ref_kind, "library_track");

        let by_track = get_audio_dj_profile_for_track(&conn, track_id)
            .expect("get track")
            .expect("track profile");
        assert_eq!(by_track.media_ref_id, track_id.to_string());
    }

    #[test]
    fn upsert_audio_dj_profile_round_trips_tidal_key() {
        let conn = setup_conn();
        let mut row = profile_row("tidal_track", "98765");
        row.tidal_id = Some(98_765);

        upsert_audio_dj_profile(&conn, &row).expect("upsert");
        let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "98765"))
            .expect("get")
            .expect("profile");
        assert_eq!(loaded.tidal_id, Some(98_765));
        assert_eq!(loaded.media_ref_id, "98765");
    }

    #[test]
    fn audio_dj_profile_allows_external_queue_item_without_track_id() {
        let conn = setup_conn();
        conn.execute(
            "INSERT INTO queue (position, source, pending_artist, pending_title)
             VALUES (1, 'radio_pending', 'Artist', 'Title')",
            [],
        )
        .expect("queue item");
        let queue_item_id = conn.last_insert_rowid();
        let mut row = profile_row("queue_item", &queue_item_id.to_string());
        row.queue_item_id = Some(queue_item_id);
        row.is_temporary = true;

        upsert_audio_dj_profile(&conn, &row).expect("upsert");
        let loaded = get_audio_dj_profile(&conn, &key("queue_item", &queue_item_id.to_string()))
            .expect("get")
            .expect("profile");
        assert_eq!(loaded.track_id, None);
        assert_eq!(loaded.queue_item_id, Some(queue_item_id));
        assert!(loaded.is_temporary);
    }

    #[test]
    fn audio_dj_profile_stores_confidence_and_scope() {
        let conn = setup_conn();
        let mut row = profile_row("tidal_track", "1");
        row.profile_confidence = 0.4;
        row.analysis_scope_ms = 30_000;

        upsert_audio_dj_profile(&conn, &row).expect("upsert");
        let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "1"))
            .expect("get")
            .expect("profile");
        assert_eq!(loaded.profile_confidence, 0.4);
        assert_eq!(loaded.analysis_scope_ms, 30_000);
    }

    #[test]
    fn audio_dj_profile_peak_blob_round_trips() {
        let conn = setup_conn();
        let mut row = profile_row("tidal_track", "1");
        row.waveform_peaks_blob = vec![1, 2, 3, 4, 5];

        upsert_audio_dj_profile(&conn, &row).expect("upsert");
        let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "1"))
            .expect("get")
            .expect("profile");

        assert_eq!(loaded.waveform_peaks_blob, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn audio_dj_profile_empty_peak_blob_loads() {
        let conn = setup_conn();
        let mut row = profile_row("tidal_track", "1");
        row.waveform_peaks_blob = Vec::new();

        upsert_audio_dj_profile(&conn, &row).expect("upsert");
        let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "1"))
            .expect("get")
            .expect("profile");

        assert!(loaded.waveform_peaks_blob.is_empty());
    }

    #[test]
    fn promote_temporary_audio_dj_profile_copies_to_stable_key() {
        let conn = setup_conn();
        let mut row = profile_row("queue_item", "44");
        row.is_temporary = true;
        upsert_audio_dj_profile(&conn, &row).expect("upsert temp");

        promote_temporary_audio_dj_profile(
            &conn,
            &key("queue_item", "44"),
            &key("tidal_track", "555"),
            Some(555),
        )
        .expect("promote");

        let stable = get_audio_dj_profile(&conn, &key("tidal_track", "555"))
            .expect("get stable")
            .expect("stable");
        let temporary = get_audio_dj_profile(&conn, &key("queue_item", "44"))
            .expect("get temp")
            .expect("temp");
        assert_eq!(stable.beat_grid_blob, temporary.beat_grid_blob);
        assert_eq!(stable.tidal_id, Some(555));
        assert!(!stable.is_temporary);
        assert!(temporary.is_temporary);
    }

    #[test]
    fn upsert_audio_dj_profile_correction_round_trips() {
        let conn = setup_conn();
        let row = correction_row("tidal_track", "1");
        upsert_audio_dj_profile_correction(&conn, &row).expect("upsert correction");

        let loaded = get_audio_dj_profile_correction(&conn, &key("tidal_track", "1"))
            .expect("get")
            .expect("correction");
        assert_eq!(loaded.bpm_multiplier, Some(2.0));
        assert_eq!(loaded.downbeat_offset_beats, Some(1));
        assert_eq!(loaded.phrase_offset_bars, Some(-2));
        assert!(loaded.safe_crossfade_only);
        assert_eq!(loaded.transition_speed_bias.as_deref(), Some("faster"));
        assert_eq!(loaded.manual_drop_blob, vec![20, 21, 22]);
    }

    #[test]
    fn audio_dj_profile_correction_rejects_unknown_transition_speed_bias() {
        let conn = setup_conn();
        let mut row = correction_row("tidal_track", "1");
        row.transition_speed_bias = Some("sideways".to_string());

        assert!(upsert_audio_dj_profile_correction(&conn, &row).is_err());
    }

    #[test]
    fn dj_engine_enabled_defaults_false() {
        let conn = setup_conn();
        assert!(!is_dj_engine_enabled(&conn).expect("enabled"));
    }

    #[test]
    fn set_dj_engine_enabled_round_trips() {
        let conn = setup_conn();
        set_dj_engine_enabled(&conn, true).expect("enable");
        assert!(is_dj_engine_enabled(&conn).expect("enabled"));
        set_dj_engine_enabled(&conn, false).expect("disable");
        assert!(!is_dj_engine_enabled(&conn).expect("disabled"));
    }

    #[test]
    fn dj_global_policy_defaults_balanced_neutral() {
        let conn = setup_conn();
        assert_eq!(
            get_dj_global_policy(&conn).expect("policy"),
            ("balanced".to_string(), "neutral".to_string())
        );
    }

    #[test]
    fn set_dj_global_policy_round_trips() {
        let conn = setup_conn();
        set_dj_global_policy(&conn, "bold", "faster").expect("set policy");
        assert_eq!(
            get_dj_global_policy(&conn).expect("policy"),
            ("bold".to_string(), "faster".to_string())
        );
    }

    #[test]
    fn set_dj_global_policy_rejects_unknown_values() {
        let conn = setup_conn();
        assert!(set_dj_global_policy(&conn, "chaos", "neutral").is_err());
        assert!(set_dj_global_policy(&conn, "safe", "sideways").is_err());
    }

    #[test]
    fn count_recent_bad_dj_feedback_for_ref_counts_from_and_to_roles() {
        let conn = setup_conn();
        let key = key("tidal_track", "1");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, user_rating, started_at
             ) VALUES (?1, ?2, 'tidal_track', '2', 'SafeCrossfade', '{}', 'v1', -1, '2026-05-21T00:00:00Z')",
            params![key.media_ref_kind, key.media_ref_id],
        )
        .expect("from event");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, user_rating, started_at
             ) VALUES ('tidal_track', '3', ?1, ?2, 'SafeCrossfade', '{}', 'v1', -1, '2026-05-21T00:00:01Z')",
            params![key.media_ref_kind, key.media_ref_id],
        )
        .expect("to event");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, user_rating, started_at
             ) VALUES (?1, ?2, 'tidal_track', '4', 'SafeCrossfade', '{}', 'v1', 1, '2026-05-21T00:00:02Z')",
            params![key.media_ref_kind, key.media_ref_id],
        )
        .expect("good event");

        assert_eq!(
            count_recent_bad_dj_feedback_for_ref(&conn, &key, 10).expect("count"),
            2
        );
        assert_eq!(
            count_recent_bad_dj_feedback_for_ref(&conn, &key, 1).expect("count"),
            0
        );
    }
}

#[test]
fn onboarding_unset_no_tidal_returns_false() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    assert!(!get_onboarding_complete(&conn).expect("read flag"));
    assert_eq!(read_onboarding_value(&conn).as_deref(), Some("0"));
    conn.execute(
        "INSERT INTO service_auth(service,user_id) VALUES('tidal','new-user')",
        [],
    )
    .unwrap();
    assert!(
        !get_onboarding_complete(&conn).unwrap(),
        "Connecting TIDAL must not finish a setup in progress"
    );
}

#[test]
fn onboarding_unset_with_tidal_writes_flag_and_returns_true() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO service_auth (service, user_id) VALUES ('tidal', 'u-123')",
        [],
    )
    .expect("seed tidal auth");

    assert!(get_onboarding_complete(&conn).expect("read flag"));
    assert_eq!(read_onboarding_value(&conn).as_deref(), Some("1"));
}

#[test]
fn create_embedding_model_inserts_run_scoped_rows_without_overwriting_active_model() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    let active = create_embedding_model(
        &conn,
        "discovery-fusion-v2:1",
        "discovery-fusion-v2",
        64,
        "ready",
        Some(r#"{"run":1}"#),
    )
    .expect("create active");
    activate_embedding_model(&conn, active.id).expect("activate active");

    let candidate = create_embedding_model(
        &conn,
        "discovery-fusion-v2:2",
        "discovery-fusion-v2",
        64,
        "training",
        Some(r#"{"run":2}"#),
    )
    .expect("create candidate");

    assert_ne!(active.id, candidate.id);
    let still_active = get_selected_discovery_embedding_model(&conn)
        .expect("selected lookup")
        .expect("selected model");
    assert_eq!(still_active.id, active.id);
    assert_eq!(still_active.model_key, "discovery-fusion-v2:1");
}

#[test]
fn neighbor_support_breakdown_round_trips_through_full_and_seed_replacement() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id, duration_ms)
         VALUES (1, 'Seed', 1, 101, 180000),
                (2, 'Neighbor', 1, 102, 181000),
                (3, 'Refresh', 1, 103, 182000)",
        [],
    )
    .expect("seed tracks");
    let model = create_embedding_model(
        &conn,
        "discovery-fusion-v2:1",
        "discovery-fusion-v2",
        64,
        "ready",
        None,
    )
    .expect("create model");

    replace_track_neighbors(
        &conn,
        model.id,
        &[NeighborWriteRow {
            track_id: 1,
            neighbor_track_id: 2,
            rank: 1,
            score: 0.91,
            behavioral_score: 0.4,
            audio_score: 0.3,
            metadata_score: 0.2,
            reason_json: None,
            primary_reason: Some("direct_transition".to_string()),
            confidence: 0.8,
            support_count: 4,
            support_transition: 2.5,
            support_colisten: 1.25,
            support_structure: 0.75,
            support_metadata: 0.5,
            candidate_in_degree: 7,
            candidate_in_degree_percentile: 0.7,
            play_count_seed: 10,
            play_count_candidate: 3,
        }],
    )
    .expect("replace neighbors");

    let rows = get_track_neighbors(&conn, model.id, 1, 10, &[]).expect("read neighbors");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].support_count, 4);
    assert_eq!(rows[0].support_transition, 2.5);
    assert_eq!(rows[0].support_colisten, 1.25);
    assert_eq!(rows[0].support_structure, 0.75);
    assert_eq!(rows[0].support_metadata, 0.5);

    replace_seed_neighbors(
        &conn,
        model.id,
        1,
        &[NeighborWriteRow {
            track_id: 1,
            neighbor_track_id: 3,
            rank: 1,
            score: 0.88,
            behavioral_score: 0.35,
            audio_score: 0.35,
            metadata_score: 0.18,
            reason_json: None,
            primary_reason: Some("session_colisten".to_string()),
            confidence: 0.77,
            support_count: 3,
            support_transition: 0.0,
            support_colisten: 2.0,
            support_structure: 1.0,
            support_metadata: 0.25,
            candidate_in_degree: 5,
            candidate_in_degree_percentile: 0.6,
            play_count_seed: 10,
            play_count_candidate: 4,
        }],
    )
    .expect("replace seed neighbors");

    let refreshed =
        get_track_neighbors(&conn, model.id, 1, 10, &[]).expect("read refreshed neighbors");
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].track_id, 3);
    assert_eq!(refreshed[0].support_count, 3);
    assert_eq!(refreshed[0].support_transition, 0.0);
    assert_eq!(refreshed[0].support_colisten, 2.0);
    assert_eq!(refreshed[0].support_structure, 1.0);
    assert_eq!(refreshed[0].support_metadata, 0.25);
}

#[test]
fn selected_discovery_model_lookup_uses_configured_engine_family() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    let legacy = create_embedding_model(
        &conn,
        "discovery-fusion:legacy",
        "discovery-fusion",
        96,
        "ready",
        None,
    )
    .expect("create legacy model");
    let v2 = create_embedding_model(
        &conn,
        "discovery-fusion-v2:default",
        "discovery-fusion-v2",
        64,
        "ready",
        None,
    )
    .expect("create v2 model");
    activate_embedding_model(&conn, v2.id).expect("activate v2");

    let selected = get_selected_discovery_embedding_model(&conn)
        .expect("selected lookup")
        .expect("selected default model");
    assert_eq!(selected.id, v2.id);

    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('discovery_engine', 'v1')",
        [],
    )
    .expect("select legacy engine");

    let selected = get_selected_discovery_embedding_model(&conn)
        .expect("selected lookup")
        .expect("selected legacy model");
    assert_eq!(selected.id, legacy.id);
    assert_eq!(selected.family, "discovery-fusion");
}

#[test]
fn is_discovery_training_running_tracks_run_status() {
    // Regression: the radio similarity rebuild gates on this so it can't run
    // a multi-minute write transaction alongside discovery training.
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    assert!(
        !is_discovery_training_running(&conn).expect("query"),
        "no runs => not training"
    );

    let run = create_training_run(&conn, None, "behavioral", "running").expect("create run");
    assert!(
        is_discovery_training_running(&conn).expect("query"),
        "a running row => training in progress, rebuild must defer"
    );

    finish_training_run(&conn, run.id, "completed").expect("finish run");
    assert!(
        !is_discovery_training_running(&conn).expect("query"),
        "completed run => training done, rebuild may proceed"
    );
}

#[test]
fn finish_training_run_with_error_preserves_cancel_reason() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    let run = create_training_run(&conn, None, "behavioral", "running").expect("create run");

    finish_training_run_with_error(
        &conn,
        run.id,
        "cancelled",
        "Laptop safety timeout stopped discovery training.",
    )
    .expect("finish with reason");

    let stored = get_training_run(&conn, run.id)
        .expect("load run")
        .expect("stored run");
    assert_eq!(stored.status, "cancelled");
    assert_eq!(
        stored.error_text.as_deref(),
        Some("Laptop safety timeout stopped discovery training.")
    );
}

#[test]
fn bulk_neighbor_loading_groups_by_seed_and_preserves_support_columns() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id)
         VALUES (1, 'Seed A', 1, 201),
                (2, 'Seed B', 1, 202),
                (3, 'Candidate A', 1, 203),
                (4, 'Candidate B', 1, 204)",
        [],
    )
    .expect("seed tracks");
    let model = create_embedding_model(
        &conn,
        "discovery-fusion-v2:bulk",
        "discovery-fusion-v2",
        64,
        "ready",
        None,
    )
    .expect("create model");

    let mk = |track_id, neighbor_track_id, rank, support_transition: f64| NeighborWriteRow {
        track_id,
        neighbor_track_id,
        rank,
        score: 0.9,
        behavioral_score: 0.4,
        audio_score: 0.3,
        metadata_score: 0.2,
        reason_json: None,
        primary_reason: None,
        confidence: 0.8,
        support_count: support_transition.round() as i64,
        support_transition,
        support_colisten: 0.0,
        support_structure: 0.0,
        support_metadata: 0.0,
        candidate_in_degree: 0,
        candidate_in_degree_percentile: 0.0,
        play_count_seed: 0,
        play_count_candidate: 0,
    };
    replace_track_neighbors(&conn, model.id, &[mk(1, 3, 1, 2.0), mk(2, 4, 1, 3.0)])
        .expect("replace neighbors");

    let grouped = get_track_neighbors_for_seeds(&conn, model.id, &[1, 2], 10).expect("bulk load");
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped.get(&1).unwrap()[0].track_id, 3);
    assert_eq!(grouped.get(&1).unwrap()[0].support_transition, 2.0);
    assert_eq!(grouped.get(&2).unwrap()[0].track_id, 4);
    assert_eq!(grouped.get(&2).unwrap()[0].support_transition, 3.0);

    // Training writes the graph in chunks: replace with the first chunk,
    // then append the rest without touching what is already there.
    append_track_neighbors(&conn, model.id, &[mk(1, 4, 2, 1.0)]).expect("append chunk");
    let grouped =
        get_track_neighbors_for_seeds(&conn, model.id, &[1, 2], 10).expect("after append");
    let seed_a: Vec<i64> = grouped
        .get(&1)
        .unwrap()
        .iter()
        .map(|n| n.track_id)
        .collect();
    assert_eq!(seed_a, vec![3, 4]);
    assert_eq!(grouped.get(&2).unwrap().len(), 1);

    replace_track_neighbors(&conn, model.id, &[mk(2, 3, 1, 1.0)]).expect("replace again");
    let grouped =
        get_track_neighbors_for_seeds(&conn, model.id, &[1, 2], 10).expect("after replace");
    assert!(!grouped.contains_key(&1));
    assert_eq!(grouped.get(&2).unwrap()[0].track_id, 3);
}

#[test]
fn completion_weighted_listen_edges_window_handles_rfc3339_rows() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, duration_ms)
         VALUES (1,'A',1,180000),(2,'B',1,180000),(3,'C',1,180000)",
        [],
    )
    .expect("tracks");
    // No session ids, so only the time window links listens.
    conn.execute(
        "INSERT INTO listen_history (id, track_id, started_at, duration_listened_ms, completed)
         VALUES
            (1, 1, '2026-10-01T10:00:00.123+00:00', 180000, 1),
            (2, 2, '2026-10-01T10:10:00.456+00:00', 180000, 1),
            (3, 3, '2026-10-01T13:00:00.789+00:00', 180000, 1)",
        [],
    )
    .expect("listens");
    let rows = get_completion_weighted_listen_edges(&conn, 45).expect("edges");
    let pairs: Vec<(i64, i64)> = rows
        .iter()
        .map(|row| (row.from_track_id, row.to_track_id))
        .collect();
    assert_eq!(pairs, vec![(1, 2)]);
}

#[test]
fn completion_weighted_listen_edges_downweight_skipped_tracks() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id, duration_ms)
         VALUES (1, 'Half Listen', 1, 301, 180000),
                (2, 'Complete Listen', 1, 302, 180000)",
        [],
    )
    .expect("seed tracks");
    conn.execute(
        "INSERT INTO listen_history
            (id, track_id, started_at, duration_listened_ms, completed, session_id, source, position_in_session)
         VALUES
            (1, 1, '2026-01-01 00:00:00', 90000, 0, 's1', 'manual', 1),
            (2, 2, '2026-01-01 00:03:00', 180000, 1, 's1', 'manual', 2)",
        [],
    )
    .expect("seed listens");

    let rows = get_completion_weighted_listen_edges(&conn, 45).expect("weighted edges");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].from_track_id, 1);
    assert_eq!(rows[0].to_track_id, 2);
    assert!((rows[0].weight - 0.5).abs() < 1e-9);
    assert_eq!(rows[0].source.as_deref(), Some("manual"));
}

#[test]
fn listen_history_edges_leave_out_early_skips() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, duration_ms)
         VALUES (1, 'Before', 1, 200000), (2, 'Kept', 1, 200000), (3, 'Skipped', 1, 200000)",
        [],
    )
    .expect("tracks");
    conn.execute(
        "INSERT INTO listen_history
            (id, track_id, started_at, duration_listened_ms, completed, session_id, transition_from_track_id)
         VALUES
            (1, 1, '2026-01-01T00:00:00+00:00', 200000, 1, 's1', NULL),
            (2, 2, '2026-01-01T00:04:00+00:00', 120000, 0, 's1', 1),
            (3, 3, '2026-01-01T00:06:00+00:00', 8000, 0, 's1', 2)",
        [],
    )
    .expect("listens");

    let transitions = get_listen_history_transition_edges(&conn).expect("transitions");
    assert_eq!(
        transitions
            .iter()
            .map(|row| (row.from_track_id, row.to_track_id))
            .collect::<Vec<_>>(),
        vec![(1, 2)]
    );
    let pairs = get_completion_weighted_listen_edges(&conn, 45).expect("pairs");
    assert!(
        pairs
            .iter()
            .all(|row| row.to_track_id != 3 && row.from_track_id != 3)
    );
}

#[test]
fn listen_history_transition_edges_preserve_source_and_completion_weight() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id, duration_ms)
         VALUES (1, 'Before', 1, 401, 200000),
                (2, 'After', 1, 402, 200000)",
        [],
    )
    .expect("seed tracks");
    conn.execute(
        "INSERT INTO listen_history
            (id, track_id, started_at, duration_listened_ms, completed, source, transition_from_track_id)
         VALUES
            (10, 2, '2026-01-01 00:04:00', 50000, 0, 'automix-new', 1)",
        [],
    )
    .expect("seed transition listen");

    let rows = get_listen_history_transition_edges(&conn).expect("transition edges");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_id, "listen_history:10");
    assert_eq!(rows[0].from_track_id, 1);
    assert_eq!(rows[0].to_track_id, 2);
    assert!((rows[0].weight - 0.25).abs() < 1e-9);
    assert_eq!(rows[0].source.as_deref(), Some("automix-new"));
}

#[test]
fn external_candidate_upsert_dedupes_unresolved_rows() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    let first = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "artist:unknown|title:signal|dur:180".to_string(),
            title: "Signal".to_string(),
            artist_name: "Unknown Artist".to_string(),
            genre_tags_json: Some(r#"["electronic"]"#.to_string()),
            duration_ms: Some(180_000),
            expires_at: "2026-02-01 00:00:00".to_string(),
        },
    )
    .expect("insert candidate");
    let second = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "artist:unknown|title:signal|dur:180".to_string(),
            title: "Signal".to_string(),
            artist_name: "Unknown Artist".to_string(),
            genre_tags_json: Some(r#"["electronic","fresh"]"#.to_string()),
            duration_ms: Some(180_000),
            expires_at: "2026-02-02 00:00:00".to_string(),
        },
    )
    .expect("upsert candidate");

    assert_eq!(first.id, second.id);
    assert_eq!(
        second.genre_tags_json.as_deref(),
        Some(r#"["electronic","fresh"]"#)
    );
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidates",
            [],
            |row| row.get(0),
        )
        .expect("count candidates");
    assert_eq!(count, 1);
}

#[test]
fn external_candidate_upsert_dedupes_unresolved_rows_by_normalized_identity() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    let first = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "provider-a:signal".to_string(),
            title: "Signal!".to_string(),
            artist_name: "Unknown Artist".to_string(),
            genre_tags_json: None,
            duration_ms: Some(181_000),
            expires_at: "2026-02-01 00:00:00".to_string(),
        },
    )
    .expect("insert candidate");
    let second = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "provider-b:signal".to_string(),
            title: "signal".to_string(),
            artist_name: "unknown-artist".to_string(),
            genre_tags_json: None,
            duration_ms: Some(185_000),
            expires_at: "2026-02-02 00:00:00".to_string(),
        },
    )
    .expect("upsert candidate");

    assert_eq!(first.id, second.id);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidates",
            [],
            |row| row.get(0),
        )
        .expect("count candidates");
    assert_eq!(count, 1);
}

#[test]
fn external_sightings_and_neighbors_replace_without_stale_rows() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id)
         VALUES (1, 'Seed', 1, 501)",
        [],
    )
    .expect("seed track");
    let model = create_embedding_model(
        &conn,
        "discovery-fusion-v2:external",
        "discovery-fusion-v2",
        64,
        "ready",
        None,
    )
    .expect("create model");
    let candidate = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: Some(9001),
            mbid: Some("mbid-9001".to_string()),
            dedupe_key: "tidal:9001".to_string(),
            title: "External".to_string(),
            artist_name: "Outside".to_string(),
            genre_tags_json: None,
            duration_ms: Some(200_000),
            expires_at: "2026-02-01 00:00:00".to_string(),
        },
    )
    .expect("candidate");

    upsert_external_candidate_sighting(
        &conn,
        &ExternalCandidateSightingUpsert {
            candidate_id: candidate.id,
            seed_track_id: 1,
            source: "lastfm_similar".to_string(),
            source_payload_json: Some(r#"{"match":0.9}"#.to_string()),
            similarity: Some(0.9),
            expires_at: "2026-02-01 00:00:00".to_string(),
        },
    )
    .expect("insert sighting");
    upsert_external_candidate_sighting(
        &conn,
        &ExternalCandidateSightingUpsert {
            candidate_id: candidate.id,
            seed_track_id: 1,
            source: "lastfm_similar".to_string(),
            source_payload_json: Some(r#"{"match":0.95}"#.to_string()),
            similarity: Some(0.95),
            expires_at: "2026-02-02 00:00:00".to_string(),
        },
    )
    .expect("update sighting");
    let sighting_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidate_sightings",
            [],
            |row| row.get(0),
        )
        .expect("count sightings");
    assert_eq!(sighting_count, 1);

    replace_external_candidate_neighbors(
        &conn,
        model.id,
        1,
        &[ExternalCandidateNeighborWriteRow {
            candidate_id: candidate.id,
            rank: 1,
            score: 0.91,
            audio_score: 0.8,
            metadata_score: 0.11,
            reason_json: Some(r#"[{"key":"lastfm_similar"}]"#.to_string()),
        }],
    )
    .expect("write neighbor");
    replace_external_candidate_neighbors(&conn, model.id, 1, &[]).expect("remove stale neighbors");
    let neighbor_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidate_neighbors",
            [],
            |row| row.get(0),
        )
        .expect("count neighbors");
    assert_eq!(neighbor_count, 0);
}

#[test]
fn external_candidate_merge_moves_sidecar_rows_before_deleting_loser() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id)
         VALUES (1, 'Seed', 1, 701)",
        [],
    )
    .expect("seed track");
    let model = create_embedding_model(
        &conn,
        "discovery-fusion-v2:external-merge",
        "discovery-fusion-v2",
        2,
        "ready",
        None,
    )
    .expect("create model");
    let winner = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: Some(9100),
            mbid: None,
            dedupe_key: "tidal:9100".to_string(),
            title: "Winner".to_string(),
            artist_name: "Outside".to_string(),
            genre_tags_json: None,
            duration_ms: Some(100_000),
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .expect("winner");
    let loser = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "fallback:winner".to_string(),
            title: "Winner".to_string(),
            artist_name: "Outside".to_string(),
            genre_tags_json: None,
            duration_ms: Some(100_000),
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .expect("loser");
    upsert_external_candidate_sighting(
        &conn,
        &ExternalCandidateSightingUpsert {
            candidate_id: loser.id,
            seed_track_id: 1,
            source: "lastfm_similar".to_string(),
            source_payload_json: None,
            similarity: Some(0.8),
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .expect("sighting");
    let feature_blob = [1_u8, 2];
    conn.execute(
        "INSERT INTO external_track_candidate_audio_features
         (candidate_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms)
         VALUES (?1, 'v', ?2, 0, 1)",
        params![loser.id, &feature_blob[..]],
    )
    .expect("feature");
    let embedding_blob = [3_u8, 4];
    conn.execute(
        "INSERT INTO external_track_candidate_embeddings
         (candidate_id, model_id, vector_blob, l2_norm)
         VALUES (?1, ?2, ?3, 1.0)",
        params![loser.id, model.id, &embedding_blob[..]],
    )
    .expect("embedding");
    replace_external_candidate_neighbors(
        &conn,
        model.id,
        1,
        &[ExternalCandidateNeighborWriteRow {
            candidate_id: loser.id,
            rank: 1,
            score: 0.7,
            audio_score: 0.7,
            metadata_score: 0.0,
            reason_json: None,
        }],
    )
    .expect("neighbor");

    merge_external_track_candidates(&conn, winner.id, loser.id).expect("merge");

    let loser_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidates WHERE id = ?1",
            params![loser.id],
            |row| row.get(0),
        )
        .expect("loser count");
    let moved_sightings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidate_sightings WHERE candidate_id = ?1",
            params![winner.id],
            |row| row.get(0),
        )
        .expect("sighting count");
    let moved_features: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidate_audio_features WHERE candidate_id = ?1",
            params![winner.id],
            |row| row.get(0),
        )
        .expect("feature count");
    let moved_embeddings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidate_embeddings WHERE candidate_id = ?1",
            params![winner.id],
            |row| row.get(0),
        )
        .expect("embedding count");
    let moved_neighbors: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_track_candidate_neighbors WHERE candidate_id = ?1",
            params![winner.id],
            |row| row.get(0),
        )
        .expect("neighbor count");
    assert_eq!(loser_count, 0);
    assert_eq!(moved_sightings, 1);
    assert_eq!(moved_features, 1);
    assert_eq!(moved_embeddings, 1);
    assert_eq!(moved_neighbors, 1);
}

#[test]
fn external_candidates_for_training_skip_expired_and_resolved_rows() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id)
         VALUES (1, 'Resolved Track', 1, 801)",
        [],
    )
    .expect("seed resolved track");
    for (key, title, expires_at, resolved_track_id) in [
        ("fresh", "Fresh", "2026-03-01 00:00:00", None),
        ("expired", "Expired", "2026-01-01 00:00:00", None),
        ("resolved", "Resolved", "2026-03-01 00:00:00", Some(1)),
    ] {
        conn.execute(
            "INSERT INTO external_track_candidates
             (dedupe_key, title, artist_name, expires_at, resolved_track_id)
             VALUES (?1, ?2, 'Outside', ?3, ?4)",
            params![key, title, expires_at, resolved_track_id],
        )
        .expect("seed candidate");
    }
    conn.execute(
        "INSERT INTO external_track_candidate_sightings
         (candidate_id, seed_track_id, source, expires_at)
         SELECT id, 1, 'lastfm_similar', '2026-03-01 00:00:00'
         FROM external_track_candidates
         WHERE dedupe_key = 'fresh'",
        [],
    )
    .expect("seed sighting");

    let rows = get_external_track_candidates_for_training(&conn, "2026-02-01 00:00:00", 10)
        .expect("training candidates");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].dedupe_key, "fresh");
    assert_eq!(
        rows[0].source_tags_json.as_deref(),
        Some(r#"["lastfm_direct","lastfm_similar"]"#)
    );
}

#[test]
fn external_training_tags_derive_lastfm_branch_from_cached_payload() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id) VALUES (1, 'Seed', 1)",
        [],
    )
    .expect("seed track");
    let candidate = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "branch-candidate".to_string(),
            title: "Branch".to_string(),
            artist_name: "Outside".to_string(),
            genre_tags_json: None,
            duration_ms: Some(180_000),
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .expect("candidate");
    upsert_external_candidate_sighting(
        &conn,
        &ExternalCandidateSightingUpsert {
            candidate_id: candidate.id,
            seed_track_id: 1,
            source: "lastfm_similar".to_string(),
            source_payload_json: Some(
                r#"{"match":0.42,"branch_from":"Parent Artist - Parent Track"}"#.to_string(),
            ),
            similarity: Some(0.42),
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .expect("sighting");

    let rows = get_external_track_candidates_for_training(&conn, "2026-02-01 00:00:00", 10)
        .expect("training candidates");

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].source_tags_json.as_deref(),
        Some(r#"["lastfm_branch","lastfm_similar"]"#)
    );
}

#[test]
fn resolved_lastfm_sightings_for_training_include_cached_payload() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id)
         VALUES (1, 'Seed', 1), (2, 'Resolved', 1)",
        [],
    )
    .expect("seed tracks");
    let candidate = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: Some(9002),
            mbid: None,
            dedupe_key: "resolved-lastfm".to_string(),
            title: "Resolved".to_string(),
            artist_name: "Artist".to_string(),
            genre_tags_json: None,
            duration_ms: Some(180_000),
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .expect("candidate");
    mark_external_candidate_resolved(&conn, Some(9002), "Resolved", "Artist", 2)
        .expect("mark resolved");
    upsert_external_candidate_sighting(
        &conn,
        &ExternalCandidateSightingUpsert {
            candidate_id: candidate.id,
            seed_track_id: 1,
            source: "lastfm_similar".to_string(),
            source_payload_json: Some(r#"{"match":0.77}"#.to_string()),
            similarity: Some(0.77),
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .expect("sighting");

    let rows =
        get_resolved_lastfm_external_sightings_for_training(&conn, "2026-02-01 00:00:00", 10)
            .expect("resolved sightings");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seed_track_id, 1);
    assert_eq!(rows[0].resolved_track_id, 2);
    assert_eq!(rows[0].similarity, 0.77);
    assert_eq!(
        rows[0].source_payload_json.as_deref(),
        Some(r#"{"match":0.77}"#)
    );
}

#[test]
fn sighted_external_candidates_follow_lastfm_links_with_a_floor() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id) VALUES (1, 'Seed', 1), (2, 'Neighbor', 1), (3, 'Elsewhere', 1)",
        [],
    )
    .expect("tracks");
    let candidate = |tidal_id: Option<i64>, title: &str| {
        upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id,
                mbid: None,
                dedupe_key: format!("test:{title}"),
                title: title.to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(200_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )
        .expect("candidate")
        .id
    };
    let direct = candidate(Some(501), "Direct");
    let via_neighbor = candidate(Some(502), "Via Neighbor");
    let weak = candidate(Some(503), "Weak");
    let unplayable = candidate(None, "Unplayable");
    let unrelated = candidate(Some(505), "Unrelated");
    for (candidate_id, seed, similarity) in [
        (direct, 1, 0.6),
        (via_neighbor, 2, 0.5),
        (weak, 1, 0.05),
        (unplayable, 1, 0.9),
        (unrelated, 3, 0.9),
    ] {
        upsert_external_candidate_sighting(
            &conn,
            &ExternalCandidateSightingUpsert {
                candidate_id,
                seed_track_id: seed,
                source: "lastfm_similar".to_string(),
                source_payload_json: None,
                similarity: Some(similarity),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )
        .expect("sighting");
    }

    let rows =
        get_sighted_external_candidates(&conn, &[(1, 1.0), (2, 0.65)], 0.15, 10).expect("sighted");

    assert_eq!(
        rows.iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Direct", "Via Neighbor"]
    );
    assert!((rows[1].score - 0.325).abs() < 1e-9);
}

#[test]
fn external_neighbor_lookup_returns_only_tidal_resolved_candidates_by_default() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id)
         VALUES (1, 'Seed', 1, 901)",
        [],
    )
    .expect("seed track");
    let model = create_embedding_model(
        &conn,
        "discovery-fusion-v2:external-read",
        "discovery-fusion-v2",
        2,
        "ready",
        None,
    )
    .expect("create model");
    let unresolved = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "unresolved-read".to_string(),
            title: "Unresolved".to_string(),
            artist_name: "Outside".to_string(),
            genre_tags_json: None,
            duration_ms: Some(100_000),
            expires_at: "2099-01-01 00:00:00".to_string(),
        },
    )
    .expect("unresolved");
    let resolved = upsert_external_track_candidate(
        &conn,
        &ExternalTrackCandidateUpsert {
            tidal_id: Some(9901),
            mbid: None,
            dedupe_key: "tidal:9901".to_string(),
            title: "Resolved".to_string(),
            artist_name: "Outside".to_string(),
            genre_tags_json: None,
            duration_ms: Some(100_000),
            expires_at: "2099-01-01 00:00:00".to_string(),
        },
    )
    .expect("resolved");
    replace_external_candidate_neighbors(
        &conn,
        model.id,
        1,
        &[
            ExternalCandidateNeighborWriteRow {
                candidate_id: unresolved.id,
                rank: 1,
                score: 0.95,
                audio_score: 0.95,
                metadata_score: 0.0,
                reason_json: None,
            },
            ExternalCandidateNeighborWriteRow {
                candidate_id: resolved.id,
                rank: 2,
                score: 0.9,
                audio_score: 0.9,
                metadata_score: 0.0,
                reason_json: None,
            },
        ],
    )
    .expect("write neighbors");

    let rows = get_external_candidate_neighbors(&conn, model.id, 1, 10, true)
        .expect("read external neighbors");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].candidate_id, resolved.id);
    assert_eq!(rows[0].tidal_id, Some(9901));
}

#[test]
fn tidal_resolution_candidate_lookup_prioritizes_sightings_similarity_and_cap() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id)
         VALUES (1, 'Seed A', 1, 901), (2, 'Seed B', 1, 902)",
        [],
    )
    .expect("seed tracks");
    for (key, title) in [
        ("one-sighting", "One"),
        ("two-sightings", "Two"),
        ("resolved", "Resolved"),
        ("already-tidal", "Already Tidal"),
    ] {
        conn.execute(
            "INSERT INTO external_track_candidates
             (dedupe_key, title, artist_name, expires_at, tidal_id)
             VALUES (?1, ?2, 'Outside', '2099-01-01 00:00:00',
                     CASE WHEN ?1 = 'already-tidal' THEN 9901 ELSE NULL END)",
            params![key, title],
        )
        .expect("candidate");
    }
    conn.execute(
        "UPDATE external_track_candidates SET resolved_track_id = 1 WHERE dedupe_key = 'resolved'",
        [],
    )
    .expect("mark resolved");
    conn.execute(
        "INSERT INTO external_track_candidate_sightings
         (candidate_id, seed_track_id, source, similarity, expires_at)
         SELECT id, 1, 'lastfm_similar', 0.60, '2099-01-01 00:00:00'
         FROM external_track_candidates WHERE dedupe_key = 'one-sighting'",
        [],
    )
    .expect("one sighting");
    conn.execute(
        "INSERT INTO external_track_candidate_sightings
         (candidate_id, seed_track_id, source, similarity, expires_at)
         SELECT id, 1, 'lastfm_similar', 0.70, '2099-01-01 00:00:00'
         FROM external_track_candidates WHERE dedupe_key = 'two-sightings'",
        [],
    )
    .expect("two sighting a");
    conn.execute(
        "INSERT INTO external_track_candidate_sightings
         (candidate_id, seed_track_id, source, similarity, expires_at)
         SELECT id, 2, 'lastfm_similar', 0.90, '2099-01-01 00:00:00'
         FROM external_track_candidates WHERE dedupe_key = 'two-sightings'",
        [],
    )
    .expect("two sighting b");

    let rows = get_unresolved_lastfm_external_candidates_for_tidal_resolution(
        &conn,
        "2026-02-01 00:00:00",
        1,
    )
    .expect("resolution candidates");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Two");
    assert_eq!(rows[0].sighting_count, 2);
    assert_eq!(rows[0].max_similarity, Some(0.90));
}

#[test]
fn onboarding_flag_present_returns_true_without_tidal() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    set_onboarding_complete(&conn).expect("set flag");

    assert!(get_onboarding_complete(&conn).expect("read flag"));
    assert_eq!(read_onboarding_value(&conn).as_deref(), Some("1"));
}

#[test]
fn discovery_presets_round_trip_mode() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    let created = create_discovery_preset(
        &conn,
        "After Hours",
        "glassy synths",
        "reference",
        r#"["tidal","soundcloud"]"#,
    )
    .expect("preset created");

    assert_eq!(created.mode, "reference");

    let presets = list_discovery_presets(&conn).expect("preset list");
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].mode, "reference");
    assert_eq!(presets[0].services, vec!["tidal", "soundcloud"]);
}

#[test]
fn search_matches_artist_names_for_tracks_and_albums() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'The Cure')", [])
        .expect("artist inserted");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, is_favorite, source) VALUES (1, 'Disintegration', 1, 1, 'tidal')",
        [],
    )
    .expect("album inserted");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source
        ) VALUES (1, 'Pictures of You', 1, 1, 420000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
        [],
    )
    .expect("track inserted");

    let results = search(&conn, "the cure", 10).expect("search results");

    assert_eq!(results.artists.len(), 1);
    assert_eq!(results.artists[0].name, "The Cure");
    assert_eq!(results.albums.len(), 1);
    assert_eq!(results.albums[0].title, "Disintegration");
    assert_eq!(results.tracks.len(), 1);
    assert_eq!(results.tracks[0].title, "Pictures of You");
}

#[test]
fn to_fts_query_treats_apostrophe_as_separator() {
    // A bare apostrophe is a string-literal opener in FTS5 and used to throw
    // "fts5: syntax error", forcing the full-table LIKE fallback. It must be
    // mapped to a separator so the query parses.
    assert_eq!(to_fts_query("don't"), "don* t*");
    assert_eq!(to_fts_query("Guns N' Roses"), "Guns* N* Roses*");
    assert!(!to_fts_query("rock 'n' roll").contains('\''));
}

#[test]
fn search_handles_apostrophe_queries_via_fts() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute(
        "INSERT INTO artists (id, name) VALUES (1, 'Guns N Roses')",
        [],
    )
    .expect("artist inserted");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source
        ) VALUES (1, 'Don''t Cry', 1, 300000, 201, 'LOSSLESS', 'tidal', 10, 0, 'tidal')",
        [],
    )
    .expect("track inserted");

    // The unicode61 tokenizer splits "Don't" into "don" + "t", so an
    // apostrophe query must still find it. Before the fix this errored and
    // fell back to a full-table LIKE scan.
    let results = search(&conn, "don't", 10).expect("apostrophe search must not error");
    assert!(
        results.tracks.iter().any(|t| t.title == "Don't Cry"),
        "expected to find \"Don't Cry\", got {:?}",
        results.tracks.iter().map(|t| &t.title).collect::<Vec<_>>()
    );
}

#[test]
fn genre_heat_rolls_descendant_listens_up_to_ancestors() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES
            (1, 'Electronic', 'electronic', NULL),
            (2, 'Drum and Bass', 'drum-and-bass', 1)",
        [],
    )
    .expect("genres inserted");
    conn.execute(
        "INSERT INTO artists (id, name) VALUES (1, 'Rufige Kru')",
        [],
    )
    .expect("artist inserted");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, duration_ms, tidal_id, best_quality, best_source, fidelity_score, is_favorite, source
        ) VALUES (1, 'Terminator', 1, 360000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
        [],
    )
    .expect("track inserted");
    conn.execute(
        "INSERT INTO track_genres (track_id, genre_id, source, confidence)
         VALUES (1, 2, 'musicbrainz', 1.0)",
        [],
    )
    .expect("track genre inserted");
    conn.execute(
        "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
         VALUES (1, datetime('now', '-10 days'), 120000, 1)",
        [],
    )
    .expect("listen inserted");

    let heat = get_genre_heat_filtered(&conn, 90, crate::genre::filter::GalaxyFilterRule::All)
        .expect("genre heat");
    let electronic = heat
        .iter()
        .find(|entry| entry.genre_id == 1)
        .expect("electronic heat");
    let dnb = heat
        .iter()
        .find(|entry| entry.genre_id == 2)
        .expect("dnb heat");

    assert_eq!(electronic.listen_count, 1);
    assert_eq!(electronic.total_listened_ms, 120000);
    assert_eq!(dnb.listen_count, 1);
    assert_eq!(dnb.total_listened_ms, 120000);
}

#[test]
fn genre_heat_returns_zero_rows_for_cold_genres() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES
            (1, 'Electronic', 'electronic', NULL),
            (2, 'Ambient', 'ambient', 1)",
        [],
    )
    .expect("genres inserted");

    let heat = get_genre_heat_filtered(&conn, 90, crate::genre::filter::GalaxyFilterRule::All)
        .expect("genre heat");
    assert_eq!(heat.len(), 2);
    assert!(heat.iter().all(|entry| entry.listen_count == 0));
    assert!(heat.iter().all(|entry| entry.total_listened_ms == 0));
}

#[test]
fn test_add_tracks_to_playlist_deduplicates() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute_batch(
        r#"
        INSERT INTO artists (id, name) VALUES (1, 'Test Artist');
        INSERT INTO albums (id, title, artist_id) VALUES (1, 'Test Album', 1);
        INSERT INTO tracks (id, title, artist_id, album_id) VALUES (1, 'Track A', 1, 1);
        INSERT INTO tracks (id, title, artist_id, album_id) VALUES (2, 'Track B', 1, 1);
        INSERT INTO tracks (id, title, artist_id, album_id) VALUES (3, 'Track C', 1, 1);
        INSERT INTO playlists (id, name, is_smart, is_synced) VALUES (1, 'My Playlist', 0, 1);
    "#,
    )
    .unwrap();

    // First call adds both tracks
    let added = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
    assert_eq!(added, 2);

    // Second call with same tracks returns 0 (already present)
    let added_again = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
    assert_eq!(added_again, 0);

    // Duplicate IDs within a single call: [1, 1] — track 1 already present, so 0 added
    let added_dup = add_tracks_to_playlist(&conn, 1, &[1, 1]).unwrap();
    assert_eq!(added_dup, 0);

    // Mixed: [1, 3] — track 1 already present, track 3 is new → 1 added
    let added_mixed = add_tracks_to_playlist(&conn, 1, &[1, 3]).unwrap();
    assert_eq!(added_mixed, 1);

    // [3, 3] — track 3 now present, duplicate in input → 0 added
    let added_dup_present = add_tracks_to_playlist(&conn, 1, &[3, 3]).unwrap();
    assert_eq!(added_dup_present, 0);
}

// ─── liked_only vs favorite_only regression ───────────────────────────
//
// Bug being guarded: favorite_only=true used to silently mean "library tracks"
// (liked tracks ∪ tracks from favorited albums), so saved-album tracks leaked
// into what the UI presented as "liked". liked_only must be strict.
fn seed_album_with_one_liked_track(conn: &Connection) {
    conn.execute(
        "INSERT INTO artists (id, name) VALUES (1, 'Brooks & Dunn')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, is_favorite, source)
         VALUES (1, '#1s ... and then some', 1, 1, 'tidal')",
        [],
    )
    .unwrap();
    // Three tracks in the favorited album; only "Neon Blue" has tracks.is_favorite = 1.
    // All three are is_library = 1 - the pre-rework sync shape (album
    // hydration used to mark every synced track as library; new syncs
    // write album fill as is_library = 0 background instead). The
    // predicate itself is unchanged, so these legacy rows still count as
    // library tracks until the user runs Reclean.
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source, is_library)
         VALUES (1, 'Neon Blue', 1, 1, 200000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source, is_library)
         VALUES (2, 'Brand New Man', 1, 1, 180000, 102, 'LOSSLESS', 'tidal', 10, 0, 'tidal', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source, is_library)
         VALUES (3, 'Boot Scootin Boogie', 1, 1, 198000, 103, 'LOSSLESS', 'tidal', 10, 0, 'tidal', 1)",
        [],
    ).unwrap();
}

#[test]
fn liked_only_excludes_album_favorited_tracks() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);

    let tracks = get_tracks(&conn, "title", "asc", 100, 0, false, true).expect("liked tracks");
    assert_eq!(
        tracks.len(),
        1,
        "liked_only must return only truly-liked tracks"
    );
    assert_eq!(tracks[0].title, "Neon Blue");

    let count = get_track_count(&conn, false, true).expect("liked count");
    assert_eq!(count, 1, "count must match liked-only data query");
}

#[test]
fn favorite_only_preserves_legacy_union_behavior() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);

    let tracks = get_tracks(&conn, "title", "asc", 100, 0, true, false).expect("library tracks");
    assert_eq!(
        tracks.len(),
        3,
        "favorite_only must keep returning all tracks from favorited albums"
    );

    let count = get_track_count(&conn, true, false).expect("library count");
    assert_eq!(count, 3, "count must match favorite_only data query");
}

// Regression for the resolver/discovery leak: a transient import
// (is_library = 0) that attaches to a favorited album by tidal_id must NOT
// surface in the library, while genuine is_library = 1 siblings still do.
#[test]
fn favorite_only_hides_transient_import_in_favorited_album() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);

    // A discovery/resolver track injected into the same favorited album:
    // not liked, not library (the "House Work" shape).
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source, is_library)
         VALUES (4, 'Injected Leak', 1, 1, 157000, 104, 'LOSSLESS', 'tidal', 10, 0, 'tidal', 0)",
        [],
    )
    .unwrap();
    // A resolver lazy-import (tidal_stream) that also landed here, unliked.
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source, is_library)
         VALUES (5, 'Stream Leak', 1, 1, 160000, 105, 'LOSSLESS', 'tidal', 10, 0, 'tidal_stream', 0)",
        [],
    )
    .unwrap();

    let tracks = get_tracks(&conn, "title", "asc", 100, 0, true, false).expect("library tracks");
    let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
    assert!(
        !titles.contains(&"Injected Leak") && !titles.contains(&"Stream Leak"),
        "transient is_library=0 tracks must stay out of the library, got {titles:?}"
    );
    assert_eq!(tracks.len(), 3, "only the 3 genuine library tracks remain");

    let count = get_track_count(&conn, true, false).expect("library count");
    assert_eq!(count, 3, "count must exclude the transient imports");

    // An explicit like on the injected track promotes it back in.
    conn.execute(
        "UPDATE tracks SET is_favorite = 1, is_library = 1 WHERE id = 4",
        [],
    )
    .unwrap();
    let after = get_track_count(&conn, true, false).expect("library count");
    assert_eq!(
        after, 4,
        "explicit like promotes a transient track to library"
    );
}

// Genre Galaxy gate: hidden enrichment fill (is_library = 0) must not
// inflate galaxy counts or track lists - the galaxy mirrors the curated
// Library predicate exactly (ARTIST_LIBRARY_TRACK_WHERE).
#[test]
fn galaxy_queries_hide_background_enrichment_fill() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);

    // Background enrichment fill on the favorited album: in the DB for
    // discovery, invisible in Library and Galaxy.
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source, is_library)
         VALUES (4, 'Hidden Fill', 1, 1, 210000, 106, 'LOSSLESS', 'tidal', 10, 0, 'tidal', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES (1, 'Country', 'country', NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
            (1, 1, 'musicbrainz', 0.9),
            (2, 1, 'musicbrainz', 0.9),
            (4, 1, 'musicbrainz', 0.9)",
        [],
    )
    .unwrap();

    let genres = get_genres_filtered(&conn, crate::genre::filter::GalaxyFilterRule::All)
        .expect("galaxy genres");
    let country = genres
        .iter()
        .find(|g| g.name == "Country")
        .expect("country node");
    assert_eq!(
        country.track_count,
        Some(2),
        "galaxy counts must cover library tracks only, not hidden fill"
    );

    let tracks =
        get_tracks_by_genre_filtered(&conn, 1, false, crate::genre::filter::GalaxyFilterRule::All)
            .expect("genre tracks");
    let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
    assert!(
        !titles.contains(&"Hidden Fill"),
        "hidden fill must not surface in galaxy track lists, got {titles:?}"
    );
    assert_eq!(tracks.len(), 2, "the two library tracks remain");

    // Liking the hidden row promotes it into the galaxy - the gate is the
    // Library predicate, not a source check.
    conn.execute(
        "UPDATE tracks SET is_favorite = 1, is_library = 1 WHERE id = 4",
        [],
    )
    .unwrap();
    let genres = get_genres_filtered(&conn, crate::genre::filter::GalaxyFilterRule::All)
        .expect("galaxy genres after like");
    let country = genres
        .iter()
        .find(|g| g.name == "Country")
        .expect("country node");
    assert_eq!(country.track_count, Some(3));
}

#[test]
fn genre_tree_counts_distinct_library_tracks_per_subtree() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);
    conn.execute(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES
            (1, 'Electronic', 'electronic', NULL),
            (2, 'House', 'house', 1),
            (3, 'Deep House', 'deep-house', 2)",
        [],
    )
    .unwrap();
    // Track 1 is tagged at two levels of one branch; it must count once.
    conn.execute(
        "INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
            (1, 2, 'musicbrainz', 0.9),
            (1, 3, 'musicbrainz', 0.9),
            (2, 3, 'musicbrainz', 0.9),
            (3, 1, 'musicbrainz', 0.9)",
        [],
    )
    .unwrap();

    let tree = get_genre_tree_filtered(&conn, crate::genre::filter::GalaxyFilterRule::All)
        .expect("genre tree");
    let electronic = tree.iter().find(|g| g.id == 1).expect("electronic root");
    let house = electronic
        .children
        .iter()
        .find(|g| g.id == 2)
        .expect("house");
    let deep = house
        .children
        .iter()
        .find(|g| g.id == 3)
        .expect("deep house");

    assert_eq!(deep.track_count, Some(2));
    assert_eq!(
        house.track_count,
        Some(2),
        "track 1 tagged twice counts once"
    );
    assert_eq!(electronic.track_count, Some(3));

    // The node count must equal what the genre page lists.
    let listed =
        get_tracks_by_genre_filtered(&conn, 1, true, crate::genre::filter::GalaxyFilterRule::All)
            .expect("electronic tracks");
    assert_eq!(electronic.track_count, Some(listed.len() as i64));
}

#[test]
fn date_added_desc_uses_newest_row_as_tiebreaker() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("seed artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source, date_added)
         VALUES (1, 'First', 1, 200000, 201, 'LOSSLESS', 'tidal', 10, 1, 'tidal', '2026-05-01T00:00:00Z'),
                (2, 'Second', 1, 200000, 202, 'LOSSLESS', 'tidal', 10, 1, 'tidal', '2026-05-01T00:00:00Z'),
                (3, 'Third', 1, 200000, 203, 'LOSSLESS', 'tidal', 10, 1, 'tidal', '2026-05-01T00:00:00Z')",
        [],
    )
    .expect("seed tracks");

    let tracks =
        get_tracks(&conn, "date_added", "desc", 100, 0, true, false).expect("date sorted tracks");

    assert_eq!(
        tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

#[test]
fn saved_date_order_compares_offsets_and_legacy_sql_dates() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE tracks(id INTEGER,date_added TEXT);
        INSERT INTO tracks VALUES(1,'2020-01-01T01:00:00+1000'),(2,'2019-12-31 16:00:00'),(3,'2019-12-31T15:30:00Z'),(4,'invalid');").unwrap();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT t.id FROM tracks t ORDER BY {}",
            saved_date_order("DESC")
        ))
        .unwrap();
    let ids = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(ids, vec![2, 3, 1, 4]);
}

#[test]
fn date_added_order_clause_has_explicit_id_tiebreaker() {
    assert_eq!(
        track_order_clause("date_added", "desc"),
        saved_date_order("DESC")
    );
    assert_eq!(
        track_order_clause("date_added", "asc"),
        saved_date_order("ASC")
    );
}

#[test]
fn random_order_clause_ignores_direction() {
    // Shuffle relies on this: the library Shuffle button sends sort_by=random
    // so the queue is a fresh random slice of the whole library, not the
    // newest-N prefix reshuffled.
    assert_eq!(track_order_clause("random", "desc"), "RANDOM()");
    assert_eq!(track_order_clause("random", "asc"), "RANDOM()");
}

#[test]
fn artist_library_tracks_and_counts_use_library_union_behavior() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, is_favorite, source)
         VALUES (2, 'Not Saved', 1, 0, 'tidal')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                              best_quality, best_source, fidelity_score, is_favorite, source)
         VALUES (4, 'Cache Only', 1, 2, 199000, 104, 'LOSSLESS', 'tidal', 10, 0, 'tidal')",
        [],
    )
    .unwrap();

    let all_tracks = get_artist_tracks(&conn, 1).expect("all artist tracks");
    assert_eq!(all_tracks.len(), 4);

    let library_tracks = get_artist_library_tracks(&conn, 1).expect("library artist tracks");
    assert_eq!(library_tracks.len(), 3);

    let (_, track_count, album_count) = get_artist_with_counts(&conn, 1)
        .expect("artist counts")
        .expect("artist exists");
    assert_eq!(track_count, 3);
    assert_eq!(album_count, 1);
}

#[test]
fn liked_only_takes_precedence_over_favorite_only() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);

    let tracks = get_tracks(&conn, "title", "asc", 100, 0, true, true).expect("strict tracks");
    assert_eq!(tracks.len(), 1, "liked_only must override favorite_only");
    assert_eq!(tracks[0].title, "Neon Blue");

    let count = get_track_count(&conn, true, true).expect("strict count");
    assert_eq!(count, 1);
}

#[test]
fn tidal_track_library_states_include_liked_flags() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);

    let states = get_tidal_track_library_states(&conn, &[101, 102, 999]).expect("tidal states");

    assert_eq!(states.len(), 2);
    assert_eq!(states.get(&101).map(|s| s.local_id), Some(1));
    assert_eq!(states.get(&101).map(|s| s.is_favorite), Some(true));
    assert_eq!(states.get(&102).map(|s| s.local_id), Some(2));
    assert_eq!(states.get(&102).map(|s| s.is_favorite), Some(false));
    assert!(!states.contains_key(&999));
}

#[test]
fn no_filter_returns_everything() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_album_with_one_liked_track(&conn);

    let tracks = get_tracks(&conn, "title", "asc", 100, 0, false, false).expect("all tracks");
    assert_eq!(tracks.len(), 3);

    let count = get_track_count(&conn, false, false).expect("all count");
    assert_eq!(count, 3);
}

// ─── FTS-first library search tests ──────────────────────────────────────

#[test]
fn library_search_multi_token_and_within_column_non_contiguous() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    // FTS5 AND-prefix semantics: tokens must all appear in the same indexed
    // column, but NOT necessarily contiguously and NOT necessarily in order.
    //   1001 — title "The Long Strokes": both tokens present, non-contiguous.
    //          (Today's LIKE on "the strokes" would MISS this — substring fail.)
    //   1002 — title "The Anthem": only "the". Missing "strokes". Should NOT match.
    //   1003 — title "Strokes": only "strokes". Missing "the". Should NOT match.
    conn.execute("INSERT INTO artists (id, name) VALUES (1001, 'Test')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (1001, 'Plain', 1001, 'tidal')",
        [],
    )
    .expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (1001, 'The Long Strokes', 1001, 1001, 200000, 1001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (1002, 'The Anthem',       1001, 1001, 200000, 1002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (1003, 'Strokes',          1001, 1001, 200000, 1003, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
        [],
    )
    .expect("tracks");

    let results = search_with_audio_filters(&conn, "the strokes", &AudioFilters::default(), 50, 0)
        .expect("library search");

    let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![1001],
        "expected only 1001 (both tokens in title, non-contiguous); got {ids:?}"
    );
}

#[test]
fn library_search_returns_track_when_album_title_matches() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute(
        "INSERT INTO artists (id, name) VALUES (2001, 'Frank Ocean')",
        [],
    )
    .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (2001, 'Blonde', 2001, 'tidal')",
        [],
    )
    .expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES (2001, 'Pink + White', 2001, 2001, 200000, 2001, 'LOSSLESS', 'tidal', 8, 0, 'tidal', 0)",
        [],
    )
    .expect("track");

    let results = search_with_audio_filters(&conn, "blonde", &AudioFilters::default(), 50, 0)
        .expect("search");
    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
    assert!(
        titles.contains(&"Pink + White"),
        "expected 'Pink + White' (album 'Blonde' matches); got {titles:?}"
    );
}

#[test]
fn library_search_audio_filter_composes_with_text() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute(
        "INSERT INTO artists (id, name) VALUES (3001, 'Miles Davis')",
        [],
    )
    .expect("artist");
    conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (3001, 'Kind of Blue', 3001, 'tidal')", []).expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (3001, 'So What (fast)', 3001, 3001, 540000, 3001, 'LOSSLESS', 'tidal', 9, 0, 'tidal', 0),
            (3002, 'Blue in Green',  3001, 3001, 330000, 3002, 'LOSSLESS', 'tidal', 9, 0, 'tidal', 0)",
        [],
    )
    .expect("tracks");
    conn.execute(
        "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (3001, 120.0), (3002, 80.0)",
        [],
    )
    .expect("dsp features");

    let filters = AudioFilters {
        bpm_min: Some(100.0),
        ..Default::default()
    };

    let results = search_with_audio_filters(&conn, "miles", &filters, 50, 0).expect("search");
    let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![3001],
        "expected only the 120-BPM track; got {ids:?}"
    );
}

#[test]
fn library_search_empty_query_with_filters_returns_filtered_set() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (4001, 'Test')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (4001, 'A', 4001, 'tidal')",
        [],
    )
    .expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (4001, 'Fast', 4001, 4001, 200000, 4001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (4002, 'Slow', 4001, 4001, 200000, 4002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
        [],
    )
    .expect("tracks");
    conn.execute(
        "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (4001, 130.0), (4002, 70.0)",
        [],
    )
    .expect("dsp");

    let filters = AudioFilters {
        bpm_min: Some(120.0),
        ..Default::default()
    };

    let results = search_with_audio_filters(&conn, "", &filters, 50, 0).expect("search");
    let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![4001]);
}

#[test]
fn shuffled_audio_search_covers_full_matching_set() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (4101, 'Test')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (4101, 'A', 4101, 'tidal')",
        [],
    )
    .expect("album");
    // 8 matching tracks with varied play_count so the deterministic ranking
    // would order them; shuffle must still surface every matching id.
    for i in 0..8i64 {
        let id = 4101 + i;
        conn.execute(
            &format!(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, duration_ms, tidal_id, best_quality,
                    best_source, fidelity_score, is_favorite, source, play_count
                 ) VALUES ({id}, 'T{i}', 4101, 4101, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', {i})"
            ),
            [],
        )
        .expect("track");
        conn.execute(
            &format!("INSERT INTO audio_dsp_features (track_id, bpm) VALUES ({id}, 130.0)"),
            [],
        )
        .expect("dsp");
    }

    let filters = AudioFilters {
        bpm_min: Some(120.0),
        ..Default::default()
    };

    let results =
        search_with_audio_filters_shuffled(&conn, "", &filters, 200).expect("shuffle search");
    let mut ids: Vec<i64> = results.iter().map(|r| r.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, (4101..4109).collect::<Vec<_>>());
}

#[test]
fn shuffled_audio_search_respects_filters_and_limit() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (4201, 'Test')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (4201, 'A', 4201, 'tidal')",
        [],
    )
    .expect("album");
    // Two fast (matching) tracks, two slow (non-matching).
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (4201, 'Fast A', 4201, 4201, 200000, 4201, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (4202, 'Fast B', 4201, 4201, 200000, 4202, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (4203, 'Slow A', 4201, 4201, 200000, 4203, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (4204, 'Slow B', 4201, 4201, 200000, 4204, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
        [],
    )
    .expect("tracks");
    conn.execute(
        "INSERT INTO audio_dsp_features (track_id, bpm) VALUES
            (4201, 140.0), (4202, 135.0), (4203, 70.0), (4204, 60.0)",
        [],
    )
    .expect("dsp");

    let filters = AudioFilters {
        bpm_min: Some(120.0),
        ..Default::default()
    };

    let results =
        search_with_audio_filters_shuffled(&conn, "", &filters, 1).expect("shuffle search");
    assert_eq!(results.len(), 1, "limit must cap the shuffled sample");
    assert!(
        matches!(results[0].id, 4201 | 4202),
        "only the fast tracks should match, got {}",
        results[0].id
    );
}

#[test]
fn liked_only_audio_search_restricts_to_favorites() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (4301, 'Test')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (4301, 'A', 4301, 'tidal')",
        [],
    )
    .expect("album");
    // Two liked, two not liked; all match the filter.
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (4301, 'Liked A',   4301, 4301, 200000, 4301, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 0),
            (4302, 'Liked B',   4301, 4301, 200000, 4302, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 0),
            (4303, 'Unliked A', 4301, 4301, 200000, 4303, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (4304, 'Unliked B', 4301, 4301, 200000, 4304, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
        [],
    )
    .expect("tracks");

    let filters = AudioFilters {
        liked_only: true,
        ..Default::default()
    };

    let mut ids: Vec<i64> = search_with_audio_filters_shuffled(&conn, "", &filters, 200)
        .expect("shuffle search")
        .iter()
        .map(|r| r.id)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![4301, 4302], "liked_only must drop non-favorites");
}

#[test]
fn library_search_empty_query_no_filters_respects_limit() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (5001, 'A')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (5001, 'A', 5001, 'tidal')",
        [],
    )
    .expect("album");
    for i in 0..5 {
        let id = 5001 + i;
        conn.execute(
            &format!(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                    fidelity_score, is_favorite, source, play_count
                 ) VALUES ({id}, 'T{i}', 5001, 5001, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
            ),
            [],
        )
        .expect("track");
    }

    let results =
        search_with_audio_filters(&conn, "", &AudioFilters::default(), 3, 0).expect("search");
    assert_eq!(results.len(), 3, "expected limit=3 to cap results");
}

#[test]
fn library_search_favorites_lead_over_play_count() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute(
        "INSERT INTO artists (id, name) VALUES (6001, 'Miles Davis')",
        [],
    )
    .expect("artist");
    conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (6001, 'Kind of Blue', 6001, 'tidal')", []).expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (6001, 'Miles A', 6001, 6001, 200000, 6001, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 0),
            (6002, 'Miles B', 6001, 6001, 200000, 6002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 1000)",
        [],
    )
    .expect("tracks");

    // Old ordering (play_count DESC, last_played_at DESC) → B leads.
    // New ordering (is_favorite DESC, ...) → A leads.
    let results =
        search_with_audio_filters(&conn, "miles", &AudioFilters::default(), 50, 0).expect("search");
    let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
    assert_eq!(
        ids.first(),
        Some(&6001),
        "favorited track should lead despite zero plays; got {ids:?}"
    );
}

#[test]
fn library_search_non_track_track_type_returns_empty() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (7001, 'Anyone')", [])
        .expect("artist");
    conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (7001, 'Anything', 7001, 'tidal')", []).expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES (7001, 'Anything', 7001, 7001, 200000, 7001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
        [],
    )
    .expect("track");

    let filters = AudioFilters {
        track_type: Some("album".to_string()),
        ..Default::default()
    };

    let results = search_with_audio_filters(&conn, "anything", &filters, 50, 0).expect("search");
    assert!(results.is_empty());
}

#[test]
fn library_search_strips_fts_special_characters() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    // Punctuation in user queries (?, /, -) must not cause FTS to error.
    // to_fts_query strips non-alphanumerics; tokenization happens within a
    // single column, so fixtures keep all match tokens together in one column.
    conn.execute("INSERT INTO artists (id, name) VALUES (8001, 'AC/DC')", [])
        .expect("artist");
    conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (8001, 'AC/DC Live', 8001, 'tidal')", []).expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (8001, 'Thunderstruck', 8001, 8001, 200000, 8001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (8002, 'Love Remix',    8001, 8001, 200000, 8002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
        [],
    )
    .expect("tracks");

    // Query "AC/DC live?" → strips to "AC DC live" → tokens AC, DC, live must
    // all appear in some indexed column. Album "AC/DC Live" tokenizes to
    // ["ac", "dc", "live"] (unicode61 splits on /), satisfying all three.
    let r1 = search_with_audio_filters(&conn, "AC/DC live?", &AudioFilters::default(), 50, 0)
        .expect("'AC/DC live?' must not error");
    assert!(
        r1.iter().any(|r| r.id == 8001),
        "expected Thunderstruck (album 'AC/DC Live' has all tokens); got ids {:?}",
        r1.iter().map(|r| r.id).collect::<Vec<_>>()
    );

    // Query "love - remix" → "love remix" → tokens must both appear in same
    // column. Track 8002's title "Love Remix" satisfies that.
    let r2 = search_with_audio_filters(&conn, "love - remix", &AudioFilters::default(), 50, 0)
        .expect("'love - remix' must not error");
    assert!(
        r2.iter().any(|r| r.id == 8002),
        "expected 'Love Remix' to match; got ids {:?}",
        r2.iter().map(|r| r.id).collect::<Vec<_>>()
    );
}

#[test]
fn global_search_tracks_fts_does_not_error_on_artist_match() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    // Setup designed to exercise the artists_fts UNION arm: track title
    // contains nothing of the query, but the artist name does.
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'The Cure')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source) VALUES (1, 'Disintegration', 1, 'tidal')",
        [],
    )
    .expect("album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES (1, 'Pictures of You', 1, 1, 420000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal', 0)",
        [],
    )
    .expect("track");

    // Calls search_tracks_fts directly so the LIKE fallback in search() can't
    // mask an FTS-side error. If the UNION+ORDER-BY SQL is broken, this errors.
    let tracks = search_tracks_fts(&conn, "the* cure*", 10)
        .expect("search_tracks_fts must run without SQL errors");
    let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
    assert!(
        titles.contains(&"Pictures of You"),
        "FTS path should return the track via artists_fts arm; got {titles:?}"
    );
}

#[test]
fn library_search_limit_is_respected_with_filters() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute(
        "INSERT INTO artists (id, name) VALUES (9101, 'Miles Davis')",
        [],
    )
    .expect("artist");
    conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (9101, 'Kind of Blue', 9101, 'tidal')", []).expect("album");
    for i in 0..3 {
        let id = 9101 + i;
        conn.execute(
            &format!(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                    fidelity_score, is_favorite, source, play_count
                 ) VALUES ({id}, 'Miles {i}', 9101, 9101, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
            ),
            [],
        )
        .expect("track");
    }
    conn.execute(
        "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (9101, 120.0), (9102, 121.0), (9103, 122.0)",
        [],
    )
    .expect("dsp");

    let filters = AudioFilters {
        bpm_min: Some(100.0),
        ..Default::default()
    };

    let results = search_with_audio_filters(&conn, "miles", &filters, 2, 0).expect("search");
    assert_eq!(
        results.len(),
        2,
        "limit=2 with both FTS bind and audio-filter binds; off-by-one would return 0 or 3"
    );
}

#[test]
fn audio_search_offset_pages_and_count_reports_full_set() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (9201, 'Pager')", [])
        .expect("artist");
    for i in 0..5i64 {
        let id = 9201 + i;
        conn.execute(
            &format!(
                "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
                    fidelity_score, is_favorite, source, play_count
                 ) VALUES ({id}, 'P{i}', 9201, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', {pc})",
                pc = 100 - i
            ),
            [],
        )
        .expect("track");
    }

    let filters = AudioFilters::default();
    let total = count_audio_filter_matches(&conn, "", &filters).expect("count");
    assert_eq!(total, 5);

    let page1 = search_with_audio_filters(&conn, "", &filters, 2, 0).expect("page1");
    let page2 = search_with_audio_filters(&conn, "", &filters, 2, 2).expect("page2");
    let page3 = search_with_audio_filters(&conn, "", &filters, 2, 4).expect("page3");
    let ids: Vec<i64> = page1
        .iter()
        .chain(page2.iter())
        .chain(page3.iter())
        .map(|r| r.id)
        .collect();
    // play_count DESC ranking: 9201 (100) .. 9205 (96); pages must not
    // overlap or skip.
    assert_eq!(ids, vec![9201, 9202, 9203, 9204, 9205]);
}

#[test]
fn resolve_genre_tokens_matches_slug_and_name_case_insensitive() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute_batch(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES
            (1, 'Rock', 'rock', NULL),
            (2, 'Hip Hop', 'hip-hop', NULL);",
    )
    .expect("genres");

    let tokens = vec![
        "Rock".to_string(),
        "hip-hop".to_string(),
        "HIP HOP".to_string(),
        "polka".to_string(),
    ];
    let (ids, unmatched) = resolve_genre_tokens(&conn, &tokens).expect("resolve");
    assert_eq!(ids, vec![1, 2, 2]);
    assert_eq!(unmatched, vec!["polka".to_string()]);
}

#[test]
fn expand_genre_descendants_walks_full_subtree() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    conn.execute_batch(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES
            (1, 'Rock', 'rock', NULL),
            (2, 'Alternative Rock', 'alternative-rock', 1),
            (3, 'Indie Rock', 'indie-rock', 2),
            (4, 'Jazz', 'jazz', NULL);",
    )
    .expect("genres");

    let mut expanded = expand_genre_descendants(&conn, &[1]).expect("expand");
    expanded.sort_unstable();
    assert_eq!(expanded, vec![1, 2, 3], "grandchildren must be included");

    // Unknown ids drop out instead of leaking into the SQL filter.
    assert!(
        expand_genre_descendants(&conn, &[999])
            .expect("expand")
            .is_empty()
    );
    assert!(
        expand_genre_descendants(&conn, &[])
            .expect("expand")
            .is_empty()
    );
}

#[test]
fn genre_filter_uses_curated_rowset_not_raw_tags() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute_batch(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES
            (1, 'Rock', 'rock', NULL),
            (2, 'Indie Rock', 'indie-rock', 1),
            (3, 'Psytrance', 'psytrance', NULL);
         INSERT INTO artists (id, name) VALUES (9301, 'G');",
    )
    .expect("seed");
    for i in 0..4i64 {
        let id = 9301 + i;
        conn.execute(
            &format!(
                "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
                    fidelity_score, is_favorite, source, play_count
                 ) VALUES ({id}, 'G{i}', 9301, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
            ),
            [],
        )
        .expect("track");
    }
    conn.execute_batch(
        "INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
            (9301, 1, 'musicbrainz', 0.8),  -- solid rock tag: matches
            (9302, 2, 'musicbrainz', 0.8),  -- indie rock (descendant): matches
            (9303, 1, 'lastfm', 0.3),        -- weak-only tag: rescued, matches
            (9304, 3, 'musicbrainz', 0.9),  -- psytrance track...
            (9304, 1, 'lastfm', 0.29);       -- ...with junk rock tag: must NOT match",
    )
    .expect("tags");

    // Route-equivalent flow: resolve token, expand descendants, filter.
    let (ids, unmatched) = resolve_genre_tokens(&conn, &["rock".to_string()]).expect("resolve");
    assert!(unmatched.is_empty());
    let expanded = expand_genre_descendants(&conn, &ids).expect("expand");

    let filters = AudioFilters {
        genre_ids: expanded,
        ..Default::default()
    };
    let mut result_ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
        .expect("search")
        .iter()
        .map(|r| r.id)
        .collect();
    result_ids.sort_unstable();
    assert_eq!(
        result_ids,
        vec![9301, 9302, 9303],
        "curated rowset must admit strong + descendant + rescued tags and drop junk"
    );

    let total = count_audio_filter_matches(&conn, "", &filters).expect("count");
    assert_eq!(total, 3, "count must apply the same genre rowset");
}

#[test]
fn genre_filter_ranks_strongest_match_over_favorite() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute_batch(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES (1, 'Rock', 'rock', NULL);
         INSERT INTO artists (id, name) VALUES (9601, 'R');",
    )
    .expect("seed");
    // Deep cut: definitive rock tag (0.9), never played, not a favorite.
    // Crossover hit: weak rock tag (0.55), favorited with heavy plays.
    conn.execute_batch(
        "INSERT INTO tracks
            (id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
             fidelity_score, is_favorite, source, play_count)
         VALUES
            (9601, 'Deep Cut', 9601, 200000, 9601, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (9602, 'Crossover Hit', 9601, 200000, 9602, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 999);
         INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
            (9601, 1, 'musicbrainz', 0.9),
            (9602, 1, 'lastfm', 0.55);",
    )
    .expect("tags");

    let filters = AudioFilters {
        genre_ids: vec![1],
        ..Default::default()
    };
    let ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
        .expect("search")
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(
        ids,
        vec![9601, 9602],
        "strongest genre match must rank first even against a heavy favorite"
    );

    // Without a genre filter, the favorite/play ranking still wins.
    let ids_plain: Vec<i64> = search_with_audio_filters(&conn, "", &AudioFilters::default(), 50, 0)
        .expect("search")
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids_plain, vec![9602, 9601]);
}

#[test]
fn normalize_key_signature_canonicalizes_user_input() {
    assert_eq!(normalize_key_signature("Am"), "Am");
    assert_eq!(normalize_key_signature("am"), "Am");
    assert_eq!(normalize_key_signature("A"), "Amaj");
    assert_eq!(normalize_key_signature("a major"), "Amaj");
    assert_eq!(normalize_key_signature("Bb"), "A#maj");
    assert_eq!(normalize_key_signature("bbm"), "A#m");
    assert_eq!(normalize_key_signature("f# minor"), "F#m");
    assert_eq!(normalize_key_signature("Cb"), "Bmaj");
    // Unrecognized input passes through for the NOCASE match to try.
    assert_eq!(normalize_key_signature("8A"), "8A");
    assert_eq!(normalize_key_signature("Axyz"), "Axyz");
}

#[test]
fn key_filter_matches_case_insensitively_with_enharmonics() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute("INSERT INTO artists (id, name) VALUES (9401, 'K')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
            fidelity_score, is_favorite, source, play_count
         ) VALUES
            (9401, 'MinorTrack', 9401, 200000, 9401, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (9402, 'SharpTrack', 9401, 200000, 9402, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
        [],
    )
    .expect("tracks");
    conn.execute(
        "INSERT INTO audio_dsp_features (track_id, key_signature, camelot_key) VALUES
            (9401, 'Am', '8A'), (9402, 'A#maj', '6B')",
        [],
    )
    .expect("dsp");

    let by_key = |key: &str| -> Vec<i64> {
        let filters = AudioFilters {
            key_signature: Some(key.to_string()),
            ..Default::default()
        };
        search_with_audio_filters(&conn, "", &filters, 50, 0)
            .expect("search")
            .iter()
            .map(|r| r.id)
            .collect()
    };
    assert_eq!(by_key("am"), vec![9401], "lowercase minor");
    assert_eq!(by_key("Bb"), vec![9402], "flat spelling of A#maj");

    let filters = AudioFilters {
        camelot_key: Some("8a".to_string()),
        ..Default::default()
    };
    let ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
        .expect("search")
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, vec![9401], "camelot NOCASE");
}

#[test]
fn artist_and_album_contains_filters_match_substrings() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    conn.execute_batch(
        "INSERT INTO artists (id, name) VALUES (9501, 'Radiohead'), (9502, 'Portishead');
         INSERT INTO albums (id, title, artist_id, source) VALUES
            (9501, 'OK Computer', 9501, 'tidal'),
            (9502, 'Dummy', 9502, 'tidal');
         INSERT INTO tracks
            (id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
             fidelity_score, is_favorite, source, play_count)
         VALUES
            (9501, 'Airbag', 9501, 9501, 200000, 9501, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
            (9502, 'Roads', 9502, 9502, 200000, 9502, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0);",
    )
    .expect("seed");

    let filters = AudioFilters {
        artist_contains: Some("RADIO".to_string()),
        ..Default::default()
    };
    let ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
        .expect("search")
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, vec![9501]);

    let filters = AudioFilters {
        album_contains: Some("dummy".to_string()),
        ..Default::default()
    };
    let ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
        .expect("search")
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, vec![9502]);
}

/// End-to-end Path A read API: cascade joins through `genre_paths` and
/// returns ancestor-expanded paths with provenance. Mirrors the Path B
/// fixture (in genre/filter.rs) — same artist/album/comp shape.
#[test]
fn get_genres_for_tracks_with_fallback_returns_provenance() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");

    // Genre tree: Electronic > Drum and Bass; Jazz; Rock.
    conn.execute_batch(
        "INSERT INTO genres (id, name, slug, parent_id) VALUES
            (1, 'Electronic', 'electronic', NULL),
            (2, 'Drum and Bass', 'drum-and-bass', 1),
            (3, 'Jazz', 'jazz', NULL),
            (4, 'Rock', 'rock', NULL);
         INSERT INTO artists (id, name) VALUES
            (1, 'CoherentArtist'),
            (2, 'PartiallyTaggedArtist'),
            (3, 'CompContributorA'),
            (4, 'CompContributorB');
         INSERT INTO albums (id, title, artist_id, source) VALUES
            (10, 'CoherentAlbum', 1, 'tidal'),
            (20, 'PartialAlbumA', 2, 'tidal'),
            (21, 'PartialAlbumB', 2, 'tidal'),
            (30, 'MultiArtistComp', 3, 'tidal');
         INSERT INTO tracks
            (id, title, artist_id, album_id, duration_ms, best_quality, best_source, fidelity_score, source)
         VALUES
            (100, 'Tagged on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
            (101, 'Empty on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
            (200, 'Tagged on partial A', 2, 20, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
            (201, 'Empty on partial B', 2, 21, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
            (300, 'Tagged on comp (artist 3)', 3, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
            (301, 'Empty on comp (artist 4)', 4, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal');
         INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
            (100, 2, 'musicbrainz', 1.0),
            (200, 3, 'musicbrainz', 0.9),
            (300, 4, 'lastfm', 0.7);",
    )
    .expect("seed fixtures");

    let result = get_genres_for_tracks_with_fallback(&conn, &[100, 101, 201, 301]).expect("query");

    // Track 100: direct genre (Drum and Bass), ancestor-expanded into two paths
    // ("Electronic" via the parent walk and "Electronic > Drum and Bass" via the leaf).
    let r100 = result.get(&100).expect("track 100 present");
    assert!(r100.iter().all(|g| g.source == GenreSource::Track));
    assert!(
        r100.iter().any(|g| g.path == "Electronic > Drum and Bass"),
        "expected leaf path for direct genre, got {r100:?}"
    );

    // Track 101: empty, rescued from album sibling (track 100, genre 2).
    let r101 = result.get(&101).expect("track 101 present");
    assert!(r101.iter().all(|g| g.source == GenreSource::AlbumFallback));
    assert!(r101.iter().any(|g| g.path == "Electronic > Drum and Bass"));

    // Track 201: empty, no album sibling, rescued from artist (track 200, genre 3 = Jazz).
    let r201 = result.get(&201).expect("track 201 present");
    assert!(r201.iter().all(|g| g.source == GenreSource::ArtistFallback));
    assert!(r201.iter().any(|g| g.path == "Jazz"));

    // Track 301: empty on multi-artist comp; album tier MUST skip;
    // artist 4 has no other tagged tracks. Track stays unrescued, so it
    // should be absent from the returned map (per existing function's
    // contract: tracks with no genres are absent rather than empty Vec).
    assert!(
        !result.contains_key(&301),
        "track 301 must NOT inherit comp-mate genres; got {:?}",
        result.get(&301)
    );
}

#[test]
fn paths_only_drops_provenance() {
    let rows = vec![
        ResolvedGenre {
            path: "Electronic > House".to_string(),
            source: GenreSource::Track,
        },
        ResolvedGenre {
            path: "Pop".to_string(),
            source: GenreSource::AlbumFallback,
        },
    ];
    let paths = ResolvedGenre::paths_only(&rows);
    assert_eq!(paths, vec!["Electronic > House", "Pop"]);
}

/// Seed one track with a distinct value in every projected column, so a
/// row-shape test can catch any column drift in `track_projection` /
/// `track_from_row`.
fn seed_fully_populated_track(conn: &Connection) {
    conn.execute(
        "INSERT INTO artists (id, name) VALUES (7, 'Projection Artist')",
        [],
    )
    .expect("seed artist");
    conn.execute(
        "INSERT INTO albums (id, title, artist_id, source, artwork_url)
         VALUES (3, 'Projection Album', 7, 'tidal', 'http://art/proj.jpg')",
        [],
    )
    .expect("seed album");
    conn.execute(
        "INSERT INTO tracks (
            id, title, artist_id, album_id, disc_number, track_number,
            duration_ms, isrc, tidal_id, ytmusic_id, soundcloud_id,
            best_quality, best_source, fidelity_score, is_favorite,
            play_count, last_played_at, date_added, source
         ) VALUES (
            42, 'Projection Track', 7, 3, 2, 11,
            234567, 'ISRCPROJ01', 99887, 'ytproj', 55443,
            'HI_RES', 'tidal', 88, 1,
            17, '2026-05-01T12:00:00Z', '2026-04-01T00:00:00Z', 'tidal'
         )",
        [],
    )
    .expect("seed track");
}

/// Every field on `Track` must round-trip through the shared projection.
/// `assert_track_is_fully_populated_seed` is reused by the two query-path
/// tests below that previously had no direct row-shape coverage.
fn assert_track_is_fully_populated_seed(track: &Track) {
    assert_eq!(track.id, 42);
    assert_eq!(track.title, "Projection Track");
    assert_eq!(track.artist_id, 7);
    assert_eq!(track.artist_name.as_deref(), Some("Projection Artist"));
    assert_eq!(track.album_id, Some(3));
    assert_eq!(track.album_title.as_deref(), Some("Projection Album"));
    assert_eq!(track.disc_number, Some(2));
    assert_eq!(track.track_number, Some(11));
    assert_eq!(track.duration_ms, Some(234567));
    assert_eq!(track.isrc.as_deref(), Some("ISRCPROJ01"));
    assert_eq!(track.tidal_id, Some(99887));
    assert_eq!(track.ytmusic_id.as_deref(), Some("ytproj"));
    assert_eq!(track.soundcloud_id, Some(55443));
    assert_eq!(track.best_quality.as_deref(), Some("HI_RES"));
    assert_eq!(track.best_source.as_deref(), Some("tidal"));
    assert_eq!(track.fidelity_score, 88);
    assert!(track.is_favorite);
    assert_eq!(track.play_count, 17);
    assert_eq!(
        track.last_played_at.as_deref(),
        Some("2026-05-01T12:00:00Z")
    );
    assert_eq!(track.date_added.as_deref(), Some("2026-04-01T00:00:00Z"));
    assert_eq!(track.source, "tidal");
    assert_eq!(track.artwork_url.as_deref(), Some("http://art/proj.jpg"));
}

#[test]
fn get_discovery_candidate_tracks_maps_every_projected_column() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_fully_populated_track(&conn);

    let tracks = get_discovery_candidate_tracks(&conn, 10).expect("candidates");
    assert_eq!(tracks.len(), 1);
    assert_track_is_fully_populated_seed(&tracks[0]);
}

#[test]
fn get_tracks_excluding_with_limit_maps_every_projected_column() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    schema::run_migrations(&conn).expect("migrations");
    seed_fully_populated_track(&conn);

    // Empty exclusion list + a generous cap returns the seed track.
    let tracks = get_tracks_excluding_with_limit(&conn, &[], 50).expect("candidates");
    assert_eq!(tracks.len(), 1);
    assert_track_is_fully_populated_seed(&tracks[0]);

    // And the exclusion path still filters correctly.
    let excluded = get_tracks_excluding_with_limit(&conn, &[42], 50).expect("excluded");
    assert!(excluded.is_empty(), "id 42 must be excluded");
}
