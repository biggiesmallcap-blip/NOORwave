//! Real WASAPI exclusive-mode output stream.
//!
//! cpal only exposes shared-mode output, which always routes through the
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

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, RwLock, RwLockReadGuard, TryLockError};
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
use crate::db::audio_settings::ExclusiveLatencyMode;

// AUDCLNT_E_* HRESULTs from <audioclient.h>. Raw i32 reps so we don't have to
// pull windows / windows-core into noor-server's direct deps just to match a
// handful of error codes. wasapi-rs's WasapiError::Windows wraps a
// windows_core::Error whose .code() returns a windows_core::HRESULT; that
// type's wire form is i32, accessible via `.0`.
const AUDCLNT_E_UNSUPPORTED_FORMAT: i32 = 0x8889_0008u32 as i32;
const AUDCLNT_E_DEVICE_IN_USE: i32 = 0x8889_000Au32 as i32;
const AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED: i32 = 0x8889_000Eu32 as i32;
const AUDCLNT_E_ENDPOINT_CREATE_FAILED: i32 = 0x8889_000Fu32 as i32;
const AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED: i32 = 0x8889_0019u32 as i32;
#[cfg(test)]
const AUDCLNT_E_INVALID_DEVICE_PERIOD: i32 = 0x8889_0020u32 as i32;
const DEVICE_IN_USE_BACKOFF_MS: &[u64] = &[50, 150, 350, 750];
const DEVICE_EVENT_TIMEOUT: Duration = Duration::from_secs(2);
const DEVICE_EVENT_CANCEL_POLL_MS: u32 = 10;

#[derive(Debug, PartialEq, Eq)]
enum DeviceEventWait {
    Ready,
    Cancelled,
    TimedOut,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ExclusiveRenderFault {
    EventTimeout = 1,
    EventWaitFailed = 2,
    DeviceWriteFailed = 3,
    RendererPanicked = 4,
}

impl ExclusiveRenderFault {
    pub(crate) fn user_message(self) -> &'static str {
        match self {
            Self::EventTimeout => {
                "WASAPI output stopped signaling audio events. Reconnect the device and resume playback."
            }
            Self::EventWaitFailed => {
                "WASAPI could not wait for the audio device. Reconnect the device and resume playback."
            }
            Self::DeviceWriteFailed => {
                "WASAPI could not write audio to the device. Reconnect the device and resume playback."
            }
            Self::RendererPanicked => {
                "WASAPI output stopped unexpectedly. Resume playback to reopen the device."
            }
        }
    }
}

fn wait_for_device_event(
    shutdown: &AtomicBool,
    force_release: &AtomicBool,
    timeout: Duration,
    mut wait: impl FnMut(u32) -> Result<(), WasapiError>,
) -> DeviceEventWait {
    let started = Instant::now();
    loop {
        if shutdown.load(Ordering::SeqCst) || force_release.load(Ordering::Acquire) {
            return DeviceEventWait::Cancelled;
        }
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return DeviceEventWait::TimedOut;
        };
        if remaining.is_zero() {
            return DeviceEventWait::TimedOut;
        }
        let wait_ms = remaining
            .as_millis()
            .clamp(1, u128::from(DEVICE_EVENT_CANCEL_POLL_MS)) as u32;
        let slice_started = Instant::now();
        let result = wait(wait_ms);
        if shutdown.load(Ordering::SeqCst) || force_release.load(Ordering::Acquire) {
            return DeviceEventWait::Cancelled;
        }
        match result {
            Ok(()) => return DeviceEventWait::Ready,
            // wasapi 0.25 maps every non-signaled Win32 result, including
            // WAIT_FAILED, to EventTimeout. Preserve the total device-loss
            // deadline instead of treating one cancellation slice as failure.
            Err(WasapiError::EventTimeout) => {
                // A failed Win32 wait can return immediately with this same
                // error. Pace its unused slice without guessing the cause.
                let unused_slice = Duration::from_millis(u64::from(wait_ms))
                    .saturating_sub(slice_started.elapsed())
                    .min(timeout.saturating_sub(started.elapsed()));
                if !unused_slice.is_zero() {
                    thread::sleep(unused_slice);
                }
            }
            Err(_) => return DeviceEventWait::Failed,
        }
    }
}

struct DeviceInUseRetryBudget {
    index: usize,
}

impl DeviceInUseRetryBudget {
    fn new() -> Self {
        Self { index: 0 }
    }

    fn next_delay_ms(&mut self) -> Option<u64> {
        let delay = DEVICE_IN_USE_BACKOFF_MS.get(self.index).copied();
        if delay.is_some() {
            self.index += 1;
        }
        delay
    }
}

struct ComApartmentGuard(bool);

impl ComApartmentGuard {
    fn initialize() -> std::result::Result<Self, ExclusiveInitFailure> {
        let hr = initialize_mta();
        if hr.is_ok() {
            Ok(Self(true))
        } else if hr.0 as u32 == 0x80010106 {
            // An existing apartment owns COM; this call must not uninitialize it.
            Ok(Self(false))
        } else {
            Err(ExclusiveInitFailure::Other(format!(
                "MTA init failed (HRESULT 0x{:08x})",
                hr.0 as u32
            )))
        }
    }
}

impl Drop for ComApartmentGuard {
    fn drop(&mut self) {
        if self.0 {
            wasapi::deinitialize();
        }
    }
}

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
    /// The driver could not create an endpoint, even with a valid PCM format.
    EndpointCreationFailed,
    /// A candidate PCM representation was rejected; later formats may work.
    FormatRejected(String),
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
                "This device rejected every exclusive-mode format at the requested sample \
                 rate and channel count. Try shared output or a supported source rate."
                    .to_string()
            }
            Self::EndpointCreationFailed => {
                "Windows could not open this audio endpoint at the requested format \
                 (HRESULT 0x8889000f). Try shared output, or reconnect the device and retry."
                    .to_string()
            }
            Self::FormatRejected(s) => format!("Exclusive mode failed: {s}"),
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
    force_release: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    // Keep the final source ownership on the control thread through join.
    source_bank: Arc<ExclusiveRenderSourceBank>,
    fault: Arc<AtomicU8>,
    output_live: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub effective_sample_rate: u32,
    #[allow(dead_code)]
    pub effective_channels: u16,
    pub transport_format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ExclusiveRenderRole {
    Active,
    Prepared,
    Fading,
}

#[derive(Clone)]
pub(crate) struct ExclusiveRenderSource {
    // Only inspected by the source publication tests; kept so they can assert
    // which slot is active vs prepared.
    #[cfg_attr(not(test), allow(dead_code))]
    pub role: ExclusiveRenderRole,
    pub shared: Arc<PlaybackSharedState>,
}

#[derive(Default)]
pub(crate) struct ExclusiveRenderSourceBank {
    sources: RwLock<Vec<ExclusiveRenderSource>>,
    contended_periods: AtomicU64,
    poisoned_periods: AtomicU64,
}

impl ExclusiveRenderSourceBank {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_sources(&self, sources: Vec<ExclusiveRenderSource>) {
        let mut guard = match self.sources.write() {
            Ok(guard) => guard,
            Err(error) => {
                warn!(target: "playback", "WASAPI source publication recovered a poisoned bank");
                self.sources.clear_poison();
                error.into_inner()
            }
        };
        // The writer replaces and reclaims the old vector; readers never own
        // a source snapshot that could become its last owner during render.
        let retired = std::mem::replace(&mut *guard, sources);
        drop(guard);
        drop(retired);
    }

    pub fn clear(&self) {
        self.set_sources(Vec::new());
    }

