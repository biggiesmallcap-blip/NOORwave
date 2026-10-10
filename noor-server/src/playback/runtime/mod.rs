use crate::db::audio_settings::ExclusiveLatencyMode;
use crate::playback::dj_lookahead::DjMediaRef;
use crate::playback::gapless::GaplessPlan;
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

mod beat_sync;
pub mod commands;
mod device;
mod dj_transition;
mod engine;
pub(crate) mod shared;
mod timeline;

pub use commands::{
    PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackTerminalReason, PlaybackTrackStatus,
    SeekToOutcome,
};
pub use device::{OutputDeviceSelection, enumerate_output_devices};
use device::{device_display_name, resolve_device};
use dj_transition::*;
use engine::PlaybackEngine;
#[cfg(test)]
use engine::SwapPauseGuard;
pub(crate) use shared::PlaybackSharedState;
#[cfg(target_os = "windows")]
pub(crate) use shared::fill_f32_from_shared;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

const DJ_MIXER_DEFAULT_MAX_BLOCK_FRAMES: usize = 8192;

/// How often the runtime loop wakes (when no command is pending) to check for a
/// stalled active engine.
const STALL_WATCHDOG_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// How long the audibly-active engine may make zero position progress -- while
/// playing and not paused -- before the watchdog force-advances the queue.
/// Sized comfortably past one healthy DASH segment timeout+retry cycle
/// (`cdn_health::HEALTHY_SEGMENT_TIMEOUT` = 12s) so a transiently-slow segment
/// that still arrives is not pre-empted, but a doomed TIDAL CDN stall recovers
/// automatically instead of freezing playback until the user manually skips.
/// The same budget covers a lost end-of-track terminal: 15s is far longer than
/// the sub-buffer gap between the buffer draining and a healthy terminal being
/// processed, so a working advance is never pre-empted by the watchdog.
const ACTIVE_STALL_RECOVERY_SECS: u64 = 15;

/// An outgoing engine that lived at least this long without producing a
/// single audible sample counts as a silent failure for the advance-cascade
/// circuit breaker. Rapid manual skips tear down much younger engines and
/// must not count toward the streak.
const SILENT_ENGINE_FAILURE_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(10);

/// After this many consecutive silent engine failures the runtime stops
/// hot-advancing: it latches pause, surfaces one clear error, and leaves the
/// queue intact for the user to resume. Without this ceiling a dead TIDAL
/// CDN made the watchdog + track-error advances burn through the entire
/// queue 15-25s at a time while pause commands appeared to do nothing --
/// the state users could only escape by restarting the server.
const MAX_SILENT_START_STREAK: u32 = 3;

/// Watchdog state for the runtime loop. The loop otherwise only advances when
/// the audio callback sends a command, and the callback goes quiet in two
/// distinct ways, both of which froze playback until the user clicked Next:
///
///   * Starved mid-track: a decoder hung on a TIDAL segment is
///     `started && !finished && written==0`. It emits no command and the
///     playhead freezes until the segment finally errors out, if it ever does.
///   * Lost end-of-track terminal: the engine is `finished` with a fully
///     drained buffer, so the callback's one-shot `TrackTerminal` was already
///     latched and sent. If it was then dropped downstream (no matching engine
///     slot, or a guard in the queue-advance handler), nothing re-issues it.
///
/// This tracker notices an active engine making no progress in either shape and
/// asks the loop to force the queue forward.
struct StallTracker {
    watching: Option<(i64, u64)>,
    last_position: u64,
    last_progress_at: std::time::Instant,
    /// True between a stall detection and the next progress/rearm. Gates the
    /// `Stalled` / `StallRecovered` event pair to one emission per episode.
    stall_flagged: bool,
}

/// One engine's watchdog-relevant state, read once per tick. Decouples the
/// stall decision from `PlaybackRuntimeLoopState` so it is unit-testable.
#[derive(Debug, Clone, Copy)]
struct EngineProbe {
    id: (i64, u64),
    position: u64,
    paused: bool,
    started: bool,
    finished: bool,
    /// No unread samples left in the buffer. Combined with `finished` this is
    /// end-of-track: there is no more audio coming and none left to play.
    drained: bool,
}

