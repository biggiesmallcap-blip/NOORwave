use super::commands::{PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackTerminalReason};
use super::timeline::AtomicHandoffTimeline;
use crate::playback::gapless::GaplessPlan;
use crate::playback::player::PlaybackSourceKind;
use anyhow::{Result, anyhow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use tracing::warn;

const GAPLESS_PREFILL_PAD_MS: usize = 250;
const NEAR_END_THRESHOLD_MS: i64 = 30_000;

#[derive(Clone, Copy, Debug)]
enum RtNotification {
    Started,
    NearEnd,
    Finished,
    Crossfade { position: u64, target: u64 },
    DropPreview { position: u64 },
}
/// Length of the seam ramps around a rendered DJ handoff: the outgoing live
/// stream fades out over this window while the installed transition buffer
/// carries a matching baked fade-in. Long enough to kill the cut click,
/// short enough that the momentary doubling of the outgoing track's audio
/// (live copy + its rendered continuation) reads as a single transient.
pub(crate) const DJ_HANDOFF_FADE_MS: u32 = 15;
/// Window over which an applied volume change slews to its new value. The
/// callback samples `volume_ctl` once per buffer, so an un-ramped change lands
/// as a step discontinuity at the buffer seam - one click per step, and a
/// slider drag emits a stream of them. This is a slew *rate*: a full 0.0<->1.0
/// move takes the whole window, a small nudge resolves proportionally sooner.
/// Long enough to put the step well below audibility, short enough that the
/// control still tracks the user's hand.
pub(crate) const VOLUME_RAMP_MS: u32 = 20;
/// Envelope length for a user pause / resume. Same reasoning as the volume
/// ramp - cutting the transport mid-waveform steps the output straight to
/// silence and clicks - but shorter, because the fade-out plays real audio on
/// its way down and that audio is not replayed on resume. 10ms is under the
/// threshold where a listener reads the pause as sluggish, and short enough
/// that the un-replayed tail stays imperceptible.
pub(crate) const TRANSPORT_FADE_MS: u32 = 10;
/// Sample count at which an active PlaybackBuffer emits a one-shot warning.
/// 50_000_000 f32 samples is ~200 MB of allocation - well past the size at
/// which the unbounded-buffer issue meaningfully impacts memory. This is
/// pure observability; the ring-buffer rewrite (deferred) is the real fix.
const BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES: usize = 50_000_000;

pub(crate) fn write_output_f32(data: &mut [f32], shared: &Arc<PlaybackSharedState>) {
    write_output_buffer(data, shared, |sample| sample);
    // Tap the finished output for the wallpaper visualiser. RT-safe: a
    // non-blocking try_lock + bounded copy, skipped on contention. Never
    // mutates `data`, so the audio is byte-for-byte unchanged.
    crate::playback::spectrum::capture_output(
        data,
        shared.device_channels,
        shared.device_sample_rate,
    );
}

#[cfg(target_os = "windows")]
pub(crate) fn fill_f32_from_shared(data: &mut [f32], shared: &Arc<PlaybackSharedState>) {
    write_output_f32(data, shared);
}

pub(crate) fn write_output_i16(data: &mut [i16], shared: &Arc<PlaybackSharedState>) {
    write_output_buffer(data, shared, |sample| {
        (sample.clamp(-1.0, 1.0) * 32_768.0).round() as i16
    })
}

pub(crate) fn write_output_u16(data: &mut [u16], shared: &Arc<PlaybackSharedState>) {
    write_output_buffer(data, shared, |sample| {
        (sample.clamp(-1.0, 1.0) * 32_768.0 + 32_768.0).round() as u16
    })
}

pub(crate) fn write_output_i32(data: &mut [i32], shared: &Arc<PlaybackSharedState>) {
    write_output_buffer(data, shared, |sample| {
        (sample.clamp(-1.0, 1.0) * 2_147_483_648.0).round() as i32
    })
}

pub(crate) fn write_output_f64(data: &mut [f64], shared: &Arc<PlaybackSharedState>) {
    write_output_buffer(data, shared, f64::from)
}

pub(crate) fn write_output_i24(data: &mut [cpal::I24], shared: &Arc<PlaybackSharedState>) {
    write_output_buffer(data, shared, |sample| {
        let value = (sample.clamp(-1.0, 1.0) * 8_388_608.0)
            .round()
            .clamp(-8_388_608.0, 8_388_607.0) as i32;
        // Saturation above also maps NaN to integer zero, so the complete
        // input domain stays within I24's representable range.
        cpal::I24::new_unchecked(value)
    })
}

/// Equal-power ramp from 1.0 at `start` to 0.0 at `start + len`, held at 0
/// afterwards. Positions are absolute-track interleaved samples; applying the
/// same gain to every sample of a frame keeps channels matched.
fn dj_handoff_fadeout_gain(pos: u64, start: u64, len: u64) -> f32 {
    if pos <= start {
        return 1.0;
    }
    let elapsed = pos - start;
    if len == 0 || elapsed >= len {
        return 0.0;
    }
    let t = elapsed as f32 / len as f32;
    (t * std::f32::consts::FRAC_PI_2).cos()
}

/// Per-frame gain delta that walks a full-scale (0<->1) move across
/// `window_ms`. Frames, not interleaved samples: the ramps advance once per
/// frame so both channels of a frame share a gain (same invariant
/// `dj_handoff_fadeout_gain` keeps).
fn ramp_step_for_ms(window_ms: u32, sample_rate: u32) -> f32 {
    let ramp_frames = u64::from(window_ms) * u64::from(sample_rate) / 1_000;
    if ramp_frames == 0 {
        // Degenerate rate (unreported device): jump rather than stall at the
        // old gain forever.
        1.0
    } else {
        1.0 / ramp_frames as f32
    }
}

fn volume_ramp_step(sample_rate: u32) -> f32 {
    ramp_step_for_ms(VOLUME_RAMP_MS, sample_rate)
}

fn transport_ramp_step(sample_rate: u32) -> f32 {
    ramp_step_for_ms(TRANSPORT_FADE_MS, sample_rate)
}

/// One frame of slew from `current` toward `target`, never overshooting.
#[inline]
fn step_gain(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else if current > target {
        (current - step).max(target)
    } else {
        current
    }
}

/// Longest the render path waits for the PCM lock before giving up a period.
/// Decoder appends and O(1) compaction hold it for microseconds; this stays
/// far inside any device period, including low-latency exclusive mode.
const RENDER_LOCK_SPIN: std::time::Duration = std::time::Duration::from_micros(100);

fn try_lock_for_render(
    buffer: &Mutex<PlaybackBuffer>,
) -> std::sync::TryLockResult<std::sync::MutexGuard<'_, PlaybackBuffer>> {
    let mut started = None;
    loop {
        match buffer.try_lock() {
            Err(std::sync::TryLockError::WouldBlock)
                if started
                    .get_or_insert_with(std::time::Instant::now)
                    .elapsed()
                    < RENDER_LOCK_SPIN =>
            {
                std::hint::spin_loop();
            }
            result => return result,
        }
    }
}

