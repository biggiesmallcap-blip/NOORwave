use super::*;

#[test]
fn cached_audio_features_reject_old_feature_version() {
    let rows = vec![queries::CachedAudioFeatureRow {
        track_id: 1,
        feature_version: "metadata-audio-proxy-v1".to_string(),
        vector_blob: pack_vector_f64(&[0.5, 0.5]),
        clip_start_ms: 0,
        clip_duration_ms: 20_000,
    }];

    let hydrated = hydrate_cached_audio_features(rows, 2, &HashSet::from([1]));

    assert!(
        hydrated.is_none(),
        "v1 proxy vectors must not be reused after v2 DSP token expansion"
    );
}

#[test]
fn cached_audio_features_reject_partial_track_coverage() {
    let rows = vec![queries::CachedAudioFeatureRow {
        track_id: 1,
        feature_version: AUDIO_PROXY_FEATURE_VERSION.to_string(),
        vector_blob: pack_vector_f64(&[0.5, 0.5]),
        clip_start_ms: 0,
        clip_duration_ms: 20_000,
    }];

    let hydrated = hydrate_cached_audio_features(rows, 2, &HashSet::from([1, 2]));

    assert!(
        hydrated.is_none(),
        "partial audio cache must recompute instead of silently dropping uncached tracks to behavioral-only fusion"
    );
}

#[test]
fn cached_audio_features_ignore_stale_rows_outside_training_corpus() {
    let rows = vec![
        queries::CachedAudioFeatureRow {
            track_id: 1,
            feature_version: AUDIO_PROXY_FEATURE_VERSION.to_string(),
            vector_blob: pack_vector_f64(&[0.5, 0.5]),
            clip_start_ms: 0,
            clip_duration_ms: 20_000,
        },
        queries::CachedAudioFeatureRow {
            track_id: 99,
            feature_version: AUDIO_PROXY_FEATURE_VERSION.to_string(),
            vector_blob: pack_vector_f64(&[0.25, 0.75]),
            clip_start_ms: 0,
            clip_duration_ms: 20_000,
        },
    ];

    let hydrated = hydrate_cached_audio_features(rows, 2, &HashSet::from([1]))
        .expect("stale cache row should not force recompute");

    assert_eq!(hydrated.len(), 1);
    assert!(hydrated.contains_key(&1));
    assert!(!hydrated.contains_key(&99));
}

#[test]
fn cached_audio_features_are_skipped_when_audio_rebuild_requested() {
    let medium = DiscoveryIntensity::Medium.params();
    let low = DiscoveryIntensity::Low.params();

    assert!(should_reuse_cached_audio_features(medium, false, false));
    assert!(!should_reuse_cached_audio_features(medium, false, true));
    assert!(!should_reuse_cached_audio_features(medium, true, false));
    assert!(!should_reuse_cached_audio_features(low, false, false));
}

#[test]
fn stored_neighbor_baseline_uses_same_typed_heldout_examples() {
    let mut grouped = HashMap::new();
    grouped.insert(
        1,
        vec![queries::EmbeddingNeighborRow {
            track_id: 2,
            title: "Target".to_string(),
            artist_name: None,
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            best_quality: None,
            score: 1.0,
            behavioral_score: 1.0,
            audio_score: 0.0,
            metadata_score: 0.0,
            reason_json: None,
            confidence: 1.0,
            support_count: 1,
            support_transition: 1.0,
            support_colisten: 0.0,
            support_structure: 0.0,
            support_metadata: 0.0,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
            play_count_seed: 0,
            play_count_candidate: 0,
            primary_reason: None,
        }],
    );
    let examples = vec![HeldoutExample {
        event_id: "transition:1".to_string(),
        from_track_id: 1,
        to_track_id: 2,
        evidence_kind: EvidenceKind::DirectTransition,
        weight: 1.0,
    }];

    let metrics = evaluate_stored_neighbors_for_heldout(&grouped, &examples);

    assert_eq!(metrics.get("baseline_heldout_count.transition"), Some(&1.0));
    assert_eq!(metrics.get("baseline_transition_recall_at_10"), Some(&1.0));
    assert_eq!(metrics.get("baseline_transition_mrr_at_20"), Some(&1.0));
}

