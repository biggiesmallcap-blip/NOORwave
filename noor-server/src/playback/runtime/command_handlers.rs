//! Handlers for the larger runtime commands. The loop in mod.rs dispatches to
//! these inside its panic guard; `return ControlFlow::Break(())` stops the loop.

use super::*;
use std::ops::ControlFlow;

/// Loop-owned resources a command handler may use. Built once per command.
pub(super) struct LoopEnv<'a> {
    pub(super) config: &'a mut PlaybackRuntimeConfig,
    pub(super) command_tx: &'a mpsc::Sender<PlaybackRuntimeCommand>,
    pub(super) event_tx: &'a tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    pub(super) device: &'a mut cpal::Device,
    pub(super) output_config: &'a mut StreamConfig,
    pub(super) output_sample_format: &'a mut SampleFormat,
    pub(super) volume_ctl: &'a Arc<AtomicU32>,
    pub(super) position_samples: &'a Arc<AtomicU64>,
    pub(super) position_source: &'a Arc<Mutex<Arc<AtomicU64>>>,
    pub(super) buffered_source: &'a Arc<Mutex<Arc<AtomicU64>>>,
    pub(super) offset_source: &'a Arc<Mutex<Arc<AtomicU64>>>,
}

/// Seek inside the active track, or report why the seek cannot land.
pub(super) fn handle_seek_to(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    target_ms: i64,
    allow_segment_seek: bool,
    respond_to: mpsc::Sender<SeekToOutcome>,
) -> ControlFlow<()> {
    let LoopEnv {
        config,
        command_tx,
        event_tx,
        device,
        output_config,
        output_sample_format,
        volume_ctl,
        position_samples,
        position_source,
        buffered_source,
        offset_source,
        ..
    } = env;
    // Phase 1: snapshot everything we need under an immutable
    // borrow of state.engine. Inside this block we decide
    // among in-buffer fast path / segment-restart / reject;
    // the actual mutation happens in phase 2 with the
    // immutable borrow already dropped (per r6 fix C).
    // Lives for one seek on the stack; boxing the job buys nothing.
    #[allow(clippy::large_enum_variant)]
    enum SeekHandling {
        InBuffer { target_samples: u64 },
        Reject,
        SegmentSeek { job: PreparedPlaybackJob },
    }
    let rate = state.device_sample_rate as u64;
    let channels = state.device_channels.max(1) as u64;
    let decision: SeekHandling = {
        let Some(engine) = state.engine.as_ref() else {
            let _ = respond_to.send(SeekToOutcome::RejectedOutOfBuffer);
            return std::ops::ControlFlow::Continue(());
        };
        let target_samples = (target_ms.max(0) as u64)
            .saturating_mul(rate)
            .saturating_mul(channels)
            / 1000;
        let offset_samples = engine.shared.source_offset_samples.load(Ordering::Relaxed);
        let buffered_samples = engine.shared.buffered_samples.load(Ordering::Relaxed);

        match evaluate_seek_decision(target_samples, offset_samples, buffered_samples, true) {
            SeekDecision::Dispatch => SeekHandling::InBuffer { target_samples },
            SeekDecision::RejectOutOfBuffer if !allow_segment_seek => SeekHandling::Reject,
            SeekDecision::RejectOutOfBuffer => {
                // Segment-restart path: find the segment whose
                // start_ms is the largest <= target_ms, build a
                // new job that starts from there. Clone the job
                // so the borrow ends with this scope.
                let Some(offsets) = engine.shared.segment_offsets_ms.get() else {
                    let _ = respond_to.send(SeekToOutcome::RejectedOutOfBuffer);
                    return std::ops::ControlFlow::Continue(());
                };
                if offsets.is_empty() {
                    let _ = respond_to.send(SeekToOutcome::RejectedOutOfBuffer);
                    return std::ops::ControlFlow::Continue(());
                }
                let target_ms_clamped = target_ms.max(0) as u64;
                let n = offsets
                    .iter()
                    .rposition(|off_ms| *off_ms <= target_ms_clamped)
                    .unwrap_or(0);
                let new_offset_ms = offsets[n];
                let new_job = {
                    let mut j = engine.job.clone();
                    j.start_from_segment_index = n;
                    j.start_from_offset_ms = new_offset_ms;
                    // Preserve the live transport intent, not
                    // the intent the ORIGINAL job carried: a
                    // seek while paused must restart the
                    // segment engine silent, still paused.
                    j.start_paused = state.user_paused;
                    j
                };
                SeekHandling::SegmentSeek { job: new_job }
            }
        }
    }; // immutable borrow of state.engine ends here

    // Phase 2: act on the decision under a mutable borrow.
    match decision {
        SeekHandling::InBuffer { target_samples } => {
            let mut suppressed = false;
            if let Some(engine) = state.engine.as_ref() {
                if let Err(error) = engine.shared.restore_source_buffer_after_seek() {
                    warn!("Could not retire rendered handoff for seek: {error}");
                    let _ = respond_to.send(SeekToOutcome::Failed);
                    return std::ops::ControlFlow::Continue(());
                }
                if let Err(error) = engine.shared.apply_in_buffer_seek(target_samples) {
                    warn!("Could not apply accepted decoded seek: {error}");
                    let _ = respond_to.send(SeekToOutcome::Failed);
                    return std::ops::ControlFlow::Continue(());
                }
                suppressed = engine
                    .shared
                    .set_manual_seek_crossfade_suppression(target_samples);
                // Reset fire-once guards so NearEnd /
                // CrossfadeStart re-fire after a backward seek.
                engine
                    .shared
                    .near_end_signaled
                    .store(false, Ordering::Relaxed);
                engine
                    .shared
                    .crossfade_start_signaled
                    .store(false, Ordering::Relaxed);
            }
            state.dj.prepared_mixer = None;
            state.dj.readiness_permanent_failure = None;
            let outcome = if suppressed {
                SeekToOutcome::DispatchedCrossfadeSuppressed
            } else {
                SeekToOutcome::Dispatched
            };
            let _ = respond_to.send(outcome);
        }
        SeekHandling::Reject => {
            let _ = respond_to.send(SeekToOutcome::RejectedOutOfBuffer);
        }
        SeekHandling::SegmentSeek { job } => {
            match transition_to_job(
                config,
                command_tx,
                device,
                output_config,
                *output_sample_format,
                event_tx,
                state,
                job,
                volume_ctl,
                position_samples,
                position_source,
                buffered_source,
                offset_source,
                true, // force_restart - bypass switch_is_noop
            ) {
                Ok(()) => {
                    let mut suppressed = false;
                    if let Some(engine) = state.engine.as_ref() {
                        let target_samples = (target_ms.max(0) as u64)
                            .saturating_mul(rate)
                            .saturating_mul(channels)
                            / 1000;
                        suppressed = engine
                            .shared
                            .set_manual_seek_crossfade_suppression(target_samples);
                    }
                    let outcome = if suppressed {
                        SeekToOutcome::DispatchedCrossfadeSuppressed
                    } else {
                        SeekToOutcome::Dispatched
                    };
                    let _ = respond_to.send(outcome);
                }
                Err(error) => {
                    warn!(
                        "Segment-seek transition failed: target_ms={}, err={:?}",
                        target_ms, error
                    );
                    let _ = respond_to.send(SeekToOutcome::Failed);
                }
            }
        }
    }
    ControlFlow::Continue(())
}

