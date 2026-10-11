use super::*;

#[test]
fn tidal_mix_fills_a_cold_seed_with_library_rows_first() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO artists (id, name) VALUES (1, 'Surgeon');
         INSERT INTO tracks (id, title, artist_id, tidal_id) VALUES (7, 'Klonk', 1, 502);",
    )
    .unwrap();
    let track =
        |id: i64, title: &str, artist: &str| -> crate::services::tidal::client::TidalTrack {
            serde_json::from_value(serde_json::json!({
                "id": id, "title": title, "duration": 360,
                "artist": {"id": 1, "name": artist},
                "album": {"id": 9, "title": "Album", "cover": "ab-cd"}
            }))
            .unwrap()
        };
    let payload = track(500, "Track 12", "Steve Bicknell");
    let mut extra = payload.extra.clone();
    extra.insert(
        "mixes".to_string(),
        serde_json::json!({"TRACK_MIX": "0012abc"}),
    );
    assert_eq!(
        tidal_mix_id(&extra, "TRACK_MIX").as_deref(),
        Some("0012abc")
    );
    assert_eq!(tidal_mix_id(&extra, "ARTIST_MIX"), None);
    extra.insert(
        "mixes".to_string(),
        serde_json::json!({"TRACK_MIX": "../x?y"}),
    );
    assert_eq!(tidal_mix_id(&extra, "TRACK_MIX"), None);

    let mix = vec![
        track(500, "Track 12", "Steve Bicknell"),
        track(501, "Bad Boy", "Jeff Mills"),
        track(502, "Klonk", "Surgeon"),
        track(503, "bad boy", "Jeff Mills"),
    ];
    let picks = tidal_mix_candidates(&conn, mix, Some(500), &[], 10);
    let ids: Vec<_> = picks
        .iter()
        .map(|c| (c.track_id, c.tidal_track_id))
        .collect();
    // Seed skipped, duplicate title skipped, library track comes in as itself.
    assert_eq!(ids, vec![(0, Some(501)), (7, Some(502))]);
    assert!(!picks[0].is_in_library && picks[1].is_in_library);
    assert!(picks.iter().all(|c| c.source == RadioSource::Tidal));
    assert_eq!(
        tidal_mix_candidates(&conn, vec![track(504, "X", "Y")], None, &picks, 0).len(),
        0
    );
}
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn recent_played_artist_names_keep_to_the_hour_window_for_rfc3339_rows() {
    let db = Database::open(":memory:").expect("in-memory db");
    db.run_migrations().expect("run migrations");
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Recent'), (2, 'Stale')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, source)
             VALUES (1, 'Now', 1, NULL, 'tidal'), (2, 'Earlier', 2, NULL, 'tidal')",
            [],
        )?;
        // RFC 3339, the way the player writes listen_history.
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at) VALUES
                (1, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-30 minutes')),
                (2, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-5 hours'))",
            [],
        )?;
        Ok(())
    })
    .expect("seed");
    assert_eq!(
        recent_played_artist_names(&db, 10),
        vec!["Recent".to_string()]
    );
}

#[test]
fn lastfm_two_hop_follows_the_learned_neighbors_links() {
    let db = Database::open(":memory:").expect("in-memory db");
    db.run_migrations().expect("run migrations");
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id) VALUES (1, 'Seed', 1), (2, 'Neighbor', 1)",
            [],
        )?;
        let model = crate::db::queries::create_embedding_model(
            conn,
            "discovery-fusion-v2:two-hop",
            crate::db::queries::DISCOVERY_ENGINE_V2_FAMILY,
            8,
            "ready",
            None,
        )?;
        crate::db::queries::activate_embedding_model(conn, model.id)?;
        conn.execute(
            "INSERT INTO track_neighbors (track_id, neighbor_track_id, model_id, rank, score)
             VALUES (1, 2, ?1, 1, 0.9)",
            rusqlite::params![model.id],
        )?;
        let candidate = crate::db::queries::upsert_external_track_candidate(
            conn,
            &crate::db::queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(7001),
                mbid: None,
                dedupe_key: "tidal:7001".to_string(),
                title: "Far Song".to_string(),
                artist_name: "Far Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(200_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        crate::db::queries::upsert_external_candidate_sighting(
            conn,
            &crate::db::queries::ExternalCandidateSightingUpsert {
                candidate_id: candidate.id,
                seed_track_id: 2,
                source: "lastfm_similar".to_string(),
                source_payload_json: None,
                similarity: Some(0.6),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        Ok(())
    })
    .expect("seed");

    let hops = lastfm_two_hop_candidates(&db, 1, 10);

    assert_eq!(hops.len(), 1);
    assert_eq!(hops[0].title, "Far Song");
    assert_eq!(hops[0].tidal_track_id, Some(7001));
    assert_eq!(hops[0].source, RadioSource::Lastfm);
    // 0.6 similarity x 0.65 hop weight x 0.9 neighbor score.
    assert!((hops[0].similarity_score - 0.351).abs() < 1e-9);
}

