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
//!
//! On-demand grab: rather than holding the device for the lifetime of the
//! `ExclusiveStream`, the render thread self-releases after `grace_secs` of
//! continuous paused state. The runtime detects the release via
//! [`ExclusiveStream::is_released`] and re-grabs on the next Resume / Play.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{info, warn};
use wasapi::{Direction, SampleType, StreamMode, WasapiError, WaveFormat, initialize_mta};

use super::runtime::{
    PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackSharedState, fill_f32_from_shared,
};

// AUDCLNT_E_* HRESULTs from <audioclient.h>. Raw i32 reps so we don't have to
// pull windows / windows-core into noor-server's direct deps just to match a
// handful of error codes. wasapi-rs's WasapiError::Windows wraps a
// windows_core::Error whose .code() returns a windows_core::HRESULT; that
// type's wire form is i32, accessible via `.0`.
const AUDCLNT_E_UNSUPPORTED_FORMAT: i32 = 0x8889_0008u32 as i32;
const AUDCLNT_E_DEVICE_IN_USE: i32 = 0x8889_000Au32 as i32;
const AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED: i32 = 0x8889_000Eu32 as i32;
const AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED: i32 = 0x8889_0019u32 as i32;
const AUDCLNT_E_INVALID_DEVICE_PERIOD: i32 = 0x8889_0020u32 as i32;

/// Reason the exclusive-mode WASAPI grab failed. Plumbed up to the route
/// layer so the UI can render a specific, actionable error rather than a
/// generic "audio failed" message.
#[derive(Debug, Clone)]
pub enum ExclusiveInitFailure {
    /// Another process holds the device in *exclusive* mode (different from a
    /// shared-mode app, which we can preempt). User has to close that app.
    DeviceInUse,
    /// Device's "Allow applications to take exclusive control" checkbox is off
    /// in Sound Control Panel. Retry/format won't help.
    ExclusiveDisabled,
    /// Tried every candidate format and the device rejected all of them in
    /// exclusive mode.
    NoFormatAccepted,
    /// Anything else, with the diagnostic string preserved for the log/UI.
    Other(String),
}

impl ExclusiveInitFailure {
    pub fn user_message(&self) -> String {
        match self {
            Self::DeviceInUse => {
                "Another application is using this device exclusively. Close it and retry."
                    .to_string()
            }
            Self::ExclusiveDisabled => {
                "Exclusive mode is disabled in Windows for this device. Enable it in Sound \
                 Control Panel \u{2192} Properties \u{2192} Advanced."
                    .to_string()
            }
            Self::NoFormatAccepted => {
                "This device rejected every exclusive-mode format we tried (f32, 24-in-32, \
                 16-bit). Pick a different output device."
                    .to_string()
            }
            Self::Other(s) => format!("Exclusive mode failed: {s}"),
        }
    }
}

impl std::fmt::Display for ExclusiveInitFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.user_message())
    }
}

impl std::error::Error for ExclusiveInitFailure {}

/// Live exclusive-mode output. Drop to stop the render thread and release the
/// device back to the OS.
pub struct ExclusiveStream {
    shutdown: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    #[allow(dead_code)]
    pub effective_sample_rate: u32,
    #[allow(dead_code)]
    pub effective_channels: u16,
}