#[test]
fn external_provider_refresh_budget_caps_full_runs() {
    let budget = plan_external_provider_refresh(true, None, 400);

    assert!(budget.should_refresh);
    assert_eq!(budget.seed_tracks, 100);
    assert_eq!(budget.lastfm_rows_per_seed, 20);
    assert_eq!(budget.tidal_new_release_rows, 500);
}

#[test]
fn discovery_engine_defaults_to_v2_and_round_trips_legacy_choice() {
    let db = Database::open_in_memory().expect("in-memory db");
    db.run_migrations().expect("migrations");

    assert_eq!(load_discovery_engine(&db), DiscoveryEngine::V2);

    set_discovery_engine(&db, DiscoveryEngine::V1).expect("set legacy engine");

    assert_eq!(load_discovery_engine(&db), DiscoveryEngine::V1);
    assert_eq!(load_discovery_engine(&db).family(), "discovery-fusion");
    assert!(!load_discovery_engine(&db).supports_training());
}

#[test]
fn training_safety_timeout_scales_by_intensity() {
    assert_eq!(
        discovery_training_safety_timeout(DiscoveryIntensity::Low).as_secs(),
        30 * 60
    );
    assert_eq!(
        discovery_training_safety_timeout(DiscoveryIntensity::Medium).as_secs(),
        30 * 60
    );
    assert_eq!(
        discovery_training_safety_timeout(DiscoveryIntensity::Max).as_secs(),
        60 * 60
    );
}

#[test]
fn training_worker_cap_adapts_by_safety_profile() {
    assert_eq!(
        discovery_training_worker_threads_for_available(
            DiscoveryTrainingSafetyProfile::LaptopSafe,
            16
        ),
        4
    );
    assert_eq!(
        discovery_training_worker_threads_for_available(
            DiscoveryTrainingSafetyProfile::Balanced,
            16
        ),
        8
    );
    assert_eq!(
        discovery_training_worker_threads_for_available(
            DiscoveryTrainingSafetyProfile::Performance,
            24
        ),
        16
    );
    assert_eq!(
        discovery_training_worker_threads_for_available(
            DiscoveryTrainingSafetyProfile::Balanced,
            2
        ),
        1
    );
}

#[test]
fn radio_from_neighbors_reports_learned_scores_without_mislabeling() {
    let db = Database::open_in_memory().expect("in-memory db");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id) VALUES (1, 'Seed', 1), (2, 'Next', 1)",
            [],
        )?;
        let model = queries::create_embedding_model(
            conn,
            "discovery-fusion-v2:radio-test",
            MODEL_FAMILY,
            8,
            "ready",
            None,
        )?;
        queries::activate_embedding_model(conn, model.id)?;
        conn.execute(
            "INSERT INTO track_neighbors
                (track_id, neighbor_track_id, model_id, rank, score, behavioral_score,
                 audio_score, metadata_score)
             VALUES (1, 2, ?1, 1, 0.9, 0.6, 0.7, 0.2)",
            rusqlite::params![model.id],
        )?;
        Ok(())
    })
    .expect("seed");

    let rows = radio_from_neighbors(&db, 1, &[], 5, 0.0)
        .expect("radio")
        .expect("active model");

    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.track_id, 2);
    assert_eq!(
        row.model_key.as_deref(),
        Some("discovery-fusion-v2:radio-test")
    );
    assert!((row.co_listen_score - 0.6).abs() < 1e-9);
    // Learned rows carry no album, artist or genre components.
    assert_eq!(row.co_album_score, 0.0);
    assert_eq!(row.co_artist_score, 0.0);
    assert_eq!(row.genre_proximity, 0.0);
}

