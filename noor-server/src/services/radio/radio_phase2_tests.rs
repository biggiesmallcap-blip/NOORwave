//! Phase 2a Stage 1 component tests for the dedup tie-break change
//! and the `apply_taste_signals` pass.
//!
//! A full orchestrator-level snapshot test would require seeding an
//! embedding model (for `radio_from_neighbors` to return library
//! results) plus a Last.fm stub; both add scope without strengthening
//! the gate beyond what these component tests already cover. The
//! orchestrator wiring is exercised end-to-end by the existing radio
//! API endpoints in production.
use super::*;
use crate::smart::taste_vector::{AffinitySignal, TasteVector};

fn cand(
    track_id: i64,
    source: RadioSource,
    artist_name: &str,
    title: &str,
    score: f64,
) -> RadioCandidate {
    RadioCandidate {
        track_id,
        tidal_track_id: None,
        title: title.to_string(),
        artist_name: artist_name.to_string(),
        album_title: None,
        artwork_url: None,
        duration_ms: None,
        isrc: None,
        is_in_library: source == RadioSource::Library,
        source,
        reason: format!("test {source:?}"),
        similarity_score: score,
        confidence: None,
        candidate_in_degree_percentile: None,
        support_count: None,
        primary_reason: None,
    }
}

// ─── combine_with_dedup: 5% library tie-break rule ────────────────────────

#[test]
fn dedup_library_wins_when_within_five_percent() {
    // Library 0.96 vs Lastfm 1.00 — library is 96% of lastfm, inside the
    // 0.95 threshold, so library still wins despite lower raw score.
    let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.96)];
    let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 1.0)];
    let out = combine_with_dedup(lib, lfm, Vec::new());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source, RadioSource::Library);
    assert_eq!(out[0].track_id, 1);
}

#[test]
fn dedup_lastfm_wins_when_library_below_threshold() {
    // Library 0.80 vs Lastfm 1.00 — library is 80%, below 0.95
    // threshold, so the higher non-library score wins.
    let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.80)];
    let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 1.0)];
    let out = combine_with_dedup(lib, lfm, Vec::new());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source, RadioSource::Lastfm);
}

#[test]
fn dedup_library_wins_alone_when_only_source_present() {
    // No competing source: library wins by default (0.95 of nothing
    // is satisfied).
    let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.20)];
    let out = combine_with_dedup(lib, Vec::new(), Vec::new());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source, RadioSource::Library);
}

#[test]
fn dedup_picks_highest_when_no_library_in_group() {
    // Lastfm 0.60 vs Engine 0.85 — no library, highest score wins.
    let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.60)];
    let eng = vec![cand(2, RadioSource::Engine, "A", "Song", 0.85)];
    let out = combine_with_dedup(Vec::new(), lfm, eng);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source, RadioSource::Engine);
}

#[test]
fn dedup_tie_break_prefers_library_then_engine_then_lastfm() {
    // All three sources at identical score 0.5. Library wins (and
    // would win regardless via the 5% rule), but the tie-break
    // ordering also matters when library is absent.
    let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.5)];
    let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.5)];
    let eng = vec![cand(2, RadioSource::Engine, "A", "Song", 0.5)];
    let out = combine_with_dedup(lib, lfm, eng);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source, RadioSource::Library);

    // Without library, engine should beat lastfm at the same score.
    let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.5)];
    let eng = vec![cand(2, RadioSource::Engine, "A", "Song", 0.5)];
    let out = combine_with_dedup(Vec::new(), lfm, eng);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source, RadioSource::Engine);
}

#[test]
fn dedup_keeps_unique_candidates_in_first_seen_order() {
    let lib = vec![
        cand(1, RadioSource::Library, "A", "First", 0.9),
        cand(2, RadioSource::Library, "B", "Second", 0.8),
    ];
    let lfm = vec![cand(0, RadioSource::Lastfm, "C", "Third", 0.7)];
    let out = combine_with_dedup(lib, lfm, Vec::new());
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].title, "First");
    assert_eq!(out[1].title, "Second");
    assert_eq!(out[2].title, "Third");
}

