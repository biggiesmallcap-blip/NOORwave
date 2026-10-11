//! CPAL shared-mode output helpers.

use crate::playback::runtime::commands::{PlaybackRuntimeCommand, PlaybackRuntimeEvent};
use crate::playback::runtime::shared::{
    PlaybackSharedState, write_output_f32, write_output_f64, write_output_i16, write_output_i24,
    write_output_i32, write_output_u16,
};
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwapBackend {
    Exclusive,
    Shared,
}

#[derive(Debug, Clone)]
pub(crate) struct SwapStreamPlan {
    pub(crate) stream_config: StreamConfig,
    pub(crate) target_sample_rate: Option<u32>,
}

pub(crate) fn swap_stream_plan(
    base: &StreamConfig,
    desired_sample_rate: Option<u32>,
    backend: SwapBackend,
) -> SwapStreamPlan {
    let stream_config = match backend {
        SwapBackend::Exclusive | SwapBackend::Shared => {
            effective_output_config(base, desired_sample_rate)
        }
    };
    let target_sample_rate = match (backend, desired_sample_rate) {
        (_, Some(_)) => Some(stream_config.sample_rate),
        (_, None) => None,
    };

    SwapStreamPlan {
        stream_config,
        target_sample_rate,
    }
}

pub(crate) fn effective_output_config(
    base: &StreamConfig,
    desired_sample_rate: Option<u32>,
) -> StreamConfig {
    let mut config = base.clone();
    if let Some(rate) = desired_sample_rate {
        config.sample_rate = rate;
    }
    config
}

pub(crate) fn output_rate_fallback_config(
    attempted: &StreamConfig,
    base: &StreamConfig,
) -> Option<StreamConfig> {
    (attempted.sample_rate != base.sample_rate).then(|| base.clone())
}

/// Retire the callback identity before CPAL tears down its native stream.
/// A driver can report a late error while Drop is joining its callback thread.
pub(crate) struct RetiringOutputStream<T> {
    stream: Option<T>,
    live: Arc<AtomicBool>,
}

pub(crate) type CpalOutputStream = RetiringOutputStream<Stream>;

impl<T> Drop for RetiringOutputStream<T> {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
        drop(self.stream.take());
    }
}

pub(crate) fn emit_cpal_stream_error(
    shared: &PlaybackSharedState,
    output_live: &Arc<AtomicBool>,
    err: cpal::Error,
) {
    if !output_live.load(Ordering::Acquire) || shared.stopped.load(Ordering::Acquire) {
        return;
    }
    let notice = match err.kind() {
        cpal::ErrorKind::Xrun => 1,
        cpal::ErrorKind::RealtimeDenied => 2,
        cpal::ErrorKind::DeviceChanged => 4,
        _ => 0,
    };
    if notice != 0 {
        // Native error callbacks may share the audio worker. Recoverable
        // notices need neither allocation nor a transport pause.
        shared
            .deferred_cpal_notices
            .fetch_or(notice, Ordering::Relaxed);
        return;
    }
    let _ = shared
        .command_tx
        .send(PlaybackRuntimeCommand::CpalOutputError {
            track_id: shared.track_id,
            generation: shared.generation,
            decoder_stop: shared.stop_flag(),
            output_live: Arc::clone(output_live),
            message: format!("Playback output stream error: {err}"),
        });
}

