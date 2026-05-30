use crate::db::audio_settings::ExclusiveLatencyMode;
use crate::playback::dj_lookahead::DjMediaRef;
use crate::playback::output::cpal_shared::{SwapBackend, swap_stream_plan};
#[cfg(target_os = "windows")]
use crate::playback::output::wasapi_exclusive::{
    ExclusiveRenderRole, ExclusiveRenderSource, ExclusiveRuntimeSink, build_exclusive_stream,
};
use crate::playback::player::{PreparedPlaybackJob, PreparedTransitionProgram};
use crate::services::audio_analysis::dj_profile::DjAnalysisJob;
use crate::services::tidal::stream::{StreamInfo, StreamRequest};
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{SampleFormat, StreamConfig};

pub mod commands;
mod device;
mod engine;
pub(crate) mod shared;

pub use commands::{
    PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackTerminalReason, PlaybackTrackStatus,
    SeekToOutcome,
};
pub use device::{OutputDeviceSelection, enumerate_output_devices};
use device::{device_display_name, resolve_device};
use engine::PlaybackEngine;
#[cfg(test)]
use engine::SwapPauseGuard;
pub(crate) use shared::PlaybackSharedState;
#[cfg(target_os = "windows")]
pub(crate) use shared::fill_f32_from_shared;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use tracing::{debug, error, info, warn};

const DJ_MIXER_DEFAULT_MAX_BLOCK_FRAMES: usize = 8192;

pub type RuntimeStreamResolver = Arc<
    dyn Fn(StreamRequest) -> Pin<Box<dyn Future<Output = Result<StreamInfo>> + Send>> + Send + Sync,
>;

/// Pure decision helper for the SeekTo handler. Moved out of `server::routes`
/// (r6 fix A: keep the playback runtime free of HTTP-layer dependencies). The
/// runtime's SeekTo handler calls this with absolute-track samples; the route
/// no longer touches it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeekDecision {
    /// Either no runtime / engine is active, or the buffer is fresh (no
    /// samples published yet), or the target is inside `[offset, buffered]`.
    /// Dispatch the seek to the runtime's in-buffer fast path.
    Dispatch,
    /// Target is strictly outside `[offset, buffered]` and the runtime has
    /// published a non-zero buffered_samples value (so we're past the
    /// cold-start window). Routed to either the segment-restart path or
    /// 409-style rejection depending on the caller's `allow_segment_seek`.
    RejectOutOfBuffer,
}

pub(crate) fn evaluate_seek_decision(
    target_samples: u64,
    buffered_start_samples: u64,
    buffered_samples: u64,
    runtime_active: bool,
) -> SeekDecision {
    if !runtime_active {
        return SeekDecision::Dispatch;
    }
    // buffered_samples == 0 means the audio callback hasn't published any
    // value yet (engine cold-starting, first callback not fired). Treat that
    // as "unknown, let the runtime decide" rather than blanket-rejecting all
    // seeks during the cold-start window.
    if buffered_samples == 0 {
        return SeekDecision::Dispatch;
    }
    if target_samples < buffered_start_samples || target_samples > buffered_samples {
        SeekDecision::RejectOutOfBuffer
    } else {
        SeekDecision::Dispatch
    }
}

#[derive(Clone)]
pub struct PlaybackRuntimeConfig {
    pub http_client: reqwest::Client,
    pub access_token: String,
    pub stream_resolver: Option<RuntimeStreamResolver>,
    /// Channel to send mono audio samples for passive DSP analysis.
    /// (track_id, mono_samples, sample_rate)
    pub analysis_tx: Option<tokio::sync::mpsc::UnboundedSender<(i64, Vec<f32>, u32)>>,
    pub dj_analysis_tx: Option<tokio::sync::mpsc::UnboundedSender<DjAnalysisJob>>,
    pub dj_engine_enabled: bool,
    pub dj_analysis_only: bool,
}

impl PlaybackRuntimeConfig {
    pub fn new(
        http_client: reqwest::Client,
        access_token: impl Into<String>,
        analysis_tx: Option<tokio::sync::mpsc::UnboundedSender<(i64, Vec<f32>, u32)>>,
    ) -> Self {
        Self {
            http_client,
            access_token: access_token.into(),
            stream_resolver: None,
            analysis_tx,
            dj_analysis_tx: None,
            dj_engine_enabled: false,
            dj_analysis_only: false,
        }
    }

    pub fn with_stream_resolver(mut self, resolver: RuntimeStreamResolver) -> Self {
        self.stream_resolver = Some(resolver);
        self
    }

    pub(crate) async fn resolve_stream(&self, request: StreamRequest) -> Result<StreamInfo> {
        if let Some(resolver) = self.stream_resolver.as_ref() {
            return resolver(request).await;
        }
        crate::services::tidal::stream::resolve_stream(
            &self.http_client,
            &self.access_token,
            &request,
        )
        .await
        .map_err(anyhow::Error::from)
    }

    pub fn with_dj_analysis(
        mut self,
        dj_engine_enabled: bool,
        dj_analysis_tx: Option<tokio::sync::mpsc::UnboundedSender<DjAnalysisJob>>,
    ) -> Self {
        self.dj_engine_enabled = dj_engine_enabled;
        self.dj_analysis_tx = dj_analysis_tx;
        self
    }

    pub fn for_dj_analysis_only(mut self) -> Self {
        self.dj_analysis_only = true;
        self
    }
}

#[derive(Clone)]
pub struct PlaybackRuntimeHandle {
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    /// f32 volume (0.0–1.0) stored as its bit-pattern in a u32.
    volume_ctl: Arc<AtomicU32>,
    /// Redirectable position reader. Normally points to the active engine's
    /// position counter. Swapped at crossfade promotion so the handle always
    /// reads from the engine that's audibly current, not the fading-out one.
    ///
    /// Real-time-safety / unwrap audit (Task 15): every access site uses
    /// `position_source.lock().unwrap()` because the protected payload is an
    /// `Arc<AtomicU64>` - mutex poisoning leaves it valid (Arc is either the
    /// old reference or the new one, both safe to read/write). The only way
    /// `.unwrap()` panics is if a code path inside the guard panics first,
    /// which is then caught by `handle_panic_in_runtime_loop` (Task 7) and
    /// surfaced to the user as `PlaybackRuntimeEvent::Error`. So these
    /// unwraps are bounded-failure, not silent corruption.
    position_source: Arc<Mutex<Arc<AtomicU64>>>,
    /// Redirectable buffered-samples reader. Parallel to `position_source`:
    /// always points at the audibly-current engine's `buffered_samples`
    /// counter, swapped at the same sites position_source is swapped (cold
    /// start in `transition_to_job` and at the two `promote_*` sites). The
    /// route-side seek ack reads through this to decide 409 vs dispatch, and
    /// the frontend reads `buffered_ms` via this for the buffered-bar
    /// scrubber. Same unwrap-audit reasoning as `position_source`.
    buffered_source: Arc<Mutex<Arc<AtomicU64>>>,
    /// Redirectable engine-offset reader (option C: true DASH segment seek).
    /// Points at the audibly-current engine's `position_offset_samples`
    /// counter. For a fresh play this reads 0; for a segment-restart engine
    /// it reads the absolute-track sample where the engine's decoded audio
    /// starts. The route-side seek handler uses this as the LOWER bound of
    /// the in-buffer decision (target must be `>= offset` to be in-buffer);
    /// the frontend reads `buffered_start_ms` via this as a visual cue.
    /// Same unwrap-audit reasoning as `position_source` / `buffered_source`.
    offset_source: Arc<Mutex<Arc<AtomicU64>>>,
}

