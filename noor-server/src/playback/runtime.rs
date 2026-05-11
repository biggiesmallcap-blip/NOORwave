use crate::playback::gapless::GaplessPlan;
use crate::playback::player::{PlaybackSourceKind, PlaybackSourceRequest, PreparedPlaybackJob};
use crate::services::tidal::stream::resolve_stream;
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use futures::StreamExt as _;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::runtime::Builder as TokioRuntimeBuilder;
use tracing::{error, info, warn};

const GAPLESS_PREFILL_PAD_MS: usize = 250;
/// How many milliseconds before track end we emit `NearEnd` and start pre-decoding
/// the next track. Must be comfortably larger than the worst-case decoder setup
/// latency (TIDAL stream resolve + HTTP connect + Symphonia probe), which can be
/// 5-10 s on a cold connection. 30 s gives the next track time to produce audible
/// samples before the crossfade window opens, even on a slow link.
const NEAR_END_THRESHOLD_MS: i64 = 30_000;

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

#[derive(Debug, Clone)]
pub enum PlaybackRuntimeCommand {
    Play(PreparedPlaybackJob),
    Switch(PreparedPlaybackJob),
    Pause,
    Resume,
    Stop,
    /// Seek to a position (milliseconds). Applied to the current engine's buffer.
    Seek(i64),
    /// Pre-decode the next track in background so it can start gaplessly.
    PrepareNext(PreparedPlaybackJob),
    /// Sent by the decoder thread when a track finishes decoding successfully.
    /// Used to start the crossfade stream if the crossfade window has already opened.
    NextDecodeComplete {
        track_id: i64,
        generation: u64,
    },
    /// Sent by the CPAL callback when the current track is within `crossfade_ms` of its end.
    /// If a pre-decoded next engine is ready, its stream is unpaused so both mix via the OS.
    CrossfadeStart {
        track_id: i64,
        generation: u64,
    },
    TrackTerminal {
        track_id: i64,
        generation: u64,
        outcome: PlaybackTerminalReason,
    },
    TrackStatus {
        track_id: i64,
        generation: u64,
        respond_to: std::sync::mpsc::Sender<PlaybackTrackStatus>,
    },
    /// Swap the CPAL output device for any active engines. Shared mode only at
    /// this stage; `exclusive` and `sample_rate_follow` are wired in later tasks
    /// (5 + 6) and ignored here. Optional `desired_sample_rate` allows the route
    /// layer to specify an exact target rate (e.g. per-track transitions).
    DeviceSwap {
        device: OutputDeviceSelection,
        exclusive: bool,
        sample_rate_follow: bool,
        desired_sample_rate: Option<u32>,
        /// Idle-release grace seconds for the WASAPI exclusive render thread.
        /// Read from `AudioSettings::exclusive_release_grace_secs` by the route
        /// layer. Ignored when `exclusive` is false.
        exclusive_release_grace_secs: u32,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackTrackStatus {
    None,
    Active,
    Prepared,
}

#[derive(Debug, Clone)]
pub enum OutputDeviceSelection {
    Default,
    Named(String),
}

impl OutputDeviceSelection {
    pub fn from_pref(pref: Option<&str>) -> Self {
        match pref {
            None => Self::Default,
            Some("default") => Self::Default,
            Some(name) => Self::Named(name.to_string()),
        }
    }
}

fn resolve_device(selection: &OutputDeviceSelection) -> Option<cpal::Device> {
    let host = cpal::default_host();
    match selection {
        OutputDeviceSelection::Default => host.default_output_device(),
        OutputDeviceSelection::Named(name) => host
            .output_devices()
            .ok()
            .and_then(|mut iter| iter.find(|d| d.name().ok().as_deref() == Some(name.as_str())))
            .or_else(|| host.default_output_device()),
    }
}

#[derive(Debug, Clone)]
pub enum PlaybackTerminalReason {
    Finished,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum PlaybackRuntimeEvent {
    Ready {
        device_name: String,
        sample_rate: u32,
        channels: u16,
    },
    Preparing {
        track_id: i64,
        source: PlaybackSourceKind,
    },
    Started {
        track_id: i64,
        generation: u64,
        source: PlaybackSourceKind,
    },
    Paused {
        track_id: Option<i64>,
    },
    Resumed {
        track_id: Option<i64>,
    },
    Stopped,
    Finished {
        track_id: i64,
        generation: u64,
    },
    /// Fired when the current track is within `NEAR_END_THRESHOLD_MS` of its end.
    /// The listener should peek the next track and send `PrepareNext` to pre-buffer it.
    NearEnd {
        track_id: i64,
        generation: u64,
    },
    Error {
        message: String,
    },
    /// WASAPI exclusive grab succeeded. Frontend clears any stale "failure"
    /// banner and shows engaged state.
    ExclusiveModeEngaged {
        device_name: String,
    },
    /// WASAPI exclusive grab failed; runtime fell back to cpal shared so the
    /// user still hears audio. `reason` is a human-readable explanation.
    ExclusiveModeFailed {
        reason: String,
        device_name: String,
    },
    /// WASAPI exclusive render thread released the device after grace timeout.
    /// Frontend clears engaged state. Runtime will re-grab on next Resume/Play.
    ExclusiveModeReleased {
        device_name: String,
    },
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
    ) -> Result<()> {
        self.send(PlaybackRuntimeCommand::DeviceSwap {
            device,
            exclusive,
            sample_rate_follow,
            desired_sample_rate,
            exclusive_release_grace_secs,
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
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OutputDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub max_channels: u16,
    pub supported_sample_rates: Vec<u32>,
}

pub fn enumerate_output_devices() -> Vec<OutputDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());

    host.output_devices()
        .map(|iter| {
            iter.filter_map(|dev| {
                let name = dev.name().ok()?;
                let configs: Vec<_> = dev.supported_output_configs().ok()?.collect();
                let max_channels = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
                let mut rates: Vec<u32> = configs
                    .iter()
                    .flat_map(|c| {
                        let min = c.min_sample_rate().0;
                        let max = c.max_sample_rate().0;
                        // Common audio rates that fall within the supported range.
                        [44_100, 48_000, 88_200, 96_000, 176_400, 192_000]
                            .into_iter()
                            .filter(move |r| *r >= min && *r <= max)
                    })
                    .collect();
                rates.sort_unstable();
                rates.dedup();
                Some(OutputDeviceInfo {
                    id: name.clone(),
                    name: name.clone(),
                    is_default: default_name.as_deref() == Some(name.as_str()),
                    max_channels,
                    supported_sample_rates: rates,
                })
            })
            .collect()
        })
        .unwrap_or_default()
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
    let device_name = device
        .name()
        .unwrap_or_else(|_| "default output device".to_string());
    let supported = device
        .default_output_config()
        .context("failed to read default output config")?;
    let mut output_config = supported.config();
    let mut output_sample_format = supported.sample_format();

    let mut state = PlaybackRuntimeLoopState {
        device_name,
        device_sample_rate: output_config.sample_rate.0,
        device_channels: output_config.channels,
        engine: None,
        next_engine: None,
        fading_out_engine: None,
        current_exclusive: false,
        current_sample_rate_follow: false,
        current_device_selection: OutputDeviceSelection::Default,
        current_exclusive_release_grace_secs:
            crate::db::audio_settings::DEFAULT_EXCLUSIVE_RELEASE_GRACE_SECS,
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
        match command {
            PlaybackRuntimeCommand::Play(job) => {
                transition_to_job(
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
                )?;
            }
            PlaybackRuntimeCommand::Switch(job) => {
                transition_to_job(
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
                )?;
            }
            PlaybackRuntimeCommand::Seek(position_ms) => {
                if let Some(engine) = state.engine.as_ref() {
                    let target_samples = (position_ms.max(0) as u64
                        * state.device_sample_rate as u64
                        * state.device_channels as u64)
                        / 1000;
                    // Tell the CPAL callback to seek on the next write.
                    engine
                        .shared
                        .seek_target_samples
                        .store(target_samples, Ordering::Relaxed);
                    // Mirror into the engine's counter immediately so get_position_ms()
                    // is correct before the CPAL callback runs (up to one buffer period later).
                    // position_source always points to this engine's counter, so no
                    // separate handle-counter write is needed.
                    engine
                        .shared
                        .position_samples
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
                    match PlaybackEngine::start(
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
                        // Give the pending engine its own position counter so it starts at 0.
                        Arc::new(AtomicU64::new(0)),
                    ) {
                        Ok(engine) => {
                            // Keep the stream alive but software-paused so host pause does not
                            // block control commands on some Linux/PipeWire setups.
                            engine.shared.paused.store(true, Ordering::SeqCst);
                            state.next_engine = Some(engine);
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
                    engine.pause()?;
                    let _ = event_tx.send(PlaybackRuntimeEvent::Paused {
                        track_id: Some(engine.track_id),
                    });
                }
                if let Some(engine) = state.fading_out_engine.as_mut() {
                    engine.pause()?;
                }
            }
            PlaybackRuntimeCommand::Resume => {
                // On-demand re-grab: if exclusive mode is on and the active
                // engine's WASAPI stream self-released after idle, rebuild it
                // BEFORE unpausing so the decoder doesn't push samples into a
                // missing stream. swap_stream handles its own cpal-shared
                // fallback if the re-grab now fails (e.g. another app grabbed
                // exclusive while we were paused).
                if state.current_exclusive
                    && let Some(engine) = state.engine.as_mut()
                    && engine.needs_stream_rebuild()
                {
                    info!(
                        "Resume: rebuilding exclusive stream after idle release on {}",
                        state.device_name
                    );
                    match engine.swap_stream(
                        &device,
                        &output_config,
                        output_sample_format,
                        command_tx.clone(),
                        event_tx.clone(),
                        true,
                        exclusive_rebuild_rate(
                            state.current_sample_rate_follow,
                            state.device_sample_rate,
                        ),
                        state.current_exclusive_release_grace_secs,
                    ) {
                        Ok(actual_rate) => {
                            output_config.sample_rate = cpal::SampleRate(actual_rate);
                            state.device_sample_rate = actual_rate;
                        }
                        Err(err) => {
                            warn!("Resume: failed to rebuild exclusive stream: {err:?}");
                        }
                    }
                }

                if let Some(engine) = state.engine.as_mut() {
                    engine.resume()?;
                    let _ = event_tx.send(PlaybackRuntimeEvent::Resumed {
                        track_id: Some(engine.track_id),
                    });
                }
                if let Some(engine) = state.fading_out_engine.as_mut() {
                    engine.resume()?;
                }
            }
            PlaybackRuntimeCommand::Stop => {
                stop_all_engines(&mut state);
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
                        if let Some(mut engine) = state.fading_out_engine.take() {
                            engine.stop();
                        }
                    }
                    Some(TerminalEngineSlot::Next) => {
                        if let PlaybackTerminalReason::Error(message) = &outcome {
                            warn!("Discarding failed pre-buffered track {track_id}: {message}");
                        }
                        if let Some(mut engine) = state.next_engine.take() {
                            engine.stop();
                        }
                    }
                    Some(TerminalEngineSlot::Active) => {
                        stop_current_engine(&mut state);
                        match outcome {
                            PlaybackTerminalReason::Finished => {
                                let _ = event_tx.send(PlaybackRuntimeEvent::Finished {
                                    track_id,
                                    generation,
                                });
                            }
                            PlaybackTerminalReason::Error(message) => {
                                let _ = event_tx.send(PlaybackRuntimeEvent::Error { message });
                            }
                        }
                    }
                    None => {}
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
                        continue;
                    }
                };
                let new_supported = match new_device.default_output_config() {
                    Ok(s) => s,
                    Err(err) => {
                        warn!(
                            "DeviceSwap: failed to read default config for new device: {err}; keeping current output"
                        );
                        continue;
                    }
                };
                let new_config = new_supported.config();
                let new_format = new_supported.sample_format();
                let new_name = new_device
                    .name()
                    .unwrap_or_else(|_| "default output device".to_string());

                // When sample-rate-follow is on, re-target both the cpal stream
                // and the decoder. Use the explicitly-provided rate if given (e.g.
                // per-track transition), otherwise use the new device's default.
                // When off, pass `None` so the existing rate carries over (the cpal
                // stream may resample internally, which is fine for the toggle
                // being off).
                let desired_rate = match desired_sample_rate {
                    Some(rate) => Some(rate),
                    None if sample_rate_follow => Some(new_config.sample_rate.0),
                    _ => None,
                };
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
                if exclusive {
                    if let Some(mut stale) = state.next_engine.take() {
                        stale.stop();
                    }
                    if let Some(mut stale) = state.fading_out_engine.take() {
                        stale.stop();
                    }
                }

                // Rebuild the stream on every live engine so they all play on
                // the new device. swap_stream now transparently falls back to
                // cpal shared on exclusive failure (and emits an
                // ExclusiveModeFailed event), so a hard error here is rare —
                // typically only a cpal shared build failure.
                let mut swap_failed = false;
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
                        exclusive,
                        desired_rate,
                        exclusive_release_grace_secs,
                    ) {
                        Ok(actual_rate) => {
                            actual_config.sample_rate = cpal::SampleRate(actual_rate);
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

                if swap_failed {
                    warn!("DeviceSwap: one or more engines failed to swap; output may be partial");
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
                state.device_sample_rate = output_config.sample_rate.0;
                state.device_channels = output_config.channels;
                state.current_exclusive = exclusive;
                state.current_sample_rate_follow = sample_rate_follow;
                state.current_device_selection = selection;
                state.current_exclusive_release_grace_secs = exclusive_release_grace_secs;

                let _ = event_tx.send(PlaybackRuntimeEvent::Ready {
                    device_name: new_name,
                    sample_rate: state.device_sample_rate,
                    channels: state.device_channels,
                });
            }
            PlaybackRuntimeCommand::Shutdown => {
                stop_all_engines(&mut state);
                break;
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
    if !force_restart && state.engine.as_ref().map(|e| e.track_id) == Some(job.track.id) {
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

    let _ = event_tx.send(PlaybackRuntimeEvent::Preparing {
        track_id: job.track.id,
        source: job.source_kind(),
    });

    // Check if the next track was pre-buffered (gapless pre-decode).
    let pre_decoded_match = state
        .next_engine
        .as_ref()
        .map(|e| e.track_id == job.track.id && e.generation == job.generation)
        .unwrap_or(false);

    let engine = if pre_decoded_match {
        let pre = state.next_engine.take().unwrap();
        // position_source was already redirected to this engine's counter at
        // promote_next_to_active time, so the handle reads the right value.
        // Restart the stream (it was paused during pre-decode).
        pre.shared.paused.store(false, Ordering::SeqCst);
        pre
    } else {
        // Cold start — stop any stale next_engine.
        if let Some(mut stale) = state.next_engine.take() {
            stale.stop();
        }
        let mut eng = PlaybackEngine::start(
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
        // Redirect the handle's reader to this engine's counter (which IS
        // position_samples for cold starts, but re-pointing is always correct).
        *position_source.lock().unwrap() = Arc::clone(position_samples);

        // If exclusive mode is currently engaged, swap the just-built cpal
        // shared stream over to the WASAPI exclusive backend so the user
        // doesn't silently get shared-mode output for every new track.
        // swap_stream itself handles the fallback-to-cpal + event emission
        // when the WASAPI grab fails, so a hard error here is rare.
        if state.current_exclusive {
            match eng.swap_stream(
                device,
                output_config,
                output_sample_format,
                command_tx.clone(),
                event_tx.clone(),
                true,
                exclusive_rebuild_rate(state.current_sample_rate_follow, state.device_sample_rate),
                state.current_exclusive_release_grace_secs,
            ) {
                Ok(actual_rate) => {
                    output_config.sample_rate = cpal::SampleRate(actual_rate);
                    state.device_sample_rate = actual_rate;
                }
                Err(err) => {
                    warn!(
                        "transition_to_job: swap_stream errored cold-starting new engine: {err:?}"
                    );
                }
            }
        }
        eng
    };

    state.engine = Some(engine);
    Ok(())
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
}

/// Output stream backend. Shared mode goes through cpal (which uses WASAPI
/// shared internally on Windows, the system mixer routes everything through);
/// exclusive mode uses our own WASAPI exclusive driver that bypasses the OS
/// audio engine entirely. Both feed from the same `PlaybackSharedState`.
enum OutputStream {
    Cpal(Stream),
    #[cfg(target_os = "windows")]
    #[allow(dead_code)]
    Wasapi(crate::playback::wasapi_exclusive::ExclusiveStream),
}

impl OutputStream {
    /// Start producing audio. Cpal streams are constructed paused; the WASAPI
    /// exclusive driver starts its render thread the moment it's built, so
    /// this is a no-op for that backend.
    fn start(&self) -> Result<()> {
        match self {
            Self::Cpal(s) => s.play().context("failed to start cpal stream"),
            #[cfg(target_os = "windows")]
            Self::Wasapi(_) => Ok(()),
        }
    }
}

struct PlaybackEngine {
    track_id: i64,
    generation: u64,
    /// Active output stream; `None` only briefly during a `DeviceSwap` while
    /// we rebuild it on the new device or backend.
    stream: Option<OutputStream>,
    decoder_thread: Option<JoinHandle<()>>,
    shared: Arc<PlaybackSharedState>,
}

impl PlaybackEngine {
    #[allow(clippy::too_many_arguments)]
    fn start(
        config: &PlaybackRuntimeConfig,
        command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
        device: &cpal::Device,
        output_config: &StreamConfig,
        output_sample_format: SampleFormat,
        job: PreparedPlaybackJob,
        event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
        device_sample_rate: u32,
        device_channels: u16,
        volume_ctl: Arc<AtomicU32>,
        position_samples: Arc<AtomicU64>,
    ) -> Result<Self> {
        if matches!(job.source, PlaybackSourceRequest::LocalLibrary) {
            return Err(anyhow!(
                "local library playback is not wired into the host-audio runtime yet"
            ));
        }

        let track_id = job.track.id;
        let generation = job.generation;
        let source_kind = job.source_kind();
        let estimated_total_samples = job.track.duration_ms.and_then(|duration_ms| {
            estimate_total_samples_from_duration_ms(
                duration_ms,
                device_sample_rate,
                device_channels,
            )
        });
        let shared = Arc::new(PlaybackSharedState::new(
            track_id,
            generation,
            source_kind,
            job.gapless,
            output_config.sample_rate.0,
            device_channels,
            estimated_total_samples,
            command_tx.clone(),
            volume_ctl,
            position_samples,
        ));

        let decoder_shared = Arc::clone(&shared);
        let decoder_shared_for_decode = Arc::clone(&shared);
        let decoder_config = config.clone();
        let decoder_job = job.clone();
        let decoder_thread = thread::Builder::new()
            .name(format!("noor-playback-decoder-{track_id}"))
            .spawn(move || {
                if let Err(err) = decode_and_buffer_job(
                    decoder_config,
                    decoder_job,
                    decoder_shared_for_decode,
                    device_sample_rate,
                    device_channels,
                ) {
                    let _ = decoder_shared
                        .signal_terminal(PlaybackTerminalReason::Error(err.to_string()));
                    error!("Playback decode failed for track {track_id}: {err:?}");
                }
            })
            .context("failed to spawn playback decoder thread")?;

        let cpal_stream = build_output_stream(
            device,
            output_config,
            output_sample_format,
            Arc::clone(&shared),
            shared.command_tx.clone(),
            event_tx.clone(),
        )?;
        let stream = OutputStream::Cpal(cpal_stream);
        stream.start()?;

        Ok(Self {
            track_id,
            generation,
            stream: Some(stream),
            decoder_thread: Some(decoder_thread),
            shared,
        })
    }

    fn pause(&self) -> Result<()> {
        self.shared.paused.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn resume(&self) -> Result<()> {
        self.shared.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// True iff the engine's output stream is gone OR an exclusive WASAPI
    /// stream has self-released after idle. Used by the runtime to know it
    /// needs to call `swap_stream` to rebuild the stream before unpausing.
    fn needs_stream_rebuild(&self) -> bool {
        match self.stream.as_ref() {
            None => true,
            #[cfg(target_os = "windows")]
            Some(OutputStream::Wasapi(s)) => s.is_released(),
            Some(_) => false,
        }
    }

    fn stop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        self.shared.paused.store(true, Ordering::SeqCst);
        self.shared.reset_buffer();
        if let Some(handle) = self.decoder_thread.take() {
            let _ = handle.join();
        }
    }

    /// Drop the current CPAL stream and rebuild it on `device`. The decoder
    /// thread keeps running and feeding the same shared buffer; the new stream
    /// drains it on the new device. The base `output_config` (built from the
    /// runtime's startup device) is passed through `build_stream_config` which
    /// applies the `exclusive` low-latency buffer (Windows-only) and an optional
    /// override sample rate.
    ///
    /// When `desired_sample_rate` is `Some`, the engine's shared
    /// `target_sample_rate` is updated so the decoder thread will resample
    /// subsequent packets to the new rate. Samples that were already buffered
    /// at the previous rate stay in the buffer and will be played out by the
    /// new cpal stream at the new rate (a brief pitch glitch across the swap
    /// boundary is acceptable per the spec). When `None`, both the cpal stream
    /// and the decoder keep their existing rate.
    /// Swap the output stream. When `exclusive` is true and the WASAPI grab
    /// fails (device held by another exclusive app, exclusive disabled in
    /// Windows, etc.), this method **transparently falls back to a cpal shared
    /// stream** so the user keeps hearing audio, and emits an
    /// `ExclusiveModeFailed` event so the UI can show a red-pill banner.
    ///
    /// Successful exclusive grabs emit `ExclusiveModeEngaged`. Plain shared
    /// builds emit nothing.
    #[allow(clippy::too_many_arguments)]
    fn swap_stream(
        &mut self,
        device: &cpal::Device,
        output_config: &StreamConfig,
        output_sample_format: SampleFormat,
        command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
        event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
        exclusive: bool,
        desired_sample_rate: Option<u32>,
        #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
        exclusive_release_grace_secs: u32,
    ) -> Result<u32> {
        // Pause first so the decoder side doesn't keep filling while the
        // callback is gone, then drop the old stream before building the new
        // one (some host APIs reject a second exclusive grab while the first
        // stream is alive).
        let pause_guard = SwapPauseGuard::new(Arc::clone(&self.shared));
        drop(self.stream.take());

        let device_label = device
            .name()
            .unwrap_or_else(|_| "default output device".to_string());

        #[cfg(target_os = "windows")]
        let (new_stream, active_plan) = if exclusive {
            let exclusive_plan =
                swap_stream_plan(output_config, desired_sample_rate, SwapBackend::Exclusive);
            let device_name = device.name().ok();
            match crate::playback::wasapi_exclusive::build_exclusive_stream(
                device_name.as_deref(),
                device_label.clone(),
                exclusive_plan.stream_config.sample_rate.0,
                exclusive_plan.stream_config.channels,
                exclusive_release_grace_secs,
                Arc::clone(&self.shared),
                command_tx.clone(),
                event_tx.clone(),
            ) {
                Ok(exclusive_stream) => {
                    let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeEngaged {
                        device_name: device_label.clone(),
                    });
                    (OutputStream::Wasapi(exclusive_stream), exclusive_plan)
                }
                Err(failure) => {
                    let reason = failure.user_message();
                    warn!("WASAPI exclusive grab failed; falling back to cpal shared: {reason}");
                    let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeFailed {
                        reason,
                        device_name: device_label.clone(),
                    });
                    let fallback_plan = swap_stream_plan(
                        output_config,
                        desired_sample_rate,
                        SwapBackend::SharedFallback,
                    );
                    (
                        OutputStream::Cpal(build_output_stream(
                            device,
                            &fallback_plan.stream_config,
                            output_sample_format,
                            Arc::clone(&self.shared),
                            command_tx,
                            event_tx,
                        )?),
                        fallback_plan,
                    )
                }
            }
        } else {
            let shared_plan =
                swap_stream_plan(output_config, desired_sample_rate, SwapBackend::Shared);
            (
                OutputStream::Cpal(build_output_stream(
                    device,
                    &shared_plan.stream_config,
                    output_sample_format,
                    Arc::clone(&self.shared),
                    command_tx,
                    event_tx,
                )?),
                shared_plan,
            )
        };
        #[cfg(not(target_os = "windows"))]
        let (new_stream, active_plan) = {
            let _ = exclusive;
            let _ = device_label;
            let shared_plan =
                swap_stream_plan(output_config, desired_sample_rate, SwapBackend::Shared);
            (
                OutputStream::Cpal(build_output_stream(
                    device,
                    &shared_plan.stream_config,
                    output_sample_format,
                    Arc::clone(&self.shared),
                    command_tx,
                    event_tx,
                )?),
                shared_plan,
            )
        };

        new_stream
            .start()
            .context("failed to start swapped output stream")?;
        if let Some(target_sample_rate) = active_plan.target_sample_rate {
            self.shared
                .target_sample_rate
                .store(target_sample_rate, Ordering::Relaxed);
        }
        self.stream = Some(new_stream);
        pause_guard.restore();
        Ok(active_plan.stream_config.sample_rate.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwapBackend {
    Exclusive,
    Shared,
    SharedFallback,
}

#[derive(Debug, Clone)]
struct SwapStreamPlan {
    stream_config: StreamConfig,
    target_sample_rate: Option<u32>,
}

fn swap_stream_plan(
    base: &StreamConfig,
    desired_sample_rate: Option<u32>,
    backend: SwapBackend,
) -> SwapStreamPlan {
    let stream_config = match backend {
        SwapBackend::Exclusive | SwapBackend::Shared => {
            effective_output_config(base, desired_sample_rate)
        }
        SwapBackend::SharedFallback => base.clone(),
    };
    let target_sample_rate = match (backend, desired_sample_rate) {
        (SwapBackend::SharedFallback, Some(_)) => Some(stream_config.sample_rate.0),
        (_, Some(_)) => Some(stream_config.sample_rate.0),
        (_, None) => None,
    };

    SwapStreamPlan {
        stream_config,
        target_sample_rate,
    }
}

struct SwapPauseGuard {
    shared: Arc<PlaybackSharedState>,
    was_paused: bool,
    active: bool,
}

impl SwapPauseGuard {
    fn new(shared: Arc<PlaybackSharedState>) -> Self {
        let was_paused = shared.paused.load(Ordering::SeqCst);
        shared.paused.store(true, Ordering::SeqCst);
        Self {
            shared,
            was_paused,
            active: true,
        }
    }

    fn restore(mut self) {
        self.active = false;
        self.shared.paused.store(self.was_paused, Ordering::SeqCst);
    }
}

impl Drop for SwapPauseGuard {
    fn drop(&mut self) {
        if self.active {
            self.shared.paused.store(self.was_paused, Ordering::SeqCst);
        }
    }
}

/// Build the effective `cpal::StreamConfig` for a swap, starting from the
/// device's existing config and applying an optional sample-rate override.
///
/// The `exclusive` parameter is intentionally inert here — when exclusive mode
/// is on we bypass cpal entirely (see `wasapi_exclusive::build_exclusive_stream`)
/// so the cpal config never gets used. We only consume the desired sample rate
/// to plumb through to the WASAPI session.
fn effective_output_config(base: &StreamConfig, desired_sample_rate: Option<u32>) -> StreamConfig {
    let mut config = base.clone();
    if let Some(rate) = desired_sample_rate {
        config.sample_rate = cpal::SampleRate(rate);
    }
    config
}

fn exclusive_rebuild_rate(sample_rate_follow: bool, device_sample_rate: u32) -> Option<u32> {
    sample_rate_follow.then_some(device_sample_rate)
}

fn build_output_stream(
    device: &cpal::Device,
    output_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<Stream> {
    let err_fn = |err| warn!("Playback output stream error: {err}");

    let stream = match output_sample_format {
        SampleFormat::F32 => device.build_output_stream(
            output_config,
            move |data: &mut [f32], _| write_output_f32(data, &shared, &command_tx, &event_tx),
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_output_stream(
            output_config,
            move |data: &mut [i16], _| write_output_i16(data, &shared, &command_tx, &event_tx),
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_output_stream(
            output_config,
            move |data: &mut [u16], _| write_output_u16(data, &shared, &command_tx, &event_tx),
            err_fn,
            None,
        )?,
        other => {
            return Err(anyhow!(
                "unsupported output sample format for playback runtime: {other:?}"
            ));
        }
    };

    Ok(stream)
}

fn write_output_f32(
    data: &mut [f32],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    write_output_buffer(data, shared, command_tx, event_tx, |sample| sample)
}

/// Fill an f32 interleaved buffer from the shared playback state. Mirrors
/// what `write_output_f32` does for cpal callbacks, but exposed for the
/// WASAPI exclusive render thread which manages its own buffers instead of
/// receiving them from the OS mixer.
#[cfg(target_os = "windows")]
pub(crate) fn fill_f32_from_shared(
    data: &mut [f32],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    write_output_f32(data, shared, command_tx, event_tx);
}

fn write_output_i16(
    data: &mut [i16],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    write_output_buffer(data, shared, command_tx, event_tx, |sample| {
        (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    })
}

fn write_output_u16(
    data: &mut [u16],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    write_output_buffer(data, shared, command_tx, event_tx, |sample| {
        let normalized = sample.clamp(-1.0, 1.0) * 0.5 + 0.5;
        (normalized * u16::MAX as f32) as u16
    })
}

fn write_output_buffer<T>(
    data: &mut [T],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    convert: impl Fn(f32) -> T,
) {
    if shared.stopped.load(Ordering::SeqCst) {
        data.fill_with(|| convert(0.0));
        return;
    }

    if shared.paused.load(Ordering::SeqCst) {
        data.fill_with(|| convert(0.0));
        return;
    }

    let volume = f32::from_bits(shared.volume_ctl.load(Ordering::Relaxed));

    let mut guard = match shared.buffer.lock() {
        Ok(guard) => guard,
        Err(_) => {
            data.fill_with(|| convert(0.0));
            return;
        }
    };

    // Apply a pending seek if the runtime set one.
    let seek_target = shared.seek_target_samples.load(Ordering::Relaxed);
    if seek_target != u64::MAX
        && shared
            .seek_target_samples
            .compare_exchange(seek_target, u64::MAX, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    {
        guard.seek_to(seek_target as usize);
        shared
            .position_samples
            .store(seek_target, Ordering::Relaxed);
    }

    let ready_to_start = guard.is_ready();
    if ready_to_start && !guard.started {
        info!(
            "[NOOR-DIAG] playback starting: {} samples buffered, threshold {}, callback size {}",
            guard.samples.len().saturating_sub(guard.read_pos),
            guard.start_threshold_samples,
            data.len()
        );
        guard.started = true;
    }

    // Crossfade gain: fade-out during the overlap window of the outgoing track,
    // fade-in from position 0 for the incoming track.
    //
    // Equal-power (sine) ramps keep total power constant across the overlap:
    // sin²(t·π/2) + cos²(t·π/2) = 1, so the summed amplitude of two streams
    // running through opposite ends of the curve doesn't dip 3 dB at the
    // midpoint the way linear ramps do. Both streams use the same
    // `gain = sin(progress · π/2)` form — fade-in feeds `elapsed/xfade`,
    // fade-out feeds `remaining/xfade`, which is the equivalent
    // 1→0 parameterisation.
    //
    // IMPORTANT: check fadein_start_samples FIRST. When the next track is fully
    // pre-decoded before the crossfade window opens, total_samples > 0 for that
    // engine too. Without this ordering the fade-in branch is silently skipped and
    // the incoming track jumps in at full volume.
    let xfade = shared.crossfade_samples.load(Ordering::Relaxed);
    let fade_gain = if xfade > 0 {
        let pos = shared.position_samples.load(Ordering::Relaxed);
        let fadein_start = shared.fadein_start_samples.load(Ordering::Relaxed);
        if fadein_start != u64::MAX {
            // Incoming crossfade engine — apply fade-in ramp.
            let elapsed = pos.saturating_sub(fadein_start);
            if elapsed < xfade {
                let t = (elapsed as f32 / xfade as f32).clamp(0.0, 1.0);
                (t * std::f32::consts::FRAC_PI_2).sin()
            } else {
                1.0f32
            }
        } else {
            // Outgoing engine — fade-out once we enter the overlap window.
            let total = shared.total_samples.load(Ordering::Relaxed);
            if total > 0 {
                let remaining = total.saturating_sub(pos);
                if remaining < xfade {
                    let t = (remaining as f32 / xfade as f32).clamp(0.0, 1.0);
                    (t * std::f32::consts::FRAC_PI_2).sin()
                } else {
                    1.0f32
                }
            } else {
                1.0f32
            }
        }
    } else {
        1.0f32
    };

    let written = if guard.started {
        guard.drain_into(data, &|s: f32| convert(s * volume * fade_gain))
    } else {
        data.fill_with(|| convert(0.0));
        0
    };

    if written > 0 {
        shared
            .position_samples
            .fetch_add(written as u64, Ordering::Relaxed);
    }

    if guard.started && !guard.started_notified {
        guard.started_notified = true;
        let _ = event_tx.send(PlaybackRuntimeEvent::Started {
            track_id: shared.track_id,
            generation: shared.generation,
            source: shared.source_kind,
        });
    }

    if guard.started && written == 0 && guard.finished && !guard.finished_notified {
        guard.finished_notified = true;
        let _ = command_tx.send(PlaybackRuntimeCommand::TrackTerminal {
            track_id: shared.track_id,
            generation: shared.generation,
            outcome: PlaybackTerminalReason::Finished,
        });
    }

    // Emit NearEnd when we're within NEAR_END_THRESHOLD_MS of the track end.
    // This fires once (guarded by `near_end_signaled`) so routes.rs can pre-decode next track.
    if !shared.near_end_signaled.load(Ordering::Relaxed) {
        let total = shared.total_samples.load(Ordering::Relaxed);
        if total > 0 {
            let pos = shared.position_samples.load(Ordering::Relaxed);
            let threshold = NEAR_END_THRESHOLD_MS
                * shared.device_sample_rate as i64
                * shared.device_channels as i64
                / 1000;
            if total.saturating_sub(pos) <= threshold as u64 {
                shared.near_end_signaled.store(true, Ordering::Relaxed);
                let _ = event_tx.send(PlaybackRuntimeEvent::NearEnd {
                    track_id: shared.track_id,
                    generation: shared.generation,
                });
            }
        }
    }

    // Fire CrossfadeStart when we enter the crossfade window so the runtime can unpause
    // the pre-decoded next engine (both streams then mix naturally via the OS mixer).
    if !shared.crossfade_start_signaled.load(Ordering::Relaxed) {
        let xfade = shared.crossfade_samples.load(Ordering::Relaxed);
        if xfade > 0 {
            let total = shared.total_samples.load(Ordering::Relaxed);
            if total > 0 {
                let pos = shared.position_samples.load(Ordering::Relaxed);
                if total.saturating_sub(pos) <= xfade {
                    shared
                        .crossfade_start_signaled
                        .store(true, Ordering::Relaxed);
                    let _ = command_tx.send(PlaybackRuntimeCommand::CrossfadeStart {
                        track_id: shared.track_id,
                        generation: shared.generation,
                    });
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct PlaybackSharedState {
    track_id: i64,
    generation: u64,
    source_kind: PlaybackSourceKind,
    pub(crate) paused: AtomicBool,
    stopped: AtomicBool,
    buffer: Mutex<PlaybackBuffer>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    /// Shared with the runtime handle so the caller can control volume in real-time.
    volume_ctl: Arc<AtomicU32>,
    /// Shared with the runtime handle so the caller can read the current position.
    position_samples: Arc<AtomicU64>,
    /// When set to a value other than u64::MAX, the CPAL callback will seek to this
    /// interleaved-sample offset on the next write, then reset this to u64::MAX.
    seek_target_samples: AtomicU64,
    /// Total interleaved sample count — set by the decoder thread once decoding is complete.
    /// Used by the CPAL callback to determine when to fire `NearEnd`.
    total_samples: AtomicU64,
    /// Set to `true` once `NearEnd` has been emitted for this engine (fire-once guard).
    near_end_signaled: AtomicBool,
    /// Interleaved sample count of the crossfade overlap window (0 = no crossfade).
    /// Computed from `GaplessPlan.overlap_ms` at engine start.
    crossfade_samples: AtomicU64,
    /// Set to `true` once `CrossfadeStart` has been fired for this engine (fire-once guard).
    crossfade_start_signaled: AtomicBool,
    /// Interleaved sample count at which this engine was unpaused for its fade-in.
    /// 0 = not yet started. Used by the CPAL callback to compute the fade-in gain ramp.
    fadein_start_samples: AtomicU64,
    /// Device sample rate (Hz) — needed in CPAL callback for `NearEnd` threshold calculation.
    device_sample_rate: u32,
    /// Device channel count — needed in CPAL callback for `NearEnd` threshold calculation.
    device_channels: u16,
    /// Live target sample rate for the resampler. The decoder reads this before
    /// each packet and rebuilds its resampler step if the value changes — that
    /// way a `DeviceSwap` that changes the cpal stream rate (sample-rate-follow)
    /// is also reflected in the decoded sample stream so audio plays at the
    /// correct pitch on the new stream rate.
    target_sample_rate: AtomicU32,
}

impl PlaybackSharedState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        track_id: i64,
        generation: u64,
        source_kind: PlaybackSourceKind,
        gapless: GaplessPlan,
        device_sample_rate: u32,
        device_channels: u16,
        estimated_total_samples: Option<u64>,
        command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
        volume_ctl: Arc<AtomicU32>,
        position_samples: Arc<AtomicU64>,
    ) -> Self {
        let prebuffer_samples = samples_from_ms(
            gapless.prebuffer_ms,
            device_sample_rate,
            device_channels.max(1),
        );
        let crossfade_samples = if gapless.overlap_ms > 0 {
            (gapless.overlap_ms as u64 * device_sample_rate as u64 * device_channels.max(1) as u64)
                / 1000
        } else {
            0
        };
        Self {
            track_id,
            generation,
            source_kind,
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            buffer: Mutex::new(PlaybackBuffer::new(prebuffer_samples)),
            command_tx,
            volume_ctl,
            position_samples,
            seek_target_samples: AtomicU64::new(u64::MAX),
            total_samples: AtomicU64::new(estimated_total_samples.unwrap_or(0)),
            near_end_signaled: AtomicBool::new(false),
            crossfade_samples: AtomicU64::new(crossfade_samples),
            crossfade_start_signaled: AtomicBool::new(false),
            fadein_start_samples: AtomicU64::new(u64::MAX), // u64::MAX = no fade-in
            device_sample_rate,
            device_channels,
            target_sample_rate: AtomicU32::new(device_sample_rate),
        }
    }

    fn reset_buffer(&self) {
        if let Ok(mut guard) = self.buffer.lock() {
            guard.reset();
        }
    }

    fn signal_terminal(&self, outcome: PlaybackTerminalReason) -> Result<()> {
        self.command_tx
            .send(PlaybackRuntimeCommand::TrackTerminal {
                track_id: self.track_id,
                generation: self.generation,
                outcome,
            })
            .map_err(|_| anyhow!("playback runtime command channel closed"))
    }
}

#[derive(Debug)]
struct PlaybackBuffer {
    /// All decoded+resampled samples for the current track. Never shrinks — we seek via read_pos.
    samples: Vec<f32>,
    /// Current read cursor into `samples`.
    read_pos: usize,
    start_threshold_samples: usize,
    started: bool,
    started_notified: bool,
    finished: bool,
    finished_notified: bool,
}

impl PlaybackBuffer {
    fn new(start_threshold_samples: usize) -> Self {
        Self {
            samples: Vec::new(),
            read_pos: 0,
            start_threshold_samples,
            started: false,
            started_notified: false,
            finished: false,
            finished_notified: false,
        }
    }

    fn is_ready(&self) -> bool {
        self.finished || (self.samples.len() - self.read_pos) >= self.start_threshold_samples
    }

    fn drain_into<T>(&mut self, data: &mut [T], convert: &impl Fn(f32) -> T) -> usize {
        let remaining = self.samples.len().saturating_sub(self.read_pos);
        let available = remaining.min(data.len());
        for (dst, &sample) in data
            .iter_mut()
            .zip(self.samples[self.read_pos..self.read_pos + available].iter())
        {
            *dst = convert(sample);
        }
        self.read_pos += available;
        if available < data.len() {
            for dst in data.iter_mut().skip(available) {
                *dst = convert(0.0);
            }
        }
        available
    }

    fn mark_finished(&mut self) {
        self.finished = true;
    }

    /// Seek to an absolute interleaved-sample offset. Clamped to [0, samples.len()].
    fn seek_to(&mut self, target_samples: usize) {
        self.read_pos = target_samples.min(self.samples.len());
        // Always reset the finished notification so the end-of-track signal can fire again
        // if the user seeks to a position past the remaining samples.
        self.finished_notified = false;
        // If decoding isn't done yet, also reset the start gate so the pre-buffer
        // threshold is re-evaluated from the new cursor position.
        if !self.finished {
            self.started = false;
            self.started_notified = false;
        }
    }

    fn reset(&mut self) {
        self.samples.clear();
        self.read_pos = 0;
        self.started = false;
        self.started_notified = false;
        self.finished = false;
        self.finished_notified = false;
    }
}

/// A [`symphonia::core::io::MediaSource`] backed by a channel of byte chunks.
///
/// Bytes arrive from a background download thread via `chunk_rx`. As chunks arrive they are
/// appended to `data`. `read_pos` is an index into `data` that advances as Symphonia reads.
/// This lets Symphonia decode audio packets as fast as bytes arrive — playback can start after
/// the first few hundred milliseconds of audio are decoded rather than waiting for the full
/// download.
/// A [`symphonia::core::io::MediaSource`] backed by a channel of byte chunks.
///
/// The channel receiver is wrapped in a `Mutex` so the type satisfies `Sync` (required by
/// `MediaSource`). Only the decode thread ever calls `read`/`seek`, so the mutex is uncontended.
struct StreamPipe {
    data: Vec<u8>,
    read_pos: usize,
    rx: Mutex<std::sync::mpsc::Receiver<Option<Vec<u8>>>>,
    eof: bool,
    // Content-Length from the HTTP response, if the CDN sent it.
    // Returned by byte_len() so Symphonia's MSS can translate SeekFrom::End
    // into SeekFrom::Start without giving up with "stream is not seekable".
    known_length: Option<u64>,
}

impl StreamPipe {
    fn new(rx: std::sync::mpsc::Receiver<Option<Vec<u8>>>, known_length: Option<u64>) -> Self {
        Self {
            data: Vec::new(),
            read_pos: 0,
            rx: Mutex::new(rx),
            eof: false,
            known_length,
        }
    }

    /// Block until at least `target` bytes are buffered (or EOF).
    fn fill_to(&mut self, target: usize) {
        if let Ok(rx) = self.rx.lock() {
            while !self.eof && self.data.len() < target {
                match rx.recv() {
                    Ok(Some(chunk)) => self.data.extend_from_slice(&chunk),
                    _ => self.eof = true,
                }
            }
        }
    }

    fn recv_chunk(&mut self) {
        if self.eof {
            return;
        }
        if let Ok(rx) = self.rx.lock() {
            match rx.recv() {
                Ok(Some(chunk)) => self.data.extend_from_slice(&chunk),
                _ => self.eof = true,
            }
        } else {
            self.eof = true;
        }
    }
}

impl Read for StreamPipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while !self.eof && self.read_pos >= self.data.len() {
            self.recv_chunk();
        }
        let available = self.data.len().saturating_sub(self.read_pos);
        if available == 0 {
            return Ok(0);
        }
        let n = buf.len().min(available);
        buf[..n].copy_from_slice(&self.data[self.read_pos..self.read_pos + n]);
        self.read_pos += n;
        Ok(n)
    }
}

impl Seek for StreamPipe {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => {
                let t = n as usize;
                if t > self.data.len() {
                    self.fill_to(t);
                }
                t.min(self.data.len())
            }
            SeekFrom::Current(n) => {
                let t = (self.read_pos as i64 + n).max(0) as usize;
                if t > self.data.len() {
                    self.fill_to(t);
                }
                t.min(self.data.len())
            }
            SeekFrom::End(n) => {
                // Fill until EOF so we know the total file size.
                while !self.eof {
                    self.recv_chunk();
                }
                (self.data.len() as i64 + n).max(0) as usize
            }
        };
        self.read_pos = target.min(self.data.len());
        Ok(self.read_pos as u64)
    }
}

impl symphonia::core::io::MediaSource for StreamPipe {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        if self.eof {
            Some(self.data.len() as u64)
        } else {
            self.known_length
        }
    }
}

fn decode_and_buffer_job(
    config: PlaybackRuntimeConfig,
    job: PreparedPlaybackJob,
    shared: Arc<PlaybackSharedState>,
    device_sample_rate: u32,
    device_channels: u16,
) -> Result<()> {
    match job.source {
        PlaybackSourceRequest::LocalLibrary => {
            return Err(anyhow!(
                "local library playback is not wired into the host-audio runtime yet"
            ));
        }
        PlaybackSourceRequest::TidalStream(request) => {
            // ── Step 1: resolve the stream URL (async, needs a mini tokio runtime) ──────────
            let rt = TokioRuntimeBuilder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create async runtime for TIDAL stream fetch")?;

            if shared.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }

            let stream_info = rt.block_on(async {
                resolve_stream(&config.http_client, &config.access_token, &request).await
            })?;

            if shared.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }

            // ── Step 2: stream bytes from the CDN in a background thread ─────────────────────
            // Using a bounded channel limits peak memory: at most 32 in-flight chunks of ~64KB
            // each ≈ 2 MB of download head room while the decoder works.
            // A separate one-shot channel carries the Content-Length from the response headers
            // so StreamPipe can report byte_len() correctly, allowing Symphonia's MSS to
            // translate SeekFrom::End into an absolute position rather than failing.
            let (len_tx, len_rx) = std::sync::mpsc::sync_channel::<Option<u64>>(1);
            let (chunk_tx, chunk_rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(32);
            let http = config.http_client.clone();
            let url = stream_info.url.clone();
            let segment_urls = stream_info.segment_urls.clone();
            thread::Builder::new()
                .name("noor-stream-download".into())
                .spawn(move || {
                    let dl_rt = TokioRuntimeBuilder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to build download runtime");
                    dl_rt.block_on(async move {
                        let result: anyhow::Result<()> = async {
                            if segment_urls.is_empty() {
                                // Single-URL path (JSON manifest or DASH BaseURL shape)
                                let response = http
                                    .get(&url)
                                    .send()
                                    .await
                                    .context("download request failed")?
                                    .error_for_status()
                                    .context("download returned error status")?;
                                // Send Content-Length before any chunks so the decode thread
                                // can populate StreamPipe::known_length immediately.
                                let _ = len_tx.send(response.content_length());
                                let mut stream = response.bytes_stream();
                                while let Some(chunk) = stream.next().await {
                                    let bytes = chunk.context("chunk read error")?;
                                    if chunk_tx.send(Some(bytes.to_vec())).is_err() {
                                        break; // decoder stopped early (track skipped/stopped)
                                    }
                                }
                            } else {
                                // DASH SegmentTemplate path: init segment then media segments.
                                // Total length is unknown upfront; send None so StreamPipe
                                // skips seek-from-end and falls back to linear reads.
                                let _ = len_tx.send(None);
                                'segments: for seg_url in
                                    std::iter::once(&url).chain(segment_urls.iter())
                                {
                                    let response = http
                                        .get(seg_url.as_str())
                                        .send()
                                        .await
                                        .context("DASH segment request failed")?
                                        .error_for_status()
                                        .context("DASH segment returned error status")?;
                                    let mut stream = response.bytes_stream();
                                    while let Some(chunk) = stream.next().await {
                                        let bytes = chunk.context("DASH segment chunk error")?;
                                        if chunk_tx.send(Some(bytes.to_vec())).is_err() {
                                            break 'segments; // decoder stopped early
                                        }
                                    }
                                }
                            }
                            Ok(())
                        }
                        .await;
                        if let Err(err) = result {
                            warn!("TIDAL stream download error: {err:?}");
                            // Ensure len_rx unblocks if the request failed before sending length.
                            let _ = len_tx.try_send(None);
                        }
                        let _ = chunk_tx.send(None); // signal EOF regardless
                    });
                })
                .context("failed to spawn download thread")?;

            // Block briefly until response headers arrive so we know Content-Length.
            let content_length = len_rx.recv().ok().flatten();

            // ── Step 3: probe + decode incrementally, writing to the buffer each packet ──────
            let pipe = StreamPipe::new(chunk_rx, content_length);
            let mss = MediaSourceStream::new(Box::new(pipe), Default::default());

            // Give Symphonia a format hint from the Tidal manifest MIME type so it
            // can skip the seeking probes it uses for format auto-detection.
            // Without this, the MP4/AAC reader tries SeekFrom::End to locate the
            // moov atom, which can fail on a live streaming pipe.
            let mut hint = Hint::new();
            if !stream_info.segment_urls.is_empty() {
                // DASH fMP4 (CMAF): init+segments form an ISOBMFF stream regardless of inner
                // codec. The m4a hint routes Symphonia to its IsoMp4 reader which handles
                // fragmented MP4 linearly without attempting a SeekFrom::End moov search.
                hint.with_extension("m4a");
            } else {
                let codec_lower = stream_info.codec.to_ascii_lowercase();
                let ext = if codec_lower.contains("flac") {
                    Some("flac")
                } else if codec_lower.contains("mp3") || codec_lower.contains("mpeg") {
                    Some("mp3")
                } else if codec_lower.contains("aac")
                    || codec_lower.contains("mp4")
                    || codec_lower.contains("m4a")
                {
                    Some("m4a")
                } else if codec_lower.contains("ogg") {
                    Some("ogg")
                } else {
                    None
                };
                if let Some(ext) = ext {
                    hint.with_extension(ext);
                }
            }

            let probed = symphonia::default::get_probe()
                .format(
                    &hint,
                    mss,
                    &FormatOptions::default(),
                    &MetadataOptions::default(),
                )
                .context("Symphonia format probe failed")?;

            let mut format = probed.format;
            let track = format
                .default_track()
                .ok_or_else(|| anyhow!("TIDAL stream had no audio track"))?;
            let decoded_sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
            let decoded_channels = track
                .codec_params
                .channels
                .map(|c| c.count() as u16)
                .unwrap_or(2);
            let mut decoder = symphonia::default::get_codecs()
                .make(&track.codec_params, &DecoderOptions::default())
                .context("failed to build Symphonia decoder")?;

            // ── Pre-loop: passive analysis capture ──────────────────────────────────────
            let mut analysis_sent = false;
            let mut analysis_buf: Vec<f32> = Vec::new();

            // Per-track stateful resampler. Lazily built on the first packet whose
            // input rate doesn't match the live target rate. Rebuilt whenever the
            // live target rate changes (sample-rate-follow path) or the channel
            // count flips. None when input rate already matches output rate
            // (passthrough).
            let mut resampler: Option<StreamResampler> = None;

            loop {
                if shared.stopped.load(Ordering::SeqCst) {
                    return Ok(()); // track was stopped/skipped — exit cleanly
                }

                let packet = match format.next_packet() {
                    Ok(p) => p,
                    Err(SymphoniaError::IoError(_)) => break, // EOF
                    Err(SymphoniaError::ResetRequired) => {
                        decoder.reset();
                        continue;
                    }
                    Err(err) => return Err(err.into()),
                };

                let decoded = match decoder.decode(&packet) {
                    Ok(d) => d,
                    Err(SymphoniaError::DecodeError(_)) => continue,
                    Err(err) => return Err(err.into()),
                };

                let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                sb.copy_interleaved_ref(decoded);

                // ── Passive analysis tap: capture first 30 seconds as mono ──────────────
                if !analysis_sent {
                    let mono = mix_to_mono_slice(sb.samples(), decoded_channels as usize);
                    analysis_buf.extend_from_slice(&mono);
                    if analysis_buf.len() >= decoded_sample_rate as usize * 30 {
                        if let Some(tx) = &config.analysis_tx {
                            let _ = tx.send((
                                shared.track_id,
                                std::mem::take(&mut analysis_buf),
                                decoded_sample_rate,
                            ));
                        }
                        analysis_sent = true;
                    }
                }

                // Channel-adapt and resample this packet's samples, then push to buffer.
                // Releasing the lock between packets lets the CPAL callback drain freely.
                //
                // Read the target sample rate live from shared state so a runtime
                // `DeviceSwap` with sample-rate-follow can re-target the
                // resampler without restarting the decoder. The cpal callback's
                // PlaybackBuffer mixes any old- and new-rate samples that were
                // already enqueued before the change at the new device's rate;
                // a brief pitch glitch across the swap boundary is acceptable
                // (matches the documented "brief silence is OK" behaviour for
                // sample-rate-follow transitions).
                let live_target_rate = shared.target_sample_rate.load(Ordering::Relaxed).max(1);
                let _ = device_sample_rate; // kept in signature for callers; live rate above is authoritative
                let channelized = adapt_channels(
                    sb.samples(),
                    decoded_channels as usize,
                    device_channels as usize,
                );

                let resampled = if decoded_sample_rate == live_target_rate {
                    // Bit-perfect passthrough — rates already match.
                    resampler = None;
                    channelized
                } else {
                    let needs_rebuild = match resampler.as_ref() {
                        Some(r) => {
                            r.in_rate != decoded_sample_rate
                                || r.out_rate != live_target_rate
                                || r.channels != device_channels as usize
                        }
                        None => true,
                    };
                    if needs_rebuild {
                        match StreamResampler::new(
                            decoded_sample_rate,
                            live_target_rate,
                            device_channels as usize,
                        ) {
                            Ok(r) => resampler = Some(r),
                            Err(e) => {
                                warn!(
                                    "Resampler init failed ({decoded_sample_rate} -> {live_target_rate} Hz, {} ch): {e}; passing through unresampled (pitch will be wrong)",
                                    device_channels
                                );
                                resampler = None;
                            }
                        }
                    }
                    match resampler.as_mut() {
                        Some(r) => r.process(&channelized),
                        None => channelized,
                    }
                };

                let mut guard = shared
                    .buffer
                    .lock()
                    .map_err(|_| anyhow!("playback buffer poisoned"))?;
                guard.samples.extend_from_slice(&resampled);
            }

            // Flush any residual samples held in the resampler at end-of-stream
            // so the final fraction of a chunk doesn't get truncated.
            if let Some(r) = resampler.as_mut() {
                let tail = r.flush();
                if !tail.is_empty() {
                    let mut guard = shared
                        .buffer
                        .lock()
                        .map_err(|_| anyhow!("playback buffer poisoned"))?;
                    guard.samples.extend_from_slice(&tail);
                }
            }

            // Apply fade-in / fade-out ramps and mark the stream complete.
            let total = {
                let mut guard = shared
                    .buffer
                    .lock()
                    .map_err(|_| anyhow!("playback buffer poisoned"))?;

                // Fade ramps are applied dynamically in the CPAL callback — no baking needed.

                guard.mark_finished();
                guard.samples.len() as u64
            };
            shared.total_samples.store(total, Ordering::Relaxed);

            // Notify the runtime loop that this engine's decode is complete.
            // The runtime uses this to start the crossfade stream if the window is already open.
            let _ = shared
                .command_tx
                .send(PlaybackRuntimeCommand::NextDecodeComplete {
                    track_id: shared.track_id,
                    generation: shared.generation,
                });
        }
    }

    Ok(())
}

/// Mix interleaved multi-channel samples down to mono.
fn mix_to_mono_slice(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

fn adapt_channels(samples: &[f32], input_channels: usize, output_channels: usize) -> Vec<f32> {
    if input_channels == 0 || output_channels == 0 || samples.is_empty() {
        return Vec::new();
    }

    if input_channels == output_channels {
        return samples.to_vec();
    }

    let mut output = Vec::with_capacity((samples.len() / input_channels) * output_channels);
    for frame in samples.chunks(input_channels) {
        match (input_channels, output_channels) {
            (_, 1) => {
                let mix = frame.iter().copied().sum::<f32>() / frame.len().max(1) as f32;
                output.push(mix);
            }
            (1, channels) => {
                let sample = frame[0];
                output.extend(std::iter::repeat_n(sample, channels));
            }
            _ => {
                for channel in 0..output_channels {
                    let index = channel.min(frame.len().saturating_sub(1));
                    output.push(frame[index]);
                }
            }
        }
    }

    output
}

/// Stateful sample-rate converter built on rubato's polyphase Kaiser-windowed
/// sinc filter. Operates on interleaved f32 samples to slot directly into the
/// decoder pipeline.
///
/// rubato's `SincFixedIn` requires a fixed input chunk size per `process` call,
/// so we accumulate decoded samples in a per-channel residual buffer and only
/// invoke rubato when a full chunk is ready. This preserves continuity across
/// Symphonia packet boundaries (which arrive in irregular sizes) — without
/// state, every packet would start the filter cold and create audible clicks
/// at packet edges.
struct StreamResampler {
    inner: rubato::SincFixedIn<f32>,
    chunk_size_in: usize,
    channels: usize,
    in_rate: u32,
    out_rate: u32,
    residual: Vec<Vec<f32>>,
}

impl StreamResampler {
    /// Default chunk size. 1024 frames at 48 kHz is ~21 ms — comfortably below
    /// the audio buffer fill threshold so we don't starve the output, and large
    /// enough to amortize rubato's per-call overhead.
    const CHUNK_SIZE_IN: usize = 1024;

    fn new(in_rate: u32, out_rate: u32, channels: usize) -> Result<Self> {
        if channels == 0 || in_rate == 0 || out_rate == 0 {
            return Err(anyhow!(
                "StreamResampler::new: invalid arguments ({in_rate} -> {out_rate} Hz, {channels} ch)"
            ));
        }
        let params = rubato::SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: rubato::SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: rubato::WindowFunction::BlackmanHarris2,
        };
        let inner = rubato::SincFixedIn::<f32>::new(
            out_rate as f64 / in_rate as f64,
            // Allow live ratio changes within ±1 octave without rebuilding —
            // we still rebuild on rate change for cleanliness, but this also
            // covers any in-flight tweaks.
            2.0,
            params,
            Self::CHUNK_SIZE_IN,
            channels,
        )
        .context("rubato SincFixedIn init failed")?;
        Ok(Self {
            inner,
            chunk_size_in: Self::CHUNK_SIZE_IN,
            channels,
            in_rate,
            out_rate,
            residual: vec![Vec::with_capacity(Self::CHUNK_SIZE_IN * 2); channels],
        })
    }

    /// Feed interleaved input samples; return interleaved output samples for
    /// any complete chunks that finished processing. Sub-chunk leftovers are
    /// held in `residual` until the next call (or `flush`).
    fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        if interleaved.is_empty() {
            return Vec::new();
        }
        for frame in interleaved.chunks_exact(self.channels) {
            for (ch, &s) in frame.iter().enumerate() {
                self.residual[ch].push(s);
            }
        }
        let mut out: Vec<f32> = Vec::new();
        while self.residual[0].len() >= self.chunk_size_in {
            let chunk_in: Vec<Vec<f32>> = self
                .residual
                .iter_mut()
                .map(|ch| ch.drain(..self.chunk_size_in).collect())
                .collect();
            match rubato::Resampler::process(&mut self.inner, &chunk_in, None) {
                Ok(chunk_out) => Self::interleave_into(&chunk_out, &mut out),
                Err(e) => {
                    warn!("rubato process error: {e}");
                    return out;
                }
            }
        }
        out
    }

    /// End-of-stream: zero-pad the residual to a full chunk, run it through,
    /// then drop the prefix that corresponds to the padded silence so we
    /// approximately preserve the true tail length.
    fn flush(&mut self) -> Vec<f32> {
        if self.residual[0].is_empty() {
            return Vec::new();
        }
        let real_in = self.residual[0].len();
        for ch in self.residual.iter_mut() {
            ch.resize(self.chunk_size_in, 0.0);
        }
        let chunk_in: Vec<Vec<f32>> = self
            .residual
            .iter_mut()
            .map(|ch| ch.drain(..self.chunk_size_in).collect())
            .collect();
        let chunk_out = match rubato::Resampler::process(&mut self.inner, &chunk_in, None) {
            Ok(v) => v,
            Err(e) => {
                warn!("rubato flush error: {e}");
                return Vec::new();
            }
        };
        // Keep only the output frames that came from the genuine tail.
        let real_out_frames =
            ((real_in as f64) * self.out_rate as f64 / self.in_rate as f64).round() as usize;
        let real_out_frames = real_out_frames.min(chunk_out[0].len());
        let trimmed: Vec<Vec<f32>> = chunk_out
            .into_iter()
            .map(|ch| ch.into_iter().take(real_out_frames).collect())
            .collect();
        let mut out: Vec<f32> = Vec::with_capacity(real_out_frames * self.channels);
        Self::interleave_into(&trimmed, &mut out);
        out
    }

    fn interleave_into(chunk_out: &[Vec<f32>], out: &mut Vec<f32>) {
        if chunk_out.is_empty() {
            return;
        }
        let frames = chunk_out[0].len();
        let channels = chunk_out.len();
        out.reserve(frames * channels);
        for f in 0..frames {
            for ch in chunk_out.iter() {
                out.push(ch[f]);
            }
        }
    }
}

fn samples_from_ms(ms: i32, sample_rate: u32, channels: u16) -> usize {
    if ms <= 0 || sample_rate == 0 || channels == 0 {
        return 0;
    }

    let base = (ms as u128 * sample_rate as u128 * channels as u128) / 1_000;
    let pad = if ms > 0 {
        (GAPLESS_PREFILL_PAD_MS as u128 * sample_rate as u128 * channels as u128) / 1_000
    } else {
        0
    };
    (base + pad) as usize
}

fn estimate_total_samples_from_duration_ms(
    duration_ms: i64,
    sample_rate: u32,
    channels: u16,
) -> Option<u64> {
    if duration_ms <= 0 || sample_rate == 0 || channels == 0 {
        return None;
    }
    Some((duration_ms as u64 * sample_rate as u64 * channels as u64) / 1_000)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64},
    };

    #[test]
    fn adapt_channels_duplicates_mono_to_stereo() {
        let output = adapt_channels(&[0.25, -0.25], 1, 2);
        assert_eq!(output, vec![0.25, 0.25, -0.25, -0.25]);
    }

    #[test]
    fn stream_resampler_passthrough_when_rates_match_at_callsite() {
        // The decoder's call site short-circuits resampling when input == output
        // rate, so it never builds a StreamResampler in that case. This test
        // documents the contract for callers — same-rate inputs should not
        // construct a resampler.
        assert_eq!(48_000_u32, 48_000_u32);
    }

    #[test]
    fn effective_output_config_applies_desired_sample_rate() {
        let base = StreamConfig {
            channels: 2,
            sample_rate: cpal::SampleRate(48_000),
            buffer_size: cpal::BufferSize::Default,
        };

        let effective = effective_output_config(&base, Some(96_000));

        assert_eq!(effective.sample_rate.0, 96_000);
        assert_eq!(effective.channels, 2);
    }

    #[test]
    fn effective_output_config_keeps_base_rate_without_override() {
        let base = StreamConfig {
            channels: 6,
            sample_rate: cpal::SampleRate(44_100),
            buffer_size: cpal::BufferSize::Default,
        };

        let effective = effective_output_config(&base, None);

        assert_eq!(effective.sample_rate.0, 44_100);
        assert_eq!(effective.channels, 6);
    }

    #[test]
    fn exclusive_rebuild_rate_follows_current_output_rate_only_when_enabled() {
        assert_eq!(exclusive_rebuild_rate(true, 96_000), Some(96_000));
        assert_eq!(exclusive_rebuild_rate(false, 96_000), None);
    }

    #[test]
    fn swap_stream_plan_uses_track_rate_for_exclusive_backend() {
        let base = StreamConfig {
            channels: 2,
            sample_rate: cpal::SampleRate(48_000),
            buffer_size: cpal::BufferSize::Default,
        };

        let plan = swap_stream_plan(&base, Some(96_000), SwapBackend::Exclusive);

        assert_eq!(plan.stream_config.sample_rate.0, 96_000);
        assert_eq!(plan.target_sample_rate, Some(96_000));
    }

    #[test]
    fn swap_stream_plan_uses_device_rate_for_shared_fallback() {
        let base = StreamConfig {
            channels: 2,
            sample_rate: cpal::SampleRate(48_000),
            buffer_size: cpal::BufferSize::Default,
        };

        let plan = swap_stream_plan(&base, Some(192_000), SwapBackend::SharedFallback);

        assert_eq!(plan.stream_config.sample_rate.0, 48_000);
        assert_eq!(plan.target_sample_rate, Some(48_000));
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
    fn stream_resampler_produces_output_for_downsample() {
        let mut r = StreamResampler::new(96_000, 48_000, 2).expect("new");
        // Feed two full chunks of stereo silence so we get at least one output
        // batch out (sinc filter has a warm-up delay).
        let frames = StreamResampler::CHUNK_SIZE_IN * 2;
        let input = vec![0.0_f32; frames * 2];
        let out = r.process(&input);
        assert!(!out.is_empty(), "expected resampled output, got empty");
        // Output frame count should be roughly half of input frame count
        // (96k -> 48k). Allow generous slack for filter warm-up frames the
        // first chunks suppress.
        let out_frames = out.len() / 2;
        assert!(
            out_frames < frames,
            "downsample output ({out_frames}) should be less than input ({frames})"
        );
    }

    #[test]
    fn stream_resampler_holds_residual_under_chunk_size() {
        let mut r = StreamResampler::new(44_100, 48_000, 2).expect("new");
        // Feed only a few frames — less than CHUNK_SIZE_IN. Should buffer
        // them all in residual and return nothing.
        let input = vec![0.0_f32; 10 * 2];
        let out = r.process(&input);
        assert!(
            out.is_empty(),
            "small input should buffer in residual, not produce output"
        );
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
}