fn build_output_stream(
    device: &cpal::Device,
    output_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<CpalOutputStream> {
    let live = Arc::new(AtomicBool::new(true));
    // Guard build failures too: callbacks queued before a failed native open
    // must not be mistaken for errors from its fallback on the same decoder.
    let mut output = RetiringOutputStream {
        stream: None,
        live: Arc::clone(&live),
    };
    let stream = match output_sample_format {
        SampleFormat::F32 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&live);
            device.build_output_stream(
                *output_config,
                move |data: &mut [f32], _| write_output_f32(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::F64 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&live);
            device.build_output_stream(
                *output_config,
                move |data: &mut [f64], _| write_output_f64(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::I24 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&live);
            device.build_output_stream(
                *output_config,
                move |data: &mut [cpal::I24], _| {
                    write_output_i24(data, &shared, &command_tx, &event_tx)
                },
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::I16 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&live);
            device.build_output_stream(
                *output_config,
                move |data: &mut [i16], _| write_output_i16(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::I32 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&live);
            device.build_output_stream(
                *output_config,
                move |data: &mut [i32], _| write_output_i32(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::U16 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&live);
            device.build_output_stream(
                *output_config,
                move |data: &mut [u16], _| write_output_u16(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        other => {
            return Err(anyhow!(
                "unsupported output sample format for playback runtime: {other:?}"
            ));
        }
    };

    output.stream = Some(stream);
    Ok(output)
}

fn start_cpal_stream(stream: &CpalOutputStream) -> Result<()> {
    stream
        .stream
        .as_ref()
        .context("output stream retired before start")?
        .play()
        .context("failed to start cpal stream")
}

fn build_started_output_stream(
    device: &cpal::Device,
    output_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<CpalOutputStream> {
    let stream = build_output_stream(
        device,
        output_config,
        output_sample_format,
        shared,
        command_tx,
        event_tx,
    )?;
    start_cpal_stream(&stream)?;
    Ok(stream)
}

pub(crate) fn build_started_output_stream_with_rate_fallback(
    device: &cpal::Device,
    attempted_config: &StreamConfig,
    fallback_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<(CpalOutputStream, u32)> {
    match build_started_output_stream(
        device,
        attempted_config,
        output_sample_format,
        Arc::clone(&shared),
        command_tx.clone(),
        event_tx.clone(),
    ) {
        Ok(stream) => Ok((stream, attempted_config.sample_rate)),
        Err(primary_error) => {
            let Some(fallback_config) =
                output_rate_fallback_config(attempted_config, fallback_config)
            else {
                return Err(primary_error);
            };
            warn!(
                "Output stream rejected or failed to start at {} Hz; falling back to {} Hz: {primary_error}",
                attempted_config.sample_rate, fallback_config.sample_rate
            );
            let stream = build_started_output_stream(
                device,
                &fallback_config,
                output_sample_format,
                shared,
                command_tx,
                event_tx,
            )
            .with_context(|| {
                format!(
                    "fallback output stream at {} Hz also failed after {} Hz was rejected or failed to start",
                    fallback_config.sample_rate, attempted_config.sample_rate
                )
            })?;
            Ok((stream, fallback_config.sample_rate))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::gapless::GaplessPlan;
    use crate::playback::player::PlaybackSourceKind;
    use std::sync::atomic::{AtomicU32, AtomicU64};

    fn shared(command_tx: mpsc::Sender<PlaybackRuntimeCommand>) -> PlaybackSharedState {
        PlaybackSharedState::new(
            42,
            7,
            PlaybackSourceKind::TidalStream,
            GaplessPlan::disabled(),
            48_000,
            2,
            None,
            command_tx,
            Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        )
    }

    #[test]
    fn cpal_error_callback_queues_identity_for_runtime_reconciliation() {
        let (command_tx, command_rx) = mpsc::channel();
        let shared = shared(command_tx);
        let live = Arc::new(AtomicBool::new(true));
        emit_cpal_stream_error(
            &shared,
            &live,
            cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable),
        );
        match command_rx.try_recv().unwrap() {
            PlaybackRuntimeCommand::CpalOutputError {
                track_id,
                generation,
                decoder_stop,
                output_live,
                message,
            } => {
                assert_eq!((track_id, generation), (42, 7));
                assert!(Arc::ptr_eq(&decoder_stop, &shared.stopped));
                assert!(Arc::ptr_eq(&output_live, &live));
                assert!(message.contains("Playback output stream error"));
            }
            other => panic!("expected native fault command, got {other:?}"),
        }
    }

    #[test]
    fn retired_output_and_decoder_callbacks_cannot_queue_faults() {
        let (command_tx, command_rx) = mpsc::channel();
        let shared = shared(command_tx);
        let live = Arc::new(AtomicBool::new(false));
        emit_cpal_stream_error(
            &shared,
            &live,
            cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable),
        );
        live.store(true, Ordering::Release);
        shared.stopped.store(true, Ordering::Release);
        emit_cpal_stream_error(
            &shared,
            &live,
            cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable),
        );
        assert!(command_rx.try_recv().is_err());
    }

    #[test]
    fn recoverable_cpal_notices_do_not_queue_faults() {
        for (kind, notice) in [
            (cpal::ErrorKind::Xrun, 1),
            (cpal::ErrorKind::RealtimeDenied, 2),
            (cpal::ErrorKind::DeviceChanged, 4),
        ] {
            let (command_tx, command_rx) = mpsc::channel();
            let shared = shared(command_tx);
            let live = Arc::new(AtomicBool::new(true));
            emit_cpal_stream_error(&shared, &live, cpal::Error::new(kind));
            assert!(
                command_rx.try_recv().is_err(),
                "recoverable notice {kind:?} stopped playback"
            );
            assert_eq!(shared.deferred_cpal_notices.load(Ordering::Relaxed), notice);
            assert!(!shared.paused.load(Ordering::Acquire));
        }
    }

    #[test]
    fn output_drop_retires_identity_before_native_destructor() {
        struct NativeDropProbe(Arc<AtomicBool>);
        impl Drop for NativeDropProbe {
            fn drop(&mut self) {
                assert!(!self.0.load(Ordering::Acquire));
            }
        }
        let live = Arc::new(AtomicBool::new(true));
        drop(RetiringOutputStream {
            stream: Some(NativeDropProbe(Arc::clone(&live))),
            live,
        });
    }

    #[test]
    fn failed_output_build_guard_retires_callback_identity() {
        let live = Arc::new(AtomicBool::new(true));
        let output: RetiringOutputStream<Stream> = RetiringOutputStream {
            stream: None,
            live: Arc::clone(&live),
        };
        drop(output);
        assert!(!live.load(Ordering::Acquire));
    }
}