impl ExclusiveStream {
    /// `true` once the render thread has self-shut due to idle (paused for
    /// longer than `grace_secs`) or because the device went away. The runtime
    /// uses this to know it must rebuild the stream before resuming.
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }
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
/// `grace_secs` is the idle-release window: the render thread releases the
/// device after this many seconds of continuous paused state.
///
/// `device_label` is the human-readable name surfaced to the UI in any
/// `ExclusiveModeReleased` event the render thread emits when it self-shuts.
///
/// Init runs on the render thread; this function blocks until the thread
/// either reports the chosen format (success) or fails the WASAPI handshake.
#[allow(clippy::too_many_arguments)]
pub fn build_exclusive_stream(
    device_pref: Option<&str>,
    device_label: String,
    desired_sample_rate: u32,
    channels: u16,
    grace_secs: u32,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> std::result::Result<ExclusiveStream, ExclusiveInitFailure> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let released = Arc::new(AtomicBool::new(false));
    let released_clone = Arc::clone(&released);
    let device_pref_owned = device_pref.map(|s| s.to_string());

    let (init_tx, init_rx) =
        mpsc::sync_channel::<std::result::Result<(u32, u16), ExclusiveInitFailure>>(1);

    let thread = thread::Builder::new()
        .name("noor-wasapi-exclusive".into())
        .spawn(move || {
            run_render_thread(
                device_pref_owned,
                device_label,
                desired_sample_rate,
                channels,
                grace_secs,
                shared,
                command_tx,
                event_tx,
                shutdown_clone,
                released_clone,
                init_tx,
            );
        })
        .map_err(|e| ExclusiveInitFailure::Other(format!("spawn render thread: {e}")))?;

    let outcome = init_rx.recv().map_err(|_| {
        ExclusiveInitFailure::Other("WASAPI render thread died before reporting init result".into())
    })?;

    match outcome {
        Ok((effective_rate, effective_channels)) => Ok(ExclusiveStream {
            shutdown,
            released,
            thread: Some(thread),
            effective_sample_rate: effective_rate,
            effective_channels,
        }),
        Err(failure) => {
            let _ = thread.join();
            Err(failure)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_render_thread(
    device_pref: Option<String>,
    device_label: String,
    desired_sample_rate: u32,
    channels: u16,
    grace_secs: u32,
    shared: Arc<PlaybackSharedState>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    shutdown: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
    init_tx: mpsc::SyncSender<std::result::Result<(u32, u16), ExclusiveInitFailure>>,
) {
    let init_result = init_audio_client(device_pref.as_deref(), desired_sample_rate, channels);
    let (audio_client, render_client, event_handle, blockalign, fmt_tag) = match init_result {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            released.store(true, Ordering::Release);
            return;
        }
    };
    let _ = init_tx.send(Ok((desired_sample_rate, channels)));

    if let Err(e) = audio_client.start_stream() {
        warn!("WASAPI start_stream failed: {e}");
        released.store(true, Ordering::Release);
        let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeReleased {
            device_name: device_label.clone(),
        });
        return;
    }

    let mut f32_scratch: Vec<f32> = Vec::new();
    let mut byte_buf: Vec<u8> = Vec::new();
    let grace = Duration::from_secs(grace_secs.max(1) as u64);
    let mut paused_since: Option<Instant> = None;

    while !shutdown.load(Ordering::SeqCst) {
        // Idle-release: when paused for >= grace_secs, exit the loop and let
        // Drop chains release the IAudioClient so other apps can use the
        // device. The runtime detects the release via `is_released()` and
        // rebuilds the stream on the next Resume/Play.
        if shared.paused.load(Ordering::Relaxed) {
            let now = Instant::now();
            match paused_since {
                None => paused_since = Some(now),
                Some(start) if now.duration_since(start) >= grace => {
                    info!(
                        target: "playback",
                        "WASAPI exclusive stream releasing device after {} s of idle",
                        grace.as_secs()
                    );
                    break;
                }
                _ => {}
            }
        } else {
            paused_since = None;
        }

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
            convert_f32_to_bytes(
                &f32_scratch,
                &mut byte_buf,
                fmt_tag,
                blockalign,
                channels as usize,
            );

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
    released.store(true, Ordering::Release);

    // Tell the world we let go of the device. The runtime listens to this
    // (via the route layer's WS bridge) so the UI can clear "engaged" state
    // and the runtime can re-grab on the next Resume/Play. Skipped on a
    // user-initiated shutdown — the engine swap that's about to happen will
    // emit its own event.
    if !shutdown.load(Ordering::SeqCst) {
        let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeReleased {
            device_name: device_label.clone(),
        });
    }
}

/// Try every candidate format in turn, attempting `initialize_client` directly
/// rather than relying on `IsFormatSupported` (which lies about exclusive-mode
/// support while the shared-mode engine has the device bound).
fn init_audio_client(
    device_pref: Option<&str>,
    desired_sample_rate: u32,
    channels: u16,
) -> std::result::Result<
    (
        wasapi::AudioClient,
        wasapi::AudioRenderClient,
        wasapi::Handle,
        usize,
        Format,
    ),
    ExclusiveInitFailure,
> {
    // COM init for the render thread. Returns RPC_E_CHANGED_MODE / S_FALSE
    // if already initialized; we only fail on hard errors.
    let hr = initialize_mta();
    if hr.is_err() {
        let code = hr.0;
        // S_FALSE (1) and RPC_E_CHANGED_MODE (0x80010106) are benign.
        if code != 1 && code as u32 != 0x80010106 {
            return Err(ExclusiveInitFailure::Other(format!(
                "MTA init failed (HRESULT 0x{code:08x})"
            )));
        }
    }

    // We re-resolve the device fresh inside try_initialize_one so each
    // candidate format gets a clean IAudioClient (the COM object is in a bad
    // state after a failed Initialize).
    let candidates: &[(usize, usize, SampleType, Format)] = &[
        (32, 32, SampleType::Float, Format::F32),
        (32, 24, SampleType::Int, Format::I32),
        (16, 16, SampleType::Int, Format::I16),
    ];

    let mut last_failure: Option<ExclusiveInitFailure> = None;

    // Backoff schedule for AUDCLNT_E_DEVICE_IN_USE retries. Some drivers /
    // Windows audio engine versions don't release the shared session within
    // the first tens of ms; ~1.3 s total budget covers cases where a flat
    // 150 ms didn't (observed empirically with Chrome holding shared mode).
    // Each entry is the sleep BEFORE that attempt's retry — so 4 attempts:
    // initial try, then sleep 50 ms, retry, sleep 150 ms, retry, sleep 350
    // ms, retry, sleep 750 ms, give up.
    const DEVICE_IN_USE_BACKOFF_MS: &[u64] = &[50, 150, 350, 750];

    'candidate: for (storebits, validbits, sample_type, fmt_tag) in candidates {
        let mut attempt: usize = 0;
        loop {
            match try_initialize_one(
                device_pref,
                *storebits,
                *validbits,
                sample_type,
                desired_sample_rate,
                channels,
                *fmt_tag,
            ) {
                Ok(v) => {
                    if attempt > 0 {
                        info!(
                            target: "playback",
                            "WASAPI exclusive grab succeeded on attempt {} ({:?})",
                            attempt + 1,
                            fmt_tag_label(*fmt_tag)
                        );
                    }
                    return Ok(v);
                }
                Err(failure) => match failure {
                    ExclusiveInitFailure::DeviceInUse
                        if attempt < DEVICE_IN_USE_BACKOFF_MS.len() =>
                    {
                        let sleep_ms = DEVICE_IN_USE_BACKOFF_MS[attempt];
                        info!(
                            target: "playback",
                            "WASAPI exclusive grab attempt {} hit DEVICE_IN_USE ({:?}); \
                             retrying after {} ms",
                            attempt + 1,
                            fmt_tag_label(*fmt_tag),
                            sleep_ms
                        );
                        std::thread::sleep(Duration::from_millis(sleep_ms));
                        attempt += 1;
                        continue;
                    }
                    ExclusiveInitFailure::ExclusiveDisabled => {
                        // No format will fix this — bail immediately so the
                        // user gets a precise message.
                        warn!(
                            target: "playback",
                            "WASAPI exclusive grab: device has \"Allow exclusive control\" disabled"
                        );
                        return Err(failure);
                    }
                    ExclusiveInitFailure::DeviceInUse => {
                        // Exhausted retry budget. Surface as DeviceInUse.
                        warn!(
                            target: "playback",
                            "WASAPI exclusive grab: DEVICE_IN_USE persisted across {} attempts ({:?}); \
                             trying next format",
                            DEVICE_IN_USE_BACKOFF_MS.len() + 1,
                            fmt_tag_label(*fmt_tag)
                        );
                        last_failure = Some(ExclusiveInitFailure::DeviceInUse);
                        continue 'candidate;
                    }
                    other => {
                        info!(
                            target: "playback",
                            "WASAPI exclusive grab attempt {} ({:?}) rejected: {}",
                            attempt + 1,
                            fmt_tag_label(*fmt_tag),
                            other
                        );
                        last_failure = Some(other);
                        continue 'candidate;
                    }
                },
            }
        }
    }

    let final_failure = last_failure.unwrap_or(ExclusiveInitFailure::NoFormatAccepted);
    warn!(
        target: "playback",
        "WASAPI exclusive grab: all candidate formats exhausted; final failure = {}",
        final_failure
    );
    Err(final_failure)
}

