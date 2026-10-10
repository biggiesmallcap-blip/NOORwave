//! Switching the active engine to a new job and the advance cascade.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn transition_to_job(
    config: &PlaybackRuntimeConfig,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    device: &cpal::Device,
    output_config: &mut StreamConfig,
    output_sample_format: SampleFormat,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    state: &mut PlaybackRuntimeLoopState,
    job: PreparedPlaybackJob,
    volume_ctl: &Arc<AtomicU32>,
    position_samples: &Arc<AtomicU64>,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
    buffered_source: &Arc<Mutex<Arc<AtomicU64>>>,
    offset_source: &Arc<Mutex<Arc<AtomicU64>>>,
    force_restart: bool,
) -> Result<()> {
    // No-op when state.engine is already playing the requested track. This
    // happens after a crossfade swap: promote_next_to_active emitted Finished
    // for the OUTGOING track, which caused routes to call switch_to(NEW track)
    // - but we already promoted that engine. Re-doing the swap would tear down
    // a perfectly good audio stream and cold-start a duplicate.
    // position_source was already redirected to the promoted engine at promotion
    // time, so the handle reads the correct counter without any extra work here.
    if switch_is_noop_for_active_job(
        force_restart,
        state.engine.as_ref().map(|e| (e.track_id, e.generation)),
        job.track.id,
        job.generation,
    ) {
        return Ok(());
    }

    // Advance-cascade circuit breaker: see `evaluate_advance_cascade`.
    let breaker_tripped = evaluate_advance_cascade(state, event_tx);

    // Adopt the dispatching route's transport intent as the loop's latch;
    // a just-tripped breaker overrides it until an explicit Resume.
    state.user_paused = job.start_paused || breaker_tripped;
    // From here down every consumer reads the job, so bake the effective
    // intent back in - the cold-start path hands the job to the engine and
    // the engine honors `start_paused` at construction.
    let mut job = job;
    job.start_paused = state.user_paused;

    // Retire the outgoing deck with a ramp rather than a cut: stopping an
    // audible engine resets its buffer mid-waveform and steps the output
    // straight to silence, which is the pop heard on skip.
    //
    // A user-initiated track change (skip / new play) also abandons any
    // in-flight crossfade - the fading-out engine has to go too, or it keeps
    // producing audio underneath the new track. Both retire in one batch so
    // they share a single fade window instead of serializing.
    state.dj.prepared_mixer = None;
    state.dj.prepared_drop_preview_mixer = None;
    let mut retiring: Vec<PlaybackEngine> = Vec::new();
    retiring.extend(state.engine.take());
    retiring.extend(state.fading_out_engine.take());
    fade_out_and_stop(retiring);

    // Reset position counter to the new engine's offset baseline (option C:
    // a segment-restart job seeds from `start_from_offset_ms` so the handle's
    // `get_position_ms` reports the correct absolute time from the first
    // CPAL callback, before the engine has actually written any samples).
    // `start_decoder_only` later stores the same value into position_samples
    // - this preemptive store is so the handle doesn't briefly read a stale 0
    // (or a stale prior-track value) between the engine teardown above and
    // the engine spawn below.
    let baseline_offset_samples = (job
        .start_from_offset_ms
        .saturating_mul(state.device_sample_rate as u64)
        .saturating_mul(state.device_channels.max(1) as u64))
        / 1000;
    position_samples.store(baseline_offset_samples, Ordering::SeqCst);

    let output_state_update = transition_output_state_update(
        job.output_sample_rate,
        state.current_sample_rate_follow,
        state.device_sample_rate,
    );
    if let Some(update) = output_state_update {
        output_config.sample_rate = update.sample_rate;
        state.device_sample_rate = update.sample_rate;
        #[cfg(target_os = "windows")]
        if update.force_exclusive_rebuild && state.current_exclusive {
            state.exclusive_sink.stream = None;
        }
    }

    let _ = event_tx.send(PlaybackRuntimeEvent::Preparing {
        track_id: job.track.id,
        source: job.source_kind(),
    });

    // Check if the next track was pre-buffered (gapless pre-decode).
    let pre_decoded_match = !force_restart
        && state
            .next_engine
            .as_ref()
            .map(|e| {
                e.track_id == job.track.id
                    && e.generation == job.generation
                    && prepared_engine_matches_output_rate(
                        e.shared.device_sample_rate,
                        job.output_sample_rate,
                        state.current_sample_rate_follow,
                    )
            })
            .unwrap_or(false);

    if pre_decoded_match {
        adopt_predecoded_next_engine(state, position_source, buffered_source, offset_source);
        #[cfg(target_os = "windows")]
        if state.current_exclusive {
            refresh_exclusive_sources(state);
        }
    } else {
        // Cold start - stop any stale next_engine.
        if let Some(mut stale) = state.next_engine.take() {
            stale.stop();
        }
        if state.current_exclusive {
            let eng = PlaybackEngine::start_decoder_only(
                config,
                command_tx,
                job,
                state.device_sample_rate,
                state.device_channels,
                Arc::clone(volume_ctl),
                Arc::clone(position_samples),
            )?;
            *position_source.lock().unwrap() = Arc::clone(&eng.shared.source_position_samples);
            *buffered_source.lock().unwrap() = Arc::clone(&eng.shared.buffered_samples);
            *offset_source.lock().unwrap() = Arc::clone(&eng.shared.source_offset_samples);
            *state.handoff_elapsed_source.lock().unwrap() =
                Arc::clone(&eng.shared.handoff_elapsed_samples);
            state.engine = Some(eng);

            #[cfg(target_os = "windows")]
            {
                refresh_exclusive_sources(state);
                match ensure_exclusive_sink_started(
                    state,
                    device,
                    output_config,
                    exclusive_rebuild_rate(
                        state.current_sample_rate_follow,
                        state.device_sample_rate,
                    ),
                    state.current_exclusive_release_grace_secs,
                    state.current_exclusive_latency_mode.clone(),
                    command_tx.clone(),
                    event_tx.clone(),
                ) {
                    Ok(actual_rate) => {
                        output_config.sample_rate = actual_rate;
                        state.device_sample_rate = actual_rate;
                    }
                    Err(err) => {
                        warn!(
                            "transition_to_job: exclusive sink failed; falling back to shared: {err:?}"
                        );
                        if let Some(engine) = state.engine.as_mut() {
                            let actual_rate = engine.swap_stream(
                                device,
                                output_config,
                                output_sample_format,
                                command_tx.clone(),
                                event_tx.clone(),
                                false,
                                exclusive_rebuild_rate(
                                    state.current_sample_rate_follow,
                                    state.device_sample_rate,
                                ),
                                state.current_exclusive_release_grace_secs,
                            )?;
                            output_config.sample_rate = actual_rate;
                            state.device_sample_rate = actual_rate;
                        }
                    }
                }
            }
        } else {
            let eng = PlaybackEngine::start(
                config,
                command_tx,
                device,
                output_config,
                output_sample_format,
                job,
                event_tx.clone(),
                state.device_sample_rate,
                state.device_channels,
                Arc::clone(volume_ctl),
                Arc::clone(position_samples),
            )?;
            let actual_start_rate = eng.shared.device_sample_rate;
            output_config.sample_rate = actual_start_rate;
            state.device_sample_rate = actual_start_rate;
            *position_source.lock().unwrap() = Arc::clone(&eng.shared.source_position_samples);
            *buffered_source.lock().unwrap() = Arc::clone(&eng.shared.buffered_samples);
            *offset_source.lock().unwrap() = Arc::clone(&eng.shared.source_offset_samples);
            *state.handoff_elapsed_source.lock().unwrap() =
                Arc::clone(&eng.shared.handoff_elapsed_samples);
            state.engine = Some(eng);
        }
    }
    if output_state_update.is_some_and(|update| update.notify_ready) {
        let _ = event_tx.send(PlaybackRuntimeEvent::Ready {
            device_name: state.device_name.clone(),
            sample_rate: state.device_sample_rate,
            channels: state.device_channels,
        });
    }
    Ok(())
}

