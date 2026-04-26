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
    },
    /// Sent by the CPAL callback when the current track is within `crossfade_ms` of its end.
    /// If a pre-decoded next engine is ready, its stream is unpaused so both mix via the OS.
    CrossfadeStart {
        track_id: i64,
    },
    TrackTerminal {
        track_id: i64,
        outcome: PlaybackTerminalReason,
    },
    /// Swap the CPAL output device for any active engines. Shared mode only at
    /// this stage; `exclusive` and `sample_rate_follow` are wired in later tasks
    /// (5 + 6) and ignored here.
    DeviceSwap {
        device: OutputDeviceSelection,
        exclusive: bool,
        sample_rate_follow: bool,
    },
    Shutdown,
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
    },
    /// Fired when the current track is within `NEAR_END_THRESHOLD_MS` of its end.
    /// The listener should peek the next track and send `PrepareNext` to pre-buffer it.
    NearEnd {
        track_id: i64,
    },
    Error {
        message: String,
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
        let samples = self
            .position_source
            .lock()
            .unwrap()
            .load(Ordering::Relaxed);
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
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok());

    host.output_devices()
        .map(|iter| {
            iter.filter_map(|dev| {
                let name = dev.name().ok()?;
                let configs: Vec<_> = dev
                    .supported_output_configs()
                    .ok()?
                    .collect();
                let max_channels = configs
                    .iter()
                    .map(|c| c.channels())
                    .max()
                    .unwrap_or(0);
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
                    &output_config,
                    output_sample_format,
                    &event_tx,
                    &mut state,
                    job,
                    &volume_ctl,
                    &position_samples,
                    &position_source,
                )?;
            }
            PlaybackRuntimeCommand::Switch(job) => {
                transition_to_job(
                    &config,
                    &command_tx,
                    &device,
                    &output_config,
                    output_sample_format,
                    &event_tx,
                    &mut state,
                    job,
                    &volume_ctl,
                    &position_samples,
                    &position_source,
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
                    .map(|e| e.track_id == job.track.id)
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
            PlaybackRuntimeCommand::CrossfadeStart { track_id } => {
                // The OUTGOING engine just entered its fade-out window and is asking
                // us to start the pre-decoded next engine, if one is ready.
                if state.engine.as_ref().map(|e| e.track_id) == Some(track_id) {
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
            PlaybackRuntimeCommand::NextDecodeComplete { track_id } => {
                // Decode for the pre-decoded next engine completed. If the outgoing
                // engine has already entered the crossfade window, promote now —
                // the user will hear a clipped fade-in, but it's better than silence.
                let pending_match = state
                    .next_engine
                    .as_ref()
                    .map(|e| e.track_id == track_id)
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
            PlaybackRuntimeCommand::TrackTerminal { track_id, outcome } => {
                // The fading-out engine reaching its terminal state is the
                // expected end of a crossfade — drop it silently. The queue
                // advance already happened at promotion time via Finished.
                let fading = state
                    .fading_out_engine
                    .as_ref()
                    .map(|e| e.track_id);
                if fading == Some(track_id) {
                    if let Some(mut engine) = state.fading_out_engine.take() {
                        engine.stop();
                    }
                } else {
                    let active = state.engine.as_ref().map(|engine| engine.track_id);
                    if handle_terminal_event(active, track_id) {
                        stop_current_engine(&mut state);
                        match outcome {
                            PlaybackTerminalReason::Finished => {
                                let _ = event_tx
                                    .send(PlaybackRuntimeEvent::Finished { track_id });
                            }
                            PlaybackTerminalReason::Error(message) => {
                                let _ = event_tx
                                    .send(PlaybackRuntimeEvent::Error { message });
                            }
                        }
                    }
                }
            }
            PlaybackRuntimeCommand::DeviceSwap {
                device: selection,
                exclusive: _exclusive,
                sample_rate_follow: _sample_rate_follow,
            } => {
                // _exclusive and _sample_rate_follow are wired in Tasks 5 + 6.
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

                // Rebuild the stream on every live engine so they all play on
                // the new device.
                let mut swap_failed = false;
                for engine_slot in [
                    state.engine.as_mut(),
                    state.next_engine.as_mut(),
                    state.fading_out_engine.as_mut(),
                ]
                .into_iter()
                .flatten()
                {
                    if let Err(err) = engine_slot.swap_stream(
                        &new_device,
                        &new_config,
                        new_format,
                        command_tx.clone(),
                        event_tx.clone(),
                    ) {
                        warn!(
                            "DeviceSwap: failed to rebuild stream for track {}: {err:?}",
                            engine_slot.track_id
                        );
                        swap_failed = true;
                    }
                }

                if swap_failed {
                    warn!("DeviceSwap: one or more engines failed to swap; output may be partial");
                }

                // Update the runtime's "current device" bindings so subsequent
                // Play / PrepareNext calls use the new device too. We keep the
                // existing state.device_sample_rate / device_channels so the
                // decoder threads stay consistent until SR-follow (Task 6).
                device = new_device;
                output_config = new_config;
                output_sample_format = new_format;
                state.device_name = new_name.clone();

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
    output_config: &StreamConfig,
    output_sample_format: SampleFormat,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    state: &mut PlaybackRuntimeLoopState,
    job: PreparedPlaybackJob,
    volume_ctl: &Arc<AtomicU32>,
    position_samples: &Arc<AtomicU64>,
    position_source: &Arc<Mutex<Arc<AtomicU64>>>,
) -> Result<()> {
    // No-op when state.engine is already playing the requested track. This
    // happens after a crossfade swap: promote_next_to_active emitted Finished
    // for the OUTGOING track, which caused routes to call switch_to(NEW track)
    // — but we already promoted that engine. Re-doing the swap would tear down
    // a perfectly good audio stream and cold-start a duplicate.
    // position_source was already redirected to the promoted engine at promotion
    // time, so the handle reads the correct counter without any extra work here.
    if state.engine.as_ref().map(|e| e.track_id) == Some(job.track.id) {
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
        .map(|e| e.track_id == job.track.id)
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
        // Redirect the handle's reader to this engine's counter (which IS
        // position_samples for cold starts, but re-pointing is always correct).
        *position_source.lock().unwrap() = Arc::clone(position_samples);
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
        state.fading_out_engine = Some(outgoing);
        // Tell the routes layer that the audible "current" track has flipped.
        // Reusing Finished keeps the existing queue-advance path.
        let _ = event_tx.send(PlaybackRuntimeEvent::Finished {
            track_id: outgoing_id,
        });
    }
}

struct PlaybackEngine {
    track_id: i64,
    source_kind: PlaybackSourceKind,
    /// Active CPAL stream; `None` only briefly during a `DeviceSwap` while we
    /// rebuild the stream on the new device.
    stream: Option<Stream>,
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
        let source_kind = job.source_kind();
        let shared = Arc::new(PlaybackSharedState::new(
            track_id,
            source_kind,
            job.gapless,
            output_config.sample_rate.0,
            device_channels,
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

        let stream = build_output_stream(
            device,
            output_config,
            output_sample_format,
            Arc::clone(&shared),
            shared.command_tx.clone(),
            event_tx.clone(),
        )?;
        stream.play().context("failed to start output stream")?;

        Ok(Self {
            track_id,
            source_kind,
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
    /// drains it on the new device. Shared mode only — sample-rate-follow is
    /// handled in Task 6, so the existing `output_config` (built from the
    /// runtime's startup device) is reused. If the new device's default sample
    /// rate differs, audio will play at the wrong pitch until SR-follow lands.
    fn swap_stream(
        &mut self,
        device: &cpal::Device,
        output_config: &StreamConfig,
        output_sample_format: SampleFormat,
        command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
        event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    ) -> Result<()> {
        // Pause first so the decoder side doesn't keep filling while the
        // callback is gone, then drop the old stream before building the new
        // one (some host APIs reject a second exclusive grab while the first
        // stream is alive).
        let was_paused = self.shared.paused.load(Ordering::SeqCst);
        self.shared.paused.store(true, Ordering::SeqCst);
        drop(self.stream.take());

        let new_stream = build_output_stream(
            device,
            output_config,
            output_sample_format,
            Arc::clone(&self.shared),
            command_tx,
            event_tx,
        )?;
        new_stream
            .play()
            .context("failed to start swapped output stream")?;
        self.stream = Some(new_stream);
        self.shared.paused.store(was_paused, Ordering::SeqCst);
        Ok(())
    }
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
    // Both ramps are linear. Together they prevent the volume-doubling that occurs
    // when two full-volume streams are mixed simultaneously.
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
                (elapsed as f32 / xfade as f32).min(1.0)
            } else {
                1.0f32
            }
        } else {
            // Outgoing engine — fade-out once we enter the overlap window.
            let total = shared.total_samples.load(Ordering::Relaxed);
            if total > 0 {
                let remaining = total.saturating_sub(pos);
                if remaining < xfade {
                    (remaining as f32 / xfade as f32).max(0.0)
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
            source: shared.source_kind,
        });
    }

    if guard.started && written == 0 && guard.finished && !guard.finished_notified {
        guard.finished_notified = true;
        let _ = command_tx.send(PlaybackRuntimeCommand::TrackTerminal {
            track_id: shared.track_id,
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
                    });
                }
            }
        }
    }
}

#[derive(Debug)]
struct PlaybackSharedState {
    track_id: i64,
    source_kind: PlaybackSourceKind,
    paused: AtomicBool,
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
}

impl PlaybackSharedState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        track_id: i64,
        source_kind: PlaybackSourceKind,
        gapless: GaplessPlan,
        device_sample_rate: u32,
        device_channels: u16,
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
            source_kind,
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            buffer: Mutex::new(PlaybackBuffer::new(prebuffer_samples)),
            command_tx,
            volume_ctl,
            position_samples,
            seek_target_samples: AtomicU64::new(u64::MAX),
            total_samples: AtomicU64::new(0),
            near_end_signaled: AtomicBool::new(false),
            crossfade_samples: AtomicU64::new(crossfade_samples),
            crossfade_start_signaled: AtomicBool::new(false),
            fadein_start_samples: AtomicU64::new(u64::MAX), // u64::MAX = no fade-in
            device_sample_rate,
            device_channels,
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

    fn extend(&mut self, samples: &[f32]) {
        self.samples.extend_from_slice(samples);
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
            thread::Builder::new()
                .name("noor-stream-download".into())
                .spawn(move || {
                    let dl_rt = TokioRuntimeBuilder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to build download runtime");
                    dl_rt.block_on(async move {
                        let result: anyhow::Result<()> = async {
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
                            let _ = tx.send((shared.track_id, std::mem::take(&mut analysis_buf), decoded_sample_rate));
                        }
                        analysis_sent = true;
                    }
                }

                // Channel-adapt and resample this packet's samples, then push to buffer.
                // Releasing the lock between packets lets the CPAL callback drain freely.
                let channelized = adapt_channels(
                    sb.samples(),
                    decoded_channels as usize,
                    device_channels as usize,
                );
                let resampled = resample_interleaved(
                    &channelized,
                    device_channels as usize,
                    decoded_sample_rate,
                    device_sample_rate,
                );

                let mut guard = shared
                    .buffer
                    .lock()
                    .map_err(|_| anyhow!("playback buffer poisoned"))?;
                guard.samples.extend_from_slice(&resampled);
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
                output.extend(std::iter::repeat(sample).take(channels));
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

fn resample_interleaved(
    samples: &[f32],
    channels: usize,
    input_rate: u32,
    output_rate: u32,
) -> Vec<f32> {
    if samples.is_empty()
        || channels == 0
        || input_rate == 0
        || output_rate == 0
        || input_rate == output_rate
    {
        return samples.to_vec();
    }

    let input_frames = samples.len() / channels;
    if input_frames == 0 {
        return Vec::new();
    }

    let output_frames =
        ((input_frames as f64) * output_rate as f64 / input_rate as f64).round() as usize;
    let frame_step = input_rate as f64 / output_rate as f64;
    let mut output = Vec::with_capacity(output_frames * channels);

    for frame_index in 0..output_frames {
        let source_pos = frame_index as f64 * frame_step;
        let left_index = source_pos.floor() as usize;
        let right_index = (left_index + 1).min(input_frames - 1);
        let fraction = (source_pos - left_index as f64) as f32;

        for channel in 0..channels {
            let left_sample = samples[left_index * channels + channel];
            let right_sample = samples[right_index * channels + channel];
            output.push(left_sample + (right_sample - left_sample) * fraction);
        }
    }

    output
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

fn handle_terminal_event(active_track_id: Option<i64>, track_id: i64) -> bool {
    active_track_id == Some(track_id)
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
    fn resample_interleaved_changes_frame_count() {
        let samples = vec![0.0, 1.0, 0.0, 1.0];
        let output = resample_interleaved(&samples, 2, 48_000, 24_000);
        assert_eq!(output.len(), 2);
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
    fn terminal_events_only_apply_to_active_track() {
        assert!(!handle_terminal_event(None, 7));
        assert!(handle_terminal_event(Some(7), 7));
        assert!(!handle_terminal_event(Some(7), 8));
    }

    #[test]
    fn shared_state_emits_finished_terminal_command() {
        let (command_tx, command_rx) = mpsc::channel();
        let shared = PlaybackSharedState::new(
            42,
            PlaybackSourceKind::TidalStream,
            GaplessPlan::disabled(),
            48_000,
            2,
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
            PlaybackRuntimeCommand::TrackTerminal { track_id, outcome } => {
                assert_eq!(track_id, 42);
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
            PlaybackSourceKind::TidalStream,
            GaplessPlan::disabled(),
            48_000,
            2,
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
            PlaybackRuntimeCommand::TrackTerminal { track_id, outcome } => {
                assert_eq!(track_id, 7);
                match outcome {
                    PlaybackTerminalReason::Error(message) => assert_eq!(message, "boom"),
                    PlaybackTerminalReason::Finished => panic!("expected error reason"),
                }
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