impl PlaybackRuntimeHandle {
    #[cfg(test)]
    pub(crate) fn test_with_command_tx(command_tx: mpsc::Sender<PlaybackRuntimeCommand>) -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        Self {
            command_tx,
            event_tx,
            volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            buffered_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            offset_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
        }
    }

    pub fn play(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Play(job))
    }

    pub fn switch_to(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Switch(job))
    }

    pub fn pause(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Pause)
    }

    pub fn resume(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Resume)
    }

    pub fn stop(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Stop)
    }

    pub fn shutdown(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Shutdown)
    }

    /// Live-swap the CPAL output device (and exclusive / sample-rate-follow flags)
    /// across any active engines. Used by the audio settings PUT route and track
    /// transitions when sample_rate_follow is enabled. Optional desired_sample_rate
    /// allows specifying an exact target (e.g. next track's native rate).
    /// `exclusive_release_grace_secs` is the idle-release grace window for the
    /// WASAPI exclusive render thread (ignored unless `exclusive` is true).
    pub fn device_swap(
        &self,
        device: OutputDeviceSelection,
        exclusive: bool,
        sample_rate_follow: bool,
        desired_sample_rate: Option<u32>,
        exclusive_release_grace_secs: u32,
        exclusive_latency_mode: ExclusiveLatencyMode,
    ) -> Result<()> {
        self.send(PlaybackRuntimeCommand::DeviceSwap {
            device,
            exclusive,
            sample_rate_follow,
            desired_sample_rate,
            exclusive_release_grace_secs,
            exclusive_latency_mode,
        })
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PlaybackRuntimeEvent> {
        self.event_tx.subscribe()
    }

    /// Segment-aware seek. Single entry point for all seek requests; the
    /// runtime decides between in-buffer fast path, forced-restart segment
    /// seek, or rejection. The `allow_segment_seek` flag opts in to the
    /// segment-restart transition; with `false`, the runtime treats
    /// out-of-buffer seeks as rejected (legacy semantics).
    ///
    /// Blocks up to 1500ms for the reply (segment-restart transitions need
    /// time to tear down the old engine and spin up the decoder thread on
    /// the new one). Returns `SeekToOutcome::Failed` on timeout / channel
    /// closure - treat as a recoverable error from the caller's perspective.
    pub fn seek_to_segment_aware(
        &self,
        position_ms: i64,
        allow_segment_seek: bool,
    ) -> SeekToOutcome {
        let (tx, rx) = std::sync::mpsc::channel();
        if self
            .send(PlaybackRuntimeCommand::SeekTo {
                target_ms: position_ms,
                allow_segment_seek,
                respond_to: tx,
            })
            .is_err()
        {
            return SeekToOutcome::Failed;
        }
        rx.recv_timeout(std::time::Duration::from_millis(1500))
            .unwrap_or(SeekToOutcome::Failed)
    }

    /// Legacy seek wrapper. Equivalent to `seek_to_segment_aware(position_ms,
    /// false)` returning a `Result<()>`. Kept so non-route callers (none
    /// today; audit `git grep "\\.seek\\("` if adding new ones) don't have to
    /// adopt the SeekToOutcome enum just to issue a plain seek. Out-of-buffer
    /// or failed transitions surface as `Err`.
    pub fn seek(&self, position_ms: i64) -> Result<()> {
        match self.seek_to_segment_aware(position_ms, false) {
            SeekToOutcome::Dispatched | SeekToOutcome::DispatchedCrossfadeSuppressed => Ok(()),
            SeekToOutcome::RejectedOutOfBuffer => Err(anyhow!("seek target is out of buffer")),
            SeekToOutcome::Failed => Err(anyhow!("seek dispatch failed")),
        }
    }

    /// Pre-decode the next track in the background so the transition is gapless.
    pub fn prepare_next(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::PrepareNext(job))
    }

    pub fn prepare_drop_preview(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::PrepareDropPreview(job))
    }

    pub fn arm_drop_preview(
        &self,
        track_id: i64,
        generation: u64,
        trigger_position_samples: u64,
    ) -> Result<()> {
        self.send(PlaybackRuntimeCommand::ArmDropPreview {
            track_id,
            generation,
            trigger_position_samples,
        })
    }

    pub fn set_dj_engine_enabled(&self, enabled: bool) -> Result<()> {
        self.send(PlaybackRuntimeCommand::SetDjEngineEnabled { enabled })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_dj_lookahead(
        &self,
        current: Option<DjMediaRef>,
        next: Option<DjMediaRef>,
        current_queue_item_id: Option<i64>,
        next_queue_item_id: Option<i64>,
        queue_generation: u64,
        deadline_samples: u64,
    ) -> Result<()> {
        self.send(PlaybackRuntimeCommand::StartDjLookahead {
            current,
            next,
            current_queue_item_id,
            next_queue_item_id,
            queue_generation,
            deadline_samples,
        })
    }

    pub fn track_status(&self, track_id: i64, generation: u64) -> PlaybackTrackStatus {
        let (tx, rx) = std::sync::mpsc::channel();
        if self
            .send(PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to: tx,
            })
            .is_err()
        {
            return PlaybackTrackStatus::None;
        }
        rx.recv_timeout(std::time::Duration::from_millis(100))
            .unwrap_or(PlaybackTrackStatus::None)
    }

    /// Set playback volume (0.0 = silent, 1.0 = full). Applied immediately to the CPAL callback.
    pub fn set_volume(&self, volume: f32) {
        self.volume_ctl
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Read the current playback position in milliseconds from the CPAL sample counter.
    pub fn get_position_ms(&self, device_sample_rate: u32, device_channels: u16) -> i64 {
        if device_sample_rate == 0 || device_channels == 0 {
            return 0;
        }
        let samples = self.position_source.lock().unwrap().load(Ordering::Relaxed);
        (samples * 1000 / (device_sample_rate as u64 * device_channels as u64)) as i64
    }

    /// Read how many ms of the current track are decoded into the playback
    /// buffer. Returns 0 when no engine is active. Used by the route-side
    /// seek ack (target > buffered -> HTTP 409) and surfaced to the frontend
    /// via `PlaybackState.buffered_ms` for the buffered-bar scrubber.
    /// Same unwrap-audit reasoning as `get_position_ms`.
    pub fn get_buffered_ms(&self, device_sample_rate: u32, device_channels: u16) -> i64 {
        if device_sample_rate == 0 || device_channels == 0 {
            return 0;
        }
        let samples = self.buffered_source.lock().unwrap().load(Ordering::Relaxed);
        (samples * 1000 / (device_sample_rate as u64 * device_channels as u64)) as i64
    }

    /// Raw buffered-sample count from the audibly-current engine. Avoids the
    /// ms conversion when the caller already has a target-in-samples (e.g.
    /// the route-side seek handler comparing target_samples to buffered).
    pub fn buffered_samples(&self) -> u64 {
        self.buffered_source.lock().unwrap().load(Ordering::Relaxed)
    }

    /// Read the engine's track-time offset in milliseconds (lower bound of
    /// the decoded range). Returns 0 for a fresh-from-start engine; returns
    /// the segment offset for a segment-restart engine. Read via the
    /// redirectable `offset_source` so it always reflects the audibly-current
    /// engine, not the fading-out one. Used by `build_live_playback_snapshot`
    /// to populate `PlaybackState.buffered_start_ms` and by the runtime's
    /// SeekTo handler indirectly via `evaluate_seek_decision`.
    pub fn get_buffered_start_ms(&self, device_sample_rate: u32, device_channels: u16) -> i64 {
        if device_sample_rate == 0 || device_channels == 0 {
            return 0;
        }
        let samples = self.offset_source.lock().unwrap().load(Ordering::Relaxed);
        (samples * 1000 / (device_sample_rate as u64 * device_channels as u64)) as i64
    }

    /// Raw offset-sample count from the audibly-current engine. Companion to
    /// `buffered_samples()`; the route-side SeekTo handler uses both as the
    /// `[offset, buffered]` bounds of the in-buffer fast path.
    pub fn buffered_start_samples(&self) -> u64 {
        self.offset_source.lock().unwrap().load(Ordering::Relaxed)
    }

    fn send(&self, command: PlaybackRuntimeCommand) -> Result<()> {
        self.command_tx
            .send(command)
            .map_err(|_| anyhow!("playback runtime command channel closed"))
    }
}

pub fn spawn_runtime(config: PlaybackRuntimeConfig) -> Result<PlaybackRuntimeHandle> {
    // Real-time safety: this channel MUST remain unbounded
    // (std::sync::mpsc::channel, NOT sync_channel). The CPAL audio callback
    // in shared.rs::write_output_buffer sends TrackTerminal and
    // CrossfadeStart commands through command_tx; a bounded channel would
    // block the audio thread on a full buffer and cause dropouts/underruns.
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, _) = tokio::sync::broadcast::channel(256);
    let worker_event_tx = event_tx.clone();
    let worker_command_tx = command_tx.clone();

    let volume_ctl = Arc::new(AtomicU32::new(1.0f32.to_bits())); // default: full volume
    // `initial_position` is the counter cold-start engines write into.
    // `position_source` wraps it in a Mutex so promote_next_to_active can
    // redirect the handle to the promoted engine's private counter instead.
    let initial_position = Arc::new(AtomicU64::new(0));
    let position_source = Arc::new(Mutex::new(Arc::clone(&initial_position)));
    // Buffered-samples mirror. Each PlaybackSharedState owns its own
    // `Arc<AtomicU64>` (initialized to 0 at construction); the handle's
    // `buffered_source` points at whichever one is audibly current. Before
    // any engine exists we point at a sentinel zero atomic so a
    // `buffered_ms()` call returns 0 cleanly.
    let buffered_source: Arc<Mutex<Arc<AtomicU64>>> =
        Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));
    // Offset mirror: same redirect pattern as `buffered_source`, but for the
    // engine's `position_offset_samples`. Before any engine exists this points
    // at a sentinel zero atomic so `get_buffered_start_ms()` returns 0.
    let offset_source: Arc<Mutex<Arc<AtomicU64>>> =
        Arc::new(Mutex::new(Arc::new(AtomicU64::new(0))));

    let worker_volume_ctl = Arc::clone(&volume_ctl);
    let worker_initial_position = Arc::clone(&initial_position);
    let worker_position_source = Arc::clone(&position_source);
    let worker_buffered_source = Arc::clone(&buffered_source);
    let worker_offset_source = Arc::clone(&offset_source);

    thread::Builder::new()
        .name("noor-playback-runtime".into())
        .spawn(move || {
            if let Err(err) = run_runtime_loop(
                config,
                command_rx,
                worker_command_tx,
                worker_event_tx.clone(),
                worker_volume_ctl,
                worker_initial_position,
                worker_position_source,
                worker_buffered_source,
                worker_offset_source,
            ) {
                let _ = worker_event_tx.send(PlaybackRuntimeEvent::Error {
                    message: err.to_string(),
                });
                error!("Playback runtime stopped: {err:?}");
            }
        })
        .context("failed to spawn playback runtime thread")?;

    Ok(PlaybackRuntimeHandle {
        command_tx,
        event_tx,
        volume_ctl,
        position_source,
        buffered_source,
        offset_source,
    })
}

