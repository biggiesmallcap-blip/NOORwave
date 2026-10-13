//! CPAL shared-mode output helpers.

use crate::playback::runtime::commands::PlaybackRuntimeCommand;
use crate::playback::runtime::shared::{
    PlaybackSharedState, write_output_f32, write_output_f64, write_output_i16, write_output_i24,
    write_output_i32, write_output_u16,
};
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::{
    Arc, OnceLock,
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
    callback: Arc<CpalCallbackState>,
}

/// The stream owner retains the diagnostic until native teardown has joined
/// callbacks. Only the first fatal producer writes; consumers never wait.
pub(crate) struct CpalCallbackState {
    live: Arc<AtomicBool>,
    claimed: AtomicBool,
    delivered: AtomicBool,
    error: OnceLock<cpal::Error>,
}

impl CpalCallbackState {
    pub(crate) fn new(live: Arc<AtomicBool>) -> Self {
        Self {
            live,
            claimed: AtomicBool::new(false),
            delivered: AtomicBool::new(false),
            error: OnceLock::new(),
        }
    }

    pub(crate) fn take_command(
        &self,
        shared: &PlaybackSharedState,
    ) -> Option<PlaybackRuntimeCommand> {
        if !self.live.load(Ordering::Acquire) || shared.stopped.load(Ordering::Acquire) {
            return None;
        }
        let error = self.error.get()?;
        if self.delivered.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(PlaybackRuntimeCommand::CpalOutputError {
            track_id: shared.track_id,
            generation: shared.generation,
            decoder_stop: shared.stop_flag(),
            output_live: Arc::clone(&self.live),
            message: format!("Playback output stream error: {error}"),
        })
    }
}

impl<T> RetiringOutputStream<T> {
    pub(crate) fn take_error(
        &self,
        shared: &PlaybackSharedState,
    ) -> Option<PlaybackRuntimeCommand> {
        self.callback.take_command(shared)
    }
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
    callback: &CpalCallbackState,
    err: cpal::Error,
) {
    if !callback.live.load(Ordering::Acquire) || shared.stopped.load(Ordering::Acquire) {
        return;
    }
    let notice = match err.kind() {
        cpal::ErrorKind::Xrun => 1,
        cpal::ErrorKind::RealtimeDenied => 2,
        cpal::ErrorKind::DeviceChanged => 4,
        // cpal 0.18 WASAPI reports a system default-device change on a
        // default-device stream as a message-less StreamInvalidated (or
        // DeviceNotAvailable when no default remains). Every genuine WASAPI
        // fault carries a message. The stream keeps rendering to its original
        // endpoint, so the runtime follows the new default instead of pausing.
        cpal::ErrorKind::StreamInvalidated | cpal::ErrorKind::DeviceNotAvailable
            if err.message().is_none() =>
        {
            8
        }
        // WASAPI falls back to SetThreadPriority when MMCSS refuses and
        // reports that fallback's failure as a backend error. Audio still
        // plays, so it is a notice rather than a fault.
        _ if is_priority_boost_failure(&err) => 2,
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
    if callback
        .claimed
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        // Only the first fatal error is kept. The claim keeps the OnceLock
        // initialization uncontended, so a callback never waits on another;
        // later errors are simply dropped.
        let _ = callback.error.set(err);
    }
}

fn is_priority_boost_failure(err: &cpal::Error) -> bool {
    err.message()
        .is_some_and(|message| message.starts_with("Failed to promote audio thread"))
}

