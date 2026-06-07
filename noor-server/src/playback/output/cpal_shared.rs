//! CPAL shared-mode output helpers.

use crate::playback::runtime::commands::{PlaybackRuntimeCommand, PlaybackRuntimeEvent};
use crate::playback::runtime::shared::{
    PlaybackSharedState, write_output_f32, write_output_i16, write_output_u16,
};
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::{Arc, mpsc};
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

/// Surface a CPAL stream-level error (device unplugged, format change,
/// backend fault, etc.) both as a log line and as a user-visible
/// PlaybackRuntimeEvent::Error. CPAL invokes its error callback off the
/// audio thread, so allocation and broadcast send are safe here.
pub(crate) fn emit_cpal_stream_error(
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    err: cpal::StreamError,
) {
    let message = format!("Playback output stream error: {err}");
    warn!("{message}");
    let _ = event_tx.send(PlaybackRuntimeEvent::Error { message });
}

fn build_output_stream(
    device: &cpal::Device,
    output_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<Stream> {
    let stream = match output_sample_format {
        SampleFormat::F32 => {
            let err_event_tx = event_tx.clone();
            device.build_output_stream(
                output_config,
                move |data: &mut [f32], _| write_output_f32(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&err_event_tx, err),
                None,
            )?
        }
        SampleFormat::I16 => {
            let err_event_tx = event_tx.clone();
            device.build_output_stream(
                output_config,
                move |data: &mut [i16], _| write_output_i16(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&err_event_tx, err),
                None,
            )?
        }
        SampleFormat::U16 => {
            let err_event_tx = event_tx.clone();
            device.build_output_stream(
                output_config,
                move |data: &mut [u16], _| write_output_u16(data, &shared, &command_tx, &event_tx),
                move |err| emit_cpal_stream_error(&err_event_tx, err),
                None,
            )?
        }
        other => {
            return Err(anyhow!(
                "unsupported output sample format for playback runtime: {other:?}"
            ));
        }
    };

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_cpal_stream_error_warns_and_emits_runtime_error_event() {
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);

        emit_cpal_stream_error(&event_tx, cpal::StreamError::DeviceNotAvailable);

        match event_rx.try_recv().expect("error event should be emitted") {
            PlaybackRuntimeEvent::Error { message } => {
                assert!(message.contains("Playback output stream error"));
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }
}

fn start_cpal_stream(stream: &Stream) -> Result<()> {
    stream.play().context("failed to start cpal stream")
}

fn build_started_output_stream(
    device: &cpal::Device,
    output_config: &StreamConfig,
    output_sample_format: SampleFormat,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<Stream> {
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
) -> Result<(Stream, u32)> {
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
