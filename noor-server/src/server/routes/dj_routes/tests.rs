use super::*;
use crate::services::audio_analysis::dj_profile::{DJ_PROFILE_VERSION, encode_f32_blob};

static TEST_AUTO_REBUILD_ACTIVE: AtomicUsize = AtomicUsize::new(0);

#[test]
fn active_scene_preserves_executed_pair_and_ends_at_resolve() {
    let db = crate::db::Database::open_in_memory().expect("db");
    db.with_conn(|conn| {
        crate::db::schema::run_migrations(conn)?;
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id) VALUES
            (1, 'Outgoing', 1), (2, 'Incoming', 1), (3, 'Following', 1)",
            [],
        )?;
        let program = noor_mix::planner::bass_swap_16_program(48_000, 2, 16_000);
        let json = serde_json::to_string(&program)?;
        conn.execute(
            "INSERT INTO dj_transition_events (from_media_ref_kind, from_media_ref_id,
            to_media_ref_kind, to_media_ref_id, template, program_json, planner_version,
            planned_start_ms, runtime_planned_start_ms, actual_start_ms, timing_status, runtime_rendered_dj_mixer, runtime_renderer_status)
            VALUES ('library_track', '1', 'library_track', '2', 'BassSwap16', ?1, 'test',
                160000, 162000, 164000, 'fired', 1, 'rendered_handoff')",
            [&json],
        )?;
        let id = conn.last_insert_rowid();
        let active = active_transition_for_event(conn, id, 8000)?.expect("audible mix");
        assert_eq!(active.outgoing.title, "Outgoing");
        assert_eq!(active.incoming.title, "Incoming");
        assert_eq!(active.program.resolve_at, program.resolve_at);
        assert_eq!(active.elapsed_ms, 8000);
        // Late joins already include skipped output frames in elapsed;
        // curve/source origin is the decoded target, not the late fire.
        assert_eq!(active.start_ms, 162000);
        assert_eq!(active.actual_start_ms, 164000);
        let incoming = DjMediaRef::LibraryTrack { track_id: 2 };
        // Pause has flushed the listening session; its installed audio
        // overlap remains frozen on the runtime output clock.
        assert_eq!(active_transition_for_runtime(conn, None, Some(&incoming), Some(8000))?
            .map(|active| active.event_id), Some(id));
        let resumed = player::ActiveListenSession::start(2, Utc::now(),
            crate::db::models::ListenSource::Unknown, None);
        assert_eq!(active_transition_for_runtime(conn, Some(&resumed), Some(&incoming), Some(8000))?
            .map(|active| active.event_id), Some(id));
        // An accepted seek clears the actual overlap clock. History
        // alone must never recreate a live transition after it.
        assert!(active_transition_for_runtime(conn, None, Some(&incoming), None)?.is_none());
        assert!(active_transition_for_runtime(conn, None,
            Some(&DjMediaRef::LibraryTrack { track_id: 3 }), Some(8000))?.is_none());
        assert!(active_transition_for_event(conn, id, 16000)?.is_none());
        assert!(active_transition_for_event(conn, id, -1)?.is_none());
        conn.execute("UPDATE tracks SET tidal_id = id * 100", [])?;
        conn.execute(
            "UPDATE dj_transition_events SET from_media_ref_kind = 'tidal_track',
             from_media_ref_id = '100', to_media_ref_kind = 'tidal_track',
             to_media_ref_id = '200' WHERE id = ?1", [id])?;
        let streamed = active_transition_for_event(conn, id, 8000)?.expect("streamed pair");
        assert_eq!(streamed.outgoing.title, "Outgoing");
        assert_eq!(streamed.incoming.title, "Incoming");
        assert_eq!(streamed.outgoing.artist.as_deref(), Some("Artist"));
        assert_eq!(streamed.incoming.artist.as_deref(), Some("Artist"));
        conn.execute(
            "UPDATE dj_transition_events SET runtime_renderer_status = 'legacy_overlap'
            WHERE id = ?1",
            [id],
        )?;
        assert!(active_transition_for_event(conn, id, 8000)?.is_none());
        // A replay of this pair starts its listen session before the new
        // promotion is persisted. It must show this execution's safety
        // programme instead of the earlier BassSwap.
        conn.execute("UPDATE dj_transition_events SET runtime_renderer_status='rendered_handoff' WHERE id=?1", [id])?;
        let safe = crate::playback::dj_engine::safe_crossfade_program(48_000, 2,
            noor_mix::Policy {default_crossfade_ms: 4000, ..Default::default()});
        conn.execute("INSERT INTO dj_transition_events (from_media_ref_kind, from_media_ref_id,
            to_media_ref_kind, to_media_ref_id, template, program_json, planner_version,
            planned_start_ms, actual_start_ms, timing_status, runtime_rendered_dj_mixer, runtime_renderer_status)
            SELECT from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, ?1, planner_version, planned_start_ms, actual_start_ms,
                timing_status, runtime_rendered_dj_mixer, runtime_renderer_status
            FROM dj_transition_events WHERE id=?2", params![serde_json::to_string(&safe)?, id])?;
        let latest = conn.last_insert_rowid();
        let stale_session = resumed.with_dj_transition_event_id(Some(id));
        let replay = active_transition_for_runtime(conn, Some(&stale_session),
            Some(&DjMediaRef::TidalTrack { tidal_id: 200, track_id: Some(2) }), Some(1000))?.expect("current execution");
        assert_eq!(replay.event_id, latest);
        assert_eq!(replay.program.template, "SafeCrossfade");
        Ok(())
    })
    .expect("executed scene");
}