#[test]
fn radio_creativity_prefers_further_neighbors() {
    let db = Database::open_in_memory().expect("in-memory db");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
        for id in 1..=6 {
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id) VALUES (?1, 'T', 1)",
                rusqlite::params![id],
            )?;
        }
        let model = queries::create_embedding_model(
            conn,
            "discovery-fusion-v2:creativity",
            MODEL_FAMILY,
            8,
            "ready",
            None,
        )?;
        queries::activate_embedding_model(conn, model.id)?;
        for (rank, (neighbor, score)) in [(2, 1.00), (3, 0.97), (4, 0.94), (5, 0.91), (6, 0.88)]
            .into_iter()
            .enumerate()
        {
            conn.execute(
                "INSERT INTO track_neighbors (track_id, neighbor_track_id, model_id, rank, score)
                 VALUES (1, ?1, ?2, ?3, ?4)",
                rusqlite::params![neighbor, model.id, rank as i64 + 1, score],
            )?;
        }
        Ok(())
    })
    .expect("seed");

    let order = |creativity: f64| {
        radio_from_neighbors(&db, 1, &[], 5, creativity)
            .expect("radio")
            .expect("model")
            .into_iter()
            .map(|row| row.track_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        order(0.0),
        vec![2, 3, 4, 5, 6],
        "no creativity keeps nearest first"
    );
    assert_ne!(
        order(0.5)[0],
        2,
        "high creativity reaches past the nearest neighbor"
    );
}

#[cfg(windows)]
#[test]
fn background_trainer_threads_really_run_at_low_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_NORMAL,
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .start_handler(|_| lower_current_thread_priority())
        .build()
        .expect("pool");
    // SAFETY: reads the scheduling priority of the calling pool thread.
    let priority = pool.install(|| unsafe { GetThreadPriority(GetCurrentThread()) });
    assert!(
        priority < THREAD_PRIORITY_NORMAL,
        "background trainer thread priority was {priority}"
    );
}

#[test]
fn background_training_stays_on_a_small_thread_budget() {
    assert_eq!(background_training_worker_threads_for_available(1), 1);
    assert_eq!(background_training_worker_threads_for_available(4), 1);
    assert_eq!(background_training_worker_threads_for_available(8), 2);
    assert_eq!(background_training_worker_threads_for_available(32), 2);
    assert_eq!(
        background_training_safety_timeout(DiscoveryIntensity::Medium),
        discovery_training_safety_timeout(DiscoveryIntensity::Medium) * 4
    );
}

#[test]
fn training_safety_profile_defaults_to_balanced_and_round_trips() {
    let db = Database::open_in_memory().expect("in-memory db");
    db.run_migrations().expect("migrations");

    assert_eq!(
        load_discovery_training_safety_profile(&db),
        DiscoveryTrainingSafetyProfile::Balanced
    );

    set_discovery_training_safety_profile(&db, DiscoveryTrainingSafetyProfile::Performance)
        .expect("set profile");

    assert_eq!(
        load_discovery_training_safety_profile(&db),
        DiscoveryTrainingSafetyProfile::Performance
    );
}

#[tokio::test]
async fn start_training_refuses_legacy_engine_without_starting_v2() {
    let db = Database::open_in_memory().expect("in-memory db");
    db.run_migrations().expect("migrations");
    set_discovery_engine(&db, DiscoveryEngine::V1).expect("select legacy engine");
    let (event_tx, _) = tokio::sync::broadcast::channel::<AppEvent>(1);

    let err = start_training(
        db.clone(),
        event_tx,
        false,
        false,
        false,
        Arc::new(AtomicBool::new(false)),
        ExternalProviderRefreshClients::default(),
    )
    .await
    .expect_err("legacy engine must not train through v2 path");

    assert!(
        err.to_string().contains("legacy discovery engine"),
        "unexpected error: {err}"
    );
    let model_count = db
        .with_conn(|conn| {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM embedding_models", [], |row| {
                    row.get(0)
                })?;
            Ok(count)
        })
        .expect("count models");
    assert_eq!(model_count, 0);
}