#[test]
fn blends_get_further_from_the_seed_as_they_get_more_adventurous() {
    // The engine lane is album, artist and co-listen siblings: the closest
    // source. Last.fm reaches furthest. Adventurous must lean away from
    // siblings, not toward them.
    let (_, fam_lfm, fam_eng) = RadioBlend::Familiar.weights();
    let (_, mix_lfm, mix_eng) = RadioBlend::Mixed.weights();
    let (_, adv_lfm, adv_eng) = RadioBlend::Adventurous.weights();
    assert!(fam_eng > mix_eng && mix_eng > adv_eng);
    assert!(fam_lfm < mix_lfm && mix_lfm < adv_lfm);
}

#[test]
fn weights_sum_to_one() {
    for blend in [
        RadioBlend::Familiar,
        RadioBlend::Mixed,
        RadioBlend::Adventurous,
    ] {
        let (a, b, c) = blend.weights();
        assert!(
            (a + b + c - 1.0).abs() < 1e-9,
            "weights for {blend:?}: {a}+{b}+{c}"
        );
    }
}

fn similar_track(idx: usize) -> crate::metadata::lastfm::LastFmSimilarTrack {
    crate::metadata::lastfm::LastFmSimilarTrack {
        artist: format!("Artist {idx}"),
        title: format!("Track {idx}"),
        mbid: None,
        match_score: 1.0 - (idx as f64 * 0.01),
    }
}

#[tokio::test]
async fn lastfm_similar_cache_reuses_same_artist_title_within_ttl() {
    let cache = new_lastfm_similar_cache();
    let calls = Arc::new(AtomicUsize::new(0));

    let first = lastfm_similar_with_cache(Some(&cache), " The Artist ", " The Song ", 2, {
        let calls = calls.clone();
        || async move {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![similar_track(1), similar_track(2), similar_track(3)])
        }
    })
    .await
    .expect("first fetch");
    assert_eq!(first.len(), 2);

    let second = lastfm_similar_with_cache(Some(&cache), "the artist", "the song", 2, {
        let calls = calls.clone();
        || async move {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![similar_track(4)])
        }
    })
    .await
    .expect("cached fetch");

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(second.len(), first.len());
    assert_eq!(second[0].title, first[0].title);
    assert_eq!(second[1].artist, first[1].artist);
}

#[tokio::test]
async fn lastfm_similar_cache_refetches_when_larger_limit_is_needed() {
    let cache = new_lastfm_similar_cache();
    let calls = Arc::new(AtomicUsize::new(0));

    let first = lastfm_similar_with_cache(Some(&cache), "Artist", "Song", 1, {
        let calls = calls.clone();
        || async move {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![similar_track(1)])
        }
    })
    .await
    .expect("first fetch");
    assert_eq!(first.len(), 1);

    let second = lastfm_similar_with_cache(Some(&cache), "Artist", "Song", 3, {
        let calls = calls.clone();
        || async move {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![similar_track(1), similar_track(2), similar_track(3)])
        }
    })
    .await
    .expect("larger fetch");

    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(second.len(), 3);
}

#[test]
fn dedup_normalizes_punctuation_and_case() {
    let a = normalize_for_dedup("*NSYNC", "Bye Bye Bye");
    let b = normalize_for_dedup("nsync!!!", "byeByeBye");
    assert_eq!(a, b);
}

#[test]
fn dedup_normalizes_unicode_whitespace() {
    let a = normalize_for_dedup("Sigur  Rós", "Hoppípolla");
    let b = normalize_for_dedup("sigurrós", "hoppípolla");
    assert_eq!(a, b);
}