pub(super) fn switch_is_noop_for_active_job(
    force_restart: bool,
    active: Option<(i64, u64)>,
    track_id: i64,
    generation: u64,
) -> bool {
    !force_restart && active == Some((track_id, generation))
}

/// A direct selection can consume a prepared next deck without going through
/// promotion. Bind every public reader to that deck before exposing it.
pub(super) fn adopt_predecoded_next_engine(
    state: &mut PlaybackRuntimeLoopState,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
    buffered_source: &Arc<Mutex<Arc<AtomicU64>>>,
    offset_source: &Arc<Mutex<Arc<AtomicU64>>>,
) {
    let pre = state
        .next_engine
        .take()
        .expect("matching prepared next engine");
    pre.shared.paused.store(state.user_paused, Ordering::SeqCst);
    *position_source.lock().unwrap() = Arc::clone(&pre.shared.source_position_samples);
    *buffered_source.lock().unwrap() = Arc::clone(&pre.shared.buffered_samples);
    *offset_source.lock().unwrap() = Arc::clone(&pre.shared.source_offset_samples);
    *state.handoff_elapsed_source.lock().unwrap() = Arc::clone(&pre.shared.handoff_elapsed_samples);
    state.engine = Some(pre);
}

/// Advance-cascade circuit breaker, evaluated as `transition_to_job` is about
/// to tear down the outgoing deck. If that deck lived past
/// `SILENT_ENGINE_FAILURE_MIN_AGE` yet never produced a single audible
/// sample, count it toward the streak; `MAX_SILENT_START_STREAK` in a row
/// means upstream streaming is down and hot-advancing further would burn
/// through the whole queue 15-25s at a time (the old restart-the-server
/// state). Returns `true` when the breaker trips: the caller latches pause so
/// the incoming engine comes up silent, one clear error event is emitted, and
/// an explicit Resume retries. A deck that DID make sound resets the streak;
/// decks torn down young (rapid manual skips) leave it untouched.
pub(super) fn evaluate_advance_cascade(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> bool {
    let outgoing_started = state.engine.as_ref().map(|engine| {
        engine
            .shared
            .buffer
            .lock()
            .map(|guard| guard.started)
            .unwrap_or(false)
    });
    match outgoing_started {
        Some(true) => {
            state.silent_start_streak = 0;
            false
        }
        Some(false)
            if state
                .engine
                .as_ref()
                .is_some_and(|e| e.created_at.elapsed() >= SILENT_ENGINE_FAILURE_MIN_AGE) =>
        {
            state.silent_start_streak = state.silent_start_streak.saturating_add(1);
            if state.silent_start_streak >= MAX_SILENT_START_STREAK && !state.user_paused {
                warn!(
                    "Advance-cascade breaker: {} consecutive decks made no audio; latching pause instead of burning the queue",
                    state.silent_start_streak
                );
                let _ = event_tx.send(PlaybackRuntimeEvent::Error {
                    message: "Playback paused: several tracks in a row produced no audio (stream source stalled). Press play to retry.".to_string(),
                });
                // Paused event so the DB/UI reconcile to a truthful paused
                // state instead of claiming playback that is not happening.
                let _ = event_tx.send(PlaybackRuntimeEvent::Paused { track_id: None });
                state.silent_start_streak = 0;
                return true;
            }
            false
        }
        _ => false,
    }
}
