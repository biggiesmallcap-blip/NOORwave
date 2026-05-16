use super::PlaybackRuntimeConfig;
use super::commands::{PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackTerminalReason};
use super::shared::{PlaybackSharedState, estimate_total_samples_from_duration_ms};
use crate::playback::decode::decode_and_buffer_job;
use crate::playback::output::cpal_shared::{
    SwapBackend, build_started_output_stream_with_rate_fallback, output_rate_fallback_config,
    swap_stream_plan,
};
use crate::playback::player::{PlaybackSourceRequest, PreparedPlaybackJob};
use anyhow::{Context, Result, anyhow};
use cpal::traits::DeviceTrait;
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use tracing::{error, warn};

enum OutputStream {
    Cpal { _stream: Stream },
}

pub(super) struct SwapPauseGuard {
    shared: Arc<PlaybackSharedState>,
    was_paused: bool,
    active: bool,
}

impl SwapPauseGuard {
    pub(super) fn new(shared: Arc<PlaybackSharedState>) -> Self {
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

pub(super) struct PlaybackEngine {
    pub(super) track_id: i64,
    pub(super) generation: u64,
    stream: Option<OutputStream>,
    pub(super) decoder_thread: Option<JoinHandle<()>>,
    pub(super) shared: Arc<PlaybackSharedState>,
    /// Snapshot of the `PreparedPlaybackJob` that started this engine. Stored
    /// so the runtime's `SeekTo` handler can compute a segment-restart job
    /// (incrementing `start_from_segment_index` / `start_from_offset_ms`)
    /// without having to plumb the original job through the engine API.
    pub(super) job: PreparedPlaybackJob,
}

fn join_decoder_thread_in_background(track_id: i64, handle: JoinHandle<()>) {
    let _ = thread::Builder::new()
        .name(format!("noor-playback-decoder-join-{track_id}"))
        .spawn(move || {
            let _ = handle.join();
        });
}

impl PlaybackEngine {
    #[cfg(test)]
    pub(super) fn test_with_shared(
        track_id: i64,
        generation: u64,
        shared: Arc<PlaybackSharedState>,
    ) -> Self {
        Self {
            track_id,
            generation,
            stream: None,
            decoder_thread: None,
            shared,
            job: PreparedPlaybackJob::test_fixture(track_id, generation),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start(
        config: &PlaybackRuntimeConfig,
        command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
        device: &cpal::Device,
        output_config: &StreamConfig,
        output_sample_format: SampleFormat,
        job: PreparedPlaybackJob,
        event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
        _device_sample_rate: u32,
        device_channels: u16,
        volume_ctl: Arc<AtomicU32>,
        position_samples: Arc<AtomicU64>,
    ) -> Result<Self> {
        let fallback_config = device
            .default_output_config()
            .map(|config| config.config())
            .unwrap_or_else(|_| output_config.clone());
        match Self::start_with_output_config(
            config,
            command_tx,
            device,
            output_config,
            output_sample_format,
            job.clone(),
            event_tx.clone(),
            device_channels,
            Arc::clone(&volume_ctl),
            Arc::clone(&position_samples),
        ) {
            Ok(engine) => Ok(engine),
            Err(primary_error) => {
                let Some(fallback_config) =
                    output_rate_fallback_config(output_config, &fallback_config)
                else {
                    return Err(primary_error);
                };
                warn!(
                    "Playback output rejected {} Hz; cold-starting at {} Hz instead: {primary_error}",
                    output_config.sample_rate, fallback_config.sample_rate
                );
                Self::start_with_output_config(
                    config,
                    command_tx,
                    device,
                    &fallback_config,
                    output_sample_format,
                    job,
                    event_tx,
                    device_channels,
                    volume_ctl,
                    position_samples,
                )
                .with_context(|| {
                    format!(
                        "fallback playback output at {} Hz failed after {} Hz was rejected",
                        fallback_config.sample_rate, output_config.sample_rate
                    )
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_with_output_config(
        config: &PlaybackRuntimeConfig,
        command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
        device: &cpal::Device,
        output_config: &StreamConfig,
        output_sample_format: SampleFormat,
        job: PreparedPlaybackJob,
        event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
        device_channels: u16,
        volume_ctl: Arc<AtomicU32>,
        position_samples: Arc<AtomicU64>,
    ) -> Result<Self> {
        let mut engine = Self::start_decoder_only(
            config,
            command_tx,
            job,
            output_config.sample_rate,
            device_channels,
            volume_ctl,
            position_samples,
        )?;

        let cpal_stream = match build_started_output_stream_with_rate_fallback(
            device,
            output_config,
            output_config,
            output_sample_format,
            Arc::clone(&engine.shared),
            engine.shared.command_tx.clone(),
            event_tx,
        ) {
            Ok((stream, _actual_rate)) => stream,
            Err(error) => {
                engine.stop();
                return Err(error);
            }
        };
        engine.stream = Some(OutputStream::Cpal {
            _stream: cpal_stream,
        });

        Ok(engine)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_decoder_only(
        config: &PlaybackRuntimeConfig,
        command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
        job: PreparedPlaybackJob,
        output_sample_rate: u32,
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
                output_sample_rate,
                device_channels,
            )
        });
        let offset_samples = (job
            .start_from_offset_ms
            .saturating_mul(output_sample_rate as u64)
            .saturating_mul(device_channels.max(1) as u64))
            / 1000;
        position_samples.store(offset_samples, Ordering::Relaxed);
        let position_offset_samples = Arc::new(AtomicU64::new(offset_samples));
        let shared = Arc::new(PlaybackSharedState::new(
            track_id,
            generation,
            source_kind,
            job.gapless.clone(),
            output_sample_rate,
            device_channels,
            estimated_total_samples,
            command_tx.clone(),
            volume_ctl,
            position_samples,
            position_offset_samples,
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
                    output_sample_rate,
                    device_channels,
                ) {
                    let _ = decoder_shared
                        .signal_terminal(PlaybackTerminalReason::Error(err.to_string()));
                    error!("Playback decode failed for track {track_id}: {err:?}");
                }
            })
            .context("failed to spawn playback decoder thread")?;

        Ok(Self {
            track_id,
            generation,
            stream: None,
            decoder_thread: Some(decoder_thread),
            shared,
            job,
        })
    }

    pub(super) fn pause(&self) -> Result<()> {
        self.shared.paused.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub(super) fn resume(&self) -> Result<()> {
        self.shared.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub(super) fn stop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        self.shared.paused.store(true, Ordering::SeqCst);
        self.shared.reset_buffer();
        if let Some(handle) = self.decoder_thread.take() {
            join_decoder_thread_in_background(self.track_id, handle);
        }
    }

    pub(super) fn drop_stream(&mut self) {
        drop(self.stream.take());
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn swap_stream(
        &mut self,
        device: &cpal::Device,
        output_config: &StreamConfig,
        output_sample_format: SampleFormat,
        command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
        event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
        exclusive: bool,
        desired_sample_rate: Option<u32>,
        _exclusive_release_grace_secs: u32,
    ) -> Result<u32> {
        let pause_guard = SwapPauseGuard::new(Arc::clone(&self.shared));
        drop(self.stream.take());

        if exclusive {
            return Err(anyhow!(
                "exclusive output is owned by the runtime WASAPI sink"
            ));
        }

        let shared_plan = swap_stream_plan(output_config, desired_sample_rate, SwapBackend::Shared);
        let (stream, actual_rate) = build_started_output_stream_with_rate_fallback(
            device,
            &shared_plan.stream_config,
            output_config,
            output_sample_format,
            Arc::clone(&self.shared),
            command_tx,
            event_tx,
        )?;
        let mut active_plan = shared_plan;
        active_plan.stream_config.sample_rate = actual_rate;
        active_plan.target_sample_rate = Some(actual_rate);

        if let Some(target_sample_rate) = active_plan.target_sample_rate {
            self.shared
                .target_sample_rate
                .store(target_sample_rate, Ordering::Relaxed);
        }
        self.stream = Some(OutputStream::Cpal { _stream: stream });
        pause_guard.restore();
        Ok(active_plan.stream_config.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::gapless::GaplessPlan;
    use crate::playback::player::PlaybackSourceKind;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    fn test_shared_state() -> Arc<PlaybackSharedState> {
        let (command_tx, _) = mpsc::channel();
        Arc::new(PlaybackSharedState::new(
            1,
            1,
            PlaybackSourceKind::TidalStream,
            GaplessPlan::disabled(),
            48_000,
            2,
            None,
            command_tx,
            Arc::new(AtomicU32::new(1.0f32.to_bits())),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        ))
    }

    #[test]
    fn stop_marks_engine_stopped_without_waiting_for_decoder_thread() {
        let shared = test_shared_state();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (decoder_started_tx, decoder_started_rx) = mpsc::channel::<()>();
        let decoder_thread = thread::spawn(move || {
            let _ = decoder_started_tx.send(());
            let _ = release_rx.recv();
        });
        decoder_started_rx
            .recv()
            .expect("decoder thread did not start");

        let mut engine = PlaybackEngine {
            track_id: 1,
            generation: 1,
            stream: None,
            decoder_thread: Some(decoder_thread),
            shared: Arc::clone(&shared),
            job: PreparedPlaybackJob::test_fixture(1, 1),
        };

        // Watchdog: if stop() blocks waiting on the decoder thread, release it
        // after 2s so the test fails loudly with a clear timing message instead
        // of hanging cargo test forever (no per-test timeout exists by default).
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_watchdog = Arc::clone(&cancel);
        let release_for_watchdog = release_tx.clone();
        let watchdog = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if cancel_for_watchdog.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = release_for_watchdog.send(());
        });

        let started = Instant::now();
        engine.stop();
        let elapsed = started.elapsed();

        cancel.store(true, Ordering::SeqCst);
        let _ = release_tx.send(());
        let _ = watchdog.join();

        assert!(shared.stopped.load(Ordering::SeqCst));
        assert!(shared.paused.load(Ordering::SeqCst));
        assert!(
            elapsed < Duration::from_millis(250),
            "stop blocked for {elapsed:?}"
        );
    }
}