fn make_cand(source: RadioSource, idx: i64, score: f64) -> RadioCandidate {
    RadioCandidate {
        track_id: idx,
        tidal_track_id: None,
        title: format!("t{idx}"),
        artist_name: format!("a{idx}"),
        album_title: None,
        artwork_url: None,
        duration_ms: None,
        isrc: None,
        is_in_library: source == RadioSource::Library,
        source,
        reason: String::new(),
        similarity_score: score,
        confidence: None,
        candidate_in_degree_percentile: None,
        support_count: None,
        primary_reason: None,
    }
}

#[test]
fn normalize_skips_when_source_has_fewer_than_five() {
    // 3-candidate source: function should leave scores untouched.
    let mut cands: Vec<RadioCandidate> = (0..3)
        .map(|i| make_cand(RadioSource::Library, i, 0.10 + 0.20 * i as f64))
        .collect();
    let originals: Vec<f64> = cands.iter().map(|c| c.similarity_score).collect();
    normalize_source_scores(&mut cands);
    let after: Vec<f64> = cands.iter().map(|c| c.similarity_score).collect();
    assert_eq!(originals, after);
}

#[test]
fn normalize_top_candidate_lands_at_one_after_blend() {
    // Library has 5 distinct, well-spread scores. The top one's
    // rank_norm = 1.0; with non-degenerate p10/p90 spread the
    // top's score sits between (0.5*1.0 + 0.5*1.0) = 1.0.
    let scores = [0.10, 0.30, 0.50, 0.70, 0.90];
    let mut cands: Vec<RadioCandidate> = scores
        .iter()
        .enumerate()
        .map(|(i, &s)| make_cand(RadioSource::Library, i as i64, s))
        .collect();
    normalize_source_scores(&mut cands);
    let top = cands
        .iter()
        .fold(0.0_f64, |max, c| max.max(c.similarity_score));
    assert!(
        (top - 1.0).abs() < 1e-6,
        "top candidate normalized to {}",
        top
    );
    let bottom = cands
        .iter()
        .fold(f64::INFINITY, |min, c| min.min(c.similarity_score));
    assert!(bottom < 0.05, "bottom candidate normalized to {}", bottom);
}

#[test]
fn normalize_handles_degenerate_spread_via_neutral_pct() {
    // All five Last.fm scores equal — p90 == p10, hits the guard.
    // Resulting normalized scores should all be 0.5*0.5 + 0.5*rank_norm,
    // which still produces distinct values from the rank component.
    let mut cands: Vec<RadioCandidate> = (0..5)
        .map(|i| make_cand(RadioSource::Lastfm, i, 1.0))
        .collect();
    normalize_source_scores(&mut cands);
    // Top = 0.5*0.5 + 0.5*1.0 = 0.75. Bottom = 0.5*0.5 + 0.5*0.0 = 0.25.
    let top = cands
        .iter()
        .fold(0.0_f64, |max, c| max.max(c.similarity_score));
    let bottom = cands
        .iter()
        .fold(f64::INFINITY, |min, c| min.min(c.similarity_score));
    assert!((top - 0.75).abs() < 1e-6, "top = {}", top);
    assert!((bottom - 0.25).abs() < 1e-6, "bottom = {}", bottom);
}

#[test]
fn confidence_penalty_only_hits_library_below_threshold() {
    let mut cands = vec![
        // Below threshold — gets penalty
        {
            let mut c = make_cand(RadioSource::Library, 1, 1.0);
            c.confidence = Some(0.30);
            c
        },
        // At threshold — passes through (strict <, not <=)
        {
            let mut c = make_cand(RadioSource::Library, 2, 1.0);
            c.confidence = Some(0.40);
            c
        },
        // No confidence (lastfm) — passes through regardless
        make_cand(RadioSource::Lastfm, 3, 1.0),
    ];
    apply_confidence_penalty(&mut cands, 0.40);
    assert!((cands[0].similarity_score - 0.75).abs() < 1e-9);
    assert!((cands[1].similarity_score - 1.0).abs() < 1e-9);
    assert!((cands[2].similarity_score - 1.0).abs() < 1e-9);
}