/// Which failure shape triggered a force-advance. Only meaningful when
/// `StallPollOutcome::force_advance` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StallKind {
    /// Decode had not finished and the engine ran dry: a hung stream mid-track.
    Starved,
    /// Decode finished and the buffer fully drained, but the queue never moved.
    /// The audio callback's one-shot terminal was lost somewhere between
    /// `finished_notified` being latched and the queue advance running.
    LostTerminal,
}

/// What one watchdog tick decided.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct StallPollOutcome {
    /// Engine starved past `ACTIVE_STALL_RECOVERY_SECS`: force the queue
    /// forward. Re-fires every threshold interval while the stall persists.
    force_advance: Option<(i64, u64)>,
    /// First tick of a stall episode: pause the listen session (track_id).
    just_stalled: Option<i64>,
    /// First progress after a stall episode on the SAME engine: resume the
    /// listen session (track_id). Engine changes do not emit this - the
    /// track-change flow flushes the session instead.
    just_recovered: Option<i64>,
    /// Set alongside `force_advance` to distinguish a mid-track starve from a
    /// lost end-of-track terminal, so the two log distinguishably.
    kind: Option<StallKind>,
}

impl StallTracker {
    fn new() -> Self {
        Self {
            watching: None,
            last_position: 0,
            last_progress_at: std::time::Instant::now(),
            stall_flagged: false,
        }
    }

    /// Re-arm against the current active engine without flagging a stall. Used
    /// when the engine is paused, finished, freshly changed, or making progress
    /// -- none of which are stalls.
    fn rearm(&mut self, id: (i64, u64), position: u64) {
        self.watching = Some(id);
        self.last_position = position;
        self.last_progress_at = std::time::Instant::now();
        self.stall_flagged = false;
    }

    /// Called on each idle watchdog tick.
    fn poll(&mut self, state: &PlaybackRuntimeLoopState) -> StallPollOutcome {
        let Some(engine) = state.engine.as_ref() else {
            self.watching = None;
            self.stall_flagged = false;
            return StallPollOutcome::default();
        };
        let (started, finished, drained) = engine
            .shared
            .buffer
            .lock()
            .map(|guard| {
                (
                    guard.started,
                    guard.finished,
                    guard.samples.len() <= guard.read_pos,
                )
            })
            .unwrap_or((false, false, false));
        self.observe(EngineProbe {
            id: (engine.track_id, engine.generation),
            position: engine.shared.position_samples.load(Ordering::Relaxed),
            paused: engine.shared.paused.load(Ordering::SeqCst),
            started,
            finished,
            drained,
        })
    }

    /// Pure decision core over one engine probe.
    fn observe(&mut self, probe: EngineProbe) -> StallPollOutcome {
        let mut outcome = StallPollOutcome::default();

        // Paused playback legitimately makes no progress.
        if probe.paused {
            self.rearm(probe.id, probe.position);
            return outcome;
        }
        // A not-yet-started engine is still doing its initial prebuffer -- on a
        // slow connection the first ~500ms can legitimately take many seconds to
        // arrive, and the playhead sits at the baseline offset the whole time.
        // That is not a stall; only a deck that WAS playing and then froze is.
        //
        // `finished` used to be exempted here too, on the reasoning that such an
        // engine "emits its own terminal via the audio callback". That terminal
        // is one-shot (`finished_notified` is latched before the send and never
        // re-armed), so when it was lost the exemption meant nothing recovered
        // the queue and playback froze at the end of the track until the user
        // hit Next. Worse, the decoder marks `finished` as soon as decode
        // completes -- with DASH lookahead that is minutes before playback
        // reaches the end -- so the exemption disarmed the watchdog across the
        // whole back half of every track. Finished engines are now watched like
        // any other; a finished engine that is still playing out its buffer
        // keeps moving its position and rearms below on its own.
        if !probe.started {
            self.rearm(probe.id, probe.position);
            return outcome;
        }
        // New track, or audible progress since the last tick -> not stalled.
        // Progress on the engine we flagged ends the stall episode: tell the
        // listener to resume the listen session it paused.
        if self.watching != Some(probe.id) || probe.position != self.last_position {
            if self.stall_flagged && self.watching == Some(probe.id) {
                outcome.just_recovered = Some(probe.id.0);
            }
            self.rearm(probe.id, probe.position);
            return outcome;
        }
        // Same track, no progress since the last tick: over budget?
        if self.last_progress_at.elapsed()
            >= std::time::Duration::from_secs(ACTIVE_STALL_RECOVERY_SECS)
        {
            // Reset the clock so we don't re-fire every tick while the synthetic
            // terminal is in flight and the queue advances.
            self.last_progress_at = std::time::Instant::now();
            if !self.stall_flagged {
                self.stall_flagged = true;
                outcome.just_stalled = Some(probe.id.0);
            }
            outcome.force_advance = Some(probe.id);
            outcome.kind = Some(if probe.finished && probe.drained {
                StallKind::LostTerminal
            } else {
                StallKind::Starved
            });
        }
        outcome
    }
}

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
    healthy: Arc<AtomicBool>,
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
    handoff_elapsed_source: Arc<Mutex<Arc<AtomicU64>>>,
}

