//! Crossfade readiness, next-deck promotion and output-rate decisions at track boundaries.

use super::*;

/// Promote the pre-decoded `next_engine` to be the new active engine. The
/// previously-active engine is moved to `fading_out_engine` where it keeps
/// producing audio with a fade-out gain ramp until its buffer drains; the new
/// engine starts immediately with a fade-in ramp.
///
/// We also broadcast a `Finished` event for the OUTGOING track so that the
/// routes layer advances the queue (updates `playback_state.current_track_id`
/// and fires the WebSocket `TrackChanged` event) at the audible-swap moment,
/// not when the fade-out finally drains. The corresponding `Switch` command
/// that comes back through the runtime is intentionally a no-op now, because
/// `transition_to_job` short-circuits when `state.engine` is already playing
/// the requested track.
/// Whether the incoming deck has buffered enough to be promoted at the
/// crossfade boundary. `is_ready()` only guarantees the ~500ms start threshold,
/// far short of a multi-second fade. Promoting a deck that holds less than the
/// fade window forces it to out-decode the fade in real time; on a slow TIDAL
/// connection it can't and starves mid-fade after the queue has already
/// advanced at promotion time (the StallTracker watchdog eventually
/// force-skips, but only after ACTIVE_STALL_RECOVERY_SECS of silence). Wait
/// for the whole fade window plus a small margin -- or a fully decoded deck --
/// before promoting. If the deck isn't there yet the caller skips the early
/// fade; the boundary path hard-cuts when the track actually ends, which is a
/// clean transition instead of a silent stall.
pub(super) fn crossfade_next_ready(
    base_ready: bool,
    finished: bool,
    unread_samples: u64,
    crossfade_samples: u64,
) -> bool {
    if finished {
        return true;
    }
    if !base_ready {
        return false;
    }
    let margin = crossfade_samples / 8;
    unread_samples >= crossfade_samples.saturating_add(margin)
}

pub(super) fn adaptive_rhythmic_transition(
    state: &PlaybackRuntimeLoopState,
) -> Option<&PreparedTransitionProgram> {
    let active = state.engine.as_ref()?;
    let incoming = state.next_engine.as_ref()?;
    let transition = incoming.job.prepared_transition.as_ref()?;
    let lookahead = state.dj.lookahead.as_ref()?;
    (state.dj.engine_enabled
        && beat_sync::required(&transition.program)
        && lookahead.matches_pair(
            transition.queue_generation,
            transition.current_queue_item_id,
            transition.next_queue_item_id,
        )
        && lookahead
            .current
            .as_ref()
            .and_then(DjMediaRef::track_id)
            .is_none_or(|id| id == active.track_id)
        && lookahead
            .next
            .track_id()
            .is_none_or(|id| id == incoming.track_id)
        && active.shared.target_sample_rate.load(Ordering::Relaxed) == state.device_sample_rate
        && incoming.shared.target_sample_rate.load(Ordering::Relaxed) == state.device_sample_rate
        && incoming.shared.device_channels == state.device_channels)
        .then_some(transition)
}

pub(super) fn adaptive_next_required_samples(
    state: &PlaybackRuntimeLoopState,
    buffer: CrossfadeReadinessSnapshot,
) -> Option<u64> {
    let transition = adaptive_rhythmic_transition(state)?;
    if prepared_dj_mixer_matches_pair(state) {
        let prepared = state.dj.prepared_mixer.as_ref()?;
        if handoff_mixer_program(&prepared.program) {
            let channels = u64::from(state.device_channels.max(1));
            let cue = prepared.program.deck_b_start_frame.saturating_mul(channels);
            if cue < buffer.read_samples {
                return Some(u64::MAX);
            }
            // Verification has already completed. Install the actual rendered
            // programme, including a shorter protected replacement, using its
            // corrected consumption and continuation rather than the original
            // plan's larger analysis budget.
            let frames = deck_b_consumed_frames(&prepared.program)?
                .saturating_add(2)
                .saturating_add(u64::from(state.device_sample_rate.max(1)) / 2);
            return Some(
                cue.saturating_add(frames.saturating_mul(channels))
                    .saturating_sub(buffer.read_samples),
            );
        }
    }
    let program = transition
        .program
        .clone()
        .rescaled_to(state.device_sample_rate.max(1));
    let beats = f64::from(program.decision.as_ref()?.duration_beats);
    if !beats.is_finite() || beats <= 0.0 {
        return None;
    }
    let channels = u64::from(state.device_channels.max(1));
    let cue = program.deck_b_start_frame.saturating_mul(channels);
    if cue < buffer.offset_samples {
        return Some(u64::MAX);
    }
    // Bound the whole decoded beat-fit window and any verified rate/cue
    // correction, then retain half a second of source continuation. The
    // outgoing countdown length is not the incoming programme's PCM demand.
    let verification_frames = |candidate: &noor_mix::TransitionProgram| {
        let beats = f64::from(candidate.decision.as_ref().unwrap().duration_beats);
        (candidate.resolve_at as f64 * 1.03
            + candidate.resolve_at as f64 / beats * 1.06
            + f64::from(state.device_sample_rate.max(1)) * 0.5)
            .ceil() as u64
            + 1
    };
    // Eligible shorter bass phrases are independently verified at build
    // time. Requiring the rejected long analysis budget would prevent them
    // from ever preparing while the next decoder is below that watermark.
    let frames = beat_sync::musical_prefixes(&program)
        .iter()
        .map(verification_frames)
        .fold(verification_frames(&program), u64::min);
    Some(
        cue.saturating_add(frames.saturating_mul(channels))
            .saturating_sub(buffer.offset_samples.saturating_add(buffer.read_samples)),
    )
}

