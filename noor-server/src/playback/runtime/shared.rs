use super::commands::{PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackTerminalReason};
use crate::playback::gapless::GaplessPlan;
use crate::playback::player::PlaybackSourceKind;
use anyhow::{Result, anyhow};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use tracing::{debug, warn};

const GAPLESS_PREFILL_PAD_MS: usize = 250;
const NEAR_END_THRESHOLD_MS: i64 = 30_000;
/// Sample count at which an active PlaybackBuffer emits a one-shot warning.
/// 50_000_000 f32 samples is ~200 MB of allocation - well past the size at
/// which the unbounded-buffer issue meaningfully impacts memory. This is
/// pure observability; the ring-buffer rewrite (deferred) is the real fix.
const BUFFER_GROWTH_WARN_THRESHOLD_SAMPLES: usize = 50_000_000;

pub(crate) fn write_output_f32(
    data: &mut [f32],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    write_output_buffer(data, shared, command_tx, event_tx, |sample| sample)
}

#[cfg(target_os = "windows")]
pub(crate) fn fill_f32_from_shared(
    data: &mut [f32],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    write_output_f32(data, shared, command_tx, event_tx);
}

pub(crate) fn write_output_i16(
    data: &mut [i16],
    shared: &Arc<PlaybackSharedState>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    write_output_buffer(data, shared, command_tx, event_tx, |sample| {
        (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    })
}

pub(crate) fn write_output_u16(
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

    // Real-time safety contract for this critical section:
    //   * No IO, no syscalls, no allocations on the hot path.
    //   * drain_into is O(data.len()) and data.len() == one CPAL callback
    //     buffer (~256-1024 samples), bounded copy.
    //   * Rare events inside this guard (started/finished/near-end/crossfade
    //     event sends, underrun and seek-reject warn calls) each fire at
    //     most once per buffer-instance per event class, so allocation
    //     amortizes to zero across the steady-state callback rate.
    //   * The off-thread growth telemetry (Task 13) is a load + conditional
    //     CAS, no allocation.
    // A future refactor that adds IO, an unbounded loop, or a per-callback
    // allocation inside this guard would regress to dropouts/underruns -
    // keep the critical section bounded.
    let mut guard = match shared.buffer.lock() {
        Ok(guard) => guard,
        Err(_) => {
            data.fill_with(|| convert(0.0));
            return;
        }
    };

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
            warn!(
                "Playback seek target is below engine offset: track_id={}, generation={}, target_samples={}, offset_samples={}",
                shared.track_id, shared.generation, seek_target, offset
            );
        } else {
            let local_target = (seek_target - offset) as usize;
            if guard.seek_to(local_target) {
                shared
                    .position_samples
                    .store(seek_target, Ordering::Relaxed);
            } else {
                warn!(
                    "Playback seek target is not decoded yet: track_id={}, generation={}, target_samples={}, offset_samples={}, buffered_samples={}",
                    shared.track_id,
                    shared.generation,
                    seek_target,
                    offset,
                    offset + guard.samples.len() as u64
                );
            }
        }
    }

    let ready_to_start = guard.is_ready();
    if ready_to_start && !guard.started {
        debug!(
            "Playback buffer started: {} samples buffered, threshold {}, callback size {}",
            guard.samples.len().saturating_sub(guard.read_pos),
            guard.start_threshold_samples,
            data.len()
        );
        guard.started = true;
    }

    let xfade = shared.crossfade_samples.load(Ordering::Relaxed);
    let fade_gain = if xfade > 0 {
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
        guard.drain_into(data, &|s: f32| convert(s * volume * fade_gain))
    } else {
        data.fill_with(|| convert(0.0));
        0
    };

    if written == data.len() {
        guard.starved_notified = false;
    }

    if written > 0 {
        shared
            .position_samples
            .fetch_add(written as u64, Ordering::Relaxed);
    }

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
        warn!(
            "Playback buffer underrun: track_id={}, generation={}, requested={}, written={}, zero_filled={}, buffered_remaining={}, total_buffered={}, read_pos={}",
            shared.track_id,
            shared.generation,
            data.len(),
            written,
            data.len().saturating_sub(written),
            guard.samples.len().saturating_sub(guard.read_pos),
            guard.samples.len(),
            guard.read_pos
        );
    }

    if guard.started && !guard.started_notified {
        guard.started_notified = true;
        let _ = event_tx.send(PlaybackRuntimeEvent::Started {
            track_id: shared.track_id,
            generation: shared.generation,
            source: shared.source_kind,
        });
    }

    if guard.started && written == 0 && guard.finished && !guard.finished_notified {
        guard.finished_notified = true;
        debug!(
            "Playback runtime finished: track_id={}, generation={}, position_samples={}, total_samples={}",
            shared.track_id,
            shared.generation,
            shared.position_samples.load(Ordering::Relaxed),
            shared.total_samples.load(Ordering::Relaxed)
        );
        let _ = command_tx.send(PlaybackRuntimeCommand::TrackTerminal {
            track_id: shared.track_id,
            generation: shared.generation,
            outcome: PlaybackTerminalReason::Finished,
        });
    }

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
                    generation: shared.generation,
                });
            }
        }
    }

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
                        generation: shared.generation,
                    });
                }
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
    pub(crate) buffer: Mutex<PlaybackBuffer>,
    pub(crate) command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    pub(crate) volume_ctl: Arc<AtomicU32>,
    pub(crate) position_samples: Arc<AtomicU64>,
    pub(crate) seek_target_samples: AtomicU64,
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
    pub(crate) fadein_start_samples: AtomicU64,
    pub(crate) device_sample_rate: u32,
    pub(crate) device_channels: u16,
    pub(crate) target_sample_rate: AtomicU32,
}

