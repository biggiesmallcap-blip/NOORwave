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
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};

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
                "This device rejected every exclusive-mode format we tried (32-bit, packed \
                 24-bit, 24-in-32, 16-bit, f32). Pick a different output device."
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
    pub transport_format: String,
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
    I24In32,
    I24Packed,
    I16,
}

struct CandidateFormat {
    storebits: usize,
    validbits: usize,
    sample_type: SampleType,
    format: Format,
}

fn exclusive_candidate_formats() -> &'static [CandidateFormat] {
    &[
        CandidateFormat {
            storebits: 32,
            validbits: 32,
            sample_type: SampleType::Int,
            format: Format::I32,
        },
        CandidateFormat {
            storebits: 24,
            validbits: 24,
            sample_type: SampleType::Int,
            format: Format::I24Packed,
        },
        CandidateFormat {
            storebits: 32,
            validbits: 24,
            sample_type: SampleType::Int,
            format: Format::I24In32,
        },
        CandidateFormat {
            storebits: 16,
            validbits: 16,
            sample_type: SampleType::Int,
            format: Format::I16,
        },
        CandidateFormat {
            storebits: 32,
            validbits: 32,
            sample_type: SampleType::Float,
            format: Format::F32,
        },
    ]
}

struct MmcssGuard {
    handle: HANDLE,
    task_name: &'static str,
}

impl Drop for MmcssGuard {
    fn drop(&mut self) {
        unsafe {
            if AvRevertMmThreadCharacteristics(self.handle) == 0 {
                warn!(
                    target: "playback",
                    "WASAPI render thread failed to revert MMCSS task {}",
                    self.task_name
                );
            }
        }
    }
}

