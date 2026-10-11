use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// Gini coefficient of an ascending-sorted distribution: 0 = perfectly even,
// approaching 1 = all mass on a few elements. Used to quantify how hub-skewed
// an in-degree distribution is.
fn gini_coefficient(sorted_asc: &[i64]) -> f64 {
    let n = sorted_asc.len() as f64;
    let sum: i64 = sorted_asc.iter().sum();
    if n == 0.0 || sum == 0 {
        return 0.0;
    }
    let mut weighted = 0.0;
    for (i, &x) in sorted_asc.iter().enumerate() {
        weighted += (2.0 * (i as f64 + 1.0) - n - 1.0) * x as f64;
    }
    weighted / (n * sum as f64)
}

// Hub-concentration eval. Builds the audio-proxy embedding two ways - uniform
// tokens vs IDF-weighted tokens - computes each track's top-K nearest
// neighbours, and reports how skewed the resulting in-degree distribution is.
// The IDF version should flatten it (lower Gini / max in-degree / top-1% share):
// fewer "everyone's neighbour" hubs. Ignored: needs the real library and runs an
// O(n^2) neighbour pass over the sample.
#[test]
#[ignore]
fn eval_tfidf_reduces_embedding_hub_concentration() {
    use rusqlite::Connection;
    let db_path = crate::paths::resolve_db_path_from_env();
    let conn = Connection::open(&db_path).expect("open env db");
    let mut tracks =
        crate::db::queries::get_embedding_track_rows(&conn).expect("load embedding tracks");
    // Deterministic stride sample: spans the whole library (not one id range)
    // while keeping the O(n^2) pass tractable.
    const SAMPLE: usize = 4000;
    if tracks.len() > SAMPLE {
        let stride = (tracks.len() / SAMPLE).max(1);
        tracks = tracks.into_iter().step_by(stride).take(SAMPLE).collect();
    }
    let dim = 64usize;
    let k = 10usize;
    let idf = compute_token_idf(&tracks);

    // Empty IDF map == the original uniform projection (every weight 1.0).
    let unit_idf = HashMap::new();
    let uniform: Vec<(Option<String>, Vec<f64>)> = tracks
        .iter()
        .map(|t| {
            (
                t.artist_name.clone(),
                hashed_projection_weighted(&metadata_tokens(t), dim, &unit_idf),
            )
        })
        .collect();
    let weighted: Vec<(Option<String>, Vec<f64>)> = tracks
        .iter()
        .map(|t| {
            (
                t.artist_name.clone(),
                hashed_projection_weighted(&metadata_tokens(t), dim, &idf),
            )
        })
        .collect();

    for (label, emb) in [("uniform", &uniform), ("tfidf ", &weighted)] {
        let n = emb.len();
        let mut indeg = vec![0i64; n];
        for i in 0..n {
            let mut sims: Vec<(usize, f64)> = (0..n)
                .filter(|&j| j != i)
                .map(|j| (j, cosine(&emb[i].1, &emb[j].1)))
                .collect();
            sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for &(j, _) in sims.iter().take(k) {
                indeg[j] += 1;
            }
        }
        let total: i64 = indeg.iter().sum();
        let max = *indeg.iter().max().unwrap_or(&0);
        let orphans = indeg.iter().filter(|&&d| d == 0).count();
        let mut asc = indeg.clone();
        asc.sort();
        let gini = gini_coefficient(&asc);
        let top1 = ((n as f64) * 0.01).ceil() as usize;
        let top1_share = asc.iter().rev().take(top1).sum::<i64>() as f64 / (total.max(1) as f64);
        let mut by_artist: HashMap<String, i64> = HashMap::new();
        for (idx, (artist, _)) in emb.iter().enumerate() {
            *by_artist
                .entry(artist.clone().unwrap_or_default())
                .or_insert(0) += indeg[idx];
        }
        let mut artists: Vec<_> = by_artist.into_iter().collect();
        artists.sort_by_key(|a| std::cmp::Reverse(a.1));
        eprintln!(
            "[{label}] n={n} max_indeg={max} gini={gini:.3} orphans={orphans} top1%_share={top1_share:.3}"
        );
        for (name, deg) in artists.iter().take(6) {
            eprintln!("    {name}: {deg}");
        }
    }
}