#[tokio::test]
async fn policy_handler_round_trips_strategy_and_rejects_partial_invalid_saves() {
    let db = crate::db::Database::open_in_memory().expect("db");
    db.with_conn(crate::db::schema::run_migrations)
        .expect("schema");
    let state = fresh_test_state_with_dj_tx(db, None);
    let response = set_policy(
        State(state.clone()),
        Json(SetDjPolicyRequest {
            mix_intent: Some("bold".into()),
            transition_speed_bias: Some("faster".into()),
            preferred_strategy: Some("club_mix".into()),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(response.preferred_strategy, "club_mix");
    assert!(
        set_policy(
            State(state.clone()),
            Json(SetDjPolicyRequest {
                mix_intent: Some("safe".into()),
                transition_speed_bias: None,
                preferred_strategy: Some("invalid".into()),
            })
        )
        .await
        .is_err()
    );
    let response = get_policy(State(state)).await.unwrap().0;
    assert_eq!(response.mix_intent, "bold");
    assert_eq!(response.preferred_strategy, "club_mix");
}

fn test_profile_row(key: &AudioDjProfileKey, version: &str) -> AudioDjProfileRow {
    AudioDjProfileRow {
        media_ref_kind: key.media_ref_kind.clone(),
        media_ref_id: key.media_ref_id.clone(),
        track_id: None,
        queue_item_id: None,
        tidal_id: None,
        profile_version: version.to_string(),
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
        waveform_peaks_blob: encode_f32_blob(&[0.0, 0.5, 1.0]),
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

fn test_deck_status(profile_ready: bool, profile_status: &str) -> DjDeckStatus {
    DjDeckStatus {
        media_ref_kind: "tidal_track".to_string(),
        media_ref_id: "123".to_string(),
        title: "Test Track".to_string(),
        artist: Some("Test Artist".to_string()),
        profile_ready,
        profile_status: profile_status.to_string(),
        profile_error: None,
        profile_retry_after_ms: None,
        profile_retry_reason: None,
        profile_confidence: profile_ready.then_some(0.85),
        beat_confidence: profile_ready.then_some(0.9),
        grid_is_synthetic: false,
        analysis_scope_ms: profile_ready.then_some(90_000),
        energy: None,
        beat_count: profile_ready.then_some(128),
        downbeat_count: profile_ready.then_some(32),
        phrase_count: profile_ready.then_some(8),
        waveform_status: if profile_ready { "ready" } else { "missing" }.to_string(),
        waveform_peaks: if profile_ready {
            vec![0.0, 0.5, 1.0]
        } else {
            Vec::new()
        },
        beat_markers_ms: Vec::new(),
        downbeat_markers_ms: Vec::new(),
        phrase_markers_ms: Vec::new(),
        drop_markers_ms: Vec::new(),
        manual_drop_markers_ms: Vec::new(),
        mix_in_markers_ms: Vec::new(),
        mix_out_markers_ms: Vec::new(),
        passive_analysis_status: None,
        passive_analysis_reason: None,
        safe_crossfade_only: false,
    }
}

fn fresh_test_state_with_dj_tx(
    db: crate::db::Database,
    dj_analysis_tx: Option<
        tokio::sync::mpsc::UnboundedSender<
            crate::services::audio_analysis::dj_profile::DjAnalysisJob,
        >,
    >,
) -> SharedState {
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    #[cfg(feature = "spotify-public")]
    let spotify_public = std::sync::Arc::new(
        crate::services::spotify_public::SpotifyPublicClient::new(db.clone())
            .expect("SpotifyPublicClient::new must succeed in tests"),
    );
    let remote = crate::server::remote::RemoteService::new(db.clone(), String::new())
        .expect("remote service");
    std::sync::Arc::new(tokio::sync::RwLock::new(crate::AppState {
        db,
        event_tx,
        http_client: reqwest::Client::new(),
        tidal_http_client: reqwest::Client::new(),
        tidal: crate::services::tidal::session::TidalSession::disconnected_for_tests(),
        stream_source: crate::server::transport::stream::ScriptedStreamSource::offline(),
        tidal_mixes_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        tidal_radio_stations_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        home_picks_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        tidal_moods_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        tidal_page_modules_cache: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        tidal_playlist_tracks_cache: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        lastfm_similar_cache: crate::services::radio::new_lastfm_similar_cache(),
        playback_runtime: None,
        playback_runtime_info: None,
        playback_generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
        current_stream_display: None,
        pending_stream_display: None,
        next_prebuffer_inflight: None,
        last_drop_preview: None,
        active_listen_session: None,
        live_listen_session: None,
        play_history: crate::playback::history::PlayHistory::default(),
        tidal_login_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rss_aggregator: std::sync::Arc::new(crate::services::rss_feeds::FeedAggregator::new(
            reqwest::Client::new(),
        )),
        analysis_tx: None,
        dj_analysis_tx,
        dj_profile_rebuild_inflight: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        audio_analysis_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        audio_analysis_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        lastfm_enrich_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        lastfm_enrich_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        musicbrainz_enrich_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        tidal_repair_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        library_video_scan_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        tidal_sync_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        tidal_sync_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        lastfm_enrich_total: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_processed: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_total: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_done: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_started_at: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        discovery_train_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        radio_similarity_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        refreshed_seeds: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        embedding_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        master_key: crate::services::crypto::MasterKey::ephemeral(),
        lastfm_api_secret: None,
        server_token: String::new(),
        remote,
        audio_active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        user_cleared_at: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        #[cfg(feature = "spotify-public")]
        spotify_public,
        sportify_client: None,
        sportify_cache_config: crate::services::sportify::cache::SportifyCacheConfig::default(),
        sportify_resolve_config: crate::services::sportify::cache::SportifyResolveConfig::default(),
        downloads: crate::services::download::DownloadManager::new(),
    }))
}

fn seed_dsp_key(conn: &rusqlite::Connection, track_id: i64, camelot_key: &str) {
    conn.execute(
        "INSERT OR IGNORE INTO artists (id, name) VALUES (1, 'Test Artist')",
        [],
    )
    .expect("artist");
    conn.execute(
        "INSERT OR IGNORE INTO tracks (id, title, artist_id, source)
         VALUES (?1, ?2, 1, 'tidal')",
        params![track_id, format!("Track {track_id}")],
    )
    .expect("track");
    queries::upsert_audio_dsp_features(
        conn,
        &crate::db::models::AudioDspFeatures {
            track_id,
            bpm: Some(120.0),
            key_signature: None,
            camelot_key: Some(camelot_key.to_string()),
            loudness_lufs: Some(-12.0),
            energy: Some(0.7),
            danceability: Some(0.7),
            beat_strength: Some(0.7),
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
    .expect("dsp features");
}

#[test]
fn correction_response_round_trips_manual_drop_markers() {
    let row = AudioDjProfileCorrectionRow {
        media_ref_kind: "tidal_track".to_string(),
        media_ref_id: "123".to_string(),
        bpm_multiplier: None,
        downbeat_offset_beats: None,
        phrase_offset_bars: None,
        safe_crossfade_only: false,
        transition_speed_bias: None,
        manual_drop_blob: encode_marker_ms_blob(&[64_000, 32_000]),
        notes: None,
        created_at: "now".to_string(),
        updated_at: "now".to_string(),
    };

    let response = correction_response(row);

    assert_eq!(response.manual_drop_markers_ms, vec![64_000, 32_000]);
}

#[test]
fn manual_drop_marker_payload_is_sorted_deduped_and_non_negative() {
    assert_eq!(
        normalize_manual_drop_markers_ms(Some(vec![64_000, 32_000, 32_000])).expect("markers"),
        vec![32_000, 64_000]
    );
    assert_eq!(
        normalize_manual_drop_markers_ms(Some(vec![-1])),
        Err(StatusCode::BAD_REQUEST)
    );
    assert_eq!(
        normalize_manual_drop_markers_ms(Some(vec![MAX_MANUAL_DROP_MARKER_MS + 1])),
        Err(StatusCode::BAD_REQUEST)
    );
}

#[test]
fn omitted_manual_drop_payload_preserves_existing_blob() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let key = AudioDjProfileKey {
        media_ref_kind: "tidal_track".to_string(),
        media_ref_id: "123".to_string(),
    };
    let existing_blob = encode_marker_ms_blob(&[32_000]);
    queries::upsert_audio_dj_profile_correction(
        &conn,
        &AudioDjProfileCorrectionRow {
            media_ref_kind: key.media_ref_kind.clone(),
            media_ref_id: key.media_ref_id.clone(),
            bpm_multiplier: None,
            downbeat_offset_beats: None,
            phrase_offset_bars: None,
            safe_crossfade_only: false,
            transition_speed_bias: None,
            manual_drop_blob: existing_blob.clone(),
            notes: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        },
    )
    .expect("correction");

    assert_eq!(
        existing_manual_drop_blob(&conn, &key).expect("existing blob"),
        existing_blob
    );
}

#[test]
fn dj_profile_current_check_requires_current_version() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let key = AudioDjProfileKey {
        media_ref_kind: "tidal_track".to_string(),
        media_ref_id: "123".to_string(),
    };

    assert!(!dj_profile_is_current_version(&conn, &key).expect("missing"));
    queries::upsert_audio_dj_profile(&conn, &test_profile_row(&key, "old_profile_v0"))
        .expect("old profile");
    assert!(!dj_profile_is_current_version(&conn, &key).expect("old"));
    queries::upsert_audio_dj_profile(&conn, &test_profile_row(&key, DJ_PROFILE_VERSION))
        .expect("current profile");

    assert!(dj_profile_is_current_version(&conn, &key).expect("current"));
    let mut current_without_waveform = test_profile_row(&key, DJ_PROFILE_VERSION);
    current_without_waveform.waveform_peaks_blob = encode_f32_blob(&[]);
    queries::upsert_audio_dj_profile(&conn, &current_without_waveform)
        .expect("profile missing waveform");
    assert!(!dj_profile_is_current_version(&conn, &key).expect("missing waveform"));
}

#[test]
fn deck_status_exposes_capped_waveform_peaks() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let key = AudioDjProfileKey {
        media_ref_kind: "tidal_track".to_string(),
        media_ref_id: "123".to_string(),
    };
    let mut row = test_profile_row(&key, DJ_PROFILE_VERSION);
    row.waveform_peaks_blob = encode_f32_blob(&vec![0.75; DJ_WAVEFORM_PEAK_COUNT + 4]);
    queries::upsert_audio_dj_profile(&conn, &row).expect("profile");

    let deck = deck_status(
        &conn,
        &DjMediaRef::TidalTrack {
            tidal_id: 123,
            track_id: None,
        },
        Some(&("Track".to_string(), Some("Artist".to_string()))),
        false,
    )
    .expect("deck status");

    assert_eq!(deck.waveform_status, "ready");
    assert_eq!(deck.waveform_peaks.len(), DJ_WAVEFORM_PEAK_COUNT);
    assert!(deck.waveform_peaks.iter().all(|peak| *peak == 0.75));
}

#[test]
fn deck_status_marks_missing_waveform() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let key = AudioDjProfileKey {
        media_ref_kind: "tidal_track".to_string(),
        media_ref_id: "123".to_string(),
    };
    let mut row = test_profile_row(&key, DJ_PROFILE_VERSION);
    row.waveform_peaks_blob = encode_f32_blob(&[]);
    queries::upsert_audio_dj_profile(&conn, &row).expect("profile");

    let deck = deck_status(
        &conn,
        &DjMediaRef::TidalTrack {
            tidal_id: 123,
            track_id: None,
        },
        Some(&("Track".to_string(), Some("Artist".to_string()))),
        false,
    )
    .expect("deck status");

    assert_eq!(deck.waveform_status, "missing");
    assert!(deck.waveform_peaks.is_empty());
    assert!(deck_needs_profile_rebuild(&deck));
}

#[tokio::test]
async fn queue_missing_profiles_for_current_pair_runs_while_playing() {
    let db = crate::db::Database::open_in_memory().expect("db");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| {
        queries::set_dj_engine_enabled(conn, true)?;
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id) VALUES (1, 'Current', 1, 111)",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id) VALUES (2, 'Incoming', 1, 222)",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (id, track_id, position, source) VALUES (10, 1, 0, 'user')",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (id, track_id, position, source) VALUES (11, 2, 1, 'user')",
            [],
        )?;
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = 10, is_playing = 1
             WHERE id = 1",
            [],
        )?;
        queries::upsert_audio_dj_profile(
            conn,
            &test_profile_row(
                &DjMediaRef::TidalTrack {
                    tidal_id: 111,
                    track_id: Some(1),
                }
                .profile_key(),
                DJ_PROFILE_VERSION,
            ),
        )?;
        Ok(())
    })
    .expect("seeded");
    let (dj_tx, _dj_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = fresh_test_state_with_dj_tx(db, Some(dj_tx));

    let attempted = queue_missing_dj_profiles_for_current_pair(state)
        .await
        .expect("queue missing profiles");

    assert_eq!(attempted, 1);
}

#[test]
fn missing_profile_refs_include_incoming_while_playback_is_active() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let current = DjMediaRef::TidalTrack {
        tidal_id: 111,
        track_id: Some(1),
    };
    let incoming = DjMediaRef::TidalTrack {
        tidal_id: 222,
        track_id: Some(2),
    };
    queries::upsert_audio_dj_profile(
        &conn,
        &test_profile_row(&current.profile_key(), DJ_PROFILE_VERSION),
    )
    .expect("current profile");
    conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
        .expect("artist");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id) VALUES (1, 'Current', 1, 111)",
        [],
    )
    .expect("current track");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, tidal_id) VALUES (2, 'Incoming', 1, 222)",
        [],
    )
    .expect("incoming track");
    conn.execute(
        "UPDATE playback_state SET current_track_id = 1, is_playing = 1 WHERE id = 1",
        [],
    )
    .expect("active playback");

    let pair = crate::playback::dj_lookahead::DjLookaheadPair {
        current: Some(current),
        next: Some(incoming.clone()),
        current_queue_item_id: Some(10),
        next_queue_item_id: Some(11),
        queue_generation: 7,
    };
    let inflight = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    let missing = missing_dj_profile_refs_for_pair(&conn, pair, &[], &inflight)
        .expect("missing profile refs");

    assert_eq!(missing, vec![incoming]);
}