/// Start decoding the next track onto the prepared deck.
pub(super) fn handle_prepare_next(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    mut job: PreparedPlaybackJob,
) -> ControlFlow<()> {
    let LoopEnv {
        config,
        command_tx,
        event_tx,
        device,
        output_config,
        output_sample_format,
        volume_ctl,
        ..
    } = env;
    gate_prepare_next_for_dj(state, &mut job);
    arm_active_transition_window(state, &job);
    // Only pre-decode if we don't already have a pending engine for this track.
    let already_pending = state
        .next_engine
        .as_ref()
        .map(|e| e.track_id == job.track.id && e.generation == job.generation)
        .unwrap_or(false);
    if !already_pending {
        // Stop any stale pending engine first.
        if let Some(mut stale) = state.next_engine.take() {
            state.dj.prepared_mixer = None;
            stale.stop();
        }
        let pending_position = Arc::new(AtomicU64::new(0));
        let engine_result = if state.current_exclusive {
            PlaybackEngine::start_decoder_only(
                config,
                command_tx,
                job,
                state.device_sample_rate,
                state.device_channels,
                Arc::clone(volume_ctl),
                pending_position,
            )
        } else {
            PlaybackEngine::start(
                config,
                command_tx,
                device,
                output_config,
                *output_sample_format,
                job,
                event_tx.clone(),
                state.device_sample_rate,
                state.device_channels,
                Arc::clone(volume_ctl),
                pending_position,
            )
        };
        match engine_result {
            Ok(engine) => {
                // Keep the stream alive but software-paused so host pause does not
                // block control commands on some Linux/PipeWire setups.
                engine.shared.paused.store(true, Ordering::SeqCst);
                state.next_engine = Some(engine);
                if can_prepare_dj_mixer_before_fire(state) {
                    let _ =
                        prepare_dj_mixer_for_pair(state, dj_mixer_max_block_samples(output_config));
                }
                #[cfg(target_os = "windows")]
                if state.current_exclusive {
                    refresh_exclusive_sources(state);
                }
            }
            Err(err) => {
                warn!("Failed to pre-buffer next track: {err:?}");
            }
        }
    }
    ControlFlow::Continue(())
}

