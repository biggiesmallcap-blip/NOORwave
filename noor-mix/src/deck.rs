pub struct DeckBuffer {
    samples: Vec<f32>,
    channels: u16,
    position: f64,
    loop_region: Option<(u64, u64)>,
}

impl DeckBuffer {
    pub fn new(samples: Vec<f32>, channels: u16) -> Self {
        Self {
            samples,
            channels: channels.max(1),
            position: 0.0,
            loop_region: None,
        }
    }

    pub fn set_loop_region(&mut self, start_frame: u64, end_frame: u64) {
        self.loop_region = (start_frame < end_frame).then_some((start_frame, end_frame));
    }

    pub fn clear_loop_region(&mut self) {
        self.loop_region = None;
    }

    pub fn tick_into(&mut self, out: &mut [f32], playback_rate: f32) {
        let channels = usize::from(self.channels);
        let frame_count = self.samples.len() / channels;
        if frame_count == 0 {
            out.fill(0.0);
            return;
        }

        let rate = if playback_rate.is_finite() {
            playback_rate.max(0.0) as f64
        } else {
            0.0
        };

        for frame in out.chunks_mut(channels) {
            let base_frame = self.position.floor().max(0.0) as usize;
            let next_frame = (base_frame + 1).min(frame_count - 1);
            let frac = (self.position - base_frame as f64) as f32;

            for (channel, sample) in frame.iter_mut().enumerate() {
                let a = self.samples[base_frame * channels + channel];
                let b = self.samples[next_frame * channels + channel];
                *sample = a + (b - a) * frac;
            }

            self.advance(rate, frame_count);
        }
    }

    fn advance(&mut self, rate: f64, frame_count: usize) {
        self.position += rate;
        if let Some((start, end)) = self.loop_region {
            let start = start as f64;
            let end = end.min(frame_count as u64) as f64;
            if start < end && self.position >= end {
                let length = end - start;
                self.position = start + (self.position - end).rem_euclid(length);
            }
        } else {
            let max_position = frame_count.saturating_sub(1) as f64;
            if self.position > max_position {
                self.position = max_position;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_into_unit_rate_is_identity() {
        let mut deck = DeckBuffer::new(vec![0.0, 1.0, 2.0, 3.0], 1);
        let mut out = [0.0; 4];
        deck.tick_into(&mut out, 1.0);
        assert_eq!(out, [0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn tick_into_half_rate_interpolates() {
        let mut deck = DeckBuffer::new(vec![0.0, 1.0, 2.0, 3.0], 1);
        let mut out = [0.0; 4];
        deck.tick_into(&mut out, 0.5);
        assert_eq!(out, [0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn loop_region_preserves_fractional_remainder() {
        let mut deck = DeckBuffer::new(vec![0.0, 1.0, 2.0, 3.0], 1);
        deck.set_loop_region(0, 2);
        let mut out = [0.0; 4];
        deck.tick_into(&mut out, 1.5);
        assert_eq!(out, [0.0, 1.5, 1.0, 0.5]);
    }
}