struct PlaybackRuntimeLoopState {
    device_name: String,
    device_sample_rate: u32,
    device_channels: u16,
    #[cfg(target_os = "windows")]
    exclusive_sink: ExclusiveRuntimeSink,
    /// Currently-audible "primary" engine. After a crossfade swap this is the
    /// incoming track; before any swap it's whatever was last started.
    engine: Option<PlaybackEngine>,
    /// Pre-decoded engine for the next track, paused until the crossfade
    /// window opens. Once unpaused it gets promoted to `engine` and the old
    /// `engine` slides into `fading_out_engine`.
    next_engine: Option<PlaybackEngine>,
    /// Temporary incoming deck for a mid-song drop preview. It is never
    /// promoted and is discarded after the preview overlay finishes.
    drop_preview_engine: Option<PlaybackEngine>,
    /// Engine that's still audible during the crossfade fade-out window. It
    /// keeps producing audio (with a fade-out gain ramp) until its buffer
    /// drains, at which point it self-terminates and we drop it silently -
    /// the queue advance has already happened at swap time.
    fading_out_engine: Option<PlaybackEngine>,
    /// Last-known exclusive mode flag from the most recent `DeviceSwap`. When
    /// `true`, freshly cold-started engines immediately swap to the WASAPI
    /// exclusive backend so the user's "bit-perfect" toggle stays in effect
    /// across track boundaries (otherwise every new track would silently
    /// fall back to cpal shared mode).
    current_exclusive: bool,
    /// Last-known sample-rate-follow flag, used the same way as `current_exclusive`.
    current_sample_rate_follow: bool,
    /// Last-known device selection for cold-started engines.
    current_device_selection: OutputDeviceSelection,
    /// Last-known idle-release grace seconds for the WASAPI exclusive render
    /// thread. Used when re-grabbing exclusive on Resume/Play after the render
    /// thread released the device, and when cold-starting new engines.
    current_exclusive_release_grace_secs: u32,
    /// Last-known WASAPI exclusive callback period policy.
    current_exclusive_latency_mode: ExclusiveLatencyMode,
    dj_engine_enabled: bool,
    dj_lookahead: Option<RuntimeDjLookahead>,
    dj_lookahead_failure: Option<DjLookaheadFailure>,
    prepared_dj_mixer: Option<PreparedDjMixer>,
    prepared_drop_preview_mixer: Option<PreparedDjMixer>,
    last_dj_renderer_failure: Option<DjRuntimeRendererFailure>,
}

struct PreparedDjMixer {
    mixer: noor_mix::Mixer,
    program: noor_mix::TransitionProgram,
    max_block_samples: usize,
    queue_generation: u64,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
    current_track_id: i64,
    next_track_id: i64,
}