fn make_test_input(
    track_count: usize,
    dim: usize,
) -> (
    Vec<EmbeddingTrackRow>,
    HashMap<i64, Vec<f64>>,
    HashMap<i64, TrainerAudioFeature>,
    HashMap<i64, Vec<f64>>,
) {
    let tracks: Vec<EmbeddingTrackRow> = (0..track_count as i64)
        .map(|i| EmbeddingTrackRow {
            track_id: i,
            title: format!("track_{i}"),
            artist_name: Some(format!("artist_{}", i / 10)),
            album_title: None,
            duration_ms: Some(180_000),
            best_quality: None,
            source: "local".to_string(),
            play_count: 0,
            is_favorite: false,
            playlist_memberships: 0,
            genre_paths: Vec::new(),
            bpm: None,
            energy: None,
            camelot_key: None,
            danceability: None,
            beat_strength: None,
            loudness_lufs: None,
        })
        .collect();

    let unit = 1.0_f64 / (dim as f64).sqrt();
    let behavioral: HashMap<i64, Vec<f64>> = tracks
        .iter()
        .map(|t| (t.track_id, vec![unit; dim]))
        .collect();
    let audio: HashMap<i64, TrainerAudioFeature> = tracks
        .iter()
        .map(|t| {
            (
                t.track_id,
                TrainerAudioFeature {
                    vector: vec![unit; dim],
                    clip_start_ms: 0,
                    clip_duration_ms: 20_000,
                    feature_version: "test".to_string(),
                },
            )
        })
        .collect();
    let fusion: HashMap<i64, Vec<f64>> = tracks
        .iter()
        .map(|t| (t.track_id, vec![unit; dim]))
        .collect();

    (tracks, behavioral, audio, fusion)
}

#[test]
fn similarity_neighbors_aborts_when_cancel_flag_set() {
    let (tracks, behavioral, audio, fusion) = make_test_input(200, 32);
    let cancel = Arc::new(AtomicBool::new(true));
    let co_score = HashMap::new();
    let co_count = HashMap::new();
    let support_buckets = HashMap::new();
    let play_counts = HashMap::new();

    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &co_score,
        &co_count,
        &support_buckets,
        &play_counts,
        10,
        None,
        Some(&cancel),
    );

    assert!(
        result.is_empty(),
        "expected zero neighbors when cancel is pre-set, got {}",
        result.len(),
    );
}

#[test]
fn metadata_tokens_include_expanded_dsp_buckets() {
    let track = EmbeddingTrackRow {
        track_id: 1,
        title: "Pulse Test".to_string(),
        artist_name: Some("NOOR".to_string()),
        album_title: None,
        duration_ms: Some(180_000),
        best_quality: Some("HI_RES".to_string()),
        source: "tidal".to_string(),
        play_count: 0,
        is_favorite: false,
        playlist_memberships: 0,
        genre_paths: Vec::new(),
        bpm: Some(123.0),
        energy: Some(0.74),
        camelot_key: Some("8B".to_string()),
        danceability: Some(0.83),
        beat_strength: Some(0.42),
        loudness_lufs: Some(-11.2),
    };

    let tokens = metadata_tokens(&track);

    assert!(tokens.iter().any(|token| token == "dance_8"));
    assert!(tokens.iter().any(|token| token == "beat_4"));
    assert!(tokens.iter().any(|token| token == "lufs_-12"));
}

#[test]
fn idf_down_weights_library_wide_tokens() {
    let mk = |id: i64, title: &str, genres: &[&str]| EmbeddingTrackRow {
        track_id: id,
        title: title.to_string(),
        artist_name: Some(format!("artist{id}")),
        album_title: None,
        duration_ms: None,
        best_quality: None,
        source: "tidal".to_string(),
        play_count: 0,
        is_favorite: false,
        playlist_memberships: 0,
        genre_paths: genres.iter().map(|g| g.to_string()).collect(),
        bpm: None,
        energy: None,
        camelot_key: None,
        danceability: None,
        beat_strength: None,
        loudness_lufs: None,
    };
    // "common" tags every track; "rare" tags one. "tidal" source is on all.
    let tracks = vec![
        mk(1, "a", &["common"]),
        mk(2, "b", &["common"]),
        mk(3, "c", &["common"]),
        mk(4, "d", &["common", "rare"]),
    ];
    let idf = compute_token_idf(&tracks);
    assert!(
        idf["genre:rare"] > idf["genre:common"],
        "a rare token must out-weigh a library-wide one"
    );
    // A token present in every track floors at weight 1.0.
    assert!((idf["genre:common"] - 1.0).abs() < 1e-9);
    assert!(
        !idf.contains_key("tidal"),
        "source words are not metadata tokens"
    );

    // Applying the weights actually moves the projection away from the uniform one.
    let dim = 32;
    let tokens = metadata_tokens(&tracks[3]);
    let unit = hashed_projection_weighted(&tokens, dim, &HashMap::new());
    let weighted = hashed_projection_weighted(&tokens, dim, &idf);
    assert!(
        cosine(&unit, &weighted) < 0.9999,
        "idf weighting should change the embedding"
    );
}

