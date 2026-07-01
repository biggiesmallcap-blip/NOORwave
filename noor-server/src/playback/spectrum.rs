//! Real-time output spectrum for the wallpaper visualiser.
//!
//! The audio callback ([`super::runtime::shared::write_output_f32`]) copies a
//! mono downmix of the samples actually going to the device into a rolling ring
//! via a NON-BLOCKING `try_lock`: if the lock is momentarily held by the FFT
//! reader, that callback's samples are simply skipped. No allocation, no FFT,
//! no blocking on the real-time audio thread, so the audible output and the
//! WASAPI/gapless timing are untouched.
//!
//! A single background task ([`run_spectrum_task`]) runs the FFT off-thread at
//! ~30 Hz and publishes log-spaced band magnitudes. Each WebSocket client polls
//! the latest frame. When the audio is silent the task stops publishing, so
//! there is no WS traffic while nothing plays.

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Mono ring length. ~93 ms at 44.1 kHz, comfortably larger than one FFT window.
const RING: usize = 4096;
/// FFT window. 2048 @ 44.1 kHz is ~46 ms, a good balance of frequency
/// resolution and responsiveness for a visualiser.
const FFT_SIZE: usize = 2048;
/// Number of log-spaced output bands. Kept small so the WS frame is tiny.
pub const NUM_BANDS: usize = 24;
const F_MIN: f32 = 40.0;
const F_MAX: f32 = 16_000.0;
/// Below this window RMS the frame is treated as silence and not published.
const SILENCE_RMS: f32 = 1.0e-4;

struct Ring {
    data: Box<[f32]>,
    pos: usize,
    filled: usize,
}

pub struct SpectrumCapture {
    ring: Mutex<Ring>,
    sample_rate: AtomicU32,
    latest: Mutex<Vec<f32>>,
    /// Bumped whenever `latest` is updated with a meaningful frame, so WS
    /// clients only forward new data and skip idle ticks.
    seq: AtomicU64,
    was_active: AtomicBool,
}

static CAPTURE: OnceLock<SpectrumCapture> = OnceLock::new();

/// Process-wide capture singleton. There is one active output stream at a time,
/// so a single global ring is correct and avoids threading a handle through the
/// audio callback signature.
pub fn global() -> &'static SpectrumCapture {
    CAPTURE.get_or_init(SpectrumCapture::empty)
}

impl SpectrumCapture {
    fn empty() -> Self {
        SpectrumCapture {
            ring: Mutex::new(Ring {
                data: vec![0.0; RING].into_boxed_slice(),
                pos: 0,
                filled: 0,
            }),
            sample_rate: AtomicU32::new(44_100),
            latest: Mutex::new(vec![0.0; NUM_BANDS]),
            seq: AtomicU64::new(0),
            was_active: AtomicBool::new(false),
        }
    }

    /// Called from the real-time audio callback with the interleaved f32 buffer
    /// that is about to be played. RT-safe: a non-blocking `try_lock` plus a
    /// bounded downmix copy, no allocation. On lock contention (the FFT reader
    /// holds it, ~20 us at 30 Hz) this callback's samples are skipped, which the
    /// visualiser never notices.
    pub fn push(&self, interleaved: &[f32], channels: u16, sample_rate: u32) {
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
        let mut g = match self.ring.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let ch = channels.max(1) as usize;
        let inv = 1.0 / ch as f32;
        let mut i = 0;
        while i + ch <= interleaved.len() {
            let mut s = 0.0;
            for c in 0..ch {
                s += interleaved[i + c];
            }
            let pos = g.pos;
            g.data[pos] = s * inv;
            g.pos = (pos + 1) % RING;
            if g.filled < RING {
                g.filled += 1;
            }
            i += ch;
        }
    }

    /// Copy the most recent FFT_SIZE samples (Hann-windowed) into `out` and
    /// return the window RMS. `None` until the ring has filled once.
    fn snapshot_window(&self, hann: &[f32], out: &mut [Complex<f32>]) -> Option<f32> {
        let g = self.ring.lock().ok()?;
        if g.filled < FFT_SIZE {
            return None;
        }
        let mut sumsq = 0.0f32;
        for (i, slot) in out.iter_mut().enumerate().take(FFT_SIZE) {
            let idx = (g.pos + RING - FFT_SIZE + i) % RING;
            let s = g.data[idx];
            sumsq += s * s;
            *slot = Complex::new(s * hann[i], 0.0);
        }
        Some((sumsq / FFT_SIZE as f32).sqrt())
    }