/// Start decoding a drop-preview deck.
pub(super) fn handle_prepare_drop_preview(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    job: PreparedPlaybackJob,
) -> ControlFlow<()> {
    let LoopEnv {
        config,
        command_tx,
        event_tx,
        device,
        output_config,
        output_sample_format,
        volume_ctl,
        ..
    } = env;
    if !state.dj.engine_enabled {
        state.dj.prepared_drop_preview_mixer = None;
        if let Some(mut stale) = state.drop_preview_engine.take() {
            stale.stop();
        }
        return std::ops::ControlFlow::Continue(());
    }
    let has_drop_preview_program = job
        .prepared_transition
        .as_ref()
        .is_some_and(|transition| transition.program.template == "DropPreview16");
    if !has_drop_preview_program {
        state.dj.prepared_drop_preview_mixer = None;
        if let Some(mut stale) = state.drop_preview_engine.take() {
            stale.stop();
        }
        return std::ops::ControlFlow::Continue(());
    }
    let already_pending = state
        .drop_preview_engine
        .as_ref()
        .map(|engine| engine.track_id == job.track.id && engine.generation == job.generation)
        .unwrap_or(false);
    if !already_pending {
        if let Some(mut stale) = state.drop_preview_engine.take() {
            state.dj.prepared_drop_preview_mixer = None;
            stale.stop();
        }
        let pending_position = Arc::new(AtomicU64::new(0));
        let engine_result = if state.current_exclusive {
            PlaybackEngine::start_decoder_only(
                config,
                command_tx,
                job,
                state.device_sample_rate,
                state.device_channels,
                Arc::clone(volume_ctl),
                pending_position,
            )
        } else {
            PlaybackEngine::start(
                config,
                command_tx,
                device,
                output_config,
                *output_sample_format,
                job,
                event_tx.clone(),
                state.device_sample_rate,
                state.device_channels,
                Arc::clone(volume_ctl),
                pending_position,
            )
        };
        match engine_result {
            Ok(engine) => {
                engine.shared.suppress_started_event();
                engine.shared.paused.store(true, Ordering::SeqCst);
                state.drop_preview_engine = Some(engine);
                let _ =
                    prepare_drop_preview_mixer(state, dj_mixer_max_block_samples(output_config));
                #[cfg(target_os = "windows")]
                if state.current_exclusive {
                    refresh_exclusive_sources(state);
                }
            }
            Err(err) => {
                warn!("Failed to pre-buffer drop preview: {err:?}");
            }
        }
    }
    ControlFlow::Continue(())
}