#[test]
fn lastfm_evidence_tags_are_directional() {
    let (tracks, behavioral, audio, fusion) = make_test_input(2, 32);
    let co_score = HashMap::new();
    let co_count = HashMap::new();
    let mut support_buckets: HashMap<i64, HashMap<i64, SupportBreakdown>> = HashMap::new();
    support_buckets.entry(0).or_default().insert(
        1,
        SupportBreakdown {
            lastfm_direct: 0.44,
            ..Default::default()
        },
    );
    let play_counts = HashMap::new();

    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &co_score,
        &co_count,
        &support_buckets,
        &play_counts,
        1,
        None,
        None,
    );

    let forward = result
        .iter()
        .find(|row| row.track_id == 0 && row.neighbor_track_id == 1)
        .expect("forward neighbor");
    let reverse = result
        .iter()
        .find(|row| row.track_id == 1 && row.neighbor_track_id == 0)
        .expect("reverse neighbor");
    assert!(forward.reason_tags.iter().any(|tag| tag == "lastfm_direct"));
    assert!(!reverse.reason_tags.iter().any(|tag| tag == "lastfm_direct"));
}

#[test]
fn similarity_neighbors_runs_normally_without_cancel() {
    let (tracks, behavioral, audio, fusion) = make_test_input(50, 32);
    let cancel = Arc::new(AtomicBool::new(false));
    let co_score = HashMap::new();
    let co_count = HashMap::new();
    let support_buckets = HashMap::new();
    let play_counts = HashMap::new();

    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &co_score,
        &co_count,
        &support_buckets,
        &play_counts,
        10,
        None,
        Some(&cancel),
    );

    // 50 tracks × top_k=10, all vectors identical → every track has 10 neighbors
    assert_eq!(result.len(), 500, "expected 50*10 = 500 neighbor rows");
}

#[test]
fn direct_transition_support_boosts_only_seed_to_candidate_direction() {
    let (tracks, behavioral, audio, fusion) = make_test_input(4, 16);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut co_score: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    co_score.entry(1).or_default().insert(2, 8.0);
    let mut co_count: HashMap<i64, HashMap<i64, i64>> = HashMap::new();
    co_count.entry(1).or_default().insert(2, 8);
    let mut support_buckets: HashMap<i64, HashMap<i64, SupportBreakdown>> = HashMap::new();
    support_buckets.entry(1).or_default().insert(
        2,
        SupportBreakdown {
            transition: 8.0,
            ..Default::default()
        },
    );
    let play_counts = HashMap::new();

    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &co_score,
        &co_count,
        &support_buckets,
        &play_counts,
        3,
        None,
        Some(&cancel),
    );

    let seed_one = result
        .iter()
        .find(|n| n.track_id == 1 && n.rank == 1)
        .expect("seed 1 top neighbor");
    assert_eq!(seed_one.neighbor_track_id, 2);

    let reverse = result
        .iter()
        .find(|n| n.track_id == 2 && n.neighbor_track_id == 1)
        .expect("reverse edge still present");
    let unrelated = result
        .iter()
        .find(|n| n.track_id == 2 && n.neighbor_track_id == 3)
        .expect("unrelated edge present");
    assert!(
        reverse.score <= unrelated.score,
        "reverse direction must not inherit the direct transition bonus"
    );
}

#[test]
fn typed_direct_evidence_builds_one_way_weighted_support() {
    let (tracks, _, _, _) = make_test_input(3, 16);
    let input = TrainerInput {
        seed: 13,
        dimension: 16,
        window_size: 3,
        min_count: 1,
        top_k: 3,
        include_audio_proxy: false,
        tracks,
        external_candidates: Vec::new(),
        sequences: Vec::new(),
        evidence_groups: vec![TrainerEvidenceGroup {
            label: "direct".to_string(),
            base_weight: 2.0,
            edges: vec![TrainerEdge {
                event_id: "transition:1".to_string(),
                from_track_id: 1,
                to_track_id: 2,
                weight: 1.0,
                evidence_kind: EvidenceKind::DirectTransition,
            }],
        }],
        heldout_pairs: Vec::new(),
        heldout_examples: Vec::new(),
        cached_audio_features: None,
    };

    let (_, co_score, co_count, support_buckets) = build_behavioral_embeddings(&input, None, None);

    assert_eq!(co_score.get(&1).and_then(|m| m.get(&2)).copied(), Some(2.0));
    assert_eq!(co_count.get(&1).and_then(|m| m.get(&2)).copied(), Some(1));
    assert_eq!(
        support_buckets
            .get(&1)
            .and_then(|m| m.get(&2))
            .map(|support| support.transition),
        Some(2.0)
    );
    assert!(
        co_score.get(&2).and_then(|m| m.get(&1)).is_none(),
        "direct transitions must not create reverse evidence"
    );
}