#[allow(clippy::too_many_arguments)]
fn try_initialize_one(
    device_pref: Option<&str>,
    storebits: usize,
    validbits: usize,
    sample_type: &SampleType,
    desired_sample_rate: u32,
    channels: u16,
    fmt_tag: Format,
) -> std::result::Result<
    (
        wasapi::AudioClient,
        wasapi::AudioRenderClient,
        wasapi::Handle,
        usize,
        Format,
    ),
    ExclusiveInitFailure,
> {
    let enumerator = wasapi::DeviceEnumerator::new()
        .map_err(|e| ExclusiveInitFailure::Other(format!("device enumerator: {e}")))?;

    let device = match device_pref {
        Some(name) => {
            let collection = enumerator
                .get_device_collection(&Direction::Render)
                .map_err(|e| ExclusiveInitFailure::Other(format!("device collection: {e}")))?;
            collection
                .get_device_with_name(name)
                .or_else(|_| enumerator.get_default_device(&Direction::Render))
                .map_err(|e| ExclusiveInitFailure::Other(format!("device lookup: {e}")))?
        }
        None => enumerator
            .get_default_device(&Direction::Render)
            .map_err(|e| ExclusiveInitFailure::Other(format!("default render device: {e}")))?,
    };

    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_iaudioclient: {e}")))?;

    let format = WaveFormat::new(
        storebits,
        validbits,
        sample_type,
        desired_sample_rate as usize,
        channels as usize,
        None,
    );

    let blockalign = format.get_blockalign() as usize;

    let (_def_period, min_period) = audio_client
        .get_device_period()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_device_period: {e}")))?;
    let mode = StreamMode::EventsExclusive {
        period_hns: min_period,
    };

    if let Err(e) = audio_client.initialize_client(&format, &Direction::Render, &mode) {
        let classified = classify_init_error(&e);
        // Always log the raw HRESULT so we have ground-truth diagnostic data
        // when classification falls through to "Other" (i.e. something we
        // didn't recognize and aren't retrying).
        if let WasapiError::Windows(win_err) = &e {
            tracing::debug!(
                target: "playback",
                "WASAPI initialize_client returned HRESULT 0x{:08x} ({:?}, {} Hz \u{00d7} {} ch) \u{2192} {:?}",
                win_err.code().0 as u32,
                fmt_tag_label(fmt_tag),
                desired_sample_rate,
                channels,
                std::mem::discriminant(&classified)
            );
        }
        return Err(classified);
    }

    let event_handle = audio_client
        .set_get_eventhandle()
        .map_err(|e| ExclusiveInitFailure::Other(format!("set_get_eventhandle: {e}")))?;

    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_audiorenderclient: {e}")))?;

    info!(
        target: "playback",
        "WASAPI exclusive stream initialized: {} Hz \u{00d7} {} ch ({:?}), {} bytes/frame, period {} hns",
        desired_sample_rate,
        channels,
        fmt_tag_label(fmt_tag),
        blockalign,
        min_period
    );

    Ok((
        audio_client,
        render_client,
        event_handle,
        blockalign,
        fmt_tag,
    ))
}

fn fmt_tag_label(fmt: Format) -> &'static str {
    match fmt {
        Format::F32 => "f32",
        Format::I32 => "i24-in-32",
        Format::I16 => "i16",
    }
}

fn classify_init_error(err: &WasapiError) -> ExclusiveInitFailure {
    match err {
        WasapiError::Windows(win_err) => {
            let code: i32 = win_err.code().0;
            if code == AUDCLNT_E_DEVICE_IN_USE {
                ExclusiveInitFailure::DeviceInUse
            } else if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
                ExclusiveInitFailure::ExclusiveDisabled
            } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT
                || code == AUDCLNT_E_INVALID_DEVICE_PERIOD
                || code == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED
            {
                ExclusiveInitFailure::Other(format!(
                    "format rejected (HRESULT 0x{:08x})",
                    code as u32
                ))
            } else {
                ExclusiveInitFailure::Other(format!(
                    "initialize_client failed: HRESULT 0x{:08x}: {win_err}",
                    code as u32
                ))
            }
        }
        WasapiError::UnsupportedFormat => {
            ExclusiveInitFailure::Other("UnsupportedFormat (rejected by wasapi-rs)".into())
        }
        other => ExclusiveInitFailure::Other(other.to_string()),
    }
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