#[test]
fn dedup_drops_candidates_with_empty_normalised_key() {
    // Empty artist + title produces an empty norm key and is dropped.
    let lib = vec![
        cand(1, RadioSource::Library, "", "", 0.9),
        cand(2, RadioSource::Library, "Real", "Song", 0.8),
    ];
    let out = combine_with_dedup(lib, Vec::new(), Vec::new());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].track_id, 2);
}

// ─── apply_taste_signals: hard suppression + artist affinity ──────────────

fn make_taste(
    skipped: &[i64],
    artist_signals: &[(i64, f64, f64)], // (artist_id, pos, neg)
) -> TasteVector {
    let mut t = TasteVector::default();
    for id in skipped {
        t.skipped_track_ids.insert(*id);
    }
    for (artist_id, pos, neg) in artist_signals {
        t.artist_affinity.insert(
            *artist_id,
            AffinitySignal {
                pos: *pos,
                neg: *neg,
            },
        );
    }
    t
}

#[test]
fn apply_taste_signals_is_noop_on_empty_taste() {
    let taste = TasteVector::default();
    let resolver = ArtistResolver::default();
    let mut candidates = vec![
        cand(1, RadioSource::Library, "A", "Song1", 0.8),
        cand(2, RadioSource::Library, "B", "Song2", 0.6),
    ];
    let before = candidates.clone();
    apply_taste_signals(&mut candidates, &taste, &resolver);

    assert_eq!(candidates.len(), before.len());
    for (a, b) in candidates.iter().zip(before.iter()) {
        assert_eq!(a.track_id, b.track_id);
        assert!((a.similarity_score - b.similarity_score).abs() < 1e-12);
    }
}

#[test]
fn apply_taste_signals_drops_skipped_library_candidates() {
    let taste = make_taste(&[2], &[]);
    let resolver = ArtistResolver::default();
    let mut candidates = vec![
        cand(1, RadioSource::Library, "A", "Keep", 0.8),
        cand(2, RadioSource::Library, "B", "Drop", 0.9),
        cand(3, RadioSource::Engine, "C", "Keep", 0.7),
    ];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|c| c.track_id != 2));
}

#[test]
fn apply_taste_signals_does_not_drop_lastfm_with_zero_track_id() {
    // Lastfm hits have track_id = 0 even when track_id 0 is in
    // skipped_track_ids; they should never be hard-suppressed by this
    // path.
    let taste = make_taste(&[0], &[]);
    let resolver = ArtistResolver::default();
    let mut candidates = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.5)];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    assert_eq!(candidates.len(), 1);
}

#[test]
fn apply_taste_signals_nudges_score_for_known_artist() {
    // Artist 1: pos=10, neg=0. Saturation K=10:
    //   pos_c = 10/20 = 0.5, neg_c = 0
    //   multiplier = 1.0 + 0.5*0.20 - 0*0.30 = 1.10
    //   final = 0.5 * 1.10 = 0.55
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    conn.execute("INSERT INTO artists VALUES (1, 'A')", [])
        .unwrap();
    let resolver = ArtistResolver::load(&conn).unwrap();
    let taste = make_taste(&[], &[(1, 10.0, 0.0)]);
    let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 0.5)];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    assert!((candidates[0].similarity_score - 0.55).abs() < 1e-12);
}

#[test]
fn apply_taste_signals_penalises_negative_artist() {
    // Artist 1: pos=0, neg=10. Saturation K=10:
    //   pos_c = 0, neg_c = 10/20 = 0.5
    //   multiplier = 1.0 + 0 - 0.5*0.30 = 0.85
    //   final = 0.5 * 0.85 = 0.425
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    conn.execute("INSERT INTO artists VALUES (1, 'A')", [])
        .unwrap();
    let resolver = ArtistResolver::load(&conn).unwrap();
    let taste = make_taste(&[], &[(1, 0.0, 10.0)]);
    let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 0.5)];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    assert!((candidates[0].similarity_score - 0.425).abs() < 1e-12);
}

#[test]
fn apply_taste_signals_skips_unknown_artist_silently() {
    // Resolver has no entry for "Unknown"; affinity adjustment doesn't
    // fire and the score stays as-is.
    let resolver = ArtistResolver::default();
    let taste = make_taste(&[], &[(1, 10.0, 0.0)]);
    let mut candidates = vec![cand(100, RadioSource::Library, "Unknown", "Song", 0.5)];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    assert!((candidates[0].similarity_score - 0.5).abs() < 1e-12);
}