/// Begin the crossfade or DJ transition into the prepared deck.
pub(super) fn handle_crossfade_start(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    track_id: i64,
    generation: u64,
    trigger_position_samples: u64,
    trigger_target_samples: u64,
) -> ControlFlow<()> {
    let LoopEnv {
        event_tx,
        output_config,
        position_source,
        buffered_source,
        offset_source,
        ..
    } = env;
    // The OUTGOING engine just entered its fade-out window and is asking
    // us to start the pre-decoded next engine, if one is ready.
    if state.engine.as_ref().map(|e| (e.track_id, e.generation)) == Some((track_id, generation)) {
        let crossfade_samples = state
            .engine
            .as_ref()
            .map(|e| e.shared.crossfade_samples.load(Ordering::Relaxed))
            .unwrap_or(0);
        let next_buffer = state
            .next_engine
            .as_ref()
            .and_then(crossfade_readiness_snapshot);
        let next_ready = next_buffer
            .map(|buffer| dj_crossfade_next_ready(state, buffer, crossfade_samples))
            .unwrap_or(false);
        if next_ready && !active_engine_suppresses_crossfade_after_seek(state) {
            let runtime_planned_start_ms =
                runtime_transition_target_ms(state, Some(trigger_target_samples));
            let trigger_actual_start_ms = samples_to_ms(
                state
                    .engine
                    .as_ref()
                    .map(|engine| {
                        engine
                            .shared
                            .output_to_source_samples(trigger_position_samples)
                    })
                    .unwrap_or(trigger_position_samples),
                state.device_sample_rate,
                state.device_channels,
            );
            // The transition is pre-rendered at prepare /
            // decode-complete time; rebuilding here would put
            // an 8-28s render on the fire path and let deck A
            // drift past the snapshot while it runs. Only
            // rebuild if nothing usable was prepared.
            if !prepared_dj_mixer_matches_pair(state) {
                let _ = prepare_dj_mixer_for_pair(state, dj_mixer_max_block_samples(output_config));
            }
            if prepared_overlay_program(state) {
                let device_sample_rate = state.device_sample_rate;
                let device_channels = state.device_channels;
                if let Err(reason) = start_prepared_overlay(
                    state,
                    event_tx,
                    "fired",
                    DjRuntimeRendererReason::None,
                    Some(trigger_actual_start_ms),
                    runtime_planned_start_ms,
                    device_sample_rate,
                    device_channels,
                ) {
                    record_current_runtime_renderer_failure(state, reason);
                }
            } else {
                let runtime_renderer = match install_prepared_handoff_mixer_buffer(state) {
                    Ok(()) => DjRuntimeRendererOutcome::rendered_handoff(),
                    Err(reason) => {
                        let failure = runtime_renderer_failure_reason(state, reason);
                        record_current_runtime_renderer_failure(state, failure);
                        DjRuntimeRendererOutcome::legacy_overlap(failure)
                    }
                };
                promote_next_to_active(
                    state,
                    event_tx,
                    position_source,
                    buffered_source,
                    offset_source,
                    "fired",
                    Some(trigger_actual_start_ms),
                    runtime_planned_start_ms,
                    runtime_renderer,
                );
            }
        } else if !active_engine_suppresses_crossfade_after_seek(state) {
            let incoming = state.next_engine.as_ref();
            let incoming_rate = incoming.map(|engine| {
                engine
                    .shared
                    .target_sample_rate
                    .load(Ordering::Relaxed)
                    .max(1)
            });
            let runtime_samples_per_second = f64::from(state.device_sample_rate.max(1))
                * f64::from(state.device_channels.max(1));
            let incoming_samples_per_second =
                f64::from(incoming_rate.unwrap_or(state.device_sample_rate).max(1))
                    * f64::from(state.device_channels.max(1));
            let program = incoming
                .and_then(|engine| engine.job.prepared_transition.as_ref())
                .map(|transition| {
                    transition
                        .program
                        .clone()
                        .rescaled_to(state.device_sample_rate.max(1))
                });
            let program_required_seconds = program.as_ref().and_then(|program| {
                deck_b_consumed_frames(program).map(|consumed| {
                    program.deck_b_start_frame.saturating_add(consumed) as f64
                        / f64::from(state.device_sample_rate.max(1))
                })
            });
            let reason = runtime_renderer_fire_block_reason(state, next_ready);
            info!(
                outgoing_track_id = track_id,
                next_track_id = incoming.map(|engine| engine.track_id),
                transition_event_id = incoming
                    .and_then(|engine| engine.job.prepared_transition.as_ref())
                    .and_then(|transition| transition.transition_event_id),
                reason = reason.as_str(),
                next_present = incoming.is_some(),
                buffer_lock_ok = next_buffer.is_some(),
                base_ready = next_buffer.map(|buffer| buffer.base_ready),
                finished = next_buffer.map(|buffer| buffer.finished),
                incoming_unread_seconds = next_buffer
                    .map(|buffer| buffer.unread_samples as f64 / incoming_samples_per_second),
                incoming_unread_runtime_seconds = next_buffer
                    .map(|buffer| buffer.unread_samples as f64 / runtime_samples_per_second),
                incoming_decoded_samples = next_buffer.map(|buffer| buffer.decoded_samples),
                incoming_read_samples = next_buffer.map(|buffer| buffer.read_samples),
                incoming_offset_samples = next_buffer.map(|buffer| buffer.offset_samples),
                start_threshold_samples = next_buffer.map(|buffer| buffer.start_threshold_samples),
                required_long_seconds = crossfade_samples.saturating_add(crossfade_samples / 8)
                    as f64
                    / runtime_samples_per_second,
                required_program_source_seconds = program_required_seconds,
                required_adaptive_unread_seconds = next_buffer
                    .and_then(|buffer| adaptive_next_required_samples(state, buffer))
                    .map(|samples| samples as f64 / runtime_samples_per_second),
                runtime_sample_rate = state.device_sample_rate,
                output_config_sample_rate = output_config.sample_rate,
                incoming_engine_sample_rate =
                    incoming.map(|engine| engine.shared.device_sample_rate),
                incoming_decoder_target_sample_rate = incoming_rate,
                outgoing_engine_sample_rate = state
                    .engine
                    .as_ref()
                    .map(|engine| engine.shared.device_sample_rate),
                outgoing_decoder_target_sample_rate = state
                    .engine
                    .as_ref()
                    .map(|engine| engine.shared.target_sample_rate.load(Ordering::Relaxed)),
                prepared_mixer_matches = prepared_dj_mixer_matches_pair(state),
                last_prepare_failure = state
                    .dj
                    .last_renderer_failure
                    .map(|failure| failure.reason.as_str()),
                "DJ transition fire blocked by incoming audio readiness"
            );
            // The next deck can't back the full fade in time. Silence the
            // outgoing track's own fade-out so it plays at full volume to its
            // end rather than fading down into a gap; the boundary then makes a
            // clean gapless cut instead of a fade-to-silence-then-pop.
            if let Some(active) = state.engine.as_ref() {
                active.shared.crossfade_samples.store(0, Ordering::Relaxed);
            }
            record_current_runtime_renderer_failure(state, reason);
        }
        // If not ready yet, NextDecodeComplete handles the late path.
    }
    ControlFlow::Continue(())
}