fn write_output_buffer<T>(
    data: &mut [T],
    shared: &Arc<PlaybackSharedState>,
    convert: impl Fn(f32) -> T,
) {
    if shared.stopped.load(Ordering::SeqCst) {
        data.fill_with(|| convert(0.0));
        return;
    }

    // `paused` is a general-purpose mute gate, not a "the user pressed pause"
    // signal: pre-decoded next decks, DJ promotions and the stream-swap guard
    // all drive it for their own reasons, and those all want today's hard,
    // instant mute (a deck that isn't audible yet has no click to avoid, and a
    // promotion must hand over at full gain or it would notch the crossfade
    // envelope that is already shaping the transition). So only an armed fade -
    // which `PlaybackEngine::pause` sets and nothing else does - earns the
    // ramped exit; every other paused path falls straight through to silence.
    let paused = shared.paused.load(Ordering::SeqCst);
    let fading_out = shared.pause_fade_armed.load(Ordering::Relaxed);
    if paused && !fading_out {
        data.fill_with(|| convert(0.0));
        return;
    }

    // Where the user wants the gain, and where the output actually is right
    // now. `volume` resumes from the last frame the previous callback emitted,
    // so the ramp is continuous across buffer seams instead of restarting (and
    // re-stepping) at every callback.
    let target_volume = f32::from_bits(shared.volume_ctl.load(Ordering::Relaxed));
    let mut volume = f32::from_bits(shared.volume_smoothed.load(Ordering::Relaxed));
    let volume_step = volume_ramp_step(shared.device_sample_rate);
    let device_channels = shared.device_channels.max(1) as usize;

    // Transport envelope, orthogonal to the user's volume: 0 while paused, 1
    // while playing, slewed between. A resume needs no arming - the gain is
    // wherever the fade-out left it and simply walks back up to 1, which makes
    // a resume mid-fade (fast pause/play tap) join the ramp already in flight
    // rather than restart it.
    let transport_target = if fading_out { 0.0 } else { 1.0 };
    let mut transport = f32::from_bits(shared.transport_gain.load(Ordering::Relaxed));
    let transport_step = transport_ramp_step(shared.device_sample_rate);

    // Decoder append/compaction and DJ publication can hold this lock. Waiting
    // without bound would miss the device deadline, so spin briefly for the
    // usual microsecond holds and only then render silence; silence preserves
    // the cursor and envelope until the next callback consumes the same samples.
    // Notifications share this buffer's fixed mailbox, so seeks and buffer
    // replacement can retire them under the same lock as the PCM cursor.
    let mut guard = match try_lock_for_render(&shared.buffer) {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => {
            shared
                .deferred_buffer_contention
                .fetch_add(1, Ordering::Relaxed);
            data.fill_with(|| convert(0.0));
            return;
        }
        Err(_) => {
            data.fill_with(|| convert(0.0));
            return;
        }
    };

    // An accepted paused seek can cancel its old pause ramp while this
    // callback waits for the buffer. Recheck before draining the new cursor.
    if shared.paused.load(Ordering::SeqCst) && !shared.pause_fade_armed.load(Ordering::Relaxed) {
        data.fill_with(|| convert(0.0));
        return;
    }

    let recovery_target = shared.recovery_seek_samples.load(Ordering::Acquire);
    if recovery_target != u64::MAX {
        let offset = shared.position_offset_samples.load(Ordering::Relaxed);
        let local = recovery_target.saturating_sub(offset);
        if local > guard.samples.len() as u64 && !guard.finished {
            data.fill_with(|| convert(0.0));
            return;
        }
        // A short/changed source can finish before the saved position. Clamp
        // to its real end so normal terminal handling remains reachable.
        let local = local.min(guard.samples.len() as u64) as usize;
        guard.seek_to(local);
        shared
            .position_samples
            .store(offset + guard.read_pos as u64, Ordering::Relaxed);
        shared.publish_source_position();
        shared
            .recovery_seek_samples
            .store(u64::MAX, Ordering::Release);
    }

    let seek_target = shared.seek_target_samples.load(Ordering::Relaxed);
    if seek_target != u64::MAX
        && shared
            .seek_target_samples
            .compare_exchange(seek_target, u64::MAX, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    {
        // The seek target is in ABSOLUTE-track samples. Convert to a
        // buffer-local index by subtracting the engine's offset. If the target
        // is below the offset (a backward seek into territory this engine
        // hasn't decoded), reject - the runtime's SeekTo handler should have
        // intercepted this, but a defense-in-depth check keeps the buffer's
        // read_pos honest if it slips through.
        let offset = shared.position_offset_samples.load(Ordering::Relaxed);
        if seek_target < offset {
            // RT-safe deferred warn: latched here, logged from the runtime
            // loop's watchdog tick (tracing does I/O; never from this thread).
            shared
                .deferred_seek_target
                .store(seek_target, Ordering::Relaxed);
            shared.deferred_seek_offset.store(offset, Ordering::Relaxed);
            shared.deferred_seek_warn_kind.store(1, Ordering::Release);
        } else {
            let local_target = (seek_target - offset) as usize;
            if guard.seek_to(local_target) {
                shared
                    .position_samples
                    .store(seek_target, Ordering::Relaxed);
            } else {
                shared
                    .deferred_seek_target
                    .store(seek_target, Ordering::Relaxed);
                shared.deferred_seek_offset.store(offset, Ordering::Relaxed);
                shared
                    .deferred_seek_buffered
                    .store(offset + guard.samples.len() as u64, Ordering::Relaxed);
                shared.deferred_seek_warn_kind.store(2, Ordering::Release);
            }
        }
    }

    let ready_to_start = guard.is_ready();
    if ready_to_start && !guard.started {
        guard.started = true;
    }

    let xfade = shared.crossfade_samples.load(Ordering::Relaxed);
    let fade_gain = if xfade > 0 && !shared.suppress_crossfade_after_seek.load(Ordering::Relaxed) {
        let pos = shared.position_samples.load(Ordering::Relaxed);
        let fadein_start = shared.fadein_start_samples.load(Ordering::Relaxed);
        if fadein_start != u64::MAX {
            let elapsed = pos.saturating_sub(fadein_start);
            if elapsed < xfade {
                let t = (elapsed as f32 / xfade as f32).clamp(0.0, 1.0);
                (t * std::f32::consts::FRAC_PI_2).sin()
            } else {
                1.0f32
            }
        } else {
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
        let dj_fadeout_start = shared.dj_fadeout_start_samples.load(Ordering::Relaxed);
        if dj_fadeout_start != u64::MAX {
            // Rendered-handoff seam: the pre-rendered mix continues this
            // track's audio on the incoming engine, so this live copy must
            // leave over a short ramp, not a hard cut. Per-sample envelope;
            // the cos() only runs for DJ_HANDOFF_FADE_MS worth of samples
            // once per transition, then the branch degenerates to gain 0.
            let fade_len = u64::from(DJ_HANDOFF_FADE_MS)
                * u64::from(shared.device_sample_rate)
                * u64::from(shared.device_channels.max(1))
                / 1000;
            let mut cursor = shared.position_samples.load(Ordering::Relaxed);
            let mut chan = 0usize;
            guard.drain_into(data, &mut |s: f32| {
                let seam_gain = dj_handoff_fadeout_gain(cursor, dj_fadeout_start, fade_len);
                cursor += 1;
                if chan == 0 {
                    volume = step_gain(volume, target_volume, volume_step);
                    transport = step_gain(transport, transport_target, transport_step);
                }
                chan = (chan + 1) % device_channels;
                convert(s * volume * transport * fade_gain * seam_gain)
            })
        } else {
            let mut chan = 0usize;
            guard.drain_into(data, &mut |s: f32| {
                if chan == 0 {
                    volume = step_gain(volume, target_volume, volume_step);
                    transport = step_gain(transport, transport_target, transport_step);
                }
                chan = (chan + 1) % device_channels;
                convert(s * volume * transport * fade_gain)
            })
        }
    } else {
        data.fill_with(|| convert(0.0));
        0
    };

    // Publish where the ramps landed so the next callback picks up mid-slew.
    // The audio thread is the only writer; Relaxed matches volume_ctl's read.
    shared
        .volume_smoothed
        .store(volume.to_bits(), Ordering::Relaxed);

    if fading_out && (transport <= 0.0 || written == 0) {
        // The fade reached silence (or the deck produced nothing to fade -
        // never started, or starved mid-ramp, in which case there is no
        // waveform left to click). Snap to zero and disarm: `paused` is
        // already latched, so from here the gate above short-circuits to
        // silence without taking the buffer lock every callback.
        transport = 0.0;
        shared.pause_fade_armed.store(false, Ordering::Relaxed);
    }
    shared
        .transport_gain
        .store(transport.to_bits(), Ordering::Relaxed);

    if written == data.len() {
        guard.starved_notified = false;
    }

    if written > 0 {
        shared
            .position_samples
            .fetch_add(written as u64, Ordering::Relaxed);
    }
    shared.publish_source_position();

    // Real-time-safe telemetry: a relaxed atomic store + a load + conditional
    // CAS, no allocation. The buffered_samples mirror lets the HTTP /
    // route-side seek ack read "how much is decoded" without taking the
    // buffer mutex. Reported as ABSOLUTE-track samples (offset + decoded len)
    // so consumers can compare against an absolute target_samples directly.
    // The decoder thread observes growth_warned and emits the actual log line
    // off this thread.
    shared.publish_buffered_samples(guard.samples.len());
    shared.signal_buffer_growth_if_threshold_crossed(guard.samples.len());

    if guard.started && !guard.finished && written < data.len() && !guard.starved_notified {
        guard.starved_notified = true;
        // RT-safe deferred warn: an underrun means the system is already
        // struggling - the worst moment to do tracing I/O from the audio
        // thread. Latch and let the runtime loop's watchdog tick log it.
        shared
            .deferred_underrun_requested
            .store(data.len() as u64, Ordering::Relaxed);
        shared
            .deferred_underrun_written
            .store(written as u64, Ordering::Relaxed);
        shared.deferred_underrun_unread.store(
            guard.samples.len().saturating_sub(guard.read_pos) as u64,
            Ordering::Relaxed,
        );
        shared.deferred_underrun_warn.store(true, Ordering::Release);
    }

    if guard.started && !guard.started_notified {
        guard.started_notified = !shared.started_event_enabled.load(Ordering::Relaxed)
            || guard.push_notification(RtNotification::Started);
    }

    if guard.started && written == 0 && guard.finished && !guard.finished_notified {
        // Latch only once the terminal is actually handed to the runtime.
        // Latching first and discarding the send result made this the single
        // unacknowledged point of failure for the whole end-of-track advance:
        // a dropped terminal was never re-issued, so playback froze on the last
        // sample of the track with the queue intact. A dropped terminal past
        // this point (unmatched engine slot, or a guard in the queue-advance
        // handler) is now caught by the stall watchdog instead.
        let sent = guard.push_notification(RtNotification::Finished);
        guard.finished_notified = sent;
    }

    if !shared.near_end_signaled.load(Ordering::Relaxed) {
        let total = shared.total_samples.load(Ordering::Relaxed);
        if total > 0 {
            let pos = shared.position_samples.load(Ordering::Relaxed);
            let threshold = NEAR_END_THRESHOLD_MS
                * shared.device_sample_rate as i64
                * shared.device_channels as i64
                / 1000;
            if total.saturating_sub(pos) <= threshold as u64
                && guard.push_notification(RtNotification::NearEnd)
            {
                shared.near_end_signaled.store(true, Ordering::Relaxed);
            }
        }
    }

    if !shared.crossfade_start_signaled.load(Ordering::Relaxed) {
        let xfade = shared.crossfade_samples.load(Ordering::Relaxed);
        if xfade > 0 {
            let pos = shared.position_samples.load(Ordering::Relaxed);
            let trigger = shared.dj_fire_trigger_samples.load(Ordering::Relaxed);
            let trigger_target = if trigger != u64::MAX {
                // Beat-anchored DJ fire: the trigger is an absolute position
                // on the decoded-audio timeline, immune to the metadata /
                // decoded duration mismatch of the total-based countdown.
                Some(trigger)
            } else {
                let total = shared.total_samples.load(Ordering::Relaxed);
                (total > 0).then(|| total.saturating_sub(xfade))
            };
            if let Some(trigger_target_samples) = trigger_target.filter(|target| pos >= *target)
                && guard.push_notification(RtNotification::Crossfade {
                    position: pos,
                    target: trigger_target_samples,
                })
            {
                shared
                    .crossfade_start_signaled
                    .store(true, Ordering::Relaxed);
            }
        }
    }

    if !shared.drop_preview_start_signaled.load(Ordering::Relaxed) {
        let trigger = shared.drop_preview_trigger_samples.load(Ordering::Relaxed);
        if trigger != u64::MAX {
            let pos = shared.position_samples.load(Ordering::Relaxed);
            if pos >= trigger
                && guard.push_notification(RtNotification::DropPreview { position: pos })
            {
                shared
                    .drop_preview_start_signaled
                    .store(true, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct PlaybackSharedState {
    pub(crate) track_id: i64,
    pub(crate) generation: u64,
    pub(crate) source_kind: PlaybackSourceKind,
    pub(crate) paused: AtomicBool,
    /// Stop signal observed by the audio callback, the decoder thread, and
    /// (via Arc clone) the TIDAL stream pipe / CDN download thread so they
    /// can bail out promptly when a track is skipped or stopped.
    pub(crate) stopped: Arc<AtomicBool>,
    /// One-shot signal flipped by the audio callback when the underlying
    /// buffer crosses BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES; the decoder
    /// thread observes this and emits the warn (off the real-time thread).
    pub(crate) growth_warned: AtomicBool,
    pub(crate) deferred_buffer_contention: AtomicU64,
    pub(crate) deferred_cpal_notices: AtomicU32,
    /// Deferred RT diagnostics, same pattern as `growth_warned`: the audio
    /// callback latches a flag plus payload atomics and the runtime loop's
    /// watchdog tick emits the log lines off the real-time thread. Payloads
    /// are Relaxed stores sequenced before the Release store on the flag.
    pub(crate) deferred_underrun_warn: AtomicBool,
    pub(crate) deferred_underrun_requested: AtomicU64,
    pub(crate) deferred_underrun_written: AtomicU64,
    pub(crate) deferred_underrun_unread: AtomicU64,
    /// 0 = none, 1 = seek target below engine offset, 2 = seek target not
    /// decoded yet.
    pub(crate) deferred_seek_warn_kind: AtomicU32,
    pub(crate) deferred_seek_target: AtomicU64,
    pub(crate) deferred_seek_offset: AtomicU64,
    pub(crate) deferred_seek_buffered: AtomicU64,
    pub(crate) buffer: Mutex<PlaybackBuffer>,
    pending_rt_notifications: Arc<AtomicBool>,
    pub(crate) command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    pub(crate) volume_ctl: Arc<AtomicU32>,
    /// f32 bits of the gain actually applied to the last frame emitted, as
    /// opposed to `volume_ctl`'s target. Owned by the audio callback (sole
    /// writer) and carried across callbacks so a volume change slews over
    /// VOLUME_RAMP_MS rather than stepping at the buffer seam. Seeded to the
    /// current target so a fresh engine starts at gain, not at silence.
    pub(crate) volume_smoothed: AtomicU32,
    /// f32 bits of the pause/resume envelope: 0 while paused, 1 while playing.
    /// Audio-thread-owned, like `volume_smoothed`. Rests at 1.0 rather than
    /// tracking `paused`, so the mechanical mute paths (pre-decode, promotion,
    /// swap guard) that flip `paused` without arming a fade come up at full
    /// gain exactly as they do today - only a fade armed by
    /// `PlaybackEngine::pause` ever drives this to 0.
    pub(crate) transport_gain: AtomicU32,
    /// Set by `PlaybackEngine::pause` to request a ramped exit, cleared by the
    /// callback once the ramp reaches silence (and by `resume`). While set, the
    /// callback keeps draining a `paused` deck so the fade has audio to work
    /// with; `paused` itself still flips synchronously for every other reader.
    pub(crate) pause_fade_armed: AtomicBool,
    pub(crate) position_samples: Arc<AtomicU64>,
    /// Public original-track position; the buffer counter above remains on
    /// the output clock so decoding, compaction and sample consumption stay
    /// unchanged when an incoming cue/rate is used.
    pub(crate) source_position_samples: Arc<AtomicU64>,
    pub(crate) source_offset_samples: Arc<AtomicU64>,
    pub(crate) handoff_elapsed_samples: Arc<AtomicU64>,
    pub(crate) handoff_timeline: AtomicHandoffTimeline,
    /// Bounded source prefix replaced by the rendered mix. On manual seek
    /// the runtime restores plain incoming audio instead of replaying deck A.
    pub(crate) handoff_source_prefix: Mutex<Option<Vec<f32>>>,
    pub(crate) seek_target_samples: AtomicU64,
    recovery_seek_samples: AtomicU64,
    pub(crate) total_samples: AtomicU64,
    /// Mirror of `buffer.samples.len()` published from the audio callback so
    /// HTTP / route-side consumers can read "how much of this track is
    /// decoded" without taking the buffer mutex. Wrapped in `Arc` so the
    /// `PlaybackRuntimeHandle`'s redirectable `buffered_source` can point at
    /// the active engine's counter (parallel to `position_samples` /
    /// `position_source`). Used by the route-side seek ack path and the
    /// buffered-bar scrubber in the frontend.
    pub(crate) buffered_samples: Arc<AtomicU64>,
    /// Track-time offset (absolute device-samples) where this engine's decoded
    /// audio begins. For a fresh play this is 0; for a segment-seek restart
    /// (option C) it is seeded from `PreparedPlaybackJob::start_from_offset_ms`
    /// at engine construction. `position_samples` and `buffered_samples` are
    /// reported as ABSOLUTE-track samples (offset + buffer-local samples), so
    /// the route-side seek ack can decide intersect/segment-seek without
    /// having to know the engine's internal slicing.
    pub(crate) position_offset_samples: Arc<AtomicU64>,
    /// Per-segment start times in milliseconds, set once by the decoder thread
    /// after the DASH manifest resolves. `OnceLock` keeps the publish lock-free
    /// for readers; an empty `Vec` (or unset cell) signals "this engine has no
    /// segment metadata" (non-DASH source or pre-resolve state).
    pub(crate) segment_offsets_ms: std::sync::OnceLock<Vec<u64>>,
    pub(crate) near_end_signaled: AtomicBool,
    pub(crate) crossfade_samples: AtomicU64,
    pub(crate) crossfade_start_signaled: AtomicBool,
    /// Absolute-track interleaved sample position at which the DJ transition
    /// should fire (`u64::MAX` = disarmed). When armed, the callback fires at
    /// `pos >= trigger` instead of counting back from `total_samples`; the
    /// beat-grid anchor lives on the decoded-audio timeline, while
    /// total-based countdown inherits the (metadata - decoded) duration error
    /// of up to ~500ms, which is most of a beat at club tempos.
    pub(crate) dj_fire_trigger_samples: AtomicU64,
    /// Absolute-track interleaved sample position where a short equal-power
    /// fade-out begins (`u64::MAX` = disarmed). Armed on the outgoing engine
    /// when a rendered DJ handoff is installed so the live stream ramps to
    /// silence over DJ_HANDOFF_FADE_MS instead of hard-cutting mid-waveform.
    pub(crate) dj_fadeout_start_samples: AtomicU64,
    pub(crate) suppress_crossfade_after_seek: AtomicBool,
    pub(crate) drop_preview_trigger_samples: AtomicU64,
    pub(crate) drop_preview_start_signaled: AtomicBool,
    pub(crate) started_event_enabled: AtomicBool,
    pub(crate) fadein_start_samples: AtomicU64,
    pub(crate) device_sample_rate: u32,
    pub(crate) device_channels: u16,
    pub(crate) target_sample_rate: AtomicU32,
}

impl PlaybackSharedState {
    /// Test helper: drains the callback mailbox into commands and events.
    /// Production delivery is `receive_runtime_command` in the runtime loop,
    /// which differs on a failed send.
    #[cfg(test)]
    pub(crate) fn drain_rt_notifications(
        &self,
        command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
        event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    ) {
        while let Some(command) = self.take_rt_command(event_tx) {
            if command_tx.send(command).is_err() {
                if let Ok(mut buffer) = self.buffer.lock() {
                    buffer.finished_notified = false;
                }
                break;
            }
        }
    }

    /// Return commands directly to the actor rather than queueing them behind
    /// a concurrently submitted seek. Unconsumed values remain under its lock.
    pub(crate) fn take_rt_command(
        &self,
        event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    ) -> Option<PlaybackRuntimeCommand> {
        if self.paused.load(Ordering::SeqCst) || self.stopped.load(Ordering::SeqCst) {
            return None;
        }
        for _ in 0..32 {
            // Empty actor polls must not compete with the device callback for PCM.
            // Producers and consumers publish this hint under the buffer lock.
            if !self.pending_rt_notifications.load(Ordering::Acquire) {
                return None;
            }
            let notification = {
                let Ok(mut buffer) = self.buffer.try_lock() else {
                    return None;
                };
                let notification = buffer.notifications[0].take()?;
                buffer.notifications.rotate_left(1);
                buffer
                    .pending_notifications
                    .store(buffer.notifications[0].is_some(), Ordering::Release);
                notification
            };
            match notification {
                RtNotification::Started => {
                    let _ = event_tx.send(PlaybackRuntimeEvent::Started {
                        track_id: self.track_id,
                        generation: self.generation,
                        source: self.source_kind,
                    });
                }
                RtNotification::NearEnd => {
                    let _ = event_tx.send(PlaybackRuntimeEvent::NearEnd {
                        track_id: self.track_id,
                        generation: self.generation,
                    });
                }
                RtNotification::Finished => {
                    return Some(PlaybackRuntimeCommand::TrackTerminal {
                        track_id: self.track_id,
                        generation: self.generation,
                        decoder_stop: self.stop_flag(),
                        outcome: PlaybackTerminalReason::Finished,
                    });
                }
                RtNotification::Crossfade { position, target } => {
                    return Some(PlaybackRuntimeCommand::CrossfadeStart {
                        track_id: self.track_id,
                        generation: self.generation,
                        trigger_position_samples: position,
                        trigger_target_samples: target,
                    });
                }
                RtNotification::DropPreview { position } => {
                    return Some(PlaybackRuntimeCommand::DropPreviewStart {
                        track_id: self.track_id,
                        generation: self.generation,
                        trigger_position_samples: position,
                    });
                }
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
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
        position_offset_samples: Arc<AtomicU64>,
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
        let volume_smoothed = AtomicU32::new(volume_ctl.load(Ordering::Relaxed));
        let source_position_samples =
            Arc::new(AtomicU64::new(position_samples.load(Ordering::Relaxed)));
        let source_offset_samples = Arc::new(AtomicU64::new(
            position_offset_samples.load(Ordering::Relaxed),
        ));
        let buffer = PlaybackBuffer::new(prebuffer_samples);
        let pending_rt_notifications = Arc::clone(&buffer.pending_notifications);
        Self {
            track_id,
            generation,
            source_kind,
            paused: AtomicBool::new(false),
            stopped: Arc::new(AtomicBool::new(false)),
            growth_warned: AtomicBool::new(false),
            deferred_buffer_contention: AtomicU64::new(0),
            deferred_cpal_notices: AtomicU32::new(0),
            deferred_underrun_warn: AtomicBool::new(false),
            deferred_underrun_requested: AtomicU64::new(0),
            deferred_underrun_written: AtomicU64::new(0),
            deferred_underrun_unread: AtomicU64::new(0),
            deferred_seek_warn_kind: AtomicU32::new(0),
            deferred_seek_target: AtomicU64::new(0),
            deferred_seek_offset: AtomicU64::new(0),
            deferred_seek_buffered: AtomicU64::new(0),
            buffer: Mutex::new(buffer),
            pending_rt_notifications,
            command_tx,
            volume_ctl,
            volume_smoothed,
            transport_gain: AtomicU32::new(1.0f32.to_bits()),
            pause_fade_armed: AtomicBool::new(false),
            position_samples,
            source_position_samples,
            source_offset_samples,
            handoff_elapsed_samples: Arc::new(AtomicU64::new(u64::MAX)),
            handoff_timeline: AtomicHandoffTimeline::default(),
            handoff_source_prefix: Mutex::new(None),
            seek_target_samples: AtomicU64::new(u64::MAX),
            recovery_seek_samples: AtomicU64::new(u64::MAX),
            total_samples: AtomicU64::new(estimated_total_samples.unwrap_or(0)),
            buffered_samples: Arc::new(AtomicU64::new(0)),
            position_offset_samples,
            segment_offsets_ms: std::sync::OnceLock::new(),
            near_end_signaled: AtomicBool::new(false),
            crossfade_samples: AtomicU64::new(crossfade_samples),
            crossfade_start_signaled: AtomicBool::new(false),
            dj_fire_trigger_samples: AtomicU64::new(u64::MAX),
            dj_fadeout_start_samples: AtomicU64::new(u64::MAX),
            suppress_crossfade_after_seek: AtomicBool::new(false),
            drop_preview_trigger_samples: AtomicU64::new(u64::MAX),
            drop_preview_start_signaled: AtomicBool::new(false),
            started_event_enabled: AtomicBool::new(true),
            fadein_start_samples: AtomicU64::new(u64::MAX),
            device_sample_rate,
            device_channels,
            target_sample_rate: AtomicU32::new(device_sample_rate),
        }
    }

    /// Emit any diagnostics the audio callback latched (underrun, rejected
    /// in-callback seek). Called from the runtime loop's watchdog tick -
    /// never from the real-time thread, which only does the latching.
    /// Returns true when the system default output endpoint changed.
    pub(crate) fn drain_deferred_rt_logs(&self) -> bool {
        let notices = self.deferred_cpal_notices.swap(0, Ordering::Relaxed);
        let default_output_changed = notices & 8 != 0;
        if default_output_changed {
            warn!(
                track_id = self.track_id,
                "System default output device changed"
            );
        }
        if notices & 1 != 0 {
            warn!(
                track_id = self.track_id,
                "Playback output reported a recoverable underrun"
            );
        }
        if notices & 2 != 0 {
            warn!(
                track_id = self.track_id,
                "Playback output could not acquire real-time scheduling"
            );
        }
        if notices & 4 != 0 {
            warn!(
                track_id = self.track_id,
                "Playback output rerouted to the new system device"
            );
        }
        let contention = self.deferred_buffer_contention.swap(0, Ordering::Relaxed);
        if contention != 0 {
            warn!(
                track_id = self.track_id,
                generation = self.generation,
                callbacks = contention,
                "Playback output skipped busy decoder buffer"
            );
        }
        if self.deferred_underrun_warn.swap(false, Ordering::AcqRel) {
            warn!(
                "Playback buffer underrun: track_id={}, generation={}, requested={}, written={}, buffered_remaining={}",
                self.track_id,
                self.generation,
                self.deferred_underrun_requested.load(Ordering::Relaxed),
                self.deferred_underrun_written.load(Ordering::Relaxed),
                self.deferred_underrun_unread.load(Ordering::Relaxed),
            );
        }
        match self.deferred_seek_warn_kind.swap(0, Ordering::AcqRel) {
            1 => warn!(
                "Playback seek target is below engine offset: track_id={}, generation={}, target_samples={}, offset_samples={}",
                self.track_id,
                self.generation,
                self.deferred_seek_target.load(Ordering::Relaxed),
                self.deferred_seek_offset.load(Ordering::Relaxed),
            ),
            2 => warn!(
                "Playback seek target is not decoded yet: track_id={}, generation={}, target_samples={}, offset_samples={}, buffered_samples={}",
                self.track_id,
                self.generation,
                self.deferred_seek_target.load(Ordering::Relaxed),
                self.deferred_seek_offset.load(Ordering::Relaxed),
                self.deferred_seek_buffered.load(Ordering::Relaxed),
            ),
            _ => {}
        }
        default_output_changed
    }

    pub(crate) fn suppress_started_event(&self) {
        self.started_event_enabled.store(false, Ordering::Relaxed);
    }

    /// Latch a pause and, when this deck is actually making sound, ask the
    /// callback to ramp it out over TRANSPORT_FADE_MS instead of cutting
    /// mid-waveform. Returns whether a fade is now in flight, so a caller that
    /// is about to tear the deck down knows whether there is anything to wait
    /// for.
    ///
    /// Arms BEFORE latching `paused`: the callback only keeps draining a paused
    /// deck while the fade is armed, so arming second would race a callback
    /// into the hard-silence gate and clip the ramp.
    ///
    /// Reserved for user-initiated transport and teardown - see the gate in
    /// `write_output_buffer` for why the mechanical mute paths must not use it.
    pub(crate) fn begin_fade_out(&self) -> bool {
        // A fade only has work to do on an audible deck. Gain alone cannot tell
        // us that: a paused deck's transport gain rests at 1.0 behind the gate,
        // and a stopped one has had its buffer reset out from under it.
        let audible = !self.stopped.load(Ordering::SeqCst)
            && !self.paused.load(Ordering::SeqCst)
            && f32::from_bits(self.transport_gain.load(Ordering::Relaxed)) > 0.0;
        if audible {
            self.pause_fade_armed.store(true, Ordering::Relaxed);
        }
        self.paused.store(true, Ordering::SeqCst);
        audible
    }

    /// Drop any in-flight fade-out. The transport gain keeps whatever value the
    /// ramp reached, so the callback walks it back up to 1.0 from there - a
    /// resume during the fade rejoins the ramp rather than jumping.
    pub(crate) fn disarm_pause_fade(&self) {
        self.pause_fade_armed.store(false, Ordering::Relaxed);
    }

    pub(crate) fn clear_drop_preview_trigger(&self) {
        self.drop_preview_trigger_samples
            .store(u64::MAX, Ordering::Relaxed);
        self.drop_preview_start_signaled
            .store(true, Ordering::Relaxed);
    }

    pub(crate) fn reset_buffer(&self) {
        if let Ok(mut guard) = self.buffer.lock() {
            guard.reset();
        }
    }

    /// Clone the stop-signal Arc so source-side threads (TIDAL download,
    /// StreamPipe) can poll it for cancellation without holding the whole
    /// PlaybackSharedState.
    ///
    /// The flag is an eventual-consistency cancellation signal: writers
    /// (engine.stop) use SeqCst, readers may use Relaxed - every reader will
    /// see the flip within at most one polling tick. Don't promote the
    /// reads to SeqCst expecting tighter ordering; there's no co-data
    /// invariant to protect.
    pub(crate) fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stopped)
    }

    pub(crate) fn set_manual_seek_crossfade_suppression(&self, target_samples: u64) -> bool {
        let total = self.total_samples.load(Ordering::Relaxed);
        // NearEnd's 30-second threshold schedules preparation; it is not the
        // audible transition window. Only protect an already-crossed window
        // or a seek with less than half a second of preparation time.
        let threshold_samples = 500_u64
            .saturating_mul(u64::from(self.device_sample_rate))
            .saturating_mul(u64::from(self.device_channels.max(1)))
            / 1_000;
        let crossfade_samples = self.crossfade_samples.load(Ordering::Relaxed);
        let countdown_start = total.saturating_sub(crossfade_samples);
        let anchor = self.dj_fire_trigger_samples.load(Ordering::Relaxed);
        let transition_start_samples = countdown_start.min(anchor);
        let suppress = total > 0
            && (target_samples >= transition_start_samples
                || total.saturating_sub(target_samples) <= threshold_samples.max(1));
        self.suppress_crossfade_after_seek
            .store(suppress, Ordering::Relaxed);
        suppress
    }

    /// Audio-thread-safe: publish the current decoded-sample count for
    /// consumers outside the buffer mutex (the route-side seek ack and the
    /// frontend buffered-bar scrubber). Published as an ABSOLUTE-track
    /// upper bound (offset + buffer length) so a route-side caller can
    /// compare directly against an absolute target_samples. Uses bounded
    /// atomic reads/stores and arithmetic, without allocating or locking.
    pub(crate) fn publish_buffered_samples(&self, samples_len: usize) {
        let offset = self.position_offset_samples.load(Ordering::Relaxed);
        let channels = u64::from(self.device_channels.max(1));
        let source_offset = self
            .handoff_timeline
            .snapshot()
            .filter(|timeline| {
                offset / channels
                    < timeline
                        .output_origin
                        .saturating_add(timeline.output_frames)
            })
            // The original incoming prefix is retained until a seek restores
            // it. While the mix is still buffered, the opening is decoded too.
            .map_or_else(
                || self.output_to_source_samples(offset),
                |timeline| timeline.output_origin.saturating_mul(channels),
            );
        self.source_offset_samples
            .store(source_offset, Ordering::Relaxed);
        self.buffered_samples.store(
            self.output_to_source_samples(offset.saturating_add(samples_len as u64)),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn output_to_source_samples(&self, output_samples: u64) -> u64 {
        let channels = u64::from(self.device_channels.max(1));
        self.handoff_timeline
            .snapshot()
            .map_or(output_samples, |timeline| {
                timeline
                    .source_frame(output_samples / channels)
                    .saturating_mul(channels)
            })
    }

    pub(crate) fn source_to_output_samples(&self, source_samples: u64) -> Option<u64> {
        let channels = u64::from(self.device_channels.max(1));
        match self.handoff_timeline.snapshot() {
            None => Some(source_samples),
            Some(timeline) => timeline
                .output_frame(source_samples / channels)
                .map(|frame| frame.saturating_mul(channels)),
        }
    }

    pub(crate) fn publish_source_position(&self) {
        let output = self.position_samples.load(Ordering::Relaxed);
        self.source_position_samples
            .store(self.output_to_source_samples(output), Ordering::Relaxed);
        let channels = u64::from(self.device_channels.max(1));
        let elapsed = self
            .handoff_timeline
            .snapshot()
            .and_then(|timeline| {
                let frames = (output / channels).saturating_sub(timeline.output_origin);
                (frames < timeline.output_frames).then(|| frames.saturating_mul(channels))
            })
            .unwrap_or(u64::MAX);
        self.handoff_elapsed_samples
            .store(elapsed, Ordering::Relaxed);
    }

    /// Off the audio thread. Preserve source sample indices and the decoded
    /// remainder while retiring the handoff mapping on an accepted seek.
    pub(crate) fn restore_source_buffer_after_seek(&self) -> Result<bool> {
        let mut guard = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("playback buffer poisoned"))?;
        let Some(timeline) = self.handoff_timeline.snapshot() else {
            return Ok(false);
        };
        let channels = u64::from(self.device_channels.max(1));
        let offset = self.position_offset_samples.load(Ordering::Relaxed);
        let output = self.position_samples.load(Ordering::Relaxed);
        let source_position = self.output_to_source_samples(output);
        let source_total =
            self.output_to_source_samples(self.total_samples.load(Ordering::Relaxed));
        let mix_end = timeline
            .output_origin
            .saturating_add(timeline.output_frames)
            .saturating_mul(channels);
        let mut prefix = self
            .handoff_source_prefix
            .lock()
            .map_err(|_| anyhow!("handoff prefix poisoned"))?;
        let source_offset = if offset < mix_end {
            let remainder_at = mix_end.saturating_sub(offset) as usize;
            let mut original = prefix
                .take()
                .ok_or_else(|| anyhow!("handoff source prefix missing"))?;
            original.extend(guard.samples.range(remainder_at.min(guard.samples.len())..));
            guard.samples = original.into();
            timeline.output_origin.saturating_mul(channels)
        } else {
            prefix.take();
            self.output_to_source_samples(offset)
        };
        guard.read_pos = source_position
            .saturating_sub(source_offset)
            .min(guard.samples.len() as u64) as usize;
        for trigger in [
            &self.dj_fire_trigger_samples,
            &self.drop_preview_trigger_samples,
        ] {
            let output_trigger = trigger.load(Ordering::Relaxed);
            if output_trigger != u64::MAX {
                trigger.store(
                    self.output_to_source_samples(output_trigger),
                    Ordering::Relaxed,
                );
            }
        }
        self.position_offset_samples
            .store(source_offset, Ordering::Relaxed);
        self.position_samples
            .store(source_position, Ordering::Relaxed);
        self.total_samples.store(source_total, Ordering::Relaxed);
        self.handoff_timeline.clear();
        self.publish_source_position();
        self.publish_buffered_samples(guard.samples.len());
        Ok(true)
    }

    /// Apply decoded seeks under the existing buffer mutex before acknowledging
    /// them. The source clock and cursor advance together, even while paused.
    pub(crate) fn apply_in_buffer_seek(&self, target_samples: u64) -> Result<()> {
        let mut guard = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("playback buffer poisoned"))?;
        let offset = self.position_offset_samples.load(Ordering::Relaxed);
        let local = target_samples
            .checked_sub(offset)
            .ok_or_else(|| anyhow!("seek precedes decoded audio"))?;
        // The seek discards queued notifications. A drop preview latched for
        // one of them would otherwise never fire.
        let dropped_preview =
            guard.has_pending_notification(|n| matches!(n, RtNotification::DropPreview { .. }));
        if !guard.seek_to(local as usize) {
            return Err(anyhow!("seek exceeds decoded audio"));
        }
        if dropped_preview {
            self.drop_preview_start_signaled
                .store(false, Ordering::Relaxed);
        }
        if self.paused.load(Ordering::SeqCst) {
            // A pause's short ramp belongs to its old playhead. It must not
            // drain audio from the newly sought position while still paused.
            self.disarm_pause_fade();
            self.transport_gain
                .store(0.0f32.to_bits(), Ordering::Relaxed);
        }
        self.seek_target_samples.store(u64::MAX, Ordering::Relaxed);
        self.recovery_seek_samples
            .store(u64::MAX, Ordering::Release);
        self.position_samples
            .store(target_samples, Ordering::Relaxed);
        self.publish_source_position();
        self.publish_buffered_samples(guard.samples.len());
        Ok(())
    }

    pub(crate) fn arm_recovery_seek_ms(&self, target_ms: u64) -> Result<()> {
        let channels = u64::from(self.device_channels);
        if channels == 0 || self.device_sample_rate == 0 {
            return Err(anyhow!("Recovery requires a valid audio clock"));
        }
        let target = target_ms
            .checked_mul(u64::from(self.device_sample_rate))
            .map(|frames| frames / 1000)
            .and_then(|frames| frames.checked_mul(channels))
            .ok_or_else(|| anyhow!("Recovery position exceeds the audio clock"))?;
        let mut guard = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("playback buffer poisoned"))?;
        if !self.paused.load(Ordering::SeqCst) || guard.started {
            return Err(anyhow!("Recovery must be armed before playback starts"));
        }
        let offset = self.position_offset_samples.load(Ordering::Relaxed) / channels * channels;
        if target < offset {
            return Err(anyhow!("Recovery position precedes the decoded segment"));
        }
        let skip = target
            .saturating_sub(offset)
            .min(guard.samples.len() as u64) as usize;
        guard.samples.drain(..skip);
        guard.read_pos = 0;
        guard.clear_rt_notifications();
        self.position_offset_samples
            .store(offset + skip as u64, Ordering::Relaxed);
        self.position_samples.store(target, Ordering::Relaxed);
        self.publish_source_position();
        self.publish_buffered_samples(guard.samples.len());
        self.recovery_seek_samples.store(target, Ordering::Release);
        Ok(())
    }

    /// Decoder-side publication is necessary while the output gate is paused:
    /// the callback cannot publish a range it never visits.
    pub(crate) fn append_decoded_samples(&self, samples: &[f32]) -> Result<usize> {
        let mut guard = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("playback buffer poisoned"))?;
        if guard.sealed_for_render {
            return Ok(guard.samples.len());
        }
        let target = self.recovery_seek_samples.load(Ordering::Acquire);
        let offset = self.position_offset_samples.load(Ordering::Relaxed);
        let skip = if target == u64::MAX {
            0
        } else {
            target.saturating_sub(offset).min(samples.len() as u64) as usize
        };
        if skip != 0 {
            // Discard preroll before allocation/backpressure. Otherwise a
            // direct stream can fill its lookahead window before reaching
            // the recovery position while the output waits in silence.
            self.position_offset_samples
                .store(offset + skip as u64, Ordering::Relaxed);
        }
        guard.samples.extend(&samples[skip..]);
        self.publish_buffered_samples(guard.samples.len());
        Ok(guard.samples.len())
    }

    /// Reserve the steady-state window once, before decoding. Growth after
    /// that would reallocate and copy every buffered sample under the lock.
    pub(crate) fn reserve_buffer_capacity(&self, samples: usize) -> Result<()> {
        let mut guard = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("playback buffer poisoned"))?;
        let additional = samples.saturating_sub(guard.samples.len());
        guard.samples.reserve(additional);
        Ok(())
    }

    pub(crate) fn unread_buffered_samples(&self) -> Result<usize> {
        let guard = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("playback buffer poisoned"))?;
        Ok(guard.unread_samples())
    }

    pub(crate) fn compact_consumed_buffer(&self, retain_behind_samples: usize) -> Result<usize> {
        let mut guard = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("playback buffer poisoned"))?;
        let removed = guard.compact_consumed(retain_behind_samples);
        if removed > 0 {
            self.position_offset_samples
                .fetch_add(removed as u64, Ordering::Relaxed);
            self.publish_buffered_samples(guard.samples.len());
        }
        Ok(removed)
    }

    /// Audio-thread-safe signal: when the underlying buffer crosses
    /// BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES, flip the growth_warned flag once.
    /// Only the CAS false -> true succeeds the first time; subsequent calls
    /// are no-ops. The decoder thread observes the flag and emits the actual
    /// log line off the real-time audio thread.
    pub(crate) fn signal_buffer_growth_if_threshold_crossed(&self, samples_len: usize) {
        if samples_len >= BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES {
            let _ = self.growth_warned.compare_exchange(
                false,
                true,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }

    pub(crate) fn signal_terminal(&self, outcome: PlaybackTerminalReason) -> Result<()> {
        // A cold output retry replaces this decoder using the same job identity.
        // Cancellation from the retired decoder must not finish the replacement.
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.command_tx
            .send(PlaybackRuntimeCommand::TrackTerminal {
                track_id: self.track_id,
                generation: self.generation,
                decoder_stop: self.stop_flag(),
                outcome,
            })
            .map_err(|_| anyhow!("playback runtime command channel closed"))
    }
}

#[derive(Debug)]
pub(crate) struct PlaybackBuffer {
    notifications: [Option<RtNotification>; 32],
    pending_notifications: Arc<AtomicBool>,
    /// A ring so dropping consumed history is O(1). With a Vec, compaction
    /// moved the whole retained+lookahead window (tens of MB) while holding
    /// the lock the audio callback needs.
    pub(crate) samples: VecDeque<f32>,
    pub(crate) read_pos: usize,
    pub(crate) start_threshold_samples: usize,
    pub(crate) started: bool,
    pub(crate) started_notified: bool,
    pub(crate) starved_notified: bool,
    pub(crate) finished: bool,
    pub(crate) finished_notified: bool,
    /// A bounded optional overlay owns this buffer after installation. A
    /// decoder still winding down must not append the original song to it.
    pub(crate) sealed_for_render: bool,
}

impl PlaybackBuffer {
    fn push_notification(&mut self, notification: RtNotification) -> bool {
        if let Some(slot) = self.notifications.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(notification);
            self.pending_notifications.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    fn has_pending_notification(&self, kind: impl Fn(&RtNotification) -> bool) -> bool {
        self.notifications.iter().flatten().any(kind)
    }

    pub(crate) fn clear_rt_notifications(&mut self) {
        self.notifications.fill(None);
        self.pending_notifications.store(false, Ordering::Release);
    }

    pub(crate) fn new(start_threshold_samples: usize) -> Self {
        Self {
            notifications: [None; 32],
            pending_notifications: Arc::new(AtomicBool::new(false)),
            samples: VecDeque::new(),
            read_pos: 0,
            start_threshold_samples,
            started: false,
            started_notified: false,
            starved_notified: false,
            finished: false,
            finished_notified: false,
            sealed_for_render: false,
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.finished || (self.samples.len() - self.read_pos) >= self.start_threshold_samples
    }

    fn drain_into<T>(&mut self, data: &mut [T], convert: &mut impl FnMut(f32) -> T) -> usize {
        let remaining = self.samples.len().saturating_sub(self.read_pos);
        let available = remaining.min(data.len());
        for (dst, &sample) in data
            .iter_mut()
            .zip(self.samples.range(self.read_pos..self.read_pos + available))
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

    fn unread_samples(&self) -> usize {
        self.samples.len().saturating_sub(self.read_pos)
    }

    fn compact_consumed(&mut self, retain_behind_samples: usize) -> usize {
        if self.read_pos <= retain_behind_samples {
            return 0;
        }
        let remove = self.read_pos - retain_behind_samples;
        self.samples.drain(0..remove);
        self.read_pos -= remove;
        remove
    }

    pub(crate) fn mark_finished(&mut self) {
        self.finished = true;
    }

    fn seek_to(&mut self, target_samples: usize) -> bool {
        if target_samples > self.samples.len() && !self.finished {
            return false;
        }
        self.read_pos = target_samples.min(self.samples.len());
        // A queued Started is discarded with the mailbox, so let it fire
        // again. A decoded track that already announced Started must not
        // re-announce (TrackChanged, DJ lookahead restart) on every scrub.
        if self.has_pending_notification(|n| matches!(n, RtNotification::Started)) {
            self.started_notified = false;
        }
        self.clear_rt_notifications();
        self.finished_notified = false;
        self.starved_notified = false;
        if !self.finished {
            self.started = false;
            self.started_notified = false;
        }
        true
    }

    fn reset(&mut self) {
        self.clear_rt_notifications();
        self.samples.clear();
        self.read_pos = 0;
        self.started = false;
        self.started_notified = false;
        self.starved_notified = false;
        self.finished = false;
        self.finished_notified = false;
        self.sealed_for_render = false;
    }
}

pub(crate) fn samples_from_ms(ms: i32, sample_rate: u32, channels: u16) -> usize {
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

pub(crate) fn estimate_total_samples_from_duration_ms(
    duration_ms: i64,
    sample_rate: u32,
    channels: u16,
) -> Option<u64> {
    if duration_ms <= 0 || sample_rate == 0 || channels == 0 {
        return None;
    }
    Some((duration_ms as u64 * sample_rate as u64 * channels as u64) / 1_000)
}

#[cfg(test)]
pub(crate) fn test_measure_allocations<T>(work: impl FnOnce() -> T) -> (T, (u64, u64)) {
    tests::measure_allocations(work)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render_and_drain(
        data: &mut [f32],
        shared: &Arc<PlaybackSharedState>,
        command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
        event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    ) {
        super::write_output_f32(data, shared);
        shared.drain_rt_notifications(command_tx, event_tx);
    }
    use crate::playback::gapless::GaplessPlan;
    use crate::playback::player::PlaybackSourceKind;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct CallbackTestAllocator;

    thread_local! {
        static ALLOCATION_COUNTS: Cell<(bool, u64, u64)> = const { Cell::new((false, 0, 0)) };
    }

    #[global_allocator]
    static TEST_ALLOCATOR: CallbackTestAllocator = CallbackTestAllocator;

    pub(super) fn measure_allocations<T>(work: impl FnOnce() -> T) -> (T, (u64, u64)) {
        struct RestoreCounts((bool, u64, u64));
        impl Drop for RestoreCounts {
            fn drop(&mut self) {
                ALLOCATION_COUNTS.with(|counts| counts.set(self.0));
            }
        }
        let restore = RestoreCounts(ALLOCATION_COUNTS.with(|counts| {
            let previous = counts.get();
            counts.set((true, 0, 0));
            previous
        }));
        let result = work();
        let (_, allocations, frees) = ALLOCATION_COUNTS.with(Cell::get);
        drop(restore);
        (result, (allocations, frees))
    }

    // Delegate unchanged to System; count only on the measuring test thread.
    unsafe impl GlobalAlloc for CallbackTestAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATION_COUNTS.try_with(|counts| {
                let (enabled, allocations, frees) = counts.get();
                if enabled {
                    counts.set((enabled, allocations + 1, frees));
                }
            });
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            let _ = ALLOCATION_COUNTS.try_with(|counts| {
                let (enabled, allocations, frees) = counts.get();
                if enabled {
                    counts.set((enabled, allocations, frees + 1));
                }
            });
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[test]
    fn audio_callback_transport_notifications_do_not_allocate_or_free() {
        let shared = test_shared_state();
        shared.append_decoded_samples(&[0.25; 16]).unwrap();
        shared.buffer.lock().unwrap().mark_finished();
        shared.total_samples.store(16, Ordering::Relaxed);
        shared.crossfade_samples.store(16, Ordering::Relaxed);
        shared
            .drop_preview_trigger_samples
            .store(8, Ordering::Relaxed);
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
        let mut output = [0.0; 16];

        ALLOCATION_COUNTS.with(|counts| counts.set((true, 0, 0)));
        super::write_output_f32(&mut output, &shared);
        super::write_output_f32(&mut output, &shared);
        let (_, allocations, frees) = ALLOCATION_COUNTS.with(|counts| {
            let result = counts.get();
            counts.set((false, 0, 0));
            result
        });

        assert_eq!(
            (allocations, frees),
            (0, 0),
            "callback transition delivery used the allocator"
        );
        // The callback must only publish into its preallocated RT handoff.
        assert!(command_rx.try_recv().is_err());
        assert!(event_rx.try_recv().is_err());
        shared.drain_rt_notifications(&command_tx, &event_tx);
        assert!(matches!(
            event_rx.try_recv().unwrap(),
            PlaybackRuntimeEvent::Started { .. }
        ));
        assert!(matches!(
            event_rx.try_recv().unwrap(),
            PlaybackRuntimeEvent::NearEnd { .. }
        ));
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            PlaybackRuntimeCommand::CrossfadeStart { .. }
        ));
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            PlaybackRuntimeCommand::DropPreviewStart { .. }
        ));
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            PlaybackRuntimeCommand::TrackTerminal { .. }
        ));
    }

    #[test]
    fn cpal_first_fatal_callback_defers_owned_error_without_allocator_use() {
        let shared = test_shared_state();
        let live = Arc::new(AtomicBool::new(true));
        let callback = crate::playback::output::cpal_shared::CpalCallbackState::new(live);
        let error = cpal::Error::with_message(
            cpal::ErrorKind::DeviceNotAvailable,
            String::from("driver diagnostic retained for runtime"),
        );
        ALLOCATION_COUNTS.with(|counts| counts.set((true, 0, 0)));
        crate::playback::output::cpal_shared::emit_cpal_stream_error(&shared, &callback, error);
        let (_, allocations, frees) = ALLOCATION_COUNTS.with(|counts| {
            let result = counts.get();
            counts.set((false, 0, 0));
            result
        });
        assert_eq!(
            (allocations, frees),
            (0, 0),
            "fatal callback used the allocator"
        );
        match callback.take_command(&shared).unwrap() {
            PlaybackRuntimeCommand::CpalOutputError { message, .. } => {
                assert!(message.contains("driver diagnostic retained for runtime"));
            }
            other => panic!("expected retained diagnostic, got {other:?}"),
        }
    }

    #[test]
    fn recovery_preroll_is_discarded_without_playback_or_backpressure_growth() {
        let shared = test_shared_state();
        shared.paused.store(true, Ordering::SeqCst);
        shared.append_decoded_samples(&[0.1; 48]).unwrap();
        shared.arm_recovery_seek_ms(1).unwrap();
        assert_eq!(shared.buffer.lock().unwrap().samples.len(), 0);
        shared.paused.store(false, Ordering::SeqCst);
        let mut output = [1.0; 8];
        write_output_f32(&mut output, &shared);
        assert_eq!(output, [0.0; 8]);
        assert_eq!(shared.position_samples.load(Ordering::Relaxed), 96);
        shared.append_decoded_samples(&[0.2; 48]).unwrap();
        assert_eq!(shared.unread_buffered_samples().unwrap(), 0);
        shared.append_decoded_samples(&[0.75; 8]).unwrap();
        shared.buffer.lock().unwrap().mark_finished();
        write_output_f32(&mut output, &shared);
        assert_eq!(output, [0.75; 8], "recovery replayed preroll");
        assert_eq!(shared.position_samples.load(Ordering::Relaxed), 104);
        assert_eq!(shared.source_position_samples.load(Ordering::Relaxed), 104);
    }

    #[test]
    fn recovery_at_44100_hz_preserves_interleaved_channel_alignment() {
        let (command_tx, _) = mpsc::channel();
        let shared = PlaybackSharedState::new(
            1,
            1,
            PlaybackSourceKind::TidalStream,
            GaplessPlan::disabled(),
            44_100,
            2,
            None,
            command_tx,
            Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        shared.paused.store(true, Ordering::SeqCst);
        shared.arm_recovery_seek_ms(999).unwrap();
        let target = shared.recovery_seek_samples.load(Ordering::Acquire);
        assert_eq!(target, 88_110);
        assert_eq!(target % 2, 0);
    }

    #[test]
    fn recovery_callback_wait_and_start_do_not_allocate_or_free() {
        let shared = test_shared_state();
        shared.paused.store(true, Ordering::SeqCst);
        shared.arm_recovery_seek_ms(1).unwrap();
        shared.paused.store(false, Ordering::SeqCst);
        let mut output = [1.0; 8];
        for ready in [false, true] {
            if ready {
                shared.append_decoded_samples(&[0.5; 104]).unwrap();
                shared.buffer.lock().unwrap().mark_finished();
            }
            ALLOCATION_COUNTS.with(|counts| counts.set((true, 0, 0)));
            write_output_f32(&mut output, &shared);
            let (_, allocations, frees) = ALLOCATION_COUNTS.with(|counts| {
                let result = counts.get();
                counts.set((false, 0, 0));
                result
            });
            assert_eq!((allocations, frees), (0, 0));
            assert_eq!(output, [if ready { 0.5 } else { 0.0 }; 8]);
        }
    }

    #[test]
    fn recovery_short_source_reaches_actual_end_instead_of_waiting_forever() {
        let shared = test_shared_state();
        shared.paused.store(true, Ordering::SeqCst);
        shared.arm_recovery_seek_ms(1000).unwrap();
        shared.append_decoded_samples(&[0.5; 64]).unwrap();
        shared.buffer.lock().unwrap().mark_finished();
        shared.paused.store(false, Ordering::SeqCst);
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut output = [1.0; 8];
        write_output_f32(&mut output, &shared);
        assert_eq!(output, [0.0; 8]);
        assert_eq!(
            shared.recovery_seek_samples.load(Ordering::Acquire),
            u64::MAX
        );
        assert_eq!(shared.source_position_samples.load(Ordering::Relaxed), 64);
        assert!(matches!(
            shared.take_rt_command(&event_tx),
            Some(PlaybackRuntimeCommand::TrackTerminal { .. })
        ));
    }

    #[test]
    fn empty_actor_polling_does_not_interrupt_buffered_pcm() {
        let shared = test_shared_state();
        const BLOCKS: usize = 20_000;
        const SAMPLES: usize = 128;
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = vec![0.25; BLOCKS * SAMPLES].into();
            buffer.started = true;
            buffer.started_notified = true;
        }
        shared.near_end_signaled.store(true, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let observer_shared = shared.clone();
        let observer_stop = stop.clone();
        let observer_barrier = barrier.clone();
        let observer = std::thread::spawn(move || {
            let (event_tx, _) = tokio::sync::broadcast::channel(8);
            observer_barrier.wait();
            while !observer_stop.load(Ordering::Relaxed) {
                assert!(observer_shared.take_rt_command(&event_tx).is_none());
            }
        });
        let mut output = [0.0; SAMPLES];
        let mut silent_blocks = 0;
        barrier.wait();
        for _ in 0..BLOCKS {
            super::write_output_f32(&mut output, &shared);
            if output.iter().all(|sample| *sample == 0.0) {
                silent_blocks += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        observer.join().unwrap();
        assert_eq!(
            silent_blocks, 0,
            "empty actor polling inserted silent PCM blocks"
        );
        assert_eq!(
            shared.position_samples.load(Ordering::Relaxed),
            (BLOCKS * SAMPLES) as u64
        );
    }

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
    fn full_notification_mailbox_retries_terminal_after_control_drains() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.started = true;
            buffer.started_notified = true;
            buffer.finished = true;
            for _ in 0..32 {
                assert!(buffer.push_notification(RtNotification::NearEnd));
            }
        }
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(64);
        super::write_output_f32(&mut [0.0; 8], &shared);
        assert!(!shared.buffer.lock().unwrap().finished_notified);
        shared.drain_rt_notifications(&command_tx, &event_tx);
        super::write_output_f32(&mut [0.0; 8], &shared);
        shared.drain_rt_notifications(&command_tx, &event_tx);
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            PlaybackRuntimeCommand::TrackTerminal { .. }
        ));
        assert!(command_rx.try_recv().is_err());
    }

    #[test]
    fn paused_notifications_wait_for_resume_and_seek_retires_old_transition() {
        let shared = test_shared_state();
        shared.append_decoded_samples(&[0.25; 64]).unwrap();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
        super::write_output_f32(&mut [0.0; 8], &shared);
        shared.paused.store(true, Ordering::SeqCst);
        shared.drain_rt_notifications(&command_tx, &event_tx);
        assert!(event_rx.try_recv().is_err());
        shared.paused.store(false, Ordering::SeqCst);
        shared.drain_rt_notifications(&command_tx, &event_tx);
        assert!(matches!(
            event_rx.try_recv().unwrap(),
            PlaybackRuntimeEvent::Started { .. }
        ));
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.push_notification(RtNotification::Crossfade {
                position: 8,
                target: 8,
            });
            buffer.push_notification(RtNotification::Finished);
        }
        shared.apply_in_buffer_seek(16).unwrap();
        shared.drain_rt_notifications(&command_tx, &event_tx);
        assert!(command_rx.try_recv().is_err());
        super::write_output_f32(&mut [0.0; 8], &shared);
        shared.drain_rt_notifications(&command_tx, &event_tx);
        assert!(matches!(
            event_rx.try_recv().unwrap(),
            PlaybackRuntimeEvent::Started { .. }
        ));
    }

    #[test]
    fn audio_callback_does_not_wait_for_decoder_buffer() {
        let shared = test_shared_state();
        shared.append_decoded_samples(&[0.25; 128]).unwrap();
        let guard = shared.buffer.lock().unwrap();
        let render_shared = Arc::clone(&shared);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let render = std::thread::spawn(move || {
            let (command_tx, _) = mpsc::channel();
            let (event_tx, _) = tokio::sync::broadcast::channel(8);
            let mut data = [1.0; 128];
            ready_tx.send(()).unwrap();
            render_and_drain(&mut data, &render_shared, &command_tx, &event_tx);
            done_tx.send(data).unwrap();
        });
        ready_rx.recv().unwrap();
        let result = done_rx.recv_timeout(std::time::Duration::from_millis(100));
        // Always release the producer and join, including the failing baseline.
        drop(guard);
        render.join().unwrap();
        let data = result.expect("audio callback waited for the decoder's buffer lock");
        assert_eq!(data, [0.0; 128]);
        assert_eq!(shared.position_samples.load(Ordering::Relaxed), 0);
        assert_eq!(shared.buffer.lock().unwrap().read_pos, 0);
    }

    #[test]
    fn stopped_decoder_cannot_terminate_its_cold_replacement() {
        let (command_tx, command_rx) = mpsc::channel();
        let shared = PlaybackSharedState::new(
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
        );
        shared.stopped.store(true, Ordering::SeqCst);
        shared
            .signal_terminal(PlaybackTerminalReason::Error("cancelled".into()))
            .unwrap();
        assert!(command_rx.try_recv().is_err());
        shared.stopped.store(false, Ordering::SeqCst);
        shared
            .signal_terminal(PlaybackTerminalReason::Error("active decode failed".into()))
            .unwrap();
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            PlaybackRuntimeCommand::TrackTerminal { .. }
        ));
    }

    #[test]
    fn shared_i16_callback_preserves_signed_pcm_values() {
        let shared = test_shared_state();
        let expected = [i16::MIN, -16_384, -1, 0, 1, 16_384, i16::MAX, 0];
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = expected
                .iter()
                .map(|pcm| f32::from(*pcm) / 32_768.0)
                .collect();
            buffer.mark_finished();
        }
        let mut data = [0; 8];
        write_output_i16(&mut data, &shared);
        assert_eq!(data, expected);
    }

    #[test]
    fn shared_i32_callback_outputs_pcm_and_unsigned_silence_is_centered() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = vec![-1.0, -0.5, 0.0, 0.5, 1.0, 0.0].into();
            buffer.mark_finished();
        }
        let mut data = [0; 6];
        write_output_i32(&mut data, &shared);
        assert_eq!(
            data,
            [i32::MIN, -1_073_741_824, 0, 1_073_741_824, i32::MAX, 0]
        );
        shared.paused.store(true, Ordering::SeqCst);
        let mut unsigned = [0; 6];
        write_output_u16(&mut unsigned, &shared);
        assert_eq!(unsigned, [32_768; 6]);
    }

    #[test]
    fn shared_i24_callback_preserves_signed_pcm_and_paused_silence() {
        let shared = test_shared_state();
        let pcm = [-8_388_608, -4_194_304, -1, 0, 1, 4_194_304, 8_388_607, 0];
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = pcm
                .iter()
                .map(|value| *value as f32 / 8_388_608.0)
                .collect();
            buffer.mark_finished();
        }
        let mut output = [cpal::I24::default(); 8];
        write_output_i24(&mut output, &shared);
        assert_eq!(output.map(|value| value.inner()), pcm);
        shared.paused.store(true, Ordering::SeqCst);
        write_output_i24(&mut output, &shared);
        assert_eq!(output.map(|value| value.inner()), [0; 8]);
    }

    #[test]
    fn shared_f64_callback_preserves_float_samples_and_paused_silence() {
        let shared = test_shared_state();
        let pcm = [
            -1.0_f32,
            -0.5,
            -f32::EPSILON,
            0.0,
            f32::EPSILON,
            0.5,
            1.0,
            0.0,
        ];
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = pcm.to_vec().into();
            buffer.mark_finished();
        }
        let mut output = [0.0_f64; 8];
        write_output_f64(&mut output, &shared);
        assert_eq!(output, pcm.map(f64::from));
        shared.paused.store(true, Ordering::SeqCst);
        write_output_f64(&mut output, &shared);
        assert_eq!(output, [0.0; 8]);
    }

    #[test]
    fn seeking_thirty_seconds_before_the_end_preserves_a_future_mix() {
        let shared = test_shared_state();
        shared.total_samples.store(120 * 96_000, Ordering::Relaxed);
        shared
            .crossfade_samples
            .store(6 * 96_000, Ordering::Relaxed);
        assert!(!shared.set_manual_seek_crossfade_suppression(90 * 96_000));
        assert!(!shared.set_manual_seek_crossfade_suppression(110 * 96_000));
        assert!(shared.set_manual_seek_crossfade_suppression(114 * 96_000));
        assert!(shared.set_manual_seek_crossfade_suppression(119 * 96_000));
        // Seeking back before the window enables this pair again.
        assert!(!shared.set_manual_seek_crossfade_suppression(90 * 96_000));
    }

    #[test]
    fn paused_seek_cancels_a_pending_pause_fade_before_moving_the_playhead() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = vec![0.7; 192_000].into();
            buffer.mark_finished();
        }
        assert!(shared.begin_fade_out());
        shared.apply_in_buffer_seek(96_000).unwrap();
        assert!(drain_gain(&shared, 128).iter().all(|sample| *sample == 0.0));
        assert_eq!(shared.position_samples.load(Ordering::Relaxed), 96_000);
    }

    #[test]
    fn paused_seek_updates_source_clock_and_cursor_without_emitting_audio() {
        let shared = test_shared_state();
        shared
            .position_offset_samples
            .store(48_000, Ordering::Relaxed);
        shared.position_samples.store(48_000, Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = vec![0.7; 192_000].into();
            buffer.mark_finished();
        }
        shared.paused.store(true, Ordering::SeqCst);
        let (command_tx, _) = mpsc::channel();
        let handle = super::super::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
        *handle.position_source.lock().unwrap() = Arc::clone(&shared.source_position_samples);
        for target in [192_000, 72_000] {
            shared.apply_in_buffer_seek(target).unwrap();
            assert_eq!(
                shared.buffer.lock().unwrap().read_pos,
                (target - 48_000) as usize
            );
            assert_eq!(
                handle.get_position_ms(48_000, 2),
                (target * 1000 / 96_000) as i64
            );
            assert_eq!(shared.seek_target_samples.load(Ordering::Relaxed), u64::MAX);
            assert!(drain_gain(&shared, 128).iter().all(|sample| *sample == 0.0));
            assert_eq!(shared.position_samples.load(Ordering::Relaxed), target);
        }
        assert!(shared.apply_in_buffer_seek(47_999).is_err());
        shared.paused.store(false, Ordering::SeqCst);
        shared.apply_in_buffer_seek(96_000).unwrap();
        assert_eq!(shared.position_samples.load(Ordering::Relaxed), 96_000);
        assert_eq!(handle.get_position_ms(48_000, 2), 1000);
        assert!(drain_gain(&shared, 128).iter().any(|sample| *sample > 0.0));
        assert_eq!(
            shared.source_position_samples.load(Ordering::Relaxed),
            96_256
        );
    }

    #[test]
    fn paused_decoder_append_publishes_the_seek_range_without_an_audio_callback() {
        let shared = test_shared_state();
        shared.paused.store(true, Ordering::SeqCst);
        shared
            .position_offset_samples
            .store(96_000, Ordering::Relaxed);
        shared.position_samples.store(96_000, Ordering::Relaxed);
        shared.append_decoded_samples(&vec![0.7; 192_000]).unwrap();
        let (command_tx, _) = mpsc::channel();
        let handle = super::super::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
        *handle.position_source.lock().unwrap() = Arc::clone(&shared.source_position_samples);
        *handle.buffered_source.lock().unwrap() = Arc::clone(&shared.buffered_samples);
        *handle.offset_source.lock().unwrap() = Arc::clone(&shared.source_offset_samples);
        assert_eq!(handle.get_buffered_start_ms(48_000, 2), 1000);
        assert_eq!(handle.get_buffered_ms(48_000, 2), 3000);
        assert_eq!(
            super::super::evaluate_seek_decision(
                192_000,
                handle.buffered_start_samples(),
                handle.buffered_samples(),
                true
            ),
            super::super::SeekDecision::Dispatch
        );
        shared.apply_in_buffer_seek(192_000).unwrap();
        assert_eq!(handle.get_position_ms(48_000, 2), 2000);
        assert!(drain_gain(&shared, 128).iter().all(|sample| *sample == 0.0));
    }

    /// Drives one callback over an all-1.0 input, so every output sample is
    /// literally the gain that was applied to it.
    fn drain_gain(shared: &Arc<PlaybackSharedState>, frames: usize) -> Vec<f32> {
        let (command_tx, _) = mpsc::channel();
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
        let mut output = vec![0.0f32; frames * 2];
        render_and_drain(&mut output, shared, &command_tx, &event_tx);
        output
    }

    fn frame_gains(output: &[f32]) -> Vec<f32> {
        output
            .as_chunks::<2>()
            .0
            .iter()
            .map(|frame| frame[0])
            .collect()
    }

    fn max_frame_delta(gains: &[f32]) -> f32 {
        gains
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn fresh_state_seeds_smoothed_volume_at_target() {
        let shared = test_shared_state();
        assert_eq!(
            f32::from_bits(shared.volume_smoothed.load(Ordering::Relaxed)),
            1.0,
            "a fresh engine must start at the live volume; seeding at 0 would \
             fade every track in over the ramp window"
        );
    }

    #[test]
    fn untouched_volume_applies_exact_flat_gain() {
        let shared = test_shared_state();
        shared.volume_ctl.store(0.5f32.to_bits(), Ordering::Relaxed);
        shared
            .volume_smoothed
            .store(0.5f32.to_bits(), Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 16].into();
        }

        let output = drain_gain(&shared, 8);

        assert!(
            output.iter().all(|&s| s == 0.5),
            "with the control untouched the ramp must be a no-op and the gain \
             exact, bit-for-bit: {output:?}"
        );
    }

    #[test]
    fn volume_change_ramps_instead_of_stepping() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 1024].into();
        }
        // Slam the control to silence mid-stream: the mute case, and the
        // harshest step the old flat-multiply produced.
        shared.volume_ctl.store(0.0f32.to_bits(), Ordering::Relaxed);

        let output = drain_gain(&shared, 512);
        let gains = frame_gains(&output);
        let step = volume_ramp_step(48_000);

        assert!(
            gains[0] >= 1.0 - step * 2.0,
            "the ramp must depart from the previously applied gain, not jump \
             to the new target: first frame was {}",
            gains[0]
        );
        assert!(
            *gains.last().expect("frames") > 0.0,
            "512 frames is shorter than the 20ms ramp, so the gain must still \
             be in flight, not already collapsed to silence"
        );
        for frame in output.as_chunks::<2>().0 {
            assert_eq!(
                frame[0], frame[1],
                "both channels of a frame must share one gain"
            );
        }
        let delta = max_frame_delta(&gains);
        assert!(
            delta <= step + 1e-6,
            "no frame-to-frame gain jump may exceed the ramp step {step}; got \
             {delta} - that discontinuity is the audible click"
        );
    }

    #[test]
    fn ramp_stays_continuous_across_the_callback_seam() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 2048].into();
        }
        shared.volume_ctl.store(0.0f32.to_bits(), Ordering::Relaxed);

        let first = frame_gains(&drain_gain(&shared, 256));
        let second = frame_gains(&drain_gain(&shared, 256));
        let step = volume_ramp_step(48_000);

        // The buffer boundary is precisely where the old once-per-callback
        // load dropped its step, so the seam gets its own assertion.
        let seam = (*first.last().expect("frames") - second[0]).abs();
        assert!(
            seam <= step + 1e-6,
            "gain must carry across callbacks; seam jumped by {seam} (max {step})"
        );
        assert!(
            max_frame_delta(&second) <= step + 1e-6,
            "the ramp must keep slewing smoothly into the next buffer"
        );
        assert!(
            second.last().copied().expect("frames") < first[0],
            "the ramp must still be progressing toward the new target"
        );
    }

    /// What `PlaybackEngine::pause` does.
    fn user_pause(shared: &Arc<PlaybackSharedState>) {
        assert!(
            shared.begin_fade_out(),
            "an audible deck must report a fade in flight, so teardown knows to wait for it"
        );
    }

    fn user_resume(shared: &Arc<PlaybackSharedState>) {
        shared.disarm_pause_fade();
        shared.paused.store(false, Ordering::SeqCst);
    }

    #[test]
    fn begin_fade_out_reports_nothing_to_fade_on_a_silent_deck() {
        // Teardown batches every deck it owns, so `begin_fade_out` is what
        // decides which of them are worth waiting for. A paused deck (pre-decode,
        // drop preview) rests at transport gain 1.0 behind the gate, so gain
        // alone would wrongly call it audible - and arming it would make it
        // drain, eating the head of a track that was never heard.
        let paused_deck = test_shared_state();
        paused_deck.paused.store(true, Ordering::SeqCst);
        assert!(
            !paused_deck.begin_fade_out(),
            "a paused deck has nothing to fade"
        );
        assert!(
            !paused_deck.pause_fade_armed.load(Ordering::Relaxed),
            "a paused deck must not arm a fade, or the callback would drain it"
        );
        assert!(
            paused_deck.paused.load(Ordering::SeqCst),
            "the pause latch must hold regardless"
        );

        let stopped_deck = test_shared_state();
        stopped_deck.stopped.store(true, Ordering::SeqCst);
        assert!(
            !stopped_deck.begin_fade_out(),
            "a stopped deck has had its buffer reset; there is nothing left to fade"
        );

        let audible_deck = test_shared_state();
        assert!(
            audible_deck.begin_fade_out(),
            "a playing deck must report a fade in flight so teardown waits for it"
        );
        assert!(audible_deck.pause_fade_armed.load(Ordering::Relaxed));
        assert!(
            audible_deck.paused.load(Ordering::SeqCst),
            "begin_fade_out must latch the pause synchronously for other readers"
        );
    }

    #[test]
    fn pause_fade_ramps_out_instead_of_cutting() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 4096].into();
        }
        user_pause(&shared);

        // 128 frames sits inside the 10ms (480-frame) fade at 48k.
        let gains = frame_gains(&drain_gain(&shared, 128));
        let step = transport_ramp_step(48_000);

        assert!(
            gains[0] >= 1.0 - step * 2.0,
            "the fade must leave from full gain rather than cutting: {}",
            gains[0]
        );
        assert!(
            *gains.last().expect("frames") > 0.0,
            "still inside the fade window; gain must not have hit silence yet"
        );
        assert!(
            *gains.last().expect("frames") < gains[0],
            "the fade must actually be heading down"
        );
        assert!(
            max_frame_delta(&gains) <= step + 1e-6,
            "pause must not step the gain - that discontinuity is the click"
        );
    }

    #[test]
    fn pause_fade_completes_then_disarms_to_hard_silence() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 8192].into();
        }
        user_pause(&shared);

        // 512 frames overshoots the 480-frame fade, so the ramp lands on zero.
        let gains = frame_gains(&drain_gain(&shared, 512));
        assert_eq!(
            *gains.last().expect("frames"),
            0.0,
            "the fade must reach true silence, not a residual floor"
        );
        assert!(
            !shared.pause_fade_armed.load(Ordering::Relaxed),
            "the callback must disarm itself once silent, so a settled pause \
             stops taking the buffer lock every callback"
        );

        let settled_at = shared.position_samples.load(Ordering::Relaxed);
        let output = drain_gain(&shared, 128);
        assert!(
            output.iter().all(|&s| s == 0.0),
            "a settled pause must be silent"
        );
        assert_eq!(
            shared.position_samples.load(Ordering::Relaxed),
            settled_at,
            "a settled pause must stop consuming audio"
        );
    }

    #[test]
    fn mechanical_pause_never_drains() {
        // Pre-decoded next decks, the swap guard and start_paused all flip
        // `paused` without arming a fade. They must not consume a sample: a
        // pre-decoded deck that drains here eats the head of the next track.
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 512].into();
        }
        shared.paused.store(true, Ordering::SeqCst);

        let output = drain_gain(&shared, 128);

        assert!(
            output.iter().all(|&s| s == 0.0),
            "a mechanically muted deck must be silent"
        );
        assert_eq!(
            shared.position_samples.load(Ordering::Relaxed),
            0,
            "a mechanically muted deck must not advance position"
        );
        assert_eq!(
            shared.buffer.lock().expect("buffer").read_pos,
            0,
            "a mechanically muted deck must leave its buffer untouched"
        );
    }

    #[test]
    fn unpause_without_fade_comes_up_at_full_gain() {
        // Promotions hard-clear `paused` (honoring the user_paused latch) and
        // must hand over at full gain: the crossfade envelope already shapes
        // the transition, and a second fade-in layered on top would notch it.
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 64].into();
        }
        shared.paused.store(true, Ordering::SeqCst);
        shared.paused.store(false, Ordering::SeqCst);

        let output = drain_gain(&shared, 16);

        assert!(
            output.iter().all(|&s| s == 1.0),
            "a promotion must come up at exact full gain with no ramp: {output:?}"
        );
    }

    #[test]
    fn resume_mid_fade_rejoins_the_ramp() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; 4096].into();
        }
        user_pause(&shared);
        let down = frame_gains(&drain_gain(&shared, 128));
        let caught_mid_fade = *down.last().expect("frames");
        assert!(
            caught_mid_fade > 0.0 && caught_mid_fade < 1.0,
            "precondition: the fade should still be in flight, got {caught_mid_fade}"
        );

        // The fast pause/play tap.
        user_resume(&shared);
        let up = frame_gains(&drain_gain(&shared, 128));
        let step = transport_ramp_step(48_000);

        assert!(
            (up[0] - caught_mid_fade).abs() <= step + 1e-6,
            "resume must pick up where the fade left off ({caught_mid_fade}), got {}",
            up[0]
        );
        assert!(
            max_frame_delta(&up) <= step + 1e-6,
            "resume must not step the gain either"
        );
        assert!(
            *up.last().expect("frames") > caught_mid_fade,
            "resume must be heading back up toward full gain"
        );
    }

    #[test]
    fn publish_buffered_samples_mirrors_value_atomically() {
        let shared = test_shared_state();
        assert_eq!(
            shared.buffered_samples.load(Ordering::Relaxed),
            0,
            "fresh state must start at 0 buffered samples"
        );

        shared.publish_buffered_samples(12_345);
        assert_eq!(
            shared.buffered_samples.load(Ordering::Relaxed),
            12_345,
            "publish must store the exact sample count"
        );

        // Idempotent overwrite is fine - the callback re-publishes on every drain.
        shared.publish_buffered_samples(0);
        assert_eq!(shared.buffered_samples.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn started_event_emits_for_normal_playback() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![0.1, 0.2].into();
        }
        let (command_tx, _) = mpsc::channel();
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(4);
        let mut output = [0.0f32; 2];

        super::write_output_f32(&mut output, &shared);
        shared.drain_rt_notifications(&command_tx, &event_tx);

        match event_rx.try_recv().expect("started event") {
            PlaybackRuntimeEvent::Started {
                track_id,
                generation,
                source,
            } => {
                assert_eq!(track_id, 1);
                assert_eq!(generation, 1);
                assert_eq!(source, PlaybackSourceKind::TidalStream);
            }
            other => panic!("expected Started, got {other:?}"),
        }
    }

    #[test]
    fn suppress_started_event_keeps_preview_from_publishing_track_start() {
        let shared = test_shared_state();
        shared.suppress_started_event();
        {
            let mut buffer = shared.buffer.lock().expect("buffer");
            buffer.samples = vec![0.1, 0.2].into();
        }
        let mut output = [0.0f32; 2];

        super::write_output_f32(&mut output, &shared);

        let buffer = shared.buffer.lock().expect("buffer");
        assert!(
            !buffer.has_pending_notification(|n| matches!(n, RtNotification::Started)),
            "drop preview playback must not publish Started"
        );
        assert!(buffer.started);
        assert!(buffer.started_notified);
    }

    #[test]
    fn signal_buffer_growth_sets_flag_once_when_threshold_crossed() {
        let shared = test_shared_state();
        assert!(!shared.growth_warned.load(Ordering::Relaxed));

        shared.signal_buffer_growth_if_threshold_crossed(BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES - 1);
        assert!(
            !shared.growth_warned.load(Ordering::Relaxed),
            "below-threshold sample count should not set the flag"
        );

        shared.signal_buffer_growth_if_threshold_crossed(BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES);
        assert!(
            shared.growth_warned.load(Ordering::Relaxed),
            "threshold-crossed should set the flag"
        );

        // Subsequent calls past threshold are idempotent no-ops via CAS.
        shared.signal_buffer_growth_if_threshold_crossed(BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES * 2);
        assert!(shared.growth_warned.load(Ordering::Relaxed));
    }

    #[test]
    fn compact_consumed_buffer_preserves_unread_and_retained_samples() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples = (0..20).map(|sample| sample as f32).collect();
            buffer.read_pos = 12;
        }

        let removed = shared
            .compact_consumed_buffer(4)
            .expect("compact consumed buffer");

        assert_eq!(removed, 8);
        let buffer = shared.buffer.lock().expect("buffer lock");
        assert_eq!(buffer.read_pos, 4);
        assert_eq!(buffer.samples[0], 8.0);
        assert_eq!(buffer.samples[4], 12.0);
        assert_eq!(buffer.unread_samples(), 8);
    }

    #[test]
    fn compact_consumed_buffer_advances_offset_and_buffered_telemetry() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples = (0..20).map(|sample| sample as f32).collect();
            buffer.read_pos = 12;
        }

        let removed = shared
            .compact_consumed_buffer(4)
            .expect("compact consumed buffer");

        assert_eq!(removed, 8);
        assert_eq!(shared.position_offset_samples.load(Ordering::Relaxed), 8);
        assert_eq!(shared.buffered_samples.load(Ordering::Relaxed), 20);
    }

    #[test]
    fn dj_flag_off_uses_legacy_write_output_path() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[0.25, -0.5, 0.75, -1.0]);
            buffer.mark_finished();
        }
        let (command_tx, _) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0; 4];

        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        assert_eq!(out, [0.25, -0.5, 0.75, -1.0]);
    }

    #[test]
    fn dj_enabled_without_ready_mixer_uses_legacy_path() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[0.125, 0.25, 0.5, 1.0]);
            buffer.mark_finished();
        }
        let (command_tx, _) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0; 4];

        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        assert_eq!(out, [0.125, 0.25, 0.5, 1.0]);
    }

    /// Regression: a segment-restart engine (offset > 0) plus an armed DJ
    /// crossfade must not immediately mute the active track. The bug was that
    /// the decoder used to store LOCAL `samples.len()` into `total_samples`
    /// while `position_samples` is ABSOLUTE (offset + local). When DJ arming
    /// then set `crossfade_samples > 0`, the fade-out branch computed
    /// `remaining = total.saturating_sub(pos)` which saturated to 0, dropping
    /// fade_gain to sin(0) = 0 — audible silence for the entire window
    /// between arming and the actual transition. The fix is that the decoder
    /// stores ABSOLUTE total (offset + samples.len()), so this test simulates
    /// the post-fix invariant and asserts no premature attenuation.
    #[test]
    fn armed_crossfade_does_not_mute_segment_restart_active() {
        let shared = test_shared_state();
        // Mimic a segment-restart engine: offset is the segment start in
        // absolute samples (e.g. 120s into a track at 48000 Hz / 2 ch).
        let offset_samples: u64 = 120 * 48_000 * 2;
        shared
            .position_offset_samples
            .store(offset_samples, Ordering::Relaxed);

        // Local buffer covers the remaining 60s of the track.
        let local_samples: u64 = 60 * 48_000 * 2;
        // Decoder-stored total is ABSOLUTE (offset + local) per the contract.
        shared
            .total_samples
            .store(offset_samples + local_samples, Ordering::Relaxed);

        // Position: ~30s into this segment's playback (still ABSOLUTE).
        let played_samples: u64 = 30 * 48_000 * 2;
        shared
            .position_samples
            .store(offset_samples + played_samples, Ordering::Relaxed);

        // DJ arming for a 13s overlap (matches the BassSwap16 user log).
        let xfade: u64 = 13 * 48_000 * 2;
        shared.crossfade_samples.store(xfade, Ordering::Relaxed);
        shared
            .crossfade_start_signaled
            .store(false, Ordering::Relaxed);

        // 30s into the segment, 30s remaining, xfade=13s — gain must be 1.0.
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[1.0, 1.0, 1.0, 1.0]);
        }

        let (command_tx, _) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0_f32; 4];
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        assert_eq!(
            out,
            [1.0, 1.0, 1.0, 1.0],
            "armed but pre-fire-window active deck must play full-volume"
        );
    }

    /// Pin the equal-power contract of the legacy (two-stream) crossfade: at
    /// every block in the window the outgoing and incoming gains satisfy
    /// in^2 + out^2 == 1, so the summed amplitude of two full-scale streams
    /// never exceeds sqrt(2) (~+3 dB into the OS mixer) and there is no
    /// mid-fade loudness dip. Guards against swapping the sine curves for
    /// linear ramps (dip) or mismatched curves (clipping past sqrt(2)).
    #[test]
    fn crossfade_gains_are_equal_power_across_the_window() {
        const XFADE: u64 = 4_800;
        const BLOCK: usize = 480;

        // Outgoing deck: fade-out branch (fadein_start stays u64::MAX);
        // total == XFADE so the whole buffer plays inside the fade window.
        let outgoing = test_shared_state();
        outgoing.total_samples.store(XFADE, Ordering::Relaxed);
        outgoing.crossfade_samples.store(XFADE, Ordering::Relaxed);
        {
            let mut buffer = outgoing.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; XFADE as usize].into();
        }

        // Incoming deck: fade-in branch from position 0.
        let incoming = test_shared_state();
        incoming.crossfade_samples.store(XFADE, Ordering::Relaxed);
        incoming.fadein_start_samples.store(0, Ordering::Relaxed);
        {
            let mut buffer = incoming.buffer.lock().expect("buffer");
            buffer.samples = vec![1.0; XFADE as usize].into();
        }

        let (command_tx, _command_rx) = mpsc::channel();
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(64);

        let mut max_sum = 0.0_f32;
        for _ in 0..(XFADE as usize / BLOCK) {
            let mut out_block = [0.0_f32; BLOCK];
            let mut in_block = [0.0_f32; BLOCK];
            render_and_drain(&mut out_block, &outgoing, &command_tx, &event_tx);
            render_and_drain(&mut in_block, &incoming, &command_tx, &event_tx);
            for (out_sample, in_sample) in out_block.iter().zip(in_block.iter()) {
                let power = out_sample * out_sample + in_sample * in_sample;
                assert!(
                    (power - 1.0).abs() < 0.05,
                    "equal-power broken: out={out_sample} in={in_sample} power={power}"
                );
                max_sum = max_sum.max(out_sample + in_sample);
            }
        }
        assert!(
            max_sum <= std::f32::consts::SQRT_2 + 0.01,
            "crossfade sum exceeded sqrt(2): {max_sum}"
        );
        assert!(
            max_sum > 1.2,
            "mid-window sum should approach sqrt(2); got {max_sum} - did the curves change?"
        );
    }

    /// Regression: the crossfade-start signal must not fire prematurely on a
    /// segment-restart engine. Same root cause as the fade-out mute bug — if
    /// `total_samples` was LOCAL while `position_samples` is ABSOLUTE,
    /// `total - pos` would saturate to 0 < xfade and CrossfadeStart would be
    /// emitted immediately, causing the runtime to promote the next engine
    /// before the actual transition window opened.
    #[test]
    fn crossfade_start_does_not_fire_early_on_segment_restart() {
        let shared = test_shared_state();
        let offset_samples: u64 = 120 * 48_000 * 2;
        shared
            .position_offset_samples
            .store(offset_samples, Ordering::Relaxed);

        let local_samples: u64 = 60 * 48_000 * 2;
        shared
            .total_samples
            .store(offset_samples + local_samples, Ordering::Relaxed);

        let played_samples: u64 = 30 * 48_000 * 2;
        shared
            .position_samples
            .store(offset_samples + played_samples, Ordering::Relaxed);

        let xfade: u64 = 13 * 48_000 * 2;
        shared.crossfade_samples.store(xfade, Ordering::Relaxed);
        shared
            .crossfade_start_signaled
            .store(false, Ordering::Relaxed);

        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[0.5, 0.5, 0.5, 0.5]);
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0_f32; 4];
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        assert!(
            !shared.crossfade_start_signaled.load(Ordering::Relaxed),
            "30s remaining with 13s xfade must leave the start-signal pending"
        );
        assert!(
            command_rx.try_recv().is_err(),
            "no CrossfadeStart command should be queued yet"
        );
    }

    #[test]
    fn crossfade_start_command_captures_callback_position() {
        let shared = test_shared_state();
        shared.total_samples.store(10_000, Ordering::Relaxed);
        shared.position_samples.store(9_600, Ordering::Relaxed);
        shared.crossfade_samples.store(500, Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[0.5, 0.5, 0.5, 0.5]);
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0_f32; 4];
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        match command_rx.try_recv().expect("crossfade command") {
            PlaybackRuntimeCommand::CrossfadeStart {
                track_id,
                generation,
                trigger_position_samples,
                trigger_target_samples,
            } => {
                assert_eq!(track_id, shared.track_id);
                assert_eq!(generation, shared.generation);
                assert_eq!(trigger_position_samples, 9_604);
                assert_eq!(trigger_target_samples, 9_500);
            }
            other => panic!("expected CrossfadeStart, got {other:?}"),
        }
    }

    /// A beat-anchored transition must hold its fire until the absolute
    /// trigger position even when the metadata-derived end-window countdown
    /// has already been crossed. Same setup as
    /// crossfade_start_command_captures_callback_position (which fires), plus
    /// an anchor slightly ahead of the playhead.
    #[test]
    fn anchored_trigger_overrides_end_window_countdown() {
        let shared = test_shared_state();
        shared.total_samples.store(10_000, Ordering::Relaxed);
        shared.position_samples.store(9_600, Ordering::Relaxed);
        shared.crossfade_samples.store(500, Ordering::Relaxed);
        shared
            .dj_fire_trigger_samples
            .store(9_800, Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[0.5, 0.5, 0.5, 0.5]);
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0_f32; 4];
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        assert!(
            !shared.crossfade_start_signaled.load(Ordering::Relaxed),
            "inside the end window but before the anchor: must not fire"
        );
        assert!(command_rx.try_recv().is_err());

        // Reaching the anchor fires on the next callback.
        shared.position_samples.store(9_800, Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[0.5, 0.5, 0.5, 0.5]);
        }
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);
        match command_rx.try_recv().expect("crossfade command") {
            PlaybackRuntimeCommand::CrossfadeStart { .. } => {}
            other => panic!("expected CrossfadeStart, got {other:?}"),
        }
    }

    /// The seam fade-out ramps the live copy of the outgoing track to
    /// silence per-sample once a rendered handoff is installed.
    #[test]
    fn dj_fadeout_ramps_live_output_to_silence() {
        let shared = test_shared_state();
        shared.total_samples.store(10_000_000, Ordering::Relaxed);
        shared.crossfade_samples.store(500, Ordering::Relaxed);
        // Already signaled: the fire happened, the handoff is installed.
        shared
            .crossfade_start_signaled
            .store(true, Ordering::Relaxed);
        shared.dj_fadeout_start_samples.store(0, Ordering::Relaxed);
        // 15ms at 48k/2ch = 1440 interleaved samples; start half-way in.
        shared.position_samples.store(720, Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[1.0, 1.0, 1.0, 1.0]);
        }

        let (command_tx, _) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0_f32; 4];
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        let expected = (720.0_f32 / 1440.0 * std::f32::consts::FRAC_PI_2).cos();
        assert!(
            (out[0] - expected).abs() < 1e-3,
            "expected ~{expected} mid-fade, got {}",
            out[0]
        );
        assert!(out[3] < out[0], "gain must keep falling within the block");

        // Past the fade window the live copy is fully silent.
        shared.position_samples.store(2_000, Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[1.0, 1.0, 1.0, 1.0]);
        }
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn drop_preview_start_command_uses_absolute_trigger_without_crossfade() {
        let shared = test_shared_state();
        shared.total_samples.store(100_000, Ordering::Relaxed);
        shared.position_samples.store(48_000, Ordering::Relaxed);
        shared
            .drop_preview_trigger_samples
            .store(48_000, Ordering::Relaxed);
        shared
            .drop_preview_start_signaled
            .store(false, Ordering::Relaxed);
        {
            let mut buffer = shared.buffer.lock().expect("buffer lock");
            buffer.samples.extend(&[0.5, 0.5, 0.5, 0.5]);
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let mut out = [0.0_f32; 4];
        render_and_drain(&mut out, &shared, &command_tx, &event_tx);

        assert_eq!(shared.crossfade_samples.load(Ordering::Relaxed), 0);
        match command_rx.try_recv().expect("drop preview command") {
            PlaybackRuntimeCommand::DropPreviewStart {
                track_id,
                generation,
                trigger_position_samples,
            } => {
                assert_eq!(track_id, shared.track_id);
                assert_eq!(generation, shared.generation);
                assert_eq!(trigger_position_samples, 48_004);
            }
            other => panic!("expected DropPreviewStart, got {other:?}"),
        }
    }

    #[test]
    fn seek_to_decoded_position_applies_immediately() {
        let mut buffer = PlaybackBuffer::new(0);
        buffer.samples = vec![0.0; 1_000].into();
        buffer.started = true;

        assert!(buffer.seek_to(400));
        assert_eq!(buffer.read_pos, 400);
        assert!(!buffer.finished_notified);
        assert!(!buffer.starved_notified);
    }

    #[test]
    fn seek_to_undecoded_position_is_rejected_until_finished() {
        let mut buffer = PlaybackBuffer::new(0);
        buffer.samples = vec![0.0; 1_000].into();
        buffer.started = true;
        buffer.read_pos = 100;

        assert!(!buffer.seek_to(1_500));
        assert_eq!(buffer.read_pos, 100);
    }

    #[test]
    fn seek_to_end_of_finished_buffer_applies() {
        let mut buffer = PlaybackBuffer::new(0);
        buffer.samples = vec![0.0; 1_000].into();
        buffer.started = true;
        buffer.finished = true;

        assert!(buffer.seek_to(1_500));
        assert_eq!(buffer.read_pos, 1_000);
    }

    #[test]
    fn seek_rearms_started_only_when_its_notification_was_discarded() {
        let mut buffer = PlaybackBuffer::new(0);
        buffer.samples = vec![0.0; 1_000].into();
        buffer.started = true;
        buffer.started_notified = true;
        buffer.finished = true;
        assert!(buffer.seek_to(400));
        assert!(
            buffer.started_notified,
            "an announced, fully decoded track must not re-announce on seek"
        );

        assert!(buffer.push_notification(RtNotification::Started));
        assert!(buffer.seek_to(200));
        assert!(!buffer.started_notified, "a discarded Started must re-fire");
    }

    #[test]
    fn seek_rearms_a_drop_preview_whose_notification_was_discarded() {
        let shared = test_shared_state();
        {
            let mut buffer = shared.buffer.lock().unwrap();
            buffer.samples = vec![0.0; 1_000].into();
            buffer.started = true;
            buffer.finished = true;
            assert!(buffer.push_notification(RtNotification::DropPreview { position: 500 }));
        }
        shared
            .drop_preview_start_signaled
            .store(true, Ordering::Relaxed);
        shared.apply_in_buffer_seek(100).unwrap();
        assert!(!shared.drop_preview_start_signaled.load(Ordering::Relaxed));

        shared
            .drop_preview_start_signaled
            .store(true, Ordering::Relaxed);
        shared.apply_in_buffer_seek(200).unwrap();
        assert!(
            shared.drop_preview_start_signaled.load(Ordering::Relaxed),
            "a preview that already fired stays latched"
        );
    }
}