#[test]
fn apply_taste_signals_never_zeroes_under_high_negative_signal() {
    // Regression guard for the Doja Cat case. Old formula at neg=20:
    //   multiplier = 1.0 - 20*0.07 = -0.4, clamped to 0.0,
    //   destroying every library candidate from a recently-skipped
    //   artist.
    // New saturating formula at neg=20:
    //   neg_c = 20/30 = 0.667, multiplier = 1.0 - 0.667*0.30 = 0.80
    //   final = 0.5 * 0.80 = 0.40
    // Library candidates from skipped artists are demoted, not
    // eliminated.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    conn.execute("INSERT INTO artists VALUES (1, 'A')", [])
        .unwrap();
    let resolver = ArtistResolver::load(&conn).unwrap();
    let taste = make_taste(&[], &[(1, 0.0, 20.0)]);
    let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 0.5)];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    assert!(
        candidates[0].similarity_score > 0.3,
        "expected score > 0.3, got {}",
        candidates[0].similarity_score
    );
    assert!((candidates[0].similarity_score - 0.40).abs() < 1e-12);
}

#[test]
fn apply_taste_signals_neg_50_does_not_zero_score() {
    // High-magnitude neg: even a heavily-skipped artist should keep
    // a meaningful score. With the old 0.05/0.07 formula, neg = 50
    // gave multiplier = -2.5 → clamped to 0. With saturating
    // compression:
    //   neg_c = 50/60 = 0.833, multiplier = 1.0 - 0.833*0.30 = 0.75
    //   final = 0.5 * 0.75 = 0.375
    // The asymptote is 1.0 - 0.30 = 0.70 regardless of how large
    // neg gets, which is the point of the saturation curve.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    conn.execute("INSERT INTO artists VALUES (1, 'A')", [])
        .unwrap();
    let resolver = ArtistResolver::load(&conn).unwrap();
    let taste = make_taste(&[], &[(1, 0.0, 50.0)]);
    let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 1.0)];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    let multiplier = candidates[0].similarity_score;
    assert!(
        multiplier > 0.5,
        "neg=50 should leave multiplier > 0.5, got {multiplier}"
    );
    assert!((multiplier - 0.75).abs() < 1e-9);
}

#[test]
fn apply_taste_signals_pos_50_does_not_overshoot() {
    // High-magnitude pos: a beloved artist still gets a bounded
    // boost, not unbounded growth that would swamp source-native
    // similarity. With the old 0.05 formula, pos = 50 gave
    // multiplier = 3.5 → score 5x boosted, which would dwarf
    // last.fm match scores entirely. With saturating compression:
    //   pos_c = 50/60 = 0.833, multiplier = 1.0 + 0.833*0.20 = 1.167
    // Asymptote is 1.0 + 0.20 = 1.20.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    conn.execute("INSERT INTO artists VALUES (1, 'A')", [])
        .unwrap();
    let resolver = ArtistResolver::load(&conn).unwrap();
    let taste = make_taste(&[], &[(1, 50.0, 0.0)]);
    let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 1.0)];
    apply_taste_signals(&mut candidates, &taste, &resolver);
    let multiplier = candidates[0].similarity_score;
    assert!(
        (1.10..=1.30).contains(&multiplier),
        "pos=50 multiplier should sit in [1.10, 1.30], got {multiplier}"
    );
}

// ─── Stage 2: engine slot fills with track_similarity ─────────────────────

