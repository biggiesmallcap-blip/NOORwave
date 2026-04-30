//! Real WASAPI exclusive-mode output stream.
//!
//! cpal 0.15 only exposes shared-mode output, which always routes through the
//! Windows audio engine (mixer, resampler, system DSP). Audiophile users want
//! bit-perfect playback that bypasses the engine entirely — `ShareMode::Exclusive`.
//!
//! This module drives `IAudioClient` directly via the `wasapi` crate to grab
//! the device exclusively. While exclusive mode is engaged, no other process
//! (including Windows itself) can play audio on this device.
//!
//! Threading model: every WASAPI call is performed on the render thread so we
//! never have to send COM interfaces across thread boundaries (wasapi-rs's
//! types contain raw pointers and aren't `Send`). The render thread reads
//! from the same `PlaybackSharedState` the cpal callback path uses, so
//! pause/stop/volume/seek still flow through the existing shared atomics.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Context, Result};
use tracing::{info, warn};
use wasapi::{
    initialize_mta, Direction, DeviceEnumerator, SampleType, ShareMode, StreamMode, WaveFormat,
};

use super::runtime::{
    fill_f32_from_shared, PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackSharedState,
};

/// Live exclusive-mode output. Drop to stop the render thread and release the
/// device back to the OS.
pub struct ExclusiveStream {
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub effective_sample_rate: u32,
    pub effective_channels: u16,
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Clone, Copy)]
enum Format {
    F32,
    I32,
    I16,
}