#[test]
fn heldout_exclusion_removes_only_matching_event_evidence() {
    let (tracks, _, _, _) = make_test_input(3, 16);
    let input = TrainerInput {
        seed: 13,
        dimension: 16,
        window_size: 3,
        min_count: 1,
        top_k: 3,
        include_audio_proxy: false,
        tracks,
        external_candidates: Vec::new(),
        sequences: Vec::new(),
        evidence_groups: vec![
            TrainerEvidenceGroup {
                label: "direct".to_string(),
                base_weight: 2.0,
                edges: vec![TrainerEdge {
                    event_id: "transition:1".to_string(),
                    from_track_id: 1,
                    to_track_id: 2,
                    weight: 1.0,
                    evidence_kind: EvidenceKind::DirectTransition,
                }],
            },
            TrainerEvidenceGroup {
                label: "colisten".to_string(),
                base_weight: 1.0,
                edges: vec![TrainerEdge {
                    event_id: "pair:1:2".to_string(),
                    from_track_id: 1,
                    to_track_id: 2,
                    weight: 0.5,
                    evidence_kind: EvidenceKind::SessionCoListen,
                }],
            },
        ],
        heldout_pairs: Vec::new(),
        heldout_examples: vec![HeldoutExample {
            event_id: "transition:1".to_string(),
            from_track_id: 1,
            to_track_id: 2,
            evidence_kind: EvidenceKind::DirectTransition,
            weight: 1.0,
        }],
        cached_audio_features: None,
    };

    let (_, co_score, co_count, support_buckets) = build_behavioral_embeddings(&input, None, None);

    assert_eq!(co_score.get(&1).and_then(|m| m.get(&2)).copied(), Some(0.5));
    assert_eq!(co_score.get(&2).and_then(|m| m.get(&1)).copied(), Some(0.5));
    assert_eq!(co_count.get(&1).and_then(|m| m.get(&2)).copied(), Some(1));
    let support = support_buckets
        .get(&1)
        .and_then(|m| m.get(&2))
        .copied()
        .unwrap_or_default();
    assert_eq!(support.transition, 0.0);
    assert_eq!(support.colisten, 0.5);
}

#[test]
fn typed_heldout_metrics_are_labeled_by_evidence_kind() {
    let neighbors = vec![TrainerNeighbor {
        track_id: 1,
        neighbor_track_id: 2,
        rank: 1,
        score: 1.0,
        behavioral_score: 1.0,
        audio_score: 0.0,
        metadata_score: 0.0,
        reason_tags: vec!["direct_transition".to_string()],
        primary_reason: Some("direct_transition".to_string()),
        confidence: 1.0,
        support_count: 1,
        support_transition: 2.0,
        support_colisten: 0.0,
        support_structure: 0.0,
        support_metadata: 0.0,
        play_count_seed: 0,
        play_count_candidate: 0,
        candidate_in_degree: 0,
        candidate_in_degree_percentile: 0.0,
    }];
    let examples = vec![HeldoutExample {
        event_id: "transition:1".to_string(),
        from_track_id: 1,
        to_track_id: 2,
        evidence_kind: EvidenceKind::DirectTransition,
        weight: 1.0,
    }];

    let metrics = evaluate_typed_heldout(&neighbors, &examples, &HashMap::new());

    assert_eq!(metrics.get("heldout_count.transition").copied(), Some(1.0));
    assert_eq!(metrics.get("transition_recall_at_10").copied(), Some(1.0));
    assert_eq!(metrics.get("transition_mrr_at_20").copied(), Some(1.0));
    assert!(
        !metrics.contains_key("colisten_recall_at_10"),
        "typed metrics must not be computed from unrelated or unlabeled pairs"
    );
}

#[test]
fn typed_diagnostics_include_skip_aware_and_cold_track_recall() {
    let neighbors = vec![
        TrainerNeighbor {
            track_id: 1,
            neighbor_track_id: 2,
            rank: 1,
            score: 1.0,
            behavioral_score: 1.0,
            audio_score: 0.0,
            metadata_score: 0.0,
            reason_tags: vec!["session_colisten".to_string()],
            primary_reason: Some("session_colisten".to_string()),
            confidence: 1.0,
            support_count: 1,
            support_transition: 0.0,
            support_colisten: 0.8,
            support_structure: 0.0,
            support_metadata: 0.0,
            play_count_seed: 10,
            play_count_candidate: 0,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
        },
        TrainerNeighbor {
            track_id: 1,
            neighbor_track_id: 3,
            rank: 2,
            score: 0.9,
            behavioral_score: 0.9,
            audio_score: 0.0,
            metadata_score: 0.0,
            reason_tags: vec!["session_colisten".to_string()],
            primary_reason: Some("session_colisten".to_string()),
            confidence: 0.8,
            support_count: 1,
            support_transition: 0.0,
            support_colisten: 0.1,
            support_structure: 0.0,
            support_metadata: 0.0,
            play_count_seed: 10,
            play_count_candidate: 5,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
        },
    ];
    let examples = vec![
        HeldoutExample {
            event_id: "full-listen".to_string(),
            from_track_id: 1,
            to_track_id: 2,
            evidence_kind: EvidenceKind::SessionCoListen,
            weight: 0.9,
        },
        HeldoutExample {
            event_id: "skip".to_string(),
            from_track_id: 1,
            to_track_id: 4,
            evidence_kind: EvidenceKind::SessionCoListen,
            weight: 0.1,
        },
    ];
    let play_counts = HashMap::from([(2, 0), (4, 0)]);

    let metrics = evaluate_typed_heldout(&neighbors, &examples, &play_counts);

    assert_eq!(metrics.get("skipped_input_rows").copied(), Some(1.0));
    assert_eq!(metrics.get("skip_aware_recall_at_10").copied(), Some(0.9));
    assert_eq!(metrics.get("cold_track_recall_at_10").copied(), Some(0.5));
}