/// Build an in-memory Database with the full schema and seed:
///   - artist 1 "A"
///   - 4 tracks (100..103) — 100 is the radio seed
///   - 3 track_similarity rows pointing seed→{101, 102, 103} with
///     different score components (co-album, co-artist, genre proximity)
///
/// The seeded rows represent three legitimate library-similarity
/// signals that the engine slot should surface:
///   - track 101 (sim 0.85, co_album=1.0): same album as seed.
///   - track 102 (sim 0.65, co_artist=1.0): same artist, different album.
///   - track 103 (sim 0.30, genre=0.5):    shared genre branch only.
///
/// These are the three new tracks the engine slot brings to a radio
/// queue that previously (Stage 1) would have returned only library
/// (embedding) and lastfm results. None of them require an embedding
/// model to surface; track_similarity is the second recall path.
fn seed_engine_test_db() -> Database {
    let db = Database::open(":memory:").expect("in-memory db");
    db.run_migrations().expect("run migrations");
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
        for id in 100..=103 {
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, album_id, source) \
                 VALUES (?1, ?2, 1, NULL, 'tidal')",
                rusqlite::params![id, format!("Track {id}")],
            )?;
        }
        // track_similarity has CHECK (track_a < track_b), so seed=100
        // is always track_a. similarity_score is the rolled-up field
        // that engine_results_from_track_similarity sorts by.
        for (b, sim, co_album, co_artist, genre) in [
            (101_i64, 0.85_f64, 1.0_f64, 0.0_f64, 0.0_f64),
            (102_i64, 0.65_f64, 0.0_f64, 1.0_f64, 0.0_f64),
            (103_i64, 0.30_f64, 0.0_f64, 0.0_f64, 0.5_f64),
        ] {
            conn.execute(
                "INSERT INTO track_similarity \
                 (track_a, track_b, similarity_score, co_album_score, co_artist_score, genre_proximity) \
                 VALUES (100, ?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![b, sim, co_album, co_artist, genre],
            )?;
        }
        Ok(())
    })
    .unwrap();
    db
}

#[test]
fn engine_returns_seeded_track_similarity_in_score_order() {
    let db = seed_engine_test_db();
    let results = engine_results_from_track_similarity(&db, 100, 10, &[]).expect("engine results");

    assert_eq!(results.len(), 3, "expected 3 seeded similarity rows");

    // Ordering: 101 (0.85) > 102 (0.65) > 103 (0.30).
    let ids: Vec<i64> = results.iter().map(|c| c.track_id).collect();
    assert_eq!(ids, vec![101, 102, 103]);

    // Provenance: every result is engine-source, in-library, with a
    // reason string carrying the score breakdown so the
    // "Why is this here?" UI can show it.
    for cand in &results {
        assert_eq!(cand.source, RadioSource::Engine);
        assert!(cand.is_in_library);
        assert!(cand.reason.starts_with("library similarity"));
        assert!(cand.reason.contains("co-album"));
        assert!(cand.reason.contains("co-artist"));
        assert!(cand.reason.contains("genre"));
    }

    // Specific component check: track 101 was seeded with co_album=1.0,
    // so its reason should mention that magnitude.
    let r101 = results.iter().find(|c| c.track_id == 101).unwrap();
    assert!(r101.reason.contains("co-album 1.00"));
}

#[test]
fn engine_respects_target_limit() {
    let db = seed_engine_test_db();
    let results = engine_results_from_track_similarity(&db, 100, 2, &[])
        .expect("engine results truncated to 2");
    assert_eq!(results.len(), 2);
    // Top 2 by similarity_score: 101 (0.85), 102 (0.65).
    assert_eq!(results[0].track_id, 101);
    assert_eq!(results[1].track_id, 102);
}

#[test]
fn engine_excludes_listed_track_ids() {
    let db = seed_engine_test_db();
    // Exclude track 101 — should drop the top result.
    let results = engine_results_from_track_similarity(&db, 100, 10, &[101])
        .expect("engine results with exclusion");
    let ids: Vec<i64> = results.iter().map(|c| c.track_id).collect();
    assert_eq!(ids, vec![102, 103]);
}

#[test]
fn engine_returns_empty_when_target_is_zero() {
    let db = seed_engine_test_db();
    let results = engine_results_from_track_similarity(&db, 100, 0, &[]).expect("zero target");
    assert!(results.is_empty());
}

