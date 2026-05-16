use super::commands::{PlaybackRuntimeCommand, PlaybackRuntimeEvent, PlaybackTerminalReason};
use crate::playback::gapless::GaplessPlan;
use crate::playback::player::PlaybackSourceKind;
use anyhow::{Result, anyhow};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use tracing::{debug, warn};

const GAPLESS_PREFILL_PAD_MS: usize = 250;
const NEAR_END_THRESHOLD_MS: i64 = 30_000;

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
        if guard.seek_to(seek_target as usize) {
            shared
                .position_samples
                .store(seek_target, Ordering::Relaxed);
        } else {
            warn!(
                "Playback seek target is not decoded yet: track_id={}, generation={}, target_samples={}, buffered_samples={}",
                shared.track_id,
                shared.generation,
                seek_target,
                guard.samples.len()
            );
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
    pub(crate) stopped: AtomicBool,
    pub(crate) buffer: Mutex<PlaybackBuffer>,
    pub(crate) command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    pub(crate) volume_ctl: Arc<AtomicU32>,
    pub(crate) position_samples: Arc<AtomicU64>,
    pub(crate) seek_target_samples: AtomicU64,
    pub(crate) total_samples: AtomicU64,
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
            stopped: AtomicBool::new(false),
            buffer: Mutex::new(PlaybackBuffer::new(prebuffer_samples)),
            command_tx,
            volume_ctl,
            position_samples,
            seek_target_samples: AtomicU64::new(u64::MAX),
            total_samples: AtomicU64::new(estimated_total_samples.unwrap_or(0)),
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
