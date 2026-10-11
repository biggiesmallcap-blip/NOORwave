use super::shared::{PlaybackBuffer, write_output_f32};
use super::*;
use crate::playback::gapless::GaplessPlan;
use crate::playback::output::cpal_shared::{effective_output_config, output_rate_fallback_config};
use crate::playback::player::PlaybackSourceKind;
use crate::playback::runtime::shared::{estimate_total_samples_from_duration_ms, samples_from_ms};
use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64},
};

#[test]
fn closed_runtime_command_channel_marks_handle_unhealthy() {
    let (command_tx, command_rx) = mpsc::channel();
    let handle = PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    assert!(handle.is_healthy());

    drop(command_rx);
    assert!(handle.pause().is_err());
    assert!(!handle.is_healthy());
}

#[test]
fn runtime_stream_resolver_overrides_static_access_token() {
    let resolver: RuntimeStreamResolver = Arc::new(|request| {
        Box::pin(async move {
            Ok(StreamInfo {
                url: "https://audio.example.test/init.mp4".to_string(),
                segment_urls: vec![],
                segment_offsets_ms: vec![],
                track_id: request.track_id,
                audio_quality: request.audio_quality,
                codec: "flac".to_string(),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
            })
        })
    });
    let config = PlaybackRuntimeConfig::new(reqwest::Client::new(), "expired-token", None)
        .with_stream_resolver(resolver);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let info = rt
        .block_on(config.resolve_stream(StreamRequest::new(42, "LOW")))
        .expect("stream info");

    assert_eq!(info.track_id, 42);
    assert_eq!(info.audio_quality, "LOW");
}

mod dj_lookahead {
    use super::*;

    fn library_ref(track_id: i64) -> DjMediaRef {
        DjMediaRef::LibraryTrack { track_id }
    }

    #[test]
    fn start_dj_lookahead_does_not_promote_next_engine() {
        let mut state = test_runtime_loop_state();
        state.next_engine = Some(test_engine_with_shared(2, 10));

        let outcome = start_dj_lookahead_in_state(
            &mut state,
            Some(library_ref(1)),
            Some(library_ref(2)),
            Some(11),
            Some(12),
            20,
            48_000,
        );

        assert_eq!(outcome, StartDjLookaheadOutcome::ReusedPreparedNext);
        assert!(state.engine.is_none());
        assert!(state.next_engine.is_some());
    }

    #[test]
    fn start_dj_lookahead_replaces_lower_hash_generation() {
        let mut state = test_runtime_loop_state();
        let outcome = start_dj_lookahead_in_state(
            &mut state,
            Some(library_ref(1)),
            Some(library_ref(2)),
            Some(11),
            Some(12),
            20,
            48_000,
        );
        assert_eq!(outcome, StartDjLookaheadOutcome::Started);

        let replacement = start_dj_lookahead_in_state(
            &mut state,
            Some(library_ref(3)),
            Some(library_ref(4)),
            Some(13),
            Some(14),
            19,
            48_000,
        );

        assert_eq!(replacement, StartDjLookaheadOutcome::Started);
        assert_eq!(
            state
                .dj
                .lookahead
                .as_ref()
                .map(|lookahead| lookahead.next_queue_item_id),
            Some(14)
        );
        assert!(state.dj.lookahead_failure.is_none());
    }

    #[test]
    fn prepared_program_rejected_when_pair_ids_change() {
        let mut state = test_runtime_loop_state();
        start_dj_lookahead_in_state(
            &mut state,
            Some(library_ref(1)),
            Some(library_ref(2)),
            Some(11),
            Some(12),
            20,
            48_000,
        );

        assert!(prepared_dj_lookahead_matches_pair(
            &state,
            20,
            Some(11),
            Some(12)
        ));
        assert!(!prepared_dj_lookahead_matches_pair(
            &state,
            20,
            Some(11),
            Some(99)
        ));
    }

    #[test]
    fn start_dj_lookahead_reuses_existing_prepared_next() {
        let mut state = test_runtime_loop_state();
        state.next_engine = Some(test_engine_with_shared(2, 10));

        let outcome = start_dj_lookahead_in_state(
            &mut state,
            Some(library_ref(1)),
            Some(library_ref(2)),
            Some(11),
            Some(12),
            20,
            48_000,
        );

        assert_eq!(outcome, StartDjLookaheadOutcome::ReusedPreparedNext);
        assert_eq!(
            state
                .dj
                .lookahead
                .as_ref()
                .map(|lookahead| lookahead.next.clone()),
            Some(library_ref(2))
        );
    }

    #[test]
    fn start_dj_lookahead_records_resolution_failure() {
        let mut state = test_runtime_loop_state();

        let outcome = start_dj_lookahead_in_state(
            &mut state,
            Some(library_ref(1)),
            None,
            Some(11),
            None,
            20,
            48_000,
        );

        assert_eq!(outcome, StartDjLookaheadOutcome::MissingNext);
        assert!(state.dj.lookahead.is_none());
        assert_eq!(
            state
                .dj
                .lookahead_failure
                .as_ref()
                .map(|failure| failure.reason),
            Some(DjLookaheadFailureReason::NextNotResolved)
        );
    }

    #[test]
    fn start_dj_lookahead_records_analysis_deadline() {
        let mut state = test_runtime_loop_state();

        start_dj_lookahead_in_state(
            &mut state,
            Some(library_ref(1)),
            Some(library_ref(2)),
            Some(11),
            Some(12),
            20,
            96_000,
        );

        assert_eq!(
            state
                .dj
                .lookahead
                .as_ref()
                .map(|lookahead| lookahead.deadline_samples),
            Some(96_000)
        );
    }

    #[test]
    fn missed_lookahead_deadline_does_not_delay_playback() {
        let mut state = test_runtime_loop_state();
        state.engine = Some(test_engine_with_shared(1, 10));

        state.dj.lookahead_failure = Some(DjLookaheadFailure {
            queue_generation: 20,
            current_queue_item_id: Some(11),
            next_queue_item_id: Some(12),
            reason: DjLookaheadFailureReason::AnalysisDeadlineMissed,
        });

        assert!(state.engine.is_some());
        assert_eq!(
            state
                .dj
                .lookahead_failure
                .as_ref()
                .map(|failure| failure.reason),
            Some(DjLookaheadFailureReason::AnalysisDeadlineMissed)
        );
    }
}

mod analysis_profile_key {
    use super::*;
    use crate::db::models::{AudioDjProfileKey, AudioDjProfileRow};
    use crate::db::{Database, queries};
    use crate::playback::decode::send_dj_analysis_job;
    use crate::playback::dj_lookahead::DjMediaRef;
    use crate::services::audio_analysis::dj_profile::{
        DJ_PROFILE_VERSION, encode_f32_blob, encode_u32_blob,
    };

    fn config(
        enabled: bool,
    ) -> (
        PlaybackRuntimeConfig,
        tokio::sync::mpsc::UnboundedReceiver<DjAnalysisJob>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            PlaybackRuntimeConfig::new(reqwest::Client::new(), "", None)
                .with_dj_analysis(enabled, Some(tx)),
            rx,
        )
    }

    fn send(
        enabled: bool,
        media_ref: DjMediaRef,
    ) -> Option<crate::services::audio_analysis::dj_profile::DjAnalysisJob> {
        let (config, mut rx) = config(enabled);
        let job = PreparedPlaybackJob::test_fixture(media_ref.track_id().unwrap_or(10), 7)
            .with_dj_media_ref(media_ref);
        send_dj_analysis_job(
            &config,
            job.dj_media_ref.clone(),
            &job,
            vec![0.0; 128],
            48_000,
            7,
        );
        rx.try_recv().ok()
    }

    #[test]
    fn dj_analysis_not_sent_when_engine_disabled() {
        let sent = send(false, DjMediaRef::LibraryTrack { track_id: 1 });
        assert!(sent.is_none());
    }

    #[test]
    fn active_decoder_sends_library_profile_key() {
        let sent = send(true, DjMediaRef::LibraryTrack { track_id: 1 }).expect("job");
        assert_eq!(sent.media_ref.profile_key().media_ref_kind, "library_track");
        assert_eq!(sent.media_ref.profile_key().media_ref_id, "1");
        assert_eq!(sent.track_id, Some(1));
    }

    #[test]
    fn prepared_next_decoder_sends_tidal_profile_key() {
        let sent = send(
            true,
            DjMediaRef::TidalTrack {
                tidal_id: 99,
                track_id: Some(10),
            },
        )
        .expect("job");
        assert_eq!(sent.media_ref.profile_key().media_ref_kind, "tidal_track");
        assert_eq!(sent.media_ref.profile_key().media_ref_id, "99");
        assert_eq!(sent.tidal_id, Some(99));
    }

    #[test]
    fn pending_next_decoder_sends_queue_item_profile_key_when_unresolved() {
        let sent = send(
            true,
            DjMediaRef::PendingQueueItem {
                queue_item_id: 44,
                pending_artist: "Artist".to_string(),
                pending_title: "Title".to_string(),
                tidal_id_hint: None,
            },
        )
        .expect("job");
        assert_eq!(sent.media_ref.profile_key().media_ref_kind, "queue_item");
        assert_eq!(sent.media_ref.profile_key().media_ref_id, "44");
        assert_eq!(sent.queue_item_id, Some(44));
    }

    #[test]
    fn pending_profile_promotes_after_tidal_resolution() {
        let db = Database::open_in_memory().expect("db");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| -> anyhow::Result<()> {
            conn.execute(
                "INSERT INTO queue (id, track_id, position, source, pending_artist, pending_title)
                 VALUES (44, NULL, 0, 'test', 'Artist', 'Title')",
                [],
            )?;
            let row = AudioDjProfileRow {
                media_ref_kind: "queue_item".to_string(),
                media_ref_id: "44".to_string(),
                track_id: None,
                queue_item_id: Some(44),
                tidal_id: None,
                profile_version: DJ_PROFILE_VERSION.to_string(),
                beat_grid_blob: encode_f32_blob(&[0.0, 0.5]),
                downbeats_blob: encode_f32_blob(&[0.0]),
                phrase_boundaries_blob: encode_u32_blob(&[0]),
                mix_in_blob: encode_f32_blob(&[]),
                mix_out_blob: encode_f32_blob(&[]),
                intro_end_seconds: None,
                outro_start_seconds: None,
                breakdown_blob: encode_f32_blob(&[]),
                drop_blob: encode_f32_blob(&[]),
                safe_transition_windows_blob: encode_f32_blob(&[]),
                energy_contour_blob: encode_f32_blob(&[]),
                vocal_presence_blob: encode_f32_blob(&[]),
                vocal_density_blob: encode_f32_blob(&[]),
                waveform_peaks_blob: encode_f32_blob(&[0.0, 1.0, 0.0]),
                lufs_loud_body: None,
                true_peak_dbtp: None,
                beat_confidence: Some(0.8),
                profile_confidence: 0.7,
                analysis_scope_ms: 30_000,
                is_temporary: true,
                source: "test".to_string(),
                computed_at: "now".to_string(),
            };
            queries::upsert_audio_dj_profile(conn, &row)?;
            queries::promote_temporary_audio_dj_profile(
                conn,
                &AudioDjProfileKey {
                    media_ref_kind: "queue_item".to_string(),
                    media_ref_id: "44".to_string(),
                },
                &AudioDjProfileKey {
                    media_ref_kind: "tidal_track".to_string(),
                    media_ref_id: "99".to_string(),
                },
                Some(99),
            )?;
            let stable = queries::get_audio_dj_profile(
                conn,
                &AudioDjProfileKey {
                    media_ref_kind: "tidal_track".to_string(),
                    media_ref_id: "99".to_string(),
                },
            )?
            .expect("stable profile");
            assert_eq!(stable.tidal_id, Some(99));
            Ok(())
        })
        .expect("promote");
    }
}

mod prepared_transition {
    use super::*;
    use crate::playback::player::PreparedTransitionProgram;

    fn program() -> noor_mix::TransitionProgram {
        noor_mix::TransitionProgram {
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
        }
    }

    fn transition(
        queue_generation: u64,
        current_queue_item_id: Option<i64>,
        next_queue_item_id: Option<i64>,
    ) -> PreparedTransitionProgram {
        PreparedTransitionProgram {
            program: program(),
            transition_event_id: None,
            fire_ahead_ms: 0,
            queue_generation,
            current_queue_item_id,
            next_queue_item_id,
            anchor_start_ms: None,
        }
    }

    fn state_with_pair() -> PlaybackRuntimeLoopState {
        let mut state = test_runtime_loop_state();
        start_dj_lookahead_in_state(
            &mut state,
            Some(DjMediaRef::LibraryTrack { track_id: 1 }),
            Some(DjMediaRef::LibraryTrack { track_id: 2 }),
            Some(11),
            Some(12),
            20,
            48_000,
        );
        state
    }

    #[test]
    fn prepare_next_preserves_transition_program() {
        let state = state_with_pair();
        let mut job = PreparedPlaybackJob::test_fixture(2, 7).with_prepared_transition(transition(
            20,
            Some(11),
            Some(12),
        ));

        assert!(!discard_stale_prepared_transition(&state, &mut job));
        assert!(job.prepared_transition.is_some());
    }

    #[test]
    fn legacy_prepare_next_has_no_transition_program() {
        let job = PreparedPlaybackJob::test_fixture(2, 7);

        assert!(job.prepared_transition.is_none());
    }

    #[test]
    fn prepared_transition_discarded_when_generation_is_stale() {
        let state = state_with_pair();
        let mut job = PreparedPlaybackJob::test_fixture(2, 7).with_prepared_transition(transition(
            19,
            Some(11),
            Some(12),
        ));

        assert!(discard_stale_prepared_transition(&state, &mut job));
        assert!(job.prepared_transition.is_none());
    }

    #[test]
    fn prepared_transition_discarded_when_next_queue_item_changes() {
        let state = state_with_pair();
        let mut job = PreparedPlaybackJob::test_fixture(2, 7).with_prepared_transition(transition(
            20,
            Some(11),
            Some(99),
        ));

        assert!(discard_stale_prepared_transition(&state, &mut job));
        assert!(job.prepared_transition.is_none());
    }
}

#[test]
fn runtime_constructs_mixer_before_audio_callback() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.25, 0.25, 0.25, 0.25]);

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    finish_engine_buffer(&next, &[0.5, 0.5, 0.5, 0.5]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    let prepared = state.dj.prepared_mixer.as_ref().expect("prepared mixer");
    assert_eq!(prepared.current_track_id, 1);
    assert_eq!(prepared.next_track_id, 2);

    // The transition audio is rendered at build time now, not at fire.
    assert!(!prepared.rendered.is_empty());
    assert!(prepared.rendered.iter().any(|sample| *sample != 0.0));
}

#[test]
fn runtime_constructs_mixer_from_decoded_transition_windows() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    {
        let mut buffer = active.shared.buffer.lock().expect("active buffer");
        buffer
            .samples
            .extend_from_slice(&[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
        buffer.read_pos = 2;
    }

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    {
        let mut buffer = next.shared.buffer.lock().expect("next buffer");
        buffer
            .samples
            .extend_from_slice(&[0.0, 0.0, 0.4, 0.4, 0.5, 0.5]);
    }

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    let prepared = state.dj.prepared_mixer.as_ref().expect("prepared mixer");
    assert_eq!(prepared.program.deck_a_start_frame, 1);
}

#[test]
fn runtime_rebuilds_mixer_from_latest_active_read_position() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    {
        let mut buffer = active.shared.buffer.lock().expect("active buffer");
        buffer
            .samples
            .extend_from_slice(&[0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4]);
    }

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    {
        let mut buffer = next.shared.buffer.lock().expect("next buffer");
        buffer
            .samples
            .extend_from_slice(&[0.0, 0.0, 0.4, 0.4, 0.5, 0.5]);
    }

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert_eq!(
        state
            .dj
            .prepared_mixer
            .as_ref()
            .expect("early mixer")
            .program
            .deck_a_start_frame,
        0
    );

    state
        .engine
        .as_ref()
        .expect("active engine")
        .shared
        .buffer
        .lock()
        .expect("active buffer")
        .read_pos = 4;

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert_eq!(
        state
            .dj
            .prepared_mixer
            .as_ref()
            .expect("rebuilt mixer")
            .program
            .deck_a_start_frame,
        2
    );
}

#[test]
fn runtime_prepared_mixer_honors_program_start_frames() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program.deck_a_start_frame = 1;
    transition.program.deck_b_start_frame = 2;

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    let prepared = state.dj.prepared_mixer.as_ref().expect("prepared mixer");

    assert!(
        prepared.rendered[..2]
            .iter()
            .all(|sample| (*sample - 0.7_f32).abs() < 1e-6)
    );
}

#[test]
fn runtime_rescales_program_frames_to_device_rate() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.3, 0.3]);

    // Program planned at half the device rate: every frame field must be
    // doubled before it can index the 48 kHz deck buffers.
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program.sample_rate = 24_000;
    transition.program.deck_b_start_frame = 1;

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    let prepared = state.dj.prepared_mixer.as_ref().expect("prepared mixer");
    assert_eq!(prepared.program.sample_rate, 48_000);
    assert_eq!(prepared.program.deck_b_start_frame, 2);
    assert_eq!(prepared.program.resolve_at, 4);
}

#[test]
fn installs_handoff_mixer_buffer_with_incoming_remainder() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
    active.shared.buffer.lock().expect("active buffer").read_pos = 2;

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    state
        .engine
        .as_ref()
        .expect("active engine")
        .shared
        .buffer
        .lock()
        .expect("active buffer")
        .read_pos = 2;
    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

    let next = state.next_engine.as_ref().expect("next engine");
    let buffer = next.shared.buffer.lock().expect("buffer lock");
    assert_samples_close(&buffer.samples, &[0.2, 0.2, 0.7, 0.7, 0.5, 0.5, 0.6, 0.6]);
    assert_eq!(buffer.read_pos, 0);
    assert!(buffer.finished);
    assert_eq!(next.shared.total_samples.load(Ordering::Relaxed), 8);
    assert_eq!(next.shared.crossfade_samples.load(Ordering::Relaxed), 0);
    assert!(state.dj.prepared_mixer.is_none());
}

#[test]
fn handoff_mixer_preserves_unfinished_next_estimated_total() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    {
        let mut buffer = next.shared.buffer.lock().expect("next buffer");
        buffer
            .samples
            .extend_from_slice(&[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);
    }

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

    let next = state.next_engine.as_ref().expect("next engine");
    let buffer = next.shared.buffer.lock().expect("buffer lock");
    assert!(!buffer.finished);
}