#[test]
fn discovery_lift_counts_low_play_neighbor_share() {
    let neighbors = vec![
        TrainerNeighbor {
            track_id: 1,
            neighbor_track_id: 2,
            rank: 1,
            score: 1.0,
            behavioral_score: 0.0,
            audio_score: 1.0,
            metadata_score: 0.0,
            reason_tags: Vec::new(),
            primary_reason: None,
            confidence: 0.5,
            support_count: 0,
            support_transition: 0.0,
            support_colisten: 0.0,
            support_structure: 0.0,
            support_metadata: 1.0,
            play_count_seed: 9,
            play_count_candidate: 0,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
        },
        TrainerNeighbor {
            track_id: 1,
            neighbor_track_id: 3,
            rank: 2,
            score: 0.9,
            behavioral_score: 0.0,
            audio_score: 0.9,
            metadata_score: 0.0,
            reason_tags: Vec::new(),
            primary_reason: None,
            confidence: 0.5,
            support_count: 0,
            support_transition: 0.0,
            support_colisten: 0.0,
            support_structure: 0.0,
            support_metadata: 1.0,
            play_count_seed: 9,
            play_count_candidate: 8,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
        },
    ];

    let metrics = evaluate_discovery_lift(&neighbors);

    assert_eq!(
        metrics.get("discovery_lift.low_play_neighbor_share_at_10"),
        Some(&0.5)
    );
    assert_eq!(
        metrics.get("discovery_lift.never_played_neighbor_share_at_10"),
        Some(&0.5)
    );
}

#[test]
fn external_candidates_emit_sidecar_neighbors_without_library_rows() {
    let track = EmbeddingTrackRow {
        track_id: 1,
        title: "Signal Bloom".to_string(),
        artist_name: Some("Outside Artist".to_string()),
        album_title: None,
        duration_ms: Some(180_000),
        best_quality: Some("HI_RES".to_string()),
        source: "tidal".to_string(),
        play_count: 0,
        is_favorite: false,
        playlist_memberships: 0,
        genre_paths: vec!["electronic > ambient".to_string()],
        bpm: Some(120.0),
        energy: Some(0.5),
        camelot_key: None,
        danceability: Some(0.6),
        beat_strength: Some(0.4),
        loudness_lufs: Some(-12.0),
    };
    let input = TrainerInput {
        seed: 13,
        dimension: 32,
        window_size: 3,
        min_count: 1,
        top_k: 3,
        include_audio_proxy: true,
        tracks: vec![track],
        external_candidates: vec![TrainerExternalCandidate {
            candidate_id: 99,
            tidal_id: Some(90099),
            title: "Signal Bloom".to_string(),
            artist_name: "Outside Artist".to_string(),
            genre_tags: vec!["electronic".to_string(), "ambient".to_string()],
            source_tags: vec!["lastfm_similar".to_string()],
            freshness_bucket: Some("fresh_30d".to_string()),
            duration_ms: Some(180_000),
        }],
        sequences: Vec::new(),
        evidence_groups: Vec::new(),
        heldout_pairs: Vec::new(),
        heldout_examples: Vec::new(),
        cached_audio_features: None,
    };

    let output = run_discovery_training(input, None, None);

    assert!(output.neighbors.is_empty());
    assert_eq!(output.external_neighbors.len(), 1);
    assert_eq!(output.external_neighbors[0].library_track_id, 1);
    assert_eq!(output.external_neighbors[0].candidate_id, 99);
    assert_eq!(output.external_neighbors[0].rank, 1);
    assert!(
        output.external_neighbors[0]
            .reason_tags
            .iter()
            .any(|tag| tag == "external_audio_proxy")
    );
}

#[test]
fn external_candidate_tokens_include_provenance_and_freshness() {
    let candidate = TrainerExternalCandidate {
        candidate_id: 77,
        tidal_id: None,
        title: "Future Signal".to_string(),
        artist_name: "Outside Artist".to_string(),
        genre_tags: vec!["ambient".to_string()],
        source_tags: vec![
            "lastfm_similar".to_string(),
            "tidal_new_release".to_string(),
        ],
        freshness_bucket: Some("fresh_7d".to_string()),
        duration_ms: Some(181_000),
    };

    let tokens = external_candidate_tokens(&candidate);

    assert!(tokens.iter().any(|token| token == "source_lastfm_similar"));
    assert!(
        tokens
            .iter()
            .any(|token| token == "source_tidal_new_release")
    );
    assert!(tokens.iter().any(|token| token == "freshness_fresh_7d"));
}