/// Fire an armed drop preview.
pub(super) fn handle_drop_preview_start(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    track_id: i64,
    generation: u64,
    trigger_position_samples: u64,
) -> ControlFlow<()> {
    let LoopEnv {
        event_tx,
        output_config,
        ..
    } = env;
    if !state.dj.engine_enabled {
        if let Some(active) = state.engine.as_ref() {
            active.shared.clear_drop_preview_trigger();
        }
        return std::ops::ControlFlow::Continue(());
    }
    if state
        .engine
        .as_ref()
        .map(|engine| (engine.track_id, engine.generation))
        == Some((track_id, generation))
    {
        let preparation =
            prepare_drop_preview_mixer(state, dj_mixer_max_block_samples(output_config));
        let actual_start_ms = samples_to_ms(
            state
                .engine
                .as_ref()
                .map(|engine| {
                    engine
                        .shared
                        .output_to_source_samples(trigger_position_samples)
                })
                .unwrap_or(trigger_position_samples),
            state.device_sample_rate,
            state.device_channels,
        );
        if let Err(reason) = preparation
            .and_then(|()| start_prepared_drop_preview_overlay(state, event_tx, actual_start_ms))
        {
            debug!("Drop preview start skipped: {}", reason.as_str());
            let _ = event_tx.send(PlaybackRuntimeEvent::DropPreviewSkipped {
                track_id,
                generation,
                queue_generation: state
                    .drop_preview_engine
                    .as_ref()
                    .and_then(|engine| engine.job.prepared_transition.as_ref())
                    .map_or(0, |plan| plan.queue_generation),
                reason: if reason == DjRuntimeRendererReason::MixerRejected {
                    "beat_sync_unverified"
                } else {
                    reason.as_str()
                },
            });
            state.dj.prepared_drop_preview_mixer = None;
            if let Some(mut engine) = state.drop_preview_engine.take() {
                engine.stop();
            }
        }
    }
    ControlFlow::Continue(())
}

/// The prepared deck finished decoding; promote it if the boundary already passed.
pub(super) fn handle_next_decode_complete(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    track_id: i64,
    generation: u64,
) -> ControlFlow<()> {
    let LoopEnv {
        event_tx,
        output_config,
        position_source,
        buffered_source,
        offset_source,
        ..
    } = env;
    // Decode for the pre-decoded next engine completed. If the outgoing
    // engine has already entered the crossfade window, promote now -
    // the user will hear a clipped fade-in, but it's better than silence.
    let pending_match = state
        .next_engine
        .as_ref()
        .map(|e| e.track_id == track_id && e.generation == generation)
        .unwrap_or(false);
    if pending_match {
        let crossfade_started = state
            .engine
            .as_ref()
            .map(|e| e.shared.crossfade_start_signaled.load(Ordering::Relaxed))
            .unwrap_or(false);
        let late_fire_reason = if crossfade_started {
            runtime_renderer_late_fire_reason(state)
        } else {
            DjRuntimeRendererReason::None
        };
        if !prepared_dj_mixer_matches_pair(state) && can_prepare_dj_mixer_before_fire(state) {
            let _ = prepare_dj_mixer_for_pair(state, dj_mixer_max_block_samples(output_config));
        }
        if crossfade_started && !active_engine_suppresses_crossfade_after_seek(state) {
            let runtime_planned_start_ms = runtime_transition_target_ms(state, None);
            if prepared_overlay_program(state) {
                let device_sample_rate = state.device_sample_rate;
                let device_channels = state.device_channels;
                if let Err(reason) = start_prepared_overlay(
                    state,
                    event_tx,
                    "late",
                    late_fire_reason,
                    None,
                    runtime_planned_start_ms,
                    device_sample_rate,
                    device_channels,
                ) {
                    record_current_runtime_renderer_failure(state, reason);
                }
            } else {
                let runtime_renderer = match install_prepared_handoff_mixer_buffer(state) {
                    Ok(()) => {
                        DjRuntimeRendererOutcome::rendered_handoff_with_reason(late_fire_reason)
                    }
                    Err(reason) => {
                        let failure = runtime_renderer_failure_reason(state, reason);
                        record_current_runtime_renderer_failure(state, failure);
                        DjRuntimeRendererOutcome::legacy_overlap(failure)
                    }
                };
                promote_next_to_active(
                    state,
                    event_tx,
                    position_source,
                    buffered_source,
                    offset_source,
                    "late",
                    None,
                    runtime_planned_start_ms,
                    runtime_renderer,
                );
            }
        }
    }
    ControlFlow::Continue(())
}