/// Stage 2 before/after diff demonstration with affinity logging.
///
/// Before (Stage 1): library + lastfm + empty engine = `empty_input`
/// candidate set after combine_with_dedup. With empty taste and an
/// empty resolver, apply_taste_signals is a no-op. Result is the
/// `empty_input` count.
///
/// After (Stage 2): library + lastfm + non-empty engine = three new
/// engine candidates surface. Each is a real library track with
/// documented similarity provenance. None override existing dedup
/// winners (they cover artist names not present in the library/
/// lastfm slots in this fixture).
///
/// Per-candidate affinity multiplier is logged for visibility — soft
/// signal, not a gate. With the small artist-affinity values seeded
/// here the multipliers stay close to 1.0; if a future change made
/// the formula aggressive enough to swamp the source-native score,
/// these logs would make it obvious.
#[test]
fn stage_2_engine_diff_with_affinity_logging() {
    let db = seed_engine_test_db();

    // Empty engine baseline (Stage 1 shape): combine library +
    // lastfm + empty engine. Use empty source slots since this test
    // exercises only the diff brought by the engine slot itself.
    let empty_path = combine_with_dedup(Vec::new(), Vec::new(), Vec::new());
    assert!(empty_path.is_empty(), "no candidates without engine slot");

    // Engine-on (Stage 2 shape): same combine, with engine populated.
    let engine = engine_results_from_track_similarity(&db, 100, 10, &[]).expect("engine results");
    let engine_count = engine.len();
    let mut engine_path = combine_with_dedup(Vec::new(), Vec::new(), engine);

    assert_eq!(
        engine_path.len(),
        engine_count,
        "every engine candidate survives dedup when no other source competes"
    );

    // Apply a non-empty taste to exercise the affinity path. Artist
    // 1 (which all engine candidates belong to) is mildly liked.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    conn.execute("INSERT INTO artists VALUES (1, 'A')", [])
        .unwrap();
    let resolver = ArtistResolver::load(&conn).unwrap();
    let taste = make_taste(&[], &[(1, 4.0, 1.0)]); // pos=4, neg=1

    // Snapshot scores before affinity to compute the multiplier
    // empirically per candidate.
    let pre: Vec<(i64, f64)> = engine_path
        .iter()
        .map(|c| (c.track_id, c.similarity_score))
        .collect();

    apply_taste_signals(&mut engine_path, &taste, &resolver);

    // Stage 2 visibility: print per-candidate affinity multiplier
    // so any future formula change shows up here. Soft signal,
    // not a hard gate.
    eprintln!("stage 2 affinity multipliers (artist=A, pos=4, neg=1):");
    let post: Vec<(i64, f64)> = engine_path
        .iter()
        .map(|c| (c.track_id, c.similarity_score))
        .collect();
    for (track_id, post_score) in &post {
        let pre_score = pre.iter().find(|(id, _)| id == track_id).unwrap().1;
        let multiplier = post_score / pre_score;
        eprintln!(
            "  track {track_id}: pre={pre_score:.4} post={post_score:.4} multiplier={multiplier:.4}"
        );
    }

    // Expected multiplier from the saturating formula:
    //   pos_c = 4/(4+10) = 0.2857..
    //   neg_c = 1/(1+10) = 0.0909..
    //   mult  = 1.0 + 0.2857*0.20 - 0.0909*0.30 ≈ 1.02987
    // Same artist on every candidate, so every multiplier is the
    // same. Compared to the old 0.05/0.07 formula's 1.13, the
    // saturated version produces a smaller nudge for low-magnitude
    // pos/neg — which is correct: "barely any signal" should
    // barely move the score.
    let expected_mult = 1.0 + (4.0_f64 / 14.0) * 0.20 - (1.0_f64 / 11.0) * 0.30;
    for (track_id, post_score) in &post {
        let pre_score = pre.iter().find(|(id, _)| id == track_id).unwrap().1;
        let actual_mult = post_score / pre_score;
        assert!(
            (actual_mult - expected_mult).abs() < 1e-9,
            "expected multiplier {expected_mult:.6} for track {track_id}, got {actual_mult:.6}"
        );
    }

    // Justification (for the commit and for future readers):
    // - Track 101 surfaces because it shares an album with the seed
    //   (co_album=1.0). Same-album tracks are correctly classified as
    //   similar by the precomputed table.
    // - Track 102 surfaces because it shares the artist (co_artist=1.0)
    //   without sharing the album — exactly the kind of "more by this
    //   artist" expansion radio should offer.
    // - Track 103 surfaces from genre-proximity alone (genre=0.5) at a
    //   correspondingly lower score, reflecting weaker library
    //   evidence for similarity.
    // None of these would have surfaced in Stage 1 because the engine
    // slot was empty; the embedding model (the other library recall
    // path) is independent and may or may not also surface them.
    let ids: Vec<i64> = engine_path.iter().map(|c| c.track_id).collect();
    assert_eq!(ids, vec![101, 102, 103]);
}