    fn try_borrow(&self) -> Option<RwLockReadGuard<'_, Vec<ExclusiveRenderSource>>> {
        match self.sources.try_read() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => {
                self.contended_periods.fetch_add(1, Ordering::Relaxed);
                None
            }
            Err(TryLockError::Poisoned(_)) => {
                self.poisoned_periods.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
}

impl ExclusiveStream {
    #[cfg(test)]
    pub(crate) fn test_live_at_rate(rate: u32) -> Self {
        let mut stream = Self::test_released_with_fault(ExclusiveRenderFault::EventTimeout);
        stream.released.store(false, Ordering::Relaxed);
        stream.fault.store(0, Ordering::Relaxed);
        stream.effective_sample_rate = rate;
        stream
    }

    #[cfg(test)]
    pub(crate) fn test_with_teardown_check(check: impl FnOnce() + Send + 'static) -> Self {
        let mut stream = Self::test_live_at_rate(48_000);
        let shutdown = Arc::clone(&stream.shutdown);
        stream.thread = Some(thread::spawn(move || {
            while !shutdown.load(Ordering::Acquire) {
                thread::yield_now();
            }
            check();
        }));
        stream
    }

    #[cfg(test)]
    pub(crate) fn test_released_with_fault(fault: ExclusiveRenderFault) -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            released: Arc::new(AtomicBool::new(true)),
            force_release: Arc::new(AtomicBool::new(false)),
            thread: None,
            source_bank: Arc::new(ExclusiveRenderSourceBank::new()),
            fault: Arc::new(AtomicU8::new(fault as u8)),
            output_live: Arc::new(AtomicBool::new(true)),
            effective_sample_rate: 48_000,
            effective_channels: 2,
            transport_format: "test".into(),
        }
    }

    pub(crate) fn take_fault(&self) -> Option<ExclusiveRenderFault> {
        match self.fault.swap(0, Ordering::AcqRel) {
            1 => Some(ExclusiveRenderFault::EventTimeout),
            2 => Some(ExclusiveRenderFault::EventWaitFailed),
            3 => Some(ExclusiveRenderFault::DeviceWriteFailed),
            4 => Some(ExclusiveRenderFault::RendererPanicked),
            _ => None,
        }
    }

    pub(crate) fn output_live(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.output_live)
    }

    /// `true` once the render thread has self-shut due to idle (paused for
    /// longer than `grace_secs`) or because the device went away. The runtime
    /// uses this to know it must rebuild the stream before resuming.
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    /// Ask the render thread to drop the device now instead of waiting out the
    /// idle grace window. Used when another app (e.g. the WebView playing a
    /// TIDAL video) needs the endpoint in shared mode. The thread exits its
    /// loop on the next tick, releases the `IAudioClient`, and flips
    /// `is_released()`; the runtime re-grabs on the next Resume/Play.
    pub fn request_release(&self) {
        self.force_release.store(true, Ordering::Release);
    }
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.output_live.store(false, Ordering::Release);
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let contended_periods = self.source_bank.contended_periods.load(Ordering::Relaxed);
        let poisoned_periods = self.source_bank.poisoned_periods.load(Ordering::Relaxed);
        if contended_periods != 0 || poisoned_periods != 0 {
            warn!(target: "playback", contended_periods, poisoned_periods,
                "WASAPI source bank rendered silence while unavailable");
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
            validbits: 24,
            sample_type: SampleType::Int,
            format: Format::I24In32,
        },
        CandidateFormat {
            storebits: 24,
            validbits: 24,
            sample_type: SampleType::Int,
            format: Format::I24Packed,
        },
        CandidateFormat {
            storebits: 32,
            validbits: 32,
            sample_type: SampleType::Int,
            format: Format::I32,
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
/// An explicit preference is a Windows endpoint ID, not a display name.
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
    latency_mode: ExclusiveLatencyMode,
    source_bank: Arc<ExclusiveRenderSourceBank>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    output_identity: Arc<()>,
) -> std::result::Result<ExclusiveStream, ExclusiveInitFailure> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let released = Arc::new(AtomicBool::new(false));
    let released_clone = Arc::clone(&released);
    let force_release = Arc::new(AtomicBool::new(false));
    let force_release_clone = Arc::clone(&force_release);
    let device_pref_owned = device_pref.map(|s| s.to_string());
    let render_source_bank = Arc::clone(&source_bank);
    let fault = Arc::new(AtomicU8::new(0));
    let render_fault = Arc::clone(&fault);
    let output_live = Arc::new(AtomicBool::new(true));

    let (init_tx, init_rx) =
        mpsc::sync_channel::<std::result::Result<(u32, u16, String), ExclusiveInitFailure>>(1);

    let thread = thread::Builder::new()
        .name("noor-wasapi-exclusive".into())
        .spawn(move || {
            run_render_thread_guarded(&released_clone, &render_fault, || {
                run_render_thread(
                    device_pref_owned,
                    device_label,
                    desired_sample_rate,
                    channels,
                    grace_secs,
                    latency_mode,
                    render_source_bank,
                    command_tx,
                    event_tx,
                    output_identity,
                    shutdown_clone,
                    Arc::clone(&released_clone),
                    force_release_clone,
                    Arc::clone(&render_fault),
                    init_tx,
                )
            });
        })
        .map_err(|e| ExclusiveInitFailure::Other(format!("spawn render thread: {e}")))?;

    let outcome = match init_rx.recv() {
        Ok(outcome) => outcome,
        Err(_) => {
            // Keep control-side source ownership until a failed initializer
            // has unwound, just as a live stream does during Drop.
            shutdown.store(true, Ordering::SeqCst);
            let _ = thread.join();
            return Err(ExclusiveInitFailure::Other(
                "WASAPI render thread died before reporting init result".into(),
            ));
        }
    };

    match outcome {
        Ok((effective_rate, effective_channels, transport_format)) => Ok(ExclusiveStream {
            shutdown,
            released,
            force_release,
            thread: Some(thread),
            source_bank,
            fault,
            output_live,
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

struct RenderThreadExitGuard<'a> {
    released: &'a AtomicBool,
    fault: &'a AtomicU8,
}

impl Drop for RenderThreadExitGuard<'_> {
    fn drop(&mut self) {
        if thread::panicking() && !self.released.load(Ordering::Acquire) {
            let _ = self.fault.compare_exchange(
                0,
                ExclusiveRenderFault::RendererPanicked as u8,
                Ordering::Release,
                Ordering::Relaxed,
            );
            self.released.store(true, Ordering::Release);
        }
    }
}

fn run_render_thread_guarded(released: &AtomicBool, fault: &AtomicU8, run: impl FnOnce()) {
    // Declared outside the renderer body so its COM/resource locals unwind
    // before the runtime can observe release and attempt another acquisition.
    let _exit = RenderThreadExitGuard { released, fault };
    run();
}

#[allow(clippy::too_many_arguments)]
fn run_render_thread(
    device_pref: Option<String>,
    device_label: String,
    desired_sample_rate: u32,
    channels: u16,
    grace_secs: u32,
    latency_mode: ExclusiveLatencyMode,
    source_bank: Arc<ExclusiveRenderSourceBank>,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    output_identity: Arc<()>,
    shutdown: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
    force_release: Arc<AtomicBool>,
    fault: Arc<AtomicU8>,
    init_tx: mpsc::SyncSender<std::result::Result<(u32, u16, String), ExclusiveInitFailure>>,
) {
    let _com = match ComApartmentGuard::initialize() {
        Ok(guard) => guard,
        Err(error) => {
            let _ = init_tx.send(Err(error));
            released.store(true, Ordering::Release);
            return;
        }
    };
    let _mmcss = enter_mmcss();
    let init_result = init_audio_client(
        device_pref.as_deref(),
        desired_sample_rate,
        channels,
        &latency_mode,
    );
    let (audio_client, render_client, event_handle, blockalign, fmt_tag) = match init_result {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            released.store(true, Ordering::Release);
            return;
        }
    };

    let buffer_frames = match audio_client.get_buffer_size() {
        Ok(frames) => frames as usize,
        Err(e) => {
            let _ = init_tx.send(Err(ExclusiveInitFailure::Other(format!(
                "get WASAPI render buffer size: {e}"
            ))));
            drop(render_client);
            drop(audio_client);
            drop(event_handle);
            released.store(true, Ordering::Release);
            return;
        }
    };
    let mut f32_scratch = vec![0.0; buffer_frames * channels as usize];
    let mut mix_scratch = vec![0.0; buffer_frames * channels as usize];
    let mut byte_buf = vec![0; buffer_frames * blockalign];
    match write_available_wasapi_frames(
        &audio_client,
        &render_client,
        channels,
        blockalign,
        fmt_tag,
        &source_bank,
        &command_tx,
        &event_tx,
        &mut f32_scratch,
        &mut mix_scratch,
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
            drop(render_client);
            drop(audio_client);
            drop(event_handle);
            released.store(true, Ordering::Release);
            return;
        }
    }

    let grace = Duration::from_secs(grace_secs.max(1) as u64);
    let init_report = Ok((
        desired_sample_rate,
        channels,
        fmt_tag_label(fmt_tag).to_string(),
    ));
    info!(
        target: "playback",
        device = %device_label,
        grace_secs = grace.as_secs(),
        "WASAPI exclusive render prepared; idle-release grace active"
    );
    if let Err(e) = audio_client.start_stream() {
        warn!("WASAPI start_stream failed: {e}");
        let _ = init_tx.send(Err(ExclusiveInitFailure::Other(format!(
            "start WASAPI stream: {e}"
        ))));
        drop(render_client);
        drop(audio_client);
        drop(event_handle);
        released.store(true, Ordering::Release);
        return;
    }
    let _ = init_tx.send(init_report);
    let mut paused_since: Option<Instant> = None;
    let mut first_fill_frames = None;
    let mut first_nonzero_frames = None;
    let mut failure = None;
    let mut write_failure = None;
    let mut release_reason = "shutdown";

    while !shutdown.load(Ordering::SeqCst) {
        // Forced release: a caller (e.g. video playback needing the endpoint in
        // shared mode) asked us to let go now, ahead of the idle grace. Same
        // teardown as idle release: break, drop the IAudioClient, flip
        // `is_released()`, re-grab on the next Resume/Play.
        if force_release.load(Ordering::Acquire) {
            release_reason = "external_request";
            break;
        }

        match wait_for_device_event(&shutdown, &force_release, DEVICE_EVENT_TIMEOUT, |ms| {
            event_handle.wait_for_event(ms)
        }) {
            DeviceEventWait::Ready => {}
            DeviceEventWait::Cancelled => {
                if force_release.load(Ordering::Acquire) {
                    release_reason = "external_request";
                }
                break;
            }
            DeviceEventWait::TimedOut => {
                failure = Some(ExclusiveRenderFault::EventTimeout);
                release_reason = "device_fault";
                break;
            }
            DeviceEventWait::Failed => {
                failure = Some(ExclusiveRenderFault::EventWaitFailed);
                release_reason = "device_fault";
                break;
            }
        }

        match write_available_wasapi_frames(
            &audio_client,
            &render_client,
            channels,
            blockalign,
            fmt_tag,
            &source_bank,
            &command_tx,
            &event_tx,
            &mut f32_scratch,
            &mut mix_scratch,
            &mut byte_buf,
        ) {
            Ok(Some(report)) => {
                if idle_release_due(
                    &mut paused_since,
                    report.sources_paused,
                    Instant::now(),
                    grace,
                ) {
                    release_reason = "idle_grace";
                    break;
                }
                first_fill_frames.get_or_insert(report.frames);
                if report.nonzero_audio {
                    first_nonzero_frames.get_or_insert(report.frames);
                }
            }
            Ok(None) => {}
            Err(e) => {
                failure = Some(ExclusiveRenderFault::DeviceWriteFailed);
                write_failure = Some(e);
                release_reason = "device_fault";
                break;
            }
        }
    }

    let _ = audio_client.stop_stream();
    drop(render_client);
    drop(audio_client);
    drop(event_handle);
    if let Some(failure) = failure {
        fault.store(failure as u8, Ordering::Release);
    }
    released.store(true, Ordering::Release);
    if let Some(error) = write_failure {
        warn!(target: "playback", "WASAPI render failure: {error}");
    }
    info!(target: "playback", release_reason, ?first_fill_frames, ?first_nonzero_frames,
        "WASAPI render stream stopped");
    // Tell the world we let go of the device. The runtime listens to this
    // (via the route layer's WS bridge) so the UI can clear "engaged" state
    // and the runtime can re-grab on the next Resume/Play. Skipped on a
    // user-initiated shutdown — the engine swap that's about to happen will
    // emit its own event.
    // The ordinary release listener can reacquire a playing endpoint. A
    // fault must instead pass through the identity-checked hard-pause path.
    if !shutdown.load(Ordering::SeqCst) && failure.is_none() {
        let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeReleased {
            output_identity,
            device_name: device_label.clone(),
        });
    }
}