#[test]
fn hub_penalty_scales_with_percentile() {
    let mut cands = vec![
        // pct=0 → multiplier=1.0 (no penalty)
        {
            let mut c = make_cand(RadioSource::Library, 1, 1.0);
            c.candidate_in_degree_percentile = Some(0.0);
            c
        },
        // pct=1.0 → multiplier=1/(1+0.5) = 0.667
        {
            let mut c = make_cand(RadioSource::Library, 2, 1.0);
            c.candidate_in_degree_percentile = Some(1.0);
            c
        },
        // No percentile data → passes through
        make_cand(RadioSource::Lastfm, 3, 1.0),
    ];
    let total = apply_hub_penalty(&mut cands, 0.5);
    assert!((cands[0].similarity_score - 1.0).abs() < 1e-9);
    let expected_top = 1.0 / 1.5;
    assert!((cands[1].similarity_score - expected_top).abs() < 1e-9);
    assert!((cands[2].similarity_score - 1.0).abs() < 1e-9);
    let expected_total = (1.0 - 1.0) + (1.0 - expected_top);
    assert!((total - expected_total).abs() < 1e-9);
}

#[test]
fn hub_penalty_zero_is_noop() {
    let mut cands = vec![{
        let mut c = make_cand(RadioSource::Library, 1, 0.7);
        c.candidate_in_degree_percentile = Some(0.9);
        c
    }];
    let total = apply_hub_penalty(&mut cands, 0.0);
    assert!((cands[0].similarity_score - 0.7).abs() < 1e-9);
    assert_eq!(total, 0.0);
}

fn make_cand_full(
    source: RadioSource,
    track_id: i64,
    artist: &str,
    title: &str,
    album: Option<&str>,
    score: f64,
) -> RadioCandidate {
    RadioCandidate {
        track_id,
        tidal_track_id: None,
        title: title.to_string(),
        artist_name: artist.to_string(),
        album_title: album.map(|s| s.to_string()),
        artwork_url: None,
        duration_ms: None,
        isrc: None,
        is_in_library: source == RadioSource::Library,
        source,
        reason: String::new(),
        similarity_score: score,
        confidence: None,
        candidate_in_degree_percentile: None,
        support_count: None,
        primary_reason: None,
    }
}

#[test]
fn diversity_rerank_spaces_same_artist() {
    // Five candidates by "A" with high scores, then one each by B and C
    // with lower scores. With same_artist_penalty active, the rerank
    // should not place A back-to-back even though A has the best raw scores.
    let cands = vec![
        make_cand_full(RadioSource::Library, 1, "A", "t1", None, 0.95),
        make_cand_full(RadioSource::Library, 2, "A", "t2", None, 0.94),
        make_cand_full(RadioSource::Library, 3, "B", "t3", None, 0.50),
        make_cand_full(RadioSource::Library, 4, "C", "t4", None, 0.40),
    ];
    let profile = crate::services::radio_config::RadioProfile {
        same_artist_penalty: 0.5,
        same_album_penalty: 0.0,
        genre_saturation_penalty: 0.0,
        diversity_weight: 1.0,
        ..crate::services::radio_config::RadioProfile::mixed()
    };
    let mut counters = RerankCounters::default();
    let queue = diversity_rerank(
        cands,
        &profile,
        RadioBlend::Mixed,
        4,
        &HashMap::new(),
        &HashSet::new(),
        false,
        &mut counters,
    );
    // First slot is a top-A. Second slot should be B (score 0.50) instead
    // of A2 (0.94 - 0.5 = 0.44).
    assert_eq!(queue[0].artist_name, "A");
    assert_eq!(
        queue[1].artist_name, "B",
        "second slot should not be same artist"
    );
}