/// Resume playback, re-acquiring an exclusive device if it was released.
pub(super) fn handle_resume(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
) -> ControlFlow<()> {
    let LoopEnv {
        command_tx,
        event_tx,
        device,
        output_config,
        output_sample_format,
        ..
    } = env;
    // Clear the user-pause latch and give the advance-cascade
    // breaker a fresh start: an explicit resume is the user
    // asking to try audio again.
    state.user_paused = false;
    state.silent_start_streak = 0;
    // On-demand re-grab: if exclusive mode is on and the active
    // engine's WASAPI stream self-released after idle, rebuild it
    // BEFORE unpausing so the decoder doesn't push samples into a
    // missing stream. swap_stream handles its own cpal-shared
    // fallback if the re-grab now fails (e.g. another app grabbed
    // exclusive while we were paused).
    if state.current_exclusive {
        #[cfg(target_os = "windows")]
        if state.exclusive_sink.needs_rebuild() {
            let regrab_start = std::time::Instant::now();
            info!(
                "Resume: rebuilding exclusive stream after idle release on {}",
                state.device_name
            );
            refresh_exclusive_sources(state);
            let rebuild_rate =
                exclusive_rebuild_rate(state.current_sample_rate_follow, state.device_sample_rate);
            let release_grace_secs = state.current_exclusive_release_grace_secs;
            let latency_mode = state.current_exclusive_latency_mode.clone();
            match ensure_exclusive_sink_started(
                state,
                device,
                output_config,
                rebuild_rate,
                release_grace_secs,
                latency_mode,
                command_tx.clone(),
                event_tx.clone(),
            ) {
                Ok(actual_rate) => {
                    output_config.sample_rate = actual_rate;
                    state.device_sample_rate = actual_rate;
                    info!(
                        target: "playback",
                        regrab_ms = regrab_start.elapsed().as_millis() as u64,
                        "Resume: exclusive re-grab complete"
                    );
                }
                Err(err) => {
                    warn!(
                        "Resume: failed to rebuild exclusive sink; falling back to shared: {err:?}"
                    );
                    // Drop the state borrow before potential cleanup
                    // so we can call stop_all_engines on the failure path
                    // without a borrow-checker conflict.
                    let swap_result = state.engine.as_mut().map(|engine| {
                        engine.swap_stream(
                            device,
                            output_config,
                            *output_sample_format,
                            command_tx.clone(),
                            event_tx.clone(),
                            false,
                            rebuild_rate,
                            release_grace_secs,
                        )
                    });
                    match swap_result {
                        Some(Ok(actual_rate)) => {
                            output_config.sample_rate = actual_rate;
                            state.device_sample_rate = actual_rate;
                        }
                        Some(Err(error)) => {
                            // Symmetric with Play/Switch error cleanup:
                            // when both exclusive rebuild and shared
                            // fallback fail, the active engine has no
                            // output stream but its decoder keeps
                            // filling the buffer. Tear it down rather
                            // than leave a silent zombie engine.
                            stop_all_engines(state);
                            state.exclusive_sink.clear();
                            report_runtime_command_error(event_tx, "Resume", error);
                        }
                        None => {}
                    }
                }
            }
        }
    }

    if let Some(engine) = state.engine.as_mut() {
        match engine.resume() {
            Ok(()) => {
                let _ = event_tx.send(PlaybackRuntimeEvent::Resumed {
                    track_id: Some(engine.track_id),
                });
            }
            Err(error) => {
                report_runtime_command_error(event_tx, "Resume", error);
            }
        }
    }
    if let Some(engine) = state.fading_out_engine.as_mut()
        && let Err(error) = engine.resume()
    {
        report_runtime_command_error(event_tx, "Resume", error);
    }
    if let Some(engine) = state
        .drop_preview_engine
        .as_mut()
        .filter(|engine| !engine.shared.paused.load(Ordering::SeqCst))
        && let Err(error) = engine.resume()
    {
        report_runtime_command_error(event_tx, "Resume", error);
    }
    ControlFlow::Continue(())
}