#[test]
fn automatic_profile_rebuild_slots_are_bounded_and_released() {
    TEST_AUTO_REBUILD_ACTIVE.store(0, Ordering::Release);

    let first = try_claim_auto_dj_profile_rebuild_slot_from(&TEST_AUTO_REBUILD_ACTIVE, 2)
        .expect("first slot");
    let second = try_claim_auto_dj_profile_rebuild_slot_from(&TEST_AUTO_REBUILD_ACTIVE, 2)
        .expect("second slot");

    assert!(try_claim_auto_dj_profile_rebuild_slot_from(&TEST_AUTO_REBUILD_ACTIVE, 2).is_none());
    drop(first);
    let replacement = try_claim_auto_dj_profile_rebuild_slot_from(&TEST_AUTO_REBUILD_ACTIVE, 2)
        .expect("released slot");
    drop(second);
    drop(replacement);
    assert_eq!(TEST_AUTO_REBUILD_ACTIVE.load(Ordering::Acquire), 0);
}

#[test]
fn automatic_profile_rebuild_supports_only_tidal_refs_explicitly() {
    assert_eq!(
        unsupported_auto_profile_rebuild_status(&DjMediaRef::LibraryTrack { track_id: 1 }),
        Some("source_unavailable")
    );
    assert_eq!(
        unsupported_auto_profile_rebuild_status(&DjMediaRef::PendingQueueItem {
            queue_item_id: 44,
            pending_artist: "Artist".to_string(),
            pending_title: "Title".to_string(),
            tidal_id_hint: None,
        }),
        Some("source_unavailable")
    );
    assert_eq!(
        unsupported_auto_profile_rebuild_status(&DjMediaRef::TidalTrack {
            tidal_id: 111,
            track_id: Some(1),
        }),
        None
    );
}

#[test]
fn dj_profile_rebuild_tries_lossless_after_low_quality_asset_not_ready() {
    let requests = dj_profile_analysis_stream_requests(28051328);

    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.track_id == 28051328));
    assert_eq!(requests[0].audio_quality, "LOW");
    assert_eq!(requests[1].audio_quality, "LOSSLESS");

    let error = anyhow::Error::msg(
        r#"TIDAL playback request was rejected: TIDAL rejected playback request with 401 Unauthorized: {"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#,
    );
    assert_eq!(
        next_dj_profile_analysis_quality(0, &error),
        Some("LOSSLESS")
    );
    assert_eq!(next_dj_profile_analysis_quality(1, &error), None);
}

#[test]
fn dj_profile_rebuild_tries_lossless_after_low_quality_cdn_failure() {
    // The LOW/AAC tier can route to an unreachable ad CDN whose segments
    // only time out. That failure must now drop to LOSSLESS instead of
    // giving up (previously only asset-not-ready fell back).
    let ad_host = anyhow::Error::msg(
        "DASH stream prebuffer failed: DASH segment 0 skipped: TIDAL ad-tier CDN host (sp-ad-cf.audio.tidal.com/0.mp4)",
    );
    assert_eq!(
        next_dj_profile_analysis_quality(0, &ad_host),
        Some("LOSSLESS")
    );

    let timeout = anyhow::Error::msg(
        "DASH stream prebuffer failed: DASH segment 3 timed out after 12s (sp-ad-cf.audio.tidal.com/3.mp4)",
    );
    assert_eq!(
        next_dj_profile_analysis_quality(0, &timeout),
        Some("LOSSLESS")
    );

    // A non-retryable failure (e.g. track genuinely gone) still gives up.
    let permanent = anyhow::Error::msg("track 404 not found");
    assert_eq!(next_dj_profile_analysis_quality(0, &permanent), None);
}

#[test]
fn renderer_status_keeps_non_renderable_template_out_of_main_renderer() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 17,
        template: "UnknownTemplate".to_string(),
        renderer_template: None,
        fallback_reason: None,
        planned_start_ms: None,
        actual_start_ms: None,
        timing_delta_ms: None,
        timing_source: None,
        timing_status: None,
        overlay_details: None,
        runtime_rendered_dj_mixer: None,
        runtime_renderer_status: None,
        runtime_renderer_reason: None,
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("UnknownTemplate"));
    assert_eq!(status.renderer_template, None);
    assert_eq!(status.renderer_mode.as_deref(), Some("legacy_overlap"));
    assert_eq!(
        status.downgrade_reason.as_deref(),
        Some("template_not_renderable")
    );
}

#[test]
fn renderer_status_exposes_drop_tease_overlay_renderer() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 171,
        template: "DropTease16".to_string(),
        renderer_template: Some("DropTease16".to_string()),
        fallback_reason: None,
        planned_start_ms: None,
        actual_start_ms: None,
        timing_delta_ms: None,
        timing_source: None,
        timing_status: None,
        overlay_details: Some(DjOverlayDetails {
            overlay_status: "armed".to_string(),
            overlay_start_ms: Some(120_000),
            overlay_end_ms: Some(151_000),
            tempo_ratio: Some(1.02),
            deck_b_start_frame: 384_000,
            drop_marker_ms: Some(8_500),
            drop_source: "program_json".to_string(),
        }),
        runtime_rendered_dj_mixer: None,
        runtime_renderer_status: None,
        runtime_renderer_reason: None,
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("DropTease16"));
    assert_eq!(status.renderer_template.as_deref(), Some("DropTease16"));
    assert_eq!(status.renderer_mode.as_deref(), Some("dj_overlay_program"));
    assert_eq!(status.downgrade_reason, None);
    assert_eq!(
        status.overlay_details,
        Some(DjOverlayDetails {
            overlay_status: "armed".to_string(),
            overlay_start_ms: Some(120_000),
            overlay_end_ms: Some(151_000),
            tempo_ratio: Some(1.02),
            deck_b_start_frame: 384_000,
            drop_marker_ms: Some(8_500),
            drop_source: "program_json".to_string(),
        })
    );
}

#[test]
fn renderer_status_exposes_filter_sweep_runtime_program() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 18,
        template: "FilterSweep".to_string(),
        renderer_template: Some("FilterSweep".to_string()),
        fallback_reason: None,
        planned_start_ms: Some(112_000),
        actual_start_ms: Some(112_144),
        timing_delta_ms: Some(144),
        timing_source: Some("downbeat_sync".to_string()),
        timing_status: Some("fired".to_string()),
        overlay_details: None,
        runtime_rendered_dj_mixer: None,
        runtime_renderer_status: None,
        runtime_renderer_reason: None,
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("FilterSweep"));
    assert_eq!(status.renderer_template.as_deref(), Some("FilterSweep"));
    assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
    assert_eq!(status.downgrade_reason, None);
    assert_eq!(status.timing_direction, "on_time");
}

#[test]
fn renderer_status_exposes_bass_swap_16_runtime_program() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 22,
        template: "BassSwap16".to_string(),
        renderer_template: Some("BassSwap16".to_string()),
        fallback_reason: None,
        planned_start_ms: Some(112_000),
        actual_start_ms: Some(112_144),
        timing_delta_ms: Some(144),
        timing_source: Some("downbeat_sync".to_string()),
        timing_status: Some("fired".to_string()),
        overlay_details: None,
        runtime_rendered_dj_mixer: None,
        runtime_renderer_status: None,
        runtime_renderer_reason: None,
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("BassSwap16"));
    assert_eq!(status.renderer_template.as_deref(), Some("BassSwap16"));
    assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
    assert_eq!(status.downgrade_reason, None);
}

#[test]
fn renderer_status_exposes_new_runtime_programs() {
    for template in ["BassSwap32", "SlamCut", "LongHarmonicBlend"] {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 23,
            template: template.to_string(),
            renderer_template: Some(template.to_string()),
            fallback_reason: None,
            planned_start_ms: Some(112_000),
            actual_start_ms: Some(112_144),
            timing_delta_ms: Some(144),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            overlay_details: None,
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
        }));

        assert_eq!(status.planned_template.as_deref(), Some(template));
        assert_eq!(status.renderer_template.as_deref(), Some(template));
        assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
        assert_eq!(status.downgrade_reason, None);
    }
}

#[test]
fn renderer_template_from_program_json_accepts_new_runtime_programs() {
    for template in ["BassSwap32", "SlamCut", "LongHarmonicBlend"] {
        let program = noor_mix::TransitionProgram {
            tier: noor_mix::Tier::FullBlend,
            template: template.to_string(),
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
            resolve_at: 1,
            loops: vec![],
            automation: vec![],
        };
        let json = serde_json::to_string(&program).expect("program json");

        assert_eq!(
            renderer_template_from_program_json(&json).as_deref(),
            Some(template)
        );
    }
}