struct RuntimeDeckSnapshot {
    deck: noor_mix::deck::DeckBuffer,
    start_frame: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeDjLookahead {
    current: Option<DjMediaRef>,
    next: DjMediaRef,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: i64,
    queue_generation: u64,
    deadline_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DjLookaheadFailure {
    queue_generation: u64,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
    reason: DjLookaheadFailureReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DjRuntimeRendererFailure {
    queue_generation: u64,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
    transition_event_id: Option<i64>,
    current_track_id: Option<i64>,
    next_track_id: Option<i64>,
    reason: DjRuntimeRendererReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DjRuntimeRendererStatus {
    RenderedHandoff,
    RenderedOverlay,
    LegacyOverlap,
    BoundaryFallback,
}

impl DjRuntimeRendererStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::RenderedHandoff => "rendered_handoff",
            Self::RenderedOverlay => "rendered_overlay",
            Self::LegacyOverlap => "legacy_overlap",
            Self::BoundaryFallback => "boundary_fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DjRuntimeRendererReason {
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
}

impl DjRuntimeRendererReason {
    fn as_str(self) -> &'static str {
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DjRuntimeRendererOutcome {
    rendered: bool,
    status: DjRuntimeRendererStatus,
    reason: DjRuntimeRendererReason,
}

impl DjRuntimeRendererOutcome {
    fn rendered_handoff() -> Self {
        Self {
            rendered: true,
            status: DjRuntimeRendererStatus::RenderedHandoff,
            reason: DjRuntimeRendererReason::None,
        }
    }

    fn rendered_handoff_with_reason(reason: DjRuntimeRendererReason) -> Self {
        Self {
            rendered: true,
            status: DjRuntimeRendererStatus::RenderedHandoff,
            reason,
        }
    }

    fn rendered_overlay() -> Self {
        Self {
            rendered: true,
            status: DjRuntimeRendererStatus::RenderedOverlay,
            reason: DjRuntimeRendererReason::None,
        }
    }

    fn legacy_overlap(reason: DjRuntimeRendererReason) -> Self {
        Self {
            rendered: false,
            status: DjRuntimeRendererStatus::LegacyOverlap,
            reason,
        }
    }

    fn boundary_fallback(reason: DjRuntimeRendererReason) -> Self {
        Self {
            rendered: false,
            status: DjRuntimeRendererStatus::BoundaryFallback,
            reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum DjLookaheadFailureReason {
    NextNotResolved,
    ResolutionFailed,
    AnalysisDeadlineMissed,
    QueueChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartDjLookaheadOutcome {
    Started,
    AlreadyCurrent,
    ReusedPreparedNext,
    MissingNext,
}

impl RuntimeDjLookahead {
    fn matches_pair(
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

fn runtime_renderer_failure_reason(
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

fn runtime_renderer_fire_block_reason(
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

fn runtime_renderer_boundary_fallback_reason(
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

fn runtime_renderer_late_fire_reason(state: &PlaybackRuntimeLoopState) -> DjRuntimeRendererReason {
    let reason =
        runtime_renderer_failure_reason(state, DjRuntimeRendererReason::PreparedMixerMissing);
    if reason == DjRuntimeRendererReason::PreparedMixerMissing {
        DjRuntimeRendererReason::NextDecodeLateAtFire
    } else {
        reason
    }
}

fn record_runtime_renderer_failure(
    state: &mut PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    reason: DjRuntimeRendererReason,
) {
    state.last_dj_renderer_failure = Some(DjRuntimeRendererFailure {
        queue_generation: transition.queue_generation,
        current_queue_item_id: transition.current_queue_item_id,
        next_queue_item_id: transition.next_queue_item_id,
        transition_event_id: transition.transition_event_id,
        current_track_id: state.engine.as_ref().map(|engine| engine.track_id),
        next_track_id: state.next_engine.as_ref().map(|engine| engine.track_id),
        reason,
    });
}

fn record_current_runtime_renderer_failure(
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

fn renderer_failure_matches_current_transition(
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
}

fn start_dj_lookahead_in_state(
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

fn prepared_dj_lookahead_matches_pair(
    state: &PlaybackRuntimeLoopState,
    queue_generation: u64,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
) -> bool {
    state.dj_lookahead.as_ref().is_some_and(|lookahead| {
        lookahead.matches_pair(queue_generation, current_queue_item_id, next_queue_item_id)
    })
}

fn discard_stale_prepared_transition(
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

fn dj_mixer_max_block_samples(output_config: &StreamConfig) -> usize {
    let channels = usize::from(output_config.channels.max(1));
    match output_config.buffer_size {
        cpal::BufferSize::Fixed(frames) => frames as usize * channels,
        cpal::BufferSize::Default => DJ_MIXER_DEFAULT_MAX_BLOCK_FRAMES * channels,
    }
}

fn dj_renderer_late_tolerance_frames(sample_rate: u32) -> u64 {
    u64::from(sample_rate.max(1)) / 2
}

fn decoded_deck_snapshot(
    engine: &PlaybackEngine,
    channels: u16,
    start_frame: u64,
    required_frames: u64,
    late_tolerance_frames: u64,
    reason: DjRuntimeRendererReason,
) -> Result<RuntimeDeckSnapshot, DjRuntimeRendererReason> {
    let guard = engine
        .shared
        .buffer
        .lock()
        .map_err(|_| DjRuntimeRendererReason::BufferLockFailed)?;
    if guard.samples.is_empty() {
        return Err(reason);
    }
    let channels = usize::from(channels.max(1));
    let frames = (guard.samples.len() / channels) as u64;
    if start_frame >= frames {
        return Err(reason);
    }
    let available_frames = frames.saturating_sub(start_frame);
    if available_frames.saturating_add(late_tolerance_frames) < required_frames.max(1) {
        return Err(reason);
    }
    Ok(RuntimeDeckSnapshot {
        deck: noor_mix::deck::DeckBuffer::new(guard.samples.clone(), channels as u16),
        start_frame,
    })
}

fn active_deck_snapshot(
    engine: &PlaybackEngine,
    channels: u16,
    program_start_frame: u64,
    required_frames: u64,
    late_tolerance_frames: u64,
) -> Result<RuntimeDeckSnapshot, DjRuntimeRendererReason> {
    let start_frame = {
        let guard = engine
            .shared
            .buffer
            .lock()
            .map_err(|_| DjRuntimeRendererReason::BufferLockFailed)?;
        if program_start_frame == 0 {
            let channels = usize::from(channels.max(1));
            (guard.read_pos / channels) as u64
        } else {
            program_start_frame
        }
    };
    decoded_deck_snapshot(
        engine,
        channels,
        start_frame,
        required_frames,
        late_tolerance_frames,
        DjRuntimeRendererReason::ActiveDeckNotDecoded,
    )
}

fn build_prepared_dj_mixer(
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

fn build_prepared_dj_mixer_for_engine(
    state: &PlaybackRuntimeLoopState,
    transition: &PreparedTransitionProgram,
    incoming: &PlaybackEngine,
    max_block_samples: usize,
) -> Result<PreparedDjMixer, DjRuntimeRendererReason> {
    let active = state
        .engine
        .as_ref()
        .ok_or(DjRuntimeRendererReason::ActiveDeckNotDecoded)?;
    if !handoff_mixer_program(&transition.program) && !overlay_mixer_program(&transition.program) {
        return Err(DjRuntimeRendererReason::ProgramNotMixerRenderable);
    }
    let mut program = transition.program.clone();
    let deck_b_consumed_frames = deck_b_consumed_frames(&program)
        .ok_or(DjRuntimeRendererReason::ProgramNotMixerRenderable)?;
    let late_tolerance_frames = dj_renderer_late_tolerance_frames(state.device_sample_rate);
    let active_snapshot = active_deck_snapshot(
        active,
        state.device_channels,
        program.deck_a_start_frame,
        program.resolve_at,
        late_tolerance_frames,
    )?;
    let next_snapshot = decoded_deck_snapshot(
        incoming,
        state.device_channels,
        program.deck_b_start_frame,
        deck_b_consumed_frames.saturating_add(1),
        late_tolerance_frames,
        DjRuntimeRendererReason::NextDeckNotDecoded,
    )?;
    program.deck_a_start_frame = active_snapshot.start_frame;
    program.deck_b_start_frame = next_snapshot.start_frame;
    let mixer = match noor_mix::Mixer::new(
        program.clone(),
        active_snapshot.deck,
        next_snapshot.deck,
        max_block_samples,
    ) {
        Ok(mixer) => mixer,
        Err(error) => {
            warn!("Prepared DJ mixer rejected transition program: {error:?}");
            return Err(DjRuntimeRendererReason::MixerRejected);
        }
    };
    Ok(PreparedDjMixer {
        mixer,
        program,
        max_block_samples,
        queue_generation: transition.queue_generation,
        current_queue_item_id: transition.current_queue_item_id,
        next_queue_item_id: transition.next_queue_item_id,
        current_track_id: active.track_id,
        next_track_id: incoming.track_id,
    })
}

fn handoff_mixer_program(program: &noor_mix::TransitionProgram) -> bool {
    matches!(
        program.template.as_str(),
        "SafeCrossfade"
            | "BassSwap16"
            | "BassSwap32"
            | "LongHarmonicBlend"
            | "FilterSweep"
            | "SlamCut"
    ) && deck_b_consumed_frames(program).is_some()
}

fn overlay_mixer_program(program: &noor_mix::TransitionProgram) -> bool {
    matches!(program.template.as_str(), "DropTease16" | "DropPreview16")
        && deck_b_consumed_frames(program).is_some()
}

fn render_prepared_mixer_to_buffer(
    prepared: &mut PreparedDjMixer,
    channels: usize,
) -> Option<Vec<f32>> {
    let render_frames = prepared.program.resolve_at as usize;
    let render_samples = render_frames.checked_mul(channels)?;
    if render_samples == 0 {
        return None;
    }
    let block_samples = prepared
        .max_block_samples
        .max(channels)
        .saturating_sub(prepared.max_block_samples.max(channels) % channels)
        .max(channels);
    let mut rendered = vec![0.0; render_samples];
    let mut master_frame = 0_u64;
    for block in rendered.chunks_mut(block_samples) {
        prepared.mixer.render_block(block, master_frame);
        master_frame = master_frame.saturating_add((block.len() / channels) as u64);
    }
    Some(rendered)
}

fn deck_b_consumed_frames(program: &noor_mix::TransitionProgram) -> Option<u64> {
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

fn install_prepared_handoff_mixer_buffer(
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

    let mut prepared = state
        .prepared_dj_mixer
        .take()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    let channels = usize::from(state.device_channels.max(1));
    let mut rendered = render_prepared_mixer_to_buffer(&mut prepared, channels)
        .ok_or(DjRuntimeRendererReason::RenderBufferFailed)?;

    let next = state
        .next_engine
        .as_ref()
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
    let remainder = guard.samples[remainder_start..].to_vec();
    rendered.extend_from_slice(&remainder);
    guard.samples = rendered;
    guard.read_pos = 0;
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
        previous_total_samples.max(rendered_total_samples)
    };
    next.shared
        .total_samples
        .store(total_samples, Ordering::Relaxed);
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

fn install_prepared_overlay_mixer_buffer(
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

    let mut prepared = state
        .prepared_dj_mixer
        .take()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    let channels = usize::from(state.device_channels.max(1));
    let rendered = render_prepared_mixer_to_buffer(&mut prepared, channels)
        .ok_or(DjRuntimeRendererReason::RenderBufferFailed)?;
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

fn install_prepared_drop_preview_mixer_buffer(
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

    let mut prepared = state
        .prepared_drop_preview_mixer
        .take()
        .ok_or(DjRuntimeRendererReason::PreparedMixerMissing)?;
    let channels = usize::from(state.device_channels.max(1));
    let rendered = render_prepared_mixer_to_buffer(&mut prepared, channels)
        .ok_or(DjRuntimeRendererReason::RenderBufferFailed)?;
    let preview = state
        .drop_preview_engine
        .as_ref()
        .ok_or(DjRuntimeRendererReason::NextDeckNotDecoded)?;
    let mut guard = match preview.shared.buffer.lock() {
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
    preview
        .shared
        .total_samples
        .store(guard.samples.len() as u64, Ordering::Relaxed);
    preview.shared.publish_buffered_samples(guard.samples.len());
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

fn prepare_dj_mixer_for_pair(
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
            Ok(())
        }
        Err(reason) => {
            state.prepared_dj_mixer = None;
            record_runtime_renderer_failure(state, &transition, reason);
            Err(reason)
        }
    }
}

fn prepare_drop_preview_mixer(
    state: &mut PlaybackRuntimeLoopState,
    max_block_samples: usize,
) -> Result<(), DjRuntimeRendererReason> {
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

fn prepared_overlay_program(state: &PlaybackRuntimeLoopState) -> bool {
    state
        .prepared_dj_mixer
        .as_ref()
        .is_some_and(|prepared| overlay_mixer_program(&prepared.program))
}

fn start_prepared_overlay(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    timing_status: &'static str,
    runtime_renderer_reason: DjRuntimeRendererReason,
    actual_start_ms_override: Option<i64>,
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
    next.shared.paused.store(false, Ordering::SeqCst);
    if let Some(transition_event_id) = transition_event_id {
        let _ = event_tx.send(PlaybackRuntimeEvent::DjTransitionPromoted {
            transition_event_id,
            outgoing_track_id,
            generation: outgoing_generation,
            actual_start_ms,
            timing_status: timing_status.to_string(),
            runtime_rendered_dj_mixer: true,
            runtime_renderer_status: DjRuntimeRendererOutcome::rendered_overlay()
                .status
                .as_str()
                .to_string(),
            runtime_renderer_reason: runtime_renderer_reason.as_str().to_string(),
        });
    }
    Ok(())
}

fn start_prepared_drop_preview_overlay(
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
    preview.shared.paused.store(false, Ordering::SeqCst);
    let _ = event_tx.send(PlaybackRuntimeEvent::DropPreviewStarted {
        track_id: active_track_id,
        generation: active_generation,
        actual_start_ms,
    });
    Ok(())
}

fn arm_active_transition_window(
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
    engine
        .shared
        .crossfade_samples
        .store(samples, Ordering::Relaxed);
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
        "DJ transition window armed"
    );
    true
}

fn arm_drop_preview_in_state(
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
    active
        .shared
        .drop_preview_trigger_samples
        .store(trigger_position_samples, Ordering::Relaxed);
    active
        .shared
        .drop_preview_start_signaled
        .store(false, Ordering::Relaxed);
    true
}

fn gate_prepare_next_for_dj(
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

fn set_dj_engine_enabled_in_state(state: &mut PlaybackRuntimeLoopState, enabled: bool) {
    state.dj_engine_enabled = enabled;
    if enabled {
        return;
    }
    state.dj_lookahead = None;
    state.dj_lookahead_failure = None;
    state.prepared_dj_mixer = None;
    state.prepared_drop_preview_mixer = None;
    state.last_dj_renderer_failure = None;
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

fn record_dj_lookahead_failure(
    state: &mut PlaybackRuntimeLoopState,
    queue_generation: u64,
    current_queue_item_id: Option<i64>,
    next_queue_item_id: Option<i64>,
    reason: DjLookaheadFailureReason,
) {
    state.dj_lookahead_failure = Some(DjLookaheadFailure {
        queue_generation,
        current_queue_item_id,
        next_queue_item_id,
        reason,
    });
}

#[allow(clippy::too_many_arguments)]
fn run_runtime_loop(
    mut config: PlaybackRuntimeConfig,
    command_rx: mpsc::Receiver<PlaybackRuntimeCommand>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    volume_ctl: Arc<AtomicU32>,
    position_samples: Arc<AtomicU64>,
    position_source: Arc<Mutex<Arc<AtomicU64>>>,
    buffered_source: Arc<Mutex<Arc<AtomicU64>>>,
    offset_source: Arc<Mutex<Arc<AtomicU64>>>,
) -> Result<()> {
    let host = cpal::default_host();
    let mut device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default output device available"))?;
    let device_name = device_display_name(&device);
    let supported = device
        .default_output_config()
        .context("failed to read default output config")?;
    let mut output_config = supported.config();
    let mut output_sample_format = supported.sample_format();

    let mut state = PlaybackRuntimeLoopState {
        device_name,
        device_sample_rate: output_config.sample_rate,
        device_channels: output_config.channels,
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
        dj_engine_enabled: config.dj_engine_enabled,
        dj_lookahead: None,
        dj_lookahead_failure: None,
        prepared_dj_mixer: None,
        prepared_drop_preview_mixer: None,
        last_dj_renderer_failure: None,
    };

    let _ = event_tx.send(PlaybackRuntimeEvent::Ready {
        device_name: state.device_name.clone(),
        sample_rate: state.device_sample_rate,
        channels: state.device_channels,
    });

    info!(
        "Playback runtime ready on {} at {} Hz / {} channels / {:?}",
        state.device_name, state.device_sample_rate, state.device_channels, output_sample_format
    );

    while let Ok(command) = command_rx.recv() {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match command {
                PlaybackRuntimeCommand::Play(job) => {
                    if let Err(error) = transition_to_job(
                        &config,
                        &command_tx,
                        &device,
                        &mut output_config,
                        output_sample_format,
                        &event_tx,
                        &mut state,
                        job,
                        &volume_ctl,
                        &position_samples,
                        &position_source,
                        &buffered_source,
                        &offset_source,
                        true,
                    ) {
                        stop_all_engines(&mut state);
                        #[cfg(target_os = "windows")]
                        state.exclusive_sink.clear();
                        report_runtime_command_error(&event_tx, "Play", error);
                    }
                }
                PlaybackRuntimeCommand::Switch(job) => {
                    if let Err(error) = transition_to_job(
                        &config,
                        &command_tx,
                        &device,
                        &mut output_config,
                        output_sample_format,
                        &event_tx,
                        &mut state,
                        job,
                        &volume_ctl,
                        &position_samples,
                        &position_source,
                        &buffered_source,
                        &offset_source,
                        false,
                    ) {
                        stop_all_engines(&mut state);
                        #[cfg(target_os = "windows")]
                        state.exclusive_sink.clear();
                        report_runtime_command_error(&event_tx, "Switch", error);
                    }
                }
                PlaybackRuntimeCommand::SeekTo {
                    target_ms,
                    allow_segment_seek,
                    respond_to,
                } => {
                    // Phase 1: snapshot everything we need under an immutable
                    // borrow of state.engine. Inside this block we decide
                    // among in-buffer fast path / segment-restart / reject;
                    // the actual mutation happens in phase 2 with the
                    // immutable borrow already dropped (per r6 fix C).
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
                        let offset_samples = engine
                            .shared
                            .position_offset_samples
                            .load(Ordering::Relaxed);
                        let buffered_samples =
                            engine.shared.buffered_samples.load(Ordering::Relaxed);

                        match evaluate_seek_decision(
                            target_samples,
                            offset_samples,
                            buffered_samples,
                            true,
                        ) {
                            SeekDecision::Dispatch => SeekHandling::InBuffer { target_samples },
                            SeekDecision::RejectOutOfBuffer if !allow_segment_seek => {
                                SeekHandling::Reject
                            }
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
                                engine
                                    .shared
                                    .seek_target_samples
                                    .store(target_samples, Ordering::Relaxed);
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
                                &config,
                                &command_tx,
                                &device,
                                &mut output_config,
                                output_sample_format,
                                &event_tx,
                                &mut state,
                                job,
                                &volume_ctl,
                                &position_samples,
                                &position_source,
                                &buffered_source,
                                &offset_source,
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
                }
                PlaybackRuntimeCommand::PrepareNext(mut job) => {
                    gate_prepare_next_for_dj(&mut state, &mut job);
                    arm_active_transition_window(&mut state, &job);
                    // Only pre-decode if we don't already have a pending engine for this track.
                    let already_pending = state
                        .next_engine
                        .as_ref()
                        .map(|e| e.track_id == job.track.id && e.generation == job.generation)
                        .unwrap_or(false);
                    if !already_pending {
                        // Stop any stale pending engine first.
                        if let Some(mut stale) = state.next_engine.take() {
                            state.prepared_dj_mixer = None;
                            stale.stop();
                        }
                        let pending_position = Arc::new(AtomicU64::new(0));
                        let engine_result = if state.current_exclusive {
                            PlaybackEngine::start_decoder_only(
                                &config,
                                &command_tx,
                                job,
                                state.device_sample_rate,
                                state.device_channels,
                                Arc::clone(&volume_ctl),
                                pending_position,
                            )
                        } else {
                            PlaybackEngine::start(
                                &config,
                                &command_tx,
                                &device,
                                &output_config,
                                output_sample_format,
                                job,
                                event_tx.clone(),
                                state.device_sample_rate,
                                state.device_channels,
                                Arc::clone(&volume_ctl),
                                pending_position,
                            )
                        };
                        match engine_result {
                            Ok(engine) => {
                                // Keep the stream alive but software-paused so host pause does not
                                // block control commands on some Linux/PipeWire setups.
                                engine.shared.paused.store(true, Ordering::SeqCst);
                                state.next_engine = Some(engine);
                                let _ = prepare_dj_mixer_for_pair(
                                    &mut state,
                                    dj_mixer_max_block_samples(&output_config),
                                );
                                #[cfg(target_os = "windows")]
                                if state.current_exclusive {
                                    refresh_exclusive_sources(&state);
                                }
                            }
                            Err(err) => {
                                warn!("Failed to pre-buffer next track: {err:?}");
                            }
                        }
                    }
                }
                PlaybackRuntimeCommand::PrepareDropPreview(job) => {
                    if !state.dj_engine_enabled {
                        state.prepared_drop_preview_mixer = None;
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
                        state.prepared_drop_preview_mixer = None;
                        if let Some(mut stale) = state.drop_preview_engine.take() {
                            stale.stop();
                        }
                        return std::ops::ControlFlow::Continue(());
                    }
                    let already_pending = state
                        .drop_preview_engine
                        .as_ref()
                        .map(|engine| {
                            engine.track_id == job.track.id && engine.generation == job.generation
                        })
                        .unwrap_or(false);
                    if !already_pending {
                        if let Some(mut stale) = state.drop_preview_engine.take() {
                            state.prepared_drop_preview_mixer = None;
                            stale.stop();
                        }
                        let pending_position = Arc::new(AtomicU64::new(0));
                        let engine_result = if state.current_exclusive {
                            PlaybackEngine::start_decoder_only(
                                &config,
                                &command_tx,
                                job,
                                state.device_sample_rate,
                                state.device_channels,
                                Arc::clone(&volume_ctl),
                                pending_position,
                            )
                        } else {
                            PlaybackEngine::start(
                                &config,
                                &command_tx,
                                &device,
                                &output_config,
                                output_sample_format,
                                job,
                                event_tx.clone(),
                                state.device_sample_rate,
                                state.device_channels,
                                Arc::clone(&volume_ctl),
                                pending_position,
                            )
                        };
                        match engine_result {
                            Ok(engine) => {
                                engine.shared.suppress_started_event();
                                engine.shared.paused.store(true, Ordering::SeqCst);
                                state.drop_preview_engine = Some(engine);
                                let _ = prepare_drop_preview_mixer(
                                    &mut state,
                                    dj_mixer_max_block_samples(&output_config),
                                );
                                #[cfg(target_os = "windows")]
                                if state.current_exclusive {
                                    refresh_exclusive_sources(&state);
                                }
                            }
                            Err(err) => {
                                warn!("Failed to pre-buffer drop preview: {err:?}");
                            }
                        }
                    }
                }
                PlaybackRuntimeCommand::SetDjEngineEnabled { enabled } => {
                    config.dj_engine_enabled = enabled;
                    set_dj_engine_enabled_in_state(&mut state, enabled);
                }
                PlaybackRuntimeCommand::StartDjLookahead {
                    current,
                    next,
                    current_queue_item_id,
                    next_queue_item_id,
                    queue_generation,
                    deadline_samples,
                } => {
                    if !state.dj_engine_enabled {
                        state.dj_lookahead = None;
                        state.prepared_dj_mixer = None;
                    } else {
                        let outcome = start_dj_lookahead_in_state(
                            &mut state,
                            current,
                            next,
                            current_queue_item_id,
                            next_queue_item_id,
                            queue_generation,
                            deadline_samples,
                        );
                        if matches!(outcome, StartDjLookaheadOutcome::MissingNext) {
                            debug!(
                                "DJ lookahead skipped because the next queue item is not resolved"
                            );
                        }
                    }
                }
                PlaybackRuntimeCommand::CrossfadeStart {
                    track_id,
                    generation,
                    trigger_position_samples,
                } => {
                    // The OUTGOING engine just entered its fade-out window and is asking
                    // us to start the pre-decoded next engine, if one is ready.
                    if state.engine.as_ref().map(|e| (e.track_id, e.generation))
                        == Some((track_id, generation))
                    {
                        let next_ready = state
                            .next_engine
                            .as_ref()
                            .and_then(|e| e.shared.buffer.lock().ok().map(|g| g.is_ready()))
                            .unwrap_or(false);
                        if next_ready && !active_engine_suppresses_crossfade_after_seek(&state) {
                            let trigger_actual_start_ms = samples_to_ms(
                                trigger_position_samples,
                                state.device_sample_rate,
                                state.device_channels,
                            );
                            let _ = prepare_dj_mixer_for_pair(
                                &mut state,
                                dj_mixer_max_block_samples(&output_config),
                            );
                            if prepared_overlay_program(&state) {
                                let device_sample_rate = state.device_sample_rate;
                                let device_channels = state.device_channels;
                                if let Err(reason) = start_prepared_overlay(
                                    &mut state,
                                    &event_tx,
                                    "fired",
                                    DjRuntimeRendererReason::None,
                                    Some(trigger_actual_start_ms),
                                    device_sample_rate,
                                    device_channels,
                                ) {
                                    record_current_runtime_renderer_failure(&mut state, reason);
                                }
                            } else {
                                let runtime_renderer =
                                    match install_prepared_handoff_mixer_buffer(&mut state) {
                                        Ok(()) => DjRuntimeRendererOutcome::rendered_handoff(),
                                        Err(reason) => {
                                            let failure =
                                                runtime_renderer_failure_reason(&state, reason);
                                            record_current_runtime_renderer_failure(
                                                &mut state, failure,
                                            );
                                            DjRuntimeRendererOutcome::legacy_overlap(failure)
                                        }
                                    };
                                promote_next_to_active(
                                    &mut state,
                                    &event_tx,
                                    &position_source,
                                    &buffered_source,
                                    &offset_source,
                                    "fired",
                                    Some(trigger_actual_start_ms),
                                    runtime_renderer,
                                );
                            }
                        } else if !active_engine_suppresses_crossfade_after_seek(&state) {
                            let reason = runtime_renderer_fire_block_reason(&state, next_ready);
                            record_current_runtime_renderer_failure(&mut state, reason);
                        }
                        // If not ready yet, NextDecodeComplete handles the late path.
                    }
                }
                PlaybackRuntimeCommand::ArmDropPreview {
                    track_id,
                    generation,
                    trigger_position_samples,
                } => {
                    arm_drop_preview_in_state(
                        &state,
                        track_id,
                        generation,
                        trigger_position_samples,
                    );
                }
                PlaybackRuntimeCommand::DropPreviewStart {
                    track_id,
                    generation,
                    trigger_position_samples,
                } => {
                    if !state.dj_engine_enabled {
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
                        let _ = prepare_drop_preview_mixer(
                            &mut state,
                            dj_mixer_max_block_samples(&output_config),
                        );
                        let actual_start_ms = samples_to_ms(
                            trigger_position_samples,
                            state.device_sample_rate,
                            state.device_channels,
                        );
                        if let Err(reason) = start_prepared_drop_preview_overlay(
                            &mut state,
                            &event_tx,
                            actual_start_ms,
                        ) {
                            debug!("Drop preview start skipped: {}", reason.as_str());
                            state.prepared_drop_preview_mixer = None;
                            if let Some(mut engine) = state.drop_preview_engine.take() {
                                engine.stop();
                            }
                        }
                    }
                }
                PlaybackRuntimeCommand::NextDecodeComplete {
                    track_id,
                    generation,
                } => {
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
                            runtime_renderer_late_fire_reason(&state)
                        } else {
                            DjRuntimeRendererReason::None
                        };
                        let _ = prepare_dj_mixer_for_pair(
                            &mut state,
                            dj_mixer_max_block_samples(&output_config),
                        );
                        if crossfade_started
                            && !active_engine_suppresses_crossfade_after_seek(&state)
                        {
                            if prepared_overlay_program(&state) {
                                let device_sample_rate = state.device_sample_rate;
                                let device_channels = state.device_channels;
                                if let Err(reason) = start_prepared_overlay(
                                    &mut state,
                                    &event_tx,
                                    "late",
                                    late_fire_reason,
                                    None,
                                    device_sample_rate,
                                    device_channels,
                                ) {
                                    record_current_runtime_renderer_failure(&mut state, reason);
                                }
                            } else {
                                let runtime_renderer =
                                    match install_prepared_handoff_mixer_buffer(&mut state) {
                                        Ok(()) => {
                                            DjRuntimeRendererOutcome::rendered_handoff_with_reason(
                                                late_fire_reason,
                                            )
                                        }
                                        Err(reason) => {
                                            let failure =
                                                runtime_renderer_failure_reason(&state, reason);
                                            record_current_runtime_renderer_failure(
                                                &mut state, failure,
                                            );
                                            DjRuntimeRendererOutcome::legacy_overlap(failure)
                                        }
                                    };
                                promote_next_to_active(
                                    &mut state,
                                    &event_tx,
                                    &position_source,
                                    &buffered_source,
                                    &offset_source,
                                    "late",
                                    None,
                                    runtime_renderer,
                                );
                            }
                        }
                    }
                }
                PlaybackRuntimeCommand::Pause => {
                    // Pause the active engine AND the fading-out engine (if any), so
                    // pressing pause during a crossfade actually stops all audio. The
                    // pre-decoded next engine is already paused so we don't touch it.
                    if let Some(engine) = state.engine.as_mut() {
                        match engine.pause() {
                            Ok(()) => {
                                let _ = event_tx.send(PlaybackRuntimeEvent::Paused {
                                    track_id: Some(engine.track_id),
                                });
                            }
                            Err(error) => {
                                report_runtime_command_error(&event_tx, "Pause", error);
                            }
                        }
                    }
                    if let Some(engine) = state.fading_out_engine.as_mut() {
                        if let Err(error) = engine.pause() {
                            report_runtime_command_error(&event_tx, "Pause", error);
                        }
                    }
                    if let Some(engine) = state.drop_preview_engine.as_mut() {
                        if let Err(error) = engine.pause() {
                            report_runtime_command_error(&event_tx, "Pause", error);
                        }
                    }
                }
                PlaybackRuntimeCommand::Resume => {
                    // On-demand re-grab: if exclusive mode is on and the active
                    // engine's WASAPI stream self-released after idle, rebuild it
                    // BEFORE unpausing so the decoder doesn't push samples into a
                    // missing stream. swap_stream handles its own cpal-shared
                    // fallback if the re-grab now fails (e.g. another app grabbed
                    // exclusive while we were paused).
                    if state.current_exclusive {
                        #[cfg(target_os = "windows")]
                        if state.exclusive_sink.needs_rebuild() {
                            info!(
                                "Resume: rebuilding exclusive stream after idle release on {}",
                                state.device_name
                            );
                            refresh_exclusive_sources(&state);
                            let rebuild_rate = exclusive_rebuild_rate(
                                state.current_sample_rate_follow,
                                state.device_sample_rate,
                            );
                            let release_grace_secs = state.current_exclusive_release_grace_secs;
                            let latency_mode = state.current_exclusive_latency_mode.clone();
                            match ensure_exclusive_sink_started(
                                &mut state,
                                &device,
                                &output_config,
                                rebuild_rate,
                                release_grace_secs,
                                latency_mode,
                                command_tx.clone(),
                                event_tx.clone(),
                            ) {
                                Ok(actual_rate) => {
                                    output_config.sample_rate = actual_rate;
                                    state.device_sample_rate = actual_rate;
                                }
                                Err(err) => {
                                    warn!(
                                        "Resume: failed to rebuild exclusive sink; falling back to shared: {err:?}"
                                    );
                                    // Drop the &mut state borrow before potential cleanup
                                    // so we can call stop_all_engines on the failure path
                                    // without a borrow-checker conflict.
                                    let swap_result = state.engine.as_mut().map(|engine| {
                                        engine.swap_stream(
                                            &device,
                                            &output_config,
                                            output_sample_format,
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
                                            stop_all_engines(&mut state);
                                            state.exclusive_sink.clear();
                                            report_runtime_command_error(
                                                &event_tx, "Resume", error,
                                            );
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
                                report_runtime_command_error(&event_tx, "Resume", error);
                            }
                        }
                    }
                    if let Some(engine) = state.fading_out_engine.as_mut() {
                        if let Err(error) = engine.resume() {
                            report_runtime_command_error(&event_tx, "Resume", error);
                        }
                    }
                    if let Some(engine) = state
                        .drop_preview_engine
                        .as_mut()
                        .filter(|engine| !engine.shared.paused.load(Ordering::SeqCst))
                    {
                        if let Err(error) = engine.resume() {
                            report_runtime_command_error(&event_tx, "Resume", error);
                        }
                    }
                }
                PlaybackRuntimeCommand::Stop => {
                    stop_all_engines(&mut state);
                    #[cfg(target_os = "windows")]
                    state.exclusive_sink.clear();
                    let _ = event_tx.send(PlaybackRuntimeEvent::Stopped);
                }
                PlaybackRuntimeCommand::TrackTerminal {
                    track_id,
                    generation,
                    outcome,
                } => {
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

                    match terminal_engine_slot(
                        active,
                        next,
                        fading,
                        drop_preview,
                        track_id,
                        generation,
                    ) {
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
                            if let PlaybackTerminalReason::Error(message) = &outcome {
                                emit_prepared_track_failure(&event_tx, track_id, message);
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
                            state.prepared_drop_preview_mixer = None;
                            if let Some(mut engine) = state.drop_preview_engine.take() {
                                engine.stop();
                            }
                        }
                        Some(TerminalEngineSlot::Active) => {
                            debug!(
                                "Playback terminal active engine: track_id={}, generation={}, outcome={:?}",
                                track_id, generation, outcome
                            );
                            if should_promote_prepared_at_boundary(
                                active, next, track_id, generation, &outcome,
                            ) {
                                promote_prepared_at_boundary(
                                    &mut state,
                                    &event_tx,
                                    &position_source,
                                    &buffered_source,
                                    &offset_source,
                                );
                            } else {
                                stop_current_engine(&mut state);
                                match outcome {
                                    PlaybackTerminalReason::Finished => {
                                        let _ = event_tx.send(PlaybackRuntimeEvent::Finished {
                                            track_id,
                                            generation,
                                        });
                                    }
                                    PlaybackTerminalReason::Error(message) => {
                                        let _ =
                                            event_tx.send(PlaybackRuntimeEvent::Error { message });
                                    }
                                }
                            }
                        }
                        None => {
                            debug!(
                                "Playback terminal ignored for unknown engine: track_id={}, generation={}, outcome={:?}",
                                track_id, generation, outcome
                            );
                        }
                    }
                    #[cfg(target_os = "windows")]
                    if state.current_exclusive {
                        refresh_exclusive_sources(&state);
                    }
                }
                PlaybackRuntimeCommand::TrackStatus {
                    track_id,
                    generation,
                    respond_to,
                } => {
                    let active = state
                        .engine
                        .as_ref()
                        .map(|engine| (engine.track_id, engine.generation))
                        == Some((track_id, generation));
                    let prepared = state
                        .next_engine
                        .as_ref()
                        .map(|engine| (engine.track_id, engine.generation))
                        == Some((track_id, generation));
                    let status = if active {
                        PlaybackTrackStatus::Active
                    } else if prepared {
                        PlaybackTrackStatus::Prepared
                    } else {
                        PlaybackTrackStatus::None
                    };
                    let _ = respond_to.send(status);
                }
                PlaybackRuntimeCommand::DeviceSwap {
                    device: selection,
                    exclusive,
                    sample_rate_follow,
                    desired_sample_rate,
                    exclusive_release_grace_secs,
                    exclusive_latency_mode,
                } => {
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
                    let requested_plan =
                        swap_stream_plan(&new_config, desired_rate, requested_backend);
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
                            refresh_exclusive_sources(&state);
                            state.exclusive_sink.stream = None;
                            match ensure_exclusive_sink_started(
                                &mut state,
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
                                    warn!(
                                        "DeviceSwap: exclusive sink failed; falling back to shared: {err:?}"
                                    );
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
                        warn!(
                            "DeviceSwap: one or more engines failed to swap; output may be partial"
                        );
                    }

                    // Update the runtime's "current device" bindings so subsequent
                    // Play / PrepareNext calls use the new device too. When
                    // sample-rate-follow drove a rate change, also update the
                    // runtime-wide `device_sample_rate` so freshly-cold-started
                    // engines spin up at the new rate (their initial
                    // `target_sample_rate` is seeded from this value).
                    device = new_device;
                    output_config = actual_config;
                    output_sample_format = new_format;
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
                }
                PlaybackRuntimeCommand::Shutdown => {
                    stop_all_engines(&mut state);
                    #[cfg(target_os = "windows")]
                    state.exclusive_sink.clear();
                    return std::ops::ControlFlow::Break(());
                }
            }
            std::ops::ControlFlow::Continue(())
        }));
        match outcome {
            Ok(std::ops::ControlFlow::Break(())) => break,
            Ok(std::ops::ControlFlow::Continue(())) => {}
            Err(payload) => {
                if let std::ops::ControlFlow::Break(()) =
                    handle_panic_in_runtime_loop(payload, &event_tx, &mut state)
                {
                    break;
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn transition_to_job(
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

    stop_current_engine(state);
    // A user-initiated track change (skip / new play) abandons any in-flight
    // crossfade - kill the fading-out engine so it doesn't keep producing audio
    // underneath the new track.
    if let Some(mut prior) = state.fading_out_engine.take() {
        prior.stop();
    }

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
    let pre_decoded_match = state
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
        let pre = state.next_engine.take().unwrap();
        // position_source was already redirected to this engine's counter at
        // promote_next_to_active time, so the handle reads the right value.
        // Restart the stream (it was paused during pre-decode).
        pre.shared.paused.store(false, Ordering::SeqCst);
        state.engine = Some(pre);
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
            *position_source.lock().unwrap() = Arc::clone(position_samples);
            *buffered_source.lock().unwrap() = Arc::clone(&eng.shared.buffered_samples);
            *offset_source.lock().unwrap() = Arc::clone(&eng.shared.position_offset_samples);
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
            *position_source.lock().unwrap() = Arc::clone(position_samples);
            *buffered_source.lock().unwrap() = Arc::clone(&eng.shared.buffered_samples);
            *offset_source.lock().unwrap() = Arc::clone(&eng.shared.position_offset_samples);
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

fn switch_is_noop_for_active_job(
    force_restart: bool,
    active: Option<(i64, u64)>,
    track_id: i64,
    generation: u64,
) -> bool {
    !force_restart && active == Some((track_id, generation))
}

fn stop_current_engine(state: &mut PlaybackRuntimeLoopState) {
    state.prepared_dj_mixer = None;
    state.prepared_drop_preview_mixer = None;
    if let Some(mut engine) = state.engine.take() {
        engine.stop();
    }
}

fn stop_all_engines(state: &mut PlaybackRuntimeLoopState) {
    stop_current_engine(state);
    if let Some(mut engine) = state.next_engine.take() {
        engine.stop();
    }
    if let Some(mut engine) = state.fading_out_engine.take() {
        engine.stop();
    }
    if let Some(mut engine) = state.drop_preview_engine.take() {
        engine.stop();
    }
}

fn report_runtime_command_error(
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    command_name: &str,
    error: anyhow::Error,
) {
    let message = format!("{command_name} failed: {error}");
    warn!("{message}");
    let _ = event_tx.send(PlaybackRuntimeEvent::Error { message });
}

/// Surface a decode/source failure on the pre-buffered next track. The
/// active track's failure already emits PlaybackRuntimeEvent::Error via
/// the TrackTerminal::Error branch for the Active slot, but the Next-slot
/// branch previously only logged - users had no signal that the upcoming
/// track silently dropped from the queue.
fn emit_prepared_track_failure(
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    track_id: i64,
    message: &str,
) {
    let surfaced = format!("Pre-buffered track {track_id} failed: {message}");
    warn!("{surfaced}");
    let _ = event_tx.send(PlaybackRuntimeEvent::Error { message: surfaced });
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Handle a panic that escaped the runtime command dispatch. Emits a
/// PlaybackRuntimeEvent::Error, tears down any active engines under a nested
/// catch_unwind, and signals whether the loop can safely continue.
///
/// If the cleanup itself panics (mutex poisoning, etc.), we emit a final
/// error event and signal Break - re-entering the dispatch loop with state
/// that may be corrupt is more dangerous than ending the runtime thread.
fn handle_panic_in_runtime_loop(
    payload: Box<dyn std::any::Any + Send>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    state: &mut PlaybackRuntimeLoopState,
) -> std::ops::ControlFlow<()> {
    let message = panic_payload_message(payload.as_ref());
    warn!("playback runtime panicked: {message}");

    let cleanup_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stop_all_engines(state);
        #[cfg(target_os = "windows")]
        state.exclusive_sink.clear();
    }));

    let _ = event_tx.send(PlaybackRuntimeEvent::Error {
        message: format!("playback runtime panicked: {message}"),
    });

    if cleanup_result.is_err() {
        let _ = event_tx.send(PlaybackRuntimeEvent::Error {
            message: "playback runtime panic cleanup also panicked; runtime exiting".to_string(),
        });
        std::ops::ControlFlow::Break(())
    } else {
        // After cleanup tore down every engine slot, signal Stopped so the UI's
        // playback-state machine snaps back to a clean idle (otherwise it would
        // remain stuck on the last Started state and the user sees a track
        // visually playing with no audio until the next user-initiated command).
        let _ = event_tx.send(PlaybackRuntimeEvent::Stopped);
        std::ops::ControlFlow::Continue(())
    }
}

#[cfg(target_os = "windows")]
fn exclusive_render_sources(
    active: Option<&PlaybackEngine>,
    prepared: Option<&PlaybackEngine>,
    fading: Option<&PlaybackEngine>,
    drop_preview: Option<&PlaybackEngine>,
) -> Vec<ExclusiveRenderSource> {
    let mut sources = Vec::new();
    if let Some(engine) = active {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Active,
            shared: Arc::clone(&engine.shared),
        });
    }
    if let Some(engine) = prepared {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Prepared,
            shared: Arc::clone(&engine.shared),
        });
    }
    if let Some(engine) = fading {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Fading,
            shared: Arc::clone(&engine.shared),
        });
    }
    if let Some(engine) = drop_preview {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Prepared,
            shared: Arc::clone(&engine.shared),
        });
    }
    sources
}

#[cfg(target_os = "windows")]
fn refresh_exclusive_sources(state: &PlaybackRuntimeLoopState) {
    state
        .exclusive_sink
        .source_bank
        .set_sources(exclusive_render_sources(
            state.engine.as_ref(),
            state.next_engine.as_ref(),
            state.fading_out_engine.as_ref(),
            state.drop_preview_engine.as_ref(),
        ));
}

#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn ensure_exclusive_sink_started(
    state: &mut PlaybackRuntimeLoopState,
    device: &cpal::Device,
    output_config: &StreamConfig,
    desired_sample_rate: Option<u32>,
    exclusive_release_grace_secs: u32,
    exclusive_latency_mode: ExclusiveLatencyMode,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<u32> {
    let exclusive_plan =
        swap_stream_plan(output_config, desired_sample_rate, SwapBackend::Exclusive);
    if !state.exclusive_sink.needs_rebuild() {
        return Ok(exclusive_plan.stream_config.sample_rate);
    }
    state.exclusive_sink.stream = None;

    let device_label = device_display_name(device);
    match build_exclusive_stream(
        Some(device_label.as_str()),
        device_label.clone(),
        exclusive_plan.stream_config.sample_rate,
        exclusive_plan.stream_config.channels,
        exclusive_release_grace_secs,
        exclusive_latency_mode,
        Arc::clone(&state.exclusive_sink.source_bank),
        command_tx,
        event_tx.clone(),
    ) {
        Ok(stream) => {
            let transport_format = stream.transport_format.clone();
            state.exclusive_sink.stream = Some(stream);
            let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeEngaged {
                device_name: device_label,
                transport_format,
            });
            Ok(exclusive_plan.stream_config.sample_rate)
        }
        Err(failure) => {
            let reason = failure.user_message();
            warn!("WASAPI exclusive grab failed; falling back to cpal shared: {reason}");
            let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeFailed {
                reason: reason.clone(),
                device_name: device_label,
            });
            Err(anyhow!(reason))
        }
    }
}

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
fn promote_next_to_active(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
    buffered_source: &Arc<Mutex<Arc<AtomicU64>>>,
    offset_source: &Arc<Mutex<Arc<AtomicU64>>>,
    timing_status: &'static str,
    actual_start_ms_override: Option<i64>,
    runtime_renderer: DjRuntimeRendererOutcome,
) {
    state.prepared_dj_mixer = None;
    let Some(next) = state.next_engine.take() else {
        return;
    };
    let transition_event_id = next
        .job
        .prepared_transition
        .as_ref()
        .and_then(|transition| transition.transition_event_id);
    if runtime_renderer.rendered {
        next.shared.crossfade_samples.store(0, Ordering::Relaxed);
        next.shared
            .fadein_start_samples
            .store(u64::MAX, Ordering::Relaxed);
    } else {
        next.shared.fadein_start_samples.store(0, Ordering::Relaxed);
    }
    next.shared.paused.store(false, Ordering::SeqCst);

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
    *position_source.lock().unwrap() = Arc::clone(&next.shared.position_samples);
    *buffered_source.lock().unwrap() = Arc::clone(&next.shared.buffered_samples);
    *offset_source.lock().unwrap() = Arc::clone(&next.shared.position_offset_samples);

    let outgoing = state.engine.take();
    state.engine = Some(next);

    // Drop any prior fading-out engine first so we never accumulate more than
    // one (the previous one would have been audibly silent by now anyway).
    if let Some(mut prior) = state.fading_out_engine.take() {
        prior.stop();
    }
    if let Some(mut outgoing) = outgoing {
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
            outgoing.stop();
        } else {
            state.fading_out_engine = Some(outgoing);
        }
        if let Some(transition_event_id) = transition_event_id {
            info!(
                transition_event_id,
                outgoing_track_id = outgoing_id,
                generation = outgoing_generation,
                actual_start_ms,
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
                timing_status: timing_status.to_string(),
                runtime_rendered_dj_mixer: runtime_renderer.rendered,
                runtime_renderer_status: runtime_renderer.status.as_str().to_string(),
                runtime_renderer_reason: runtime_renderer.reason.as_str().to_string(),
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

fn track_position_ms(shared: &PlaybackSharedState, sample_rate: u32, channels: u16) -> i64 {
    let samples = shared.position_samples.load(Ordering::Relaxed);
    samples_to_ms(samples, sample_rate, channels)
}

fn samples_to_ms(samples: u64, sample_rate: u32, channels: u16) -> i64 {
    if sample_rate == 0 || channels == 0 {
        return 0;
    }
    (samples.saturating_mul(1000) / (u64::from(sample_rate) * u64::from(channels))) as i64
}

fn promote_prepared_at_boundary(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
    buffered_source: &Arc<Mutex<Arc<AtomicU64>>>,
    offset_source: &Arc<Mutex<Arc<AtomicU64>>>,
) {
    let runtime_renderer = DjRuntimeRendererOutcome::boundary_fallback(
        runtime_renderer_boundary_fallback_reason(state),
    );
    state.prepared_dj_mixer = None;
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
    next.shared.paused.store(false, Ordering::SeqCst);
    *position_source.lock().unwrap() = Arc::clone(&next.shared.position_samples);
    *buffered_source.lock().unwrap() = Arc::clone(&next.shared.buffered_samples);
    *offset_source.lock().unwrap() = Arc::clone(&next.shared.position_offset_samples);

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
                timing_status: "missed".to_string(),
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

fn exclusive_rebuild_rate(sample_rate_follow: bool, device_sample_rate: u32) -> Option<u32> {
    sample_rate_follow.then_some(device_sample_rate)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputStateUpdate {
    sample_rate: u32,
    force_exclusive_rebuild: bool,
    notify_ready: bool,
}

fn device_swap_target_sample_rate(
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

fn transition_output_state_update(
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

fn transition_output_sample_rate(
    job_sample_rate: Option<u32>,
    sample_rate_follow: bool,
    current_sample_rate: u32,
) -> Option<u32> {
    if !sample_rate_follow {
        return None;
    }
    job_sample_rate.filter(|rate| *rate > 0 && *rate != current_sample_rate)
}

fn prepared_engine_matches_output_rate(
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
enum TerminalEngineSlot {
    Active,
    Next,
    FadingOut,
    DropPreview,
}

fn terminal_engine_slot(
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

fn should_promote_prepared_at_boundary(
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

fn active_engine_suppresses_crossfade_after_seek(state: &PlaybackRuntimeLoopState) -> bool {
    state.engine.as_ref().is_some_and(|engine| {
        engine
            .shared
            .suppress_crossfade_after_seek
            .load(Ordering::Relaxed)
    })
}

#[cfg(test)]
mod tests {
    use super::shared::{PlaybackBuffer, write_output_f32};
    use super::*;
    use crate::playback::gapless::GaplessPlan;
    use crate::playback::output::cpal_shared::{
        effective_output_config, output_rate_fallback_config,
    };
    use crate::playback::player::PlaybackSourceKind;
    use crate::playback::runtime::shared::{
        estimate_total_samples_from_duration_ms, samples_from_ms,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64},
    };

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
                    .dj_lookahead
                    .as_ref()
                    .map(|lookahead| lookahead.next_queue_item_id),
                Some(14)
            );
            assert!(state.dj_lookahead_failure.is_none());
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
                    .dj_lookahead
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
            assert!(state.dj_lookahead.is_none());
            assert_eq!(
                state
                    .dj_lookahead_failure
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
                    .dj_lookahead
                    .as_ref()
                    .map(|lookahead| lookahead.deadline_samples),
                Some(96_000)
            );
        }

        #[test]
        fn missed_lookahead_deadline_does_not_delay_playback() {
            let mut state = test_runtime_loop_state();
            state.engine = Some(test_engine_with_shared(1, 10));

            record_dj_lookahead_failure(
                &mut state,
                20,
                Some(11),
                Some(12),
                DjLookaheadFailureReason::AnalysisDeadlineMissed,
            );

            assert!(state.engine.is_some());
            assert_eq!(
                state
                    .dj_lookahead_failure
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
            let mut job = PreparedPlaybackJob::test_fixture(2, 7)
                .with_prepared_transition(transition(20, Some(11), Some(12)));

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
            let mut job = PreparedPlaybackJob::test_fixture(2, 7)
                .with_prepared_transition(transition(19, Some(11), Some(12)));

            assert!(discard_stale_prepared_transition(&state, &mut job));
            assert!(job.prepared_transition.is_none());
        }

        #[test]
        fn prepared_transition_discarded_when_next_queue_item_changes() {
            let state = state_with_pair();
            let mut job = PreparedPlaybackJob::test_fixture(2, 7)
                .with_prepared_transition(transition(20, Some(11), Some(99)));

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
        let prepared = state.prepared_dj_mixer.as_mut().expect("prepared mixer");
        assert_eq!(prepared.queue_generation, 20);
        assert_eq!(prepared.current_queue_item_id, Some(11));
        assert_eq!(prepared.next_queue_item_id, Some(12));
        assert_eq!(prepared.current_track_id, 1);
        assert_eq!(prepared.next_track_id, 2);

        let mut out = [0.0; 4];
        prepared.mixer.render_block(&mut out, 0);
        assert!(out.iter().any(|sample| *sample != 0.0));
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
        let prepared = state.prepared_dj_mixer.as_ref().expect("prepared mixer");
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
                .prepared_dj_mixer
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
                .prepared_dj_mixer
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
        let prepared = state.prepared_dj_mixer.as_mut().expect("prepared mixer");
        let mut out = [0.0; 2];

        prepared.mixer.render_block(&mut out, 0);

        assert!(out.iter().all(|sample| (*sample - 0.7).abs() < 1e-6));
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
        assert!(state.prepared_dj_mixer.is_none());
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
                48_000,
                2
            )
            .is_ok()
        );

        assert!(state.prepared_dj_mixer.is_none());
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

        assert!(state.prepared_drop_preview_mixer.is_none());
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
        state.dj_engine_enabled = false;
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
    fn mixer_promotion_stops_outgoing_without_legacy_fade() {
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
            DjRuntimeRendererOutcome::rendered_handoff(),
        );

        assert!(outgoing_stopped.load(Ordering::SeqCst));
        assert!(state.fading_out_engine.is_none());
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
    fn dj_flag_off_does_not_construct_mixer() {
        let mut state = state_with_ready_dj_pair();
        state.dj_engine_enabled = false;

        assert_eq!(
            prepare_dj_mixer_for_pair(&mut state, 64),
            Err(DjRuntimeRendererReason::DjDisabled)
        );
        assert!(state.prepared_dj_mixer.is_none());
    }

    #[test]
    fn dj_flag_off_ignores_transition_program_field() {
        let mut state = test_runtime_loop_state();
        state.dj_engine_enabled = false;
        let mut job = PreparedPlaybackJob::test_fixture(2, 21)
            .with_prepared_transition(test_prepared_transition_program(20, Some(11), Some(12)));

        assert!(!gate_prepare_next_for_dj(&mut state, &mut job));
        assert!(job.prepared_transition.is_none());
        assert!(state.prepared_dj_mixer.is_none());
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

        assert!(!state.dj_engine_enabled);
        assert!(state.prepared_dj_mixer.is_none());
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
        next.job =
            PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(fresh_transition);
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
    fn swap_stream_plan_uses_device_rate_for_shared_fallback() {
        let base = StreamConfig {
            channels: 2,
            sample_rate: 48_000,
            buffer_size: cpal::BufferSize::Default,
        };

        let plan = swap_stream_plan(&base, Some(192_000), SwapBackend::SharedFallback);

        assert_eq!(plan.stream_config.sample_rate, 48_000);
        assert_eq!(plan.target_sample_rate, Some(48_000));
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
            volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            buffered_source: Arc::clone(&buffered_source),
            offset_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
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
            volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            buffered_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(48_000)))),
            offset_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
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

    fn test_runtime_loop_state() -> PlaybackRuntimeLoopState {
        PlaybackRuntimeLoopState {
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
            dj_engine_enabled: true,
            dj_lookahead: None,
            dj_lookahead_failure: None,
            prepared_dj_mixer: None,
            prepared_drop_preview_mixer: None,
            last_dj_renderer_failure: None,
        }
    }

    #[test]
    fn emit_prepared_track_failure_sends_error_event() {
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);

        emit_prepared_track_failure(&event_tx, 42, "decode failed: malformed packet");

        match event_rx.try_recv().expect("error event should be emitted") {
            PlaybackRuntimeEvent::Error { message } => {
                assert!(message.contains("Pre-buffered track 42 failed"));
                assert!(message.contains("decode failed: malformed packet"));
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    #[test]
    fn handle_panic_in_runtime_loop_clears_state_and_emits_error() {
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
        let mut state = test_runtime_loop_state();
        state.engine = Some(test_engine_with_shared(1, 1));
        state.next_engine = Some(test_engine_with_shared(2, 1));

        let payload: Box<dyn std::any::Any + Send> =
            Box::new(String::from("synthetic dispatch panic"));
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
        let payload: Box<dyn std::any::Any + Send> =
            Box::new(String::from("synthetic dispatch panic"));
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
            volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            buffered_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            offset_source: Arc::clone(&offset_source),
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
}