#[tokio::test]
async fn start_training_marks_run_and_model_failed_when_setup_errors_after_creation() {
    let db = Database::open_in_memory().expect("in-memory db");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| {
        conn.execute("DROP TABLE tracks", [])?;
        Ok(())
    })
    .expect("break trainer input setup");
    let (event_tx, _) = tokio::sync::broadcast::channel::<AppEvent>(1);

    let err = start_training(
        db.clone(),
        event_tx,
        false,
        false,
        false,
        Arc::new(AtomicBool::new(false)),
        ExternalProviderRefreshClients::default(),
    )
    .await
    .expect_err("broken setup should fail training");

    assert!(
        err.to_string().contains("no such table: tracks"),
        "unexpected error: {err}"
    );
    let (run_status, run_progress, run_error, model_status) = db
        .with_conn(|conn| {
            let run = queries::get_latest_training_run(conn)?.expect("training run should exist");
            let model_id = run.model_id.expect("training run should have model");
            let model_status: String = conn.query_row(
                "SELECT status FROM embedding_models WHERE id = ?1",
                [model_id],
                |row| row.get(0),
            )?;
            Ok((run.status, run.progress, run.error_text, model_status))
        })
        .expect("read failed run");

    assert_eq!(run_status, "failed");
    assert_eq!(run_progress, 0.05);
    assert!(
        run_error
            .as_deref()
            .is_some_and(|text| text.contains("no such table: tracks")),
        "run should store failure text, got {run_error:?}"
    );
    assert_eq!(model_status, "failed");
}

#[test]
fn external_provider_refresh_budget_skips_fresh_incremental_runs() {
    let now =
        chrono::NaiveDateTime::parse_from_str("2026-02-02 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
    let last_refresh =
        chrono::NaiveDateTime::parse_from_str("2026-02-02 01:00:00", "%Y-%m-%d %H:%M:%S").unwrap();

    let budget = plan_external_provider_refresh_at(false, Some(last_refresh), 400, now);

    assert!(!budget.should_refresh);
    assert_eq!(budget.seed_tracks, 0);
    assert_eq!(budget.lastfm_rows_per_seed, 0);
    assert_eq!(budget.tidal_new_release_rows, 0);
}

#[test]
fn external_provider_refresh_persists_lastfm_and_tidal_candidates() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    conn.execute(
        "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
         VALUES (1, 'Seed Track', 1, 200000, 101)",
        [],
    )
    .unwrap();
    let seeds = vec![EmbeddingTrackRow {
        track_id: 1,
        title: "Seed Track".to_string(),
        artist_name: Some("Seed Artist".to_string()),
        album_title: None,
        duration_ms: Some(200_000),
        best_quality: Some("LOSSLESS".to_string()),
        source: "tidal".to_string(),
        play_count: 0,
        is_favorite: false,
        playlist_memberships: 0,
        genre_paths: vec![],
        bpm: None,
        energy: None,
        camelot_key: None,
        danceability: None,
        beat_strength: None,
        loudness_lufs: None,
    }];
    let mut lastfm = HashMap::new();
    lastfm.insert(
        1,
        vec![ExternalLastfmCandidate {
            artist: "Similar Artist".to_string(),
            title: "Similar Track".to_string(),
            mbid: Some("mbid-1".to_string()),
            match_score: 0.91,
            branch_from: Some("Branch Artist - Branch Track".to_string()),
        }],
    );
    let tidal = vec![ExternalTidalCandidate {
        tidal_id: 9001,
        artist_name: "New Artist".to_string(),
        title: "New Track".to_string(),
        genre_tags: vec!["new-release".to_string()],
        duration_ms: Some(180_000),
    }];
    let mut tidal_similar = HashMap::new();
    tidal_similar.insert(
        1,
        vec![ExternalTidalCandidate {
            tidal_id: 9002,
            artist_name: "Similar Tidal Artist".to_string(),
            title: "Similar Tidal Track".to_string(),
            genre_tags: vec!["tidal-similar".to_string()],
            duration_ms: Some(181_000),
        }],
    );

    let report = persist_external_provider_refresh(
        &conn,
        &seeds,
        &lastfm,
        &tidal,
        &tidal_similar,
        chrono::NaiveDateTime::parse_from_str("2026-02-02 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
    )
    .unwrap();

    assert_eq!(report.candidates_upserted, 3);
    assert_eq!(report.sightings_upserted, 3);
    assert_eq!(report.lastfm_candidates_upserted, 1);
    assert_eq!(report.lastfm_sightings_upserted, 1);
    assert_eq!(report.tidal_new_release_candidates_upserted, 1);
    assert_eq!(report.tidal_new_release_sightings_upserted, 1);
    assert_eq!(report.tidal_similar_rows_seen, 1);
    assert_eq!(report.tidal_similar_candidates_upserted, 1);
    assert_eq!(report.tidal_similar_sightings_upserted, 1);
    let sources = conn
        .prepare("SELECT source FROM external_track_candidate_sightings ORDER BY source")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        sources,
        vec!["lastfm_similar", "tidal_new_release", "tidal_similar"]
    );
    let refresh_at: String = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'discovery_external_refresh_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(refresh_at, "2026-02-02 12:00:00");
}