#[test]
fn confidence_floors_for_metadata_only_edges() {
    // No co_count entries → every edge is "pure metadata", confidence = 0.25.
    let (tracks, behavioral, audio, fusion) = make_test_input(20, 16);
    let cancel = Arc::new(AtomicBool::new(false));
    let co_score = HashMap::new();
    let co_count = HashMap::new();
    let support_buckets = HashMap::new();
    let play_counts = HashMap::new();

    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &co_score,
        &co_count,
        &support_buckets,
        &play_counts,
        5,
        None,
        Some(&cancel),
    );

    assert!(!result.is_empty());
    assert!(
        result.iter().all(|n| (n.confidence - 0.25).abs() < 1e-6),
        "all metadata-only edges should sit at the 0.25 confidence floor",
    );
    assert!(result.iter().all(|n| n.support_count == 0));
}

#[test]
fn confidence_grows_with_support() {
    let (tracks, behavioral, audio, fusion) = make_test_input(10, 16);
    let cancel = Arc::new(AtomicBool::new(false));
    // Edge from track 0 → track 1 has 50 supporting events; everyone else has 0.
    let mut co_count: HashMap<i64, HashMap<i64, i64>> = HashMap::new();
    co_count
        .entry(tracks[0].track_id)
        .or_default()
        .insert(tracks[1].track_id, 50);
    let mut co_score: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    co_score
        .entry(tracks[0].track_id)
        .or_default()
        .insert(tracks[1].track_id, 50.0);
    let support_buckets = HashMap::new();
    let play_counts = HashMap::new();

    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &co_score,
        &co_count,
        &support_buckets,
        &play_counts,
        5,
        None,
        Some(&cancel),
    );

    let strongly_supported = result
        .iter()
        .find(|n| n.track_id == tracks[0].track_id && n.neighbor_track_id == tracks[1].track_id)
        .expect("edge present");
    assert!(
        strongly_supported.confidence > 0.7,
        "expected strong-evidence edge confidence > 0.7, got {}",
        strongly_supported.confidence,
    );
    assert_eq!(strongly_supported.support_count, 50);
}

#[test]
fn reason_hit_rates_bucket_by_primary_reason() {
    let mk = |seed: i64, neighbor: i64, primary: &str| TrainerNeighbor {
        track_id: seed,
        neighbor_track_id: neighbor,
        rank: 1,
        score: 0.0,
        behavioral_score: 0.0,
        audio_score: 0.0,
        metadata_score: 0.0,
        reason_tags: vec![primary.to_string()],
        primary_reason: Some(primary.to_string()),
        confidence: 0.0,
        support_count: 0,
        support_transition: 0.0,
        support_colisten: 0.0,
        support_structure: 0.0,
        support_metadata: 0.0,
        play_count_seed: 0,
        play_count_candidate: 0,
        candidate_in_degree: 0,
        candidate_in_degree_percentile: 0.0,
    };
    // Seed 1 has neighbors [10 behavioral, 20 harmonic_match, 30 behavioral].
    // Held-out target for seed 1 is 10 → behavioral hits, harmonic misses.
    let neighbors = vec![
        mk(1, 10, "behavioral"),
        mk(1, 20, "harmonic_match"),
        mk(1, 30, "behavioral"),
    ];
    let heldout = vec![(1, 10)];
    let rates = compute_reason_hit_rates(&neighbors, &heldout);

    let beh = rates
        .iter()
        .find(|r| r.primary_reason == "behavioral")
        .unwrap();
    assert_eq!(beh.impressions, 2);
    assert_eq!(beh.hits, 1);
    assert!((beh.hit_rate - 0.5).abs() < 1e-9);

    let harm = rates
        .iter()
        .find(|r| r.primary_reason == "harmonic_match")
        .unwrap();
    assert_eq!(harm.impressions, 1);
    assert_eq!(harm.hits, 0);
    assert_eq!(harm.hit_rate, 0.0);
}

#[test]
fn reason_hit_rates_flag_insufficient_data() {
    // 1 impression < MIN_REASON_IMPRESSIONS (20) → insufficient_data = true.
    let mk = |seed: i64, neighbor: i64, primary: &str| TrainerNeighbor {
        track_id: seed,
        neighbor_track_id: neighbor,
        rank: 1,
        score: 0.0,
        behavioral_score: 0.0,
        audio_score: 0.0,
        metadata_score: 0.0,
        reason_tags: vec![primary.to_string()],
        primary_reason: Some(primary.to_string()),
        confidence: 0.0,
        support_count: 0,
        support_transition: 0.0,
        support_colisten: 0.0,
        support_structure: 0.0,
        support_metadata: 0.0,
        play_count_seed: 0,
        play_count_candidate: 0,
        candidate_in_degree: 0,
        candidate_in_degree_percentile: 0.0,
    };
    let neighbors = vec![mk(1, 10, "rare_tag")];
    let heldout = vec![(1, 10)];
    let rates = compute_reason_hit_rates(&neighbors, &heldout);
    assert!(rates[0].insufficient_data);
}