/// Build and start an exclusive-mode output stream targeting `device_pref`
/// (None = system default) at `desired_sample_rate` Hz, `channels`-channel.
///
/// Init runs on the render thread; this function blocks until the thread
/// either reports the chosen format (success) or fails the WASAPI handshake.
pub fn build_exclusive_stream(
    device_pref: Option<&str>,
    desired_sample_rate: u32,
    channels: u16,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<ExclusiveStream> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let device_pref_owned = device_pref.map(|s| s.to_string());

    let (init_tx, init_rx) = mpsc::sync_channel::<Result<(u32, u16)>>(1);

    let thread = thread::Builder::new()
        .name("noor-wasapi-exclusive".into())
        .spawn(move || {
            run_render_thread(
                device_pref_owned,
                desired_sample_rate,
                channels,
                shared,
                command_tx,
                event_tx,
                shutdown_clone,
                init_tx,
            );
        })
        .context("failed to spawn WASAPI render thread")?;

    let (effective_rate, effective_channels) = init_rx
        .recv()
        .context("WASAPI render thread died before reporting init result")?
        ?;

    Ok(ExclusiveStream {
        shutdown,
        thread: Some(thread),
        effective_sample_rate: effective_rate,
        effective_channels,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_render_thread(
    device_pref: Option<String>,
    desired_sample_rate: u32,
    channels: u16,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    shutdown: Arc<AtomicBool>,
    init_tx: mpsc::SyncSender<Result<(u32, u16)>>,
) {
    let init_result = init_audio_client(device_pref.as_deref(), desired_sample_rate, channels);
    let (audio_client, render_client, event_handle, blockalign, fmt_tag) = match init_result {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let _ = init_tx.send(Ok((desired_sample_rate, channels)));

    if let Err(e) = audio_client.start_stream() {
        warn!("WASAPI start_stream failed: {e}");
        return;
    }

    let mut f32_scratch: Vec<f32> = Vec::new();
    let mut byte_buf: Vec<u8> = Vec::new();

    while !shutdown.load(Ordering::SeqCst) {
        let frames = match audio_client.get_available_space_in_frames() {
            Ok(n) => n as usize,
            Err(e) => {
                warn!("WASAPI get_available_space failed: {e}");
                break;
            }
        };

        if frames > 0 {
            let interleaved = frames * channels as usize;
            f32_scratch.resize(interleaved, 0.0);
            f32_scratch.fill(0.0);

            fill_f32_from_shared(&mut f32_scratch, &shared, &command_tx, &event_tx);

            let bytes_needed = frames * blockalign;
            byte_buf.resize(bytes_needed, 0);
            convert_f32_to_bytes(&f32_scratch, &mut byte_buf, fmt_tag, blockalign, channels as usize);

            if let Err(e) = render_client.write_to_device(frames, &byte_buf, None) {
                warn!("WASAPI write_to_device failed: {e}");
                break;
            }
        }

        if event_handle.wait_for_event(2000).is_err() {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            // No event within 2 s while not shutting down: device probably
            // disappeared (USB DAC unplug etc.). Bail and let the next
            // DeviceSwap rebuild on whatever's there now.
            warn!("WASAPI render thread: event wait timed out, exiting render loop");
            break;
        }
    }

    let _ = audio_client.stop_stream();
}

fn init_audio_client(
    device_pref: Option<&str>,
    desired_sample_rate: u32,
    channels: u16,
) -> Result<(
    wasapi::AudioClient,
    wasapi::AudioRenderClient,
    wasapi::Handle,
    usize,
    Format,
)> {
    // COM init for the render thread. Returns RPC_E_CHANGED_MODE / S_FALSE
    // if already initialized; we only fail on hard errors.
    let hr = initialize_mta();
    if hr.is_err() {
        let code = hr.0;
        // S_FALSE (1) and RPC_E_CHANGED_MODE (0x80010106) are benign.
        if code != 1 && code as u32 != 0x80010106 {
            return Err(anyhow!("WASAPI MTA init failed (HRESULT 0x{code:08x})"));
        }
    }

    let enumerator =
        DeviceEnumerator::new().map_err(|e| anyhow!("WASAPI device enumerator failed: {e}"))?;

    let device = match device_pref {
        Some(name) => {
            let collection = enumerator
                .get_device_collection(&Direction::Render)
                .map_err(|e| anyhow!("WASAPI device collection failed: {e}"))?;
            collection
                .get_device_with_name(name)
                .or_else(|_| enumerator.get_default_device(&Direction::Render))
                .map_err(|e| anyhow!("WASAPI device lookup failed: {e}"))?
        }
        None => enumerator
            .get_default_device(&Direction::Render)
            .map_err(|e| anyhow!("WASAPI default render device failed: {e}"))?,
    };

    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| anyhow!("WASAPI get_iaudioclient failed: {e}"))?;

    // Try f32 first (best fidelity, what most modern DACs accept), then fall
    // back to 24-in-32 int and 16-bit int. Stop at the first format the
    // device accepts in EXCLUSIVE mode.
    let candidates: &[(usize, usize, SampleType, Format)] = &[
        (32, 32, SampleType::Float, Format::F32),
        (32, 24, SampleType::Int, Format::I32),
        (16, 16, SampleType::Int, Format::I16),
    ];

    let mut chosen: Option<(WaveFormat, Format)> = None;
    for (storebits, validbits, sample_type, fmt_tag) in candidates {
        let wf = WaveFormat::new(
            *storebits,
            *validbits,
            sample_type,
            desired_sample_rate as usize,
            channels as usize,
            None,
        );
        match audio_client.is_supported(&wf, &ShareMode::Exclusive) {
            Ok(None) => {
                chosen = Some((wf, *fmt_tag));
                break;
            }
            Ok(Some(_)) => continue,
            Err(_) => continue,
        }
    }

    let (format, fmt_tag) = chosen.ok_or_else(|| {
        anyhow!(
            "no exclusive-mode format accepted by device at {} Hz × {} ch (tried f32, i24-in-32, i16)",
            desired_sample_rate,
            channels
        )
    })?;

    let blockalign = format.get_blockalign() as usize;

    // Use the device's minimum period for low latency. EventsExclusive
    // sets buffer_duration == period.
    let (_def_period, min_period) = audio_client
        .get_device_period()
        .map_err(|e| anyhow!("WASAPI get_device_period failed: {e}"))?;
    let mode = StreamMode::EventsExclusive {
        period_hns: min_period,
    };

    audio_client
        .initialize_client(&format, &Direction::Render, &mode)
        .map_err(|e| anyhow!("WASAPI initialize_client (exclusive) failed: {e}"))?;

    let event_handle = audio_client
        .set_get_eventhandle()
        .map_err(|e| anyhow!("WASAPI set_get_eventhandle failed: {e}"))?;

    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|e| anyhow!("WASAPI get_audiorenderclient failed: {e}"))?;

    info!(
        target: "playback",
        "WASAPI exclusive stream initialized: {} Hz × {} ch, {} bytes/frame, period {} hns",
        desired_sample_rate, channels, blockalign, min_period
    );

    Ok((audio_client, render_client, event_handle, blockalign, fmt_tag))
}

fn convert_f32_to_bytes(
    src: &[f32],
    dst: &mut [u8],
    fmt: Format,
    blockalign: usize,
    channels: usize,
) {
    match fmt {
        Format::F32 => {
            // Each sample is 4 bytes, channels * 4 = blockalign.
            debug_assert_eq!(blockalign, channels * 4);
            for (chunk, &s) in dst.chunks_exact_mut(4).zip(src.iter()) {
                chunk.copy_from_slice(&s.to_le_bytes());
            }
        }
        Format::I32 => {
            // 24-in-32: WAVEFORMATEXTENSIBLE with 32 storebits + 24 validbits
            // means the device looks at the high 24 bits. Filling the full
            // i32 range is acceptable; the bottom 8 bits are ignored.
            debug_assert_eq!(blockalign, channels * 4);
            for (chunk, &s) in dst.chunks_exact_mut(4).zip(src.iter()) {
                let v = (s.clamp(-1.0, 1.0) * (i32::MAX as f32)) as i32;
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
        Format::I16 => {
            debug_assert_eq!(blockalign, channels * 2);
            for (chunk, &s) in dst.chunks_exact_mut(2).zip(src.iter()) {
                let v = (s.clamp(-1.0, 1.0) * (i16::MAX as f32)) as i16;
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}