#[test]
fn lastfm_branch_candidate_attenuates_parent_and_child_match() {
    let parent = ExternalLastfmCandidate {
        artist: "Parent Artist".to_string(),
        title: "Parent Track".to_string(),
        mbid: None,
        match_score: 0.8,
        branch_from: None,
    };
    let child = LastFmSimilarTrack {
        artist: "Child Artist".to_string(),
        title: "Child Track".to_string(),
        mbid: Some("child-mbid".to_string()),
        match_score: 0.5,
    };

    let branched = ExternalLastfmCandidate::branch_from(child, &parent);

    assert_eq!(branched.artist, "Child Artist");
    assert_eq!(branched.title, "Child Track");
    assert_eq!(branched.mbid.as_deref(), Some("child-mbid"));
    assert_eq!(
        branched.branch_from.as_deref(),
        Some("Parent Artist - Parent Track")
    );
    assert!((branched.match_score - 0.26).abs() < f64::EPSILON);
}

#[test]
fn lastfm_resolved_edges_weight_direct_above_branch() {
    let edges = lastfm_resolved_edges_from_rows(vec![
        queries::ResolvedLastfmExternalSightingRow {
            seed_track_id: 1,
            resolved_track_id: 2,
            similarity: 0.8,
            source_payload_json: Some(r#"{"match":0.8}"#.to_string()),
        },
        queries::ResolvedLastfmExternalSightingRow {
            seed_track_id: 1,
            resolved_track_id: 3,
            similarity: 0.8,
            source_payload_json: Some(
                r#"{"match":0.8,"branch_from":"Parent - Track"}"#.to_string(),
            ),
        },
    ]);

    let direct = edges
        .iter()
        .find(|edge| edge.evidence_kind == EvidenceKind::LastfmDirectSimilarity)
        .expect("direct edge");
    let branch = edges
        .iter()
        .find(|edge| edge.evidence_kind == EvidenceKind::LastfmBranchSimilarity)
        .expect("branch edge");
    assert_eq!(edges.len(), 2);
    assert!(direct.weight > branch.weight);
    assert_eq!(direct.from_track_id, 1);
    assert_eq!(direct.to_track_id, 2);
}

#[test]
fn lastfm_resolved_edges_dedupe_same_seed_and_track_by_kind() {
    let edges = lastfm_resolved_edges_from_rows(vec![
        queries::ResolvedLastfmExternalSightingRow {
            seed_track_id: 1,
            resolved_track_id: 2,
            similarity: 0.4,
            source_payload_json: Some(r#"{"match":0.4}"#.to_string()),
        },
        queries::ResolvedLastfmExternalSightingRow {
            seed_track_id: 1,
            resolved_track_id: 2,
            similarity: 0.9,
            source_payload_json: Some(r#"{"match":0.9}"#.to_string()),
        },
    ]);

    assert_eq!(edges.len(), 1);
    assert!((edges[0].weight - (0.9 * LASTFM_DIRECT_EDGE_WEIGHT)).abs() < f64::EPSILON);
}

fn tidal_search_track(
    id: i64,
    title: &str,
    artist_name: &str,
    duration_seconds: i64,
) -> TidalSearchTrack {
    TidalSearchTrack {
        id,
        title: title.to_string(),
        duration: duration_seconds,
        artist_name: Some(artist_name.to_string()),
        ..TidalSearchTrack::default()
    }
}

#[test]
fn tidal_resolution_exact_artist_title_match_resolves() {
    let decision = classify_tidal_search_resolution(
        "Heroes",
        "David Bowie",
        Some(183_000),
        &[tidal_search_track(42, "Heroes", "David Bowie", 183)],
    );

    assert!(matches!(
        decision,
        TidalSearchResolutionDecision::Resolved(candidate) if candidate.tidal_id == 42
    ));
}

#[test]
fn tidal_resolution_punctuation_and_case_differences_resolve() {
    let decision = classify_tidal_search_resolution(
        "B.O.B.",
        "OutKast",
        Some(304_000),
        &[tidal_search_track(43, "B O B", "OUTKAST", 305)],
    );

    assert!(matches!(
        decision,
        TidalSearchResolutionDecision::Resolved(candidate) if candidate.tidal_id == 43
    ));
}

#[test]
fn tidal_resolution_wrong_artist_or_title_rejects() {
    let wrong_artist = classify_tidal_search_resolution(
        "Teardrop",
        "Massive Attack",
        Some(330_000),
        &[tidal_search_track(44, "Teardrop", "Newton Faulkner", 330)],
    );
    let wrong_title = classify_tidal_search_resolution(
        "Teardrop",
        "Massive Attack",
        Some(330_000),
        &[tidal_search_track(45, "Angel", "Massive Attack", 330)],
    );

    assert_eq!(wrong_artist, TidalSearchResolutionDecision::Rejected);
    assert_eq!(wrong_title, TidalSearchResolutionDecision::Rejected);
}

#[test]
fn tidal_resolution_duration_mismatch_rejects() {
    let decision = classify_tidal_search_resolution(
        "Windowlicker",
        "Aphex Twin",
        Some(367_000),
        &[tidal_search_track(46, "Windowlicker", "Aphex Twin", 120)],
    );

    assert_eq!(decision, TidalSearchResolutionDecision::Rejected);
}

#[test]
fn tidal_resolution_multiple_strong_candidates_are_ambiguous() {
    let decision = classify_tidal_search_resolution(
        "Midnight City",
        "M83",
        Some(244_000),
        &[
            tidal_search_track(47, "Midnight City", "M83", 244),
            tidal_search_track(48, "Midnight City", "M83", 245),
        ],
    );

    assert_eq!(decision, TidalSearchResolutionDecision::Ambiguous);
}

#[test]
fn external_tidal_resolution_updates_sidecar_without_importing_track() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    let candidate = queries::upsert_external_track_candidate(
        &conn,
        &queries::ExternalTrackCandidateUpsert {
            tidal_id: None,
            mbid: None,
            dedupe_key: "lastfm:burial:archangel".to_string(),
            title: "Archangel".to_string(),
            artist_name: "Burial".to_string(),
            genre_tags_json: None,
            duration_ms: None,
            expires_at: "2026-03-01 00:00:00".to_string(),
        },
    )
    .unwrap();

    let updated = queries::resolve_external_candidate_tidal_metadata(
        &conn,
        candidate.id,
        &queries::ExternalCandidateTidalResolution {
            tidal_id: 9001,
            genre_tags_json: Some(r#"["dubstep"]"#.to_string()),
            duration_ms: Some(244_000),
        },
    )
    .unwrap();

    assert_eq!(updated.tidal_id, Some(9001));
    assert_eq!(updated.resolved_track_id, None);
    assert_eq!(updated.title, "Archangel");
    assert_eq!(updated.artist_name, "Burial");
    let track_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(track_count, 0);
}

#[test]
fn provider_rate_limit_errors_are_detected_for_cooldown() {
    let error = anyhow::anyhow!("Last.fm HTTP 429: too many requests");

    assert!(is_provider_rate_limit_error(&error));
}

#[test]
fn transition_source_weighting_prefers_manual_completed_edges() {
    let manual = transition_evidence_weight(Some("queue"), true, 1.0);
    let passive = transition_evidence_weight(Some("automix"), false, 1.0);

    assert!(manual > passive);
    assert!(passive < 1.0);
}

fn gate_metrics(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), *value))
        .collect()
}