#[test]
fn artist_cap_breaks_single_artist_marathon() {
    // Six top-ranked tracks by one artist plus lower-ranked alternatives:
    // the cap must interleave instead of letting the marathon through.
    let ranked = vec![
        make_cand_full(RadioSource::Library, 1, "Celine", "t1", None, 0.99),
        make_cand_full(RadioSource::Library, 2, "Celine", "t2", None, 0.98),
        make_cand_full(RadioSource::Library, 3, "Celine", "t3", None, 0.97),
        make_cand_full(RadioSource::Library, 4, "Celine", "t4", None, 0.96),
        make_cand_full(RadioSource::Library, 5, "Celine", "t5", None, 0.95),
        make_cand_full(RadioSource::Library, 6, "Celine", "t6", None, 0.94),
        make_cand_full(RadioSource::Library, 7, "Other A", "t7", None, 0.50),
        make_cand_full(RadioSource::Library, 8, "Other B", "t8", None, 0.40),
    ];
    let out = enforce_artist_diversity(ranked, &[], 6);
    assert_eq!(out.len(), 6);
    // Run cap: never more than two Celine slots in a row.
    let mut run = 0usize;
    let mut max_run = 0usize;
    for cand in &out {
        if cand.artist_name == "Celine" {
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 0;
        }
    }
    assert!(
        max_run <= 2,
        "consecutive run {max_run} exceeds cap: {out:?}"
    );
    // Window cap: at most two Celine slots in this (sub-window) queue plus
    // the degenerate tail once alternatives run out.
    let celine = out.iter().filter(|c| c.artist_name == "Celine").count();
    assert!(
        celine <= 4,
        "expected window cap to bound Celine, got {celine}"
    );
    // Rank order preserved among allowed picks: the two alternatives fill
    // the slots the cap denies to Celine.
    assert_eq!(out[2].artist_name, "Other A");
    assert_eq!(out[3].artist_name, "Other B");
}

#[test]
fn artist_cap_counts_recently_played_history() {
    // Two plays of the artist just happened: the first slot of the next
    // refill must not extend the run even though the artist ranks first.
    let ranked = vec![
        make_cand_full(RadioSource::Library, 1, "Celine", "t1", None, 0.99),
        make_cand_full(RadioSource::Library, 2, "Other", "t2", None, 0.10),
    ];
    let recent = vec!["Celine".to_string(), "Celine".to_string()];
    let out = enforce_artist_diversity(ranked, &recent, 2);
    assert_eq!(out[0].artist_name, "Other");
    assert_eq!(out[1].artist_name, "Celine");
}

#[test]
fn artist_cap_degrades_to_repetition_when_pool_is_single_artist() {
    // A one-artist pool must still fill the queue (better repetition than
    // silence) - the cap only bites when alternatives exist.
    let ranked = vec![
        make_cand_full(RadioSource::Library, 1, "Solo", "t1", None, 0.9),
        make_cand_full(RadioSource::Library, 2, "Solo", "t2", None, 0.8),
        make_cand_full(RadioSource::Library, 3, "Solo", "t3", None, 0.7),
        make_cand_full(RadioSource::Library, 4, "Solo", "t4", None, 0.6),
    ];
    let out = enforce_artist_diversity(ranked, &[], 4);
    assert_eq!(out.len(), 4);
    assert_eq!(out[0].title, "t1");
}

#[test]
fn artist_cap_ignores_blank_artists() {
    // Unattributable candidates can never form a run and never get blocked.
    let ranked = vec![
        make_cand_full(RadioSource::Lastfm, 0, "", "t1", None, 0.9),
        make_cand_full(RadioSource::Lastfm, 0, "", "t2", None, 0.8),
        make_cand_full(RadioSource::Lastfm, 0, "", "t3", None, 0.7),
    ];
    let out = enforce_artist_diversity(ranked, &[], 3);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].title, "t1");
}

#[test]
fn diversity_rerank_hard_skips_recent_tracks() {
    let cands = vec![
        make_cand_full(RadioSource::Library, 1, "A", "t1", None, 0.95),
        make_cand_full(RadioSource::Library, 2, "B", "t2", None, 0.40),
    ];
    let mut recent = HashSet::new();
    recent.insert(1);
    let profile = crate::services::radio_config::RadioProfile::mixed();
    let mut counters = RerankCounters::default();
    let queue = diversity_rerank(
        cands,
        &profile,
        RadioBlend::Mixed,
        2,
        &HashMap::new(),
        &recent,
        false,
        &mut counters,
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].track_id, 2);
    assert_eq!(counters.repetition_skips, 1);
}