#[test]
fn compute_in_degree_uses_average_rank_for_ties() {
    // Hand-rolled neighbor list: candidate A has in-degree 1, B has 1, C has 3.
    let mk = |track_id: i64, neighbor_id: i64| TrainerNeighbor {
        track_id,
        neighbor_track_id: neighbor_id,
        rank: 1,
        score: 0.0,
        behavioral_score: 0.0,
        audio_score: 0.0,
        metadata_score: 0.0,
        reason_tags: vec![],
        primary_reason: None,
        confidence: 0.0,
        support_count: 0,
        support_transition: 0.0,
        support_colisten: 0.0,
        support_structure: 0.0,
        support_metadata: 0.0,
        play_count_seed: 0,
        play_count_candidate: 0,
        candidate_in_degree: 0,
        candidate_in_degree_percentile: 0.0,
    };
    let mut neighbors = vec![
        mk(1, 100), // A
        mk(2, 200), // B
        mk(3, 300), // C
        mk(4, 300), // C
        mk(5, 300), // C
    ];
    compute_in_degree(&mut neighbors);

    let pct_a = neighbors
        .iter()
        .find(|n| n.neighbor_track_id == 100)
        .unwrap()
        .candidate_in_degree_percentile;
    let pct_b = neighbors
        .iter()
        .find(|n| n.neighbor_track_id == 200)
        .unwrap()
        .candidate_in_degree_percentile;
    let pct_c = neighbors
        .iter()
        .find(|n| n.neighbor_track_id == 300)
        .unwrap()
        .candidate_in_degree_percentile;

    // 3 distinct tracks, ranks: A,B share count 1 (avg 0.5), C alone at count 3 (rank 2).
    // percentile = avg_rank / N where N=3.
    assert!((pct_a - 0.5 / 3.0).abs() < 1e-9, "A pct = {}", pct_a);
    assert!((pct_b - 0.5 / 3.0).abs() < 1e-9, "B pct = {}", pct_b);
    assert!((pct_c - 2.0 / 3.0).abs() < 1e-9, "C pct = {}", pct_c);

    let in_a = neighbors
        .iter()
        .find(|n| n.neighbor_track_id == 100)
        .unwrap()
        .candidate_in_degree;
    let in_c = neighbors
        .iter()
        .find(|n| n.neighbor_track_id == 300)
        .unwrap()
        .candidate_in_degree;
    assert_eq!(in_a, 1);
    assert_eq!(in_c, 3);
}

fn behavioral_input(sequences: Vec<Vec<i64>>) -> TrainerInput {
    let (tracks, _, _, _) = make_test_input(25, 96);
    TrainerInput {
        seed: 13,
        dimension: 96,
        window_size: 1,
        min_count: 1,
        top_k: 3,
        include_audio_proxy: false,
        tracks,
        external_candidates: Vec::new(),
        sequences: vec![TrainerSequenceGroup {
            label: "listen_history".to_string(),
            weight: 1.0,
            sequences,
        }],
        evidence_groups: Vec::new(),
        heldout_pairs: Vec::new(),
        heldout_examples: Vec::new(),
        cached_audio_features: None,
    }
}

#[test]
fn behavioral_vectors_match_tracks_heard_in_the_same_contexts() {
    // 1 and 2 never meet, but both sit between 10 and 11. 3 sits elsewhere.
    let input = behavioral_input(vec![vec![10, 1, 11], vec![10, 2, 11], vec![20, 3, 21]]);
    let (embeddings, _, _, _) = build_behavioral_embeddings(&input, None, None);
    let shared = cosine(&embeddings[&1], &embeddings[&2]);
    let unrelated = cosine(&embeddings[&1], &embeddings[&3]);
    assert!(shared > 0.5, "shared-context cosine was {shared}");
    assert!(unrelated.abs() < 0.3, "unrelated cosine was {unrelated}");
}

#[test]
fn behavioral_vectors_match_a_direct_co_listen() {
    let input = behavioral_input(vec![vec![1, 2]]);
    let (embeddings, _, _, _) = build_behavioral_embeddings(&input, None, None);
    let direct = cosine(&embeddings[&1], &embeddings[&2]);
    assert!(direct > 0.9, "direct co-listen cosine was {direct}");
}