#[test]
fn activation_accepts_an_improvement_below_the_old_absolute_floor() {
    // Model 17 on a real library: better than the active model on the same
    // held-out set, but under the old fixed 0.15 floor.
    let metrics = gate_metrics(&[
        ("coverage_ratio", 1.0),
        ("transition_recall_at_10", 0.131),
        ("baseline_transition_recall_at_10", 0.116),
        ("baseline_coverage_ratio", 1.0),
        (
            "baseline_trainer_config_version",
            TRAINER_CONFIG_VERSION as f64,
        ),
        ("evidence_count.listen_history", 5000.0),
    ]);
    assert!(should_activate_model(&metrics));
}

#[test]
fn activation_rejects_a_recall_regression_at_the_same_trainer_version() {
    let metrics = gate_metrics(&[
        ("coverage_ratio", 1.0),
        ("transition_recall_at_10", 0.110),
        ("baseline_transition_recall_at_10", 0.116),
        ("baseline_coverage_ratio", 1.0),
        (
            "baseline_trainer_config_version",
            TRAINER_CONFIG_VERSION as f64,
        ),
    ]);
    assert!(!should_activate_model(&metrics));
}

#[test]
fn activation_allows_a_small_dip_when_replacing_an_older_trainer_version() {
    let older = (TRAINER_CONFIG_VERSION - 1) as f64;
    let small_dip = gate_metrics(&[
        ("coverage_ratio", 1.0),
        ("transition_recall_at_10", 0.110),
        ("baseline_transition_recall_at_10", 0.116),
        ("baseline_trainer_config_version", older),
    ]);
    assert!(should_activate_model(&small_dip));
    let large_dip = gate_metrics(&[
        ("coverage_ratio", 1.0),
        ("transition_recall_at_10", 0.090),
        ("baseline_transition_recall_at_10", 0.116),
        ("baseline_trainer_config_version", older),
    ]);
    assert!(!should_activate_model(&large_dip));
}