#[test]
fn diversity_rerank_relaxes_when_all_scores_negative() {
    // Only one candidate, an artist match against the queue, with a
    // crushing penalty. Without relaxation it'd score below zero and never
    // be picked. With relaxation, the penalty drops and it gets picked.
    let mut counters = RerankCounters::default();
    let queue_already = vec![make_cand_full(
        RadioSource::Library,
        10,
        "Solo",
        "first",
        None,
        0.5,
    )];
    let mut cands = vec![make_cand_full(
        RadioSource::Library,
        11,
        "Solo",
        "second",
        None,
        0.05,
    )];
    let profile = crate::services::radio_config::RadioProfile {
        same_artist_penalty: 0.5,
        diversity_weight: 1.0,
        ..crate::services::radio_config::RadioProfile::mixed()
    };
    // Manually invoke the inner machinery: pre-populate queue, then call
    // the rerank with a one-candidate pool. (Easier than coaxing the full
    // function into emitting the same setup.)
    cands.extend(queue_already.clone());
    let queue = diversity_rerank(
        cands,
        &profile,
        RadioBlend::Mixed,
        2,
        &HashMap::new(),
        &HashSet::new(),
        false,
        &mut counters,
    );
    // Both got placed; the second slot relaxed past the artist penalty.
    assert_eq!(queue.len(), 2);
    assert!(counters.penalty_relaxations >= 1);
}

#[test]
fn diversity_rerank_genre_saturation_steers_toward_jazz_when_electronic_floods() {
    // 5 electronic candidates + 1 jazz. With genre threshold of 3 in last
    // 10, the first 4 electronic slots fill freely (the 4th is the slot
    // where excess just becomes 1). When the 5th slot evaluates, the
    // remaining electronic would score below jazz, so jazz gets picked.
    let cands: Vec<RadioCandidate> = (1..=6)
        .map(|i| {
            make_cand_full(
                RadioSource::Library,
                i,
                &format!("artist{i}"),
                &format!("t{i}"),
                None,
                0.5,
            )
        })
        .collect();
    let mut primary_genres = HashMap::new();
    for i in 1..=5 {
        primary_genres.insert(i, "electronic".to_string());
    }
    primary_genres.insert(6, "jazz".to_string());
    let profile = crate::services::radio_config::RadioProfile {
        genre_saturation_penalty: 1.0,
        same_artist_penalty: 0.0,
        same_album_penalty: 0.0,
        diversity_weight: 1.0,
        ..crate::services::radio_config::RadioProfile::mixed()
    };
    let mut counters = RerankCounters::default();
    let queue = diversity_rerank(
        cands,
        &profile,
        RadioBlend::Mixed,
        5,
        &primary_genres,
        &HashSet::new(),
        false,
        &mut counters,
    );
    let jazz_index = queue.iter().position(|c| c.track_id == 6);
    assert!(
        jazz_index.is_some(),
        "jazz candidate should be selected when electronic saturates"
    );
}

#[test]
fn diversity_rerank_counter_fires_when_penalty_applies_to_chosen() {
    // No alternative genre — every candidate is electronic. Once the queue
    // has 4+ electronic, every remaining pick triggers the saturation
    // penalty, so the counter increments on subsequent slots.
    let cands: Vec<RadioCandidate> = (1..=6)
        .map(|i| {
            make_cand_full(
                RadioSource::Library,
                i,
                &format!("artist{i}"),
                &format!("t{i}"),
                None,
                0.5,
            )
        })
        .collect();
    let primary_genres: HashMap<i64, String> =
        (1..=6).map(|i| (i, "electronic".to_string())).collect();
    let profile = crate::services::radio_config::RadioProfile {
        genre_saturation_penalty: 0.1,
        same_artist_penalty: 0.0,
        same_album_penalty: 0.0,
        diversity_weight: 1.0,
        ..crate::services::radio_config::RadioProfile::mixed()
    };
    let mut counters = RerankCounters::default();
    let _queue = diversity_rerank(
        cands,
        &profile,
        RadioBlend::Mixed,
        6,
        &primary_genres,
        &HashSet::new(),
        false,
        &mut counters,
    );
    assert!(
        counters.genre_saturation_penalties >= 2,
        "expected ≥2 penalty applications by slot 6 with all-electronic, got {}",
        counters.genre_saturation_penalties,
    );
}