    /// Off-thread: FFT the latest window and publish bands. Publishes on any
    /// frame with signal, plus one settling frame on the transition to silence,
    /// then goes quiet so idle playback produces no WS traffic.
    fn compute(&self, fft: &Arc<dyn Fft<f32>>, hann: &[f32], scratch: &mut [Complex<f32>]) {
        let Some(rms) = self.snapshot_window(hann, scratch) else {
            return;
        };
        let signal = rms > SILENCE_RMS;
        let was = self.was_active.swap(signal, Ordering::Relaxed);
        if !signal && !was {
            return; // persistently silent
        }
        let bands = if signal {
            fft.process(scratch);
            let sr = self.sample_rate.load(Ordering::Relaxed) as f32;
            compute_bands(scratch, sr)
        } else {
            vec![0.0; NUM_BANDS]
        };
        if let Ok(mut latest) = self.latest.lock() {
            *latest = bands;
        }
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// WS client poll: returns the latest bands if a new frame arrived since
    /// `last_seq`, else `None` (nothing new / idle).
    pub fn poll(&self, last_seq: &mut u64) -> Option<Vec<f32>> {
        let s = self.seq.load(Ordering::Acquire);
        if s == *last_seq {
            return None;
        }
        *last_seq = s;
        self.latest.lock().ok().map(|b| b.clone())
    }
}

/// Aggregate FFT magnitudes into `NUM_BANDS` log-spaced bands, mapped to a 0..1
/// perceptual scale with a mild high-frequency lift so treble bands read.
fn compute_bands(spectrum: &[Complex<f32>], sample_rate: f32) -> Vec<f32> {
    let half = FFT_SIZE / 2;
    let bin_hz = sample_rate / FFT_SIZE as f32;
    let mut bands = vec![0.0f32; NUM_BANDS];
    let mut counts = [0u32; NUM_BANDS];
    let ln_ratio = (F_MAX / F_MIN).ln() / NUM_BANDS as f32;
    for (i, bin) in spectrum.iter().enumerate().take(half).skip(1) {
        let f = i as f32 * bin_hz;
        if f < F_MIN || f > F_MAX {
            continue;
        }
        let b = ((f / F_MIN).ln() / ln_ratio).floor() as isize;
        if b < 0 || b as usize >= NUM_BANDS {
            continue;
        }
        bands[b as usize] += bin.norm();
        counts[b as usize] += 1;
    }
    for k in 0..NUM_BANDS {
        let avg = if counts[k] > 0 {
            bands[k] / counts[k] as f32
        } else {
            0.0
        };
        // Normalize by window size, to dB, map [-70, -6] dB -> [0, 1].
        let norm = avg / FFT_SIZE as f32 * 2.0;
        let db = 20.0 * (norm + 1.0e-9).log10();
        let v = ((db + 70.0) / 64.0).clamp(0.0, 1.0);
        let tilt = 1.0 + 0.6 * (k as f32 / NUM_BANDS as f32);
        bands[k] = (v * tilt).clamp(0.0, 1.0);
    }
    bands
}

/// Background loop: recomputes the spectrum ~every 33 ms. Spawned once at boot.
pub async fn run_spectrum_task() {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let hann: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos())
        .collect();
    let mut scratch = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let cap = global();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(33));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        cap.compute(&fft, &hann, &mut scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_ring_publishes_nothing() {
        // Isolated instance: a fresh (empty) ring never publishes.
        let cap = SpectrumCapture::empty();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let hann = vec![1.0f32; FFT_SIZE];
        let mut scratch = vec![Complex::new(0.0, 0.0); FFT_SIZE];
        cap.compute(&fft, &hann, &mut scratch);
        assert_eq!(cap.seq.load(Ordering::Acquire), 0);
    }

    #[test]
    fn tone_produces_nonzero_bands() {
        // Feed a 1 kHz sine through push, then compute and expect signal.
        let cap = SpectrumCapture::empty();
        let sr = 44_100u32;
        let mut buf = vec![0.0f32; RING * 2];
        for (n, s) in buf.iter_mut().enumerate() {
            *s = (std::f32::consts::TAU * 1000.0 * n as f32 / sr as f32).sin() * 0.5;
        }
        // push as mono (channels = 1) in chunks
        for chunk in buf.chunks(512) {
            cap.push(chunk, 1, sr);
        }
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let hann: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        let mut scratch = vec![Complex::new(0.0, 0.0); FFT_SIZE];
        cap.compute(&fft, &hann, &mut scratch);
        let bands = cap.latest.lock().unwrap().clone();
        assert!(
            bands.iter().any(|&b| b > 0.05),
            "1 kHz tone should light a band: {bands:?}"
        );
    }
}