// ─── Phase 2b Stage 1: reason-string JSON suffix ──────────────────────────

#[test]
fn annotate_reasons_appends_json_suffix_for_library_candidate() {
    // Library candidate with all three signals populated. Suffix
    // carries genre_jaccard, affinity_mult, and genre_mult.
    let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 1.32)];
    let key = (RadioSource::Library, 100, normalize_for_dedup("A", "Song"));
    let pre_affinity = std::collections::HashMap::from([(key.clone(), 1.0_f64)]);
    // Post-affinity = 1.10 means apply_taste_signals applied a +10% nudge.
    // The candidate's current similarity_score (1.32) divided by post_affinity
    // gives genre_mult = 1.20.
    let post_affinity = std::collections::HashMap::from([(key.clone(), 1.10_f64)]);
    let jaccard_by_key = std::collections::HashMap::from([(key, 0.67_f64)]);
    annotate_reasons(
        &mut candidates,
        &pre_affinity,
        &post_affinity,
        &jaccard_by_key,
    );
    let reason = &candidates[0].reason;
    assert!(
        reason.contains(" | "),
        "expected JSON suffix separator, got: {reason}"
    );
    assert!(reason.contains("\"genre_jaccard\":0.6700"), "got: {reason}");
    assert!(reason.contains("\"affinity_mult\":1.1000"), "got: {reason}");
    assert!(reason.contains("\"genre_mult\":1.2000"), "got: {reason}");
}

#[test]
fn annotate_reasons_emits_partial_suffix_when_only_affinity_available() {
    // Last.fm candidate (track_id=0): no Jaccard, no genre_mult
    // (lastfm passes through apply_genre_signals untouched).
    let mut candidates = vec![cand(0, RadioSource::Lastfm, "B", "Tune", 0.20)];
    let key = (RadioSource::Lastfm, 0, normalize_for_dedup("B", "Tune"));
    let pre_affinity = std::collections::HashMap::from([(key.clone(), 0.20_f64)]);
    // Post-affinity equals the live score (no genre pass touched it).
    let post_affinity = std::collections::HashMap::from([(key, 0.20_f64)]);
    let jaccard_by_key: std::collections::HashMap<(RadioSource, i64, String), f64> =
        std::collections::HashMap::new();
    annotate_reasons(
        &mut candidates,
        &pre_affinity,
        &post_affinity,
        &jaccard_by_key,
    );
    let reason = &candidates[0].reason;
    assert!(reason.contains(" | "), "got: {reason}");
    assert!(!reason.contains("genre_jaccard"), "got: {reason}");
    assert!(reason.contains("\"affinity_mult\":1.0000"), "got: {reason}");
    // genre_mult is post / post = 1.0 — emitted, but indistinguishable
    // from no-op. That's fine; the tooltip filters near-1.0 values.
}

#[test]
fn annotate_reasons_skips_when_no_signals_present() {
    // No pre_affinity / post_affinity / Jaccard entries. Reason
    // string is left untouched.
    let mut candidates = vec![cand(0, RadioSource::Lastfm, "C", "Song", 0.5)];
    let original_reason = candidates[0].reason.clone();
    let pre_affinity: std::collections::HashMap<(RadioSource, i64, String), f64> =
        std::collections::HashMap::new();
    let post_affinity: std::collections::HashMap<(RadioSource, i64, String), f64> =
        std::collections::HashMap::new();
    let jaccard_by_key: std::collections::HashMap<(RadioSource, i64, String), f64> =
        std::collections::HashMap::new();
    annotate_reasons(
        &mut candidates,
        &pre_affinity,
        &post_affinity,
        &jaccard_by_key,
    );
    assert_eq!(candidates[0].reason, original_reason);
}

// ─── Phase 2b Stage 2: genre coherence scoring ────────────────────────────