impl PlaybackSharedState {
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
        Self {
            track_id,
            generation,
            source_kind,
            paused: AtomicBool::new(false),
            stopped: Arc::new(AtomicBool::new(false)),
            growth_warned: AtomicBool::new(false),
            buffer: Mutex::new(PlaybackBuffer::new(prebuffer_samples)),
            command_tx,
            volume_ctl,
            position_samples,
            seek_target_samples: AtomicU64::new(u64::MAX),
            total_samples: AtomicU64::new(estimated_total_samples.unwrap_or(0)),
            buffered_samples: Arc::new(AtomicU64::new(0)),
            position_offset_samples,
            segment_offsets_ms: std::sync::OnceLock::new(),
            near_end_signaled: AtomicBool::new(false),
            crossfade_samples: AtomicU64::new(crossfade_samples),
            crossfade_start_signaled: AtomicBool::new(false),
            fadein_start_samples: AtomicU64::new(u64::MAX),
            device_sample_rate,
            device_channels,
            target_sample_rate: AtomicU32::new(device_sample_rate),
        }
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

    /// Audio-thread-safe: publish the current decoded-sample count for
    /// consumers outside the buffer mutex (the route-side seek ack and the
    /// frontend buffered-bar scrubber). Published as an ABSOLUTE-track
    /// upper bound (offset + buffer length) so a route-side caller can
    /// compare directly against an absolute target_samples. Cost: a load
    /// plus a single Relaxed atomic store, no allocation - consistent with
    /// the other telemetry atomics flipped inside the CPAL critical section.
    pub(crate) fn publish_buffered_samples(&self, samples_len: usize) {
        let offset = self.position_offset_samples.load(Ordering::Relaxed);
        self.buffered_samples
            .store(offset.saturating_add(samples_len as u64), Ordering::Relaxed);
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
        self.command_tx
            .send(PlaybackRuntimeCommand::TrackTerminal {
                track_id: self.track_id,
                generation: self.generation,
                outcome,
            })
            .map_err(|_| anyhow!("playback runtime command channel closed"))
    }
}

#[derive(Debug)]
pub(crate) struct PlaybackBuffer {
    pub(crate) samples: Vec<f32>,
    pub(crate) read_pos: usize,
    pub(crate) start_threshold_samples: usize,
    pub(crate) started: bool,
    pub(crate) started_notified: bool,
    pub(crate) starved_notified: bool,
    pub(crate) finished: bool,
    pub(crate) finished_notified: bool,
}

impl PlaybackBuffer {
    pub(crate) fn new(start_threshold_samples: usize) -> Self {
        Self {
            samples: Vec::new(),
            read_pos: 0,
            start_threshold_samples,
            started: false,
            started_notified: false,
            starved_notified: false,
            finished: false,
            finished_notified: false,
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
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

    pub(crate) fn mark_finished(&mut self) {
        self.finished = true;
    }

    fn seek_to(&mut self, target_samples: usize) -> bool {
        if target_samples > self.samples.len() && !self.finished {
            return false;
        }
        self.read_pos = target_samples.min(self.samples.len());
        self.finished_notified = false;
        self.starved_notified = false;
        if !self.finished {
            self.started = false;
            self.started_notified = false;
        }
        true
    }

    fn reset(&mut self) {
        self.samples.clear();
        self.read_pos = 0;
        self.started = false;
        self.started_notified = false;
        self.starved_notified = false;
        self.finished = false;
        self.finished_notified = false;
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
mod tests {
    use super::*;
    use crate::playback::gapless::GaplessPlan;
    use crate::playback::player::PlaybackSourceKind;

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
    fn seek_to_decoded_position_applies_immediately() {
        let mut buffer = PlaybackBuffer::new(0);
        buffer.samples = vec![0.0; 1_000];
        buffer.started = true;

        assert!(buffer.seek_to(400));
        assert_eq!(buffer.read_pos, 400);
        assert!(!buffer.finished_notified);
        assert!(!buffer.starved_notified);
    }

    #[test]
    fn seek_to_undecoded_position_is_rejected_until_finished() {
        let mut buffer = PlaybackBuffer::new(0);
        buffer.samples = vec![0.0; 1_000];
        buffer.started = true;
        buffer.read_pos = 100;

        assert!(!buffer.seek_to(1_500));
        assert_eq!(buffer.read_pos, 100);
    }

    #[test]
    fn seek_to_end_of_finished_buffer_applies() {
        let mut buffer = PlaybackBuffer::new(0);
        buffer.samples = vec![0.0; 1_000];
        buffer.started = true;
        buffer.finished = true;

        assert!(buffer.seek_to(1_500));
        assert_eq!(buffer.read_pos, 1_000);
    }
}