#[test]
fn installs_rate_adjusted_handoff_remainder_from_consumed_frames() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition
        .program
        .automation
        .push(noor_mix::AutomationEvent {
            param: noor_mix::Param::PlaybackRate(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: transition.program.resolve_at,
            from: 0.97,
            to: 0.97,
            curve: noor_mix::Curve::Linear,
        });

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

    let next = state.next_engine.as_ref().expect("next engine");
    let buffer = next.shared.buffer.lock().expect("buffer lock");
    assert_eq!(buffer.samples.len(), 10);
    assert_samples_close(&buffer.samples[4..], &[0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);
    assert_eq!(next.shared.total_samples.load(Ordering::Relaxed), 10);
}

#[test]
fn handoff_mixer_preserves_unfinished_next_buffer() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    let estimated_total_samples =
        estimate_total_samples_from_duration_ms(180_000, 48_000, 2).expect("estimated total");
    next.shared
        .total_samples
        .store(estimated_total_samples, Ordering::Relaxed);
    {
        let mut buffer = next.shared.buffer.lock().expect("next buffer");
        buffer
            .samples
            .extend_from_slice(&[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);
    }

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

    let next = state.next_engine.as_ref().expect("next engine");
    let buffer = next.shared.buffer.lock().expect("buffer lock");
    assert!(!buffer.finished);
    assert_eq!(
        next.shared.total_samples.load(Ordering::Relaxed),
        estimated_total_samples
    );
}

#[test]
fn drop_tease_program_starts_overlay_without_promotion() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2]);

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(99);
    transition.program.template = "DropTease16".to_string();
    transition.program.automation = vec![
        noor_mix::AutomationEvent {
            param: noor_mix::Param::DeckGain(noor_mix::DeckId::A),
            start_sample: 0,
            end_sample: transition.program.resolve_at,
            from: 0.0,
            to: 0.0,
            curve: noor_mix::Curve::Linear,
        },
        noor_mix::AutomationEvent {
            param: noor_mix::Param::DeckGain(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: transition.program.resolve_at,
            from: 1.0,
            to: 1.0,
            curve: noor_mix::Curve::Linear,
        },
    ];

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert_eq!(
        install_prepared_handoff_mixer_buffer(&mut state),
        Err(DjRuntimeRendererReason::ProgramNotMixerRenderable)
    );
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    assert!(
        start_prepared_overlay(
            &mut state,
            &event_tx,
            "fired",
            DjRuntimeRendererReason::None,
            None,
            None,
            48_000,
            2
        )
        .is_ok()
    );

    assert!(state.dj.prepared_mixer.is_none());
    assert_eq!(state.engine.as_ref().map(|engine| engine.track_id), Some(1));
    assert_eq!(
        state.next_engine.as_ref().map(|engine| engine.track_id),
        Some(2)
    );
    let next = state.next_engine.as_ref().expect("overlay engine");
    assert!(!next.shared.paused.load(Ordering::SeqCst));
    let buffer = next.shared.buffer.lock().expect("buffer lock");
    assert_samples_close(&buffer.samples, &[0.0, 0.0, 0.4, 0.4]);
    match event_rx.try_recv().expect("overlay event") {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            transition_event_id,
            actual_start_ms,
            timing_status,
            runtime_rendered_dj_mixer,
            runtime_renderer_status,
            runtime_renderer_reason,
            ..
        } => {
            assert_eq!(transition_event_id, 99);
            assert_eq!(actual_start_ms, 1_000);
            assert_eq!(timing_status, "fired");
            assert!(runtime_rendered_dj_mixer);
            assert_eq!(runtime_renderer_status, "rendered_overlay");
            assert_eq!(runtime_renderer_reason, "none");
        }
        other => panic!("expected overlay event, got {other:?}"),
    }
}

#[test]
fn prepared_overlay_reports_captured_fire_position() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2]);

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(100);
    transition.program.template = "DropTease16".to_string();
    transition.program.automation = vec![
        noor_mix::AutomationEvent {
            param: noor_mix::Param::DeckGain(noor_mix::DeckId::A),
            start_sample: 0,
            end_sample: transition.program.resolve_at,
            from: 0.0,
            to: 0.0,
            curve: noor_mix::Curve::Linear,
        },
        noor_mix::AutomationEvent {
            param: noor_mix::Param::DeckGain(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: transition.program.resolve_at,
            from: 1.0,
            to: 1.0,
            curve: noor_mix::Curve::Linear,
        },
    ];

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4]);

    state.engine = Some(active);
    state.next_engine = Some(next);
    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    assert!(
        start_prepared_overlay(
            &mut state,
            &event_tx,
            "fired",
            DjRuntimeRendererReason::None,
            Some(2_500),
            None,
            48_000,
            2
        )
        .is_ok()
    );

    match event_rx.try_recv().expect("overlay event") {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            actual_start_ms, ..
        } => {
            assert_eq!(actual_start_ms, 2_500);
        }
        other => panic!("expected overlay event, got {other:?}"),
    }
}

#[test]
fn drop_preview_overlay_starts_without_promotion_or_crossfade_window() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2]);

    let mut real_next = test_engine_with_shared(2, 21);
    real_next.shared.paused.store(true, Ordering::SeqCst);
    real_next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    finish_engine_buffer(&real_next, &[0.3, 0.3, 0.4, 0.4]);

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program.template = "DropPreview16".to_string();
    transition.program.automation = vec![
        noor_mix::AutomationEvent {
            param: noor_mix::Param::DeckGain(noor_mix::DeckId::A),
            start_sample: 0,
            end_sample: transition.program.resolve_at,
            from: 0.0,
            to: 0.0,
            curve: noor_mix::Curve::Linear,
        },
        noor_mix::AutomationEvent {
            param: noor_mix::Param::DeckGain(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: transition.program.resolve_at,
            from: 0.65,
            to: 0.65,
            curve: noor_mix::Curve::Linear,
        },
    ];
    let mut preview = test_engine_with_shared(2, 21);
    preview.shared.paused.store(true, Ordering::SeqCst);
    preview.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&preview, &[0.0, 0.0, 0.4, 0.4]);

    state.engine = Some(active);
    state.next_engine = Some(real_next);
    state.drop_preview_engine = Some(preview);

    assert!(prepare_drop_preview_mixer(&mut state, 64).is_ok());
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    assert!(start_prepared_drop_preview_overlay(&mut state, &event_tx, 120_000).is_ok());

    assert!(state.dj.prepared_drop_preview_mixer.is_none());
    assert_eq!(state.engine.as_ref().map(|engine| engine.track_id), Some(1));
    assert_eq!(
        state.next_engine.as_ref().map(|engine| engine.track_id),
        Some(2)
    );
    assert!(
        state
            .next_engine
            .as_ref()
            .expect("real next engine")
            .shared
            .paused
            .load(Ordering::SeqCst)
    );
    let active = state.engine.as_ref().expect("active engine");
    assert_eq!(active.shared.crossfade_samples.load(Ordering::Relaxed), 0);
    let preview = state.drop_preview_engine.as_ref().expect("preview engine");
    assert!(!preview.shared.paused.load(Ordering::SeqCst));
    let buffer = preview.shared.buffer.lock().expect("preview buffer");
    assert_samples_close(&buffer.samples, &[0.0, 0.0, 0.26, 0.26]);
    match event_rx.try_recv().expect("preview event") {
        PlaybackRuntimeEvent::DropPreviewStarted {
            track_id,
            generation,
            actual_start_ms,
            ..
        } => {
            assert_eq!(track_id, 1);
            assert_eq!(generation, 20);
            assert_eq!(actual_start_ms, 120_000);
        }
        other => panic!("expected preview event, got {other:?}"),
    }
    assert!(
        event_rx.try_recv().is_err(),
        "preview must not finish outgoing"
    );
}

fn verified_preview_fixture() -> PlaybackRuntimeLoopState {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );
    let active = test_engine_with_shared(1, 20);
    let mut pcm = vec![0.0; 18 * 48_000 * 2];
    for beat in 0..36 {
        let start = ((0.02 + beat as f64 * 0.5) * 48_000.0) as usize;
        for frame in 0..2400 {
            let t = frame as f64 / 48_000.0;
            let kick =
                (0.4 * (2.0 * std::f64::consts::PI * 60.0 * t).cos() * (-t * 80.0).exp()) as f32;
            for channel in 0..2 {
                if let Some(sample) = pcm.get_mut((start + frame) * 2 + channel) {
                    *sample = kick;
                }
            }
        }
    }
    finish_engine_buffer(&active, &pcm);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.anchor_start_ms = Some(1000);
    transition.program = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
    transition.program.template = "DropPreview16".into();
    transition.program.decision = Some(noor_mix::program::TransitionDecision {
        strategy: "DropPreview16".into(),
        confidence: 0.9,
        score: 0.8,
        reason: "Verified preview fixture".into(),
        energy_direction: "preview".into(),
        incoming_entry_seconds: 0.0,
        incoming_drop_seconds: None,
        outgoing_window: "mid_song_preview".into(),
        duration_beats: 24.0,
        candidates: vec![],
    });
    let mut preview = test_engine_with_shared(2, 20);
    preview.job = PreparedPlaybackJob::test_fixture(2, 20).with_prepared_transition(transition);
    preview.shared.paused.store(true, Ordering::SeqCst);
    finish_engine_buffer(&preview, &pcm);
    let next = test_engine_with_shared(3, 20);
    next.shared.paused.store(true, Ordering::SeqCst);
    state.engine = Some(active);
    state.next_engine = Some(next);
    state.drop_preview_engine = Some(preview);
    state
}

#[test]
fn preview_verifies_decoded_beats_and_joins_live_outgoing_clock() {
    let mut state = verified_preview_fixture();
    prepare_drop_preview_mixer(&mut state, 1024).unwrap();
    assert_eq!(
        state
            .dj
            .prepared_drop_preview_mixer
            .as_ref()
            .unwrap()
            .program
            .template,
        "DropPreview16"
    );
    let active = state.engine.as_ref().unwrap();
    active.shared.buffer.lock().unwrap().read_pos = 3 * 48_000 * 2;
    active
        .shared
        .position_samples
        .store(3 * 48_000 * 2, Ordering::Relaxed);
    install_prepared_drop_preview_mixer_buffer(&mut state).unwrap();
    let preview = state.drop_preview_engine.as_ref().unwrap();
    let rendered_len = preview.shared.buffer.lock().unwrap().samples.len();
    preview
        .shared
        .append_decoded_samples(&[0.99; 1024])
        .unwrap();
    assert_eq!(
        preview.shared.buffer.lock().unwrap().samples.len(),
        rendered_len,
        "late decoder data must not extend a bounded rendered preview into the original song"
    );
    assert_eq!(
        state
            .drop_preview_engine
            .as_ref()
            .unwrap()
            .shared
            .buffer
            .lock()
            .unwrap()
            .read_pos,
        2 * 48_000 * 2
    );
    assert_eq!(state.engine.as_ref().unwrap().track_id, 1);
    assert_eq!(state.next_engine.as_ref().unwrap().track_id, 3);
    assert!(
        state
            .next_engine
            .as_ref()
            .unwrap()
            .shared
            .paused
            .load(Ordering::SeqCst)
    );
}

#[test]
fn unverified_preview_clears_stale_render_without_handoff_fallback() {
    let mut state = verified_preview_fixture();
    prepare_drop_preview_mixer(&mut state, 1024).unwrap();
    let preview = state.drop_preview_engine.as_ref().unwrap();
    preview.shared.buffer.lock().unwrap().samples.fill(0.2);
    assert_eq!(
        prepare_drop_preview_mixer(&mut state, 1024),
        Err(DjRuntimeRendererReason::MixerRejected)
    );
    assert!(state.dj.prepared_drop_preview_mixer.is_none());
    assert!(state.dj.prepared_mixer.is_none());
    assert_eq!(state.engine.as_ref().unwrap().track_id, 1);
    assert_eq!(state.next_engine.as_ref().unwrap().track_id, 3);
    assert!(
        state
            .drop_preview_engine
            .as_ref()
            .unwrap()
            .shared
            .paused
            .load(Ordering::SeqCst)
    );
}

#[test]
fn preview_does_not_join_after_its_bass_swap_window() {
    let mut state = verified_preview_fixture();
    prepare_drop_preview_mixer(&mut state, 1024).unwrap();
    let active = state.engine.as_ref().unwrap();
    active.shared.buffer.lock().unwrap().read_pos = 14 * 48_000 * 2;
    active
        .shared
        .position_samples
        .store(14 * 48_000 * 2, Ordering::Relaxed);
    assert_eq!(
        install_prepared_drop_preview_mixer_buffer(&mut state),
        Err(DjRuntimeRendererReason::HandoffSeamTooLate)
    );
    assert_eq!(state.engine.as_ref().unwrap().track_id, 1);
    assert_eq!(state.next_engine.as_ref().unwrap().track_id, 3);
}

#[test]
fn arming_drop_preview_sets_absolute_trigger_without_crossfade_samples() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    active.shared.crossfade_samples.store(0, Ordering::Relaxed);
    state.engine = Some(active);

    assert!(arm_drop_preview_in_state(&state, 1, 20, 144_000));

    assert_eq!(
        state.next_engine.as_ref().map(|engine| engine.track_id),
        None
    );
    let active = state.engine.as_ref().expect("active engine");
    assert_eq!(active.shared.crossfade_samples.load(Ordering::Relaxed), 0);
    assert_eq!(
        active
            .shared
            .drop_preview_trigger_samples
            .load(Ordering::Relaxed),
        144_000
    );
}

#[test]
fn dj_flag_off_refuses_to_arm_drop_preview() {
    let mut state = test_runtime_loop_state();
    state.dj.engine_enabled = false;
    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .drop_preview_trigger_samples
        .store(99, Ordering::Relaxed);
    state.engine = Some(active);

    assert!(!arm_drop_preview_in_state(&state, 1, 20, 144_000));

    let active = state.engine.as_ref().expect("active engine");
    assert_eq!(
        active
            .shared
            .drop_preview_trigger_samples
            .load(Ordering::Relaxed),
        u64::MAX
    );
}

#[test]
fn mixer_promotion_seam_fades_outgoing_instead_of_hard_stop() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    let outgoing_stopped = Arc::clone(&active.shared.stopped);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(88);
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);

    state.engine = Some(active);
    state.next_engine = Some(next);
    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));

    promote_next_to_active(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
        "fired",
        None,
        None,
        DjRuntimeRendererOutcome::rendered_handoff(),
    );

    // The rendered mix carries the outgoing track's continuation, so the
    // live copy must ramp out over the seam window (then drain to its
    // natural end in the fading slot), never hard-cut mid-waveform.
    assert!(!outgoing_stopped.load(Ordering::SeqCst));
    let fading = state.fading_out_engine.as_ref().expect("fading engine");
    assert_eq!(fading.track_id, 1);
    assert_eq!(
        fading
            .shared
            .dj_fadeout_start_samples
            .load(Ordering::Relaxed),
        96_000
    );
    assert_eq!(state.engine.as_ref().map(|engine| engine.track_id), Some(2));
    match event_rx.try_recv().expect("timing event") {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            transition_event_id,
            actual_start_ms,
            timing_status,
            runtime_rendered_dj_mixer,
            runtime_renderer_status,
            runtime_renderer_reason,
            ..
        } => {
            assert_eq!(transition_event_id, 88);
            assert_eq!(actual_start_ms, 1_000);
            assert_eq!(timing_status, "fired");
            assert!(runtime_rendered_dj_mixer);
            assert_eq!(runtime_renderer_status, "rendered_handoff");
            assert_eq!(runtime_renderer_reason, "none");
        }
        other => panic!("expected timing event, got {other:?}"),
    }
}

#[test]
fn promotion_honors_user_pause_latch() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(89);
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);

    state.engine = Some(active);
    state.next_engine = Some(next);
    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

    // The user paused while the transition was already prepared. The
    // promoted deck must come up silent - promotions never un-pause.
    state.user_paused = true;

    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));

    promote_next_to_active(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
        "fired",
        None,
        None,
        DjRuntimeRendererOutcome::rendered_handoff(),
    );

    let promoted = state.engine.as_ref().expect("promoted engine");
    assert_eq!(promoted.track_id, 2);
    assert!(
        promoted.shared.paused.load(Ordering::SeqCst),
        "promotion must honor the user-pause latch"
    );
}

#[test]
fn advance_cascade_breaker_trips_after_repeated_silent_decks() {
    let mut state = test_runtime_loop_state();
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);

    for round in 1..=MAX_SILENT_START_STREAK {
        let mut engine = test_engine_with_shared(round as i64, u64::from(round));
        // Backdate construction so the silent deck counts as a failure,
        // not a rapid manual skip.
        engine.created_at = std::time::Instant::now() - SILENT_ENGINE_FAILURE_MIN_AGE * 2;
        state.engine = Some(engine);
        let tripped = evaluate_advance_cascade(&mut state, &event_tx);
        if round < MAX_SILENT_START_STREAK {
            assert!(!tripped, "streak {round} must not trip the breaker yet");
        } else {
            assert!(tripped, "breaker must trip at streak {round}");
        }
    }
    assert_eq!(
        state.silent_start_streak, 0,
        "streak resets after the breaker fires"
    );
    assert!(
        matches!(event_rx.try_recv(), Ok(PlaybackRuntimeEvent::Error { .. })),
        "breaker surfaces one clear error"
    );
    assert!(
        matches!(
            event_rx.try_recv(),
            Ok(PlaybackRuntimeEvent::Paused { track_id: None })
        ),
        "breaker emits Paused so DB/UI reconcile to a truthful state"
    );
}

#[test]
fn advance_cascade_ignores_young_decks_and_resets_on_audio() {
    let mut state = test_runtime_loop_state();
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);

    // Young silent deck (a rapid manual skip): streak untouched.
    state.engine = Some(test_engine_with_shared(1, 1));
    assert!(!evaluate_advance_cascade(&mut state, &event_tx));
    assert_eq!(state.silent_start_streak, 0);

    // Old silent deck: counts toward the streak.
    let mut old_engine = test_engine_with_shared(2, 2);
    old_engine.created_at = std::time::Instant::now() - SILENT_ENGINE_FAILURE_MIN_AGE * 2;
    state.engine = Some(old_engine);
    assert!(!evaluate_advance_cascade(&mut state, &event_tx));
    assert_eq!(state.silent_start_streak, 1);

    // A deck that actually produced audio resets the streak.
    let played = test_engine_with_shared(3, 3);
    played.shared.buffer.lock().expect("buffer lock").started = true;
    state.engine = Some(played);
    assert!(!evaluate_advance_cascade(&mut state, &event_tx));
    assert_eq!(state.silent_start_streak, 0);
}

#[test]
fn mixer_promotion_reports_captured_fire_position() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(101);
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);

    state.engine = Some(active);
    state.next_engine = Some(next);

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));

    promote_next_to_active(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
        "fired",
        Some(2_750),
        None,
        DjRuntimeRendererOutcome::legacy_overlap(DjRuntimeRendererReason::PreparedMixerMissing),
    );

    match event_rx.try_recv().expect("timing event") {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            actual_start_ms, ..
        } => {
            assert_eq!(actual_start_ms, 2_750);
        }
        other => panic!("expected timing event, got {other:?}"),
    }
}

#[test]
fn prepared_dj_program_arms_active_transition_window() {
    let mut state = test_runtime_loop_state();
    state.device_sample_rate = 48_000;
    state.device_channels = 2;
    let active = test_engine_with_shared(1, 20);
    assert_eq!(active.shared.crossfade_samples.load(Ordering::Relaxed), 0);
    state.engine = Some(active);

    let mut job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    job.gapless = GaplessPlan {
        enabled: true,
        overlap_ms: 1_000,
        prebuffer_ms: 500,
        requires_stream_metadata: false,
    };

    assert!(arm_active_transition_window(&mut state, &job));
    let active = state.engine.as_ref().expect("active engine");
    assert_eq!(
        active.shared.crossfade_samples.load(Ordering::Relaxed),
        96_000
    );
    assert!(
        !active
            .shared
            .crossfade_start_signaled
            .load(Ordering::Relaxed)
    );
}