/// An engine reached its end or failed; advance, promote or stop.
pub(super) fn handle_track_terminal(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    track_id: i64,
    generation: u64,
    outcome: PlaybackTerminalReason,
) -> ControlFlow<()> {
    let LoopEnv {
        event_tx,
        position_source,
        buffered_source,
        offset_source,
        ..
    } = env;
    // The fading-out engine reaching its terminal state is the
    // expected end of a crossfade - drop it silently. The queue
    // advance already happened at promotion time via Finished.
    let fading = state
        .fading_out_engine
        .as_ref()
        .map(|e| (e.track_id, e.generation));
    let drop_preview = state
        .drop_preview_engine
        .as_ref()
        .map(|engine| (engine.track_id, engine.generation));
    let next = state
        .next_engine
        .as_ref()
        .map(|engine| (engine.track_id, engine.generation));
    let active = state
        .engine
        .as_ref()
        .map(|engine| (engine.track_id, engine.generation));

    match terminal_engine_slot(active, next, fading, drop_preview, track_id, generation) {
        Some(TerminalEngineSlot::FadingOut) => {
            debug!(
                "Playback terminal ignored for fading engine: track_id={}, generation={}, outcome={:?}",
                track_id, generation, outcome
            );
            if let Some(mut engine) = state.fading_out_engine.take() {
                engine.stop();
            }
        }
        Some(TerminalEngineSlot::Next) => {
            debug!(
                "Playback terminal ignored for prepared engine: track_id={}, generation={}, outcome={:?}",
                track_id, generation, outcome
            );
            if let PlaybackTerminalReason::Error(message) = &outcome
                && let Some(next_engine) = state.next_engine.as_ref()
            {
                emit_prepared_track_failure(event_tx, &next_engine.job, message);
            }
            if let Some(mut engine) = state.next_engine.take() {
                engine.stop();
            }
        }
        Some(TerminalEngineSlot::DropPreview) => {
            debug!(
                "Playback terminal ignored for drop preview engine: track_id={}, generation={}, outcome={:?}",
                track_id, generation, outcome
            );
            state.dj.prepared_drop_preview_mixer = None;
            if let Some(mut engine) = state.drop_preview_engine.take() {
                engine.stop();
            }
        }
        Some(TerminalEngineSlot::Active) => {
            debug!(
                "Playback terminal active engine: track_id={}, generation={}, outcome={:?}",
                track_id, generation, outcome
            );
            if should_promote_prepared_at_boundary(active, next, track_id, generation, &outcome) {
                promote_prepared_at_boundary(
                    state,
                    event_tx,
                    position_source,
                    buffered_source,
                    offset_source,
                );
            } else {
                stop_current_engine(state);
                match outcome {
                    PlaybackTerminalReason::Finished => {
                        let _ = event_tx.send(PlaybackRuntimeEvent::Finished {
                            track_id,
                            generation,
                        });
                    }
                    PlaybackTerminalReason::Error(message) => {
                        let _ = event_tx.send(PlaybackRuntimeEvent::TrackError {
                            track_id,
                            generation,
                            message,
                        });
                    }
                }
            }
        }
        None => {
            // The terminal is one-shot (`finished_notified` is
            // latched before the send), so a terminal that
            // matches no live slot is an advance that will
            // never be re-issued from the audio callback. The
            // stall watchdog is the backstop; surface it at
            // warn so the drop is visible when it happens.
            warn!(
                target: "noor.playback.advance",
                event = "terminal_unmatched_engine",
                track_id,
                generation,
                outcome = ?outcome,
                active = ?active,
                next = ?next,
                fading = ?fading,
                drop_preview = ?drop_preview,
                "playback terminal matched no live engine slot; advance dropped"
            );
        }
    }
    #[cfg(target_os = "windows")]
    if state.current_exclusive {
        refresh_exclusive_sources(state);
    }
    ControlFlow::Continue(())
}

