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
mod engine;
pub(crate) mod shared;
mod timeline;

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
    dj_engine_enabled: bool,
    dj_lookahead: Option<RuntimeDjLookahead>,
    dj_lookahead_failure: Option<DjLookaheadFailure>,
    prepared_dj_mixer: Option<PreparedDjMixer>,
    prepared_drop_preview_mixer: Option<PreparedDjMixer>,
    last_dj_renderer_failure: Option<DjRuntimeRendererFailure>,
    /// Do not repeat a rejected DSP/render attempt just because a later fire
    /// miss replaces the public diagnostic reason with a decode delay.
    dj_readiness_permanent_failure: Option<DjRuntimeRendererFailure>,
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

struct PreparedDjMixer {
    program: noor_mix::TransitionProgram,
    max_block_samples: usize,
    // Absolute output-clock origin survives decoder buffer compaction. The
    // program's start frame remains buffer-local for the captured Mixer deck.
    deck_a_output_start_frame: u64,
    /// The full transition mix, rendered at build (prepare/decode-complete)
    /// time rather than at fire time. Rendering 8-28s of dual-deck audio
    /// takes long enough that doing it inside the fire handler used to let
    /// deck A advance past the snapshot the render was built from, so the
    /// handoff audibly repeated ~100-300ms of the outgoing track. At install
    /// the buffer is joined by skipping however far deck A actually moved.
    rendered: Vec<f32>,
    current_track_id: i64,
    next_track_id: i64,
}

struct RuntimeDeckSnapshot {
    samples: Vec<f32>,
    start_frame: u64,
    output_start_frame: u64,
}