#[test]
fn handoff_source_clock_survives_callback_decode_compaction_seek_and_next_anchor() {
    for seek_during_mix in [true, false] {
        let mut state = test_runtime_loop_state();
        start_dj_lookahead_in_state(
            &mut state,
            Some(DjMediaRef::LibraryTrack { track_id: 1 }),
            Some(DjMediaRef::LibraryTrack { track_id: 2 }),
            Some(11),
            Some(12),
            20,
            48_000,
        );
        let active = test_engine_with_shared(1, 20);
        finish_engine_buffer(&active, &vec![0.2; 120_000 * 2]);
        let original_b = (0..192_000 * 2)
            .map(|sample| 0.05 + (sample % 512) as f32 / 10_000.0)
            .collect::<Vec<_>>();
        let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
        transition.program =
            noor_mix::planner::long_harmonic_blend_program(48_000, 2, 1_000, 1.015625);
        transition.program.deck_b_start_frame = 96_000;
        let source_consumed = deck_b_consumed_frames(&transition.program).unwrap();
        assert_eq!(source_consumed, 48_750);
        let mut next = test_engine_with_shared(2, 21);
        next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
        next.shared
            .total_samples
            .store(192_000 * 2, Ordering::Relaxed);
        next.shared.buffer.lock().unwrap().samples = original_b[..160_000 * 2].to_vec();
        state.engine = Some(active);
        state.next_engine = Some(next);
        assert!(prepare_dj_mixer_for_pair(&mut state, 1024).is_ok());
        assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

        let (command_tx, _) = mpsc::channel();
        let mut handle = PlaybackRuntimeHandle::test_with_command_tx(command_tx);
        handle.handoff_elapsed_source = Arc::clone(&state.handoff_elapsed_source);
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        promote_next_to_active(
            &mut state,
            &event_tx,
            &handle.position_source,
            &handle.buffered_source,
            &handle.offset_source,
            "fired",
            Some(0),
            Some(0),
            DjRuntimeRendererOutcome::rendered_handoff(),
        );
        let incoming = state.engine.as_ref().unwrap();
        assert_eq!(
            handle.get_position_ms(48_000, 2),
            2_000,
            "report the incoming source cue"
        );
        assert_eq!(handle.get_dj_handoff_elapsed_ms(48_000, 2), Some(0));
        // Subsequent decoder packets are appended to the transformed
        // buffer, then EOF publishes offset + output-buffer length.
        {
            let mut guard = incoming.shared.buffer.lock().unwrap();
            guard.samples.extend_from_slice(&original_b[160_000 * 2..]);
            guard.mark_finished();
            let total = incoming
                .shared
                .position_offset_samples
                .load(Ordering::Relaxed)
                + guard.samples.len() as u64;
            incoming
                .shared
                .total_samples
                .store(total, Ordering::Relaxed);
        }
        assert_eq!(
            incoming
                .shared
                .output_to_source_samples(incoming.shared.total_samples.load(Ordering::Relaxed)),
            192_000 * 2
        );
        let frames_to_play = if seek_during_mix { 24_000 } else { 60_000 };
        let (callback_tx, _) = mpsc::channel();
        let mut output = vec![0.0; frames_to_play * 2];
        write_output_f32(&mut output, &incoming.shared, &callback_tx, &event_tx);
        let source_frame = if seek_during_mix { 120_375 } else { 156_750 };
        assert_eq!(
            incoming
                .shared
                .source_position_samples
                .load(Ordering::Relaxed),
            source_frame * 2
        );
        assert_eq!(
            handle.get_position_ms(48_000, 2),
            (source_frame * 1000 / 48_000) as i64
        );
        assert_eq!(handle.get_buffered_ms(48_000, 2), 4_000);
        assert_eq!(
            handle.get_dj_handoff_elapsed_ms(48_000, 2),
            if seek_during_mix { Some(500) } else { None }
        );
        assert!(incoming.shared.compact_consumed_buffer(6_000 * 2).unwrap() > 0);
        let source_offset = handle.buffered_start_samples();
        assert_eq!(
            source_offset,
            if seek_during_mix {
                0
            } else {
                incoming.shared.output_to_source_samples(
                    incoming
                        .shared
                        .position_offset_samples
                        .load(Ordering::Relaxed),
                )
            }
        );
        let seek_frame = if seek_during_mix { 48_000 } else { 180_000 };
        assert_eq!(
            evaluate_seek_decision(
                seek_frame * 2,
                source_offset,
                incoming.shared.buffered_samples.load(Ordering::Relaxed),
                true
            ),
            SeekDecision::Dispatch,
            "retained incoming intro must be available before its skipped cue"
        );

        // A future source-grid anchor is converted onto the current
        // output buffer clock, including cue/rate and compaction offset.
        let mut future = test_prepared_transition_program(20, Some(12), Some(13));
        future.anchor_start_ms = Some(3_500);
        let mut future_job =
            PreparedPlaybackJob::test_fixture(3, 21).with_prepared_transition(future.clone());
        future_job.gapless = GaplessPlan {
            enabled: true,
            overlap_ms: 500,
            prebuffer_ms: 500,
            requires_stream_metadata: false,
        };
        assert_eq!(
            anchored_deck_a_frame(&state, &future, incoming),
            Some(71_250 - (frames_to_play as u64 - 6_000))
        );
        assert!(arm_active_transition_window(&mut state, &future_job));
        let incoming = state.engine.as_ref().unwrap();
        assert_eq!(
            incoming
                .shared
                .dj_fire_trigger_samples
                .load(Ordering::Relaxed),
            71_250 * 2
        );

        // Seeking restores incoming source audio once. The retained
        // prefix handles a seek during the overlap; once compacted past
        // it, the pure B remainder needs only a source offset adjustment.
        assert!(incoming.shared.restore_source_buffer_after_seek().unwrap());
        assert!(incoming.shared.handoff_timeline.snapshot().is_none());
        assert_eq!(handle.get_dj_handoff_elapsed_ms(48_000, 2), None);
        assert_eq!(
            incoming
                .shared
                .dj_fire_trigger_samples
                .load(Ordering::Relaxed),
            168_000 * 2
        );
        let restored_offset = incoming
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed);
        assert_eq!(
            incoming.shared.buffer.lock().unwrap().samples,
            original_b[restored_offset as usize..]
        );
        incoming
            .shared
            .seek_target_samples
            .store(seek_frame * 2, Ordering::Relaxed);
        incoming
            .shared
            .set_manual_seek_crossfade_suppression(seek_frame * 2);
        let mut after_seek = [0.0; 256];
        write_output_f32(&mut after_seek, &incoming.shared, &callback_tx, &event_tx);
        assert_eq!(
            &after_seek[..],
            &original_b[(seek_frame * 2) as usize..(seek_frame * 2 + 256) as usize],
            "an accepted seek must play B source samples, never replay A's rendered mix"
        );
        assert_eq!(
            handle.get_position_ms(48_000, 2),
            ((seek_frame * 2 + 256) * 1000 / 96_000) as i64
        );
        assert!(!incoming.shared.restore_source_buffer_after_seek().unwrap());
    }
}

#[test]
fn callback_countdown_target_uses_decoded_end_and_survives_dispatch_delay() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .total_samples
        .store(409_000 * 96, Ordering::Relaxed);
    active
        .shared
        .position_samples
        .store(402_850 * 96, Ordering::Relaxed);
    active.shared.buffer.lock().unwrap().samples = vec![0.2; 256];
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.fire_ahead_ms = 150;
    transition.anchor_start_ms = None;
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    next.job.gapless = GaplessPlan {
        enabled: true,
        overlap_ms: 6_000,
        prebuffer_ms: 500,
        requires_stream_metadata: false,
    };
    state.engine = Some(active);
    state.next_engine = Some(next);
    let job = state.next_engine.as_ref().unwrap().job.clone();
    assert!(arm_active_transition_window(&mut state, &job));
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    write_output_f32(
        &mut [0.0; 256],
        &state.engine.as_ref().unwrap().shared,
        &command_tx,
        &event_tx,
    );
    let captured_target = match command_rx.try_recv().unwrap() {
        PlaybackRuntimeCommand::CrossfadeStart {
            trigger_target_samples,
            ..
        } => trigger_target_samples,
        other => panic!("expected captured countdown target, got {other:?}"),
    };
    assert_eq!(captured_target, 402_850 * 96);
    // If a decoder update changes its duration while the runtime command
    // waits, the fire still reports the threshold this callback observed.
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .total_samples
        .store(500_000 * 96, Ordering::Relaxed);
    assert_eq!(
        runtime_transition_target_ms(&state, Some(captured_target)),
        Some(403_000)
    );
    state
        .next_engine
        .as_mut()
        .unwrap()
        .job
        .prepared_transition
        .as_mut()
        .unwrap()
        .anchor_start_ms = Some(400_750);
    assert_eq!(
        runtime_transition_target_ms(&state, Some(captured_target)),
        Some(400_750)
    );
}

#[test]
fn prepared_dj_program_applies_fire_ahead_to_trigger_window() {
    let mut state = test_runtime_loop_state();
    state.device_sample_rate = 48_000;
    state.device_channels = 2;
    state.engine = Some(test_engine_with_shared(1, 20));

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.fire_ahead_ms = 231;
    let mut job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    job.gapless = GaplessPlan {
        enabled: true,
        overlap_ms: 1_000,
        prebuffer_ms: 500,
        requires_stream_metadata: false,
    };

    assert!(arm_active_transition_window(&mut state, &job));
    let active = state.engine.as_ref().expect("active engine");
    assert_eq!(
        active.shared.crossfade_samples.load(Ordering::Relaxed),
        118_176
    );
}

#[test]
fn beat_anchored_plan_arms_absolute_fire_trigger() {
    let mut state = test_runtime_loop_state();
    state.device_sample_rate = 48_000;
    state.device_channels = 2;
    state.engine = Some(test_engine_with_shared(1, 20));

    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    // Grid downbeat at 3:00.000 on the decoded-audio timeline.
    transition.anchor_start_ms = Some(180_000);
    let mut job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    job.gapless = GaplessPlan {
        enabled: true,
        overlap_ms: 1_000,
        prebuffer_ms: 500,
        requires_stream_metadata: false,
    };

    assert!(arm_active_transition_window(&mut state, &job));
    let active = state.engine.as_ref().expect("active engine");
    // 180s * 48_000 * 2ch interleaved samples.
    assert_eq!(
        active
            .shared
            .dj_fire_trigger_samples
            .load(Ordering::Relaxed),
        17_280_000
    );
    // The from-end window is still armed as the fade envelope + fallback.
    assert_eq!(
        active.shared.crossfade_samples.load(Ordering::Relaxed),
        96_000
    );
}

#[test]
fn gridless_plan_rearm_clears_stale_fire_trigger() {
    let mut state = test_runtime_loop_state();
    state.device_sample_rate = 48_000;
    state.device_channels = 2;
    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .dj_fire_trigger_samples
        .store(123_456, Ordering::Relaxed);
    state.engine = Some(active);

    let mut job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    job.gapless = GaplessPlan {
        enabled: true,
        overlap_ms: 1_000,
        prebuffer_ms: 500,
        requires_stream_metadata: false,
    };

    assert!(arm_active_transition_window(&mut state, &job));
    let active = state.engine.as_ref().expect("active engine");
    assert_eq!(
        active
            .shared
            .dj_fire_trigger_samples
            .load(Ordering::Relaxed),
        u64::MAX
    );
}

#[test]
fn handoff_install_skips_to_live_deck_a_position() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    // Pre-render with deck A at frame 0 (read_pos 0), like a build at
    // prepare time.
    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());

    // By the time the fire is handled, the live playhead has moved one
    // frame past the position the render starts at.
    state
        .engine
        .as_ref()
        .expect("active engine")
        .shared
        .buffer
        .lock()
        .expect("active buffer")
        .read_pos = 2;

    assert!(install_prepared_handoff_mixer_buffer(&mut state).is_ok());

    let next = state.next_engine.as_ref().expect("next engine");
    let buffer = next.shared.buffer.lock().expect("buffer lock");
    // Rendered mix is [0.1, 0.1, 0.6, 0.6] (frame 0: A only pre-sync,
    // frame 1: A frame 1 + B frame 1); joining one frame in plays the
    // transition from frame 1 so deck A stays continuous with the live
    // stream.
    assert_eq!(buffer.read_pos, 2);
    // position = offset + read_pos so this track's own future near-end /
    // fire math is not shifted by the seam offset.
    assert_eq!(next.shared.position_samples.load(Ordering::Relaxed), 2);
}

#[test]
fn rhythmic_render_locks_decoded_kicks_across_the_whole_overlap() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );
    fn kicks(bpm: f64, phase: f64, channel: usize, hz: f64) -> Vec<f32> {
        let mut pcm = vec![0.0; 48_000 * 18 * 2];
        let mut beat = phase;
        while beat < 18.0 {
            let start = (beat * 48_000.0).round() as usize;
            for frame in 0..2400 {
                let t = frame as f64 / 48_000.0;
                let sample = (2.0 * std::f64::consts::PI * hz * t).cos() * (-t * 80.0).exp();
                if (start + frame) * 2 + channel < pcm.len() {
                    pcm[(start + frame) * 2 + channel] += sample as f32;
                }
            }
            beat += 60.0 / bpm;
        }
        pcm
    }
    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &kicks(121.0, 0.173, 0, 60.0));
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
    transition.program.automation = [noor_mix::DeckId::A, noor_mix::DeckId::B]
        .into_iter()
        .map(|deck| noor_mix::AutomationEvent {
            param: noor_mix::Param::DeckGain(deck),
            start_sample: 0,
            end_sample: 576_000,
            from: 0.35,
            to: 0.35,
            curve: noor_mix::Curve::Linear,
        })
        .collect();
    transition
        .program
        .automation
        .push(noor_mix::AutomationEvent {
            param: noor_mix::Param::PlaybackRate(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: 576_000,
            from: 0.98,
            to: 0.98,
            curve: noor_mix::Curve::Linear,
        });
    transition.program.decision = Some(noor_mix::program::TransitionDecision {
        strategy: "BassSwap16".into(),
        confidence: 0.65,
        score: 0.8,
        reason: "Rhythmic overlap".into(),
        energy_direction: "steady".into(),
        incoming_entry_seconds: 0.0,
        incoming_drop_seconds: None,
        outgoing_window: "phrase_end".into(),
        duration_beats: 24.0,
        candidates: vec![],
    });
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &kicks(122.0, 0.031, 1, 110.0));
    state.engine = Some(active);
    state.next_engine = Some(next);
    prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
    let prepared = state.dj.prepared_mixer.as_ref().unwrap();
    let rate = prepared
        .program
        .automation
        .iter()
        .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
        .unwrap()
        .to as f64;
    assert!(
        (rate - 121.0 / 122.0).abs() < 0.001,
        "tempo lock must come from actual PCM, rate={rate}"
    );
    let cue = prepared.program.deck_b_start_frame as f64 / 48_000.0;
    let incoming_first = (0.031 + 60.0 / 122.0 - cue) / rate;
    assert!(
        (incoming_first - 0.173).abs() < 0.02,
        "phase must lock at the seam, cue={cue}"
    );
    // Verify actual rendered stereo PCM, with the two decks isolated into
    // different channels: every beat remains aligned, not just the first.
    for beat in 2..22 {
        let expected = (0.173 + beat as f64 * 60.0 / 121.0) * 48_000.0;
        let peak = |channel: usize| {
            let center = expected as usize;
            ((center - 1200)..(center + 1200))
                .max_by(|a, b| {
                    prepared.rendered[*a * 2 + channel]
                        .abs()
                        .total_cmp(&prepared.rendered[*b * 2 + channel].abs())
                })
                .unwrap()
        };
        assert!(
            peak(0).abs_diff(peak(1)) < 960,
            "rendered kicks drifted on beat {beat}"
        );
    }
    let verified_cue = prepared.program.deck_b_start_frame;
    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    let executed = &state
        .next_engine
        .as_ref()
        .unwrap()
        .job
        .prepared_transition
        .as_ref()
        .unwrap()
        .program;
    assert_eq!(executed.deck_b_start_frame, verified_cue);
    assert!(
        executed
            .decision
            .as_ref()
            .unwrap()
            .reason
            .contains("verified in the decoded mix window")
    );
}

#[test]
fn long_bass_mix_verifies_musical_prefixes_before_using_a_protected_overlap() {
    // All alternatives retain the same source starts. A missing/drifting
    // tail can shorten the overlap, but cannot certify an unstable prefix.
    for (name, jump_after, stop_after, incoming_seconds, expected_beats, expected_seconds) in [
        ("stable full window", None, None, 30, 48.0, 24),
        ("late phase jump", Some(18.0), None, 30, 32.0, 16),
        ("vanishing tail percussion", None, Some(18.0), 30, 32.0, 16),
        ("only longer prefix decoded", None, None, 20, 32.0, 16),
        ("only shorter prefix decoded", None, None, 11, 16.0, 8),
        ("unstable prefixes", Some(4.0), None, 30, 0.0, 4),
    ] {
        let mut state = test_runtime_loop_state();
        start_dj_lookahead_in_state(
            &mut state,
            Some(DjMediaRef::LibraryTrack { track_id: 1 }),
            Some(DjMediaRef::LibraryTrack { track_id: 2 }),
            Some(11),
            Some(12),
            20,
            48_000,
        );
        let kicks = |seconds: usize, jump: Option<f64>, stop: Option<f64>| {
            let mut pcm = vec![0.0; seconds * 48_000 * 2];
            for beat in 0..seconds * 2 {
                let time = 0.02 + beat as f64 * 0.5;
                if stop.is_some_and(|end| time >= end) {
                    break;
                }
                let actual = time
                    + if jump.is_some_and(|after| time >= after) {
                        0.17
                    } else {
                        0.0
                    };
                let start = (actual * 48_000.0).round() as usize;
                for frame in 0..2400 {
                    let t = frame as f64 / 48_000.0;
                    let kick =
                        ((2.0 * std::f64::consts::PI * 60.0 * t).cos() * (-t * 80.0).exp()) as f32;
                    for channel in 0..2 {
                        if let Some(sample) = pcm.get_mut((start + frame) * 2 + channel) {
                            *sample = kick;
                        }
                    }
                }
            }
            pcm
        };
        let active = test_engine_with_shared(1, 20);
        finish_engine_buffer(&active, &kicks(30, jump_after, stop_after));
        active
            .shared
            .total_samples
            .store(30 * 96_000, Ordering::Relaxed);
        active.shared.buffer.lock().unwrap().read_pos = 48_000 * 2;
        active
            .shared
            .position_samples
            .store(48_000 * 2, Ordering::Relaxed);
        let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
        transition.program = noor_mix::planner::bass_swap_32_program(48_000, 2, 24_000);
        transition.program.deck_a_start_frame = 48_000;
        transition.program.deck_b_start_frame = 48_000;
        transition.program.decision = Some(noor_mix::program::TransitionDecision {
            strategy: "BassSwap32".into(),
            confidence: 0.9,
            score: 0.8,
            reason: "Long rhythmic bass swap".into(),
            energy_direction: "steady".into(),
            incoming_entry_seconds: 1.0,
            incoming_drop_seconds: None,
            outgoing_window: "phrase_end".into(),
            duration_beats: 48.0,
            candidates: vec![],
        });
        let original = transition.program.clone();
        let mut next = test_engine_with_shared(2, 21);
        next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
        // The short-window cases deliberately never emit decode EOF.
        next.shared.buffer.lock().unwrap().samples = kicks(incoming_seconds, None, None);
        state.engine = Some(active);
        state.next_engine = Some(next);
        let buffer = crossfade_readiness_snapshot(state.next_engine.as_ref().unwrap()).unwrap();
        assert!(
            dj_crossfade_next_ready(&state, buffer, 24 * 96_000),
            "{name}"
        );
        assert!(dj_pcm_readiness_wakeup(&state).is_some(), "{name}");

        prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
        let prepared = state.dj.prepared_mixer.as_ref().unwrap();
        let expected_template = if expected_beats == 48.0 {
            "BassSwap32"
        } else if expected_beats > 0.0 {
            "BassSwap16"
        } else {
            "SafeCrossfade"
        };
        assert_eq!(prepared.program.template, expected_template, "{name}");
        assert_eq!(
            prepared.program.resolve_at,
            expected_seconds * 48_000,
            "{name}"
        );
        assert_eq!(
            prepared.program.decision.as_ref().unwrap().duration_beats,
            expected_beats,
            "{name}"
        );
        assert_eq!(
            prepared.program.deck_a_start_frame, original.deck_a_start_frame,
            "{name}"
        );
        assert!(
            prepared
                .program
                .deck_b_start_frame
                .abs_diff(original.deck_b_start_frame)
                < 960,
            "{name}"
        );
        if expected_beats > 0.0 && expected_beats < 48.0 {
            assert!(
                prepared
                    .program
                    .decision
                    .as_ref()
                    .unwrap()
                    .reason
                    .contains("shorter verified phrase"),
                "{name}"
            );
            // Keep the existing envelope/EQ shape and values; only its
            // musical timeline is compressed to the verified phrase.
            for event in original
                .automation
                .iter()
                .filter(|event| !matches!(event.param, noor_mix::Param::PlaybackRate(_)))
            {
                let actual = prepared
                    .program
                    .automation
                    .iter()
                    .find(|actual| {
                        actual.param == event.param
                            && actual.from == event.from
                            && actual.to == event.to
                    })
                    .unwrap();
                assert_eq!(actual.curve, event.curve);
            }
            noor_mix::planner::safety::validate_audio_safety(
                &prepared.program,
                &Default::default(),
            )
            .unwrap();
        }
        install_prepared_handoff_mixer_buffer(&mut state).unwrap();
        let incoming = state.next_engine.as_ref().unwrap();
        let executed = &incoming.job.prepared_transition.as_ref().unwrap().program;
        assert_eq!(executed.template, expected_template, "{name}");
        assert_eq!(
            executed.decision.as_ref().unwrap().duration_beats,
            expected_beats,
            "{name}"
        );
        assert_eq!(
            incoming
                .shared
                .handoff_timeline
                .snapshot()
                .unwrap()
                .source_start,
            executed.deck_b_start_frame,
            "{name}"
        );
    }
}

