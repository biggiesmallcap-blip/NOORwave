//! DJ transition preparation for the runtime loop: lookahead, prepared mixers
//! (handoff, overlay, drop preview), renderer failures and PCM readiness.
//! A child of the runtime module, so it shares the loop state and decks.

use super::*;

pub(super) struct PreparedDjMixer {
    pub(super) program: noor_mix::TransitionProgram,
    pub(super) max_block_samples: usize,
    // Absolute output-clock origin survives decoder buffer compaction. The
    // program's start frame remains buffer-local for the captured Mixer deck.
    pub(super) deck_a_output_start_frame: u64,
    /// The full transition mix, rendered at build (prepare/decode-complete)
    /// time rather than at fire time. Rendering 8-28s of dual-deck audio
    /// takes long enough that doing it inside the fire handler used to let
    /// deck A advance past the snapshot the render was built from, so the
    /// handoff audibly repeated ~100-300ms of the outgoing track. At install
    /// the buffer is joined by skipping however far deck A actually moved.
    pub(super) rendered: Vec<f32>,
    pub(super) current_track_id: i64,
    pub(super) next_track_id: i64,
}

pub(super) struct RuntimeDeckSnapshot {
    pub(super) samples: Vec<f32>,
    pub(super) start_frame: u64,
    pub(super) output_start_frame: u64,
}