#[test]
fn overlay_details_from_program_json_exposes_drop_tease_facts() {
    let program = noor_mix::TransitionProgram {
        tier: noor_mix::Tier::FullBlend,
        template: "DropTease16".to_string(),
        drop_source: None,
        decision: None,
        sample_rate: 48_000,
        channels: 2,
        deck_a_start_frame: 0,
        deck_b_start_frame: 384_000,
        sync_start: 0,
        intro_start: 0,
        swap_start: 24_000,
        fade_start: 24_000,
        resolve_at: 48_000,
        loops: vec![],
        automation: vec![noor_mix::AutomationEvent {
            param: noor_mix::Param::PlaybackRate(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: 48_000,
            from: 1.02,
            to: 1.02,
            curve: noor_mix::Curve::Linear,
        }],
    };
    let legacy_json = serde_json::to_string(&program).expect("program json");

    assert_eq!(
        overlay_details_from_program_json(&legacy_json, Some(120_000), Some("fired")),
        Some(DjOverlayDetails {
            overlay_status: "fired".to_string(),
            overlay_start_ms: Some(120_000),
            overlay_end_ms: Some(121_000),
            tempo_ratio: Some(1.0199999809265137),
            deck_b_start_frame: 384_000,
            drop_marker_ms: Some(8_500),
            drop_source: "program_json".to_string(),
        })
    );

    let manual_program = noor_mix::TransitionProgram {
        drop_source: Some("manual_drop_cue".to_string()),
        decision: None,
        ..program
    };
    let manual_json = serde_json::to_string(&manual_program).expect("manual program json");

    assert_eq!(
        overlay_details_from_program_json(&manual_json, Some(120_000), Some("fired"))
            .expect("manual details")
            .drop_source,
        "manual_drop_cue"
    );
}

#[test]
fn overlay_details_source_labels_manual_and_profile_drop_markers() {
    let details = Some(DjOverlayDetails {
        overlay_status: "armed".to_string(),
        overlay_start_ms: Some(120_000),
        overlay_end_ms: Some(121_000),
        tempo_ratio: Some(1.0),
        deck_b_start_frame: 384_000,
        drop_marker_ms: Some(32_000),
        drop_source: "program_json".to_string(),
    });
    let mut incoming = test_deck_status(true, "ready");
    incoming.drop_markers_ms = vec![32_000];

    assert_eq!(
        annotate_overlay_drop_source(details.clone(), Some(&incoming))
            .expect("profile details")
            .drop_source,
        "profile_drop_candidate"
    );

    incoming.manual_drop_markers_ms = vec![32_000];
    assert_eq!(
        annotate_overlay_drop_source(details, Some(&incoming))
            .expect("manual details")
            .drop_source,
        "manual_drop_cue"
    );
}

#[test]
fn annotate_overlay_drop_source_preserves_program_source() {
    let details = Some(DjOverlayDetails {
        overlay_status: "armed".to_string(),
        overlay_start_ms: Some(120_000),
        overlay_end_ms: Some(121_000),
        tempo_ratio: Some(1.0),
        deck_b_start_frame: 384_000,
        drop_marker_ms: Some(32_000),
        drop_source: "manual_drop_cue".to_string(),
    });
    let mut incoming = test_deck_status(true, "ready");
    incoming.drop_markers_ms = vec![32_000];

    assert_eq!(
        annotate_overlay_drop_source(details, Some(&incoming))
            .expect("details")
            .drop_source,
        "manual_drop_cue"
    );
}

#[test]
fn renderer_status_marks_safe_crossfade_renderer_as_pending() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 19,
        template: "SafeCrossfade".to_string(),
        renderer_template: None,
        fallback_reason: None,
        planned_start_ms: None,
        actual_start_ms: None,
        timing_delta_ms: None,
        timing_source: None,
        timing_status: None,
        overlay_details: None,
        runtime_rendered_dj_mixer: None,
        runtime_renderer_status: None,
        runtime_renderer_reason: None,
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("SafeCrossfade"));
    assert_eq!(status.renderer_template, None);
    assert_eq!(status.renderer_mode.as_deref(), Some("legacy_overlap"));
    assert_eq!(
        status.downgrade_reason.as_deref(),
        Some("dj_program_renderer_pending")
    );
}

#[test]
fn renderer_status_exposes_safe_crossfade_runtime_program() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 20,
        template: "SlamCut".to_string(),
        renderer_template: Some("SafeCrossfade".to_string()),
        fallback_reason: None,
        planned_start_ms: Some(112_000),
        actual_start_ms: Some(112_144),
        timing_delta_ms: Some(144),
        timing_source: Some("downbeat_sync".to_string()),
        timing_status: Some("fired".to_string()),
        overlay_details: None,
        runtime_rendered_dj_mixer: Some(true),
        runtime_renderer_status: Some("rendered_handoff".to_string()),
        runtime_renderer_reason: Some("none".to_string()),
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("SlamCut"));
    assert_eq!(status.renderer_template.as_deref(), Some("SafeCrossfade"));
    assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
    assert_eq!(
        status.downgrade_reason.as_deref(),
        Some("template_not_renderable")
    );
    assert_eq!(status.planning_reason, None);
    assert_eq!(status.planned_start_ms, Some(112_000));
    assert_eq!(status.actual_start_ms, Some(112_144));
    assert_eq!(status.timing_delta_ms, Some(144));
    assert_eq!(status.timing_source.as_deref(), Some("downbeat_sync"));
    assert_eq!(status.timing_status.as_deref(), Some("fired"));
    assert_eq!(status.runtime_rendered_dj_mixer, Some(true));
    assert_eq!(
        status.runtime_renderer_status.as_deref(),
        Some("rendered_handoff")
    );
    assert_eq!(status.runtime_renderer_reason.as_deref(), Some("none"));
}

#[test]
fn renderer_status_keeps_safe_crossfade_planning_reason_out_of_downgrade() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 20,
        template: "SafeCrossfade".to_string(),
        renderer_template: Some("SafeCrossfade".to_string()),
        fallback_reason: Some("next_profile_missing".to_string()),
        planned_start_ms: Some(112_000),
        actual_start_ms: Some(112_144),
        timing_delta_ms: Some(144),
        timing_source: Some("downbeat_sync".to_string()),
        timing_status: Some("fired".to_string()),
        overlay_details: None,
        runtime_rendered_dj_mixer: None,
        runtime_renderer_status: None,
        runtime_renderer_reason: None,
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("SafeCrossfade"));
    assert_eq!(status.renderer_template.as_deref(), Some("SafeCrossfade"));
    assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
    assert_eq!(status.downgrade_reason, None);
    assert_eq!(
        status.planning_reason.as_deref(),
        Some("next_profile_missing")
    );
}

#[test]
fn renderer_status_reports_timing_unstable_as_downgrade_reason() {
    let status = renderer_status_for_transition(Some(&OpenTransition {
        id: 21,
        template: "FilterSweep".to_string(),
        renderer_template: Some("SafeCrossfade".to_string()),
        fallback_reason: Some("timing_unstable".to_string()),
        planned_start_ms: Some(202_091),
        actual_start_ms: Some(202_640),
        timing_delta_ms: Some(549),
        timing_source: Some("downbeat_sync".to_string()),
        timing_status: Some("fired".to_string()),
        overlay_details: None,
        runtime_rendered_dj_mixer: Some(false),
        runtime_renderer_status: Some("legacy_overlap".to_string()),
        runtime_renderer_reason: Some("prepared_mixer_missing".to_string()),
        rejected_alternatives: Vec::new(),
    }));

    assert_eq!(status.planned_template.as_deref(), Some("FilterSweep"));
    assert_eq!(status.renderer_template.as_deref(), Some("SafeCrossfade"));
    assert_eq!(status.downgrade_reason.as_deref(), Some("timing_unstable"));
    assert_eq!(status.planning_reason, None);
    assert_eq!(status.runtime_rendered_dj_mixer, Some(false));
    assert_eq!(
        status.runtime_renderer_status.as_deref(),
        Some("legacy_overlap")
    );
    assert_eq!(
        status.runtime_renderer_reason.as_deref(),
        Some("prepared_mixer_missing")
    );
}

#[test]
fn latest_completed_timing_transition_returns_last_fired_row() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status
         ) VALUES (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 222040, 40, 'downbeat_sync', 'fired'
         )",
        [],
    )
    .expect("insert");

    let transition = latest_completed_timing_transition(&conn)
        .expect("query")
        .expect("transition");

    assert_eq!(transition.planned_start_ms, Some(222_000));
    assert_eq!(transition.actual_start_ms, Some(222_040));
    assert_eq!(transition.timing_delta_ms, Some(40));
    assert_eq!(transition.timing_status.as_deref(), Some("fired"));
}

#[test]
fn timing_history_keeps_completed_rows_when_newer_pair_is_armed() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status
         ) VALUES (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 222040, 40, 'downbeat_sync', 'fired'
         )",
        [],
    )
    .expect("insert fired");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            timing_source, timing_status
         ) VALUES (
            'tidal_track', '2', 'tidal_track', '3',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            300000, 'beat_sync', 'armed'
         )",
        [],
    )
    .expect("insert armed");

    let current = DjMediaRef::TidalTrack {
        tidal_id: 2,
        track_id: None,
    };
    let next = DjMediaRef::TidalTrack {
        tidal_id: 3,
        track_id: None,
    };
    let open = latest_open_transition_for_pair(&conn, Some(&current), Some(&next))
        .expect("open")
        .expect("armed row");
    let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

    assert_eq!(open.timing_status.as_deref(), Some("armed"));
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].event_id, 1);
    assert_eq!(history[0].actual_start_ms, Some(222_040));
    assert_eq!(history[0].timing_status.as_deref(), Some("fired"));
    assert_eq!(history[0].timing_quality, "tight");
    assert_eq!(history[0].timing_direction, "on_time");
}

#[test]
fn timing_history_filters_duplicate_missed_row_when_pair_already_fired() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            timing_source, timing_status
         ) VALUES (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 'downbeat_sync', 'missed'
         )",
        [],
    )
    .expect("insert duplicate missed");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status
         ) VALUES (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 222040, 40, 'downbeat_sync', 'fired'
         )",
        [],
    )
    .expect("insert fired");

    let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].timing_status.as_deref(), Some("fired"));
    assert_eq!(history[0].timing_delta_ms, Some(40));
}

#[test]
fn timing_history_keeps_newer_missed_attempt_after_older_fired_pair() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status
         ) VALUES (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 222040, 40, 'downbeat_sync', 'fired'
         )",
        [],
    )
    .expect("insert fired");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            timing_source, timing_status
         ) VALUES (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 'downbeat_sync', 'missed'
         )",
        [],
    )
    .expect("insert newer missed");

    let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

    assert_eq!(history.len(), 2);
    assert_eq!(history[0].timing_status.as_deref(), Some("missed"));
    assert_eq!(history[1].timing_status.as_deref(), Some("fired"));
}

#[test]
fn timing_history_filters_manual_seek_suppressed_boundary_rows() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status,
            runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
         ) VALUES (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 240000, 18000, 'downbeat_sync', 'late',
            0, 'boundary_fallback', 'manual_seek_suppressed'
         )",
        [],
    )
    .expect("insert manual seek row");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status,
            runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
         ) VALUES (
            'tidal_track', '2', 'tidal_track', '3',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            222000, 222040, 40, 'downbeat_sync', 'fired',
            1, 'rendered_handoff', 'none'
         )",
        [],
    )
    .expect("insert rendered row");

    let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].timing_delta_ms, Some(40));
    assert_eq!(history[0].runtime_renderer_reason.as_deref(), Some("none"));
}