/// An offline benchmark of the same decoded-rhythm correction and
/// per-frame gain/EQ renderer used by prepared handoffs. Input synthesis
/// and PCM copies are excluded, and each run verifies its audible result.
#[test]
#[ignore = "offline DJ CPU benchmark: run with --ignored --nocapture"]
fn benchmark_verified_bass_render_96khz() {
    const SAMPLE_RATE: u32 = 96_000;
    const CHANNELS: u16 = 2;
    const SECONDS: usize = 24;
    const BLOCK_SAMPLES: usize = 1024;
    let thread_cpu_ms = || {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::FILETIME;
            use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
            let empty = || FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut created, mut ended, mut kernel, mut user) =
                (empty(), empty(), empty(), empty());
            // The current-thread pseudo handle is valid for this call,
            // and all four writable FILETIME pointers remain in scope.
            let succeeded = unsafe {
                GetThreadTimes(
                    GetCurrentThread(),
                    &mut created,
                    &mut ended,
                    &mut kernel,
                    &mut user,
                )
            };
            if succeeded == 0 {
                return None;
            }
            let ticks = |time: FILETIME| {
                (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
            };
            Some((ticks(kernel) + ticks(user)) as f64 / 10_000.0)
        }
        #[cfg(not(windows))]
        {
            None::<f64>
        }
    };
    let cpu_delta =
        |start: Option<f64>, end: Option<f64>| start.zip(end).map(|(start, end)| end - start);

    let kicks = |bpm: f64, phase: f64, channel: usize, frequency: f64| {
        // Extra decoded audio covers the verified incoming rate and cue
        // movement, rather than repeating or extrapolating a short clip.
        let mut pcm = vec![0.0; 30 * SAMPLE_RATE as usize * usize::from(CHANNELS)];
        for beat in 0..64 {
            let start =
                ((phase + beat as f64 * 60.0 / bpm) * f64::from(SAMPLE_RATE)).round() as usize;
            for frame in 0..SAMPLE_RATE as usize / 20 {
                let t = frame as f64 / f64::from(SAMPLE_RATE);
                let kick = (0.4
                    * (2.0 * std::f64::consts::PI * frequency * t).cos()
                    * (-t * 80.0).exp()) as f32;
                if let Some(sample) = pcm.get_mut((start + frame) * 2 + channel) {
                    *sample = kick;
                }
            }
        }
        pcm
    };
    let outgoing = kicks(121.0, 0.173, 0, 60.0);
    let incoming = kicks(122.0, 0.031, 1, 110.0);
    let mut original =
        noor_mix::planner::bass_swap_32_program(SAMPLE_RATE, CHANNELS, SECONDS as u32 * 1000);
    original.decision = Some(noor_mix::program::TransitionDecision {
        strategy: "BassSwap32".into(),
        confidence: 0.9,
        score: 0.8,
        reason: "Offline verified bass render benchmark".into(),
        energy_direction: "steady".into(),
        incoming_entry_seconds: 0.0,
        incoming_drop_seconds: None,
        outgoing_window: "phrase_end".into(),
        duration_beats: 48.0,
        candidates: vec![],
    });

    for run in 1..=3 {
        // Mixer takes ownership of decoded PCM. Allocate these copies
        // outside the timer so the two build profiles compare kernels.
        let deck_a = noor_mix::deck::DeckBuffer::new(outgoing.clone(), CHANNELS);
        let deck_b = noor_mix::deck::DeckBuffer::new(incoming.clone(), CHANNELS);
        let mut program = original.clone();
        let total_started = std::time::Instant::now();
        let total_cpu_started = thread_cpu_ms();
        let verification_started = std::time::Instant::now();
        let verification_cpu_started = thread_cpu_ms();
        let sync = beat_sync::synchronize_or_shorten_checked(&mut program, &outgoing, &incoming)
            .expect("stable complete kick trains must verify without fallback");
        let verification_ms = verification_started.elapsed().as_secs_f64() * 1000.0;
        let verification_cpu_ms = cpu_delta(verification_cpu_started, thread_cpu_ms());
        assert_eq!(program.template, "BassSwap32");
        assert_eq!(program.resolve_at, SECONDS as u64 * u64::from(SAMPLE_RATE));
        assert!((f64::from(sync.rate) - 121.0 / 122.0).abs() < 0.001);
        assert!(sync.confidence >= 0.5);
        assert!(sync.residual_ms <= 40.0);
        noor_mix::planner::safety::validate_audio_safety(&program, &Default::default()).unwrap();

        let prepare_started = std::time::Instant::now();
        let prepare_cpu_started = thread_cpu_ms();
        let mut mixer =
            noor_mix::Mixer::new(program.clone(), deck_a, deck_b, BLOCK_SAMPLES).unwrap();
        let prepare_ms = prepare_started.elapsed().as_secs_f64() * 1000.0;
        let prepare_cpu_ms = cpu_delta(prepare_cpu_started, thread_cpu_ms());
        let render_started = std::time::Instant::now();
        let render_cpu_started = thread_cpu_ms();
        let rendered = render_mixer_to_buffer(
            &mut mixer,
            program.resolve_at,
            usize::from(CHANNELS),
            BLOCK_SAMPLES,
        )
        .unwrap();
        let render_ms = render_started.elapsed().as_secs_f64() * 1000.0;
        let render_cpu_ms = cpu_delta(render_cpu_started, thread_cpu_ms());
        let total_ms = total_started.elapsed().as_secs_f64() * 1000.0;
        let total_cpu_ms = cpu_delta(total_cpu_started, thread_cpu_ms());

        assert_eq!(rendered.len(), SECONDS * SAMPLE_RATE as usize * 2);
        assert!(rendered.iter().all(|sample| sample.is_finite()));
        let peak = rendered
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(peak > 0.01 && peak <= 0.98 + f32::EPSILON);
        let channel_rms = |start: usize, end: usize, channel: usize| {
            let power = (start..end)
                .map(|frame| f64::from(rendered[frame * 2 + channel]).powi(2))
                .sum::<f64>();
            (power / (end - start) as f64).sqrt()
        };
        let early = [
            channel_rms(SAMPLE_RATE as usize, 5 * SAMPLE_RATE as usize, 0),
            channel_rms(SAMPLE_RATE as usize, 5 * SAMPLE_RATE as usize, 1),
        ];
        let late = [
            channel_rms(19 * SAMPLE_RATE as usize, 23 * SAMPLE_RATE as usize, 0),
            channel_rms(19 * SAMPLE_RATE as usize, 23 * SAMPLE_RATE as usize, 1),
        ];
        assert!(early[0] > early[1], "outgoing bass must own the opening");
        assert!(late[1] > late[0], "incoming bass must own the ending");

        // Preserve the real bass/gain automation. Channel separation lets
        // us measure both physical kick peaks during the audible handoff.
        let mut peak_phase_error_ms = 0.0_f64;
        for beat in [20, 22, 24, 26] {
            let expected_seconds = 0.173 + beat as f64 * 60.0 / 121.0;
            let expected_frame = expected_seconds * f64::from(SAMPLE_RATE);
            let radius = SAMPLE_RATE as usize / 25;
            let center = expected_frame.round() as usize;
            let peaks: Vec<_> = (0..2)
                .map(|channel| {
                    ((center - radius)..=(center + radius))
                        .max_by(|&a, &b| {
                            rendered[a * 2 + channel]
                                .abs()
                                .total_cmp(&rendered[b * 2 + channel].abs())
                        })
                        .unwrap()
                })
                .collect();
            let delta_ms = peaks[0].abs_diff(peaks[1]) as f64 / f64::from(SAMPLE_RATE) * 1000.0;
            peak_phase_error_ms = peak_phase_error_ms.max(delta_ms);
            assert!(
                delta_ms <= 25.0,
                "rendered kick phase error {delta_ms:.3}ms"
            );
        }
        let cue = program.deck_b_start_frame as f64 / f64::from(SAMPLE_RATE);
        let first_incoming = (0.031 + 60.0 / 122.0 - cue) / f64::from(sync.rate);
        assert!((first_incoming - 0.173).abs() < 0.02);
        println!(
            "DJ_CPU_BENCH run={run} sample_rate={SAMPLE_RATE} channels={CHANNELS} duration_s={SECONDS} samples={} verification_ms={verification_ms:.3} verification_cpu_ms={verification_cpu_ms:?} mixer_prepare_ms={prepare_ms:.3} mixer_prepare_cpu_ms={prepare_cpu_ms:?} render_ms={render_ms:.3} render_cpu_ms={render_cpu_ms:?} total_ms={total_ms:.3} total_cpu_ms={total_cpu_ms:?} render_realtime_ratio={:.5} rate={:.8} confidence={:.6} residual_ms={:.3} rendered_peak_phase_ms={peak_phase_error_ms:.3} peak={peak:.6} early_rms={early:?} late_rms={late:?}",
            rendered.len(),
            render_ms / (SECONDS as f64 * 1000.0),
            sync.rate,
            sync.confidence,
            sync.residual_ms,
        );
    }
}

#[test]
fn musical_bass_prefixes_do_not_alter_drops_loops_or_other_strategies() {
    let mut program = noor_mix::planner::bass_swap_32_program(48_000, 2, 24_000);
    program.decision = Some(noor_mix::program::TransitionDecision {
        strategy: "BassSwap32".into(),
        confidence: 0.9,
        score: 0.8,
        reason: "Bass swap".into(),
        energy_direction: "steady".into(),
        incoming_entry_seconds: 0.0,
        incoming_drop_seconds: None,
        outgoing_window: "phrase_end".into(),
        duration_beats: 48.0,
        candidates: vec![],
    });
    assert_eq!(beat_sync::musical_prefixes(&program).len(), 2);
    let mut drop = program.clone();
    drop.decision.as_mut().unwrap().incoming_drop_seconds = Some(24.0);
    assert!(beat_sync::musical_prefixes(&drop).is_empty());
    let mut verified_drop = program.clone();
    verified_drop.drop_source = Some("manual".into());
    assert!(beat_sync::musical_prefixes(&verified_drop).is_empty());
    let mut looped = program.clone();
    looped.loops.push(noor_mix::program::LoopRegion {
        deck: noor_mix::DeckId::B,
        start_frame: 0,
        end_frame: 48_000,
    });
    assert!(beat_sync::musical_prefixes(&looped).is_empty());
    program.template = "ClubMix".into();
    assert!(beat_sync::musical_prefixes(&program).is_empty());
}

#[test]
fn verified_drop_mix_preserves_source_cues_with_a_nonzero_incoming_offset() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );
    let mut pcm = vec![0.0; 18 * 48_000 * 2];
    for beat in 0..36 {
        let start = ((0.02 + beat as f64 * 0.5) * 48_000.0).round() as usize;
        for frame in 0..2400 {
            let t = frame as f64 / 48_000.0;
            let kick = ((2.0 * std::f64::consts::PI * 60.0 * t).cos() * (-t * 80.0).exp()) as f32;
            for channel in 0..2 {
                if let Some(sample) = pcm.get_mut((start + frame) * 2 + channel) {
                    *sample = kick;
                }
            }
        }
    }
    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &pcm);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
    transition.program.template = "DropSwap".into();
    transition.program.drop_source = Some("manual".into());
    transition.program.deck_b_start_frame = 2 * 48_000;
    let source_drop = 2.0 + transition.program.swap_start as f32 / 48_000.0;
    transition.program.decision = Some(noor_mix::program::TransitionDecision {
        strategy: "DropSwap".into(),
        confidence: 0.9,
        score: 0.8,
        reason: "Verified source drop".into(),
        energy_direction: "lift".into(),
        incoming_entry_seconds: 2.0,
        incoming_drop_seconds: Some(source_drop),
        outgoing_window: "phrase_end".into(),
        duration_beats: 24.0,
        candidates: vec![],
    });
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    // A restart decoded from source second one. Both its source cue and
    // drop remain absolute source positions, while Mixer indexes local PCM.
    next.shared
        .position_offset_samples
        .store(48_000 * 2, Ordering::Relaxed);
    next.shared
        .position_samples
        .store(48_000 * 2, Ordering::Relaxed);
    finish_engine_buffer(&next, &pcm[48_000 * 2..]);
    state.engine = Some(active);
    state.next_engine = Some(next);

    prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
    let prepared = state.dj.prepared_mixer.as_ref().unwrap();
    assert_eq!(prepared.program.template, "DropSwap");
    let source_cue = 48_000 + prepared.program.deck_b_start_frame;
    assert!(source_cue.abs_diff(2 * 48_000) <= 1440);
    let decision = prepared.program.decision.as_ref().unwrap();
    assert!(
        (f64::from(decision.incoming_entry_seconds) - source_cue as f64 / 48_000.0).abs()
            <= 1.0 / 48_000.0,
        "verified decision must describe the actual source cue"
    );
    assert_eq!(decision.incoming_drop_seconds, Some(source_drop));
    assert!(
        decision
            .reason
            .contains("verified in the decoded mix window")
    );
    let rate = prepared
        .program
        .automation
        .iter()
        .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
        .unwrap()
        .to;
    let arrival =
        decision.incoming_entry_seconds + prepared.program.swap_start as f32 * rate / 48_000.0;
    assert!((arrival - source_drop).abs() <= 0.03);

    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    let next = state.next_engine.as_ref().unwrap();
    let executed = &next.job.prepared_transition.as_ref().unwrap().program;
    assert_eq!(executed.template, "DropSwap");
    assert_eq!(executed.deck_b_start_frame, source_cue);
    assert_eq!(
        next.shared
            .handoff_timeline
            .snapshot()
            .unwrap()
            .source_start,
        source_cue
    );
    assert_eq!(
        executed.decision.as_ref().unwrap().incoming_drop_seconds,
        Some(source_drop)
    );
    assert!(
        (executed.decision.as_ref().unwrap().incoming_entry_seconds - source_cue as f32 / 48_000.0)
            .abs()
            <= 1.0 / 48_000.0
    );
}

#[test]
fn unverified_rhythmic_audio_uses_a_short_protected_render() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );
    let active = test_engine_with_shared(1, 20);
    let pcm: Vec<f32> = (0..48_000 * 18 * 2)
        .map(|sample| ((sample / 2) as f32 * 0.01).sin() * 0.2)
        .collect();
    finish_engine_buffer(&active, &pcm);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
    transition.program.decision = Some(noor_mix::program::TransitionDecision {
        strategy: "BassSwap16".into(),
        confidence: 0.65,
        score: 0.8,
        reason: "Rhythmic overlap".into(),
        energy_direction: "steady".into(),
        incoming_entry_seconds: 0.0,
        incoming_drop_seconds: None,
        outgoing_window: "phrase_end".into(),
        duration_beats: 24.0,
        candidates: vec![],
    });
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &pcm);
    state.engine = Some(active);
    state.next_engine = Some(next);
    prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
    let prepared = state.dj.prepared_mixer.as_ref().unwrap();
    assert_eq!(prepared.program.template, "SafeCrossfade");
    assert_eq!(prepared.program.resolve_at, 4 * 48_000);
    assert!(
        prepared
            .program
            .automation
            .iter()
            .all(|event| !matches!(event.param, noor_mix::Param::PlaybackRate(_)))
    );
    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    assert_eq!(
        state
            .next_engine
            .as_ref()
            .unwrap()
            .job
            .prepared_transition
            .as_ref()
            .unwrap()
            .program
            .template,
        "SafeCrossfade"
    );
}

fn state_with_unverified_rhythmic_tracks() -> PlaybackRuntimeLoopState {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );
    let active = test_engine_with_shared(1, 20);
    // Distinct outgoing levels reveal any replay of the earlier render.
    // Neither deck has percussion suitable for a long fixed-rate lock.
    let outgoing: Vec<f32> = (0..18 * 48_000 * 2)
        .map(|sample| if sample < 3 * 48_000 * 2 { 0.05 } else { 0.4 })
        .collect();
    finish_engine_buffer(&active, &outgoing);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(77);
    transition.anchor_start_ms = Some(0);
    transition.program = noor_mix::planner::bass_swap_16_program(48_000, 2, 12_000);
    transition.program.decision = Some(noor_mix::program::TransitionDecision {
        strategy: "BassSwap16".into(),
        confidence: 0.65,
        score: 0.8,
        reason: "Rhythmic overlap".into(),
        energy_direction: "steady".into(),
        incoming_entry_seconds: 0.0,
        incoming_drop_seconds: None,
        outgoing_window: "phrase_end".into(),
        duration_beats: 24.0,
        candidates: vec![],
    });
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &vec![0.1; 18 * 48_000 * 2]);
    active
        .shared
        .crossfade_samples
        .store(12 * 48_000 * 2, Ordering::Relaxed);
    next.shared
        .crossfade_samples
        .store(12 * 48_000 * 2, Ordering::Relaxed);
    state.engine = Some(active);
    state.next_engine = Some(next);
    state
}

fn state_with_late_unverified_rhythmic_mix() -> PlaybackRuntimeLoopState {
    let mut state = state_with_unverified_rhythmic_tracks();
    prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
    let prepared = state.dj.prepared_mixer.as_ref().unwrap();
    assert_eq!(prepared.program.template, "SafeCrossfade");
    assert_eq!(prepared.program.resolve_at, 4 * 48_000);
    let active = state.engine.as_ref().unwrap();
    active.shared.buffer.lock().unwrap().read_pos = 3 * 48_000 * 2;
    active
        .shared
        .position_samples
        .store(3 * 48_000 * 2, Ordering::Relaxed);
    state
}