#[test]
fn activation_rejects_a_coverage_loss() {
    let metrics = gate_metrics(&[
        ("coverage_ratio", 0.90),
        ("transition_recall_at_10", 0.20),
        ("baseline_transition_recall_at_10", 0.10),
        ("baseline_coverage_ratio", 0.97),
        (
            "baseline_trainer_config_version",
            TRAINER_CONFIG_VERSION as f64,
        ),
    ]);
    assert!(!should_activate_model(&metrics));
}

#[test]
fn first_activation_keeps_the_absolute_tiers() {
    let established = |recall: f64| {
        gate_metrics(&[
            ("coverage_ratio", 0.9),
            ("transition_recall_at_10", recall),
            ("evidence_count.listen_history", 100.0),
        ])
    };
    assert!(!should_activate_model(&established(0.12)));
    assert!(should_activate_model(&established(0.16)));
    let cold_start = gate_metrics(&[("coverage_ratio", 0.6)]);
    assert!(should_activate_model(&cold_start));
}

#[test]
fn trainer_config_version_defaults_to_one_for_old_models() {
    assert_eq!(trainer_config_version_from_json(None), 1);
    assert_eq!(
        trainer_config_version_from_json(Some(r#"{"trainer":"rust"}"#)),
        1
    );
    assert_eq!(
        trainer_config_version_from_json(Some(r#"{"trainer_config_version":3}"#)),
        3
    );
}