#[test]
fn latest_fired_timing_deltas_exclude_impossible_and_preview_rows() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status
         ) VALUES
         (
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            100000, 100120, 120, 'downbeat_sync', 'fired'
         ),
         (
            'tidal_track', '2', 'tidal_track', '3',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            100000, 140001, 40001, 'downbeat_sync', 'fired'
         ),
         (
            'tidal_track', '3', 'tidal_track', '4',
            'DropPreview16', '{\"template\":\"DropPreview16\"}', 'dj-v1',
            100000, 100240, 240, 'drop_preview', 'fired'
         )",
        [],
    )
    .expect("insert timing rows");

    let deltas = latest_fired_dj_timing_deltas(&conn, 20).expect("deltas");

    assert_eq!(deltas, vec![120]);
}

#[test]
fn timing_history_includes_track_pair_labels() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO artists (id, name) VALUES (10, 'Outgoing Artist'), (11, 'Incoming Artist')",
        [],
    )
    .expect("insert artists");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, source)
         VALUES (100, 'Outgoing Track', 10, 'tidal'), (101, 'Incoming Track', 11, 'tidal')",
        [],
    )
    .expect("insert tracks");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_track_id, to_track_id,
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, fallback_reason, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status
         ) VALUES (
            100, 101,
            'tidal_track', '1', 'tidal_track', '2',
            'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
            'next_profile_missing',
            10000, 10320, 320, 'downbeat_sync', 'fired'
         )",
        [],
    )
    .expect("insert event");

    let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

    assert_eq!(history[0].from_title.as_deref(), Some("Outgoing Track"));
    assert_eq!(history[0].from_artist.as_deref(), Some("Outgoing Artist"));
    assert_eq!(history[0].to_title.as_deref(), Some("Incoming Track"));
    assert_eq!(history[0].to_artist.as_deref(), Some("Incoming Artist"));
    assert_eq!(
        history[0].planning_reason.as_deref(),
        Some("next_profile_missing")
    );
    assert_eq!(history[0].timing_direction, "late");
}

#[test]
fn timing_history_resolves_tidal_media_ref_labels_without_track_ids() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    conn.execute(
        "INSERT INTO artists (id, name) VALUES (10, 'Outgoing Artist'), (11, 'Incoming Artist')",
        [],
    )
    .expect("insert artists");
    conn.execute(
        "INSERT INTO tracks (id, title, artist_id, source, tidal_id)
         VALUES (100, 'Outgoing Track', 10, 'tidal', 2501),
                (101, 'Incoming Track', 11, 'tidal', 2502)",
        [],
    )
    .expect("insert tracks");
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
            template, program_json, planner_version, planned_start_ms,
            actual_start_ms, timing_delta_ms, timing_source, timing_status
         ) VALUES (
            'tidal_track', '2501', 'tidal_track', '2502',
            'BassSwap16', '{\"template\":\"BassSwap16\"}', 'dj-v1',
            10000, 10120, 120, 'downbeat_sync', 'fired'
         )",
        [],
    )
    .expect("insert event");

    let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

    assert_eq!(history[0].from_title.as_deref(), Some("Outgoing Track"));
    assert_eq!(history[0].from_artist.as_deref(), Some("Outgoing Artist"));
    assert_eq!(history[0].to_title.as_deref(), Some("Incoming Track"));
    assert_eq!(history[0].to_artist.as_deref(), Some("Incoming Artist"));
}

#[test]
fn timing_quality_labels_delta_bands_and_missed() {
    assert_eq!(timing_quality(Some("fired"), Some(150)), "tight");
    assert_eq!(timing_quality(Some("fired"), Some(-500)), "usable");
    assert_eq!(timing_quality(Some("late"), Some(1000)), "loose");
    assert_eq!(timing_quality(Some("late"), Some(1001)), "bad");
    assert_eq!(timing_quality(Some("missed"), None), "bad");
    assert_eq!(timing_quality(Some("armed"), None), "pending");
    assert_eq!(timing_quality(Some("fired"), None), "bad");
}

#[test]
fn timing_history_distinguishes_decoded_target_from_metadata_estimate() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    conn.execute("INSERT INTO dj_transition_events
        (template, program_json, planner_version, planned_start_ms, timing_source, timing_status)
        VALUES ('SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'test', 401000, 'fallback_overlap', 'armed')", []).unwrap();
    let id = conn.last_insert_rowid();
    queries::update_dj_transition_fire_timing_with_runtime_target(
        &conn,
        id,
        403006,
        Some(403000),
        "fired",
        true,
        "rendered_handoff",
        "none",
    )
    .unwrap();
    let history = latest_dj_transition_timing_history(&conn, 5).unwrap();
    assert_eq!(history[0].planned_start_ms, Some(401000));
    assert_eq!(history[0].runtime_planned_start_ms, Some(403000));
    assert_eq!(history[0].actual_start_ms, Some(403006));
    assert_eq!(history[0].timing_delta_ms, Some(6));
    assert_eq!(history[0].timing_quality, "tight");
    // Actual lateness stays visible even with a decoded target.
    queries::update_dj_transition_fire_timing_with_runtime_target(
        &conn,
        id,
        405006,
        Some(403000),
        "fired",
        true,
        "rendered_handoff",
        "none",
    )
    .unwrap();
    let history = latest_dj_transition_timing_history(&conn, 5).unwrap();
    assert_eq!(history[0].timing_delta_ms, Some(2006));
    assert_eq!(history[0].timing_quality, "bad");
}

#[test]
fn timing_direction_labels_delta_direction_and_status() {
    assert_eq!(timing_direction(Some("fired"), Some(150)), "on_time");
    assert_eq!(timing_direction(Some("fired"), Some(-151)), "early");
    assert_eq!(timing_direction(Some("fired"), Some(151)), "late");
    assert_eq!(timing_direction(Some("late"), Some(0)), "late");
    assert_eq!(timing_direction(Some("missed"), None), "missed");
    assert_eq!(timing_direction(Some("armed"), None), "pending");
    assert_eq!(timing_direction(Some("fired"), None), "unknown");
}

#[test]
fn timing_history_summary_counts_quality_and_status() {
    let events = vec![
        DjTimingHistoryEvent {
            event_id: 1,
            from_title: Some("A".to_string()),
            from_artist: Some("Artist A".to_string()),
            to_title: Some("B".to_string()),
            to_artist: Some("Artist B".to_string()),
            planned_template: "SafeCrossfade".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            planning_reason: None,
            planned_start_ms: Some(10_000),
            runtime_planned_start_ms: None,
            actual_start_ms: Some(10_100),
            timing_delta_ms: Some(100),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            timing_quality: "tight".to_string(),
            timing_direction: "on_time".to_string(),
            runtime_rendered_dj_mixer: Some(true),
            runtime_renderer_status: Some("rendered_handoff".to_string()),
            runtime_renderer_reason: Some("none".to_string()),
            started_at: "now".to_string(),
            rejected_alternatives: Vec::new(),
        },
        DjTimingHistoryEvent {
            event_id: 2,
            from_title: Some("B".to_string()),
            from_artist: Some("Artist B".to_string()),
            to_title: Some("C".to_string()),
            to_artist: Some("Artist C".to_string()),
            planned_template: "SafeCrossfade".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            planning_reason: Some("next_profile_missing".to_string()),
            planned_start_ms: Some(20_000),
            runtime_planned_start_ms: None,
            actual_start_ms: Some(20_800),
            timing_delta_ms: Some(800),
            timing_source: Some("beat_sync".to_string()),
            timing_status: Some("late".to_string()),
            timing_quality: "loose".to_string(),
            timing_direction: "late".to_string(),
            runtime_rendered_dj_mixer: Some(false),
            runtime_renderer_status: Some("legacy_overlap".to_string()),
            runtime_renderer_reason: Some("next_deck_not_decoded".to_string()),
            started_at: "now".to_string(),
            rejected_alternatives: Vec::new(),
        },
        DjTimingHistoryEvent {
            event_id: 3,
            from_title: Some("C".to_string()),
            from_artist: Some("Artist C".to_string()),
            to_title: Some("D".to_string()),
            to_artist: Some("Artist D".to_string()),
            planned_template: "SafeCrossfade".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            planning_reason: Some("analysis_late".to_string()),
            planned_start_ms: Some(30_000),
            runtime_planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: Some("fallback_overlap".to_string()),
            timing_status: Some("missed".to_string()),
            timing_quality: "bad".to_string(),
            timing_direction: "missed".to_string(),
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            started_at: "now".to_string(),
            rejected_alternatives: Vec::new(),
        },
    ];

    let summary = summarize_timing_history(&events, &[100, 800, -1_200, 40]);

    assert_eq!(summary.event_count, 3);
    assert_eq!(summary.average_delta_ms, Some(450));
    assert_eq!(summary.average_abs_delta_ms, Some(450));
    assert_eq!(summary.median_abs_delta_ms, Some(450));
    assert_eq!(summary.worst_abs_delta_ms, Some(1_200));
    assert_eq!(summary.tight_count, 1);
    assert_eq!(summary.loose_count, 1);
    assert_eq!(summary.bad_count, 1);
    assert_eq!(summary.late_count, 1);
    assert_eq!(summary.missed_count, 1);
}

#[test]
fn timing_history_summary_excludes_impossible_deltas_from_averages() {
    let events = vec![
        DjTimingHistoryEvent {
            event_id: 1,
            from_title: None,
            from_artist: None,
            to_title: None,
            to_artist: None,
            planned_template: "SafeCrossfade".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            planning_reason: None,
            planned_start_ms: Some(10_000),
            runtime_planned_start_ms: None,
            actual_start_ms: Some(10_100),
            timing_delta_ms: Some(100),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            timing_quality: "tight".to_string(),
            timing_direction: "on_time".to_string(),
            runtime_rendered_dj_mixer: Some(true),
            runtime_renderer_status: Some("rendered_handoff".to_string()),
            runtime_renderer_reason: Some("none".to_string()),
            started_at: "now".to_string(),
            rejected_alternatives: Vec::new(),
        },
        DjTimingHistoryEvent {
            event_id: 2,
            from_title: None,
            from_artist: None,
            to_title: None,
            to_artist: None,
            planned_template: "SafeCrossfade".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            planning_reason: None,
            planned_start_ms: Some(10_000),
            runtime_planned_start_ms: None,
            actual_start_ms: Some(50_001),
            timing_delta_ms: Some(40_001),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            timing_quality: "bad".to_string(),
            timing_direction: "late".to_string(),
            runtime_rendered_dj_mixer: Some(true),
            runtime_renderer_status: Some("rendered_handoff".to_string()),
            runtime_renderer_reason: Some("none".to_string()),
            started_at: "now".to_string(),
            rejected_alternatives: Vec::new(),
        },
    ];

    let summary = summarize_timing_history(&events, &[100]);

    assert_eq!(summary.event_count, 2);
    assert_eq!(summary.average_delta_ms, Some(100));
    assert_eq!(summary.average_abs_delta_ms, Some(100));
    assert_eq!(summary.tight_count, 1);
    assert_eq!(summary.bad_count, 1);
}

#[test]
fn fire_ahead_evidence_requires_positive_majority_and_median() {
    let passing = vec![
        220, 210, 205, 200, 195, 190, 185, 180, 175, 170, 165, 160, 155, 151, 149, -20, -40, -60,
        -80, -100,
    ];
    let mixed = vec![
        220, 210, 205, 200, 195, 190, 185, 180, 175, 170, -165, -160, -155, -151, -149, -20, -40,
        -60, -80, -100,
    ];
    let low_median = vec![
        151, 151, 150, 150, 149, 149, 148, 148, 147, 147, 146, 146, 145, 145, 144, -20, -40, -60,
        -80, -100,
    ];

    assert!(fire_ahead_evidence_passes(&passing));
    assert!(!fire_ahead_evidence_passes(&mixed));
    assert!(!fire_ahead_evidence_passes(&low_median));
    assert!(!fire_ahead_evidence_passes(&passing[..19]));
}

#[test]
fn ready_pair_transition_is_due_only_near_track_end() {
    assert!(!ready_pair_transition_due(90_000, Some(180_000)));
    assert!(ready_pair_transition_due(151_000, Some(180_000)));
    assert!(ready_pair_transition_due(180_000, Some(180_000)));
    assert!(!ready_pair_transition_due(151_000, None));
}

#[test]
fn drop_preview_selects_nearest_safe_mid_song_marker() {
    let mut current = test_deck_status(true, "ready");
    current.phrase_markers_ms = vec![40_000, 120_000];
    current.downbeat_markers_ms = vec![132_000, 190_000];

    assert_eq!(
        select_drop_preview_fire_ms(&current, Some(240_000)),
        Some(132_000)
    );
}

#[test]
fn drop_preview_rejects_unsafe_mid_song_window() {
    let mut current = test_deck_status(true, "ready");
    current.phrase_markers_ms = vec![40_000, 190_000];
    current.downbeat_markers_ms = vec![59_000];

    assert_eq!(select_drop_preview_fire_ms(&current, Some(240_000)), None);
}

#[test]
fn drop_preview_prefers_manual_drop_marker() {
    let mut next = test_deck_status(true, "ready");
    next.drop_markers_ms = vec![32_000];
    next.manual_drop_markers_ms = vec![24_000];

    assert_eq!(incoming_drop_marker(Some(&next)), Some((24_000, "manual")));
}

#[test]
fn drop_preview_status_arms_compatible_ready_pair() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    seed_dsp_key(&conn, 1, "8A");
    seed_dsp_key(&conn, 2, "8B");
    let current_ref = DjMediaRef::TidalTrack {
        tidal_id: 111,
        track_id: Some(1),
    };
    let next_ref = DjMediaRef::TidalTrack {
        tidal_id: 222,
        track_id: Some(2),
    };
    let mut current = test_deck_status(true, "ready");
    current.phrase_markers_ms = vec![128_000];
    let mut next = test_deck_status(true, "ready");
    next.drop_markers_ms = vec![32_000];

    let status = drop_preview_status(
        &conn,
        true,
        Some(&current_ref),
        Some(&next_ref),
        Some(&current),
        Some(&next),
        Some(240_000),
        None,
    )
    .expect("preview status");

    assert_eq!(
        status,
        DjDropPreviewStatus {
            status: "armed".to_string(),
            planned_fire_ms: Some(128_000),
            actual_fire_ms: None,
            incoming_drop_ms: Some(32_000),
            source: Some("profile".to_string()),
            reason: None,
        }
    );
}