#[derive(Clone, Copy)]
pub(super) struct CrossfadeReadinessSnapshot {
    pub(super) base_ready: bool,
    pub(super) finished: bool,
    pub(super) unread_samples: u64,
    pub(super) decoded_samples: u64,
    pub(super) read_samples: u64,
    pub(super) offset_samples: u64,
    pub(super) start_threshold_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeDjLookahead {
    pub(super) current: Option<DjMediaRef>,
    pub(super) next: DjMediaRef,
    pub(super) current_queue_item_id: Option<i64>,
    pub(super) next_queue_item_id: i64,
    pub(super) queue_generation: u64,
    pub(super) deadline_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DjLookaheadFailure {
    pub(super) queue_generation: u64,
    pub(super) current_queue_item_id: Option<i64>,
    pub(super) next_queue_item_id: Option<i64>,
    pub(super) reason: DjLookaheadFailureReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DjRuntimeRendererFailure {
    pub(super) queue_generation: u64,
    pub(super) current_queue_item_id: Option<i64>,
    pub(super) next_queue_item_id: Option<i64>,
    pub(super) transition_event_id: Option<i64>,
    pub(super) current_track_id: Option<i64>,
    pub(super) next_track_id: Option<i64>,
    pub(super) current_engine_generation: Option<u64>,
    pub(super) next_engine_generation: Option<u64>,
    pub(super) reason: DjRuntimeRendererReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DjRuntimeRendererStatus {
    RenderedHandoff,
    RenderedOverlay,
    LegacyOverlap,
    BoundaryFallback,
}

impl DjRuntimeRendererStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::RenderedHandoff => "rendered_handoff",
            Self::RenderedOverlay => "rendered_overlay",
            Self::LegacyOverlap => "legacy_overlap",
            Self::BoundaryFallback => "boundary_fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DjRuntimeRendererReason {
    None,
    PreparedMixerMissing,
    LookaheadPairMismatch,
    ProgramNotMixerRenderable,
    ActiveDeckNotDecoded,
    NextDeckNotDecoded,
    MixerRejected,
    ActiveTrackChanged,
    NextTrackChanged,
    RenderBufferFailed,
    BufferLockFailed,
    DjDisabled,
    NextDecodeLateAtFire,
    NextDeckMissingAtFire,
    TransitionPlanMissingAtFire,
    SyncWindowNotSignaled,
    ManualSeekSuppressed,
    /// The live deck A playhead is already past the midpoint of the rendered
    /// transition, so joining it would play only the tail of the blend.
    HandoffSeamTooLate,
    /// A late protected overlap could not be rebuilt; the live decks use
    /// the short seam ramps instead of reviving an unverified long overlap.
    ProtectedHandoffCut,
}

impl DjRuntimeRendererReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PreparedMixerMissing => "prepared_mixer_missing",
            Self::LookaheadPairMismatch => "lookahead_pair_mismatch",
            Self::ProgramNotMixerRenderable => "program_not_mixer_renderable",
            Self::ActiveDeckNotDecoded => "active_deck_not_decoded",
            Self::NextDeckNotDecoded => "next_deck_not_decoded",
            Self::MixerRejected => "mixer_rejected",
            Self::ActiveTrackChanged => "active_track_changed",
            Self::NextTrackChanged => "next_track_changed",
            Self::RenderBufferFailed => "render_buffer_failed",
            Self::BufferLockFailed => "buffer_lock_failed",
            Self::DjDisabled => "dj_disabled",
            Self::NextDecodeLateAtFire => "next_decode_late_at_fire",
            Self::NextDeckMissingAtFire => "next_deck_missing_at_fire",
            Self::TransitionPlanMissingAtFire => "transition_plan_missing_at_fire",
            Self::SyncWindowNotSignaled => "sync_window_not_signaled",
            Self::ManualSeekSuppressed => "manual_seek_suppressed",
            Self::HandoffSeamTooLate => "handoff_seam_too_late",
            Self::ProtectedHandoffCut => "protected_handoff_cut",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DjRuntimeRendererOutcome {
    pub(super) rendered: bool,
    pub(super) status: DjRuntimeRendererStatus,
    pub(super) reason: DjRuntimeRendererReason,
}

impl DjRuntimeRendererOutcome {
    pub(super) fn rendered_handoff() -> Self {
        Self {
            rendered: true,
            status: DjRuntimeRendererStatus::RenderedHandoff,
            reason: DjRuntimeRendererReason::None,
        }
    }

    pub(super) fn rendered_handoff_with_reason(reason: DjRuntimeRendererReason) -> Self {
        Self {
            rendered: true,
            status: DjRuntimeRendererStatus::RenderedHandoff,
            reason,
        }
    }

    pub(super) fn rendered_overlay() -> Self {
        Self {
            rendered: true,
            status: DjRuntimeRendererStatus::RenderedOverlay,
            reason: DjRuntimeRendererReason::None,
        }
    }

    pub(super) fn legacy_overlap(reason: DjRuntimeRendererReason) -> Self {
        Self {
            rendered: false,
            status: DjRuntimeRendererStatus::LegacyOverlap,
            reason,
        }
    }

    pub(super) fn boundary_fallback(reason: DjRuntimeRendererReason) -> Self {
        Self {
            rendered: false,
            status: DjRuntimeRendererStatus::BoundaryFallback,
            reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum DjLookaheadFailureReason {
    NextNotResolved,
    ResolutionFailed,
    AnalysisDeadlineMissed,
    QueueChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartDjLookaheadOutcome {
    Started,
    AlreadyCurrent,
    ReusedPreparedNext,
    MissingNext,
}

impl RuntimeDjLookahead {
    pub(super) fn matches_pair(
        &self,
        queue_generation: u64,
        current_queue_item_id: Option<i64>,
        next_queue_item_id: Option<i64>,
    ) -> bool {
        self.queue_generation == queue_generation
            && self.current_queue_item_id == current_queue_item_id
            && Some(self.next_queue_item_id) == next_queue_item_id
    }
}

pub(super) fn runtime_renderer_failure_reason(
    state: &PlaybackRuntimeLoopState,
    reason: DjRuntimeRendererReason,
) -> DjRuntimeRendererReason {
    if reason != DjRuntimeRendererReason::PreparedMixerMissing {
        return reason;
    }
    let Some(failure) = state.last_dj_renderer_failure else {
        return DjRuntimeRendererReason::PreparedMixerMissing;
    };
    if renderer_failure_matches_current_transition(state, failure) {
        failure.reason
    } else {
        DjRuntimeRendererReason::PreparedMixerMissing
    }
}

pub(super) fn runtime_renderer_fire_block_reason(
    state: &PlaybackRuntimeLoopState,
    next_ready: bool,
) -> DjRuntimeRendererReason {
    let Some(next) = state.next_engine.as_ref() else {
        return DjRuntimeRendererReason::NextDeckMissingAtFire;
    };
    if next.job.prepared_transition.is_none() {
        return DjRuntimeRendererReason::TransitionPlanMissingAtFire;
    }
    if !next_ready {
        return DjRuntimeRendererReason::NextDecodeLateAtFire;
    }
    DjRuntimeRendererReason::PreparedMixerMissing
}

pub(super) fn runtime_renderer_boundary_fallback_reason(
    state: &PlaybackRuntimeLoopState,
) -> DjRuntimeRendererReason {
    if active_engine_suppresses_crossfade_after_seek(state) {
        return DjRuntimeRendererReason::ManualSeekSuppressed;
    }
    let reason =
        runtime_renderer_failure_reason(state, DjRuntimeRendererReason::PreparedMixerMissing);
    if reason != DjRuntimeRendererReason::PreparedMixerMissing {
        return reason;
    }
    let crossfade_signaled = state
        .engine
        .as_ref()
        .map(|engine| {
            engine
                .shared
                .crossfade_start_signaled
                .load(Ordering::Relaxed)
        })
        .unwrap_or(false);
    if crossfade_signaled {
        DjRuntimeRendererReason::PreparedMixerMissing
    } else {
        DjRuntimeRendererReason::SyncWindowNotSignaled
    }
}

pub(super) fn runtime_renderer_late_fire_reason(
    state: &PlaybackRuntimeLoopState,
) -> DjRuntimeRendererReason {
    let reason =
        runtime_renderer_failure_reason(state, DjRuntimeRendererReason::PreparedMixerMissing);
    if reason == DjRuntimeRendererReason::PreparedMixerMissing {
        DjRuntimeRendererReason::NextDecodeLateAtFire
    } else {
        reason
    }
}

pub(super) fn record_runtime_renderer_failure(
    state: &mut PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    reason: DjRuntimeRendererReason,
) {
    let failure = DjRuntimeRendererFailure {
        queue_generation: transition.queue_generation,
        current_queue_item_id: transition.current_queue_item_id,
        next_queue_item_id: transition.next_queue_item_id,
        transition_event_id: transition.transition_event_id,
        current_track_id: state.engine.as_ref().map(|engine| engine.track_id),
        next_track_id: state.next_engine.as_ref().map(|engine| engine.track_id),
        current_engine_generation: state.engine.as_ref().map(|engine| engine.generation),
        next_engine_generation: state.next_engine.as_ref().map(|engine| engine.generation),
        reason,
    };
    if matches!(
        reason,
        DjRuntimeRendererReason::ProgramNotMixerRenderable
            | DjRuntimeRendererReason::MixerRejected
            | DjRuntimeRendererReason::RenderBufferFailed
            | DjRuntimeRendererReason::BufferLockFailed
    ) {
        state.dj_readiness_permanent_failure = Some(failure);
    }
    state.last_dj_renderer_failure = Some(failure);
}

pub(super) fn record_current_runtime_renderer_failure(
    state: &mut PlaybackRuntimeLoopState,
    reason: DjRuntimeRendererReason,
) {
    let transition = state
        .next_engine
        .as_ref()
        .and_then(|engine| engine.job.prepared_transition.as_ref())
        .cloned();
    if let Some(transition) = transition {
        record_runtime_renderer_failure(state, &transition, reason);
    } else {
        state.last_dj_renderer_failure = None;
    }
}

pub(super) fn renderer_failure_matches_current_transition(
    state: &PlaybackRuntimeLoopState,
    failure: DjRuntimeRendererFailure,
) -> bool {
    let Some(transition) = state
        .next_engine
        .as_ref()
        .and_then(|engine| engine.job.prepared_transition.as_ref())
    else {
        return false;
    };
    failure.queue_generation == transition.queue_generation
        && failure.current_queue_item_id == transition.current_queue_item_id
        && failure.next_queue_item_id == transition.next_queue_item_id
        && failure.transition_event_id == transition.transition_event_id
        && failure.current_track_id == state.engine.as_ref().map(|engine| engine.track_id)
        && failure.next_track_id == state.next_engine.as_ref().map(|engine| engine.track_id)
        && failure.current_engine_generation
            == state.engine.as_ref().map(|engine| engine.generation)
        && failure.next_engine_generation
            == state.next_engine.as_ref().map(|engine| engine.generation)
}

pub(super) fn start_dj_lookahead_in_state(
    state: &mut PlaybackRuntimeLoopState,
    current: Option<DjMediaRef>,
    next: Option<DjMediaRef>,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
    queue_generation: u64,
    deadline_samples: u64,
) -> StartDjLookaheadOutcome {
    let Some(next) = next else {
        state.dj_lookahead = None;
        state.dj_lookahead_failure = Some(DjLookaheadFailure {
            queue_generation,
            current_queue_item_id,
            next_queue_item_id,
            reason: DjLookaheadFailureReason::NextNotResolved,
        });
        return StartDjLookaheadOutcome::MissingNext;
    };
    let Some(next_queue_item_id) = next_queue_item_id else {
        state.dj_lookahead = None;
        state.dj_lookahead_failure = Some(DjLookaheadFailure {
            queue_generation,
            current_queue_item_id,
            next_queue_item_id: None,
            reason: DjLookaheadFailureReason::NextNotResolved,
        });
        return StartDjLookaheadOutcome::MissingNext;
    };

    if state.dj_lookahead.as_ref().is_some_and(|lookahead| {
        lookahead.matches_pair(
            queue_generation,
            current_queue_item_id,
            Some(next_queue_item_id),
        )
    }) {
        return StartDjLookaheadOutcome::AlreadyCurrent;
    }

    let prepared_next = next.track_id().is_some_and(|track_id| {
        state
            .next_engine
            .as_ref()
            .is_some_and(|engine| engine.track_id == track_id)
    });
    state.dj_lookahead = Some(RuntimeDjLookahead {
        current,
        next,
        current_queue_item_id,
        next_queue_item_id,
        queue_generation,
        deadline_samples,
    });
    state.dj_lookahead_failure = None;
    if prepared_next {
        StartDjLookaheadOutcome::ReusedPreparedNext
    } else {
        StartDjLookaheadOutcome::Started
    }
}

pub(super) fn prepared_dj_lookahead_matches_pair(
    state: &PlaybackRuntimeLoopState,
    queue_generation: u64,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
) -> bool {
    state.dj_lookahead.as_ref().is_some_and(|lookahead| {
        lookahead.matches_pair(queue_generation, current_queue_item_id, next_queue_item_id)
    })
}

pub(super) fn discard_stale_prepared_transition(
    state: &PlaybackRuntimeLoopState,
    job: &mut PreparedPlaybackJob,
) -> bool {
    let Some(transition) = job.prepared_transition.as_ref() else {
        return false;
    };
    if prepared_dj_lookahead_matches_pair(
        state,
        transition.queue_generation,
        transition.current_queue_item_id,
        transition.next_queue_item_id,
    ) {
        return false;
    }
    job.prepared_transition = None;
    true
}

pub(super) fn dj_mixer_max_block_samples(output_config: &StreamConfig) -> usize {
    let channels = usize::from(output_config.channels.max(1));
    match output_config.buffer_size {
        cpal::BufferSize::Fixed(frames) => frames as usize * channels,
        cpal::BufferSize::Default => DJ_MIXER_DEFAULT_MAX_BLOCK_FRAMES * channels,
    }
}

pub(super) fn dj_renderer_late_tolerance_frames(sample_rate: u32) -> u64 {
    u64::from(sample_rate.max(1)) / 2
}

pub(super) fn decoded_deck_snapshot(
    engine: &PlaybackEngine,
    channels: u16,
    start_frame: u64,
    cue_is_local: bool,
    required_frames: u64,
    late_tolerance_frames: u64,
    reason: DjRuntimeRendererReason,
) -> Result<RuntimeDeckSnapshot, DjRuntimeRendererReason> {
    let guard = engine
        .shared
        .buffer
        .lock()
        .map_err(|_| DjRuntimeRendererReason::BufferLockFailed)?;
    // Planner cues use original source time. A restarted or compacted deck's
    // PCM begins at its published offset; resolve both under the buffer mutex
    // so a snapshot cannot certify audio at a different source position.
    // Protected recovery already supplies a normalized buffer-local cue.
    let start_frame = if cue_is_local {
        start_frame
    } else {
        let channel_count = u64::from(channels.max(1));
        let output_frame = engine
            .shared
            .source_to_output_samples(start_frame.saturating_mul(channel_count))
            .ok_or(reason)?
            / channel_count;
        let offset_frame = engine
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            / channel_count;
        output_frame.checked_sub(offset_frame).ok_or(reason)?
    };
    snapshot_decoded_buffer(
        engine,
        &guard,
        channels,
        start_frame,
        required_frames,
        late_tolerance_frames,
        reason,
    )
}

pub(super) fn snapshot_decoded_buffer(
    engine: &PlaybackEngine,
    buffer: &shared::PlaybackBuffer,
    channels: u16,
    start_frame: u64,
    required_frames: u64,
    late_tolerance_frames: u64,
    reason: DjRuntimeRendererReason,
) -> Result<RuntimeDeckSnapshot, DjRuntimeRendererReason> {
    if buffer.samples.is_empty() {
        return Err(reason);
    }
    let channels = usize::from(channels.max(1));
    let frames = (buffer.samples.len() / channels) as u64;
    if start_frame >= frames {
        return Err(reason);
    }
    let available_frames = frames.saturating_sub(start_frame);
    if available_frames.saturating_add(late_tolerance_frames) < required_frames.max(1) {
        return Err(reason);
    }
    Ok(RuntimeDeckSnapshot {
        samples: buffer.samples.clone(),
        start_frame,
        // The caller holds the buffer lock, so the decoder cannot move the
        // offset between capturing the PCM and its absolute render origin.
        output_start_frame: engine
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            / channels as u64
            + start_frame,
    })
}

pub(super) fn active_deck_snapshot(
    engine: &PlaybackEngine,
    channels: u16,
    program_start_frame: u64,
    anchored_output_start_frame: Option<u64>,
    required_frames: u64,
    late_tolerance_frames: u64,
) -> Result<RuntimeDeckSnapshot, DjRuntimeRendererReason> {
    let guard = engine
        .shared
        .buffer
        .lock()
        .map_err(|_| DjRuntimeRendererReason::BufferLockFailed)?;
    let channel_count = u64::from(channels.max(1));
    let start_frame = if let Some(output_frame) = anchored_output_start_frame {
        let offset_frames = engine
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            / channel_count;
        output_frame
            .checked_sub(offset_frames)
            .ok_or(DjRuntimeRendererReason::ActiveDeckNotDecoded)?
    } else if program_start_frame == 0 {
        guard.read_pos as u64 / channel_count
    } else {
        program_start_frame
    };
    snapshot_decoded_buffer(
        engine,
        &guard,
        channels,
        start_frame,
        required_frames,
        late_tolerance_frames,
        DjRuntimeRendererReason::ActiveDeckNotDecoded,
    )
}

pub(super) fn build_prepared_dj_mixer(
    state: &PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    max_block_samples: usize,
) -> Result<PreparedDjMixer, DjRuntimeRendererReason> {
    let next = state
        .next_engine
        .as_ref()
        .ok_or(DjRuntimeRendererReason::NextDeckNotDecoded)?;
    if !prepared_dj_lookahead_matches_pair(
        state,
        transition.queue_generation,
        transition.current_queue_item_id,
        transition.next_queue_item_id,
    ) {
        return Err(DjRuntimeRendererReason::LookaheadPairMismatch);
    }
    build_prepared_dj_mixer_for_engine(state, transition, next, max_block_samples)
}

pub(super) fn build_prepared_dj_mixer_for_engine(
    state: &PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    incoming: &PlaybackEngine,
    max_block_samples: usize,
) -> Result<PreparedDjMixer, DjRuntimeRendererReason> {
    build_prepared_dj_mixer_for_engine_at_start(
        state,
        transition,
        incoming,
        max_block_samples,
        false,
    )
}

pub(super) fn build_prepared_dj_mixer_for_engine_at_start(
    state: &PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    incoming: &PlaybackEngine,
    max_block_samples: usize,
    force_live_start: bool,
) -> Result<PreparedDjMixer, DjRuntimeRendererReason> {
    let active = state
        .engine
        .as_ref()
        .ok_or(DjRuntimeRendererReason::ActiveDeckNotDecoded)?;
    if !handoff_mixer_program(&transition.program) && !overlay_mixer_program(&transition.program) {
        return Err(DjRuntimeRendererReason::ProgramNotMixerRenderable);
    }
    // Deck buffers are decoded at the device rate; a program planned at any
    // other rate must have its frame fields rescaled or every marker (and
    // deck B's sync start) lands off by the rate ratio.
    let mut program = transition
        .program
        .clone()
        .rescaled_to(state.device_sample_rate.max(1));
    if force_live_start {
        program.deck_a_start_frame = 0;
    }
    if let Err(error) = noor_mix::planner::safety::validate_audio_safety(
        &program,
        &noor_mix::planner::safety::AudioSafetyPolicy::default(),
    ) {
        warn!("Prepared DJ transition program failed audio safety: {error:?}");
        return Err(DjRuntimeRendererReason::MixerRejected);
    }
    // deck_a_start_frame == 0 means "wherever deck A is when this build
    // runs", which is only correct for a build inside the fire handler. A
    // beat-anchored plan is built ahead of time, so pin deck A to the
    // planned fire position instead; the install-time skip then reconciles
    // the (small) distance the live deck actually travelled past it.
    let anchored_output_start_frame = (program.deck_a_start_frame == 0 && !force_live_start)
        .then(|| anchored_deck_a_output_frame(state, transition, active))
        .flatten();
    let mut deck_b_required_frames = deck_b_consumed_frames(&program)
        .ok_or(DjRuntimeRendererReason::ProgramNotMixerRenderable)?;
    // Preserve the original first-choice plan, but let the same decoded cue
    // prepare a shorter bass phrase when only its complete PCM is available.
    for prefix in beat_sync::musical_prefixes(&program) {
        if let Some(consumed) = deck_b_consumed_frames(&prefix) {
            deck_b_required_frames = deck_b_required_frames.min(consumed);
        }
    }
    let late_tolerance_frames = dj_renderer_late_tolerance_frames(state.device_sample_rate);
    let active_snapshot = active_deck_snapshot(
        active,
        state.device_channels,
        program.deck_a_start_frame,
        anchored_output_start_frame,
        program.resolve_at,
        late_tolerance_frames,
    )?;
    let next_snapshot = decoded_deck_snapshot(
        incoming,
        state.device_channels,
        program.deck_b_start_frame,
        force_live_start,
        deck_b_required_frames.saturating_add(1),
        late_tolerance_frames,
        DjRuntimeRendererReason::NextDeckNotDecoded,
    )?;
    program.deck_a_start_frame = active_snapshot.start_frame;
    program.deck_b_start_frame = next_snapshot.start_frame;
    if beat_sync::required(&program) {
        match beat_sync::synchronize_or_shorten_checked(
            &mut program,
            &active_snapshot.samples,
            &next_snapshot.samples,
        ) {
            Ok(sync) => {
                info!(
                    current_track_id = active.track_id,
                    next_track_id = incoming.track_id,
                    rate = sync.rate,
                    cue_shift_frames = sync.cue_shift_frames,
                    confidence = sync.confidence,
                    residual_ms = sync.residual_ms,
                    "DJ beat sync verified against decoded mix audio"
                );
            }
            Err(failures) => {
                if program.template == "DropPreview16" {
                    // A preview is an optional overlay. An unverified overlay
                    // must never turn into a handoff or replace either live deck.
                    info!(
                        current_track_id = active.track_id,
                        next_track_id = incoming.track_id,
                        rejections = %failures,
                        "DJ drop preview skipped: decoded beat sync unverified"
                    );
                    return Err(DjRuntimeRendererReason::MixerRejected);
                }
                // A long rhythmic overlap needs more than a successful timer or
                // an opening-grid projection. Preserve the established protected
                // renderer, with a short overlap when local percussion is unclear.
                let mut fallback = crate::playback::dj_engine::safe_crossfade_program(
                    program.sample_rate,
                    program.channels,
                    noor_mix::Policy {
                        default_crossfade_ms: 4_000,
                        ..Default::default()
                    },
                );
                fallback.deck_a_start_frame = program.deck_a_start_frame;
                fallback.deck_b_start_frame = program.deck_b_start_frame;
                fallback.decision = program.decision.clone();
                if let Some(decision) = fallback.decision.as_mut() {
                    decision.strategy = "SafeCrossfade".into();
                    decision.reason = format!(
                        "Decoded rhythm does not support a reliable long beat lock ({failures}); using a short protected overlap"
                    );
                    decision.duration_beats = 0.0;
                }
                program = fallback;
                info!(
                    current_track_id = active.track_id,
                    next_track_id = incoming.track_id,
                    rejections = %failures,
                    "DJ long overlap shortened: decoded beat sync unverified"
                );
            }
        }
    }
    if let Err(error) = noor_mix::planner::safety::validate_audio_safety(
        &program,
        &noor_mix::planner::safety::AudioSafetyPolicy::default(),
    ) {
        warn!("Corrected DJ transition program failed audio safety: {error:?}");
        return Err(DjRuntimeRendererReason::MixerRejected);
    }
    let mut mixer = match noor_mix::Mixer::new(
        program.clone(),
        noor_mix::deck::DeckBuffer::new(active_snapshot.samples, state.device_channels),
        noor_mix::deck::DeckBuffer::new(next_snapshot.samples, state.device_channels),
        max_block_samples,
    ) {
        Ok(mixer) => mixer,
        Err(error) => {
            warn!("Prepared DJ mixer rejected transition program: {error:?}");
            return Err(DjRuntimeRendererReason::MixerRejected);
        }
    };
    let rendered = render_mixer_to_buffer(
        &mut mixer,
        program.resolve_at,
        usize::from(state.device_channels.max(1)),
        max_block_samples,
    )
    .ok_or(DjRuntimeRendererReason::RenderBufferFailed)?;
    Ok(PreparedDjMixer {
        program,
        max_block_samples,
        deck_a_output_start_frame: active_snapshot.output_start_frame,
        rendered,
        current_track_id: active.track_id,
        next_track_id: incoming.track_id,
    })
}

/// Buffer-local deck A frame for a beat-anchored transition: the anchor is
/// absolute track time on the decoded-audio timeline, the deck buffer may
/// start mid-track after a segment seek.
#[cfg(test)]
pub(super) fn anchored_deck_a_frame(
    state: &PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    active: &PlaybackEngine,
) -> Option<u64> {
    let anchor_frame_abs = anchored_deck_a_output_frame(state, transition, active)?;
    let channels = u64::from(state.device_channels.max(1));
    let offset_frames = active
        .shared
        .position_offset_samples
        .load(Ordering::Relaxed)
        / channels;
    let local = anchor_frame_abs.checked_sub(offset_frames)?;
    (local > 0).then_some(local)
}

pub(super) fn anchored_deck_a_output_frame(
    state: &PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    active: &PlaybackEngine,
) -> Option<u64> {
    let channels = u64::from(state.device_channels.max(1));
    let anchor_samples = if let Some(anchor_ms) = transition.anchor_start_ms.filter(|ms| *ms > 0) {
        let source_anchor = (anchor_ms as u64)
            .saturating_mul(u64::from(state.device_sample_rate.max(1)))
            .saturating_mul(channels)
            / 1000;
        active.shared.source_to_output_samples(source_anchor)?
    } else {
        let total = active.shared.total_samples.load(Ordering::Relaxed);
        let mut overlap = active.shared.crossfade_samples.load(Ordering::Relaxed);
        if beat_sync::required(&transition.program) {
            // The countdown is expressed in whole milliseconds, while a
            // beat-derived programme retains fractional-millisecond frames.
            // Its PCM window must not overrun EOF by that rounding difference.
            let exact_window = transition
                .program
                .clone()
                .rescaled_to(state.device_sample_rate.max(1))
                .resolve_at
                .saturating_mul(channels);
            overlap = overlap.max(exact_window);
        }
        if total == 0 || overlap == 0 {
            return None;
        }
        total.saturating_sub(overlap)
    };
    Some(anchor_samples / channels)
}

pub(super) fn handoff_mixer_program(program: &noor_mix::TransitionProgram) -> bool {
    matches!(
        program.template.as_str(),
        "SafeCrossfade"
            | "BassSwap16"
            | "BassSwap32"
            | "LongHarmonicBlend"
            | "FilterSweep"
            | "SlamCut"
            | "ClubMix"
            | "QuickMix"
            | "EnergyLift"
            | "EnergyReset"
            | "DropSwap"
    ) && deck_b_consumed_frames(program).is_some()
}

pub(super) fn overlay_mixer_program(program: &noor_mix::TransitionProgram) -> bool {
    matches!(program.template.as_str(), "DropTease16" | "DropPreview16")
        && deck_b_consumed_frames(program).is_some()
}

pub(super) fn render_mixer_to_buffer(
    mixer: &mut noor_mix::Mixer,
    resolve_at: u64,
    channels: usize,
    max_block_samples: usize,
) -> Option<Vec<f32>> {
    let render_frames = resolve_at as usize;
    let render_samples = render_frames.checked_mul(channels)?;
    if render_samples == 0 {
        return None;
    }
    let block_samples = max_block_samples
        .max(channels)
        .saturating_sub(max_block_samples.max(channels) % channels)
        .max(channels);
    let mut rendered = vec![0.0; render_samples];
    let mut master_frame = 0_u64;
    for block in rendered.chunks_mut(block_samples) {
        mixer.render_block(block, master_frame);
        master_frame = master_frame.saturating_add((block.len() / channels) as u64);
    }
    Some(rendered)
}

pub(super) fn deck_b_consumed_frames(program: &noor_mix::TransitionProgram) -> Option<u64> {
    let mut deck_b_rate = 1.0_f32;
    let mut deck_b_rate_event_seen = false;
    for event in &program.automation {
        let noor_mix::Param::PlaybackRate(deck) = event.param else {
            continue;
        };
        if deck != noor_mix::DeckId::B
            || deck_b_rate_event_seen
            || event.start_sample != 0
            || event.end_sample < program.resolve_at
            || (event.from - event.to).abs() > 0.0001
            || !event.to.is_finite()
        {
            return None;
        }
        deck_b_rate = event.to;
        deck_b_rate_event_seen = true;
    }
    Some(((program.resolve_at as f64) * deck_b_rate.max(0.0) as f64).floor() as u64)
}

pub(super) fn install_prepared_handoff_mixer_buffer(
    state: &mut PlaybackRuntimeLoopState,
) -> Result<(), DjRuntimeRendererReason> {
    let result = install_prepared_handoff_mixer_buffer_once(state);
    if result.is_ok()
        || !state.dj_engine_enabled
        || matches!(
            result,
            Err(DjRuntimeRendererReason::LookaheadPairMismatch
                | DjRuntimeRendererReason::ActiveTrackChanged
                | DjRuntimeRendererReason::NextTrackChanged
                | DjRuntimeRendererReason::DjDisabled
                | DjRuntimeRendererReason::ManualSeekSuppressed)
        )
    {
        return result;
    }
    // A long adaptive overlap may be unavailable before local verification
    // can even run, or its protected replacement may have arrived late.
    // Neither failure permits the original long legacy overlap. Try one
    // small protected mix from the live outgoing cursor and the same cue.
    let recovery = (|| {
        let active = state.engine.as_ref()?;
        let incoming = state.next_engine.as_ref()?;
        let mut transition = state
            .next_engine
            .as_ref()?
            .job
            .prepared_transition
            .as_ref()?
            .clone();
        if !beat_sync::required(&transition.program)
            || !prepared_dj_lookahead_matches_pair(
                state,
                transition.queue_generation,
                transition.current_queue_item_id,
                transition.next_queue_item_id,
            )
        {
            return None;
        }
        let lookahead = state.dj_lookahead.as_ref()?;
        if lookahead
            .current
            .as_ref()
            .and_then(DjMediaRef::track_id)
            .is_some_and(|track_id| track_id != active.track_id)
            || lookahead
                .next
                .track_id()
                .is_some_and(|track_id| track_id != incoming.track_id)
        {
            return None;
        }
        let (source_program, max_block_samples, cue_is_local) =
            state.prepared_dj_mixer.as_ref().map_or_else(
                || {
                    (
                        transition
                            .program
                            .clone()
                            .rescaled_to(state.device_sample_rate.max(1)),
                        DJ_MIXER_DEFAULT_MAX_BLOCK_FRAMES
                            * usize::from(state.device_channels.max(1)),
                        false,
                    )
                },
                |prepared| (prepared.program.clone(), prepared.max_block_samples, true),
            );
        // Prepared Mixer cues address its captured buffer. A programme that
        // never prepared still addresses source time; normalize that cue
        // against the incoming buffer offset under the same mutex as PCM.
        let incoming_offset_frames = {
            let _buffer_guard = incoming.shared.buffer.lock().ok();
            incoming
                .shared
                .position_offset_samples
                .load(Ordering::Relaxed)
                / u64::from(state.device_channels.max(1))
        };
        let mut protected = crate::playback::dj_engine::safe_crossfade_program(
            state.device_sample_rate,
            state.device_channels,
            noor_mix::Policy {
                default_crossfade_ms: 4_000,
                ..Default::default()
            },
        );
        protected.deck_b_start_frame = if cue_is_local {
            source_program.deck_b_start_frame
        } else {
            source_program
                .deck_b_start_frame
                .saturating_sub(incoming_offset_frames)
        };
        protected.decision = source_program.decision;
        if let Some(decision) = protected.decision.as_mut() {
            decision.strategy = "SafeCrossfade".into();
            decision.reason = "Long overlap could not join reliably; protected overlap rebuilt from current outgoing audio".into();
            decision.incoming_entry_seconds =
                incoming_offset_frames.saturating_add(protected.deck_b_start_frame) as f32
                    / state.device_sample_rate.max(1) as f32;
            decision.duration_beats = 0.0;
            decision.incoming_drop_seconds = None;
        }
        transition.program = protected;
        Some((transition, max_block_samples))
    })();
    let Some((transition, max_block_samples)) = recovery else {
        return result;
    };
    let rebuilt = state
        .next_engine
        .as_ref()
        .ok_or(DjRuntimeRendererReason::NextDeckNotDecoded)
        .and_then(|incoming| {
            build_prepared_dj_mixer_for_engine_at_start(
                state,
                &transition,
                incoming,
                max_block_samples,
                true,
            )
        });
    let recovery_result = rebuilt.and_then(|prepared| {
        state.prepared_dj_mixer = Some(prepared);
        install_prepared_handoff_mixer_buffer_once(state)
    });
    if recovery_result.is_ok() {
        return recovery_result;
    }
    warn!(
        reason = recovery_result.err().map(DjRuntimeRendererReason::as_str),
        "DJ protected overlap could not join live audio; using a short seam cut"
    );
    arm_protected_handoff_cut(state, &transition.program);
    Err(DjRuntimeRendererReason::ProtectedHandoffCut)
}

pub(super) fn arm_protected_handoff_cut(
    state: &mut PlaybackRuntimeLoopState,
    protected_program: &noor_mix::TransitionProgram,
) {
    state.prepared_dj_mixer = None;
    let cut_samples = u64::from(shared::DJ_HANDOFF_FADE_MS)
        * u64::from(state.device_sample_rate.max(1))
        * u64::from(state.device_channels.max(1))
        / 1_000;
    if let Some(outgoing) = state.engine.as_ref() {
        outgoing
            .shared
            .crossfade_samples
            .store(0, Ordering::Relaxed);
        outgoing.shared.dj_fadeout_start_samples.store(
            outgoing.shared.position_samples.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
    if let Some(incoming) = state.next_engine.as_mut() {
        // Preserve the protected incoming cue when its decoded buffer is
        // available. The recovery uses original source PCM, with unity rate.
        if let Ok(mut buffer) = incoming.shared.buffer.lock() {
            let cue = protected_program
                .deck_b_start_frame
                .saturating_mul(u64::from(state.device_channels.max(1)));
            if cue < buffer.samples.len() as u64 {
                buffer.read_pos = cue as usize;
                incoming.shared.position_samples.store(
                    incoming
                        .shared
                        .position_offset_samples
                        .load(Ordering::Relaxed)
                        .saturating_add(cue),
                    Ordering::Relaxed,
                );
                incoming.shared.publish_source_position();
            }
        }
        incoming
            .shared
            .crossfade_samples
            .store(cut_samples, Ordering::Relaxed);
        incoming.job.gapless.overlap_ms = shared::DJ_HANDOFF_FADE_MS as i32;
        if let Some(transition) = incoming.job.prepared_transition.as_mut() {
            let mut cut = noor_mix::planner::slam_cut_program(
                state.device_sample_rate,
                state.device_channels,
                shared::DJ_HANDOFF_FADE_MS,
            );
            cut.deck_b_start_frame = incoming
                .shared
                .source_position_samples
                .load(Ordering::Relaxed)
                / u64::from(state.device_channels.max(1));
            cut.decision = protected_program.decision.clone();
            if let Some(decision) = cut.decision.as_mut() {
                decision.strategy = "SlamCut".into();
                decision.reason = "Decoded beat sync was unverified and the protected overlap could not join live audio; using a short seam cut".into();
                decision.incoming_entry_seconds =
                    cut.deck_b_start_frame as f32 / state.device_sample_rate.max(1) as f32;
                decision.incoming_drop_seconds = None;
                decision.duration_beats = 0.0;
            }
            transition.program = cut;
        }
    }
}

pub(super) fn install_prepared_handoff_mixer_buffer_once(
    state: &mut PlaybackRuntimeLoopState,
) -> Result<(), DjRuntimeRendererReason> {
    let prepared = state
        .prepared_dj_mixer
        .as_ref()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    if !handoff_mixer_program(&prepared.program) {
        return Err(DjRuntimeRendererReason::ProgramNotMixerRenderable);
    }
    if state
        .engine
        .as_ref()
        .map(|engine| engine.track_id != prepared.current_track_id)
        .unwrap_or(true)
    {
        return Err(DjRuntimeRendererReason::ActiveTrackChanged);
    }
    if state
        .next_engine
        .as_ref()
        .map(|engine| engine.track_id != prepared.next_track_id)
        .unwrap_or(true)
    {
        return Err(DjRuntimeRendererReason::NextTrackChanged);
    }

    // How far has the live deck A playhead moved past the frame the render
    // starts at? The rendered buffer must be joined at that offset or the
    // handoff replays (or drops) exactly that stretch of the outgoing track.
    let channels = usize::from(state.device_channels.max(1));
    let live_deck_a_output_frame = {
        let active = state
            .engine
            .as_ref()
            .ok_or(DjRuntimeRendererReason::ActiveTrackChanged)?;
        let guard = active
            .shared
            .buffer
            .lock()
            .map_err(|_| DjRuntimeRendererReason::BufferLockFailed)?;
        active
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            .saturating_add(guard.read_pos as u64)
            / channels as u64
    };
    let resolve_at = prepared.program.resolve_at;
    let skip_frames = live_deck_a_output_frame.saturating_sub(prepared.deck_a_output_start_frame);
    // Joining past the halfway point means most of the transition already
    // "happened" while we weren't playing it; a plain fallback sounds better
    // than the tail of a blend.
    if skip_frames.saturating_mul(2) > resolve_at {
        return Err(DjRuntimeRendererReason::HandoffSeamTooLate);
    }

    let prepared = state
        .prepared_dj_mixer
        .take()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    let mut rendered = prepared.rendered;
    if rendered.is_empty() {
        return Err(DjRuntimeRendererReason::RenderBufferFailed);
    }
    let skip_samples = (skip_frames as usize).saturating_mul(channels);
    bake_seam_fade_in(
        &mut rendered,
        skip_samples,
        channels,
        state.device_sample_rate,
    );

    let next = state
        .next_engine
        .as_mut()
        .ok_or(DjRuntimeRendererReason::NextDeckNotDecoded)?;
    let deck_b_consumed_frames = deck_b_consumed_frames(&prepared.program)
        .ok_or(DjRuntimeRendererReason::ProgramNotMixerRenderable)?;
    let deck_b_resume_frame = prepared
        .program
        .deck_b_start_frame
        .saturating_add(deck_b_consumed_frames);
    let deck_b_resume_sample = (deck_b_resume_frame as usize).saturating_mul(channels);
    let mut guard = match next.shared.buffer.lock() {
        Ok(guard) => guard,
        Err(_) => return Err(DjRuntimeRendererReason::BufferLockFailed),
    };
    let was_finished = guard.finished;
    let previous_total_samples = next.shared.total_samples.load(Ordering::Relaxed);
    let remainder_start = deck_b_resume_sample.min(guard.samples.len());
    let original_offset_samples = next.shared.position_offset_samples.load(Ordering::Relaxed);
    let mut original_prefix = next
        .shared
        .handoff_source_prefix
        .lock()
        .map_err(|_| DjRuntimeRendererReason::BufferLockFailed)?;
    *original_prefix = Some(guard.samples[..remainder_start].to_vec());
    next.shared
        .handoff_timeline
        .install(timeline::HandoffTimeline {
            output_origin: original_offset_samples / channels as u64,
            source_start: original_offset_samples / channels as u64
                + prepared.program.deck_b_start_frame,
            output_frames: prepared.program.resolve_at,
            source_frames: deck_b_consumed_frames,
        });
    let remainder = guard.samples[remainder_start..].to_vec();
    rendered.extend_from_slice(&remainder);
    guard.samples = rendered;
    guard.read_pos = skip_samples.min(guard.samples.len());
    guard.started = false;
    guard.started_notified = false;
    guard.starved_notified = false;
    guard.finished_notified = false;
    guard.finished = was_finished;
    let rendered_total_samples = next
        .shared
        .position_offset_samples
        .load(Ordering::Relaxed)
        .saturating_add(guard.samples.len() as u64);
    let total_samples = if was_finished || previous_total_samples == 0 {
        rendered_total_samples
    } else {
        next.shared
            .source_to_output_samples(previous_total_samples)
            .unwrap_or(rendered_total_samples)
            .max(rendered_total_samples)
    };
    next.shared
        .total_samples
        .store(total_samples, Ordering::Relaxed);
    // Keep position = offset + read_pos consistent with the skipped join so
    // this track's own future near-end / fire math is not shifted by the
    // seam offset.
    next.shared.position_samples.store(
        next.shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            .saturating_add(guard.read_pos as u64),
        Ordering::Relaxed,
    );
    next.shared.publish_buffered_samples(guard.samples.len());
    next.shared.publish_source_position();
    // Persist and expose the audio that actually executed, including the
    // verified cue/rate or the protected short fallback. Buffer-local A
    // positions must not escape as source-track cues.
    if let Some(transition) = next.job.prepared_transition.as_mut() {
        let mut executed = prepared.program.clone();
        executed.deck_a_start_frame = 0;
        executed.deck_b_start_frame =
            original_offset_samples / channels as u64 + prepared.program.deck_b_start_frame;
        transition.program = executed;
    }
    next.shared.crossfade_samples.store(0, Ordering::Relaxed);
    next.shared
        .crossfade_start_signaled
        .store(true, Ordering::Relaxed);
    next.shared
        .fadein_start_samples
        .store(u64::MAX, Ordering::Relaxed);
    Ok(())
}

/// Ramp the first DJ_HANDOFF_FADE_MS of the joined transition audio from
/// silence, per frame so channels stay matched. Pairs with the outgoing
/// engine's dj_fadeout so the stream swap is two short equal-power ramps
/// instead of a hard cut into a hard start. Capped at a quarter of the
/// remaining transition so degenerate (test-sized) programs pass through
/// untouched.
pub(super) fn bake_seam_fade_in(
    rendered: &mut [f32],
    start_sample: usize,
    channels: usize,
    rate: u32,
) {
    let channels = channels.max(1);
    let remaining_frames = rendered.len().saturating_sub(start_sample) / channels;
    let fade_frames = ((u64::from(shared::DJ_HANDOFF_FADE_MS) * u64::from(rate.max(1)) / 1000)
        as usize)
        .min(remaining_frames / 4);
    if fade_frames == 0 {
        return;
    }
    for (index, sample) in rendered
        .iter_mut()
        .skip(start_sample)
        .take(fade_frames * channels)
        .enumerate()
    {
        let frame = index / channels;
        let t = frame as f32 / fade_frames as f32;
        *sample *= (t * std::f32::consts::FRAC_PI_2).sin();
    }
}

pub(super) fn install_prepared_overlay_mixer_buffer(
    state: &mut PlaybackRuntimeLoopState,
) -> Result<(), DjRuntimeRendererReason> {
    let prepared = state
        .prepared_dj_mixer
        .as_ref()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    if !overlay_mixer_program(&prepared.program) {
        return Err(DjRuntimeRendererReason::ProgramNotMixerRenderable);
    }
    if state
        .engine
        .as_ref()
        .map(|engine| engine.track_id != prepared.current_track_id)
        .unwrap_or(true)
    {
        return Err(DjRuntimeRendererReason::ActiveTrackChanged);
    }
    if state
        .next_engine
        .as_ref()
        .map(|engine| engine.track_id != prepared.next_track_id)
        .unwrap_or(true)
    {
        return Err(DjRuntimeRendererReason::NextTrackChanged);
    }

    let prepared = state
        .prepared_dj_mixer
        .take()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    let rendered = prepared.rendered;
    if rendered.is_empty() {
        return Err(DjRuntimeRendererReason::RenderBufferFailed);
    }
    let next = state
        .next_engine
        .as_ref()
        .ok_or(DjRuntimeRendererReason::NextDeckNotDecoded)?;
    let mut guard = match next.shared.buffer.lock() {
        Ok(guard) => guard,
        Err(_) => return Err(DjRuntimeRendererReason::BufferLockFailed),
    };
    guard.samples = rendered;
    guard.read_pos = 0;
    guard.started = false;
    guard.started_notified = false;
    guard.starved_notified = false;
    guard.finished_notified = false;
    guard.finished = true;
    next.shared
        .total_samples
        .store(guard.samples.len() as u64, Ordering::Relaxed);
    next.shared.publish_buffered_samples(guard.samples.len());
    next.shared.crossfade_samples.store(0, Ordering::Relaxed);
    next.shared
        .crossfade_start_signaled
        .store(true, Ordering::Relaxed);
    next.shared
        .fadein_start_samples
        .store(u64::MAX, Ordering::Relaxed);
    Ok(())
}

pub(super) fn install_prepared_drop_preview_mixer_buffer(
    state: &mut PlaybackRuntimeLoopState,
) -> Result<(), DjRuntimeRendererReason> {
    let prepared = state
        .prepared_drop_preview_mixer
        .as_ref()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    if prepared.program.template != "DropPreview16" || !overlay_mixer_program(&prepared.program) {
        return Err(DjRuntimeRendererReason::ProgramNotMixerRenderable);
    }
    if state
        .engine
        .as_ref()
        .map(|engine| engine.track_id != prepared.current_track_id)
        .unwrap_or(true)
    {
        return Err(DjRuntimeRendererReason::ActiveTrackChanged);
    }
    if state
        .drop_preview_engine
        .as_ref()
        .map(|engine| engine.track_id != prepared.next_track_id)
        .unwrap_or(true)
    {
        return Err(DjRuntimeRendererReason::NextTrackChanged);
    }

    // The outgoing track continues while verification/rendering runs. Join
    // the preview at the same elapsed output frame so its verified beats
    // follow the live track, rather than replaying the original preview cue.
    let channels = usize::from(state.device_channels.max(1));
    let live_output_frame = {
        let active = state
            .engine
            .as_ref()
            .ok_or(DjRuntimeRendererReason::ActiveTrackChanged)?;
        let guard = active
            .shared
            .buffer
            .lock()
            .map_err(|_| DjRuntimeRendererReason::BufferLockFailed)?;
        active
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            .saturating_add(guard.read_pos as u64)
            / channels as u64
    };
    if live_output_frame < prepared.deck_a_output_start_frame {
        return Err(DjRuntimeRendererReason::HandoffSeamTooLate);
    }
    let skip_frames = live_output_frame - prepared.deck_a_output_start_frame;
    if skip_frames.saturating_mul(2) > prepared.program.resolve_at {
        return Err(DjRuntimeRendererReason::HandoffSeamTooLate);
    }

    let prepared = state
        .prepared_drop_preview_mixer
        .take()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    let mut rendered = prepared.rendered;
    if rendered.is_empty() {
        return Err(DjRuntimeRendererReason::RenderBufferFailed);
    }
    let skip_samples = (skip_frames as usize).saturating_mul(channels);
    bake_seam_fade_in(
        &mut rendered,
        skip_samples,
        channels,
        state.device_sample_rate,
    );
    let preview = state
        .drop_preview_engine
        .as_ref()
        .ok_or(DjRuntimeRendererReason::NextDeckNotDecoded)?;
    let mut guard = match preview.shared.buffer.lock() {
        Ok(guard) => guard,
        Err(_) => return Err(DjRuntimeRendererReason::BufferLockFailed),
    };
    guard.samples = rendered;
    guard.read_pos = skip_samples.min(guard.samples.len());
    guard.started = false;
    guard.started_notified = false;
    guard.starved_notified = false;
    guard.finished_notified = false;
    guard.finished = true;
    guard.sealed_for_render = true;
    preview
        .shared
        .total_samples
        .store(guard.samples.len() as u64, Ordering::Relaxed);
    preview.shared.publish_buffered_samples(guard.samples.len());
    preview.shared.position_samples.store(
        preview
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            .saturating_add(guard.read_pos as u64),
        Ordering::Relaxed,
    );
    preview.shared.crossfade_samples.store(0, Ordering::Relaxed);
    preview
        .shared
        .crossfade_start_signaled
        .store(true, Ordering::Relaxed);
    preview
        .shared
        .fadein_start_samples
        .store(u64::MAX, Ordering::Relaxed);
    Ok(())
}

/// True when the already-prepared (and pre-rendered) DJ mixer is for exactly
/// the active/next engine pair currently in state.
pub(super) fn prepared_dj_mixer_matches_pair(state: &PlaybackRuntimeLoopState) -> bool {
    let Some(prepared) = state.prepared_dj_mixer.as_ref() else {
        return false;
    };
    let active_id = state.engine.as_ref().map(|engine| engine.track_id);
    let next_id = state.next_engine.as_ref().map(|engine| engine.track_id);
    active_id == Some(prepared.current_track_id) && next_id == Some(prepared.next_track_id)
}

pub(super) fn can_prepare_dj_mixer_before_fire(state: &PlaybackRuntimeLoopState) -> bool {
    let Some(active) = state.engine.as_ref() else {
        return false;
    };
    let anchored = state
        .next_engine
        .as_ref()
        .and_then(|next| next.job.prepared_transition.as_ref())
        .is_some_and(|transition| transition.anchor_start_ms.is_some());
    anchored
        || active.shared.total_samples.load(Ordering::Relaxed) > 0
        || active
            .shared
            .crossfade_start_signaled
            .load(Ordering::Relaxed)
}

/// The next decoder can pause at its high-water mark without ever reaching
/// EOF. Wake the existing serialized completion handler when real PCM becomes
/// sufficient instead of waiting for a notification that may never arrive.
pub(super) fn dj_pcm_readiness_wakeup(
    state: &PlaybackRuntimeLoopState,
) -> Option<PlaybackRuntimeCommand> {
    let transition = adaptive_rhythmic_transition(state)?;
    let active = state.engine.as_ref()?;
    let incoming = state.next_engine.as_ref()?;
    if active.shared.stopped.load(Ordering::Relaxed)
        || incoming.shared.stopped.load(Ordering::Relaxed)
        || active_engine_suppresses_crossfade_after_seek(state)
        || !can_prepare_dj_mixer_before_fire(state)
        || state
            .dj_readiness_permanent_failure
            .is_some_and(|failure| renderer_failure_matches_current_transition(state, failure))
    {
        return None;
    }
    let fired = active
        .shared
        .crossfade_start_signaled
        .load(Ordering::Relaxed);
    let prepared_matches = prepared_dj_mixer_matches_pair(state);
    if (!fired && prepared_matches)
        || (fired && (state.user_paused || active.shared.paused.load(Ordering::Relaxed)))
    {
        return None;
    }
    let program = transition
        .program
        .clone()
        .rescaled_to(state.device_sample_rate.max(1));
    let channels = u64::from(state.device_channels.max(1));
    let next_buffer = crossfade_readiness_snapshot(incoming)?;
    let full_incoming = next_buffer.base_ready
        && adaptive_next_required_samples(state, next_buffer)
            .is_some_and(|required| required != u64::MAX && next_buffer.unread_samples >= required);
    let (full_outgoing, live_outgoing) = {
        let buffer = active.shared.buffer.lock().ok()?;
        let offset_frames = active
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed)
            / channels;
        let available_frames = buffer.samples.len() as u64 / channels;
        let live_frame = buffer.read_pos as u64 / channels;
        let start = if program.deck_a_start_frame == 0 {
            match anchored_deck_a_output_frame(state, transition, active) {
                Some(anchor) => anchor.checked_sub(offset_frames),
                None => Some(live_frame),
            }
        } else {
            Some(program.deck_a_start_frame)
        };
        let full = if fired && prepared_matches {
            let prepared = state.prepared_dj_mixer.as_ref()?;
            offset_frames
                .saturating_add(live_frame)
                .saturating_sub(prepared.deck_a_output_start_frame)
                <= prepared.program.resolve_at / 2
        } else {
            start.is_some_and(|start| {
                start.saturating_add(program.resolve_at) <= available_frames
                    && (!fired || live_frame.saturating_sub(start) <= program.resolve_at / 2)
            })
        };
        let protected_frames = u64::from(state.device_sample_rate.max(1)) * 9 / 2;
        (
            full,
            live_frame.saturating_add(protected_frames) <= available_frames,
        )
    };
    if !(full_incoming && full_outgoing) {
        // A late anchor may have compacted away, or its midpoint may already
        // have passed. The install helper then builds a fresh four-second
        // protected overlap from live A, preserving the actual incoming cue.
        let cue_samples = if prepared_matches {
            state
                .prepared_dj_mixer
                .as_ref()?
                .program
                .deck_b_start_frame
                .saturating_mul(channels)
                .saturating_add(next_buffer.offset_samples)
        } else {
            program.deck_b_start_frame.saturating_mul(channels)
        };
        let protected_samples = u64::from(state.device_sample_rate.max(1)) * channels * 9 / 2;
        let protected_incoming = cue_samples
            .checked_sub(next_buffer.offset_samples)
            .is_some_and(|cue| {
                next_buffer.base_ready
                    && cue.saturating_add(protected_samples) <= next_buffer.decoded_samples
            });
        if !fired || full_outgoing || !live_outgoing || !protected_incoming {
            return None;
        }
    }
    Some(PlaybackRuntimeCommand::NextDecodeComplete {
        track_id: incoming.track_id,
        generation: incoming.generation,
    })
}

pub(super) fn crossfade_readiness_snapshot(
    engine: &PlaybackEngine,
) -> Option<CrossfadeReadinessSnapshot> {
    let buffer = engine.shared.buffer.lock().ok()?;
    Some(CrossfadeReadinessSnapshot {
        base_ready: buffer.is_ready(),
        finished: buffer.finished,
        unread_samples: buffer.samples.len().saturating_sub(buffer.read_pos) as u64,
        decoded_samples: buffer.samples.len() as u64,
        read_samples: buffer.read_pos as u64,
        offset_samples: engine
            .shared
            .position_offset_samples
            .load(Ordering::Relaxed),
        start_threshold_samples: buffer.start_threshold_samples as u64,
    })
}

pub(super) fn resolved_analysis_stream_in_state(
    state: &PlaybackRuntimeLoopState,
    track_id: i64,
) -> Option<StreamInfo> {
    [state.engine.as_ref(), state.next_engine.as_ref()]
        .into_iter()
        .flatten()
        .find(|deck| {
            deck.track_id == track_id
                && !deck.shared.stopped.load(Ordering::Relaxed)
                && deck
                    .shared
                    .buffer
                    .lock()
                    .is_ok_and(|buffer| buffer.is_ready())
        })
        .and_then(|deck| deck.job.resolved_stream.as_ref())
        .filter(|resolved| resolved.is_fresh())
        .map(|resolved| resolved.info.clone())
}

pub(super) fn prepare_dj_mixer_for_pair(
    state: &mut PlaybackRuntimeLoopState,
    max_block_samples: usize,
) -> Result<(), DjRuntimeRendererReason> {
    if !state.dj_engine_enabled {
        state.prepared_dj_mixer = None;
        record_current_runtime_renderer_failure(state, DjRuntimeRendererReason::DjDisabled);
        return Err(DjRuntimeRendererReason::DjDisabled);
    }
    let Some(transition) = state
        .next_engine
        .as_ref()
        .and_then(|engine| engine.job.prepared_transition.as_ref())
        .cloned()
    else {
        state.prepared_dj_mixer = None;
        state.last_dj_renderer_failure = None;
        return Err(DjRuntimeRendererReason::PreparedMixerMissing);
    };
    match build_prepared_dj_mixer(state, &transition, max_block_samples) {
        Ok(prepared) => {
            state.prepared_dj_mixer = Some(prepared);
            state.last_dj_renderer_failure = None;
            state.dj_readiness_permanent_failure = None;
            Ok(())
        }
        Err(reason) => {
            state.prepared_dj_mixer = None;
            record_runtime_renderer_failure(state, &transition, reason);
            Err(reason)
        }
    }
}

pub(super) fn prepare_drop_preview_mixer(
    state: &mut PlaybackRuntimeLoopState,
    max_block_samples: usize,
) -> Result<(), DjRuntimeRendererReason> {
    // A failed fire-time rebuild must not leave an earlier render playable.
    state.prepared_drop_preview_mixer = None;
    if !state.dj_engine_enabled {
        state.prepared_drop_preview_mixer = None;
        return Err(DjRuntimeRendererReason::DjDisabled);
    }
    let Some((transition, incoming)) = state.drop_preview_engine.as_ref().and_then(|engine| {
        engine
            .job
            .prepared_transition
            .as_ref()
            .map(|transition| (transition.clone(), engine))
    }) else {
        state.prepared_drop_preview_mixer = None;
        return Err(DjRuntimeRendererReason::PreparedMixerMissing);
    };
    if transition.program.template != "DropPreview16" {
        state.prepared_drop_preview_mixer = None;
        return Err(DjRuntimeRendererReason::ProgramNotMixerRenderable);
    }
    if !prepared_dj_lookahead_matches_pair(
        state,
        transition.queue_generation,
        transition.current_queue_item_id,
        transition.next_queue_item_id,
    ) || incoming
        .job
        .dj_media_ref
        .as_ref()
        .is_some_and(|media| state.dj_lookahead.as_ref().map(|pair| &pair.next) != Some(media))
    {
        return Err(DjRuntimeRendererReason::LookaheadPairMismatch);
    }
    match build_prepared_dj_mixer_for_engine(state, &transition, incoming, max_block_samples) {
        Ok(prepared) => {
            state.prepared_drop_preview_mixer = Some(prepared);
            Ok(())
        }
        Err(reason) => {
            state.prepared_drop_preview_mixer = None;
            Err(reason)
        }
    }
}

pub(super) fn prepared_overlay_program(state: &PlaybackRuntimeLoopState) -> bool {
    state
        .prepared_dj_mixer
        .as_ref()
        .is_some_and(|prepared| overlay_mixer_program(&prepared.program))
}

pub(super) fn start_prepared_overlay(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    timing_status: &'static str,
    runtime_renderer_reason: DjRuntimeRendererReason,
    actual_start_ms_override: Option<i64>,
    runtime_planned_start_ms: Option<i64>,
    device_sample_rate: u32,
    device_channels: u16,
) -> Result<(), DjRuntimeRendererReason> {
    let transition_event_id = state
        .next_engine
        .as_ref()
        .and_then(|next| next.job.prepared_transition.as_ref())
        .and_then(|transition| transition.transition_event_id);
    let Some(active) = state.engine.as_ref() else {
        return Err(DjRuntimeRendererReason::ActiveDeckNotDecoded);
    };
    let outgoing_track_id = active.track_id;
    let outgoing_generation = active.generation;
    let actual_start_ms = actual_start_ms_override
        .unwrap_or_else(|| track_position_ms(&active.shared, device_sample_rate, device_channels));
    active.shared.crossfade_samples.store(0, Ordering::Relaxed);

    install_prepared_overlay_mixer_buffer(state)?;
    let Some(next) = state.next_engine.as_ref() else {
        return Err(DjRuntimeRendererReason::NextDeckNotDecoded);
    };
    // Honor the user-pause latch: a promotion never un-pauses on its own.
    next.shared
        .paused
        .store(state.user_paused, Ordering::SeqCst);
    if let Some(transition_event_id) = transition_event_id {
        let _ = event_tx.send(PlaybackRuntimeEvent::DjTransitionPromoted {
            transition_event_id,
            outgoing_track_id,
            generation: outgoing_generation,
            actual_start_ms,
            runtime_planned_start_ms,
            timing_status: timing_status.to_string(),
            runtime_rendered_dj_mixer: true,
            runtime_renderer_status: DjRuntimeRendererOutcome::rendered_overlay()
                .status
                .as_str()
                .to_string(),
            runtime_renderer_reason: runtime_renderer_reason.as_str().to_string(),
            runtime_program_json: None,
        });
    }
    Ok(())
}

pub(super) fn start_prepared_drop_preview_overlay(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    actual_start_ms: i64,
) -> Result<(), DjRuntimeRendererReason> {
    let Some(active) = state.engine.as_ref() else {
        return Err(DjRuntimeRendererReason::ActiveDeckNotDecoded);
    };
    let active_track_id = active.track_id;
    let active_generation = active.generation;

    install_prepared_drop_preview_mixer_buffer(state)?;
    let Some(preview) = state.drop_preview_engine.as_ref() else {
        return Err(DjRuntimeRendererReason::NextDeckNotDecoded);
    };
    // Honor the user-pause latch: a preview never un-pauses on its own.
    preview
        .shared
        .paused
        .store(state.user_paused, Ordering::SeqCst);
    let _ = event_tx.send(PlaybackRuntimeEvent::DropPreviewStarted {
        track_id: active_track_id,
        generation: active_generation,
        actual_start_ms,
        queue_generation: preview
            .job
            .prepared_transition
            .as_ref()
            .map_or(0, |plan| plan.queue_generation),
    });
    Ok(())
}

/// Replace only the musical instructions of an unheard prepared deck. Its
/// decoder, original PCM, queue identity and event identity stay intact.
pub(super) fn update_prepared_transition_in_state(
    state: &mut PlaybackRuntimeLoopState,
    transition: PreparedTransitionProgram,
    gapless: GaplessPlan,
) -> bool {
    if !state.dj_engine_enabled
        || !prepared_dj_lookahead_matches_pair(
            state,
            transition.queue_generation,
            transition.current_queue_item_id,
            transition.next_queue_item_id,
        )
        || transition.program.validate().is_err()
        || noor_mix::planner::safety::validate_audio_safety(
            &transition.program,
            &noor_mix::planner::safety::AudioSafetyPolicy::default(),
        )
        .is_err()
        || gapless.overlap_ms <= 0
    {
        return false;
    }
    let (Some(active), Some(next)) = (state.engine.as_ref(), state.next_engine.as_ref()) else {
        return false;
    };
    let Some(previous) = next.job.prepared_transition.as_ref() else {
        return false;
    };
    if previous.transition_event_id != transition.transition_event_id
        || previous.queue_generation != transition.queue_generation
        || previous.current_queue_item_id != transition.current_queue_item_id
        || previous.next_queue_item_id != transition.next_queue_item_id
        || active
            .shared
            .crossfade_start_signaled
            .load(Ordering::Relaxed)
    {
        return false;
    }
    let samples = |ms: u64| {
        ms.saturating_mul(u64::from(state.device_sample_rate))
            .saturating_mul(u64::from(state.device_channels.max(1)))
            / 1000
    };
    let position = active.shared.position_samples.load(Ordering::Relaxed);
    let total = active.shared.total_samples.load(Ordering::Relaxed);
    // Exact decoded length arrives at EOF. Metadata guards only an unheard
    // update before then; it never becomes the scheduler's audio fire anchor.
    let update_guard_total = (total > 0).then_some(total).or_else(|| {
        active
            .job
            .track
            .duration_ms
            .filter(|ms| *ms > 0)
            .and_then(|ms| active.shared.source_to_output_samples(samples(ms as u64)))
    });
    let old_anchor = active
        .shared
        .dj_fire_trigger_samples
        .load(Ordering::Relaxed);
    let old_target = if old_anchor != u64::MAX {
        Some(old_anchor)
    } else {
        update_guard_total.map(|total| {
            total.saturating_sub(active.shared.crossfade_samples.load(Ordering::Relaxed))
        })
    };
    let new_target = if let Some(anchor) = transition.anchor_start_ms.filter(|ms| *ms >= 0) {
        active
            .shared
            .source_to_output_samples(samples(anchor as u64))
    } else {
        update_guard_total.map(|total| {
            total.saturating_sub(samples(
                gapless.overlap_ms as u64 + u64::from(transition.fire_ahead_ms),
            ))
        })
    };
    let deadline = position.saturating_add(samples(2000));
    if old_target.is_none_or(|target| target <= deadline)
        || new_target.is_none_or(|target| target <= deadline)
    {
        return false;
    }
    let next = state.next_engine.as_mut().expect("checked prepared deck");
    next.job.prepared_transition = Some(transition);
    next.job.gapless = gapless;
    let job = next.job.clone();
    state.prepared_dj_mixer = None;
    state.last_dj_renderer_failure = None;
    state.dj_readiness_permanent_failure = None;
    arm_active_transition_window(state, &job)
}

pub(super) fn arm_active_transition_window(
    state: &mut PlaybackRuntimeLoopState,
    job: &PreparedPlaybackJob,
) -> bool {
    let Some(transition) = job.prepared_transition.as_ref() else {
        return false;
    };
    if job.gapless.overlap_ms <= 0 {
        return false;
    }
    let Some(engine) = state.engine.as_ref() else {
        return false;
    };
    let trigger_ms = u64::from(job.gapless.overlap_ms as u32)
        .saturating_add(u64::from(transition.fire_ahead_ms));
    let samples = trigger_ms
        .saturating_mul(state.device_sample_rate as u64)
        .saturating_mul(state.device_channels.max(1) as u64)
        / 1000;
    if samples == 0 {
        return false;
    }
    state.dj_readiness_permanent_failure = None;
    engine
        .shared
        .crossfade_samples
        .store(samples, Ordering::Relaxed);
    // Beat-anchored plans fire at an absolute decoded-audio position; the
    // from-end countdown stays as the fallback for plans without a grid.
    // Always (re)store so a re-arm with a gridless plan clears a stale
    // anchor from an earlier plan on the same engine.
    let anchor_trigger_samples = transition
        .anchor_start_ms
        .filter(|anchor_ms| *anchor_ms >= 0)
        .and_then(|anchor_ms| {
            let source_samples = (anchor_ms as u64)
                .saturating_mul(state.device_sample_rate as u64)
                .saturating_mul(state.device_channels.max(1) as u64)
                / 1000;
            engine.shared.source_to_output_samples(source_samples)
        });
    engine.shared.dj_fire_trigger_samples.store(
        anchor_trigger_samples.unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    engine
        .shared
        .crossfade_start_signaled
        .store(false, Ordering::Relaxed);
    info!(
        track_id = engine.track_id,
        next_track_id = job.track.id,
        overlap_ms = job.gapless.overlap_ms,
        fire_ahead_ms = transition.fire_ahead_ms,
        overlap_samples = samples,
        anchor_start_ms = transition.anchor_start_ms,
        "DJ transition window armed"
    );
    true
}

/// The original planned_start_ms is retained in the database. This target
/// records what the audio scheduler actually counted toward, so duration
/// mismatch is inspectable separately from fire precision.
pub(super) fn runtime_transition_target_ms(
    state: &PlaybackRuntimeLoopState,
    callback_target_samples: Option<u64>,
) -> Option<i64> {
    let next = state.next_engine.as_ref()?;
    let transition = next.job.prepared_transition.as_ref()?;
    if let Some(anchor) = transition.anchor_start_ms {
        return Some(anchor);
    }
    let target = if let Some(callback_target) = callback_target_samples {
        // The countdown window includes the calibrated fire-ahead amount;
        // report the musical target before that compensation.
        callback_target.saturating_add(
            u64::from(transition.fire_ahead_ms)
                .saturating_mul(u64::from(state.device_sample_rate))
                .saturating_mul(u64::from(state.device_channels.max(1)))
                / 1_000,
        )
    } else {
        let total = state
            .engine
            .as_ref()?
            .shared
            .total_samples
            .load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        total.saturating_sub(
            (next.job.gapless.overlap_ms.max(0) as u64)
                .saturating_mul(u64::from(state.device_sample_rate))
                .saturating_mul(u64::from(state.device_channels.max(1)))
                / 1_000,
        )
    };
    let source_target = state
        .engine
        .as_ref()?
        .shared
        .output_to_source_samples(target);
    Some(samples_to_ms(
        source_target,
        state.device_sample_rate,
        state.device_channels,
    ))
}

pub(super) fn arm_drop_preview_in_state(
    state: &PlaybackRuntimeLoopState,
    track_id: i64,
    generation: u64,
    trigger_position_samples: u64,
) -> bool {
    if !state.dj_engine_enabled {
        if let Some(active) = state.engine.as_ref() {
            active.shared.clear_drop_preview_trigger();
        }
        return false;
    }
    let Some(active) = state
        .engine
        .as_ref()
        .filter(|engine| engine.track_id == track_id && engine.generation == generation)
    else {
        return false;
    };
    active.shared.drop_preview_trigger_samples.store(
        active
            .shared
            .source_to_output_samples(trigger_position_samples)
            .unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    active
        .shared
        .drop_preview_start_signaled
        .store(false, Ordering::Relaxed);
    true
}

pub(super) fn gate_prepare_next_for_dj(
    state: &mut PlaybackRuntimeLoopState,
    job: &mut PreparedPlaybackJob,
) -> bool {
    if !state.dj_engine_enabled {
        job.prepared_transition = None;
        state.prepared_dj_mixer = None;
        return false;
    }
    if discard_stale_prepared_transition(state, job) {
        state.prepared_dj_mixer = None;
    }
    job.prepared_transition.is_some()
}

pub(super) fn set_dj_engine_enabled_in_state(state: &mut PlaybackRuntimeLoopState, enabled: bool) {
    state.dj_engine_enabled = enabled;
    if enabled {
        return;
    }
    state.dj_lookahead = None;
    state.dj_lookahead_failure = None;
    state.prepared_dj_mixer = None;
    state.prepared_drop_preview_mixer = None;
    state.last_dj_renderer_failure = None;
    state.dj_readiness_permanent_failure = None;
    if let Some(engine) = state.engine.as_ref() {
        engine.shared.clear_drop_preview_trigger();
    }
    if let Some(engine) = state.next_engine.as_mut() {
        engine.job.prepared_transition = None;
    }
    if let Some(mut engine) = state.drop_preview_engine.take() {
        engine.stop();
    }
}