#[test]
fn genre_branch_requires_shared_genre_paths() {
    let (mut tracks, behavioral, audio, fusion) = make_test_input(3, 16);
    for track in &mut tracks {
        track.best_quality = Some("LOSSLESS".to_string());
        track.source = "tidal".to_string();
    }
    tracks[0].genre_paths = vec!["Electronic > House".to_string()];
    tracks[1].genre_paths = vec!["Electronic > House".to_string()];
    // tracks[2] has no genres; it shares only quality, source and artist words.
    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        2,
        None,
        None,
    );
    let tags = |a: i64, b: i64| {
        result
            .iter()
            .find(|n| n.track_id == a && n.neighbor_track_id == b)
            .map(|n| n.reason_tags.clone())
            .expect("edge")
    };
    assert!(tags(0, 1).iter().any(|t| t == "genre_branch"));
    assert!(!tags(0, 2).iter().any(|t| t == "genre_branch"));
}

#[test]
fn artist_only_tracks_get_no_proxy_vector() {
    let row = |track_id: i64, artist: &str, genres: Vec<String>| EmbeddingTrackRow {
        track_id,
        title: format!("Track {track_id}"),
        artist_name: Some(artist.to_string()),
        album_title: None,
        duration_ms: Some(200_000),
        best_quality: None,
        source: "tidal".to_string(),
        play_count: 0,
        is_favorite: false,
        playlist_memberships: 0,
        genre_paths: genres,
        bpm: None,
        energy: None,
        camelot_key: None,
        danceability: None,
        beat_strength: None,
        loudness_lufs: None,
    };
    let tracks = vec![
        row(1, "Steve Bicknell", Vec::new()),
        row(2, "Gunna", Vec::new()),
        row(3, "Surgeon", vec!["Electronic > Techno".to_string()]),
    ];
    let idf = compute_token_idf(&tracks);
    let features = build_audio_proxy_features(&tracks, 64, &idf, None, None);
    // A lone artist token relates to no other artist; it only produced
    // hash-collision neighbors.
    assert!(!features.contains_key(&1));
    assert!(!features.contains_key(&2));
    assert!(features.contains_key(&3));
    let fusion = fuse_embeddings(&tracks, &HashMap::new(), &features);
    assert_eq!(fusion.keys().copied().collect::<Vec<_>>(), vec![3]);
}

#[test]
fn metadata_tokens_skip_title_album_quality_and_source_words() {
    let track = EmbeddingTrackRow {
        track_id: 1,
        title: "Kathy's Song".to_string(),
        artist_name: Some("Simon & Garfunkel".to_string()),
        album_title: Some("Sounds of Silence".to_string()),
        duration_ms: Some(200_000),
        best_quality: Some("LOSSLESS".to_string()),
        source: "tidal".to_string(),
        play_count: 3,
        is_favorite: false,
        playlist_memberships: 0,
        genre_paths: vec!["Folk > Folk Rock".to_string()],
        bpm: None,
        energy: None,
        camelot_key: None,
        danceability: None,
        beat_strength: None,
        loudness_lufs: None,
    };
    let tokens = metadata_tokens(&track);
    assert!(tokens.contains(&"artist:simon & garfunkel".to_string()));
    assert!(tokens.contains(&"genre:folk".to_string()));
    assert!(tokens.contains(&"genre:folk rock".to_string()));
    for word in [
        "simon", "song", "kathy's", "silence", "lossless", "tidal", "dur_6",
    ] {
        assert!(!tokens.iter().any(|t| t == word), "unexpected token {word}");
    }
}

#[test]
fn primary_reason_prefers_behavior_over_metadata_similarity() {
    let (tracks, behavioral, audio, fusion) = make_test_input(3, 16);
    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        2,
        None,
        None,
    );
    assert!(!result.is_empty());
    for row in &result {
        assert_eq!(row.primary_reason.as_deref(), Some("behavioral"));
        assert!(row.reason_tags.iter().any(|t| t == "metadata_similarity"));
        assert!(!row.reason_tags.iter().any(|t| t == "audio_texture"));
    }
}

#[test]
fn library_sequences_count_as_structure_not_co_listen() {
    let mut input = behavioral_input(vec![vec![1, 2]]);
    input.sequences[0].label = "album_tracks".to_string();
    let (_, _, _, support) = build_behavioral_embeddings(&input, None, None);
    let bucket = support[&1][&2];
    assert_eq!(bucket.colisten, 0.0);
    assert!(bucket.structure > 0.0);

    let input = behavioral_input(vec![vec![1, 2]]);
    let (_, _, _, support) = build_behavioral_embeddings(&input, None, None);
    assert!(support[&1][&2].colisten > 0.0);
}

#[test]
fn engaged_tracks_outrank_transient_catalog_rows_at_equal_similarity() {
    let (mut tracks, behavioral, audio, fusion) = make_test_input(3, 16);
    tracks[1].play_count = 4; // engaged
    // tracks[2] stays transient: never played, favorited or playlisted.
    let result = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        2,
        None,
        None,
    );
    let ranked = result
        .iter()
        .filter(|row| row.track_id == 0)
        .map(|row| row.neighbor_track_id)
        .collect::<Vec<_>>();
    assert_eq!(ranked, vec![1, 2]);
}