fn enter_mmcss() -> Option<MmcssGuard> {
    for task_name in ["Pro Audio", "Audio"] {
        let mut wide: Vec<u16> = task_name.encode_utf16().collect();
        wide.push(0);
        let mut task_index = 0_u32;
        let handle = unsafe { AvSetMmThreadCharacteristicsW(wide.as_ptr(), &mut task_index) };
        if !handle.is_null() {
            info!(
                target: "playback",
                "WASAPI render thread entered MMCSS task {}",
                task_name
            );
            return Some(MmcssGuard { handle, task_name });
        }
    }
    warn!(
        target: "playback",
        "WASAPI render thread failed to enter MMCSS Pro Audio/Audio tasks"
    );
    None
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
        mpsc::sync_channel::<std::result::Result<(u32, u16, String), ExclusiveInitFailure>>(1);

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
        Ok((effective_rate, effective_channels, transport_format)) => Ok(ExclusiveStream {
            shutdown,
            released,
            thread: Some(thread),
            effective_sample_rate: effective_rate,
            effective_channels,
            transport_format,
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
    init_tx: mpsc::SyncSender<std::result::Result<(u32, u16, String), ExclusiveInitFailure>>,
) {
    let _mmcss = enter_mmcss();
    let init_result = init_audio_client(device_pref.as_deref(), desired_sample_rate, channels);
    let (audio_client, render_client, event_handle, blockalign, fmt_tag) = match init_result {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            released.store(true, Ordering::Release);
            return;
        }
    };

    let mut f32_scratch: Vec<f32> = Vec::new();
    let mut byte_buf: Vec<u8> = Vec::new();
    match write_available_wasapi_frames(
        &audio_client,
        &render_client,
        channels,
        blockalign,
        fmt_tag,
        &shared,
        &command_tx,
        &event_tx,
        &mut f32_scratch,
        &mut byte_buf,
    ) {
        Ok(Some(report)) => {
            info!(
                target: "playback",
                "WASAPI exclusive primed first buffer: {} frames, nonzero_audio={}",
                report.frames,
                report.nonzero_audio
            );
        }
        Ok(None) => {
            warn!(target: "playback", "WASAPI exclusive first-buffer prime found no writable frames");
        }
        Err(e) => {
            let _ = init_tx.send(Err(ExclusiveInitFailure::Other(format!(
                "prime initial WASAPI buffer: {e}"
            ))));
            released.store(true, Ordering::Release);
            return;
        }
    }

    if let Err(e) = audio_client.start_stream() {
        warn!("WASAPI start_stream failed: {e}");
        let _ = init_tx.send(Err(ExclusiveInitFailure::Other(format!(
            "start WASAPI stream: {e}"
        ))));
        released.store(true, Ordering::Release);
        let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeReleased {
            device_name: device_label.clone(),
        });
        return;
    }
    let _ = init_tx.send(Ok((
        desired_sample_rate,
        channels,
        fmt_tag_label(fmt_tag).to_string(),
    )));

    let grace = Duration::from_secs(grace_secs.max(1) as u64);
    let mut paused_since: Option<Instant> = None;
    let mut logged_post_start_fill = false;
    let mut logged_first_nonzero_fill = false;

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

        match write_available_wasapi_frames(
            &audio_client,
            &render_client,
            channels,
            blockalign,
            fmt_tag,
            &shared,
            &command_tx,
            &event_tx,
            &mut f32_scratch,
            &mut byte_buf,
        ) {
            Ok(Some(report)) => {
                if !logged_post_start_fill {
                    info!(
                        target: "playback",
                        "WASAPI exclusive post-start fill: {} frames, nonzero_audio={}",
                        report.frames,
                        report.nonzero_audio
                    );
                    logged_post_start_fill = true;
                }
                if report.nonzero_audio && !logged_first_nonzero_fill {
                    info!(
                        target: "playback",
                        "WASAPI exclusive first nonzero audio fill: {} frames",
                        report.frames
                    );
                    logged_first_nonzero_fill = true;
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!("WASAPI write_to_device failed: {e}");
                break;
            }
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

struct RenderWriteReport {
    frames: usize,
    nonzero_audio: bool,
}

#[allow(clippy::too_many_arguments)]
fn write_available_wasapi_frames(
    audio_client: &wasapi::AudioClient,
    render_client: &wasapi::AudioRenderClient,
    channels: u16,
    blockalign: usize,
    fmt_tag: Format,
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    f32_scratch: &mut Vec<f32>,
    byte_buf: &mut Vec<u8>,
) -> std::result::Result<Option<RenderWriteReport>, String> {
    let frames = audio_client
        .get_available_space_in_frames()
        .map_err(|e| format!("get_available_space: {e}"))? as usize;

    if frames == 0 {
        return Ok(None);
    }

    let interleaved = frames * channels as usize;
    f32_scratch.resize(interleaved, 0.0);
    f32_scratch.fill(0.0);

    fill_f32_from_shared(f32_scratch, shared, command_tx, event_tx);
    let nonzero_audio = f32_scratch.iter().any(|sample| sample.abs() > f32::EPSILON);

    let bytes_needed = frames * blockalign;
    byte_buf.resize(bytes_needed, 0);
    convert_f32_to_bytes(
        f32_scratch.as_slice(),
        byte_buf.as_mut_slice(),
        fmt_tag,
        blockalign,
        channels as usize,
    );

    render_client
        .write_to_device(frames, byte_buf.as_slice(), None)
        .map_err(|e| format!("write_to_device: {e}"))?;

    Ok(Some(RenderWriteReport {
        frames,
        nonzero_audio,
    }))
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

    let mut last_failure: Option<ExclusiveInitFailure> = None;

    // Backoff schedule for AUDCLNT_E_DEVICE_IN_USE retries. Some drivers /
    // Windows audio engine versions don't release the shared session within
    // the first tens of ms; ~1.3 s total budget covers cases where a flat
    // 150 ms didn't (observed empirically with Chrome holding shared mode).
    // Each entry is the sleep BEFORE that attempt's retry — so 4 attempts:
    // initial try, then sleep 50 ms, retry, sleep 150 ms, retry, sleep 350
    // ms, retry, sleep 750 ms, give up.
    const DEVICE_IN_USE_BACKOFF_MS: &[u64] = &[50, 150, 350, 750];

    'candidate: for candidate in exclusive_candidate_formats() {
        let mut attempt: usize = 0;
        loop {
            match try_initialize_one(
                device_pref,
                candidate.storebits,
                candidate.validbits,
                &candidate.sample_type,
                desired_sample_rate,
                channels,
                candidate.format,
            ) {
                Ok(v) => {
                    if attempt > 0 {
                        info!(
                            target: "playback",
                            "WASAPI exclusive grab succeeded on attempt {} ({:?})",
                            attempt + 1,
                            fmt_tag_label(candidate.format)
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
                            fmt_tag_label(candidate.format),
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
                            fmt_tag_label(candidate.format)
                        );
                        last_failure = Some(ExclusiveInitFailure::DeviceInUse);
                        continue 'candidate;
                    }
                    other => {
                        info!(
                            target: "playback",
                            "WASAPI exclusive grab attempt {} ({:?}) rejected: {}",
                            attempt + 1,
                            fmt_tag_label(candidate.format),
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

    let (def_period, min_period) = audio_client
        .get_device_period()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_device_period: {e}")))?;
    let mut chosen_period = audio_client
        .calculate_aligned_period_near(3 * min_period / 2, Some(128), &format)
        .map_err(|e| ExclusiveInitFailure::Other(format!("calculate aligned period: {e}")))?;
    let mode = StreamMode::EventsExclusive {
        period_hns: chosen_period,
    };

    if let Err(e) = audio_client.initialize_client(&format, &Direction::Render, &mode) {
        let classification = classify_init_error_kind(&e);
        // Always log the raw HRESULT so we have ground-truth diagnostic data
        // when classification falls through to "Other" (i.e. something we
        // didn't recognize and aren't retrying).
        if let WasapiError::Windows(win_err) = &e {
            tracing::debug!(
                target: "playback",
                "WASAPI initialize_client returned HRESULT 0x{:08x} ({:?}, {} Hz \u{00d7} {} ch, period {} hns) \u{2192} {:?}",
                win_err.code().0 as u32,
                fmt_tag_label(fmt_tag),
                desired_sample_rate,
                channels,
                chosen_period,
                classification
            );
        }
        if classification == InitErrorClassification::RepairAlignedPeriod {
            let aligned_frames = audio_client.get_buffer_size().map_err(|err| {
                ExclusiveInitFailure::Other(format!("get aligned buffer size: {err}"))
            })?;
            let aligned_period =
                wasapi::calculate_period_100ns(aligned_frames as i64, desired_sample_rate as i64);
            info!(
                target: "playback",
                "WASAPI exclusive retrying aligned buffer: {} frames, period {} hns",
                aligned_frames,
                aligned_period
            );
            let mut retry_client = device.get_iaudioclient().map_err(|err| {
                ExclusiveInitFailure::Other(format!("get_iaudioclient retry: {err}"))
            })?;
            let retry_mode = StreamMode::EventsExclusive {
                period_hns: aligned_period,
            };
            if let Err(retry_err) =
                retry_client.initialize_client(&format, &Direction::Render, &retry_mode)
            {
                return Err(classify_init_error(&retry_err));
            }
            audio_client = retry_client;
            chosen_period = aligned_period;
        } else {
            return Err(classify_init_error(&e));
        }
    }

    let buffer_frames = audio_client.get_buffer_size().unwrap_or(0);
    let event_handle = audio_client
        .set_get_eventhandle()
        .map_err(|e| ExclusiveInitFailure::Other(format!("set_get_eventhandle: {e}")))?;

    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_audiorenderclient: {e}")))?;

    info!(
        target: "playback",
        "WASAPI exclusive stream initialized: {} Hz \u{00d7} {} ch ({:?}), {} bytes/frame, default period {} hns, min period {} hns, chosen period {} hns, buffer {} frames",
        desired_sample_rate,
        channels,
        fmt_tag_label(fmt_tag),
        blockalign,
        def_period,
        min_period,
        chosen_period,
        buffer_frames
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
        Format::I32 => "i32",
        Format::I24In32 => "i24-in-32",
        Format::I24Packed => "i24-packed",
        Format::I16 => "i16",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitErrorClassification {
    DeviceInUse,
    ExclusiveDisabled,
    RepairAlignedPeriod,
    FormatRejected,
    Other,
}

fn classify_init_hresult(code: i32) -> InitErrorClassification {
    if code == AUDCLNT_E_DEVICE_IN_USE {
        InitErrorClassification::DeviceInUse
    } else if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
        InitErrorClassification::ExclusiveDisabled
    } else if code == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
        InitErrorClassification::RepairAlignedPeriod
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT || code == AUDCLNT_E_INVALID_DEVICE_PERIOD {
        InitErrorClassification::FormatRejected
    } else {
        InitErrorClassification::Other
    }
}

fn classify_init_error_kind(err: &WasapiError) -> InitErrorClassification {
    match err {
        WasapiError::Windows(win_err) => classify_init_hresult(win_err.code().0),
        WasapiError::UnsupportedFormat => InitErrorClassification::FormatRejected,
        _ => InitErrorClassification::Other,
    }
}

fn classify_init_error(err: &WasapiError) -> ExclusiveInitFailure {
    match err {
        WasapiError::Windows(win_err) => {
            let code: i32 = win_err.code().0;
            match classify_init_hresult(code) {
                InitErrorClassification::DeviceInUse => ExclusiveInitFailure::DeviceInUse,
                InitErrorClassification::ExclusiveDisabled => {
                    ExclusiveInitFailure::ExclusiveDisabled
                }
                InitErrorClassification::FormatRejected
                | InitErrorClassification::RepairAlignedPeriod => ExclusiveInitFailure::Other(
                    format!("format rejected (HRESULT 0x{:08x})", code as u32),
                ),
                InitErrorClassification::Other => ExclusiveInitFailure::Other(format!(
                    "initialize_client failed: HRESULT 0x{:08x}: {win_err}",
                    code as u32
                )),
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
            debug_assert_eq!(blockalign, channels * 4);
            for (chunk, &s) in dst.chunks_exact_mut(4).zip(src.iter()) {
                let v = f32_to_i32_pcm(s);
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
        Format::I24In32 => {
            // 24-in-32: WAVEFORMATEXTENSIBLE with 32 storebits + 24 validbits
            // means the valid PCM payload is left-aligned, leaving the low
            // byte clear.
            debug_assert_eq!(blockalign, channels * 4);
            for (chunk, &s) in dst.chunks_exact_mut(4).zip(src.iter()) {
                let v = f32_to_i24_pcm(s) << 8;
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
        Format::I24Packed => {
            debug_assert_eq!(blockalign, channels * 3);
            for (chunk, &s) in dst.chunks_exact_mut(3).zip(src.iter()) {
                let v = f32_to_i24_pcm(s);
                chunk.copy_from_slice(&v.to_le_bytes()[0..3]);
            }
        }
        Format::I16 => {
            debug_assert_eq!(blockalign, channels * 2);
            for (chunk, &s) in dst.chunks_exact_mut(2).zip(src.iter()) {
                let v = f32_to_i16_pcm(s);
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

fn f32_to_i16_pcm(sample: f32) -> i16 {
    if sample <= -1.0 {
        i16::MIN
    } else if sample >= 1.0 {
        i16::MAX
    } else {
        (sample * i16::MAX as f32) as i16
    }
}

fn f32_to_i24_pcm(sample: f32) -> i32 {
    if sample <= -1.0 {
        -8_388_608
    } else if sample >= 1.0 {
        8_388_607
    } else {
        (sample * 8_388_607.0) as i32
    }
}

fn f32_to_i32_pcm(sample: f32) -> i32 {
    if sample <= -1.0 {
        i32::MIN
    } else if sample >= 1.0 {
        i32::MAX
    } else {
        (sample * i32::MAX as f32) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_init_hresult_maps_device_in_use() {
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_DEVICE_IN_USE),
            InitErrorClassification::DeviceInUse
        );
    }

    #[test]
    fn classify_init_hresult_maps_exclusive_disabled() {
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED),
            InitErrorClassification::ExclusiveDisabled
        );
    }

    #[test]
    fn classify_init_hresult_marks_buffer_alignment_as_repairable() {
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED),
            InitErrorClassification::RepairAlignedPeriod
        );
    }

    #[test]
    fn classify_init_hresult_marks_invalid_period_as_format_rejection() {
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_INVALID_DEVICE_PERIOD),
            InitErrorClassification::FormatRejected
        );
    }

    #[test]
    fn convert_f32_to_bytes_writes_f32_little_endian() {
        let mut dst = vec![0_u8; 8];

        convert_f32_to_bytes(&[0.5, -0.5], &mut dst, Format::F32, 8, 2);

        let mut expected = Vec::new();
        expected.extend_from_slice(&0.5_f32.to_le_bytes());
        expected.extend_from_slice(&(-0.5_f32).to_le_bytes());
        assert_eq!(dst, expected);
    }

    #[test]
    fn convert_f32_to_bytes_clamps_integer_formats() {
        let mut i16_bytes = vec![0_u8; 4];
        convert_f32_to_bytes(&[2.0, -2.0], &mut i16_bytes, Format::I16, 4, 2);
        assert_eq!(&i16_bytes[0..2], &i16::MAX.to_le_bytes());
        assert_eq!(&i16_bytes[2..4], &i16::MIN.to_le_bytes());

        let mut i32_bytes = vec![0_u8; 8];
        convert_f32_to_bytes(&[2.0, -2.0], &mut i32_bytes, Format::I32, 8, 2);
        assert_eq!(&i32_bytes[0..4], &i32::MAX.to_le_bytes());
        assert_eq!(&i32_bytes[4..8], &i32::MIN.to_le_bytes());
    }

    #[test]
    fn convert_f32_to_bytes_writes_packed_i24_little_endian() {
        let mut dst = vec![0_u8; 9];

        convert_f32_to_bytes(&[1.0, -1.0, 0.0], &mut dst, Format::I24Packed, 3, 1);

        assert_eq!(
            dst,
            vec![0xff, 0xff, 0x7f, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn convert_f32_to_bytes_writes_i24_in_32_left_aligned() {
        let mut dst = vec![0_u8; 12];

        convert_f32_to_bytes(&[1.0, -1.0, 0.5], &mut dst, Format::I24In32, 4, 1);

        assert_eq!(
            dst,
            vec![
                0x00, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x00, 0x80, 0x00, 0xff, 0xff, 0x3f
            ]
        );
    }

    #[test]
    fn candidate_formats_prefer_integer_formats_before_float() {
        let labels: Vec<_> = exclusive_candidate_formats()
            .iter()
            .map(|candidate| fmt_tag_label(candidate.format))
            .collect();

        assert_eq!(labels, vec!["i32", "i24-packed", "i24-in-32", "i16", "f32"]);
    }

    #[test]
    fn format_labels_match_candidate_formats() {
        assert_eq!(fmt_tag_label(Format::F32), "f32");
        assert_eq!(fmt_tag_label(Format::I32), "i32");
        assert_eq!(fmt_tag_label(Format::I24In32), "i24-in-32");
        assert_eq!(fmt_tag_label(Format::I24Packed), "i24-packed");
        assert_eq!(fmt_tag_label(Format::I16), "i16");
    }
}