#[test]
fn dj_readiness_prepares_when_pcm_arrives_before_decode_eof() {
    let mut state = state_with_unverified_rhythmic_tracks();
    state.user_paused = true;
    let incoming = state.next_engine.as_ref().unwrap();
    {
        let mut buffer = incoming.shared.buffer.lock().unwrap();
        buffer.finished = false;
        buffer.samples.truncate(8 * 48_000 * 2);
    }
    assert!(dj_pcm_readiness_wakeup(&state).is_none());
    incoming
        .shared
        .append_decoded_samples(&vec![0.1; 7 * 48_000 * 2])
        .unwrap();
    assert!(matches!(
        dj_pcm_readiness_wakeup(&state),
        Some(PlaybackRuntimeCommand::NextDecodeComplete {
            track_id: 2,
            generation: 21
        })
    ));
    // The existing completion handler prepares this paused pair without
    // advancing it. The prepared render becomes the successful-attempt latch.
    prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
    assert!(dj_pcm_readiness_wakeup(&state).is_none());
    assert_eq!(state.engine.as_ref().unwrap().track_id, 1);
    assert!(
        !state
            .next_engine
            .as_ref()
            .unwrap()
            .shared
            .buffer
            .lock()
            .unwrap()
            .finished
    );
}

#[test]
fn dj_readiness_recovers_a_missed_anchor_from_live_pcm_without_waiting_for_eof() {
    let mut state = state_with_unverified_rhythmic_tracks();
    let transition = state
        .next_engine
        .as_mut()
        .unwrap()
        .job
        .prepared_transition
        .as_mut()
        .unwrap();
    transition.anchor_start_ms = Some(1_000);
    transition.program.deck_b_start_frame = 2 * 48_000;
    let outgoing = state.engine.as_ref().unwrap();
    outgoing.shared.buffer.lock().unwrap().read_pos = 6 * 48_000 * 2;
    outgoing
        .shared
        .position_samples
        .store(6 * 48_000 * 2, Ordering::Relaxed);
    outgoing
        .shared
        .crossfade_start_signaled
        .store(true, Ordering::Relaxed);
    outgoing.shared.compact_consumed_buffer(48_000 * 2).unwrap();
    let incoming = state.next_engine.as_ref().unwrap();
    incoming
        .shared
        .position_offset_samples
        .store(48_000 * 2, Ordering::Relaxed);
    {
        let mut buffer = incoming.shared.buffer.lock().unwrap();
        buffer.finished = false;
        buffer.samples.truncate(6 * 48_000 * 2);
    }
    assert!(matches!(
        dj_pcm_readiness_wakeup(&state),
        Some(PlaybackRuntimeCommand::NextDecodeComplete {
            track_id: 2,
            generation: 21
        })
    ));
    assert_eq!(
        prepare_dj_mixer_for_pair(&mut state, 1024),
        Err(DjRuntimeRendererReason::ActiveDeckNotDecoded)
    );
    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    let incoming = state.next_engine.as_ref().unwrap();
    let executed = &incoming.job.prepared_transition.as_ref().unwrap().program;
    assert_eq!(executed.template, "SafeCrossfade");
    assert_eq!(executed.resolve_at, 4 * 48_000);
    assert_eq!(executed.deck_b_start_frame, 2 * 48_000);
    assert!(
        incoming.shared.buffer.lock().unwrap().samples[40 * 48 * 2] > 0.39,
        "join live outgoing audio, not its earlier anchor"
    );
    assert_eq!(
        incoming
            .shared
            .source_position_samples
            .load(Ordering::Relaxed),
        2 * 48_000 * 2,
        "preserve the incoming source cue across its buffer offset"
    );
}

#[test]
fn prepared_short_mix_uses_its_actual_pcm_budget_at_fire_and_retry() {
    let mut state = state_with_unverified_rhythmic_tracks();
    let incoming = state.next_engine.as_ref().unwrap();
    {
        let mut buffer = incoming.shared.buffer.lock().unwrap();
        buffer.finished = false;
        buffer.samples.truncate(25 * 48_000); // 12.5 seconds, stereo.
    }
    let buffer = crossfade_readiness_snapshot(incoming).unwrap();
    assert!(!dj_crossfade_next_ready(&state, buffer, 12 * 96_000));
    prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
    assert_eq!(
        state.dj.prepared_mixer.as_ref().unwrap().program.template,
        "SafeCrossfade"
    );
    assert!(dj_crossfade_next_ready(&state, buffer, 12 * 96_000));
    let outgoing = state.engine.as_ref().unwrap();
    outgoing
        .shared
        .crossfade_start_signaled
        .store(true, Ordering::Relaxed);
    outgoing.shared.buffer.lock().unwrap().read_pos = 48_000 * 2;
    outgoing
        .shared
        .position_samples
        .store(48_000 * 2, Ordering::Relaxed);
    assert!(
        dj_pcm_readiness_wakeup(&state).is_some(),
        "install the ready short programme before its midpoint"
    );
    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    assert_eq!(
        state
            .next_engine
            .as_ref()
            .unwrap()
            .job
            .prepared_transition
            .as_ref()
            .unwrap()
            .program
            .template,
        "SafeCrossfade"
    );
}

#[test]
fn dj_readiness_respects_transport_pair_and_permanent_failure_guards() {
    let mut state = state_with_unverified_rhythmic_tracks();
    assert!(dj_pcm_readiness_wakeup(&state).is_some());
    state.dj.engine_enabled = false;
    assert!(dj_pcm_readiness_wakeup(&state).is_none());
    state.dj.engine_enabled = true;
    state.dj.lookahead.as_mut().unwrap().queue_generation += 1;
    assert!(dj_pcm_readiness_wakeup(&state).is_none());
    state.dj.lookahead.as_mut().unwrap().queue_generation -= 1;
    let outgoing = state.engine.as_ref().unwrap();
    outgoing
        .shared
        .suppress_crossfade_after_seek
        .store(true, Ordering::Relaxed);
    assert!(dj_pcm_readiness_wakeup(&state).is_none());
    outgoing
        .shared
        .suppress_crossfade_after_seek
        .store(false, Ordering::Relaxed);
    outgoing
        .shared
        .crossfade_start_signaled
        .store(true, Ordering::Relaxed);
    state.user_paused = true;
    assert!(dj_pcm_readiness_wakeup(&state).is_none());
    state.user_paused = false;
    outgoing
        .shared
        .crossfade_start_signaled
        .store(false, Ordering::Relaxed);
    let mut job = state.next_engine.as_ref().unwrap().job.clone();
    job.gapless.overlap_ms = 12_000;
    let transition = job.prepared_transition.as_ref().unwrap();
    record_runtime_renderer_failure(
        &mut state,
        transition,
        DjRuntimeRendererReason::MixerRejected,
    );
    record_runtime_renderer_failure(
        &mut state,
        transition,
        DjRuntimeRendererReason::NextDecodeLateAtFire,
    );
    assert!(
        dj_pcm_readiness_wakeup(&state).is_none(),
        "a fire miss must not erase a permanent render rejection"
    );
    state.next_engine.as_mut().unwrap().generation += 1;
    assert!(
        dj_pcm_readiness_wakeup(&state).is_some(),
        "an old decoder generation cannot block a fresh engine"
    );
    record_runtime_renderer_failure(
        &mut state,
        transition,
        DjRuntimeRendererReason::MixerRejected,
    );
    assert!(dj_pcm_readiness_wakeup(&state).is_none());
    assert!(arm_active_transition_window(&mut state, &job));
    assert!(
        dj_pcm_readiness_wakeup(&state).is_some(),
        "an explicit plan rearm permits a fresh attempt"
    );
}

#[test]
fn partial_long_mix_preparation_promotes_a_short_protected_render() {
    let mut state = state_with_unverified_rhythmic_tracks();
    for engine in [
        state.engine.as_ref().unwrap(),
        state.next_engine.as_ref().unwrap(),
    ] {
        engine
            .shared
            .buffer
            .lock()
            .unwrap()
            .samples
            .truncate(6 * 48_000 * 2);
    }
    // An incoming restart already decoded from source second one. Its
    // planned second-two cue is buffer-local second one, not second two.
    let incoming = state.next_engine.as_mut().unwrap();
    incoming
        .shared
        .position_offset_samples
        .store(48_000 * 2, Ordering::Relaxed);
    incoming
        .shared
        .position_samples
        .store(48_000 * 2, Ordering::Relaxed);
    incoming.shared.publish_source_position();
    incoming
        .job
        .prepared_transition
        .as_mut()
        .unwrap()
        .program
        .deck_b_start_frame = 2 * 48_000;
    assert_eq!(
        prepare_dj_mixer_for_pair(&mut state, 1024),
        Err(DjRuntimeRendererReason::ActiveDeckNotDecoded)
    );
    assert!(state.dj.prepared_mixer.is_none());
    // Although the original twelve-second overlap is unavailable, four
    // seconds can still be rendered safely from the current outgoing cue.
    let active = state.engine.as_ref().unwrap();
    active.shared.buffer.lock().unwrap().read_pos = 48_000 * 2;
    active
        .shared
        .position_samples
        .store(48_000 * 2, Ordering::Relaxed);
    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    promote_next_to_active(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
        "late",
        None,
        None,
        DjRuntimeRendererOutcome::rendered_handoff(),
    );
    let active = state.engine.as_ref().unwrap();
    let executed = &active.job.prepared_transition.as_ref().unwrap().program;
    assert_eq!(executed.template, "SafeCrossfade");
    assert_eq!(executed.resolve_at, 4 * 48_000);
    assert_eq!(executed.deck_b_start_frame, 2 * 48_000);
    assert_eq!(
        active
            .shared
            .handoff_timeline
            .snapshot()
            .unwrap()
            .source_start,
        2 * 48_000
    );
    assert_eq!(active.shared.crossfade_samples.load(Ordering::Relaxed), 0);
    match event_rx.try_recv().unwrap() {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            runtime_rendered_dj_mixer,
            runtime_program_json,
            ..
        } => {
            assert!(runtime_rendered_dj_mixer);
            let executed: noor_mix::TransitionProgram =
                serde_json::from_str(&runtime_program_json.unwrap()).unwrap();
            assert_eq!(executed.template, "SafeCrossfade");
            assert_eq!(executed.resolve_at, 4 * 48_000);
        }
        other => panic!("expected short protected event, got {other:?}"),
    }
}

#[test]
fn protected_recovery_preserves_track_and_queue_pair_guards() {
    let mut state = state_with_unverified_rhythmic_tracks();
    state.dj.lookahead.as_mut().unwrap().queue_generation += 1;
    assert_eq!(
        install_prepared_handoff_mixer_buffer(&mut state),
        Err(DjRuntimeRendererReason::PreparedMixerMissing)
    );
    assert!(state.dj.prepared_mixer.is_none());
    assert_eq!(
        state
            .next_engine
            .as_ref()
            .unwrap()
            .shared
            .crossfade_samples
            .load(Ordering::Relaxed),
        12 * 48_000 * 2
    );
    state.dj.lookahead.as_mut().unwrap().queue_generation -= 1;
    state.dj.lookahead.as_mut().unwrap().next = DjMediaRef::LibraryTrack { track_id: 3 };
    assert_eq!(
        install_prepared_handoff_mixer_buffer(&mut state),
        Err(DjRuntimeRendererReason::PreparedMixerMissing)
    );
    assert!(
        state
            .next_engine
            .as_ref()
            .unwrap()
            .shared
            .handoff_timeline
            .snapshot()
            .is_none()
    );
}

#[test]
fn late_protected_handoff_rebuilds_from_live_audio_after_compaction() {
    let mut state = state_with_late_unverified_rhythmic_mix();
    let active = state.engine.as_ref().unwrap();
    assert!(active.shared.compact_consumed_buffer(4_800 * 2).unwrap() > 0);
    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    let next = state.next_engine.as_ref().unwrap();
    let buffer = next.shared.buffer.lock().unwrap();
    assert_eq!(buffer.read_pos, 0);
    assert!(
        buffer.samples[2_400 * 2] > 0.3,
        "play the live outgoing level, not its earlier render"
    );
    let executed = &next.job.prepared_transition.as_ref().unwrap().program;
    assert_eq!(executed.template, "SafeCrossfade");
    assert_eq!(executed.resolve_at, 4 * 48_000);
    assert!(
        executed
            .decision
            .as_ref()
            .unwrap()
            .reason
            .contains("rebuilt from current outgoing audio")
    );
    assert_eq!(
        next.shared
            .handoff_timeline
            .snapshot()
            .unwrap()
            .output_frames,
        4 * 48_000
    );
    assert_eq!(next.shared.crossfade_samples.load(Ordering::Relaxed), 0);
}

#[test]
fn failed_late_protected_rebuild_uses_and_reports_a_short_cut() {
    let mut state = state_with_late_unverified_rhythmic_mix();
    // A can continue normally but has too little decoded tail for a fresh
    // four-second mix. The original twelve-second overlap stays rejected.
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .buffer
        .lock()
        .unwrap()
        .samples
        .truncate(5 * 48_000 * 2);
    assert_eq!(
        install_prepared_handoff_mixer_buffer(&mut state),
        Err(DjRuntimeRendererReason::ProtectedHandoffCut)
    );
    let next = state.next_engine.as_ref().unwrap();
    assert_eq!(
        next.shared.crossfade_samples.load(Ordering::Relaxed),
        15 * 48 * 2
    );
    let executed = &next.job.prepared_transition.as_ref().unwrap().program;
    assert_eq!(executed.template, "SlamCut");
    assert_eq!(executed.resolve_at, 15 * 48);
    assert_eq!(
        state
            .engine
            .as_ref()
            .unwrap()
            .shared
            .crossfade_samples
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(
        state
            .engine
            .as_ref()
            .unwrap()
            .shared
            .dj_fadeout_start_samples
            .load(Ordering::Relaxed),
        3 * 48_000 * 2
    );
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    promote_next_to_active(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
        "late",
        None,
        None,
        DjRuntimeRendererOutcome::legacy_overlap(DjRuntimeRendererReason::ProtectedHandoffCut),
    );
    match event_rx.try_recv().unwrap() {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            runtime_renderer_reason,
            runtime_program_json,
            ..
        } => {
            assert_eq!(runtime_renderer_reason, "protected_handoff_cut");
            let executed: noor_mix::TransitionProgram =
                serde_json::from_str(&runtime_program_json.unwrap()).unwrap();
            assert_eq!(executed.template, "SlamCut");
            assert_eq!(executed.resolve_at, 15 * 48);
        }
        other => panic!("expected executed cut event, got {other:?}"),
    }
}

#[test]
fn handoff_install_preserves_absolute_origin_when_outgoing_buffer_compacts() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );
    let active = test_engine_with_shared(1, 20);
    let outgoing = (0..256)
        .map(|sample| sample as f32 / 1_000.0)
        .collect::<Vec<_>>();
    finish_engine_buffer(&active, &outgoing);
    {
        let mut buffer = active.shared.buffer.lock().unwrap();
        buffer.read_pos = 20 * 2;
    }
    active
        .shared
        .position_samples
        .store(20 * 2, Ordering::Relaxed);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program = noor_mix::planner::slam_cut_program(48_000, 2, 1);
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    finish_engine_buffer(&next, &vec![0.1; 256]);
    state.engine = Some(active);
    state.next_engine = Some(next);
    prepare_dj_mixer_for_pair(&mut state, 64).unwrap();
    assert_eq!(
        state
            .dj
            .prepared_mixer
            .as_ref()
            .unwrap()
            .program
            .deck_a_start_frame,
        20
    );
    {
        let active = state.engine.as_ref().unwrap();
        active.shared.buffer.lock().unwrap().read_pos = 24 * 2;
        active
            .shared
            .position_samples
            .store(24 * 2, Ordering::Relaxed);
        // Decoder compaction moves the local cursor back to frame four,
        // while the absolute playhead stays at frame twenty-four.
        assert_eq!(
            active.shared.compact_consumed_buffer(4 * 2).unwrap(),
            20 * 2
        );
        assert_eq!(active.shared.buffer.lock().unwrap().read_pos, 4 * 2);
    }
    install_prepared_handoff_mixer_buffer(&mut state).unwrap();
    let next = state.next_engine.as_ref().unwrap();
    assert_eq!(
        next.shared.buffer.lock().unwrap().read_pos,
        4 * 2,
        "join four frames into the existing rendered transition, without replaying deck A"
    );
    assert_eq!(next.shared.position_samples.load(Ordering::Relaxed), 4 * 2);
}

#[test]
fn handoff_install_rejects_join_past_transition_midpoint() {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );

    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    finish_engine_buffer(&next, &[0.0, 0.0, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6]);

    state.engine = Some(active);
    state.next_engine = Some(next);

    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());

    // Playhead ran 2 of the 2 rendered frames past the render start: only
    // the tail of the blend is left, which sounds worse than the plain
    // fallback overlap.
    state
        .engine
        .as_ref()
        .expect("active engine")
        .shared
        .buffer
        .lock()
        .expect("active buffer")
        .read_pos = 4;

    assert_eq!(
        install_prepared_handoff_mixer_buffer(&mut state),
        Err(DjRuntimeRendererReason::HandoffSeamTooLate)
    );
}

#[test]
fn dj_flag_off_does_not_construct_mixer() {
    let mut state = state_with_ready_dj_pair();
    state.dj.engine_enabled = false;

    assert_eq!(
        prepare_dj_mixer_for_pair(&mut state, 64),
        Err(DjRuntimeRendererReason::DjDisabled)
    );
    assert!(state.dj.prepared_mixer.is_none());
}

#[test]
fn dj_flag_off_ignores_transition_program_field() {
    let mut state = test_runtime_loop_state();
    state.dj.engine_enabled = false;
    let mut job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));

    assert!(!gate_prepare_next_for_dj(&mut state, &mut job));
    assert!(job.prepared_transition.is_none());
    assert!(state.dj.prepared_mixer.is_none());
}

#[test]
fn disabling_dj_discards_ready_mixer_without_stopping_playback() {
    let mut state = state_with_ready_dj_pair();
    assert!(prepare_dj_mixer_for_pair(&mut state, 64).is_ok());
    let active = state.engine.as_ref().expect("active engine");
    active
        .shared
        .drop_preview_trigger_samples
        .store(144_000, Ordering::Relaxed);
    active
        .shared
        .drop_preview_start_signaled
        .store(false, Ordering::Relaxed);

    set_dj_engine_enabled_in_state(&mut state, false);

    assert!(!state.dj.engine_enabled);
    assert!(state.dj.prepared_mixer.is_none());
    let active = state.engine.as_ref().expect("active engine");
    let next = state.next_engine.as_ref().expect("next engine");
    assert!(!active.shared.stopped.load(Ordering::SeqCst));
    assert!(!next.shared.stopped.load(Ordering::SeqCst));
    assert!(next.job.prepared_transition.is_none());
    assert_eq!(
        active
            .shared
            .drop_preview_trigger_samples
            .load(Ordering::Relaxed),
        u64::MAX
    );
}

#[test]
fn automatic_crossfade_promotion_emits_timing_event() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_offset_samples
        .store(192_000, Ordering::Relaxed);
    active
        .shared
        .position_samples
        .store(288_000, Ordering::Relaxed);
    let mut next = test_engine_with_shared(2, 21);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(77);
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
    state.engine = Some(active);
    state.next_engine = Some(next);

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));

    promote_next_to_active(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
        "fired",
        None,
        None,
        DjRuntimeRendererOutcome::legacy_overlap(DjRuntimeRendererReason::PreparedMixerMissing),
    );

    match event_rx.try_recv().expect("timing event") {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            transition_event_id,
            outgoing_track_id,
            generation,
            actual_start_ms,
            timing_status,
            runtime_rendered_dj_mixer,
            runtime_renderer_status,
            runtime_renderer_reason,
            ..
        } => {
            assert_eq!(transition_event_id, 77);
            assert_eq!(outgoing_track_id, 1);
            assert_eq!(generation, 20);
            assert_eq!(actual_start_ms, 3_000);
            assert_eq!(timing_status, "fired");
            assert!(!runtime_rendered_dj_mixer);
            assert_eq!(runtime_renderer_status, "legacy_overlap");
            assert_eq!(runtime_renderer_reason, "prepared_mixer_missing");
        }
        other => panic!("expected timing event, got {other:?}"),
    }
    match event_rx.try_recv().expect("finished event") {
        PlaybackRuntimeEvent::Finished {
            track_id,
            generation,
        } => {
            assert_eq!(track_id, 1);
            assert_eq!(generation, 20);
        }
        other => panic!("expected finished event, got {other:?}"),
    }
}