impl PlaybackRuntimeHandle {
    #[cfg(test)]
    pub(crate) fn test_publish_event(&self, event: PlaybackRuntimeEvent) -> bool {
        self.event_tx.send(event).is_ok()
    }

    #[cfg(test)]
    pub(crate) fn test_publish_position(&self, samples: u64) {
        self.position_source
            .lock()
            .unwrap()
            .store(samples, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn test_with_command_tx(command_tx: mpsc::Sender<PlaybackRuntimeCommand>) -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        Self {
            command_tx,
            event_tx,
            healthy: Arc::new(AtomicBool::new(true)),
            volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            buffered_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            offset_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            handoff_elapsed_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(u64::MAX)))),
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

    /// Release the WASAPI exclusive device now (ahead of the idle grace) so the
    /// WebView can play a video's audio in shared mode. No-op outside Windows
    /// exclusive mode. Callers should pause playback first; the runtime
    /// re-grabs exclusive on the next Resume/Play.
    pub fn release_exclusive_now(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::ReleaseExclusiveNow)
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

    pub fn request_ready(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::RequestReady)
    }

    pub(crate) fn resolved_analysis_stream(&self, track_id: i64) -> Option<StreamInfo> {
        let (respond_to, response) = mpsc::channel();
        self.send(PlaybackRuntimeCommand::ResolvedAnalysisStream {
            track_id,
            respond_to,
        })
        .ok()?;
        response.recv_timeout(Duration::from_secs(2)).ok().flatten()
    }

    pub(crate) fn update_prepared_transition(
        &self,
        transition: PreparedTransitionProgram,
        gapless: GaplessPlan,
    ) -> bool {
        let (respond_to, response) = mpsc::channel();
        if self
            .send(PlaybackRuntimeCommand::UpdatePreparedTransition {
                transition,
                gapless,
                respond_to,
            })
            .is_err()
        {
            return false;
        }
        response
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or(false)
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    pub(crate) fn is_same_runtime(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.healthy, &other.healthy)
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

    /// Actual output-clock progress of an installed, currently audible
    /// handoff. None before fire, after resolution, or after a manual seek.
    pub fn get_dj_handoff_elapsed_ms(&self, sample_rate: u32, channels: u16) -> Option<i64> {
        if sample_rate == 0 || channels == 0 {
            return None;
        }
        let samples = self
            .handoff_elapsed_source
            .lock()
            .unwrap()
            .load(Ordering::Relaxed);
        (samples != u64::MAX).then(|| samples_to_ms(samples, sample_rate, channels))
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
        self.command_tx.send(command).map_err(|_| {
            self.healthy.store(false, Ordering::Release);
            anyhow!("playback runtime command channel closed")
        })
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
    let healthy = Arc::new(AtomicBool::new(true));
    let worker_healthy = Arc::clone(&healthy);

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
    let handoff_elapsed_source = Arc::new(Mutex::new(Arc::new(AtomicU64::new(u64::MAX))));

    let worker_volume_ctl = Arc::clone(&volume_ctl);
    let worker_initial_position = Arc::clone(&initial_position);
    let worker_position_source = Arc::clone(&position_source);
    let worker_buffered_source = Arc::clone(&buffered_source);
    let worker_offset_source = Arc::clone(&offset_source);
    let worker_handoff_elapsed_source = Arc::clone(&handoff_elapsed_source);

    thread::Builder::new()
        .name("noor-playback-runtime".into())
        .spawn(move || {
            let exit_message = if let Err(err) = run_runtime_loop(
                config,
                command_rx,
                worker_command_tx,
                worker_event_tx.clone(),
                worker_volume_ctl,
                worker_initial_position,
                worker_position_source,
                worker_buffered_source,
                worker_offset_source,
                worker_handoff_elapsed_source,
            ) {
                let _ = worker_event_tx.send(PlaybackRuntimeEvent::Error {
                    message: err.to_string(),
                });
                error!("Playback runtime stopped: {err:?}");
                Some(err.to_string())
            } else {
                None
            };
            worker_healthy.store(false, Ordering::Release);
            let _ = worker_event_tx.send(PlaybackRuntimeEvent::Exited {
                message: exit_message,
            });
        })
        .context("failed to spawn playback runtime thread")?;

    Ok(PlaybackRuntimeHandle {
        command_tx,
        event_tx,
        healthy,
        volume_ctl,
        position_source,
        buffered_source,
        offset_source,
        handoff_elapsed_source,
    })
}

struct PlaybackRuntimeLoopState {
    handoff_elapsed_source: Arc<Mutex<Arc<AtomicU64>>>,
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
    /// DJ transition preparation (lookahead, prepared mixers, renderer failures).
    dj: DjTransitionState,
    /// User transport intent as most recently processed by this loop: `true`
    /// from a Pause command until a Resume (or an explicitly-unpaused job)
    /// clears it. Every engine cold start and promotion consults this, so an
    /// auto-advance, crossfade promotion, or prepared-overlay swap can never
    /// un-pause audio behind the user's back. This latch is what makes the
    /// pause button reliable while the queue is advancing through failures.
    user_paused: bool,
    /// Consecutive engine teardowns where the outgoing deck lived past
    /// `SILENT_ENGINE_FAILURE_MIN_AGE` without ever producing audio. Feeds
    /// the advance-cascade circuit breaker; reset whenever a deck actually
    /// makes sound.
    silent_start_streak: u32,
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
    handoff_elapsed_source: Arc<Mutex<Arc<AtomicU64>>>,
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
        handoff_elapsed_source,
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
        dj: DjTransitionState::new(config.dj_engine_enabled),
        user_paused: false,
        silent_start_streak: 0,
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

    let mut stall_tracker = StallTracker::new();
    let mut last_dj_readiness_check = Instant::now();
    loop {
        // Check between commands as well as on idle ticks: a busy cockpit
        // must not starve preparation or a decode-delayed handoff. Dispatch
        // directly, retaining this loop's generation checks and panic guard.
        let readiness_wakeup = if last_dj_readiness_check.elapsed() >= STALL_WATCHDOG_TICK {
            last_dj_readiness_check = Instant::now();
            dj_pcm_readiness_wakeup(&state)
        } else {
            None
        };
        let command = match readiness_wakeup
            .map(Ok)
            .unwrap_or_else(|| command_rx.recv_timeout(STALL_WATCHDOG_TICK))
        {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Emit any warns the audio callback latched (underrun,
                // rejected in-callback seek) - the callback itself must not
                // touch tracing.
                for engine in [
                    state.engine.as_ref(),
                    state.next_engine.as_ref(),
                    state.fading_out_engine.as_ref(),
                    state.drop_preview_engine.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    engine.shared.drain_deferred_rt_logs();
                }
                // Idle tick: nothing to dispatch. Check whether the audible deck
                // has frozen on a hung TIDAL segment and, if so, force the queue
                // forward (the audio callback can't, because a starved-but-not-
                // finished engine emits no command).
                let stall = stall_tracker.poll(&state);
                if let Some(track_id) = stall.just_stalled {
                    let _ = event_tx.send(PlaybackRuntimeEvent::Stalled { track_id });
                }
                if let Some(track_id) = stall.just_recovered {
                    let _ = event_tx.send(PlaybackRuntimeEvent::StallRecovered { track_id });
                }
                if let Some((track_id, generation)) = stall.force_advance {
                    match stall.kind {
                        Some(StallKind::LostTerminal) => warn!(
                            target: "noor.playback.advance",
                            event = "watchdog_lost_terminal",
                            track_id,
                            generation,
                            stalled_secs = ACTIVE_STALL_RECOVERY_SECS,
                            "track finished and drained but the queue never advanced; \
                             end-of-track terminal was lost. Forcing queue advance"
                        ),
                        _ => warn!(
                            target: "noor.playback.advance",
                            event = "watchdog_starved",
                            track_id,
                            generation,
                            stalled_secs = ACTIVE_STALL_RECOVERY_SECS,
                            "no audio progress on track; forcing queue advance"
                        ),
                    }
                    // Reuse the natural end-of-track advance (Finished): promotes a
                    // ready prepared deck if there is one, otherwise cold-starts
                    // the next queue track. A silent skip, not an error toast.
                    let _ = command_tx.send(PlaybackRuntimeCommand::TrackTerminal {
                        track_id,
                        generation,
                        outcome: PlaybackTerminalReason::Finished,
                    });
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
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
                PlaybackRuntimeCommand::RequestReady => {
                    let _ = event_tx.send(PlaybackRuntimeEvent::Ready {
                        device_name: state.device_name.clone(),
                        sample_rate: state.device_sample_rate,
                        channels: state.device_channels,
                    });
                }
                PlaybackRuntimeCommand::ResolvedAnalysisStream {
                    track_id,
                    respond_to,
                } => {
                    let stream = resolved_analysis_stream_in_state(&state, track_id);
                    info!(
                        track_id,
                        reused = stream.is_some(),
                        "DJ background analysis source lookup"
                    );
                    let _ = respond_to.send(stream);
                }
                PlaybackRuntimeCommand::UpdatePreparedTransition {
                    transition,
                    gapless,
                    respond_to,
                } => {
                    let accepted =
                        update_prepared_transition_in_state(&mut state, transition, gapless);
                    let _ = respond_to.send(accepted);
                    if accepted && can_prepare_dj_mixer_before_fire(&state) {
                        let _ = prepare_dj_mixer_for_pair(
                            &mut state,
                            dj_mixer_max_block_samples(&output_config),
                        );
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
                        let offset_samples =
                            engine.shared.source_offset_samples.load(Ordering::Relaxed);
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
                                if let Err(error) = engine.shared.restore_source_buffer_after_seek()
                                {
                                    warn!("Could not retire rendered handoff for seek: {error}");
                                    let _ = respond_to.send(SeekToOutcome::Failed);
                                    return std::ops::ControlFlow::Continue(());
                                }
                                if let Err(error) =
                                    engine.shared.apply_in_buffer_seek(target_samples)
                                {
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
                            state.dj.prepared_mixer = None;
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
                                if can_prepare_dj_mixer_before_fire(&state) {
                                    let _ = prepare_dj_mixer_for_pair(
                                        &mut state,
                                        dj_mixer_max_block_samples(&output_config),
                                    );
                                }
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
                        .map(|engine| {
                            engine.track_id == job.track.id && engine.generation == job.generation
                        })
                        .unwrap_or(false);
                    if !already_pending {
                        if let Some(mut stale) = state.drop_preview_engine.take() {
                            state.dj.prepared_drop_preview_mixer = None;
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
                    if !state.dj.engine_enabled {
                        state.dj.lookahead = None;
                        state.dj.prepared_mixer = None;
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
                    trigger_target_samples,
                } => {
                    // The OUTGOING engine just entered its fade-out window and is asking
                    // us to start the pre-decoded next engine, if one is ready.
                    if state.engine.as_ref().map(|e| (e.track_id, e.generation))
                        == Some((track_id, generation))
                    {
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
                            .map(|buffer| {
                                dj_crossfade_next_ready(&state, buffer, crossfade_samples)
                            })
                            .unwrap_or(false);
                        if next_ready && !active_engine_suppresses_crossfade_after_seek(&state) {
                            let runtime_planned_start_ms =
                                runtime_transition_target_ms(&state, Some(trigger_target_samples));
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
                            if !prepared_dj_mixer_matches_pair(&state) {
                                let _ = prepare_dj_mixer_for_pair(
                                    &mut state,
                                    dj_mixer_max_block_samples(&output_config),
                                );
                            }
                            if prepared_overlay_program(&state) {
                                let device_sample_rate = state.device_sample_rate;
                                let device_channels = state.device_channels;
                                if let Err(reason) = start_prepared_overlay(
                                    &mut state,
                                    &event_tx,
                                    "fired",
                                    DjRuntimeRendererReason::None,
                                    Some(trigger_actual_start_ms),
                                    runtime_planned_start_ms,
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
                                    runtime_planned_start_ms,
                                    runtime_renderer,
                                );
                            }
                        } else if !active_engine_suppresses_crossfade_after_seek(&state) {
                            let incoming = state.next_engine.as_ref();
                            let incoming_rate = incoming.map(|engine| {
                                engine
                                    .shared
                                    .target_sample_rate
                                    .load(Ordering::Relaxed)
                                    .max(1)
                            });
                            let runtime_samples_per_second =
                                f64::from(state.device_sample_rate.max(1))
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
                            let reason = runtime_renderer_fire_block_reason(&state, next_ready);
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
                                    .map(|buffer| buffer.unread_samples as f64
                                        / incoming_samples_per_second),
                                incoming_unread_runtime_seconds = next_buffer
                                    .map(|buffer| buffer.unread_samples as f64
                                        / runtime_samples_per_second),
                                incoming_decoded_samples =
                                    next_buffer.map(|buffer| buffer.decoded_samples),
                                incoming_read_samples =
                                    next_buffer.map(|buffer| buffer.read_samples),
                                incoming_offset_samples =
                                    next_buffer.map(|buffer| buffer.offset_samples),
                                start_threshold_samples =
                                    next_buffer.map(|buffer| buffer.start_threshold_samples),
                                required_long_seconds =
                                    crossfade_samples.saturating_add(crossfade_samples / 8) as f64
                                        / runtime_samples_per_second,
                                required_program_source_seconds = program_required_seconds,
                                required_adaptive_unread_seconds = next_buffer
                                    .and_then(|buffer| adaptive_next_required_samples(
                                        &state, buffer
                                    ))
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
                                outgoing_decoder_target_sample_rate =
                                    state.engine.as_ref().map(|engine| engine
                                        .shared
                                        .target_sample_rate
                                        .load(Ordering::Relaxed)),
                                prepared_mixer_matches = prepared_dj_mixer_matches_pair(&state),
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
                        let preparation = prepare_drop_preview_mixer(
                            &mut state,
                            dj_mixer_max_block_samples(&output_config),
                        );
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
                        if let Err(reason) = preparation.and_then(|()| {
                            start_prepared_drop_preview_overlay(
                                &mut state,
                                &event_tx,
                                actual_start_ms,
                            )
                        }) {
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
                        if !prepared_dj_mixer_matches_pair(&state)
                            && can_prepare_dj_mixer_before_fire(&state)
                        {
                            let _ = prepare_dj_mixer_for_pair(
                                &mut state,
                                dj_mixer_max_block_samples(&output_config),
                            );
                        }
                        if crossfade_started
                            && !active_engine_suppresses_crossfade_after_seek(&state)
                        {
                            let runtime_planned_start_ms =
                                runtime_transition_target_ms(&state, None);
                            if prepared_overlay_program(&state) {
                                let device_sample_rate = state.device_sample_rate;
                                let device_channels = state.device_channels;
                                if let Err(reason) = start_prepared_overlay(
                                    &mut state,
                                    &event_tx,
                                    "late",
                                    late_fire_reason,
                                    None,
                                    runtime_planned_start_ms,
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
                                    runtime_planned_start_ms,
                                    runtime_renderer,
                                );
                            }
                        }
                    }
                }
                PlaybackRuntimeCommand::Pause => {
                    // Latch the user's intent FIRST: every engine start and
                    // promotion from here on comes up silent until Resume (or
                    // an explicitly-unpaused job) clears the latch. This is
                    // what stops a queued auto-advance from un-pausing audio
                    // moments after the user hit pause.
                    state.user_paused = true;
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
                    if let Some(engine) = state.fading_out_engine.as_mut()
                        && let Err(error) = engine.pause()
                    {
                        report_runtime_command_error(&event_tx, "Pause", error);
                    }
                    if let Some(engine) = state.drop_preview_engine.as_mut()
                        && let Err(error) = engine.pause()
                    {
                        report_runtime_command_error(&event_tx, "Pause", error);
                    }
                    // Instrumentation only (slice C0): correlate an explicit user
                    // pause with the render thread's idle-release timing in the logs.
                    // This is the exact hook point where C1 will request an early
                    // device release on user pause.
                    #[cfg(target_os = "windows")]
                    if state.current_exclusive {
                        tracing::debug!(
                            target: "playback",
                            grace_secs = state.current_exclusive_release_grace_secs,
                            "Pause: user pause with exclusive active; device frees after idle grace (C1 release-on-pause hook point)"
                        );
                    }
                }
                PlaybackRuntimeCommand::ReleaseExclusiveNow => {
                    // Yield the exclusive endpoint now so the WebView can play a
                    // video in shared mode. Pause first (idempotent if already
                    // paused) so the render thread isn't mid-fill when it drops
                    // the device, then ask it to release ahead of the idle grace.
                    if state.current_exclusive {
                        if let Some(engine) = state.engine.as_mut() {
                            let _ = engine.pause();
                        }
                        if let Some(engine) = state.fading_out_engine.as_mut() {
                            let _ = engine.pause();
                        }
                        #[cfg(target_os = "windows")]
                        {
                            info!(
                                "ReleaseExclusiveNow: dropping exclusive device on {} for shared-mode video playback",
                                state.device_name
                            );
                            state.exclusive_sink.request_release();
                        }
                    }
                }
                PlaybackRuntimeCommand::Resume => {
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
                    if let Some(engine) = state.fading_out_engine.as_mut()
                        && let Err(error) = engine.resume()
                    {
                        report_runtime_command_error(&event_tx, "Resume", error);
                    }
                    if let Some(engine) = state
                        .drop_preview_engine
                        .as_mut()
                        .filter(|engine| !engine.shared.paused.load(Ordering::SeqCst))
                        && let Err(error) = engine.resume()
                    {
                        report_runtime_command_error(&event_tx, "Resume", error);
                    }
                }
                PlaybackRuntimeCommand::Stop => {
                    stop_all_engines(&mut state);
                    // A stopped session has no transport intent to preserve.
                    state.user_paused = false;
                    state.silent_start_streak = 0;
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
                            if let PlaybackTerminalReason::Error(message) = &outcome
                                && let Some(next_engine) = state.next_engine.as_ref()
                            {
                                emit_prepared_track_failure(&event_tx, &next_engine.job, message);
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

fn switch_is_noop_for_active_job(
    force_restart: bool,
    active: Option<(i64, u64)>,
    track_id: i64,
    generation: u64,
) -> bool {
    !force_restart && active == Some((track_id, generation))
}

/// A direct selection can consume a prepared next deck without going through
/// promotion. Bind every public reader to that deck before exposing it.
fn adopt_predecoded_next_engine(
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
fn evaluate_advance_cascade(
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

/// Ceiling on how long a teardown will wait for retiring decks to finish their
/// fade. The ramp itself is TRANSPORT_FADE_MS; the rest is slack for a couple of
/// callback periods. A deck whose callback has stopped running (device gone,
/// stream never started) will never finish its ramp, so the wait must be capped
/// rather than driven by the audio thread.
const FADE_OUT_WAIT_TIMEOUT: Duration = Duration::from_millis(40);
const FADE_OUT_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Ramp every audible deck in `engines` down to silence, then stop them all.
///
/// `PlaybackEngine::stop` resets the buffer out from under the callback, so a
/// deck that is still making sound gets truncated mid-waveform - a step to zero,
/// which is the pop heard on skip and stop. Fading first costs one short,
/// bounded block of the command loop (which is a `recv_timeout` loop, not a
/// real-time one, so a few ms of added skip latency is inaudible).
///
/// The wait is synchronous on purpose. Parking retiring decks for a later
/// reaping tick would keep the outgoing engine's stream alive past this point,
/// and in WASAPI exclusive mode it still owns the device - the incoming engine
/// could not grab it. Finishing the fade here keeps teardown ordered.
///
/// Decks that are not audible (paused pre-decode, drop preview, already stopped)
/// report nothing to fade and are stopped immediately, so the common paths pay
/// no wait at all.
fn fade_out_and_stop(mut engines: Vec<PlaybackEngine>) {
    let mut any_fading = false;
    for engine in engines.iter() {
        // Not `any()` - every deck must be armed, and `any()` short-circuits.
        any_fading |= engine.shared.begin_fade_out();
    }

    if any_fading {
        let deadline = Instant::now() + FADE_OUT_WAIT_TIMEOUT;
        while Instant::now() < deadline
            && engines
                .iter()
                .any(|engine| engine.shared.pause_fade_armed.load(Ordering::Relaxed))
        {
            std::thread::sleep(FADE_OUT_POLL_INTERVAL);
        }
    }

    for engine in engines.iter_mut() {
        engine.stop();
    }
}

fn stop_current_engine(state: &mut PlaybackRuntimeLoopState) {
    state.dj.prepared_mixer = None;
    state.dj.prepared_drop_preview_mixer = None;
    fade_out_and_stop(state.engine.take().into_iter().collect());
}

fn stop_all_engines(state: &mut PlaybackRuntimeLoopState) {
    state.dj.readiness_permanent_failure = None;
    state.dj.prepared_mixer = None;
    state.dj.prepared_drop_preview_mixer = None;
    // Retire as one batch so the audible decks share a single fade window
    // instead of serializing one after another.
    let mut retiring: Vec<PlaybackEngine> = Vec::new();
    retiring.extend(state.engine.take());
    retiring.extend(state.next_engine.take());
    retiring.extend(state.fading_out_engine.take());
    retiring.extend(state.drop_preview_engine.take());
    fade_out_and_stop(retiring);
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

/// Surface a decode/source failure on the pre-buffered next track without
/// treating it as an active-track playback failure.
fn emit_prepared_track_failure(
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    job: &PreparedPlaybackJob,
    message: &str,
) {
    let track_id = job.track.id;
    let tidal_id = match &job.source {
        crate::playback::player::PlaybackSourceRequest::TidalStream(request) => {
            Some(request.track_id)
        }
        crate::playback::player::PlaybackSourceRequest::LocalLibrary => None,
    };
    let surfaced = format!("Pre-buffered track {track_id} failed: {message}");
    warn!("{surfaced}");
    let _ = event_tx.send(PlaybackRuntimeEvent::PreparedTrackError {
        track_id,
        generation: job.generation,
        tidal_id,
        message: surfaced,
    });
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
fn crossfade_next_ready(
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

fn adaptive_rhythmic_transition(
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

fn adaptive_next_required_samples(
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

fn dj_crossfade_next_ready(
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

fn promote_next_to_active(
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

fn track_position_ms(shared: &PlaybackSharedState, sample_rate: u32, channels: u16) -> i64 {
    let samples = shared.output_to_source_samples(shared.position_samples.load(Ordering::Relaxed));
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
mod tests;