#[test]
fn drop_preview_status_reports_actual_fire() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    seed_dsp_key(&conn, 1, "8A");
    seed_dsp_key(&conn, 2, "8B");
    let current_ref = DjMediaRef::TidalTrack {
        tidal_id: 111,
        track_id: Some(1),
    };
    let next_ref = DjMediaRef::TidalTrack {
        tidal_id: 222,
        track_id: Some(2),
    };
    let mut current = test_deck_status(true, "ready");
    current.phrase_markers_ms = vec![128_000];
    let mut next = test_deck_status(true, "ready");
    next.drop_markers_ms = vec![32_000];

    let status = drop_preview_status(
        &conn,
        true,
        Some(&current_ref),
        Some(&next_ref),
        Some(&current),
        Some(&next),
        Some(240_000),
        Some(128_008),
    )
    .expect("preview status");

    assert_eq!(status.status, "fired");
    assert_eq!(status.planned_fire_ms, Some(128_000));
    assert_eq!(status.actual_fire_ms, Some(128_008));
}

#[test]
fn drop_preview_status_skips_harmonic_mismatch() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    seed_dsp_key(&conn, 1, "8A");
    seed_dsp_key(&conn, 2, "2B");
    let current_ref = DjMediaRef::TidalTrack {
        tidal_id: 111,
        track_id: Some(1),
    };
    let next_ref = DjMediaRef::TidalTrack {
        tidal_id: 222,
        track_id: Some(2),
    };
    let mut current = test_deck_status(true, "ready");
    current.phrase_markers_ms = vec![128_000];
    let mut next = test_deck_status(true, "ready");
    next.drop_markers_ms = vec![32_000];

    let status = drop_preview_status(
        &conn,
        true,
        Some(&current_ref),
        Some(&next_ref),
        Some(&current),
        Some(&next),
        Some(240_000),
        None,
    )
    .expect("preview status");

    assert_eq!(status.status, "skipped");
    assert_eq!(status.reason.as_deref(), Some("harmonic_incompatible"));
}

#[test]
fn drop_preview_status_reports_retrying_asset_unavailable() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let current_ref = DjMediaRef::TidalTrack {
        tidal_id: 111,
        track_id: Some(1),
    };
    let next_ref = DjMediaRef::TidalTrack {
        tidal_id: 222,
        track_id: Some(2),
    };
    let mut current = test_deck_status(true, "ready");
    current.phrase_markers_ms = vec![128_000];
    let mut next = test_deck_status(false, "retrying");
    next.profile_retry_reason = Some("asset_not_ready".to_string());

    let status = drop_preview_status(
        &conn,
        true,
        Some(&current_ref),
        Some(&next_ref),
        Some(&current),
        Some(&next),
        Some(240_000),
        None,
    )
    .expect("preview status");

    assert_eq!(status.status, "skipped");
    assert_eq!(
        status.reason.as_deref(),
        Some("next_profile_retrying_asset_not_ready")
    );
}

#[test]
fn ready_pair_transition_planning_cooldown_suppresses_same_generation() {
    let mut attempts = HashMap::new();
    let now = Instant::now();
    let inside_retry = now + Duration::from_secs(DJ_READY_PAIR_PLANNING_RETRY_SECS - 1);
    let after_retry = now + Duration::from_secs(DJ_READY_PAIR_PLANNING_RETRY_SECS + 1);

    assert!(claim_ready_pair_transition_planning_at(
        &mut attempts,
        32335,
        4,
        now
    ));
    assert!(!claim_ready_pair_transition_planning_at(
        &mut attempts,
        32335,
        4,
        inside_retry
    ));
    assert!(claim_ready_pair_transition_planning_at(
        &mut attempts,
        32335,
        5,
        inside_retry
    ));
    assert!(claim_ready_pair_transition_planning_at(
        &mut attempts,
        32335,
        4,
        after_retry
    ));
}

#[test]
fn pair_planning_status_reports_server_state() {
    let ready_current = test_deck_status(true, "ready");
    let ready_next = test_deck_status(true, "ready");
    let missing_next = test_deck_status(false, "missing");
    let retrying_next = test_deck_status(false, "retrying");
    let failed_next = test_deck_status(false, "decode_failed");
    let armed_transition = OpenTransition {
        id: 31,
        template: "SafeCrossfade".to_string(),
        renderer_template: Some("SafeCrossfade".to_string()),
        fallback_reason: None,
        planned_start_ms: Some(180_000),
        actual_start_ms: None,
        timing_delta_ms: None,
        timing_source: Some("downbeat_sync".to_string()),
        timing_status: Some("armed".to_string()),
        overlay_details: None,
        runtime_rendered_dj_mixer: None,
        runtime_renderer_status: None,
        runtime_renderer_reason: None,
        rejected_alternatives: Vec::new(),
    };

    assert_eq!(
        pair_planning_status(false, Some(&ready_current), Some(&ready_next), None, false),
        "disabled"
    );
    assert_eq!(
        pair_planning_status(true, None, Some(&ready_next), None, false),
        "pair_missing"
    );
    assert_eq!(
        pair_planning_status(true, Some(&ready_current), Some(&failed_next), None, false),
        "profile_failed"
    );
    assert_eq!(
        pair_planning_status(true, Some(&ready_current), Some(&missing_next), None, false),
        "waiting_for_profiles"
    );
    assert_eq!(
        pair_planning_status(true, Some(&ready_current), Some(&retrying_next), None, true),
        "waiting_for_profiles"
    );
    assert!(ready_pair_can_request_transition_planning(
        Some(&ready_current),
        Some(&retrying_next)
    ));
    assert!(!ready_pair_can_request_transition_planning(
        Some(&ready_current),
        Some(&failed_next)
    ));
    assert_eq!(
        pair_planning_status(
            true,
            Some(&ready_current),
            Some(&ready_next),
            Some(&armed_transition),
            true
        ),
        "armed"
    );
    assert_eq!(
        pair_planning_status(true, Some(&ready_current), Some(&ready_next), None, false),
        "waiting_for_window"
    );
    assert_eq!(
        pair_planning_status(true, Some(&ready_current), Some(&ready_next), None, true),
        "ready_to_plan"
    );
}

