#[derive(Debug, Clone, Copy)]
pub struct BandGains {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

pub struct IsolatorEq {
    sample_rate: u32,
    channels: u16,
    low_hp_state: Vec<[f32; 4]>,
    low_hp_prev_input: Vec<[f32; 4]>,
    high_state: Vec<f32>,
    high_prev_input: Vec<f32>,
}

impl IsolatorEq {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let channels = channels.max(1);
        Self {
            sample_rate: sample_rate.max(1),
            channels,
            low_hp_state: vec![[0.0; 4]; usize::from(channels)],
            low_hp_prev_input: vec![[0.0; 4]; usize::from(channels)],
            high_state: vec![0.0; usize::from(channels)],
            high_prev_input: vec![0.0; usize::from(channels)],
        }
    }

    pub fn process_in_place(&mut self, block: &mut [f32], gains: BandGains) {
        let low_gain = sanitize_gain(gains.low);
        let mid_gain = sanitize_gain(gains.mid);
        let high_gain = sanitize_gain(gains.high);
        let low_alpha = one_pole_high_alpha(220.0, self.sample_rate);
        let high_alpha = one_pole_high_alpha(4_000.0, self.sample_rate);
        let channels = usize::from(self.channels);

        for frame in block.chunks_mut(channels) {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let input = *sample;
                let low_remainder = self.low_removed_signal(input, channel, low_alpha);

                let high = high_alpha
                    * (self.high_state[channel] + low_remainder - self.high_prev_input[channel]);
                self.high_prev_input[channel] = low_remainder;
                self.high_state[channel] = high;

                let low = input - low_remainder;
                let mid = low_remainder - high;
                *sample = low * low_gain + mid * mid_gain + high * high_gain;
            }
        }
    }

    fn low_removed_signal(&mut self, input: f32, channel: usize, alpha: f32) -> f32 {
        let mut stage_input = input;
        for stage in 0..4 {
            let output = alpha
                * (self.low_hp_state[channel][stage] + stage_input
                    - self.low_hp_prev_input[channel][stage]);
            self.low_hp_prev_input[channel][stage] = stage_input;
            self.low_hp_state[channel][stage] = output;
            stage_input = output;
        }
        stage_input
    }
}

fn sanitize_gain(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn one_pole_high_alpha(cutoff_hz: f32, sample_rate: u32) -> f32 {
    let dt = 1.0 / sample_rate as f32;
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
    rc / (rc + dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, seconds: f32, sample_rate: u32) -> Vec<f32> {
        let samples = (seconds * sample_rate as f32) as usize;
        (0..samples)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn low_kill_reduces_50_hz_sine_by_40_db() {
        let sample_rate = 48_000;
        let mut block = sine(50.0, 3.0, sample_rate);
        let input_tail = rms(&block[96_000..]);
        let mut eq = IsolatorEq::new(sample_rate, 1);
        eq.process_in_place(
            &mut block,
            BandGains {
                low: 0.0,
                mid: 1.0,
                high: 1.0,
            },
        );
        let output_tail = rms(&block[96_000..]);
        assert!(output_tail / input_tail < 0.01);
    }

    #[test]
    fn unity_gains_do_not_change_rms_by_more_than_one_db() {
        let sample_rate = 48_000;
        let mut block = sine(1_000.0, 1.0, sample_rate);
        let input = rms(&block);
        let mut eq = IsolatorEq::new(sample_rate, 1);
        eq.process_in_place(
            &mut block,
            BandGains {
                low: 1.0,
                mid: 1.0,
                high: 1.0,
            },
        );
        let output = rms(&block);
        let db = 20.0 * (output / input).log10().abs();
        assert!(db < 1.0, "rms changed by {db} dB");
    }

    #[test]
    fn nonfinite_gain_is_treated_as_zero() {
        let mut block = vec![0.5; 128];
        let mut zeroed = block.clone();
        let mut eq = IsolatorEq::new(48_000, 1);
        eq.process_in_place(
            &mut block,
            BandGains {
                low: f32::NAN,
                mid: f32::INFINITY,
                high: f32::NEG_INFINITY,
            },
        );
        let mut reference = IsolatorEq::new(48_000, 1);
        reference.process_in_place(
            &mut zeroed,
            BandGains {
                low: 0.0,
                mid: 0.0,
                high: 0.0,
            },
        );
        assert_eq!(block, zeroed);
    }
}