fn build_output_stream(
    device: &cpal::Device,
    output_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
) -> Result<CpalOutputStream> {
    let live = Arc::new(AtomicBool::new(true));
    let callback = Arc::new(CpalCallbackState::new(Arc::clone(&live)));
    // Guard build failures too: callbacks queued before a failed native open
    // must not be mistaken for errors from its fallback on the same decoder.
    let mut output = RetiringOutputStream {
        stream: None,
        live: Arc::clone(&live),
        callback: Arc::clone(&callback),
    };
    let stream = match output_sample_format {
        SampleFormat::F32 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&callback);
            device.build_output_stream(
                *output_config,
                move |data: &mut [f32], _| write_output_f32(data, &shared),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::F64 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&callback);
            device.build_output_stream(
                *output_config,
                move |data: &mut [f64], _| write_output_f64(data, &shared),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::I24 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&callback);
            device.build_output_stream(
                *output_config,
                move |data: &mut [cpal::I24], _| write_output_i24(data, &shared),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::I16 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&callback);
            device.build_output_stream(
                *output_config,
                move |data: &mut [i16], _| write_output_i16(data, &shared),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::I32 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&callback);
            device.build_output_stream(
                *output_config,
                move |data: &mut [i32], _| write_output_i32(data, &shared),
                move |err| emit_cpal_stream_error(&error_shared, &error_live, err),
                None,
            )?
        }
        SampleFormat::U16 => {
            let error_shared = Arc::clone(&shared);
            let error_live = Arc::clone(&callback);
            device.build_output_stream(
                *output_config,
                move |data: &mut [u16], _| write_output_u16(data, &shared),
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
) -> Result<CpalOutputStream> {
    let stream = build_output_stream(device, output_config, output_sample_format, shared)?;
    start_cpal_stream(&stream)?;
    Ok(stream)
}

pub(crate) fn build_started_output_stream_with_rate_fallback(
    device: &cpal::Device,
    attempted_config: &StreamConfig,
    fallback_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
) -> Result<(CpalOutputStream, u32)> {
    match build_started_output_stream(
        device,
        attempted_config,
        output_sample_format,
        Arc::clone(&shared),
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

    /// WASAPI device removal always carries the HRESULT text; only a
    /// default-device change arrives without a message.
    fn device_removed() -> cpal::Error {
        cpal::Error::with_message(
            cpal::ErrorKind::DeviceNotAvailable,
            "The audio endpoint device has been unplugged",
        )
    }

    #[test]
    fn cpal_error_callback_retains_identity_for_runtime_reconciliation() {
        let (command_tx, command_rx) = mpsc::channel();
        let shared = shared(command_tx);
        let live = Arc::new(AtomicBool::new(true));
        let callback = Arc::new(CpalCallbackState::new(Arc::clone(&live)));
        emit_cpal_stream_error(&shared, &callback, device_removed());
        assert!(command_rx.try_recv().is_err());
        match callback.take_command(&shared).unwrap() {
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
        assert!(callback.take_command(&shared).is_none());
    }

    #[test]
    fn retired_output_and_decoder_callbacks_cannot_queue_faults() {
        let (command_tx, command_rx) = mpsc::channel();
        let shared = shared(command_tx);
        let live = Arc::new(AtomicBool::new(false));
        let callback = Arc::new(CpalCallbackState::new(Arc::clone(&live)));
        emit_cpal_stream_error(&shared, &callback, device_removed());
        live.store(true, Ordering::Release);
        shared.stopped.store(true, Ordering::Release);
        emit_cpal_stream_error(&shared, &callback, device_removed());
        assert!(command_rx.try_recv().is_err());
        assert!(callback.take_command(&shared).is_none());
    }

    #[test]
    fn recoverable_cpal_notices_do_not_queue_faults() {
        for (error, notice) in [
            (cpal::Error::new(cpal::ErrorKind::Xrun), 1),
            (cpal::Error::new(cpal::ErrorKind::RealtimeDenied), 2),
            (cpal::Error::new(cpal::ErrorKind::DeviceChanged), 4),
            (cpal::Error::new(cpal::ErrorKind::StreamInvalidated), 8),
            (cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable), 8),
            (
                cpal::Error::with_message(
                    cpal::ErrorKind::BackendError,
                    "Failed to promote audio thread to real-time priority: Access is denied.",
                ),
                2,
            ),
        ] {
            let (command_tx, command_rx) = mpsc::channel();
            let shared = shared(command_tx);
            let live = Arc::new(AtomicBool::new(true));
            let callback = Arc::new(CpalCallbackState::new(Arc::clone(&live)));
            let label = error.to_string();
            emit_cpal_stream_error(&shared, &callback, error);
            assert!(
                command_rx.try_recv().is_err(),
                "recoverable notice {label} stopped playback"
            );
            assert!(
                callback.take_command(&shared).is_none(),
                "{label} became fatal"
            );
            assert_eq!(shared.deferred_cpal_notices.load(Ordering::Relaxed), notice);
            assert!(!shared.paused.load(Ordering::Acquire));
        }
    }

    #[test]
    fn output_drop_retires_identity_before_native_destructor() {
        struct NativeDropProbe(Arc<AtomicBool>, std::sync::Weak<CpalCallbackState>);
        impl Drop for NativeDropProbe {
            fn drop(&mut self) {
                assert!(!self.0.load(Ordering::Acquire));
                let callback = self
                    .1
                    .upgrade()
                    .expect("owner retains diagnostics through teardown");
                assert_eq!(
                    callback.error.get().unwrap().message(),
                    Some("retained through join")
                );
            }
        }
        let live = Arc::new(AtomicBool::new(true));
        let callback = Arc::new(CpalCallbackState::new(Arc::clone(&live)));
        let (command_tx, _) = mpsc::channel();
        emit_cpal_stream_error(
            &shared(command_tx),
            &callback,
            cpal::Error::with_message(cpal::ErrorKind::DeviceNotAvailable, "retained through join"),
        );
        drop(RetiringOutputStream {
            stream: Some(NativeDropProbe(
                Arc::clone(&live),
                Arc::downgrade(&callback),
            )),
            callback,
            live,
        });
    }

    #[test]
    fn fatal_diagnostic_is_delivered_once_and_not_inherited_by_replacement() {
        let (command_tx, _) = mpsc::channel();
        let shared = shared(command_tx);
        let live = Arc::new(AtomicBool::new(true));
        let callback = CpalCallbackState::new(Arc::clone(&live));
        for message in ["first diagnostic", "duplicate diagnostic"] {
            emit_cpal_stream_error(
                &shared,
                &callback,
                cpal::Error::with_message(cpal::ErrorKind::DeviceNotAvailable, message),
            );
        }
        match callback.take_command(&shared).unwrap() {
            PlaybackRuntimeCommand::CpalOutputError { message, .. } => {
                assert!(message.contains("first diagnostic"))
            }
            other => panic!("expected fault, got {other:?}"),
        }
        assert!(callback.take_command(&shared).is_none());
        let replacement = CpalCallbackState::new(Arc::new(AtomicBool::new(true)));
        assert!(replacement.take_command(&shared).is_none());
        live.store(false, Ordering::Release);
        assert!(callback.take_command(&shared).is_none());
    }

    #[test]
    fn failed_output_build_guard_retires_callback_identity() {
        let live = Arc::new(AtomicBool::new(true));
        let callback = Arc::new(CpalCallbackState::new(Arc::clone(&live)));
        let output: RetiringOutputStream<Stream> = RetiringOutputStream {
            stream: None,
            callback,
            live: Arc::clone(&live),
        };
        drop(output);
        assert!(!live.load(Ordering::Acquire));
    }
}