#[test]
fn profile_rebuild_inflight_reports_running_for_recent_duplicate() {
    let inflight = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let first = mark_dj_profile_rebuild_inflight(
        &inflight,
        "tidal_track:250295727",
        std::time::Duration::from_secs(60),
    )
    .expect("first mark");
    let second = mark_dj_profile_rebuild_inflight(
        &inflight,
        "tidal_track:250295727",
        std::time::Duration::from_secs(60),
    )
    .expect("second mark");

    assert_eq!(first, ProfileRebuildInflightDecision::Start);
    assert_eq!(second, ProfileRebuildInflightDecision::AlreadyRunning);
}

#[test]
fn forced_profile_rebuild_bypasses_recent_inflight_marker() {
    let inflight = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let first = mark_dj_profile_rebuild_inflight(
        &inflight,
        "tidal_track:250295728",
        std::time::Duration::from_secs(60),
    )
    .expect("first mark");
    let forced = mark_dj_profile_rebuild_inflight(
        &inflight,
        "tidal_track:250295728",
        std::time::Duration::ZERO,
    )
    .expect("force mark");

    assert_eq!(first, ProfileRebuildInflightDecision::Start);
    assert_eq!(forced, ProfileRebuildInflightDecision::Start);
}

#[test]
fn retryable_profile_rebuild_errors_are_retrying() {
    let error = anyhow::anyhow!("DASH stream prebuffer failed");

    assert_eq!(profile_rebuild_failure_status(&error), "retrying");
    assert_eq!(
        profile_rebuild_error_message(&error, "retrying"),
        "DASH stream prebuffer failed. Retrying analysis."
    );
}

#[test]
fn exhausted_quality_asset_not_ready_stops_automatic_analysis() {
    let error = anyhow::Error::from(tidal_stream::StreamResolveError::StreamRejected {
        message: r#"TIDAL rejected playback request with 401 Unauthorized: {"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#.to_string(),
    }).context("Resolving the LOSSLESS analysis stream");

    assert_eq!(profile_rebuild_failure_status(&error), "source_unavailable");
    assert_eq!(
        profile_rebuild_error_message(&error, "source_unavailable"),
        "Track is unavailable on TIDAL. Automatic analysis stopped."
    );
    assert_eq!(
        next_dj_profile_analysis_quality(0, &error),
        Some("LOSSLESS")
    );
    assert_eq!(next_dj_profile_analysis_quality(1, &error), None);
}

#[test]
fn unresolved_pending_rows_do_not_requeue_unsupported_analysis() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let incoming = DjMediaRef::TidalTrack {
        tidal_id: 864210,
        track_id: None,
    };
    let pair = crate::playback::dj_lookahead::DjLookaheadPair {
        current: Some(DjMediaRef::PendingQueueItem {
            queue_item_id: 43,
            pending_artist: "Unavailable artist".to_string(),
            pending_title: "Unavailable track".to_string(),
            tidal_id_hint: None,
        }),
        next: Some(incoming.clone()),
        current_queue_item_id: Some(43),
        next_queue_item_id: Some(44),
        queue_generation: 1,
    };
    let inflight = Arc::new(Mutex::new(HashMap::new()));
    let missing = missing_dj_profile_refs_for_pair(&conn, pair, &[], &inflight)
        .expect("automatic analysis candidates");
    assert_eq!(missing, vec![incoming]);
}

#[test]
fn cached_profile_does_not_erase_known_unavailable_asset() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 864211,
        track_id: None,
    };
    let key = media_ref.profile_key();
    let rebuild_key = dj_profile_inflight_key(&key);
    queries::upsert_audio_dj_profile(&conn, &test_profile_row(&key, DJ_PROFILE_VERSION))
        .expect("cached profile");
    record_dj_profile_rebuild_failure(
        &rebuild_key,
        "source_unavailable",
        "Unavailable asset".to_string(),
    );
    let deck = deck_status(&conn, &media_ref, None, false).expect("deck status");
    assert!(deck.profile_ready);
    assert_eq!(deck.profile_status, "source_unavailable");
    assert!(!deck_needs_profile_rebuild(&deck));
    assert!(recent_dj_profile_rebuild_failure(&rebuild_key).is_some());
    let ready = test_deck_status(true, "ready");
    let unavailable_current = drop_preview_status(
        &conn,
        true,
        Some(&media_ref),
        Some(&media_ref),
        Some(&deck),
        Some(&ready),
        Some(240_000),
        None,
    )
    .expect("unavailable outgoing preview");
    assert_eq!(
        unavailable_current.reason.as_deref(),
        Some("current_source_unavailable")
    );
    let unavailable_next = drop_preview_status(
        &conn,
        true,
        Some(&media_ref),
        Some(&media_ref),
        Some(&ready),
        Some(&deck),
        Some(240_000),
        None,
    )
    .expect("unavailable incoming preview");
    assert_eq!(
        unavailable_next.reason.as_deref(),
        Some("next_source_unavailable")
    );
    clear_dj_profile_rebuild_failure(&rebuild_key);
}

#[test]
fn fresh_tidal_resolution_clears_only_unavailable_suppression() {
    let unavailable_key = "tidal_track:864213";
    let transient_key = "tidal_track:864214";
    clear_dj_profile_rebuild_failure(unavailable_key);
    clear_dj_profile_rebuild_failure(transient_key);
    record_unavailable_tidal_source(864213);
    record_dj_profile_rebuild_failure(
        transient_key,
        "retrying",
        "DASH stream prebuffer failed".to_string(),
    );
    assert!(recent_dj_profile_rebuild_failure(unavailable_key).is_some());
    clear_unavailable_tidal_source(864213);
    clear_unavailable_tidal_source(864214);
    assert!(recent_dj_profile_rebuild_failure(unavailable_key).is_none());
    assert_eq!(
        recent_dj_profile_rebuild_failure(transient_key)
            .expect("transient attempts survive resolution")
            .attempts,
        1
    );
    clear_dj_profile_rebuild_failure(transient_key);
}

#[test]
fn typed_tidal_transient_failures_keep_quality_fallback_and_retry() {
    for status in [
        reqwest::StatusCode::REQUEST_TIMEOUT,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let error = anyhow::Error::from(tidal_stream::StreamResolveError::UpstreamHttp {
            status,
            body: "Temporary upstream failure".to_string(),
        })
        .context("Resolving analysis stream");
        assert_eq!(
            profile_rebuild_failure_status(&error),
            "retrying",
            "{status}"
        );
        assert_eq!(
            next_dj_profile_analysis_quality(0, &error),
            Some("LOSSLESS"),
            "{status}"
        );
    }
    let rejection = anyhow::Error::from(tidal_stream::StreamResolveError::StreamRejected {
        message: "TIDAL rejected playback request with 403 Forbidden".to_string(),
    })
    .context("Resolving analysis stream");
    assert_eq!(
        profile_rebuild_failure_status(&rejection),
        "source_unavailable"
    );
    assert_eq!(next_dj_profile_analysis_quality(0, &rejection), None);
}

#[tokio::test]
async fn known_unavailable_source_does_not_start_another_automatic_batch() {
    let db = crate::db::Database::open_in_memory().expect("db");
    db.with_conn(|conn| {
        crate::db::schema::run_migrations(conn)?;
        Ok(())
    })
    .expect("migrations");
    let (dj_tx, _dj_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = fresh_test_state_with_dj_tx(db, Some(dj_tx.clone()));
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 864215,
        track_id: None,
    };
    let rebuild_key = dj_profile_inflight_key(&media_ref.profile_key());
    record_unavailable_tidal_source(864215);
    let response = queue_tidal_profile_rebuild(state.clone(), media_ref, dj_tx, false)
        .await
        .expect("automatic batch decision");
    assert!(!response.accepted);
    assert_eq!(response.status, "source_unavailable");
    assert!(!dj_profile_rebuild_is_inflight(
        &state.read().await.dj_profile_rebuild_inflight,
        &rebuild_key
    ));
    let failure = recent_dj_profile_rebuild_failure(&rebuild_key).expect("terminal suppression");
    assert_eq!(failure.attempts, 1);
    assert!(failure.next_retry_at.is_none());
    clear_dj_profile_rebuild_failure(&rebuild_key);
}

#[test]
fn deck_status_exposes_recent_profile_decode_failure() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 12198473,
        track_id: None,
    };
    let key = media_ref.profile_key();
    let rebuild_key = dj_profile_inflight_key(&key);
    clear_dj_profile_rebuild_failure(&rebuild_key);
    record_dj_profile_rebuild_failure(
        &rebuild_key,
        "decode_failed",
        "DASH stream prebuffer failed".to_string(),
    );

    let deck = deck_status(&conn, &media_ref, None, false).expect("deck status");

    assert!(!deck.profile_ready);
    assert_eq!(deck.profile_status, "decode_failed");
    assert_eq!(
        deck.profile_error.as_deref(),
        Some("DASH stream prebuffer failed")
    );
    assert!(!deck_needs_profile_rebuild(&deck));

    clear_dj_profile_rebuild_failure(&rebuild_key);
}

#[test]
fn deck_status_exposes_recent_profile_retrying_failure() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 12198475,
        track_id: None,
    };
    let key = media_ref.profile_key();
    let rebuild_key = dj_profile_inflight_key(&key);
    clear_dj_profile_rebuild_failure(&rebuild_key);
    record_dj_profile_rebuild_failure(
        &rebuild_key,
        "retrying",
        "DASH stream prebuffer failed. Retrying analysis.".to_string(),
    );

    let deck = deck_status(&conn, &media_ref, None, false).expect("deck status");

    assert!(!deck.profile_ready);
    assert_eq!(deck.profile_status, "retrying");
    assert_eq!(
        deck.profile_error.as_deref(),
        Some("DASH stream prebuffer failed. Retrying analysis.")
    );
    assert!(deck.profile_retry_after_ms.is_some_and(|ms| ms > 0));
    assert!(deck.profile_retry_after_ms.is_some_and(|ms| ms <= 25_000));
    assert_eq!(deck.profile_retry_reason.as_deref(), Some("dash_prebuffer"));
    assert!(!deck_needs_profile_rebuild(&deck));

    clear_dj_profile_rebuild_failure(&rebuild_key);
}