#[derive(Clone, Copy)]
struct CrossfadeReadinessSnapshot {
    base_ready: bool,
    finished: bool,
    unread_samples: u64,
    decoded_samples: u64,
    read_samples: u64,
    offset_samples: u64,
    start_threshold_samples: u64,
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
    current_engine_generation: Option<u64>,
    next_engine_generation: Option<u64>,
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
    /// The live deck A playhead is already past the midpoint of the rendered
    /// transition, so joining it would play only the tail of the blend.
    HandoffSeamTooLate,
    /// A late protected overlap could not be rebuilt; the live decks use
    /// the short seam ramps instead of reviving an unverified long overlap.
    ProtectedHandoffCut,
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
            Self::HandoffSeamTooLate => "handoff_seam_too_late",
            Self::ProtectedHandoffCut => "protected_handoff_cut",
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
        && failure.current_engine_generation
            == state.engine.as_ref().map(|engine| engine.generation)
        && failure.next_engine_generation
            == state.next_engine.as_ref().map(|engine| engine.generation)
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

fn snapshot_decoded_buffer(
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

fn active_deck_snapshot(
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
    build_prepared_dj_mixer_for_engine_at_start(
        state,
        transition,
        incoming,
        max_block_samples,
        false,
    )
}

fn build_prepared_dj_mixer_for_engine_at_start(
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
fn anchored_deck_a_frame(
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

fn anchored_deck_a_output_frame(
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

fn handoff_mixer_program(program: &noor_mix::TransitionProgram) -> bool {
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

fn overlay_mixer_program(program: &noor_mix::TransitionProgram) -> bool {
    matches!(program.template.as_str(), "DropTease16" | "DropPreview16")
        && deck_b_consumed_frames(program).is_some()
}

fn render_mixer_to_buffer(
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

fn arm_protected_handoff_cut(
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

fn install_prepared_handoff_mixer_buffer_once(
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
fn bake_seam_fade_in(rendered: &mut [f32], start_sample: usize, channels: usize, rate: u32) {
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
fn prepared_dj_mixer_matches_pair(state: &PlaybackRuntimeLoopState) -> bool {
    let Some(prepared) = state.prepared_dj_mixer.as_ref() else {
        return false;
    };
    let active_id = state.engine.as_ref().map(|engine| engine.track_id);
    let next_id = state.next_engine.as_ref().map(|engine| engine.track_id);
    active_id == Some(prepared.current_track_id) && next_id == Some(prepared.next_track_id)
}

fn can_prepare_dj_mixer_before_fire(state: &PlaybackRuntimeLoopState) -> bool {
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
fn dj_pcm_readiness_wakeup(state: &PlaybackRuntimeLoopState) -> Option<PlaybackRuntimeCommand> {
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

fn crossfade_readiness_snapshot(engine: &PlaybackEngine) -> Option<CrossfadeReadinessSnapshot> {
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

fn resolved_analysis_stream_in_state(
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

fn prepare_drop_preview_mixer(
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
fn update_prepared_transition_in_state(
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
fn runtime_transition_target_ms(
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
        dj_engine_enabled: config.dj_engine_enabled,
        dj_lookahead: None,
        dj_lookahead_failure: None,
        prepared_dj_mixer: None,
        prepared_drop_preview_mixer: None,
        last_dj_renderer_failure: None,
        dj_readiness_permanent_failure: None,
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
                            state.prepared_dj_mixer = None;
                            state.dj_readiness_permanent_failure = None;
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
                                    .last_dj_renderer_failure
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
    state.prepared_dj_mixer = None;
    state.prepared_drop_preview_mixer = None;
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
    state.prepared_dj_mixer = None;
    state.prepared_drop_preview_mixer = None;
    fade_out_and_stop(state.engine.take().into_iter().collect());
}

fn stop_all_engines(state: &mut PlaybackRuntimeLoopState) {
    state.dj_readiness_permanent_failure = None;
    state.prepared_dj_mixer = None;
    state.prepared_drop_preview_mixer = None;
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
    let lookahead = state.dj_lookahead.as_ref()?;
    (state.dj_engine_enabled
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
        let prepared = state.prepared_dj_mixer.as_ref()?;
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
    state.prepared_dj_mixer = None;
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

            state.dj_lookahead_failure = Some(DjLookaheadFailure {
                queue_generation: 20,
                current_queue_item_id: Some(11),
                next_queue_item_id: Some(12),
                reason: DjLookaheadFailureReason::AnalysisDeadlineMissed,
            });

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
        let prepared = state.prepared_dj_mixer.as_ref().expect("prepared mixer");
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
        let prepared = state.prepared_dj_mixer.as_ref().expect("prepared mixer");

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
        let prepared = state.prepared_dj_mixer.as_ref().expect("prepared mixer");
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
        assert!(state.prepared_dj_mixer.is_none());
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
                let kick = (0.4 * (2.0 * std::f64::consts::PI * 60.0 * t).cos() * (-t * 80.0).exp())
                    as f32;
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
        assert!(state.prepared_drop_preview_mixer.is_none());
        assert!(state.prepared_dj_mixer.is_none());
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
            next.job =
                PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
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
                incoming.shared.output_to_source_samples(
                    incoming.shared.total_samples.load(Ordering::Relaxed)
                ),
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
        let prepared = state.prepared_dj_mixer.as_ref().unwrap();
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
                        let kick = ((2.0 * std::f64::consts::PI * 60.0 * t).cos()
                            * (-t * 80.0).exp()) as f32;
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
            next.job =
                PreparedPlaybackJob::test_fixture(2, 21).with_prepared_transition(transition);
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
            let prepared = state.prepared_dj_mixer.as_ref().unwrap();
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
            let sync =
                beat_sync::synchronize_or_shorten_checked(&mut program, &outgoing, &incoming)
                    .expect("stable complete kick trains must verify without fallback");
            let verification_ms = verification_started.elapsed().as_secs_f64() * 1000.0;
            let verification_cpu_ms = cpu_delta(verification_cpu_started, thread_cpu_ms());
            assert_eq!(program.template, "BassSwap32");
            assert_eq!(program.resolve_at, SECONDS as u64 * u64::from(SAMPLE_RATE));
            assert!((f64::from(sync.rate) - 121.0 / 122.0).abs() < 0.001);
            assert!(sync.confidence >= 0.5);
            assert!(sync.residual_ms <= 40.0);
            noor_mix::planner::safety::validate_audio_safety(&program, &Default::default())
                .unwrap();

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
                let kick =
                    ((2.0 * std::f64::consts::PI * 60.0 * t).cos() * (-t * 80.0).exp()) as f32;
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
        let prepared = state.prepared_dj_mixer.as_ref().unwrap();
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
            (executed.decision.as_ref().unwrap().incoming_entry_seconds
                - source_cue as f32 / 48_000.0)
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
        let prepared = state.prepared_dj_mixer.as_ref().unwrap();
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
        let prepared = state.prepared_dj_mixer.as_ref().unwrap();
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
            state.prepared_dj_mixer.as_ref().unwrap().program.template,
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
        state.dj_engine_enabled = false;
        assert!(dj_pcm_readiness_wakeup(&state).is_none());
        state.dj_engine_enabled = true;
        state.dj_lookahead.as_mut().unwrap().queue_generation += 1;
        assert!(dj_pcm_readiness_wakeup(&state).is_none());
        state.dj_lookahead.as_mut().unwrap().queue_generation -= 1;
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
        assert!(state.prepared_dj_mixer.is_none());
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
        state.dj_lookahead.as_mut().unwrap().queue_generation += 1;
        assert_eq!(
            install_prepared_handoff_mixer_buffer(&mut state),
            Err(DjRuntimeRendererReason::PreparedMixerMissing)
        );
        assert!(state.prepared_dj_mixer.is_none());
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
        state.dj_lookahead.as_mut().unwrap().queue_generation -= 1;
        state.dj_lookahead.as_mut().unwrap().next = DjMediaRef::LibraryTrack { track_id: 3 };
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
                .prepared_dj_mixer
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

    fn native_adaptive_readiness_case()
    -> (PlaybackRuntimeLoopState, CrossfadeReadinessSnapshot, u64) {
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
        state.dj_lookahead.as_mut().unwrap().queue_generation += 1;
        assert!(!dj_crossfade_next_ready(&state, buffer, outgoing_window));
        state.dj_lookahead.as_mut().unwrap().queue_generation -= 1;
        state.dj_engine_enabled = false;
        assert!(!dj_crossfade_next_ready(&state, buffer, outgoing_window));
        state.dj_engine_enabled = true;
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
            dj_engine_enabled: true,
            dj_lookahead: None,
            dj_lookahead_failure: None,
            prepared_dj_mixer: None,
            prepared_drop_preview_mixer: None,
            last_dj_renderer_failure: None,
            dj_readiness_permanent_failure: None,
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
        job.source = crate::playback::player::PlaybackSourceRequest::TidalStream(
            StreamRequest::new(222, "LOSSLESS"),
        );
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
        transition.program = crate::playback::dj_engine::safe_crossfade_program(
            48_000,
            2,
            noor_mix::Policy::default(),
        );
        let gapless = GaplessPlan {
            enabled: true,
            overlap_ms: 6000,
            prebuffer_ms: 500,
            requires_stream_metadata: false,
        };
        state.next_engine.as_mut().unwrap().job.prepared_transition = Some(transition.clone());
        state.next_engine.as_mut().unwrap().job.gapless = gapless.clone();
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
        let mut slower = gapless.clone();
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
            &mut state,
            stale,
            gapless.clone()
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
            gapless.clone()
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
        let rendered = state.prepared_dj_mixer.as_ref().unwrap();
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
}