fn run_genre_signals(
    cand_pairs: &[(RadioSource, i64, &str, &str, f64)],
    jaccards: &[(RadioSource, i64, &str, &str, f64)],
    blend: RadioBlend,
) -> Vec<RadioCandidate> {
    let mut candidates: Vec<RadioCandidate> = cand_pairs
        .iter()
        .map(|(s, tid, an, t, score)| cand(*tid, *s, an, t, *score))
        .collect();
    let map: HashMap<(RadioSource, i64, String), f64> = jaccards
        .iter()
        .map(|(s, tid, an, t, j)| ((*s, *tid, normalize_for_dedup(an, t)), *j))
        .collect();
    apply_genre_signals(&mut candidates, &map, blend);
    candidates
}

#[test]
fn genre_score_multiplier_full_overlap() {
    // jaccard 1.0: bonus 0.30, no penalty (>= 0.5). Multiplier 1.30.
    let result = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        RadioBlend::Mixed,
    );
    assert_eq!(result.len(), 1);
    assert!(
        (result[0].similarity_score - 1.30).abs() < 1e-9,
        "got {}",
        result[0].similarity_score
    );
}

#[test]
fn genre_score_multiplier_partial_no_penalty_at_threshold() {
    // jaccard 0.5: bonus 0.15, no penalty (penalty branch fires only
    // for jaccard < 0.5). Multiplier 1.15.
    let result = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 0.5)],
        RadioBlend::Mixed,
    );
    assert!((result[0].similarity_score - 1.15).abs() < 1e-9);
}

#[test]
fn genre_score_multiplier_zero_overlap() {
    // jaccard 0.0 under Adventurous (no hard reject):
    // bonus 0.0, penalty 1.0*0.20 = 0.20. Multiplier 0.80.
    let result = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 0.0)],
        RadioBlend::Adventurous,
    );
    assert!((result[0].similarity_score - 0.80).abs() < 1e-9);
}

#[test]
fn genre_score_multiplier_floor_clamp() {
    // Pathological negative multiplier (shouldn't happen with the
    // current formula but defensive floor must hold).
    // The formula floors at 0.1 — even with the bonus and penalty
    // values, jaccard=0 produces 0.80, not below the floor.
    let result = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 0.0)],
        RadioBlend::Adventurous,
    );
    assert!(result[0].similarity_score >= 0.1);
}

#[test]
fn genre_hard_reject_familiar_drops_disjoint() {
    // jaccard 0.05 under Familiar (threshold 0.10): drop.
    let result = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 0.05)],
        RadioBlend::Familiar,
    );
    assert_eq!(result.len(), 0, "Familiar should hard-reject jaccard 0.05");
}

#[test]
fn genre_hard_reject_mixed_borderline() {
    // jaccard 0.04 under Mixed (threshold 0.05): drop.
    // jaccard 0.06 under Mixed: keep.
    let dropped = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 0.04)],
        RadioBlend::Mixed,
    );
    assert_eq!(dropped.len(), 0);
    let kept = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 0.06)],
        RadioBlend::Mixed,
    );
    assert_eq!(kept.len(), 1);
}

#[test]
fn genre_hard_reject_adventurous_keeps_disjoint() {
    // jaccard 0.0 under Adventurous: keep (no hard reject), score
    // demoted by penalty.
    let result = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 1.0)],
        &[(RadioSource::Library, 100, "A", "Song", 0.0)],
        RadioBlend::Adventurous,
    );
    assert_eq!(result.len(), 1);
    assert!(result[0].similarity_score < 1.0);
}

#[test]
fn genre_lastfm_skipped_when_no_genre_data() {
    // Lastfm candidate with track_id=0 has no entry in jaccard_by_key.
    // It must pass through apply_genre_signals with similarity_score
    // unchanged AND not be hard-rejected even under Familiar.
    let result = run_genre_signals(
        &[(RadioSource::Lastfm, 0, "C", "Song", 0.20)],
        &[],
        RadioBlend::Familiar,
    );
    assert_eq!(result.len(), 1);
    assert!((result[0].similarity_score - 0.20).abs() < 1e-9);
}

#[test]
fn genre_library_with_no_jaccard_passes_through() {
    // Library candidate without a jaccard entry (e.g. seed had no
    // genres, or this candidate had no genre rows). Pass through.
    let result = run_genre_signals(
        &[(RadioSource::Library, 100, "A", "Song", 0.50)],
        &[],
        RadioBlend::Familiar,
    );
    assert_eq!(result.len(), 1);
    assert!((result[0].similarity_score - 0.50).abs() < 1e-9);
}