#[test]
fn due_retrying_profile_failure_needs_rebuild() {
    let mut deck = test_deck_status(false, "retrying");
    deck.profile_retry_after_ms = Some(0);

    assert!(deck_needs_profile_rebuild(&deck));
}

#[test]
fn outdated_dj_analysis_preserves_failure_backoff_and_retries_only_when_due() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 129999877,
        track_id: None,
    };
    let key = media_ref.profile_key();
    let failure_key = dj_profile_inflight_key(&key);
    clear_dj_profile_rebuild_failure(&failure_key);
    let mut row = test_profile_row(&key, "dj_profile_v2");
    row.source = "dj_playback".into();
    row.waveform_peaks_blob = encode_f32_blob(&[0.2, 0.5]);
    queries::upsert_audio_dj_profile(&conn, &row).unwrap();
    record_dj_profile_rebuild_failure(
        &failure_key,
        "retrying",
        "DASH stream prebuffer failed".into(),
    );
    let pair = crate::playback::dj_lookahead::DjLookaheadPair {
        current: Some(media_ref.clone()),
        next: None,
        current_queue_item_id: None,
        next_queue_item_id: None,
        queue_generation: 0,
    };
    let inflight = Arc::new(Mutex::new(HashMap::new()));
    assert!(
        missing_dj_profile_refs_for_pair(&conn, pair.clone(), &[], &inflight)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        recent_dj_profile_rebuild_failure(&failure_key)
            .unwrap()
            .attempts,
        1
    );
    profile_rebuild_failures()
        .lock()
        .unwrap()
        .get_mut(&failure_key)
        .unwrap()
        .next_retry_at = Some(Instant::now() - Duration::from_secs(1));
    assert_eq!(
        missing_dj_profile_refs_for_pair(&conn, pair.clone(), &[], &inflight).unwrap(),
        vec![media_ref]
    );
    for _ in 1..DJ_PROFILE_MAX_TRANSIENT_ATTEMPTS {
        record_dj_profile_rebuild_failure(
            &failure_key,
            "retrying",
            "DASH stream prebuffer failed".into(),
        );
    }
    assert!(
        missing_dj_profile_refs_for_pair(&conn, pair, &[], &inflight)
            .unwrap()
            .is_empty()
    );
    clear_dj_profile_rebuild_failure(&failure_key);
}

#[test]
fn exhausted_analysis_does_not_restart_after_five_minutes_of_polling() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 129999878,
        track_id: None,
    };
    let key = dj_profile_inflight_key(&media_ref.profile_key());
    clear_dj_profile_rebuild_failure(&key);
    for _ in 0..DJ_PROFILE_MAX_TRANSIENT_ATTEMPTS {
        record_dj_profile_rebuild_failure(&key, "retrying", "DASH stream prebuffer failed".into());
    }
    profile_rebuild_failures()
        .lock()
        .unwrap()
        .get_mut(&key)
        .unwrap()
        .recorded_at =
        Instant::now() - Duration::from_secs(DJ_PROFILE_REBUILD_FAILURE_TTL_SECS + 60);
    let pair = crate::playback::dj_lookahead::DjLookaheadPair {
        current: Some(media_ref),
        next: None,
        current_queue_item_id: None,
        next_queue_item_id: None,
        queue_generation: 0,
    };
    let inflight = Arc::new(Mutex::new(HashMap::new()));
    assert!(
        missing_dj_profile_refs_for_pair(&conn, pair, &[], &inflight)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        recent_dj_profile_rebuild_failure(&key).unwrap().status,
        "decode_failed"
    );
    clear_dj_profile_rebuild_failure(&key);
}

#[test]
fn missing_waveform_respects_analysis_inflight_backoff_and_exhaustion() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 129999879,
        track_id: None,
    };
    let profile_key = media_ref.profile_key();
    let key = dj_profile_inflight_key(&profile_key);
    clear_dj_profile_rebuild_failure(&key);
    let mut row = test_profile_row(&profile_key, DJ_PROFILE_VERSION);
    row.source = "dj_playback_measured".into();
    row.waveform_peaks_blob.clear();
    queries::upsert_audio_dj_profile(&conn, &row).unwrap();
    let pair = crate::playback::dj_lookahead::DjLookaheadPair {
        current: Some(media_ref),
        next: None,
        current_queue_item_id: None,
        next_queue_item_id: None,
        queue_generation: 0,
    };
    let inflight = Arc::new(Mutex::new(HashMap::from([(key.clone(), Instant::now())])));
    assert!(
        missing_dj_profile_refs_for_pair(&conn, pair.clone(), &[], &inflight)
            .unwrap()
            .is_empty()
    );
    inflight.lock().unwrap().clear();
    record_dj_profile_rebuild_failure(&key, "retrying", "DASH stream prebuffer failed".into());
    assert!(
        missing_dj_profile_refs_for_pair(&conn, pair.clone(), &[], &inflight)
            .unwrap()
            .is_empty()
    );
    for _ in 1..DJ_PROFILE_MAX_TRANSIENT_ATTEMPTS {
        record_dj_profile_rebuild_failure(&key, "retrying", "DASH stream prebuffer failed".into());
    }
    assert!(
        missing_dj_profile_refs_for_pair(&conn, pair.clone(), &[], &inflight)
            .unwrap()
            .is_empty()
    );
    // Imported profiles without waveforms need the same retry protection.
    row.source = "manual_import".into();
    queries::upsert_audio_dj_profile(&conn, &row).unwrap();
    assert!(
        missing_dj_profile_refs_for_pair(&conn, pair, &[], &inflight)
            .unwrap()
            .is_empty()
    );
    clear_dj_profile_rebuild_failure(&key);
}

#[test]
fn asset_not_ready_retrying_profile_reports_retry_reason() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 12198476,
        track_id: None,
    };
    let key = media_ref.profile_key();
    let rebuild_key = dj_profile_inflight_key(&key);
    clear_dj_profile_rebuild_failure(&rebuild_key);
    record_dj_profile_rebuild_failure(
        &rebuild_key,
        "retrying",
        "TIDAL asset is not ready. Retrying analysis.".to_string(),
    );

    let deck = deck_status(&conn, &media_ref, None, false).expect("deck status");

    assert_eq!(deck.profile_status, "retrying");
    assert_eq!(
        deck.profile_retry_reason.as_deref(),
        Some("asset_not_ready")
    );
    assert!(deck.profile_retry_after_ms.is_some_and(|ms| ms > 0));
    assert!(deck.profile_retry_after_ms.is_some_and(|ms| ms <= 25_000));

    clear_dj_profile_rebuild_failure(&rebuild_key);
}

#[test]
fn retryable_profile_rebuild_failure_clears_inflight() {
    let inflight = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = "tidal_track:250295729";
    let first = mark_dj_profile_rebuild_inflight(
        &inflight,
        key,
        std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS),
    )
    .expect("first mark");

    finish_dj_profile_rebuild_failure(
        &inflight,
        key,
        "retrying",
        "TIDAL asset is not ready. Retrying analysis.".to_string(),
    );

    let second = mark_dj_profile_rebuild_inflight(
        &inflight,
        key,
        std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS),
    )
    .expect("second mark");

    assert_eq!(first, ProfileRebuildInflightDecision::Start);
    assert_eq!(second, ProfileRebuildInflightDecision::Start);
    clear_dj_profile_rebuild_failure(key);
}

#[test]
fn profile_rebuild_backoff_grows_and_caps() {
    assert_eq!(profile_rebuild_backoff(1), Duration::from_secs(25));
    assert_eq!(profile_rebuild_backoff(2), Duration::from_secs(50));
    assert_eq!(profile_rebuild_backoff(3), Duration::from_secs(100));
    assert_eq!(profile_rebuild_backoff(4), Duration::from_secs(200));
    assert_eq!(
        profile_rebuild_backoff(50),
        Duration::from_secs(DJ_PROFILE_MAX_RETRY_BACKOFF_SECS)
    );
}

#[test]
fn retrying_profile_gives_up_after_attempt_cap() {
    // A permanently-failing stream (e.g. one that only resolves to ad
    // segments) must back off and finally stop, not loop forever.
    let key = "tidal_track:988877665";
    clear_dj_profile_rebuild_failure(key);

    let mut last_delay = Duration::ZERO;
    for _ in 1..DJ_PROFILE_MAX_TRANSIENT_ATTEMPTS {
        let delay = record_dj_profile_rebuild_failure(
            key,
            "retrying",
            "DASH stream prebuffer failed. Retrying analysis.".to_string(),
        )
        .expect("still retrying before the cap");
        assert!(delay >= last_delay, "backoff must not shrink");
        last_delay = delay;
    }

    // The capped attempt gives up: no retry is scheduled and the failure is
    // terminal, so deck_needs_profile_rebuild stops re-queuing it.
    let final_delay = record_dj_profile_rebuild_failure(
        key,
        "retrying",
        "DASH stream prebuffer failed. Retrying analysis.".to_string(),
    );
    assert!(final_delay.is_none(), "loop must stop at the attempt cap");
    let failure = recent_dj_profile_rebuild_failure(key).expect("failure recorded");
    assert_eq!(failure.status, "decode_failed");
    assert!(failure.retry_reason.is_none());

    clear_dj_profile_rebuild_failure(key);
}

#[test]
fn deck_status_exposes_inflight_profile_as_analyzing() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::schema::run_migrations(&conn).expect("migrations");
    let media_ref = DjMediaRef::TidalTrack {
        tidal_id: 12198474,
        track_id: None,
    };

    let deck = deck_status(&conn, &media_ref, None, true).expect("deck status");

    assert!(!deck.profile_ready);
    assert_eq!(deck.profile_status, "analyzing");
    assert!(deck.profile_error.is_none());
}