/// Move output to another device, mode or sample rate.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_device_swap(
    env: LoopEnv<'_>,
    state: &mut PlaybackRuntimeLoopState,
    selection: OutputDeviceSelection,
    exclusive: bool,
    sample_rate_follow: bool,
    desired_sample_rate: Option<u32>,
    exclusive_release_grace_secs: u32,
    exclusive_latency_mode: ExclusiveLatencyMode,
) -> ControlFlow<()> {
    let LoopEnv {
        command_tx,
        event_tx,
        device,
        output_config,
        output_sample_format,
        ..
    } = env;
    // `exclusive` is honored as of Task 5 (Windows-only low-latency
    // buffer + dedicated code path; full ShareMode::Exclusive is a
    // follow-up). `sample_rate_follow` is wired here in Task 6 by
    // re-targeting the cpal stream AND the decoder resampler to
    // the new device's default rate when the toggle is on. The
    // route layer (Task 7) is the one that flips this toggle and
    // also re-issues `DeviceSwap` on track transitions when the
    // next track's native rate differs from the current stream
    // rate - runtime.rs has no view of the next track's StreamInfo
    // until decode begins, so it cannot drive that comparison
    // itself. Optional `desired_sample_rate` allows the route layer
    // to specify an exact target (e.g. next track's native rate).
    let new_device = match resolve_device(&selection) {
        Some(d) => d,
        None => {
            warn!("DeviceSwap: no output device available; keeping current output");
            return std::ops::ControlFlow::Continue(());
        }
    };
    let new_supported = match new_device.default_output_config() {
        Ok(s) => s,
        Err(err) => {
            warn!(
                "DeviceSwap: failed to read default config for new device: {err}; keeping current output"
            );
            return std::ops::ControlFlow::Continue(());
        }
    };
    let new_config = new_supported.config();
    let new_format = new_supported.sample_format();
    let new_name = device_display_name(&new_device);

    let has_live_engines = state.engine.is_some()
        || state.next_engine.is_some()
        || state.fading_out_engine.is_some()
        || state.drop_preview_engine.is_some();
    let desired_rate = device_swap_target_sample_rate(
        desired_sample_rate,
        sample_rate_follow,
        has_live_engines,
        state.device_sample_rate,
        new_config.sample_rate,
    );
    let requested_backend = if exclusive {
        SwapBackend::Exclusive
    } else {
        SwapBackend::Shared
    };
    let requested_plan = swap_stream_plan(&new_config, desired_rate, requested_backend);
    let mut actual_config = requested_plan.stream_config.clone();

    // In exclusive mode only one stream can hold the device, so
    // drop the pre-buffered + fading engines and only swap the
    // active one. Any in-flight crossfade is sacrificed at this
    // point; the user is intentionally trading multi-stream mixing
    // for bit-perfect output.
    // Rebuild the stream on every live engine so they all play on
    // the new device. swap_stream now transparently falls back to
    // cpal shared on exclusive failure (and emits an
    // ExclusiveModeFailed event), so a hard error here is rare -
    // typically only a cpal shared build failure.
    let mut swap_failed = false;
    if exclusive {
        #[cfg(target_os = "windows")]
        {
            for engine_slot in [
                state.engine.as_mut(),
                state.next_engine.as_mut(),
                state.fading_out_engine.as_mut(),
                state.drop_preview_engine.as_mut(),
            ]
            .into_iter()
            .flatten()
            {
                engine_slot.drop_stream();
                if let Some(target_rate) = requested_plan.target_sample_rate {
                    engine_slot
                        .shared
                        .target_sample_rate
                        .store(target_rate, Ordering::Relaxed);
                }
            }
            refresh_exclusive_sources(state);
            state.exclusive_sink.stream = None;
            match ensure_exclusive_sink_started(
                state,
                &new_device,
                &new_config,
                desired_rate,
                exclusive_release_grace_secs,
                exclusive_latency_mode.clone(),
                command_tx.clone(),
                event_tx.clone(),
            ) {
                Ok(actual_rate) => {
                    actual_config.sample_rate = actual_rate;
                }
                Err(err) => {
                    warn!("DeviceSwap: exclusive sink failed; falling back to shared: {err:?}");
                    swap_failed = true;
                    for engine_slot in [
                        state.engine.as_mut(),
                        state.next_engine.as_mut(),
                        state.fading_out_engine.as_mut(),
                        state.drop_preview_engine.as_mut(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        match engine_slot.swap_stream(
                            &new_device,
                            &new_config,
                            new_format,
                            command_tx.clone(),
                            event_tx.clone(),
                            false,
                            desired_rate,
                            exclusive_release_grace_secs,
                        ) {
                            Ok(actual_rate) => {
                                actual_config.sample_rate = actual_rate;
                            }
                            Err(err) => {
                                warn!(
                                    "DeviceSwap: failed to rebuild shared fallback for track {}: {err:?}",
                                    engine_slot.track_id
                                );
                            }
                        }
                    }
                }
            }
        }
    } else {
        #[cfg(target_os = "windows")]
        state.exclusive_sink.clear();

        for engine_slot in [
            state.engine.as_mut(),
            state.next_engine.as_mut(),
            state.fading_out_engine.as_mut(),
            state.drop_preview_engine.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            match engine_slot.swap_stream(
                &new_device,
                &new_config,
                new_format,
                command_tx.clone(),
                event_tx.clone(),
                false,
                desired_rate,
                exclusive_release_grace_secs,
            ) {
                Ok(actual_rate) => {
                    actual_config.sample_rate = actual_rate;
                }
                Err(err) => {
                    warn!(
                        "DeviceSwap: failed to rebuild stream for track {}: {err:?}",
                        engine_slot.track_id
                    );
                    swap_failed = true;
                }
            }
        }
    }

    if swap_failed {
        warn!("DeviceSwap: one or more engines failed to swap; output may be partial");
    }

    // Update the runtime's "current device" bindings so subsequent
    // Play / PrepareNext calls use the new device too. When
    // sample-rate-follow drove a rate change, also update the
    // runtime-wide `device_sample_rate` so freshly-cold-started
    // engines spin up at the new rate (their initial
    // `target_sample_rate` is seeded from this value).
    *device = new_device;
    *output_config = actual_config;
    *output_sample_format = new_format;
    state.device_name = new_name.clone();
    state.device_sample_rate = output_config.sample_rate;
    state.device_channels = output_config.channels;
    state.current_exclusive = exclusive;
    state.current_sample_rate_follow = sample_rate_follow;
    state.current_device_selection = selection;
    state.current_exclusive_release_grace_secs = exclusive_release_grace_secs;
    state.current_exclusive_latency_mode = exclusive_latency_mode;

    let _ = event_tx.send(PlaybackRuntimeEvent::Ready {
        device_name: new_name,
        sample_rate: state.device_sample_rate,
        channels: state.device_channels,
    });
    ControlFlow::Continue(())
}