pub(super) fn dj_crossfade_next_ready(
    state: &PlaybackRuntimeLoopState,
    buffer: CrossfadeReadinessSnapshot,
    crossfade_samples: u64,
) -> bool {
    match adaptive_next_required_samples(state, buffer) {
        Some(u64::MAX) => false,
        Some(required) => buffer.finished || buffer.base_ready && buffer.unread_samples >= required,
        None => crossfade_next_ready(
            buffer.base_ready,
            buffer.finished,
            buffer.unread_samples,
            crossfade_samples,
        ),
    }
}

pub(super) fn promote_next_to_active(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
    buffered_source: &Arc<Mutex<Arc<AtomicU64>>>,
    offset_source: &Arc<Mutex<Arc<AtomicU64>>>,
    timing_status: &'static str,
    actual_start_ms_override: Option<i64>,
    runtime_planned_start_ms: Option<i64>,
    runtime_renderer: DjRuntimeRendererOutcome,
) {
    state.dj.prepared_mixer = None;
    let Some(next) = state.next_engine.take() else {
        return;
    };
    let transition_event_id = next
        .job
        .prepared_transition
        .as_ref()
        .and_then(|transition| transition.transition_event_id);
    let runtime_program_json = (runtime_renderer.rendered
        || runtime_renderer.reason == DjRuntimeRendererReason::ProtectedHandoffCut)
        .then(|| {
            next.job
                .prepared_transition
                .as_ref()
                .and_then(|transition| serde_json::to_string(&transition.program).ok())
        })
        .flatten();
    if runtime_renderer.rendered {
        next.shared.crossfade_samples.store(0, Ordering::Relaxed);
        next.shared
            .fadein_start_samples
            .store(u64::MAX, Ordering::Relaxed);
    } else {
        let fadein_start =
            if runtime_renderer.reason == DjRuntimeRendererReason::ProtectedHandoffCut {
                next.shared.position_samples.load(Ordering::Relaxed)
            } else {
                0
            };
        next.shared
            .fadein_start_samples
            .store(fadein_start, Ordering::Relaxed);
    }
    // Honor the user-pause latch: crossfade promotion must not un-pause a
    // deck behind the user's back (the paused-button-but-audio-playing bug).
    next.shared
        .paused
        .store(state.user_paused, Ordering::SeqCst);

    // Redirect the handle's position + buffered + offset readers to the
    // incoming engine's counters BEFORE sliding it into state.engine so
    // get_position_ms / get_buffered_ms / get_buffered_start_ms() immediately
    // reflect the new track starting from 0 instead of the fading-out track's
    // frozen end values.
    //
    // If lock() panics (the only failure mode is a prior poisoning) the
    // moved-out `next` is dropped without stop() being called and its decoder
    // thread keeps fetching until natural EOF or the CDN timeout (~30s
    // bounded). Task 7's catch_unwind around the dispatch loop catches the
    // panic and emits Error+Stopped. The "preserve frozen-position UX" win
    // was judged to outweigh the rare-poisoning bandwidth blip.
    *position_source.lock().unwrap() = Arc::clone(&next.shared.source_position_samples);
    *buffered_source.lock().unwrap() = Arc::clone(&next.shared.buffered_samples);
    *offset_source.lock().unwrap() = Arc::clone(&next.shared.source_offset_samples);
    *state.handoff_elapsed_source.lock().unwrap() =
        Arc::clone(&next.shared.handoff_elapsed_samples);

    let outgoing = state.engine.take();
    state.engine = Some(next);

    // Drop any prior fading-out engine first so we never accumulate more than
    // one (the previous one would have been audibly silent by now anyway).
    if let Some(mut prior) = state.fading_out_engine.take() {
        prior.stop();
    }
    if let Some(outgoing) = outgoing {
        let outgoing_id = outgoing.track_id;
        let outgoing_generation = outgoing.generation;
        let actual_start_ms = actual_start_ms_override.unwrap_or_else(|| {
            track_position_ms(
                &outgoing.shared,
                state.device_sample_rate,
                state.device_channels,
            )
        });
        if runtime_renderer.rendered {
            // The installed mix carries this track's own continuation, so the
            // live copy must leave over the short seam ramp, not a hard cut.
            // It then drains silently to its natural end and TrackTerminal
            // reaps it from the fading slot, same as the legacy path.
            outgoing.shared.dj_fadeout_start_samples.store(
                outgoing.shared.position_samples.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }
        state.fading_out_engine = Some(outgoing);
        if let Some(transition_event_id) = transition_event_id {
            info!(
                transition_event_id,
                outgoing_track_id = outgoing_id,
                generation = outgoing_generation,
                actual_start_ms,
                runtime_planned_start_ms,
                timing_status,
                rendered_dj_mixer = runtime_renderer.rendered,
                runtime_renderer_status = runtime_renderer.status.as_str(),
                runtime_renderer_reason = runtime_renderer.reason.as_str(),
                "DJ transition promotion fired"
            );
            let _ = event_tx.send(PlaybackRuntimeEvent::DjTransitionPromoted {
                transition_event_id,
                outgoing_track_id: outgoing_id,
                generation: outgoing_generation,
                actual_start_ms,
                runtime_planned_start_ms,
                timing_status: timing_status.to_string(),
                runtime_rendered_dj_mixer: runtime_renderer.rendered,
                runtime_renderer_status: runtime_renderer.status.as_str().to_string(),
                runtime_renderer_reason: runtime_renderer.reason.as_str().to_string(),
                runtime_program_json,
            });
        }
        // Tell the routes layer that the audible "current" track has flipped.
        // Reusing Finished keeps the existing queue-advance path.
        let _ = event_tx.send(PlaybackRuntimeEvent::Finished {
            track_id: outgoing_id,
            generation: outgoing_generation,
        });
    }
    #[cfg(target_os = "windows")]
    if state.current_exclusive {
        refresh_exclusive_sources(state);
    }
}

pub(super) fn track_position_ms(
    shared: &PlaybackSharedState,
    sample_rate: u32,
    channels: u16,
) -> i64 {
    let samples = shared.output_to_source_samples(shared.position_samples.load(Ordering::Relaxed));
    samples_to_ms(samples, sample_rate, channels)
}

pub(super) fn samples_to_ms(samples: u64, sample_rate: u32, channels: u16) -> i64 {
    if sample_rate == 0 || channels == 0 {
        return 0;
    }
    (samples.saturating_mul(1000) / (u64::from(sample_rate) * u64::from(channels))) as i64
}

pub(super) fn promote_prepared_at_boundary(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
    buffered_source: &Arc<Mutex<Arc<AtomicU64>>>,
    offset_source: &Arc<Mutex<Arc<AtomicU64>>>,
) {
    let runtime_renderer = DjRuntimeRendererOutcome::boundary_fallback(
        runtime_renderer_boundary_fallback_reason(state),
    );
    state.dj.prepared_mixer = None;
    let Some(next) = state.next_engine.take() else {
        return;
    };
    let transition_event_id = next
        .job
        .prepared_transition
        .as_ref()
        .and_then(|transition| transition.transition_event_id);
    next.shared
        .fadein_start_samples
        .store(u64::MAX, Ordering::Relaxed);
    // Honor the user-pause latch: boundary promotion never un-pauses on its own.
    next.shared
        .paused
        .store(state.user_paused, Ordering::SeqCst);
    *position_source.lock().unwrap() = Arc::clone(&next.shared.source_position_samples);
    *buffered_source.lock().unwrap() = Arc::clone(&next.shared.buffered_samples);
    *offset_source.lock().unwrap() = Arc::clone(&next.shared.source_offset_samples);
    *state.handoff_elapsed_source.lock().unwrap() =
        Arc::clone(&next.shared.handoff_elapsed_samples);

    let outgoing = state.engine.take();
    state.engine = Some(next);

    if let Some(mut prior) = state.fading_out_engine.take() {
        prior.stop();
    }
    if let Some(mut outgoing) = outgoing {
        let outgoing_id = outgoing.track_id;
        let outgoing_generation = outgoing.generation;
        let boundary_handoff_ms = track_position_ms(
            &outgoing.shared,
            state.device_sample_rate,
            state.device_channels,
        );
        outgoing.stop();
        if let Some(transition_event_id) = transition_event_id {
            info!(
                transition_event_id,
                outgoing_track_id = outgoing_id,
                generation = outgoing_generation,
                boundary_handoff_ms,
                timing_status = "missed",
                runtime_renderer_status = runtime_renderer.status.as_str(),
                runtime_renderer_reason = runtime_renderer.reason.as_str(),
                "DJ transition boundary fallback missed planned fire"
            );
            let _ = event_tx.send(PlaybackRuntimeEvent::DjTransitionPromoted {
                transition_event_id,
                outgoing_track_id: outgoing_id,
                generation: outgoing_generation,
                actual_start_ms: boundary_handoff_ms,
                runtime_planned_start_ms: None,
                timing_status: "missed".to_string(),
                runtime_program_json: None,
                runtime_rendered_dj_mixer: false,
                runtime_renderer_status: runtime_renderer.status.as_str().to_string(),
                runtime_renderer_reason: runtime_renderer.reason.as_str().to_string(),
            });
        }
        let _ = event_tx.send(PlaybackRuntimeEvent::Finished {
            track_id: outgoing_id,
            generation: outgoing_generation,
        });
    }
    #[cfg(target_os = "windows")]
    if state.current_exclusive {
        refresh_exclusive_sources(state);
    }
}

pub(super) fn exclusive_rebuild_rate(
    sample_rate_follow: bool,
    device_sample_rate: u32,
) -> Option<u32> {
    sample_rate_follow.then_some(device_sample_rate)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputStateUpdate {
    pub(super) sample_rate: u32,
    pub(super) force_exclusive_rebuild: bool,
    pub(super) notify_ready: bool,
}

pub(super) fn device_swap_target_sample_rate(
    requested_sample_rate: Option<u32>,
    sample_rate_follow: bool,
    has_live_engines: bool,
    current_sample_rate: u32,
    device_default_sample_rate: u32,
) -> Option<u32> {
    if let Some(rate) = requested_sample_rate {
        return Some(rate);
    }
    if has_live_engines {
        return Some(current_sample_rate);
    }
    sample_rate_follow.then_some(device_default_sample_rate)
}

pub(super) fn transition_output_state_update(
    job_sample_rate: Option<u32>,
    sample_rate_follow: bool,
    current_sample_rate: u32,
) -> Option<OutputStateUpdate> {
    transition_output_sample_rate(job_sample_rate, sample_rate_follow, current_sample_rate).map(
        |sample_rate| OutputStateUpdate {
            sample_rate,
            force_exclusive_rebuild: true,
            notify_ready: true,
        },
    )
}

pub(super) fn transition_output_sample_rate(
    job_sample_rate: Option<u32>,
    sample_rate_follow: bool,
    current_sample_rate: u32,
) -> Option<u32> {
    if !sample_rate_follow {
        return None;
    }
    job_sample_rate.filter(|rate| *rate > 0 && *rate != current_sample_rate)
}

pub(super) fn prepared_engine_matches_output_rate(
    engine_sample_rate: u32,
    job_sample_rate: Option<u32>,
    sample_rate_follow: bool,
) -> bool {
    !sample_rate_follow
        || job_sample_rate
            .map(|rate| rate == engine_sample_rate)
            .unwrap_or(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalEngineSlot {
    Active,
    Next,
    FadingOut,
    DropPreview,
}

pub(super) fn terminal_engine_slot(
    active: Option<(i64, u64)>,
    next: Option<(i64, u64)>,
    fading: Option<(i64, u64)>,
    drop_preview: Option<(i64, u64)>,
    track_id: i64,
    generation: u64,
) -> Option<TerminalEngineSlot> {
    let target = Some((track_id, generation));
    if fading == target {
        Some(TerminalEngineSlot::FadingOut)
    } else if drop_preview == target {
        Some(TerminalEngineSlot::DropPreview)
    } else if next == target {
        Some(TerminalEngineSlot::Next)
    } else if active == target {
        Some(TerminalEngineSlot::Active)
    } else {
        None
    }
}

pub(super) fn should_promote_prepared_at_boundary(
    active: Option<(i64, u64)>,
    next: Option<(i64, u64)>,
    track_id: i64,
    generation: u64,
    outcome: &PlaybackTerminalReason,
) -> bool {
    matches!(outcome, PlaybackTerminalReason::Finished)
        && active == Some((track_id, generation))
        && next.is_some_and(|(_, next_generation)| next_generation == generation)
}

pub(super) fn active_engine_suppresses_crossfade_after_seek(
    state: &PlaybackRuntimeLoopState,
) -> bool {
    state.engine.as_ref().is_some_and(|engine| {
        engine
            .shared
            .suppress_crossfade_after_seek
            .load(Ordering::Relaxed)
    })
}