#[test]
fn diversity_rerank_source_quota_bonus_promotes_underrepresented_source() {
    // 3 library candidates, 3 lastfm candidates, identical raw scores.
    // With Familiar blend (lib=0.60, lfm=0.30, eng=0.10) and the bonus
    // enabled, library should get 60% target → seed picks tilt library
    // until the quota balances. Specifically: slot 2 has lib_count=1,
    // target=0.60×1=0.60, actual=1 ≥ target → no bonus. lfm count=0,
    // target=0.30×1=0.30, actual=0 < target → +5%. So slot 2 should be lfm.
    let mut cands = Vec::new();
    for i in 1..=3 {
        cands.push(make_cand_full(
            RadioSource::Library,
            i,
            &format!("la{i}"),
            "t",
            None,
            0.5,
        ));
    }
    for i in 100..=102 {
        cands.push(make_cand_full(
            RadioSource::Lastfm,
            0,
            &format!("fa{i}"),
            "t",
            None,
            0.5,
        ));
    }
    let profile = crate::services::radio_config::RadioProfile {
        same_artist_penalty: 0.0,
        same_album_penalty: 0.0,
        genre_saturation_penalty: 0.0,
        diversity_weight: 1.0,
        ..crate::services::radio_config::RadioProfile::familiar()
    };
    let mut counters = RerankCounters::default();
    let queue = diversity_rerank(
        cands,
        &profile,
        RadioBlend::Familiar,
        6,
        &HashMap::new(),
        &HashSet::new(),
        true, // quota bonus on
        &mut counters,
    );
    // Final mix should reflect blend weights: with 6 slots and
    // (0.60, 0.30, 0.10), library ≈ 3-4 slots, lastfm ≈ 2 slots.
    let lib_count = queue
        .iter()
        .filter(|c| c.source == RadioSource::Library)
        .count();
    let lfm_count = queue
        .iter()
        .filter(|c| c.source == RadioSource::Lastfm)
        .count();
    assert!(
        (3..=4).contains(&lib_count),
        "expected ~3-4 library slots with source quota, got {lib_count}",
    );
    assert!(
        lfm_count >= 1,
        "lastfm should be present in queue under quota bonus, got {lfm_count}",
    );
}

#[test]
fn normalize_isolates_per_source() {
    // Library scores are tightly clustered around 0.7, lastfm around 0.3.
    // Without normalization, library's narrow band would lose to lastfm's
    // wider one in any rank-mixing step. After normalization, each source's
    // top should map to ~1.0 independent of the other.
    let mut cands: Vec<RadioCandidate> = Vec::new();
    let lib_scores = [0.65, 0.68, 0.70, 0.72, 0.75];
    for (i, &s) in lib_scores.iter().enumerate() {
        cands.push(make_cand(RadioSource::Library, 100 + i as i64, s));
    }
    let lfm_scores = [0.10, 0.20, 0.30, 0.40, 0.50];
    for (i, &s) in lfm_scores.iter().enumerate() {
        cands.push(make_cand(RadioSource::Lastfm, 200 + i as i64, s));
    }

    normalize_source_scores(&mut cands);

    let lib_top = cands
        .iter()
        .filter(|c| c.source == RadioSource::Library)
        .map(|c| c.similarity_score)
        .fold(0.0_f64, f64::max);
    let lfm_top = cands
        .iter()
        .filter(|c| c.source == RadioSource::Lastfm)
        .map(|c| c.similarity_score)
        .fold(0.0_f64, f64::max);
    assert!((lib_top - 1.0).abs() < 1e-6);
    assert!((lfm_top - 1.0).abs() < 1e-6);
}

#[test]
fn session_id_starts_with_rad_and_is_unique() {
    let a = new_session_id();
    // Force a tick so the nanos count differs.
    std::thread::sleep(std::time::Duration::from_nanos(1));
    let b = new_session_id();
    assert!(a.starts_with("rad_"));
    assert!(b.starts_with("rad_"));
    // Note: rare race could fail this — but `Duration::from_nanos(1)` plus the
    // syscall round-trip makes collision astronomically unlikely.
    assert_ne!(a, b, "session ids should differ across calls");
}

#[test]
fn radio_blend_default_is_mixed() {
    assert_eq!(RadioBlend::default(), RadioBlend::Mixed);
}

#[test]
fn radio_blend_serde_roundtrip() {
    for blend in [
        RadioBlend::Familiar,
        RadioBlend::Mixed,
        RadioBlend::Adventurous,
    ] {
        let s = serde_json::to_string(&blend).unwrap();
        let back: RadioBlend = serde_json::from_str(&s).unwrap();
        assert_eq!(blend, back);
    }
}
