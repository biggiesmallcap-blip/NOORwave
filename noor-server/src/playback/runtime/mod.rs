use crate::db::audio_settings::ExclusiveLatencyMode;
use crate::playback::output::cpal_shared::{SwapBackend, swap_stream_plan};
#[cfg(target_os = "windows")]
use crate::playback::output::wasapi_exclusive::{
    ExclusiveRenderRole, ExclusiveRenderSource, ExclusiveRuntimeSink, build_exclusive_stream,
};
use crate::playback::player::PreparedPlaybackJob;
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{SampleFormat, StreamConfig};

pub mod commands;
mod device;
mod engine;
pub(crate) mod shared;

pub use commands::{
    PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackTerminalReason, PlaybackTrackStatus,
};
pub use device::{OutputDeviceSelection, enumerate_output_devices};
use device::{device_display_name, resolve_device};
use engine::PlaybackEngine;
#[cfg(test)]
use engine::SwapPauseGuard;
pub(crate) use shared::PlaybackSharedState;
#[cfg(target_os = "windows")]
pub(crate) use shared::fill_f32_from_shared;

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct PlaybackRuntimeConfig {
    pub http_client: reqwest::Client,
    pub access_token: String,
    /// Channel to send mono audio samples for passive DSP analysis.
    /// (track_id, mono_samples, sample_rate)
    pub analysis_tx: Option<tokio::sync::mpsc::UnboundedSender<(i64, Vec<f32>, u32)>>,
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
            analysis_tx,
        }
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
    position_source: Arc<Mutex<Arc<AtomicU64>>>,
}

impl PlaybackRuntimeHandle {
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

    /// Seek to a position (milliseconds) within the current track.
    pub fn seek(&self, position_ms: i64) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Seek(position_ms))
    }

    /// Pre-decode the next track in the background so the transition is gapless.
    pub fn prepare_next(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::PrepareNext(job))
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

    let worker_volume_ctl = Arc::clone(&volume_ctl);
    let worker_initial_position = Arc::clone(&initial_position);
    let worker_position_source = Arc::clone(&position_source);

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
    /// Engine that's still audible during the crossfade fade-out window. It
    /// keeps producing audio (with a fade-out gain ramp) until its buffer
    /// drains, at which point it self-terminates and we drop it silently —
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
}