#[test]
fn legacy_overlap_uses_last_prepare_failure_when_mixer_missing() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    let mut next = test_engine_with_shared(2, 21);
    let transition = test_prepared_transition_program(20, Some(11), Some(12));
    next.job =
        PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition.clone());
    state.engine = Some(active);
    state.next_engine = Some(next);
    record_runtime_renderer_failure(
        &mut state,
        &transition,
        DjRuntimeRendererReason::NextDeckNotDecoded,
    );

    assert_eq!(
        runtime_renderer_failure_reason(&state, DjRuntimeRendererReason::PreparedMixerMissing),
        DjRuntimeRendererReason::NextDeckNotDecoded
    );
    assert_eq!(
        runtime_renderer_failure_reason(&state, DjRuntimeRendererReason::BufferLockFailed),
        DjRuntimeRendererReason::BufferLockFailed
    );
}

#[test]
fn legacy_overlap_ignores_prepare_failure_from_different_pair() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    let mut next = test_engine_with_shared(2, 21);
    let stale_transition = test_prepared_transition_program(20, Some(11), Some(12));
    record_runtime_renderer_failure(
        &mut state,
        &stale_transition,
        DjRuntimeRendererReason::NextDeckNotDecoded,
    );

    let fresh_transition = test_prepared_transition_program(21, Some(11), Some(13));
    next.job = PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(fresh_transition);
    state.engine = Some(active);
    state.next_engine = Some(next);

    assert_eq!(
        runtime_renderer_failure_reason(&state, DjRuntimeRendererReason::PreparedMixerMissing),
        DjRuntimeRendererReason::PreparedMixerMissing
    );
}

#[test]
fn runtime_renderer_fire_block_reason_names_fire_miss_cause() {
    let mut state = test_runtime_loop_state();
    assert_eq!(
        runtime_renderer_fire_block_reason(&state, false),
        DjRuntimeRendererReason::NextDeckMissingAtFire
    );

    state.next_engine = Some(test_engine_with_shared(2, 21));
    assert_eq!(
        runtime_renderer_fire_block_reason(&state, false),
        DjRuntimeRendererReason::TransitionPlanMissingAtFire
    );

    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    state.next_engine = Some(next);
    assert_eq!(
        runtime_renderer_fire_block_reason(&state, false),
        DjRuntimeRendererReason::NextDecodeLateAtFire
    );
}

#[test]
fn runtime_renderer_late_fire_reason_preserves_decode_late_cause() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    let mut next = test_engine_with_shared(2, 21);
    let transition = test_prepared_transition_program(20, Some(11), Some(12));
    next.job =
        PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition.clone());
    state.engine = Some(active);
    state.next_engine = Some(next);

    assert_eq!(
        runtime_renderer_late_fire_reason(&state),
        DjRuntimeRendererReason::NextDecodeLateAtFire
    );

    record_runtime_renderer_failure(
        &mut state,
        &transition,
        DjRuntimeRendererReason::NextDecodeLateAtFire,
    );
    assert_eq!(
        runtime_renderer_late_fire_reason(&state),
        DjRuntimeRendererReason::NextDecodeLateAtFire
    );
}

#[test]
fn boundary_fallback_promotion_emits_missed_timing_event() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(480_000, Ordering::Relaxed);
    let mut next = test_engine_with_shared(2, 20);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(78);
    next.job = PreparedPlaybackJob::test_fixture(2, 20).with_prepared_transition(transition);
    state.engine = Some(active);
    state.next_engine = Some(next);

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));

    promote_prepared_at_boundary(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
    );

    match event_rx.try_recv().expect("timing event") {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            transition_event_id,
            outgoing_track_id,
            generation,
            actual_start_ms,
            timing_status,
            runtime_rendered_dj_mixer,
            runtime_renderer_status,
            runtime_renderer_reason,
            ..
        } => {
            assert_eq!(transition_event_id, 78);
            assert_eq!(outgoing_track_id, 1);
            assert_eq!(generation, 20);
            assert_eq!(actual_start_ms, 5_000);
            assert_eq!(timing_status, "missed");
            assert!(!runtime_rendered_dj_mixer);
            assert_eq!(runtime_renderer_status, "boundary_fallback");
            assert_eq!(runtime_renderer_reason, "sync_window_not_signaled");
        }
        other => panic!("expected timing event, got {other:?}"),
    }
    match event_rx.try_recv().expect("finished event") {
        PlaybackRuntimeEvent::Finished {
            track_id,
            generation,
        } => {
            assert_eq!(track_id, 1);
            assert_eq!(generation, 20);
        }
        other => panic!("expected finished event, got {other:?}"),
    }
}

#[test]
fn boundary_fallback_after_manual_seek_reports_seek_suppression() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    active
        .shared
        .position_samples
        .store(480_000, Ordering::Relaxed);
    active
        .shared
        .suppress_crossfade_after_seek
        .store(true, Ordering::Relaxed);
    let mut next = test_engine_with_shared(2, 20);
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(79);
    next.job = PreparedPlaybackJob::test_fixture(2, 20).with_prepared_transition(transition);
    state.engine = Some(active);
    state.next_engine = Some(next);

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let position_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let buffered_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    let offset_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));

    promote_prepared_at_boundary(
        &mut state,
        &event_tx,
        &position_source,
        &buffered_source,
        &offset_source,
    );

    match event_rx.try_recv().expect("timing event") {
        PlaybackRuntimeEvent::DjTransitionPromoted {
            runtime_renderer_status,
            runtime_renderer_reason,
            ..
        } => {
            assert_eq!(runtime_renderer_status, "boundary_fallback");
            assert_eq!(runtime_renderer_reason, "manual_seek_suppressed");
        }
        other => panic!("expected timing event, got {other:?}"),
    }
}

#[test]
fn effective_output_config_applies_desired_sample_rate() {
    let base = StreamConfig {
        channels: 2,
        sample_rate: 48_000,
        buffer_size: cpal::BufferSize::Default,
    };

    let effective = effective_output_config(&base, Some(96_000));

    assert_eq!(effective.sample_rate, 96_000);
    assert_eq!(effective.channels, 2);
}

#[test]
fn effective_output_config_keeps_base_rate_without_override() {
    let base = StreamConfig {
        channels: 6,
        sample_rate: 44_100,
        buffer_size: cpal::BufferSize::Default,
    };

    let effective = effective_output_config(&base, None);

    assert_eq!(effective.sample_rate, 44_100);
    assert_eq!(effective.channels, 6);
}

#[test]
fn exclusive_rebuild_rate_follows_current_output_rate_only_when_enabled() {
    assert_eq!(exclusive_rebuild_rate(true, 96_000), Some(96_000));
    assert_eq!(exclusive_rebuild_rate(false, 96_000), None);
}

#[test]
fn device_swap_preserves_active_rate_without_explicit_target() {
    assert_eq!(
        device_swap_target_sample_rate(None, true, true, 44_100, 48_000),
        Some(44_100)
    );
    assert_eq!(
        device_swap_target_sample_rate(None, false, true, 96_000, 48_000),
        Some(96_000)
    );
}

#[test]
fn device_swap_uses_default_follow_rate_when_idle() {
    assert_eq!(
        device_swap_target_sample_rate(None, true, false, 44_100, 48_000),
        Some(48_000)
    );
    assert_eq!(
        device_swap_target_sample_rate(None, false, false, 44_100, 48_000),
        None
    );
}

#[test]
fn device_swap_explicit_target_overrides_active_rate() {
    assert_eq!(
        device_swap_target_sample_rate(Some(192_000), true, true, 44_100, 48_000),
        Some(192_000)
    );
}

#[test]
fn transition_output_sample_rate_uses_job_rate_only_when_following() {
    assert_eq!(
        transition_output_sample_rate(Some(96_000), true, 44_100),
        Some(96_000)
    );
    assert_eq!(
        transition_output_sample_rate(Some(96_000), false, 44_100),
        None
    );
    assert_eq!(
        transition_output_sample_rate(Some(44_100), true, 44_100),
        None
    );
    assert_eq!(transition_output_sample_rate(None, true, 44_100), None);
}

#[test]
fn sample_rate_follow_transition_rebuilds_output_state() {
    assert_eq!(
        transition_output_state_update(Some(96_000), true, 44_100),
        Some(OutputStateUpdate {
            sample_rate: 96_000,
            force_exclusive_rebuild: true,
            notify_ready: true,
        })
    );
    assert_eq!(
        transition_output_state_update(Some(44_100), true, 44_100),
        None
    );
}

#[test]
fn prepared_engine_rate_must_match_when_sample_rate_following() {
    assert!(prepared_engine_matches_output_rate(
        96_000,
        Some(96_000),
        true
    ));
    assert!(!prepared_engine_matches_output_rate(
        44_100,
        Some(96_000),
        true
    ));
    assert!(prepared_engine_matches_output_rate(
        44_100,
        Some(96_000),
        false
    ));
    assert!(prepared_engine_matches_output_rate(44_100, None, true));
}

#[test]
fn swap_stream_plan_uses_track_rate_for_exclusive_backend() {
    let base = StreamConfig {
        channels: 2,
        sample_rate: 48_000,
        buffer_size: cpal::BufferSize::Default,
    };

    let plan = swap_stream_plan(&base, Some(96_000), SwapBackend::Exclusive);

    assert_eq!(plan.stream_config.sample_rate, 96_000);
    assert_eq!(plan.target_sample_rate, Some(96_000));
}

#[test]
fn output_rate_fallback_uses_base_when_desired_rate_was_rejected() {
    let base = StreamConfig {
        channels: 2,
        sample_rate: 192_000,
        buffer_size: cpal::BufferSize::Default,
    };
    let attempted = StreamConfig {
        channels: 2,
        sample_rate: 176_400,
        buffer_size: cpal::BufferSize::Default,
    };

    let fallback = output_rate_fallback_config(&attempted, &base).expect("fallback");

    assert_eq!(fallback.sample_rate, 192_000);
}

#[test]
fn output_rate_fallback_is_none_when_attempt_already_uses_base_rate() {
    let base = StreamConfig {
        channels: 2,
        sample_rate: 192_000,
        buffer_size: cpal::BufferSize::Default,
    };

    assert!(output_rate_fallback_config(&base, &base).is_none());
}

#[test]
fn swap_pause_guard_restores_previous_pause_state_on_drop() {
    let (command_tx, _command_rx) = mpsc::channel();
    let shared = Arc::new(PlaybackSharedState::new(
        42,
        0,
        PlaybackSourceKind::TidalStream,
        GaplessPlan::disabled(),
        48_000,
        2,
        None,
        command_tx,
        Arc::new(AtomicU32::new(1.0f32.to_bits())),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    ));

    {
        let _guard = SwapPauseGuard::new(Arc::clone(&shared));
        assert!(shared.paused.load(Ordering::SeqCst));
    }

    assert!(!shared.paused.load(Ordering::SeqCst));
}

#[test]
fn write_output_f32_drains_ready_buffer_at_96khz() {
    let (command_tx, _command_rx) = mpsc::channel();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let position = Arc::new(AtomicU64::new(0));
    let shared = Arc::new(PlaybackSharedState::new(
        42,
        0,
        PlaybackSourceKind::TidalStream,
        GaplessPlan::disabled(),
        96_000,
        2,
        None,
        command_tx.clone(),
        Arc::new(AtomicU32::new(1.0f32.to_bits())),
        Arc::clone(&position),
        Arc::new(AtomicU64::new(0)),
    ));
    {
        let mut buffer = shared.buffer.lock().unwrap();
        buffer.samples.extend_from_slice(&[0.25, -0.25, 0.5, -0.5]);
        buffer.mark_finished();
    }

    let mut out = vec![0.0_f32; 4];
    write_output_f32(&mut out, &shared, &command_tx, &event_tx);

    assert_eq!(out, vec![0.25, -0.25, 0.5, -0.5]);
    assert_eq!(position.load(Ordering::Relaxed), 4);
}

#[test]
fn write_output_f32_outputs_silence_when_paused_at_96khz() {
    let (command_tx, _command_rx) = mpsc::channel();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let position = Arc::new(AtomicU64::new(0));
    let shared = Arc::new(PlaybackSharedState::new(
        42,
        0,
        PlaybackSourceKind::TidalStream,
        GaplessPlan::disabled(),
        96_000,
        2,
        None,
        command_tx.clone(),
        Arc::new(AtomicU32::new(1.0f32.to_bits())),
        Arc::clone(&position),
        Arc::new(AtomicU64::new(0)),
    ));
    shared.paused.store(true, Ordering::SeqCst);
    {
        let mut buffer = shared.buffer.lock().unwrap();
        buffer.samples.extend_from_slice(&[0.25, -0.25, 0.5, -0.5]);
        buffer.mark_finished();
    }

    let mut out = vec![1.0_f32; 4];
    write_output_f32(&mut out, &shared, &command_tx, &event_tx);

    assert_eq!(out, vec![0.0, 0.0, 0.0, 0.0]);
    assert_eq!(position.load(Ordering::Relaxed), 0);
}

#[test]
fn prebuffer_samples_expand_with_gapless_padding() {
    let samples = samples_from_ms(1_500, 48_000, 2);
    assert!(samples > 0);
}

#[test]
fn finished_buffer_becomes_ready_without_threshold() {
    let mut buffer = PlaybackBuffer::new(48_000);
    assert!(!buffer.is_ready());
    buffer.mark_finished();
    assert!(buffer.is_ready());
}

#[test]
fn terminal_engine_slot_identifies_prebuffered_track() {
    assert_eq!(
        terminal_engine_slot(Some((1, 1)), Some((2, 1)), None, None, 2, 1),
        Some(TerminalEngineSlot::Next)
    );
    assert_eq!(
        terminal_engine_slot(Some((1, 1)), Some((2, 1)), Some((3, 1)), None, 3, 1),
        Some(TerminalEngineSlot::FadingOut)
    );
    assert_eq!(
        terminal_engine_slot(Some((1, 1)), Some((2, 1)), Some((3, 1)), Some((4, 1)), 4, 1),
        Some(TerminalEngineSlot::DropPreview)
    );
    assert_eq!(
        terminal_engine_slot(Some((1, 1)), Some((2, 1)), Some((3, 1)), None, 4, 1),
        None
    );
}

#[test]
fn active_finish_promotes_prepared_track_at_boundary() {
    assert!(should_promote_prepared_at_boundary(
        Some((1, 7)),
        Some((2, 7)),
        1,
        7,
        &PlaybackTerminalReason::Finished
    ));
    assert!(!should_promote_prepared_at_boundary(
        Some((1, 7)),
        None,
        1,
        7,
        &PlaybackTerminalReason::Finished
    ));
    assert!(!should_promote_prepared_at_boundary(
        Some((1, 7)),
        Some((2, 7)),
        1,
        7,
        &PlaybackTerminalReason::Error("decode failed".to_string())
    ));
}

#[test]
fn switch_noop_requires_same_track_and_generation() {
    assert!(switch_is_noop_for_active_job(false, Some((42, 7)), 42, 7));
    assert!(!switch_is_noop_for_active_job(false, Some((42, 7)), 42, 8));
    assert!(!switch_is_noop_for_active_job(true, Some((42, 7)), 42, 7));
}

#[test]
fn estimates_total_samples_from_track_duration() {
    assert_eq!(
        estimate_total_samples_from_duration_ms(180_000, 48_000, 2),
        Some(17_280_000)
    );
    assert_eq!(estimate_total_samples_from_duration_ms(0, 48_000, 2), None);
}

#[test]
fn shared_state_emits_finished_terminal_command() {
    let (command_tx, command_rx) = mpsc::channel();
    let shared = PlaybackSharedState::new(
        42,
        0,
        PlaybackSourceKind::TidalStream,
        GaplessPlan::disabled(),
        48_000,
        2,
        None,
        command_tx,
        Arc::new(AtomicU32::new(1.0f32.to_bits())),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    );

    shared
        .signal_terminal(PlaybackTerminalReason::Finished)
        .expect("terminal signal should be sent");

    match command_rx
        .try_recv()
        .expect("terminal command should be queued")
    {
        PlaybackRuntimeCommand::TrackTerminal {
            track_id,
            generation,
            outcome,
        } => {
            assert_eq!(track_id, 42);
            assert_eq!(generation, 0);
            assert!(matches!(outcome, PlaybackTerminalReason::Finished));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn shared_state_emits_error_terminal_command() {
    let (command_tx, command_rx) = mpsc::channel();
    let shared = PlaybackSharedState::new(
        7,
        0,
        PlaybackSourceKind::TidalStream,
        GaplessPlan::disabled(),
        48_000,
        2,
        None,
        command_tx,
        Arc::new(AtomicU32::new(1.0f32.to_bits())),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    );

    shared
        .signal_terminal(PlaybackTerminalReason::Error("boom".to_string()))
        .expect("terminal signal should be sent");

    match command_rx
        .try_recv()
        .expect("terminal command should be queued")
    {
        PlaybackRuntimeCommand::TrackTerminal {
            track_id,
            generation,
            outcome,
        } => {
            assert_eq!(track_id, 7);
            assert_eq!(generation, 0);
            match outcome {
                PlaybackTerminalReason::Error(message) => assert_eq!(message, "boom"),
                PlaybackTerminalReason::Finished => panic!("expected error reason"),
            }
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[cfg(target_os = "windows")]
#[test]
fn exclusive_render_sources_include_active_prepared_and_fading() {
    let active = test_engine_with_shared(10, 1);
    let prepared = test_engine_with_shared(11, 1);
    let fading = test_engine_with_shared(9, 1);
    let drop_preview = test_engine_with_shared(12, 1);

    let sources = exclusive_render_sources(
        Some(&active),
        Some(&prepared),
        Some(&fading),
        Some(&drop_preview),
    );

    assert_eq!(sources.len(), 4);
    assert_eq!(sources[0].role, ExclusiveRenderRole::Active);
    assert_eq!(sources[1].role, ExclusiveRenderRole::Prepared);
    assert_eq!(sources[2].role, ExclusiveRenderRole::Fading);
    assert_eq!(sources[3].role, ExclusiveRenderRole::Prepared);
}

#[test]
fn buffered_source_redirect_makes_handle_read_from_new_engine() {
    // Regression for the codex P1 finding: a `buffered_ms()` accessor
    // tied to the initial engine's atomic would silently read stale data
    // after a Switch or crossfade promotion. The handle must follow the
    // same redirect pattern as `position_source` - this test pins that.

    let (command_tx, _) = std::sync::mpsc::channel();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);

    let engine_a_buffered = Arc::new(AtomicU64::new(48_000)); // 1000 ms @ 48k mono
    let engine_b_buffered = Arc::new(AtomicU64::new(96_000)); // 1000 ms @ 48k stereo

    let buffered_source: Arc<Mutex<Arc<AtomicU64>>> =
        Arc::new(Mutex::new(Arc::clone(&engine_a_buffered)));

    let handle = PlaybackRuntimeHandle {
        command_tx,
        event_tx,
        healthy: Arc::new(AtomicBool::new(true)),
        volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
        buffered_source: Arc::clone(&buffered_source),
        offset_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
        handoff_elapsed_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(u64::MAX)))),
    };

    assert_eq!(
        handle.buffered_samples(),
        48_000,
        "fresh handle must read from engine A's counter"
    );
    assert_eq!(handle.get_buffered_ms(48_000, 1), 1000);

    // Simulate transition_to_job / promote_*: redirect the source to
    // engine B's counter, exactly the way the runtime loop does it.
    *buffered_source.lock().unwrap() = Arc::clone(&engine_b_buffered);

    assert_eq!(
        handle.buffered_samples(),
        96_000,
        "after redirect the handle MUST read engine B, not stale A"
    );
    assert_eq!(handle.get_buffered_ms(48_000, 2), 1000);

    // After redirect, mutating engine A's counter must NOT leak through.
    engine_a_buffered.store(999_999, Ordering::Relaxed);
    assert_eq!(
        handle.buffered_samples(),
        96_000,
        "stale engine writes must not affect the redirected handle"
    );
}

#[test]
fn get_buffered_ms_returns_zero_for_invalid_device_config() {
    let (command_tx, _) = std::sync::mpsc::channel();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let handle = PlaybackRuntimeHandle {
        command_tx,
        event_tx,
        healthy: Arc::new(AtomicBool::new(true)),
        volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
        buffered_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(48_000)))),
        offset_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
        handoff_elapsed_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(u64::MAX)))),
    };
    assert_eq!(handle.get_buffered_ms(0, 2), 0);
    assert_eq!(handle.get_buffered_ms(48_000, 0), 0);
}

