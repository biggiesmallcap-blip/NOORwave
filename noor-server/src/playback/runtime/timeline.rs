//! Incoming source time for a rendered handoff buffer. Audio buffer indices
//! remain on the output clock; transport and musical markers use source time.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HandoffTimeline {
    pub output_origin: u64,
    pub source_start: u64,
    pub output_frames: u64,
    pub source_frames: u64,
}

impl HandoffTimeline {
    pub fn source_frame(self, output_frame: u64) -> u64 {
        let elapsed = output_frame.saturating_sub(self.output_origin);
        let consumed = if elapsed < self.output_frames {
            ((u128::from(elapsed) * u128::from(self.source_frames))
                / u128::from(self.output_frames.max(1))) as u64
        } else {
            self.source_frames
                .saturating_add(elapsed - self.output_frames)
        };
        self.source_start.saturating_add(consumed)
    }

    pub fn output_frame(self, source_frame: u64) -> Option<u64> {
        let consumed = source_frame.checked_sub(self.source_start)?;
        let elapsed = if consumed < self.source_frames {
            (u128::from(consumed) * u128::from(self.output_frames))
                .div_ceil(u128::from(self.source_frames.max(1))) as u64
        } else {
            self.output_frames
                .saturating_add(consumed - self.source_frames)
        };
        Some(self.output_origin.saturating_add(elapsed))
    }
}

/// Published under the existing audio-buffer mutex. Callbacks need four
/// atomic loads and no additional lock, allocation or unbounded retry.
#[derive(Debug)]
pub(crate) struct AtomicHandoffTimeline {
    output_origin: AtomicU64,
    source_start: AtomicU64,
    output_frames: AtomicU64,
    source_frames: AtomicU64,
}

impl Default for AtomicHandoffTimeline {
    fn default() -> Self {
        Self {
            output_origin: AtomicU64::new(u64::MAX),
            source_start: AtomicU64::new(0),
            output_frames: AtomicU64::new(0),
            source_frames: AtomicU64::new(0),
        }
    }
}

impl AtomicHandoffTimeline {
    pub fn install(&self, timeline: HandoffTimeline) {
        self.source_start
            .store(timeline.source_start, Ordering::Relaxed);
        self.output_frames
            .store(timeline.output_frames, Ordering::Relaxed);
        self.source_frames
            .store(timeline.source_frames, Ordering::Relaxed);
        self.output_origin
            .store(timeline.output_origin, Ordering::Release);
    }

    pub fn snapshot(&self) -> Option<HandoffTimeline> {
        let output_origin = self.output_origin.load(Ordering::Acquire);
        (output_origin != u64::MAX).then(|| HandoffTimeline {
            output_origin,
            source_start: self.source_start.load(Ordering::Relaxed),
            output_frames: self.output_frames.load(Ordering::Relaxed),
            source_frames: self.source_frames.load(Ordering::Relaxed),
        })
    }

    pub fn clear(&self) {
        self.output_origin.store(u64::MAX, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_time_remains_continuous_across_the_rate_handoff() {
        for source_frames in [97_000, 100_000, 103_000] {
            let timeline = HandoffTimeline {
                output_origin: 50_000,
                source_start: 800_000,
                output_frames: 100_000,
                source_frames,
            };
            assert_eq!(timeline.source_frame(50_000), 800_000);
            assert_eq!(timeline.source_frame(150_000), 800_000 + source_frames);
            assert_eq!(timeline.source_frame(160_000), 810_000 + source_frames);
            assert_eq!(timeline.output_frame(799_999), None);
            for elapsed in [0, 1, 48_000, 99_999, 100_000, 120_000] {
                let output = 50_000 + elapsed;
                let source = timeline.source_frame(output);
                let roundtrip = timeline.output_frame(source).unwrap();
                assert!(roundtrip.abs_diff(output) <= 1);
            }
        }
    }
}
