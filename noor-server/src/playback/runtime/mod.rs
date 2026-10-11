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

mod command_handlers;
#[cfg(target_os = "windows")]
mod exclusive_output;
mod handle;
mod job_switch;
mod promotion;
mod stall;
mod teardown;

use command_handlers::*;
#[cfg(target_os = "windows")]
use exclusive_output::*;
pub(crate) use handle::*;
use job_switch::*;
use promotion::*;
use stall::*;
use teardown::*;

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
            let env = LoopEnv {
                config: &mut config,
                command_tx: &command_tx,
                event_tx: &event_tx,
                device: &mut device,
                output_config: &mut output_config,
                output_sample_format: &mut output_sample_format,
                volume_ctl: &volume_ctl,
                position_samples: &position_samples,
                position_source: &position_source,
                buffered_source: &buffered_source,
                offset_source: &offset_source,
            };
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
                    return handle_seek_to(
                        env,
                        &mut state,
                        target_ms,
                        allow_segment_seek,
                        respond_to,
                    );
                }
                PlaybackRuntimeCommand::PrepareNext(job) => {
                    return handle_prepare_next(env, &mut state, job);
                }
                PlaybackRuntimeCommand::PrepareDropPreview(job) => {
                    return handle_prepare_drop_preview(env, &mut state, job);
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
                    return handle_crossfade_start(
                        env,
                        &mut state,
                        track_id,
                        generation,
                        trigger_position_samples,
                        trigger_target_samples,
                    );
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
                    return handle_drop_preview_start(
                        env,
                        &mut state,
                        track_id,
                        generation,
                        trigger_position_samples,
                    );
                }
                PlaybackRuntimeCommand::NextDecodeComplete {
                    track_id,
                    generation,
                } => {
                    return handle_next_decode_complete(env, &mut state, track_id, generation);
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
                    return handle_resume(env, &mut state);
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
                    return handle_track_terminal(env, &mut state, track_id, generation, outcome);
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
                    return handle_device_swap(
                        env,
                        &mut state,
                        selection,
                        exclusive,
                        sample_rate_follow,
                        desired_sample_rate,
                        exclusive_release_grace_secs,
                        exclusive_latency_mode,
                    );
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

#[cfg(test)]
mod tests;