#[test]
fn report_runtime_command_error_emits_error_event() {
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);

    report_runtime_command_error(
        &event_tx,
        "Play",
        anyhow::anyhow!("output device rejected stream"),
    );

    match event_rx.try_recv().expect("error event should be emitted") {
        PlaybackRuntimeEvent::Error { message } => {
            assert!(message.contains("Play failed"));
            assert!(message.contains("output device rejected stream"));
        }
        other => panic!("expected error event, got {other:?}"),
    }
}

#[test]
fn manual_seek_near_end_suppresses_crossfade_promotion() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    let total_samples = 120 * 48_000 * 2;
    active
        .shared
        .total_samples
        .store(total_samples, Ordering::Relaxed);

    active
        .shared
        .set_manual_seek_crossfade_suppression(total_samples - 48_000);
    state.engine = Some(active);

    assert!(active_engine_suppresses_crossfade_after_seek(&state));
}

#[test]
fn manual_seek_past_crossfade_window_suppresses_crossfade_promotion() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    let total_samples = 120 * 48_000 * 2;
    let crossfade_samples = 30 * 48_000 * 2;
    active
        .shared
        .total_samples
        .store(total_samples, Ordering::Relaxed);
    active
        .shared
        .crossfade_samples
        .store(crossfade_samples, Ordering::Relaxed);

    active
        .shared
        .set_manual_seek_crossfade_suppression(total_samples - crossfade_samples);
    state.engine = Some(active);

    assert!(active_engine_suppresses_crossfade_after_seek(&state));
}

#[test]
fn manual_seek_before_near_end_keeps_crossfade_promotion_enabled() {
    let mut state = test_runtime_loop_state();
    let active = test_engine_with_shared(1, 20);
    let total_samples = 120 * 48_000 * 2;
    active
        .shared
        .total_samples
        .store(total_samples, Ordering::Relaxed);

    active.shared.set_manual_seek_crossfade_suppression(0);
    state.engine = Some(active);

    assert!(!active_engine_suppresses_crossfade_after_seek(&state));
}

// DIAGNOSE repro (crossfade stall): the CrossfadeStart handler promotes the
// incoming deck the moment `is_ready()` is true, i.e. once it has buffered
// its ~500ms prebuffer threshold. That gate never looks at the crossfade
// length, so with an 8s fade the deck is promoted holding well under 1s of
// audio while it owes 8s. On a slow TIDAL connection it starves a couple
// seconds into the new track and playback freezes (there is no stall
// watchdog). With crossfade OFF this promotion path never runs, which is
// why the same tracks don't stall with the fade disabled.
#[test]
fn crossfade_promotion_gate_accepts_next_deck_that_cannot_cover_the_fade() {
    let sample_rate = 48_000u32;
    let channels = 2u16;
    let crossfade_ms = 8_000i32;

    let plan = GaplessPlan {
        enabled: true,
        overlap_ms: crossfade_ms,
        prebuffer_ms: 500,
        requires_stream_metadata: true,
    };
    let (command_tx, _) = mpsc::channel();
    let next_shared = Arc::new(PlaybackSharedState::new(
        2,
        1,
        PlaybackSourceKind::TidalStream,
        plan,
        sample_rate,
        channels,
        None,
        command_tx,
        Arc::new(AtomicU32::new(1.0f32.to_bits())),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    ));

    // Buffer the deck to exactly its readiness threshold: this is the moment
    // is_ready() first flips true and the CrossfadeStart gate would promote.
    let (unread, ready) = {
        let mut buf = next_shared.buffer.lock().unwrap();
        let threshold = buf.start_threshold_samples;
        buf.samples = vec![0.1f32; threshold];
        (buf.samples.len() - buf.read_pos, buf.is_ready())
    };

    let crossfade_samples =
        (crossfade_ms as usize) * sample_rate as usize * channels as usize / 1_000;

    assert!(
        ready,
        "is_ready() (the gate the CrossfadeStart handler uses) is satisfied at the prebuffer threshold"
    );
    assert!(
        unread < crossfade_samples,
        "deck promoted with {unread} samples buffered but owes a {crossfade_samples}-sample fade \
         ({:.0}% of the window) -> it starves mid-fade on a slow connection",
        unread as f32 / crossfade_samples as f32 * 100.0
    );
}

// Regression for the fix: the crossfade promotion gate must wait until the
// incoming deck has buffered the whole fade window (plus margin), not just
// the ~500ms start threshold. A deck that only passes is_ready() must be
// deferred so it can't be promoted into a fade it will starve through.
#[test]
fn crossfade_next_ready_requires_the_full_fade_window() {
    let crossfade = 8 * 48_000u64 * 2; // 8s @ 48k stereo
    let threshold = 750 * 48_000u64 * 2 / 1_000; // ~500ms prebuffer + pad

    // is_ready() true but only the prebuffer threshold buffered -> defer.
    assert!(
        !crossfade_next_ready(true, false, threshold, crossfade),
        "a deck at only the prebuffer threshold must NOT be promoted into an 8s fade"
    );
    // Buffered past the fade window plus margin -> promote.
    assert!(
        crossfade_next_ready(true, false, crossfade + crossfade / 8, crossfade),
        "a deck holding the whole fade window (plus margin) is safe to promote"
    );
    // Fully decoded short track -> always safe, even if tiny.
    assert!(
        crossfade_next_ready(true, true, 1_000, crossfade),
        "a finished deck never starves and is always promotable"
    );
    // Not even past the base prebuffer threshold -> never.
    assert!(
        !crossfade_next_ready(false, false, crossfade * 2, crossfade),
        "a deck that has not reached the base start threshold is never ready"
    );
}

fn native_adaptive_readiness_case() -> (PlaybackRuntimeLoopState, CrossfadeReadinessSnapshot, u64) {
    let mut state = state_with_unverified_rhythmic_tracks();
    let transition = state
        .next_engine
        .as_mut()
        .unwrap()
        .job
        .prepared_transition
        .as_mut()
        .unwrap();
    let decision = transition.program.decision.clone();
    transition.program = noor_mix::planner::bass_swap_32_program(48_000, 2, 23_226);
    // Keep the full-window readiness regression independent of optional
    // BassSwap32 musical prefix recovery, which is exercised with PCM.
    transition.program.template = "BassSwap16".into();
    transition.program.resolve_at = 1_114_854; // Native programme: 23.226125s.
    transition.program.deck_b_start_frame = 36_818; // Native source cue: 0.767033s.
    transition.program.decision = decision;
    transition.program.decision.as_mut().unwrap().duration_beats = 48.0;
    let unread = (26.097739583333333_f64 * 96_000.0).floor() as u64;
    let buffer = CrossfadeReadinessSnapshot {
        base_ready: true,
        finished: false,
        unread_samples: unread,
        decoded_samples: unread,
        read_samples: 0,
        offset_samples: 0,
        start_threshold_samples: 72_000,
    };
    (state, buffer, 23_226 * 96)
}

#[test]
fn adaptive_readiness_accepts_the_native_pcm_window_without_the_outgoing_margin() {
    let (state, buffer, outgoing_window) = native_adaptive_readiness_case();
    assert!(!crossfade_next_ready(
        true,
        false,
        buffer.unread_samples,
        outgoing_window
    ));
    let required_seconds =
        adaptive_next_required_samples(&state, buffer).unwrap() as f64 / 96_000.0;
    assert!((25.70..25.71).contains(&required_seconds));
    assert!(dj_crossfade_next_ready(&state, buffer, outgoing_window));
    assert!(!dj_crossfade_next_ready(
        &state,
        CrossfadeReadinessSnapshot {
            base_ready: false,
            ..buffer
        },
        outgoing_window
    ));
}

#[test]
fn adaptive_readiness_includes_incoming_cue_corrected_rate_and_buffer_offset() {
    let (mut state, mut buffer, outgoing_window) = native_adaptive_readiness_case();
    let transition = state
        .next_engine
        .as_mut()
        .unwrap()
        .job
        .prepared_transition
        .as_mut()
        .unwrap();
    transition.program.resolve_at = 1_097_134; // Confronted: 22.856958s.
    transition.program.deck_b_start_frame = 127_251; // 2.651063s source cue.
    buffer.unread_samples = 25_900 * 96;
    assert!(!dj_crossfade_next_ready(&state, buffer, outgoing_window));
    let original_requirement = adaptive_next_required_samples(&state, buffer).unwrap();
    buffer.offset_samples = 96_000;
    buffer.read_samples = 24_000;
    assert_eq!(
        adaptive_next_required_samples(&state, buffer).unwrap(),
        original_requirement - 120_000
    );
    buffer.offset_samples = 3 * 96_000; // Cue is no longer decoded.
    assert!(!dj_crossfade_next_ready(
        &state,
        CrossfadeReadinessSnapshot {
            finished: true,
            ..buffer
        },
        outgoing_window
    ));
}

#[test]
fn adaptive_readiness_preserves_legacy_disabled_stale_and_rate_mismatch_gates() {
    let (mut state, buffer, outgoing_window) = native_adaptive_readiness_case();
    state
        .next_engine
        .as_ref()
        .unwrap()
        .shared
        .target_sample_rate
        .store(44_100, Ordering::Relaxed);
    assert!(!dj_crossfade_next_ready(&state, buffer, outgoing_window));
    state
        .next_engine
        .as_ref()
        .unwrap()
        .shared
        .target_sample_rate
        .store(48_000, Ordering::Relaxed);
    state.dj.lookahead.as_mut().unwrap().queue_generation += 1;
    assert!(!dj_crossfade_next_ready(&state, buffer, outgoing_window));
    state.dj.lookahead.as_mut().unwrap().queue_generation -= 1;
    state.dj.engine_enabled = false;
    assert!(!dj_crossfade_next_ready(&state, buffer, outgoing_window));
    state.dj.engine_enabled = true;
    state
        .next_engine
        .as_mut()
        .unwrap()
        .job
        .prepared_transition
        .as_mut()
        .unwrap()
        .program
        .decision = None;
    assert!(!dj_crossfade_next_ready(&state, buffer, outgoing_window));
    assert!(dj_crossfade_next_ready(
        &state,
        CrossfadeReadinessSnapshot {
            finished: true,
            ..buffer
        },
        outgoing_window
    ));
}

#[test]
fn stall_tracker_flags_starved_active_engine_after_threshold() {
    let mut state = test_runtime_loop_state();
    let engine = test_engine_with_shared(7, 3);
    engine
        .shared
        .position_samples
        .store(48_000, Ordering::Relaxed);
    engine.shared.buffer.lock().unwrap().started = true; // it WAS playing
    state.engine = Some(engine);

    let mut tracker = StallTracker::new();
    // First poll arms the tracker against the active engine; not a stall yet.
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());

    // Simulate the stall clock having run past the budget with the playhead
    // frozen at the same position (decoder starved on a hung segment).
    tracker.last_progress_at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(
            ACTIVE_STALL_RECOVERY_SECS + 1,
        ))
        .expect("instant underflow");
    let stalled = tracker.poll(&state);
    assert_eq!(
        stalled.force_advance,
        Some((7, 3)),
        "a frozen, unfinished, playing engine past the budget must force advance"
    );
    assert_eq!(
        stalled.just_stalled,
        Some(7),
        "the first budget crossing starts a stall episode for the listener"
    );
}

#[test]
fn stall_tracker_emits_stalled_once_and_recovered_on_progress() {
    let mut state = test_runtime_loop_state();
    let engine = test_engine_with_shared(7, 3);
    engine
        .shared
        .position_samples
        .store(48_000, Ordering::Relaxed);
    engine.shared.buffer.lock().unwrap().started = true;
    state.engine = Some(engine);

    let mut tracker = StallTracker::new();
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());

    let stale = || {
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(
                ACTIVE_STALL_RECOVERY_SECS + 1,
            ))
            .expect("instant underflow")
    };

    // Budget elapses frozen: one Stalled emission plus the force advance.
    tracker.last_progress_at = stale();
    let stalled = tracker.poll(&state);
    assert_eq!(stalled.just_stalled, Some(7));
    assert_eq!(stalled.force_advance, Some((7, 3)));

    // Still frozen a budget later: the advance re-fires (retry), the
    // Stalled emission does not repeat (one pause per episode).
    tracker.last_progress_at = stale();
    let still = tracker.poll(&state);
    assert!(still.just_stalled.is_none());
    assert_eq!(still.force_advance, Some((7, 3)));

    // The hung segment finally arrives: progress on the same engine emits
    // StallRecovered exactly once, then everything is quiet again.
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    let recovered = tracker.poll(&state);
    assert_eq!(recovered.just_recovered, Some(7));
    assert!(recovered.force_advance.is_none());
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());
}

#[test]
fn stall_tracker_does_not_skip_a_track_still_doing_initial_buffering() {
    // A fresh deck on a slow connection has not crossed its prebuffer
    // threshold yet (started == false, playhead at the baseline). That is
    // buffering, not a stall -- force-skipping it would drop the track
    // before it ever plays a sample (regression caught by the fix grill).
    let mut state = test_runtime_loop_state();
    let engine = test_engine_with_shared(7, 3);
    // started stays false: no samples buffered past the start threshold.
    state.engine = Some(engine);

    let mut tracker = StallTracker::new();
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());
    tracker.last_progress_at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(
            ACTIVE_STALL_RECOVERY_SECS + 1,
        ))
        .expect("instant underflow");
    assert_eq!(
        tracker.poll(&state),
        StallPollOutcome::default(),
        "a deck still doing initial buffering must not be force-skipped"
    );
}

#[test]
fn stall_tracker_ignores_progress_and_paused_engines() {
    let mut state = test_runtime_loop_state();
    let engine = test_engine_with_shared(7, 3);
    engine
        .shared
        .position_samples
        .store(48_000, Ordering::Relaxed);
    engine.shared.buffer.lock().unwrap().started = true;
    state.engine = Some(engine);

    let mut tracker = StallTracker::new();
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());

    let stale = || {
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(
                ACTIVE_STALL_RECOVERY_SECS + 1,
            ))
            .expect("instant underflow")
    };

    // Audible progress since the last tick resets the stall timer. No
    // stall episode was flagged, so no StallRecovered fires either.
    tracker.last_progress_at = stale();
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    assert_eq!(
        tracker.poll(&state),
        StallPollOutcome::default(),
        "progress is not a stall"
    );

    // Paused playback legitimately makes no progress.
    tracker.last_progress_at = stale();
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .paused
        .store(true, Ordering::SeqCst);
    assert_eq!(
        tracker.poll(&state),
        StallPollOutcome::default(),
        "paused is not a stall"
    );
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .paused
        .store(false, Ordering::SeqCst);
}

/// A finished engine that still has buffered audio left is mid-playout,
/// not stalled: the callback is draining it and the position moves.
#[test]
fn stall_tracker_ignores_finished_engine_still_draining() {
    let mut state = test_runtime_loop_state();
    let engine = test_engine_with_shared(7, 3);
    {
        let mut guard = engine.shared.buffer.lock().unwrap();
        guard.started = true;
        guard.samples = vec![0.0; 4_800];
        guard.read_pos = 0;
        guard.mark_finished();
    }
    engine
        .shared
        .position_samples
        .store(48_000, Ordering::Relaxed);
    state.engine = Some(engine);

    let mut tracker = StallTracker::new();
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());

    // Playout advanced the position: still healthy even though finished.
    tracker.last_progress_at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(
            ACTIVE_STALL_RECOVERY_SECS + 1,
        ))
        .expect("instant underflow");
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .position_samples
        .store(96_000, Ordering::Relaxed);
    assert_eq!(
        tracker.poll(&state),
        StallPollOutcome::default(),
        "a finished engine still playing out its buffer is not stalled"
    );
}

/// The end-of-track regression this watchdog change exists for: decode
/// finished, the buffer fully drained, the position is pinned at the end
/// and nothing is advancing. That is the audio callback's one-shot
/// terminal having been lost, and the watchdog is the only thing left that
/// can recover it.
#[test]
fn stall_tracker_force_advances_finished_drained_engine() {
    let mut state = test_runtime_loop_state();
    let engine = test_engine_with_shared(7, 3);
    {
        let mut guard = engine.shared.buffer.lock().unwrap();
        guard.started = true;
        guard.samples = vec![0.0; 4_800];
        guard.read_pos = 4_800; // fully drained
        guard.mark_finished();
        guard.finished_notified = true; // terminal already consumed and lost
    }
    engine
        .shared
        .position_samples
        .store(48_000, Ordering::Relaxed);
    state.engine = Some(engine);

    let mut tracker = StallTracker::new();
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());

    tracker.last_progress_at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(
            ACTIVE_STALL_RECOVERY_SECS + 1,
        ))
        .expect("instant underflow");
    let outcome = tracker.poll(&state);
    assert_eq!(
        outcome.force_advance,
        Some((7, 3)),
        "a drained finished engine making no progress must force the queue forward"
    );
    assert_eq!(outcome.kind, Some(StallKind::LostTerminal));
}

/// A paused engine at the end of its buffer is the user having pressed
/// pause on the last moments of a track. Never force-advance that.
#[test]
fn stall_tracker_ignores_paused_finished_drained_engine() {
    let mut state = test_runtime_loop_state();
    let engine = test_engine_with_shared(7, 3);
    {
        let mut guard = engine.shared.buffer.lock().unwrap();
        guard.started = true;
        guard.read_pos = 0;
        guard.mark_finished();
    }
    engine.shared.paused.store(true, Ordering::SeqCst);
    state.engine = Some(engine);

    let mut tracker = StallTracker::new();
    assert_eq!(tracker.poll(&state), StallPollOutcome::default());
    tracker.last_progress_at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(
            ACTIVE_STALL_RECOVERY_SECS + 1,
        ))
        .expect("instant underflow");
    assert_eq!(
        tracker.poll(&state),
        StallPollOutcome::default(),
        "paused wins over drained"
    );
}