struct RenderWriteReport {
    frames: usize,
    nonzero_audio: bool,
    sources_paused: Option<bool>,
}

enum RenderWriteFailure {
    AvailableSpace(WasapiError),
    Capacity,
    DeviceWrite(WasapiError),
}

impl std::fmt::Display for RenderWriteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AvailableSpace(error) => write!(f, "get_available_space: {error}"),
            Self::Capacity => f.write_str("WASAPI render buffer exceeds its initialized capacity"),
            Self::DeviceWrite(error) => write!(f, "write_to_device: {error}"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_available_wasapi_frames(
    audio_client: &wasapi::AudioClient,
    render_client: &wasapi::AudioRenderClient,
    channels: u16,
    blockalign: usize,
    fmt_tag: Format,
    source_bank: &ExclusiveRenderSourceBank,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    f32_scratch: &mut [f32],
    mix_scratch: &mut [f32],
    byte_buf: &mut [u8],
) -> std::result::Result<Option<RenderWriteReport>, RenderWriteFailure> {
    let frames = audio_client
        .get_available_space_in_frames()
        .map_err(RenderWriteFailure::AvailableSpace)? as usize;

    if frames == 0 {
        return Ok(None);
    }

    let interleaved = frames * channels as usize;
    let bytes_needed = frames * blockalign;
    if interleaved > f32_scratch.len()
        || interleaved > mix_scratch.len()
        || bytes_needed > byte_buf.len()
    {
        return Err(RenderWriteFailure::Capacity);
    }
    let f32_scratch = &mut f32_scratch[..interleaved];
    let mix_scratch = &mut mix_scratch[..interleaved];
    let byte_buf = &mut byte_buf[..bytes_needed];
    let sources_paused =
        mix_published_sources(source_bank, f32_scratch, mix_scratch, command_tx, event_tx);
    let nonzero_audio = f32_scratch.iter().any(|sample| sample.abs() > f32::EPSILON);

    convert_f32_to_bytes(
        f32_scratch,
        byte_buf,
        fmt_tag,
        blockalign,
        channels as usize,
    );

    render_client
        .write_to_device(frames, byte_buf, None)
        .map_err(RenderWriteFailure::DeviceWrite)?;

    Ok(Some(RenderWriteReport {
        frames,
        nonzero_audio,
        sources_paused,
    }))
}

fn mix_published_sources(
    source_bank: &ExclusiveRenderSourceBank,
    out: &mut [f32],
    mix_scratch: &mut [f32],
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Option<bool> {
    out.fill(0.0);
    let sources = source_bank.try_borrow()?;
    let paused = source_bank_all_paused(&sources);
    for source in sources.iter() {
        mix_scratch.fill(0.0);
        fill_f32_from_shared(mix_scratch, &source.shared, command_tx, event_tx);
        mix_f32_into(out, mix_scratch);
    }
    // The read guard drops here, before conversion and device transfer. Source
    // replacement can wait only for bounded, nonblocking PCM mixing.
    Some(paused)
}

fn idle_release_due(
    paused_since: &mut Option<Instant>,
    sources_paused: Option<bool>,
    now: Instant,
    grace: Duration,
) -> bool {
    if sources_paused != Some(true) {
        // Unavailable sources cannot establish a continuously paused interval.
        *paused_since = None;
        return false;
    }
    let start = paused_since.get_or_insert(now);
    now.duration_since(*start) >= grace
}

fn mix_f32_into(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = (*d + *s).clamp(-1.0, 1.0);
    }
}

fn source_bank_all_paused(sources: &[ExclusiveRenderSource]) -> bool {
    sources
        .iter()
        .all(|source| source.shared.paused.load(Ordering::Relaxed))
}

/// Try every candidate format in turn, attempting `initialize_client` directly
/// rather than relying on `IsFormatSupported` (which lies about exclusive-mode
/// support while the shared-mode engine has the device bound).
fn init_audio_client(
    device_pref: Option<&str>,
    desired_sample_rate: u32,
    channels: u16,
    latency_mode: &ExclusiveLatencyMode,
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
    initialize_candidates(
        |candidate| {
            try_initialize_one(
                device_pref,
                candidate.storebits,
                candidate.validbits,
                &candidate.sample_type,
                desired_sample_rate,
                channels,
                candidate.format,
                latency_mode,
            )
        },
        |delay_ms| thread::sleep(Duration::from_millis(delay_ms)),
    )
}

fn initialize_candidates<T>(
    mut initialize: impl FnMut(&CandidateFormat) -> std::result::Result<T, ExclusiveInitFailure>,
    mut wait: impl FnMut(u64),
) -> std::result::Result<T, ExclusiveInitFailure> {
    let mut last_failure: Option<ExclusiveInitFailure> = None;

    // Some drivers / Windows audio engine versions don't release the shared
    // session within the first tens of ms. Keep this budget global because
    // DEVICE_IN_USE is about device ownership, not the candidate sample format.
    let mut device_in_use_budget = DeviceInUseRetryBudget::new();

    'candidate: for candidate in exclusive_candidate_formats() {
        let mut attempt: usize = 0;
        loop {
            match initialize(candidate) {
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
                    ExclusiveInitFailure::ExclusiveDisabled => {
                        // No format will fix this. Bail immediately so the
                        // user gets a precise message.
                        warn!(
                            target: "playback",
                            "WASAPI exclusive grab: device has \"Allow exclusive control\" disabled"
                        );
                        return Err(failure);
                    }
                    ExclusiveInitFailure::DeviceInUse
                    | ExclusiveInitFailure::EndpointCreationFailed => {
                        if let Some(sleep_ms) = device_in_use_budget.next_delay_ms() {
                            info!(
                                target: "playback",
                                "WASAPI exclusive grab attempt {} hit {} ({:?}); \
                                 retrying after {} ms",
                                attempt + 1,
                                failure,
                                fmt_tag_label(candidate.format),
                                sleep_ms
                            );
                            wait(sleep_ms);
                            attempt += 1;
                            continue;
                        }
                        if matches!(failure, ExclusiveInitFailure::DeviceInUse) {
                            return Err(failure);
                        }
                        // Some drivers use endpoint-creation errors for a
                        // particular format. Still try the other representations,
                        // but never hide this error behind a later float rejection.
                        last_failure = Some(failure);
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
                        if !matches!(other, ExclusiveInitFailure::FormatRejected(_)) {
                            last_failure.get_or_insert(other);
                        }
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

fn select_probed_exclusive_format(
    requested: WaveFormat,
    probed: std::result::Result<WaveFormat, WasapiError>,
) -> WaveFormat {
    if let Ok(supported) = probed {
        let same_sample_type = matches!(
            (requested.get_subformat(), supported.get_subformat()),
            (Ok(requested_type), Ok(supported_type)) if requested_type == supported_type
        );
        if supported.get_samplespersec() == requested.get_samplespersec()
            && supported.get_nchannels() == requested.get_nchannels()
            && supported.get_bitspersample() == requested.get_bitspersample()
            && supported.get_validbitspersample() == requested.get_validbitspersample()
            && supported.get_blockalign() == requested.get_blockalign()
            && same_sample_type
        {
            return supported;
        }
    }
    // Initialize remains authoritative when a driver's support query rejects
    // the format; a probe must never alter source rate or PCM interpretation.
    requested
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
    latency_mode: &ExclusiveLatencyMode,
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
        Some(id) => enumerator.get_device(id).map_err(|e| {
            ExclusiveInitFailure::Other(format!("selected output device lookup: {e}"))
        })?,
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
    let probed = audio_client.is_supported_exclusive_with_quirks(&format);
    let format = select_probed_exclusive_format(format, probed);

    let blockalign = format.get_blockalign() as usize;

    let (def_period, min_period) = audio_client
        .get_device_period()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_device_period: {e}")))?;
    let target_period = target_exclusive_period_hns(def_period, min_period, latency_mode);
    let mut chosen_period = audio_client
        .calculate_aligned_period_near(target_period, Some(128), &format)
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
            // Windows requires releasing the failed client before activating
            // its replacement, even when Initialize only allocated a buffer.
            drop(audio_client);
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

    let buffer_frames = audio_client
        .get_buffer_size()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_buffer_size: {e}")))?;
    let event_handle = audio_client
        .set_get_eventhandle()
        .map_err(|e| ExclusiveInitFailure::Other(format!("set_get_eventhandle: {e}")))?;

    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|e| ExclusiveInitFailure::Other(format!("get_audiorenderclient: {e}")))?;

    info!(
        target: "playback",
        "WASAPI exclusive stream initialized: {} Hz \u{00d7} {} ch ({:?}), {} bytes/frame, latency {:?}, default period {} hns, min period {} hns, chosen period {} hns, buffer {} frames",
        desired_sample_rate,
        channels,
        fmt_tag_label(fmt_tag),
        blockalign,
        latency_mode,
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

fn target_exclusive_period_hns(
    default_period: i64,
    min_period: i64,
    latency_mode: &ExclusiveLatencyMode,
) -> i64 {
    match latency_mode {
        ExclusiveLatencyMode::Stable => default_period,
        ExclusiveLatencyMode::LowLatency => {
            min_period + default_period.saturating_sub(min_period) / 2
        }
        ExclusiveLatencyMode::UltraLowLatency => 3 * min_period / 2,
    }
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
    EndpointCreationFailed,
    RepairAlignedPeriod,
    FormatRejected,
    Other,
}

fn classify_init_hresult(code: i32) -> InitErrorClassification {
    if code == AUDCLNT_E_DEVICE_IN_USE {
        InitErrorClassification::DeviceInUse
    } else if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
        InitErrorClassification::ExclusiveDisabled
    } else if code == AUDCLNT_E_ENDPOINT_CREATE_FAILED {
        InitErrorClassification::EndpointCreationFailed
    } else if code == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
        InitErrorClassification::RepairAlignedPeriod
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT {
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
                InitErrorClassification::EndpointCreationFailed => {
                    ExclusiveInitFailure::EndpointCreationFailed
                }
                InitErrorClassification::FormatRejected => ExclusiveInitFailure::FormatRejected(
                    format!("format rejected (HRESULT 0x{:08x})", code as u32),
                ),
                InitErrorClassification::RepairAlignedPeriod | InitErrorClassification::Other => {
                    ExclusiveInitFailure::Other(format!(
                        "initialize_client failed: HRESULT 0x{:08x}: {win_err}",
                        code as u32
                    ))
                }
            }
        }
        WasapiError::UnsupportedFormat => {
            ExclusiveInitFailure::FormatRejected("UnsupportedFormat (rejected by wasapi-rs)".into())
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
            for (chunk, &s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.iter()) {
                *chunk = s.to_le_bytes();
            }
        }
        Format::I32 => {
            debug_assert_eq!(blockalign, channels * 4);
            for (chunk, &s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.iter()) {
                let v = f32_to_i32_pcm(s);
                *chunk = v.to_le_bytes();
            }
        }
        Format::I24In32 => {
            // 24-in-32: WAVEFORMATEXTENSIBLE with 32 storebits + 24 validbits
            // means the valid PCM payload is left-aligned, leaving the low
            // byte clear.
            debug_assert_eq!(blockalign, channels * 4);
            for (chunk, &s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.iter()) {
                let v = f32_to_i24_pcm(s) << 8;
                *chunk = v.to_le_bytes();
            }
        }
        Format::I24Packed => {
            debug_assert_eq!(blockalign, channels * 3);
            for (chunk, &s) in dst.as_chunks_mut::<3>().0.iter_mut().zip(src.iter()) {
                let v = f32_to_i24_pcm(s);
                chunk.copy_from_slice(&v.to_le_bytes()[0..3]);
            }
        }
        Format::I16 => {
            debug_assert_eq!(blockalign, channels * 2);
            for (chunk, &s) in dst.as_chunks_mut::<2>().0.iter_mut().zip(src.iter()) {
                let v = f32_to_i16_pcm(s);
                *chunk = v.to_le_bytes();
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
        (sample * 32_768.0).round() as i16
    }
}

fn f32_to_i24_pcm(sample: f32) -> i32 {
    if sample <= -1.0 {
        -8_388_608
    } else if sample >= 1.0 {
        8_388_607
    } else {
        (sample * 8_388_608.0)
            .round()
            .clamp(-8_388_608.0, 8_388_607.0) as i32
    }
}

fn f32_to_i32_pcm(sample: f32) -> i32 {
    if sample <= -1.0 {
        i32::MIN
    } else if sample >= 1.0 {
        i32::MAX
    } else {
        (sample * 2_147_483_648.0).round() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "briefly opens silent hardware output; set NOOR_AUDIO_SMOKE_DEVICE explicitly"]
    fn exclusive_output_silent_hardware_smoke() {
        use cpal::traits::{DeviceTrait, HostTrait};

        let selected_name = std::env::var("NOOR_AUDIO_SMOKE_DEVICE")
            .expect("set NOOR_AUDIO_SMOKE_DEVICE to the selected output name or endpoint ID");
        let host = cpal::default_host();
        let mut selected_devices = host
            .output_devices()
            .expect("enumerate output devices")
            .filter(|device| {
                device
                    .description()
                    .is_ok_and(|info| info.name() == selected_name)
                    || device
                        .id()
                        .is_ok_and(|id| id.to_string() == selected_name || id.id() == selected_name)
            });
        let device = selected_devices
            .next()
            .expect("selected test output must exist");
        assert!(
            selected_devices.next().is_none(),
            "test selection is ambiguous; use the exact endpoint ID"
        );
        let device_id = device.id().expect("selected endpoint ID").id().to_owned();
        let channels = device
            .default_output_config()
            .expect("shared config")
            .channels();
        let (command_tx, _) = mpsc::channel();
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(32);

        for (rate, idle_release) in [
            (44_100, false),
            (96_000, false),
            (44_100, true),
            (96_000, false),
        ] {
            let output_identity = Arc::new(());
            let stream = build_exclusive_stream(
                Some(&device_id),
                selected_name.clone(),
                rate,
                channels,
                1,
                ExclusiveLatencyMode::Stable,
                Arc::new(ExclusiveRenderSourceBank::new()),
                command_tx.clone(),
                event_tx.clone(),
                Arc::clone(&output_identity),
            )
            .unwrap_or_else(|error| panic!("silent exclusive open at {rate} Hz failed: {error}"));
            assert_eq!(stream.effective_sample_rate, rate);
            assert_eq!(stream.effective_channels, channels);
            assert!(!stream.is_released());
            if !idle_release {
                stream.request_release();
            }
            let deadline = Instant::now() + Duration::from_secs(4);
            while !stream.is_released() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(
                stream.is_released(),
                "silent output did not release at {rate} Hz"
            );
            assert!(
                stream.take_fault().is_none(),
                "normal release at {rate} Hz reported a device fault"
            );
            let notification_deadline = Instant::now() + Duration::from_secs(2);
            let notification = loop {
                match event_rx.try_recv() {
                    Ok(event) => break event,
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                        if Instant::now() < notification_deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("native release notification missing: {error}"),
                }
            };
            assert!(matches!(notification,
                PlaybackRuntimeEvent::ExclusiveModeReleased { output_identity: released_identity, .. }
                    if Arc::ptr_eq(&released_identity, &output_identity)));
            drop(stream);
            eprintln!(
                "silent exclusive {rate} Hz open/release passed; idle_release={idle_release}"
            );
        }

        let fallback_config = device
            .default_output_config()
            .expect("shared fallback config");
        let mut requested = fallback_config.config();
        requested.sample_rate = 88_200;
        let shared = test_shared_state(1, 1);
        shared.paused.store(true, Ordering::SeqCst);
        let (stream, actual_rate) =
            crate::playback::output::cpal_shared::build_started_output_stream_with_rate_fallback(
                &device,
                &requested,
                &fallback_config.config(),
                fallback_config.sample_format(),
                shared,
                command_tx,
                event_tx,
            )
            .expect("88.2 kHz request must have a playable shared fallback");
        eprintln!("silent shared 88200 Hz request opened at {actual_rate} Hz");
        drop(stream);
    }

    #[test]
    fn exclusive_stream_missing_selected_endpoint_does_not_open_default_output() {
        let (command_tx, _) = mpsc::channel();
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
        let result = build_exclusive_stream(
            Some("noor-regression-missing-endpoint"),
            "Missing selected output".into(),
            44_100,
            2,
            1,
            ExclusiveLatencyMode::Stable,
            Arc::new(ExclusiveRenderSourceBank::new()),
            command_tx,
            event_tx,
            Arc::new(()),
        );
        match result {
            Err(ExclusiveInitFailure::Other(reason)) => {
                assert!(reason.contains("selected output device lookup"), "{reason}");
            }
            Err(other) => panic!("wrong missing-endpoint failure: {other}"),
            Ok(_) => panic!("missing selected endpoint must never fall back to default output"),
        }
        assert!(event_rx.try_recv().is_err());
    }

    #[test]
    fn exclusive_candidate_loop_retries_endpoint_creation_without_masking_it() {
        let mut attempted = Vec::new();
        let mut waited = Vec::new();
        let result = initialize_candidates::<()>(
            |candidate| {
                let label = fmt_tag_label(candidate.format);
                attempted.push(label);
                if label == "f32" {
                    Err(ExclusiveInitFailure::FormatRejected(
                        "HRESULT 0x88890008".into(),
                    ))
                } else {
                    Err(ExclusiveInitFailure::EndpointCreationFailed)
                }
            },
            |delay| waited.push(delay),
        );
        assert_eq!(waited, DEVICE_IN_USE_BACKOFF_MS);
        assert_eq!(
            attempted.len(),
            exclusive_candidate_formats().len() + waited.len()
        );
        assert!(matches!(
            result,
            Err(ExclusiveInitFailure::EndpointCreationFailed)
        ));
    }

    #[test]
    fn exclusive_candidate_loop_recovers_transient_endpoint_creation() {
        let mut attempts = 0;
        let result = initialize_candidates(
            |_| {
                attempts += 1;
                if attempts == 1 {
                    Err(ExclusiveInitFailure::EndpointCreationFailed)
                } else {
                    Ok(())
                }
            },
            |_| {},
        );
        assert!(result.is_ok());
        assert_eq!(attempts, 2);
    }

    #[test]
    fn exclusive_candidate_loop_reports_all_format_rejections() {
        let mut attempts = 0;
        let result = initialize_candidates::<()>(
            |_| {
                attempts += 1;
                Err(ExclusiveInitFailure::FormatRejected(
                    "unsupported format".into(),
                ))
            },
            |_| panic!("unsupported formats should not consume ownership retry budget"),
        );
        assert!(matches!(
            result,
            Err(ExclusiveInitFailure::NoFormatAccepted)
        ));
        assert_eq!(attempts, exclusive_candidate_formats().len());
    }

    #[test]
    fn exclusive_candidate_loop_stops_when_control_is_disabled_or_device_busy() {
        let mut attempts = 0;
        let result = initialize_candidates::<()>(
            |_| {
                attempts += 1;
                Err(ExclusiveInitFailure::ExclusiveDisabled)
            },
            |_| panic!("disabled exclusive control cannot be retried"),
        );
        assert!(matches!(
            result,
            Err(ExclusiveInitFailure::ExclusiveDisabled)
        ));
        assert_eq!(attempts, 1);

        let mut waited = Vec::new();
        let result = initialize_candidates::<()>(
            |_| Err(ExclusiveInitFailure::DeviceInUse),
            |delay| waited.push(delay),
        );
        assert!(matches!(result, Err(ExclusiveInitFailure::DeviceInUse)));
        assert_eq!(waited, DEVICE_IN_USE_BACKOFF_MS);
    }

    #[test]
    fn classify_init_hresult_distinguishes_endpoint_creation_and_unsupported_format() {
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_ENDPOINT_CREATE_FAILED),
            InitErrorClassification::EndpointCreationFailed
        );
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_UNSUPPORTED_FORMAT),
            InitErrorClassification::FormatRejected
        );
    }

    #[test]
    fn integer_transport_preserves_every_decoded_i16_sample() {
        for pcm in i16::MIN..=i16::MAX {
            let decoded = f32::from(pcm) / 32_768.0;
            let mut bytes = [0; 2];
            convert_f32_to_bytes(&[decoded], &mut bytes, Format::I16, 2, 1);
            assert_eq!(i16::from_le_bytes(bytes), pcm, "PCM sample {pcm}");
        }
    }

    #[test]
    fn integer_transport_preserves_decoded_i24_samples() {
        for pcm in [-8_388_608_i32, -4_194_304, -1, 0, 1, 4_194_304, 8_388_607] {
            let decoded = pcm as f32 / 8_388_608.0;
            let mut packed = [0; 3];
            let mut padded = [0; 4];
            convert_f32_to_bytes(&[decoded], &mut packed, Format::I24Packed, 3, 1);
            convert_f32_to_bytes(&[decoded], &mut padded, Format::I24In32, 4, 1);
            assert_eq!(packed, pcm.to_le_bytes()[..3]);
            assert_eq!(i32::from_le_bytes(padded), pcm << 8);
        }
    }

    #[test]
    fn classify_init_hresult_maps_device_in_use() {
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_DEVICE_IN_USE),
            InitErrorClassification::DeviceInUse
        );
    }

    #[test]
    fn device_in_use_retry_budget_is_global() {
        let mut budget = DeviceInUseRetryBudget::new();

        assert_eq!(budget.next_delay_ms(), Some(50));
        assert_eq!(budget.next_delay_ms(), Some(150));
        assert_eq!(budget.next_delay_ms(), Some(350));
        assert_eq!(budget.next_delay_ms(), Some(750));
        assert_eq!(budget.next_delay_ms(), None);
        assert_eq!(budget.next_delay_ms(), None);
    }

    #[test]
    fn exclusive_source_bank_replaces_sources_atomically() {
        let bank = ExclusiveRenderSourceBank::new();
        assert!(bank.try_borrow().expect("empty source bank").is_empty());

        bank.set_sources(vec![
            ExclusiveRenderSource {
                role: ExclusiveRenderRole::Active,
                shared: test_shared_state(1, 1),
            },
            ExclusiveRenderSource {
                role: ExclusiveRenderRole::Prepared,
                shared: test_shared_state(2, 1),
            },
        ]);

        let snapshot = bank.try_borrow().expect("published sources");
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].role, ExclusiveRenderRole::Active);
        assert_eq!(snapshot[1].role, ExclusiveRenderRole::Prepared);

        drop(snapshot);
        bank.clear();
        assert!(bank.try_borrow().expect("cleared source bank").is_empty());
    }

    #[test]
    fn mix_f32_sources_sums_and_clamps_samples() {
        let mut out = vec![0.0_f32; 4];
        let a = vec![0.75, -0.75, 0.25, -0.25];
        let b = vec![0.75, -0.75, -0.5, 0.5];

        mix_f32_into(&mut out, &a);
        mix_f32_into(&mut out, &b);

        assert_eq!(out, vec![1.0, -1.0, -0.25, 0.25]);
    }

    fn test_shared_state(track_id: i64, generation: u64) -> Arc<PlaybackSharedState> {
        let (command_tx, _) = std::sync::mpsc::channel();
        Arc::new(PlaybackSharedState::new(
            track_id,
            generation,
            crate::playback::player::PlaybackSourceKind::TidalStream,
            crate::playback::gapless::GaplessPlan::disabled(),
            48_000,
            2,
            None,
            command_tx,
            Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        ))
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
    fn classify_init_hresult_preserves_invalid_period_as_initialization_error() {
        assert_eq!(
            classify_init_hresult(AUDCLNT_E_INVALID_DEVICE_PERIOD),
            InitErrorClassification::Other
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
                0x00, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x40
            ]
        );
    }

    #[test]
    fn candidate_formats_prefer_24_bit_before_full_width_i32() {
        let labels: Vec<_> = exclusive_candidate_formats()
            .iter()
            .map(|candidate| fmt_tag_label(candidate.format))
            .collect();

        assert_eq!(labels, vec!["i24-in-32", "i24-packed", "i32", "i16", "f32"]);
    }

    #[test]
    fn exclusive_latency_modes_pick_expected_period_targets() {
        use crate::db::audio_settings::ExclusiveLatencyMode;

        let default_period = 10_000;
        let min_period = 2_000;

        assert_eq!(
            target_exclusive_period_hns(default_period, min_period, &ExclusiveLatencyMode::Stable),
            default_period
        );
        assert_eq!(
            target_exclusive_period_hns(
                default_period,
                min_period,
                &ExclusiveLatencyMode::LowLatency
            ),
            6_000
        );
        assert_eq!(
            target_exclusive_period_hns(
                default_period,
                min_period,
                &ExclusiveLatencyMode::UltraLowLatency
            ),
            3_000
        );
    }

    #[test]
    fn format_labels_match_candidate_formats() {
        assert_eq!(fmt_tag_label(Format::F32), "f32");
        assert_eq!(fmt_tag_label(Format::I32), "i32");
        assert_eq!(fmt_tag_label(Format::I24In32), "i24-in-32");
        assert_eq!(fmt_tag_label(Format::I24Packed), "i24-packed");
        assert_eq!(fmt_tag_label(Format::I16), "i16");
    }

    #[test]
    fn exclusive_source_borrows_reuse_published_storage_without_owning_sources() {
        let bank = ExclusiveRenderSourceBank::new();
        let shared = test_shared_state(1, 1);
        let weak = Arc::downgrade(&shared);
        bank.set_sources(vec![ExclusiveRenderSource {
            role: ExclusiveRenderRole::Active,
            shared,
        }]);

        let first = bank.try_borrow().expect("first borrow");
        let second = bank.try_borrow().expect("second borrow");
        assert_eq!(first.as_ptr(), second.as_ptr());
        assert_eq!(
            weak.strong_count(),
            1,
            "the bank must own the sole source reference"
        );

        drop(first);
        drop(second);
        bank.clear();
        assert!(
            weak.upgrade().is_none(),
            "the publisher reclaimed retired PCM state"
        );
        assert!(bank.try_borrow().expect("cleared bank").is_empty());
    }

    #[test]
    fn exclusive_source_contention_renders_silence_without_advancing_pcm_or_idle_timer() {
        let bank = ExclusiveRenderSourceBank::new();
        let shared = test_shared_state(1, 1);
        shared.paused.store(false, Ordering::Relaxed);
        shared.buffer.lock().expect("buffer").samples = vec![0.5; 8];
        bank.set_sources(vec![ExclusiveRenderSource {
            role: ExclusiveRenderRole::Active,
            shared: Arc::clone(&shared),
        }]);
        let writer = bank.sources.write().expect("source bank writer");
        let (command_tx, _) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [1.0; 8];
        let mut scratch = [1.0; 8];
        let paused = mix_published_sources(&bank, &mut out, &mut scratch, &command_tx, &event_tx);
        assert_eq!(paused, None);
        assert_eq!(out, [0.0; 8]);
        assert_eq!(shared.source_position_samples.load(Ordering::Relaxed), 0);
        assert_eq!(shared.buffer.lock().expect("buffer").read_pos, 0);
        assert_eq!(bank.contended_periods.load(Ordering::Relaxed), 1);
        let now = Instant::now();
        let mut paused_since = Some(now - Duration::from_secs(10));
        assert!(!idle_release_due(
            &mut paused_since,
            paused,
            now,
            Duration::from_secs(1)
        ));
        assert!(paused_since.is_none());

        drop(writer);
        bank.clear();
        assert_eq!(
            mix_published_sources(&bank, &mut out, &mut scratch, &command_tx, &event_tx),
            Some(true)
        );
        assert_eq!(bank.contended_periods.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn exclusive_source_replacement_waits_for_borrow_then_publisher_reclaims_state() {
        let bank = Arc::new(ExclusiveRenderSourceBank::new());
        let shared = test_shared_state(1, 1);
        let weak = Arc::downgrade(&shared);
        bank.set_sources(vec![ExclusiveRenderSource {
            role: ExclusiveRenderRole::Active,
            shared,
        }]);
        let borrowed = bank.try_borrow().expect("render borrow");
        let publisher_bank = Arc::clone(&bank);
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let publisher = thread::spawn(move || {
            started_tx.send(()).expect("publish start");
            publisher_bank.clear();
            done_tx
                .send(weak.upgrade().is_none())
                .expect("reclamation result");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("publisher started");
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(borrowed.len(), 1);
        drop(borrowed);
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("publisher reclaimed the last source")
        );
        publisher.join().expect("publisher exited");
    }

    #[test]
    fn exclusive_stream_keeps_final_bank_ownership_until_after_render_join() {
        let bank = Arc::new(ExclusiveRenderSourceBank::new());
        let weak = Arc::downgrade(&bank);
        let render_bank = Arc::clone(&bank);
        let thread = thread::spawn(move || drop(render_bank));
        let mut stream = ExclusiveStream {
            shutdown: Arc::new(AtomicBool::new(false)),
            released: Arc::new(AtomicBool::new(false)),
            force_release: Arc::new(AtomicBool::new(false)),
            thread: Some(thread),
            source_bank: bank,
            fault: Arc::new(AtomicU8::new(0)),
            output_live: Arc::new(AtomicBool::new(true)),
            effective_sample_rate: 48_000,
            effective_channels: 2,
            transport_format: "test".into(),
        };
        stream
            .thread
            .take()
            .expect("render thread")
            .join()
            .expect("render exit");
        assert!(
            weak.upgrade().is_some(),
            "render exit must not reclaim the bank"
        );
        drop(stream);
        assert!(
            weak.upgrade().is_none(),
            "control-thread stream drop reclaims the bank"
        );
    }

    #[test]
    fn exclusive_fault_marker_is_consumed_once_and_release_does_not_retire_identity() {
        let stream = ExclusiveStream {
            shutdown: Arc::new(AtomicBool::new(false)),
            released: Arc::new(AtomicBool::new(false)),
            force_release: Arc::new(AtomicBool::new(false)),
            thread: None,
            source_bank: Arc::new(ExclusiveRenderSourceBank::new()),
            fault: Arc::new(AtomicU8::new(0)),
            output_live: Arc::new(AtomicBool::new(true)),
            effective_sample_rate: 48_000,
            effective_channels: 2,
            transport_format: "test".into(),
        };
        let identity = stream.output_live();
        assert!(stream.take_fault().is_none());
        stream.released.store(true, Ordering::Release);
        assert!(
            identity.load(Ordering::Acquire),
            "natural release must preserve fault event identity"
        );
        assert!(
            stream.take_fault().is_none(),
            "normal release must not produce a fault"
        );
        for failure in [
            ExclusiveRenderFault::EventTimeout,
            ExclusiveRenderFault::EventWaitFailed,
            ExclusiveRenderFault::DeviceWriteFailed,
            ExclusiveRenderFault::RendererPanicked,
        ] {
            stream.fault.store(failure as u8, Ordering::Release);
            assert_eq!(stream.take_fault(), Some(failure));
            assert_eq!(stream.take_fault(), None);
        }
        drop(stream);
        assert!(
            !identity.load(Ordering::Acquire),
            "replaced outputs must retire queued fault events"
        );
    }

    #[test]
    fn exclusive_source_publication_recovers_poison_and_reports_silent_periods() {
        let bank = Arc::new(ExclusiveRenderSourceBank::new());
        let poisoned_bank = Arc::clone(&bank);
        assert!(
            thread::spawn(move || {
                let _writer = poisoned_bank.sources.write().expect("writer");
                panic!("source publisher fault");
            })
            .join()
            .is_err()
        );
        assert!(bank.try_borrow().is_none());
        assert_eq!(bank.poisoned_periods.load(Ordering::Relaxed), 1);
        bank.clear();
        assert!(bank.try_borrow().expect("recovered publication").is_empty());
    }

    #[test]
    fn exclusive_idle_grace_requires_continuously_confirmed_paused_sources() {
        let now = Instant::now();
        let grace = Duration::from_secs(1);
        let mut paused_since = None;
        assert!(!idle_release_due(&mut paused_since, Some(true), now, grace));
        assert!(!idle_release_due(
            &mut paused_since,
            None,
            now + grace,
            grace
        ));
        assert!(!idle_release_due(
            &mut paused_since,
            Some(true),
            now + grace,
            grace
        ));
        assert!(idle_release_due(
            &mut paused_since,
            Some(true),
            now + grace * 2,
            grace
        ));
        assert!(!idle_release_due(
            &mut paused_since,
            Some(false),
            now + grace * 2,
            grace
        ));
        assert!(paused_since.is_none());
    }

    #[test]
    fn exclusive_device_wait_does_not_enter_wait_after_cancellation() {
        for force in [false, true] {
            let shutdown = AtomicBool::new(!force);
            let force_release = AtomicBool::new(force);
            assert_eq!(
                wait_for_device_event(&shutdown, &force_release, DEVICE_EVENT_TIMEOUT, |_| {
                    panic!("a cancelled transport must not wait for a device event")
                }),
                DeviceEventWait::Cancelled,
            );
        }
    }

    #[test]
    fn exclusive_device_wait_paces_immediate_timeout_results() {
        let shutdown = AtomicBool::new(false);
        let force_release = AtomicBool::new(false);
        let mut calls = 0;
        let result =
            wait_for_device_event(&shutdown, &force_release, Duration::from_millis(35), |_| {
                calls += 1;
                Err(WasapiError::EventTimeout)
            });
        assert_eq!(result, DeviceEventWait::TimedOut);
        assert!(calls <= 8, "immediate event timeouts spun {calls} times");
    }

    #[test]
    fn exclusive_renderer_unwind_publishes_fault_after_resources_drop() {
        struct RenderResource<'a> {
            released: &'a AtomicBool,
            dropped: &'a AtomicBool,
        }
        impl Drop for RenderResource<'_> {
            fn drop(&mut self) {
                assert!(
                    !self.released.load(Ordering::Acquire),
                    "release preceded resource teardown"
                );
                self.dropped.store(true, Ordering::Release);
            }
        }
        let released = AtomicBool::new(false);
        let fault = AtomicU8::new(0);
        let dropped = AtomicBool::new(false);
        let outcome = std::panic::catch_unwind(|| {
            run_render_thread_guarded(&released, &fault, || {
                let _resource = RenderResource {
                    released: &released,
                    dropped: &dropped,
                };
                panic!("injected renderer dependency panic");
            });
        });
        assert!(outcome.is_err());
        assert!(dropped.load(Ordering::Acquire));
        assert!(
            released.load(Ordering::Acquire),
            "renderer panic left output marked live"
        );
        assert_eq!(
            fault.load(Ordering::Acquire),
            ExclusiveRenderFault::RendererPanicked as u8
        );
    }

    #[test]
    fn exclusive_renderer_guard_preserves_normal_release_and_prior_fault() {
        let released = AtomicBool::new(false);
        let fault = AtomicU8::new(0);
        run_render_thread_guarded(&released, &fault, || {
            released.store(true, Ordering::Release)
        });
        assert_eq!(fault.load(Ordering::Acquire), 0);
        assert!(released.load(Ordering::Acquire));
        released.store(false, Ordering::Release);
        fault.store(
            ExclusiveRenderFault::DeviceWriteFailed as u8,
            Ordering::Release,
        );
        assert!(
            std::panic::catch_unwind(|| {
                run_render_thread_guarded(&released, &fault, || panic!("teardown panic"));
            })
            .is_err()
        );
        assert!(released.load(Ordering::Acquire));
        assert_eq!(
            fault.load(Ordering::Acquire),
            ExclusiveRenderFault::DeviceWriteFailed as u8
        );
        // A panic after a completed release must not add a second fault event.
        fault.store(0, Ordering::Release);
        assert!(
            std::panic::catch_unwind(|| {
                run_render_thread_guarded(&released, &fault, || panic!("post-release panic"));
            })
            .is_err()
        );
        assert_eq!(fault.load(Ordering::Acquire), 0);
    }

    #[test]
    fn exclusive_device_wait_observes_cancellation_when_device_never_signals() {
        for force in [false, true] {
            let shutdown = AtomicBool::new(false);
            let force_release = AtomicBool::new(false);
            let mut calls = 0;
            let result =
                wait_for_device_event(&shutdown, &force_release, DEVICE_EVENT_TIMEOUT, |ms| {
                    assert!(
                        ms <= DEVICE_EVENT_CANCEL_POLL_MS,
                        "cancellation polling requested {ms} ms"
                    );
                    calls += 1;
                    if force {
                        force_release.store(true, Ordering::Release);
                    } else {
                        shutdown.store(true, Ordering::SeqCst);
                    }
                    Err(WasapiError::EventTimeout)
                });
            assert_eq!(result, DeviceEventWait::Cancelled);
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn exclusive_device_wait_preserves_total_timeout_for_non_signaling_device() {
        assert_eq!(DEVICE_EVENT_TIMEOUT, Duration::from_secs(2));
        let shutdown = AtomicBool::new(false);
        let force_release = AtomicBool::new(false);
        let timeout = Duration::from_millis(35);
        let started = Instant::now();
        let mut calls = 0;
        let result = wait_for_device_event(&shutdown, &force_release, timeout, |ms| {
            assert!(ms <= DEVICE_EVENT_CANCEL_POLL_MS);
            calls += 1;
            thread::sleep(Duration::from_millis(u64::from(ms)));
            Err(WasapiError::EventTimeout)
        });
        assert_eq!(result, DeviceEventWait::TimedOut);
        assert!(started.elapsed() >= timeout);
        assert!(
            calls >= 2,
            "one short timeout must not invalidate the endpoint"
        );
    }

    #[test]
    fn exclusive_device_wait_returns_events_and_non_timeout_errors_immediately() {
        let shutdown = AtomicBool::new(false);
        let force_release = AtomicBool::new(false);
        assert_eq!(
            wait_for_device_event(&shutdown, &force_release, DEVICE_EVENT_TIMEOUT, |_| Ok(())),
            DeviceEventWait::Ready,
        );
        let mut calls = 0;
        assert_eq!(
            wait_for_device_event(&shutdown, &force_release, DEVICE_EVENT_TIMEOUT, |_| {
                calls += 1;
                Err(WasapiError::ClientNotInit)
            }),
            DeviceEventWait::Failed,
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn exclusive_device_wait_prioritizes_release_over_simultaneous_device_signal() {
        let shutdown = AtomicBool::new(false);
        let force_release = AtomicBool::new(false);
        assert_eq!(
            wait_for_device_event(&shutdown, &force_release, DEVICE_EVENT_TIMEOUT, |_| {
                force_release.store(true, Ordering::Release);
                Ok(())
            }),
            DeviceEventWait::Cancelled,
        );
    }

    #[test]
    fn exclusive_format_probe_uses_supported_channel_mask_without_changing_pcm_layout() {
        let requested = WaveFormat::new(32, 24, &SampleType::Int, 48_000, 2, Some(3));
        let supported = WaveFormat::new(32, 24, &SampleType::Int, 48_000, 2, Some(0));
        let selected = select_probed_exclusive_format(requested, Ok(supported));
        assert_eq!(selected.get_dwchannelmask(), 0);
        assert_eq!(selected.get_samplespersec(), 48_000);
        assert_eq!(selected.get_blockalign(), 8);
        assert_eq!(selected.get_validbitspersample(), 24);
    }

    #[test]
    fn exclusive_format_probe_cannot_change_source_rate_channels_or_representation() {
        for supported in [
            WaveFormat::new(32, 24, &SampleType::Int, 96_000, 2, Some(3)),
            WaveFormat::new(32, 24, &SampleType::Int, 48_000, 1, Some(1)),
            WaveFormat::new(16, 16, &SampleType::Int, 48_000, 2, Some(3)),
            WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, Some(3)),
        ] {
            let requested = WaveFormat::new(32, 24, &SampleType::Int, 48_000, 2, Some(3));
            let selected = select_probed_exclusive_format(requested, Ok(supported));
            assert_eq!(selected.get_samplespersec(), 48_000);
            assert_eq!(selected.get_nchannels(), 2);
            assert_eq!(selected.get_bitspersample(), 32);
            assert_eq!(selected.get_validbitspersample(), 24);
            assert_eq!(selected.get_dwchannelmask(), 3);
        }
    }

    #[test]
    fn exclusive_format_probe_failure_preserves_initialize_as_authoritative_attempt() {
        let requested = WaveFormat::new(32, 24, &SampleType::Int, 48_000, 2, Some(3));
        let selected =
            select_probed_exclusive_format(requested, Err(WasapiError::UnsupportedFormat));
        assert_eq!(selected.get_samplespersec(), 48_000);
        assert_eq!(selected.get_bitspersample(), 32);
        assert_eq!(selected.get_validbitspersample(), 24);
        assert_eq!(selected.get_dwchannelmask(), 3);
    }

    #[test]
    fn exclusive_format_probe_accepts_unambiguous_waveformatex_representation() {
        let requested = WaveFormat::new(16, 16, &SampleType::Int, 44_100, 2, Some(3));
        let supported = requested.to_waveformatex().expect("unambiguous16-bit PCM");
        let selected = select_probed_exclusive_format(requested, Ok(supported));
        let cb_size = selected.as_waveformatex_ref().cbSize;
        assert_eq!(cb_size, 0);
        assert_eq!(selected.get_samplespersec(), 44_100);
        assert_eq!(selected.get_blockalign(), 4);
        assert_eq!(selected.get_validbitspersample(), 16);
    }
}