fn run_runtime_loop(
    config: PlaybackRuntimeConfig,
    command_rx: mpsc::Receiver<PlaybackRuntimeCommand>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    volume_ctl: Arc<AtomicU32>,
    position_samples: Arc<AtomicU64>,
    position_source: Arc<Mutex<Arc<AtomicU64>>>,
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
        fading_out_engine: None,
        current_exclusive: false,
        current_sample_rate_follow: false,
        current_device_selection: OutputDeviceSelection::Default,
        current_exclusive_release_grace_secs:
            crate::db::audio_settings::DEFAULT_EXCLUSIVE_RELEASE_GRACE_SECS,
        current_exclusive_latency_mode: ExclusiveLatencyMode::Stable,
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
                        false,
                    ) {
                        stop_all_engines(&mut state);
                        #[cfg(target_os = "windows")]
                        state.exclusive_sink.clear();
                        report_runtime_command_error(&event_tx, "Switch", error);
                    }
                }
                PlaybackRuntimeCommand::Seek(position_ms) => {
                    if let Some(engine) = state.engine.as_ref() {
                        let target_samples = (position_ms.max(0) as u64
                            * state.device_sample_rate as u64
                            * state.device_channels as u64)
                            / 1000;
                        // Tell the CPAL callback to seek on the next write. The
                        // callback will accept (and update position_samples) only
                        // if the target is within already-decoded samples or the
                        // buffer is finished; otherwise it warns and leaves the
                        // position counter untouched. This keeps the runtime-side
                        // position honest about what has actually been played.
                        // (Route/UI-side position handling is a separate follow-up.)
                        engine
                            .shared
                            .seek_target_samples
                            .store(target_samples, Ordering::Relaxed);
                        // Reset fire-once guards so NearEnd / CrossfadeStart re-fire correctly
                        // if the user seeks backward past those thresholds.
                        engine
                            .shared
                            .near_end_signaled
                            .store(false, Ordering::Relaxed);
                        engine
                            .shared
                            .crossfade_start_signaled
                            .store(false, Ordering::Relaxed);
                    }
                }
                PlaybackRuntimeCommand::PrepareNext(job) => {
                    // Only pre-decode if we don't already have a pending engine for this track.
                    let already_pending = state
                        .next_engine
                        .as_ref()
                        .map(|e| e.track_id == job.track.id && e.generation == job.generation)
                        .unwrap_or(false);
                    if !already_pending {
                        // Stop any stale pending engine first.
                        if let Some(mut stale) = state.next_engine.take() {
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
                PlaybackRuntimeCommand::CrossfadeStart {
                    track_id,
                    generation,
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
                        if next_ready {
                            promote_next_to_active(&mut state, &event_tx, &position_source);
                        }
                        // If not ready yet, NextDecodeComplete handles the late path.
                    }
                }
                PlaybackRuntimeCommand::NextDecodeComplete {
                    track_id,
                    generation,
                } => {
                    // Decode for the pre-decoded next engine completed. If the outgoing
                    // engine has already entered the crossfade window, promote now —
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
                        if crossfade_started {
                            promote_next_to_active(&mut state, &event_tx, &position_source);
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
                    // expected end of a crossfade — drop it silently. The queue
                    // advance already happened at promotion time via Finished.
                    let fading = state
                        .fading_out_engine
                        .as_ref()
                        .map(|e| (e.track_id, e.generation));
                    let next = state
                        .next_engine
                        .as_ref()
                        .map(|engine| (engine.track_id, engine.generation));
                    let active = state
                        .engine
                        .as_ref()
                        .map(|engine| (engine.track_id, engine.generation));

                    match terminal_engine_slot(active, next, fading, track_id, generation) {
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
                    // rate — runtime.rs has no view of the next track's StreamInfo
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
                        || state.fading_out_engine.is_some();
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
                    // ExclusiveModeFailed event), so a hard error here is rare —
                    // typically only a cpal shared build failure.
                    let mut swap_failed = false;
                    if exclusive {
                        #[cfg(target_os = "windows")]
                        {
                            for engine_slot in [
                                state.engine.as_mut(),
                                state.next_engine.as_mut(),
                                state.fading_out_engine.as_mut(),
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
    force_restart: bool,
) -> Result<()> {
    // No-op when state.engine is already playing the requested track. This
    // happens after a crossfade swap: promote_next_to_active emitted Finished
    // for the OUTGOING track, which caused routes to call switch_to(NEW track)
    // — but we already promoted that engine. Re-doing the swap would tear down
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
    // crossfade — kill the fading-out engine so it doesn't keep producing audio
    // underneath the new track.
    if let Some(mut prior) = state.fading_out_engine.take() {
        prior.stop();
    }

    // Reset position counter for the new track (safe: old engine is fully stopped above).
    position_samples.store(0, Ordering::SeqCst);

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
        // Cold start — stop any stale next_engine.
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
            state.engine = Some(eng);
            *position_source.lock().unwrap() = Arc::clone(position_samples);

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
) {
    let Some(next) = state.next_engine.take() else {
        return;
    };
    next.shared.fadein_start_samples.store(0, Ordering::Relaxed);
    next.shared.paused.store(false, Ordering::SeqCst);

    // Redirect the handle's position reader to the incoming engine's counter
    // BEFORE sliding it into state.engine so get_position_ms() immediately
    // reflects the new track starting from 0 instead of the fading-out track's
    // frozen end position.
    *position_source.lock().unwrap() = Arc::clone(&next.shared.position_samples);

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
        state.fading_out_engine = Some(outgoing);
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

fn promote_prepared_at_boundary(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
) {
    let Some(next) = state.next_engine.take() else {
        return;
    };
    next.shared
        .fadein_start_samples
        .store(u64::MAX, Ordering::Relaxed);
    next.shared.paused.store(false, Ordering::SeqCst);
    *position_source.lock().unwrap() = Arc::clone(&next.shared.position_samples);

    let outgoing = state.engine.take();
    state.engine = Some(next);

    if let Some(mut prior) = state.fading_out_engine.take() {
        prior.stop();
    }
    if let Some(mut outgoing) = outgoing {
        let outgoing_id = outgoing.track_id;
        let outgoing_generation = outgoing.generation;
        outgoing.stop();
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
}

fn terminal_engine_slot(
    active: Option<(i64, u64)>,
    next: Option<(i64, u64)>,
    fading: Option<(i64, u64)>,
    track_id: i64,
    generation: u64,
) -> Option<TerminalEngineSlot> {
    let target = Some((track_id, generation));
    if fading == target {
        Some(TerminalEngineSlot::FadingOut)
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
            terminal_engine_slot(Some((1, 1)), Some((2, 1)), None, 2, 1),
            Some(TerminalEngineSlot::Next)
        );
        assert_eq!(
            terminal_engine_slot(Some((1, 1)), Some((2, 1)), Some((3, 1)), 3, 1),
            Some(TerminalEngineSlot::FadingOut)
        );
        assert_eq!(
            terminal_engine_slot(Some((1, 1)), Some((2, 1)), Some((3, 1)), 4, 1),
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

        let sources = exclusive_render_sources(Some(&active), Some(&prepared), Some(&fading));

        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].role, ExclusiveRenderRole::Active);
        assert_eq!(sources[1].role, ExclusiveRenderRole::Prepared);
        assert_eq!(sources[2].role, ExclusiveRenderRole::Fading);
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

    fn test_runtime_loop_state() -> PlaybackRuntimeLoopState {
        PlaybackRuntimeLoopState {
            device_name: "test".to_string(),
            device_sample_rate: 48_000,
            device_channels: 2,
            #[cfg(target_os = "windows")]
            exclusive_sink: ExclusiveRuntimeSink::new(),
            engine: None,
            next_engine: None,
            fading_out_engine: None,
            current_exclusive: false,
            current_sample_rate_follow: false,
            current_device_selection: OutputDeviceSelection::Default,
            current_exclusive_release_grace_secs:
                crate::db::audio_settings::DEFAULT_EXCLUSIVE_RELEASE_GRACE_SECS,
            current_exclusive_latency_mode: ExclusiveLatencyMode::Stable,
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
            )),
        )
    }
}