fn test_runtime_loop_state() -> PlaybackRuntimeLoopState {
    PlaybackRuntimeLoopState {
        handoff_elapsed_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(u64::MAX)))),
        device_name: "test".to_string(),
        device_sample_rate: 48_000,
        device_channels: 2,
        #[cfg(target_os = "windows")]
        exclusive_sink: ExclusiveRuntimeSink::new(),
        engine: None,
        next_engine: None,
        drop_preview_engine: None,
        fading_out_engine: None,
        current_exclusive: false,
        current_sample_rate_follow: false,
        current_device_selection: OutputDeviceSelection::Default,
        current_exclusive_release_grace_secs:
            crate::db::audio_settings::DEFAULT_EXCLUSIVE_RELEASE_GRACE_SECS,
        current_exclusive_latency_mode: ExclusiveLatencyMode::Stable,
        dj: DjTransitionState::new(true),
        user_paused: false,
        silent_start_streak: 0,
    }
}

#[test]
fn emit_prepared_track_failure_sends_prepared_error_event() {
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);

    let mut job = PreparedPlaybackJob::test_fixture(42, 7);
    // The request is the actual decoded source, even if other metadata is
    // older or has since been healed to a different catalog id.
    job.track.tidal_id = Some(111);
    job.source = crate::playback::player::PlaybackSourceRequest::TidalStream(StreamRequest::new(
        222, "LOSSLESS",
    ));
    emit_prepared_track_failure(&event_tx, &job, "decode failed: malformed packet");

    match event_rx.try_recv().expect("error event should be emitted") {
        PlaybackRuntimeEvent::PreparedTrackError {
            track_id,
            generation,
            tidal_id,
            message,
        } => {
            assert_eq!(track_id, 42);
            assert_eq!(generation, 7);
            assert_eq!(tidal_id, Some(222));
            assert!(message.contains("Pre-buffered track 42 failed"));
            assert!(message.contains("decode failed: malformed packet"));
        }
        other => panic!("expected prepared track error event, got {other:?}"),
    }
}

#[test]
fn handle_panic_in_runtime_loop_clears_state_and_emits_error() {
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let mut state = test_runtime_loop_state();
    state.engine = Some(test_engine_with_shared(1, 1));
    state.next_engine = Some(test_engine_with_shared(2, 1));

    let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("synthetic dispatch panic"));
    let outcome = handle_panic_in_runtime_loop(payload, &event_tx, &mut state);

    assert!(matches!(outcome, std::ops::ControlFlow::Continue(())));
    assert!(state.engine.is_none());
    assert!(state.next_engine.is_none());

    match event_rx.try_recv().expect("error event should be emitted") {
        PlaybackRuntimeEvent::Error { message } => {
            assert!(message.contains("playback runtime panicked"));
            assert!(message.contains("synthetic dispatch panic"));
        }
        other => panic!("expected error event, got {other:?}"),
    }
    match event_rx
        .try_recv()
        .expect("stopped event should follow the error event")
    {
        PlaybackRuntimeEvent::Stopped => {}
        other => panic!("expected stopped event, got {other:?}"),
    }
}

#[test]
fn runtime_recovery_composes_after_command_error_and_panic() {
    // Composition-level integration test for Phase B/C resilience: prove
    // that the runtime's recovery primitives (report_runtime_command_error,
    // stop_all_engines, handle_panic_in_runtime_loop) compose so the
    // runtime stays responsive across a command-error AND a panic in the
    // same session. A future plan will extract dispatch_command from
    // run_runtime_loop's match body to enable per-command coverage; this
    // test catches a regression that would break the recovery contract
    // these primitives together provide.
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);
    let mut state = test_runtime_loop_state();
    state.engine = Some(test_engine_with_shared(1, 1));
    state.next_engine = Some(test_engine_with_shared(2, 1));

    // 1. Simulate a Play command that returned Err: report the error and
    //    tear down engines (same sequence as run_runtime_loop's Play arm).
    report_runtime_command_error(&event_tx, "Play", anyhow::anyhow!("transition failed"));
    stop_all_engines(&mut state);
    assert!(state.engine.is_none(), "engine slot cleared after error");
    assert!(state.next_engine.is_none(), "next slot cleared after error");
    match event_rx.try_recv().expect("error event") {
        PlaybackRuntimeEvent::Error { message } => {
            assert!(message.contains("Play failed"));
        }
        other => panic!("expected error event, got {other:?}"),
    }

    // 2. Simulate a subsequent successful Play: engine re-populates.
    state.engine = Some(test_engine_with_shared(10, 2));
    assert_eq!(state.engine.as_ref().unwrap().track_id, 10);

    // 3. Simulate a panic in dispatch: handle_panic_in_runtime_loop should
    //    clear all engines and signal the loop can continue.
    let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("synthetic dispatch panic"));
    let outcome = handle_panic_in_runtime_loop(payload, &event_tx, &mut state);
    assert!(
        matches!(outcome, std::ops::ControlFlow::Continue(())),
        "loop should continue after recoverable panic"
    );
    assert!(state.engine.is_none(), "engine slot cleared after panic");

    // The panic handler emits Error + Stopped.
    match event_rx.try_recv().expect("panic error event") {
        PlaybackRuntimeEvent::Error { message } => {
            assert!(message.contains("playback runtime panicked"));
        }
        other => panic!("expected error event, got {other:?}"),
    }
    match event_rx.try_recv().expect("stopped event") {
        PlaybackRuntimeEvent::Stopped => {}
        other => panic!("expected stopped event, got {other:?}"),
    }

    // 4. The loop is still operational: state accepts a new engine.
    state.engine = Some(test_engine_with_shared(20, 3));
    assert_eq!(state.engine.as_ref().unwrap().track_id, 20);
}

fn test_engine_with_shared(track_id: i64, generation: u64) -> PlaybackEngine {
    let (command_tx, _) = mpsc::channel();
    PlaybackEngine::test_with_shared(
        track_id,
        generation,
        Arc::new(PlaybackSharedState::new(
            track_id,
            generation,
            PlaybackSourceKind::TidalStream,
            GaplessPlan::disabled(),
            48_000,
            2,
            None,
            command_tx,
            Arc::new(AtomicU32::new(1.0f32.to_bits())),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        )),
    )
}

#[test]
fn selecting_predecoded_next_binds_transport_readers_and_preserves_pause() {
    let mut state = test_runtime_loop_state();
    state.user_paused = true;
    let pre = test_engine_with_shared(2, 10);
    pre.shared
        .position_offset_samples
        .store(96_000, Ordering::Relaxed);
    pre.shared.position_samples.store(96_000, Ordering::Relaxed);
    pre.shared
        .append_decoded_samples(&vec![0.7; 288_000])
        .unwrap();
    pre.shared.apply_in_buffer_seek(192_000).unwrap();
    state.next_engine = Some(pre);
    let (command_tx, _) = mpsc::channel();
    let handle = PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    state.handoff_elapsed_source = Arc::clone(&handle.handoff_elapsed_source);
    handle.test_publish_position(9_600_000);
    adopt_predecoded_next_engine(
        &mut state,
        &handle.position_source,
        &handle.buffered_source,
        &handle.offset_source,
    );
    assert_eq!(handle.get_position_ms(48_000, 2), 2000);
    assert_eq!(handle.get_buffered_start_ms(48_000, 2), 1000);
    assert_eq!(handle.get_buffered_ms(48_000, 2), 4000);
    let active = state.engine.as_ref().unwrap();
    assert!(active.shared.paused.load(Ordering::SeqCst));
    active.shared.apply_in_buffer_seek(288_000).unwrap();
    assert_eq!(handle.get_position_ms(48_000, 2), 3000);
    assert!(state.next_engine.is_none());
}

#[test]
fn prepared_policy_update_keeps_decoder_and_identity_and_refuses_late_replacement() {
    let mut state = state_with_ready_dj_pair();
    state.device_sample_rate = 48_000;
    state.device_channels = 2;
    state.engine.as_mut().unwrap().job.track.duration_ms = Some(120_000);
    let active = state.engine.as_ref().unwrap();
    assert_eq!(active.shared.total_samples.load(Ordering::Relaxed), 0);
    active
        .shared
        .position_samples
        .store(90 * 96_000, Ordering::Relaxed);
    let original_pcm = state.next_engine.as_ref().unwrap().shared.clone();
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.transition_event_id = Some(101);
    transition.program =
        crate::playback::dj_engine::safe_crossfade_program(48_000, 2, noor_mix::Policy::default());
    let gapless = GaplessPlan {
        enabled: true,
        overlap_ms: 6000,
        prebuffer_ms: 500,
        requires_stream_metadata: false,
    };
    state.next_engine.as_mut().unwrap().job.prepared_transition = Some(transition.clone());
    state.next_engine.as_mut().unwrap().job.gapless = gapless;
    let job = state.next_engine.as_ref().unwrap().job.clone();
    assert!(arm_active_transition_window(&mut state, &job));
    transition.program = crate::playback::dj_engine::safe_crossfade_program(
        48_000,
        2,
        noor_mix::Policy {
            default_crossfade_ms: 9000,
            ..Default::default()
        },
    );
    let mut slower = gapless;
    slower.overlap_ms = 9000;
    assert!(update_prepared_transition_in_state(
        &mut state,
        transition.clone(),
        slower
    ));
    assert_eq!(
        state
            .engine
            .as_ref()
            .unwrap()
            .shared
            .total_samples
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(
        state
            .engine
            .as_ref()
            .unwrap()
            .shared
            .dj_fire_trigger_samples
            .load(Ordering::Relaxed),
        u64::MAX
    );
    let next = state.next_engine.as_ref().unwrap();
    assert!(Arc::ptr_eq(&original_pcm, &next.shared));
    assert_eq!(next.shared.buffer.lock().unwrap().samples, vec![0.5; 4]);
    assert_eq!(
        next.job
            .prepared_transition
            .as_ref()
            .unwrap()
            .transition_event_id,
        Some(101)
    );
    assert_eq!(
        state
            .engine
            .as_ref()
            .unwrap()
            .shared
            .crossfade_samples
            .load(Ordering::Relaxed),
        9 * 96_000
    );
    let mut stale = transition.clone();
    stale.next_queue_item_id = Some(99);
    assert!(!update_prepared_transition_in_state(
        &mut state, stale, gapless
    ));
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .position_samples
        .store(112 * 96_000, Ordering::Relaxed);
    assert!(!update_prepared_transition_in_state(
        &mut state,
        transition.clone(),
        gapless
    ));
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .position_samples
        .store(90 * 96_000, Ordering::Relaxed);
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .crossfade_start_signaled
        .store(true, Ordering::Relaxed);
    assert!(!update_prepared_transition_in_state(
        &mut state, transition, gapless
    ));
}

#[test]
fn early_gridless_preparation_waits_for_length_and_renders_the_tail_not_the_opening() {
    let mut state = state_with_ready_dj_pair();
    state.device_sample_rate = 48_000;
    state.device_channels = 2;
    let mut transition = test_prepared_transition_program(20, Some(11), Some(12));
    transition.program = crate::playback::dj_engine::safe_crossfade_program(
        48_000,
        2,
        noor_mix::Policy {
            default_crossfade_ms: 1000,
            ..Default::default()
        },
    );
    let mut job = state.next_engine.as_ref().unwrap().job.clone();
    job.prepared_transition = Some(transition);
    job.gapless.overlap_ms = 1000;
    state.next_engine.as_mut().unwrap().job = job.clone();
    assert!(arm_active_transition_window(&mut state, &job));
    assert!(!can_prepare_dj_mixer_before_fire(&state));
    let active = state.engine.as_ref().unwrap();
    let mut samples = vec![0.2; 6 * 96_000];
    samples.extend(vec![0.7; 96_000]);
    active.shared.buffer.lock().unwrap().samples = samples;
    active
        .shared
        .total_samples
        .store(7 * 96_000, Ordering::Relaxed);
    state
        .next_engine
        .as_ref()
        .unwrap()
        .shared
        .buffer
        .lock()
        .unwrap()
        .samples = vec![0.3; 2 * 96_000];
    assert!(can_prepare_dj_mixer_before_fire(&state));
    prepare_dj_mixer_for_pair(&mut state, 1024).unwrap();
    let rendered = state.dj.prepared_mixer.as_ref().unwrap();
    assert_eq!(rendered.program.deck_a_start_frame, 6 * 48_000);
    assert!(
        rendered.rendered[0] > 0.5,
        "must contain the outgoing tail, not its opening"
    );
}

#[test]
fn rhythmic_tail_snapshot_retains_fractional_millisecond_frames_at_eof() {
    let (state, _, _) = native_adaptive_readiness_case();
    let active = state.engine.as_ref().unwrap();
    let mut transition = state
        .next_engine
        .as_ref()
        .unwrap()
        .job
        .prepared_transition
        .clone()
        .unwrap();
    transition.anchor_start_ms = None;
    transition.program.resolve_at = 371_618; // 7.7420417s at 48 kHz.
    let total_frames = 10 * 48_000;
    active.shared.buffer.lock().unwrap().samples = vec![0.2; total_frames * 2];
    active
        .shared
        .total_samples
        .store((total_frames * 2) as u64, Ordering::Relaxed);
    active
        .shared
        .crossfade_samples
        .store(371_616 * 2, Ordering::Relaxed);
    let anchor = anchored_deck_a_output_frame(&state, &transition, active).unwrap();
    let snapshot =
        active_deck_snapshot(active, 2, 0, Some(anchor), transition.program.resolve_at, 0)
            .expect("complete fractional-beat window must fit before EOF");
    assert_eq!(
        snapshot.start_frame + transition.program.resolve_at,
        total_frames as u64
    );
}

#[test]
fn analysis_reuses_only_fresh_manifests_of_decks_that_decoded_audio() {
    let mut state = state_with_ready_dj_pair();
    let info: StreamInfo = serde_json::from_value(serde_json::json!({
        "url":"https://audio.example.test/track.flac","segment_urls":[],"trackId":1,
        "audioQuality":"LOSSLESS","codec":"flac","sampleRate":44100,"bitDepth":16
    }))
    .unwrap();
    state.engine.as_mut().unwrap().job.resolved_stream =
        Some(crate::playback::player::ResolvedStream {
            info,
            resolved_at: Instant::now(),
        });
    assert_eq!(
        resolved_analysis_stream_in_state(&state, 1).unwrap().codec,
        "flac"
    );
    assert!(resolved_analysis_stream_in_state(&state, 99).is_none());
    state
        .engine
        .as_mut()
        .unwrap()
        .job
        .resolved_stream
        .as_mut()
        .unwrap()
        .resolved_at -= Duration::from_secs(61);
    assert!(resolved_analysis_stream_in_state(&state, 1).is_none());
    state
        .engine
        .as_mut()
        .unwrap()
        .job
        .resolved_stream
        .as_mut()
        .unwrap()
        .resolved_at = Instant::now();
    state
        .engine
        .as_ref()
        .unwrap()
        .shared
        .stopped
        .store(true, Ordering::Relaxed);
    assert!(resolved_analysis_stream_in_state(&state, 1).is_none());
}

fn state_with_ready_dj_pair() -> PlaybackRuntimeLoopState {
    let mut state = test_runtime_loop_state();
    start_dj_lookahead_in_state(
        &mut state,
        Some(DjMediaRef::LibraryTrack { track_id: 1 }),
        Some(DjMediaRef::LibraryTrack { track_id: 2 }),
        Some(11),
        Some(12),
        20,
        48_000,
    );
    let active = test_engine_with_shared(1, 20);
    finish_engine_buffer(&active, &[0.25, 0.25, 0.25, 0.25]);
    let mut next = test_engine_with_shared(2, 21);
    next.job = PreparedPlaybackJob::test_fixture(2, 21)
        .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));
    finish_engine_buffer(&next, &[0.5, 0.5, 0.5, 0.5]);
    state.engine = Some(active);
    state.next_engine = Some(next);
    state
}

fn finish_engine_buffer(engine: &PlaybackEngine, samples: &[f32]) {
    let mut buffer = engine.shared.buffer.lock().expect("buffer lock");
    buffer.samples.extend_from_slice(samples);
    buffer.mark_finished();
}

fn assert_samples_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (*actual - *expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }
}

fn test_prepared_transition_program(
    queue_generation: u64,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
) -> PreparedTransitionProgram {
    PreparedTransitionProgram {
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
        transition_event_id: None,
        fire_ahead_ms: 0,
        queue_generation,
        current_queue_item_id,
        next_queue_item_id,
        anchor_start_ms: None,
    }
}

// -- Option C: evaluate_seek_decision unit tests (moved from server::routes
//    per r6 fix A; the helper now lives in this module). --

#[test]
fn evaluate_seek_decision_dispatches_when_no_runtime_active() {
    assert_eq!(
        super::evaluate_seek_decision(1_000_000, 0, 0, false),
        super::SeekDecision::Dispatch,
    );
}

#[test]
fn evaluate_seek_decision_dispatches_when_buffer_is_fresh() {
    assert_eq!(
        super::evaluate_seek_decision(500_000, 0, 0, true),
        super::SeekDecision::Dispatch,
    );
}

#[test]
fn evaluate_seek_decision_dispatches_when_target_within_buffered() {
    assert_eq!(
        super::evaluate_seek_decision(100_000, 0, 200_000, true),
        super::SeekDecision::Dispatch,
    );
    assert_eq!(
        super::evaluate_seek_decision(200_000, 0, 200_000, true),
        super::SeekDecision::Dispatch,
    );
}

#[test]
fn evaluate_seek_decision_rejects_target_strictly_past_buffer() {
    assert_eq!(
        super::evaluate_seek_decision(300_000, 0, 200_000, true),
        super::SeekDecision::RejectOutOfBuffer,
    );
}

#[test]
fn evaluate_seek_decision_rejects_target_below_offset() {
    // r5 finding (P2): decoded range after segment-restart is
    // [offset, buffered], not [0, buffered]. A backward seek below the
    // offset must NOT take the fast path.
    assert_eq!(
        super::evaluate_seek_decision(10_000, 30_000, 50_000, true),
        super::SeekDecision::RejectOutOfBuffer,
    );
}

#[test]
fn evaluate_seek_decision_dispatches_target_within_post_offset_range() {
    // Same offset as the test above; target sits inside [30k, 50k].
    assert_eq!(
        super::evaluate_seek_decision(40_000, 30_000, 50_000, true),
        super::SeekDecision::Dispatch,
    );
}

#[test]
fn offset_source_redirect_makes_handle_read_from_new_engine() {
    // r2 codex finding (P1) extended for option C: the handle's
    // get_buffered_start_ms must follow the same redirect pattern as
    // position_source / buffered_source so a Switch / promotion swaps the
    // reader to the new engine's offset atomic, not the stale one.
    let (command_tx, _) = std::sync::mpsc::channel();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);

    let engine_a_offset = Arc::new(AtomicU64::new(0));
    let engine_b_offset = Arc::new(AtomicU64::new(48_000 * 2 * 30)); // 30 s @ 48k stereo

    let offset_source: Arc<Mutex<Arc<AtomicU64>>> =
        Arc::new(Mutex::new(Arc::clone(&engine_a_offset)));

    let handle = PlaybackRuntimeHandle {
        command_tx,
        event_tx,
        healthy: Arc::new(AtomicBool::new(true)),
        volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
        buffered_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
        offset_source: Arc::clone(&offset_source),
        handoff_elapsed_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(u64::MAX)))),
    };

    assert_eq!(handle.buffered_start_samples(), 0);
    assert_eq!(handle.get_buffered_start_ms(48_000, 2), 0);

    *offset_source.lock().unwrap() = Arc::clone(&engine_b_offset);

    assert_eq!(handle.buffered_start_samples(), 48_000 * 2 * 30);
    assert_eq!(handle.get_buffered_start_ms(48_000, 2), 30_000);

    // Stale writes to engine A must not leak through.
    engine_a_offset.store(999_999, Ordering::Relaxed);
    assert_eq!(handle.buffered_start_samples(), 48_000 * 2 * 30);
}
