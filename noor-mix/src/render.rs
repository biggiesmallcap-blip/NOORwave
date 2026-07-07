use crate::automation::param_value_at;
use crate::eq::{BandGains, IsolatorEq};
use crate::limiter::SafetyLimiter;
use crate::program::{DeckId, LoopRegion, Param, TransitionProgram};

pub struct Mixer {
    program: TransitionProgram,
    deck_a: crate::deck::DeckBuffer,
    deck_b: crate::deck::DeckBuffer,
    scratch_a: Vec<f32>,
    scratch_b: Vec<f32>,
    eq_a: IsolatorEq,
    eq_b: IsolatorEq,
    limiter: SafetyLimiter,
}

impl Mixer {
    pub fn new(
        program: TransitionProgram,
        deck_a: crate::deck::DeckBuffer,
        deck_b: crate::deck::DeckBuffer,
        max_block_samples: usize,
    ) -> anyhow::Result<Self> {
        program.validate()?;
        let sample_rate = program.sample_rate;
        let channels = program.channels;
        let mut deck_a = deck_a;
        let mut deck_b = deck_b;
        deck_a.set_position_frame(program.deck_a_start_frame);
        deck_b.set_position_frame(program.deck_b_start_frame);
        apply_loop_regions(&program, &mut deck_a, &mut deck_b);
        Ok(Self {
            program,
            deck_a,
            deck_b,
            scratch_a: vec![0.0; max_block_samples],
            scratch_b: vec![0.0; max_block_samples],
            eq_a: IsolatorEq::new(sample_rate, channels),
            eq_b: IsolatorEq::new(sample_rate, channels),
            limiter: SafetyLimiter::new(0.98),
        })
    }

    pub fn render_block(&mut self, out: &mut [f32], master_sample: u64) {
        assert!(out.len() <= self.scratch_a.len());
        assert!(out.len() <= self.scratch_b.len());

        let Self {
            program,
            deck_a,
            deck_b,
            scratch_a,
            scratch_b,
            eq_a,
            eq_b,
            limiter,
        } = self;
        let scratch_a = &mut scratch_a[..out.len()];
        let scratch_b = &mut scratch_b[..out.len()];
        scratch_a.fill(0.0);
        scratch_b.fill(0.0);

        // Playback-rate events are constant-valued (validated upstream), so a
        // per-block lookup cannot differ from a per-frame one.
        let rate_a = param_at(program, Param::PlaybackRate(DeckId::A), master_sample);
        let rate_b = param_at(program, Param::PlaybackRate(DeckId::B), master_sample);
        deck_a.tick_into(scratch_a, rate_a);
        deck_b.tick_into(scratch_b, rate_b);

        // Gain and EQ automation are evaluated per frame so the rendered audio
        // is independent of how callers slice the render into blocks.
        let channels = usize::from(program.channels);
        for (frame_index, (frame_out, (frame_a, frame_b))) in out
            .chunks_mut(channels)
            .zip(
                scratch_a
                    .chunks_mut(channels)
                    .zip(scratch_b.chunks_mut(channels)),
            )
            .enumerate()
        {
            let sample = master_sample + frame_index as u64;
            eq_a.process_in_place(frame_a, deck_band_gains(program, DeckId::A, sample));
            eq_b.process_in_place(frame_b, deck_band_gains(program, DeckId::B, sample));
            let gain_a = param_at(program, Param::DeckGain(DeckId::A), sample);
            let gain_b = param_at(program, Param::DeckGain(DeckId::B), sample);
            for (sample_out, (sample_a, sample_b)) in
                frame_out.iter_mut().zip(frame_a.iter().zip(frame_b.iter()))
            {
                *sample_out = sample_a * gain_a + sample_b * gain_b;
            }
        }

        limiter.process_in_place(out);
    }

    #[cfg(test)]
    fn scratch_capacities(&self) -> (usize, usize) {
        (self.scratch_a.capacity(), self.scratch_b.capacity())
    }
}

fn apply_loop_regions(
    program: &TransitionProgram,
    deck_a: &mut crate::deck::DeckBuffer,
    deck_b: &mut crate::deck::DeckBuffer,
) {
    for region in &program.loops {
        apply_loop_region(region, deck_a, deck_b);
    }
}

fn apply_loop_region(
    region: &LoopRegion,
    deck_a: &mut crate::deck::DeckBuffer,
    deck_b: &mut crate::deck::DeckBuffer,
) {
    match region.deck {
        DeckId::A => deck_a.set_loop_region(region.start_frame, region.end_frame),
        DeckId::B => deck_b.set_loop_region(region.start_frame, region.end_frame),
    }
}

fn deck_band_gains(program: &TransitionProgram, deck: DeckId, sample: u64) -> BandGains {
    BandGains {
        low: param_at(program, Param::LowGain(deck), sample),
        mid: param_at(program, Param::MidGain(deck), sample),
        high: param_at(program, Param::HighGain(deck), sample),
    }
}

fn param_at(program: &TransitionProgram, param: Param, sample: u64) -> f32 {
    param_value_at(&program.automation, param, sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::DeckBuffer;
    use crate::program::{AutomationEvent, Curve, Tier};

    fn valid_program() -> TransitionProgram {
        TransitionProgram {
            tier: Tier::FullBlend,
            template: "SafeCrossfade".to_string(),
            drop_source: None,
            sample_rate: 48_000,
            channels: 1,
            deck_a_start_frame: 0,
            deck_b_start_frame: 0,
            sync_start: 0,
            intro_start: 0,
            swap_start: 4,
            fade_start: 4,
            resolve_at: 8,
            loops: vec![],
            automation: vec![],
        }
    }

    #[test]
    fn new_rejects_invalid_program() {
        let mut program = valid_program();
        program.sample_rate = 0;
        let result = Mixer::new(
            program,
            DeckBuffer::new(vec![0.0; 8], 1),
            DeckBuffer::new(vec![0.0; 8], 1),
            8,
        );
        assert!(result.is_err());
    }

    #[test]
    fn render_block_sums_two_unity_decks() {
        let mut mixer = Mixer::new(
            valid_program(),
            DeckBuffer::new(vec![0.25; 8], 1),
            DeckBuffer::new(vec![0.5; 8], 1),
            8,
        )
        .expect("mixer");
        let mut out = [0.0; 4];
        mixer.render_block(&mut out, 0);
        assert_eq!(out, [0.75; 4]);
    }

    #[test]
    fn render_block_applies_program_start_frames() {
        let mut program = valid_program();
        program.deck_a_start_frame = 2;
        program.deck_b_start_frame = 4;
        let mut mixer = Mixer::new(
            program,
            DeckBuffer::new(vec![0.0, 0.1, 0.2, 0.3, 0.4], 1),
            DeckBuffer::new(vec![0.0, 0.1, 0.2, 0.3, 0.4], 1),
            8,
        )
        .expect("mixer");
        let mut out = [0.0; 3];

        mixer.render_block(&mut out, 0);

        assert_samples_close(&out, &[0.6, 0.7, 0.8]);
    }

    #[test]
    fn render_block_applies_program_loop_regions() {
        let mut program = valid_program();
        program.loops = vec![LoopRegion {
            deck: DeckId::A,
            start_frame: 1,
            end_frame: 3,
        }];
        let mut mixer = Mixer::new(
            program,
            DeckBuffer::new(vec![0.0, 0.1, 0.2, 0.3], 1),
            DeckBuffer::new(vec![0.0; 4], 1),
            8,
        )
        .expect("mixer");
        let mut out = [0.0; 5];

        mixer.render_block(&mut out, 0);

        assert_samples_close(&out, &[0.0, 0.1, 0.2, 0.1, 0.2]);
    }

    #[test]
    fn render_block_applies_equal_power_fade() {
        let mut program = valid_program();
        program.automation = vec![AutomationEvent {
            param: Param::DeckGain(DeckId::B),
            start_sample: 0,
            end_sample: 4,
            from: 0.0,
            to: 1.0,
            curve: Curve::EqualPowerIn,
        }];
        let mut mixer = Mixer::new(
            program,
            DeckBuffer::new(vec![0.4; 8], 1),
            DeckBuffer::new(vec![0.4; 8], 1),
            8,
        )
        .expect("mixer");
        let mut out = [0.0; 5];
        mixer.render_block(&mut out, 0);
        assert!((out[0] - 0.4).abs() < 1e-6);
        assert!(out[2] > out[0]);
        assert!((out[4] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn render_block_never_exceeds_limiter_ceiling() {
        let mut mixer = Mixer::new(
            valid_program(),
            DeckBuffer::new(vec![1.0; 8], 1),
            DeckBuffer::new(vec![1.0; 8], 1),
            8,
        )
        .expect("mixer");
        let mut out = [0.0; 4];
        mixer.render_block(&mut out, 0);
        assert!(out.iter().all(|sample| sample.abs() <= 0.98));
    }

    fn sine(freq: f32, frames: usize, sample_rate: u32) -> Vec<f32> {
        (0..frames)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|value| value * value).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn assert_samples_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (*actual - *expected).abs() < 1e-6,
                "expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn render_block_applies_low_gain_automation() {
        let sample_rate = 48_000;
        let frames = sample_rate as usize * 3;
        let source = sine(50.0, frames, sample_rate);
        let mut program = valid_program();
        program.sample_rate = sample_rate;
        program.resolve_at = frames as u64;
        program.swap_start = frames as u64 / 2;
        program.fade_start = program.swap_start;
        program.automation = vec![
            AutomationEvent {
                param: Param::DeckGain(DeckId::A),
                start_sample: 0,
                end_sample: frames as u64,
                from: 1.0,
                to: 1.0,
                curve: Curve::Linear,
            },
            AutomationEvent {
                param: Param::LowGain(DeckId::A),
                start_sample: 0,
                end_sample: frames as u64,
                from: 0.0,
                to: 0.0,
                curve: Curve::Linear,
            },
        ];
        let mut mixer = Mixer::new(
            program,
            DeckBuffer::new(source.clone(), 1),
            DeckBuffer::new(vec![0.0; frames], 1),
            frames,
        )
        .expect("mixer");
        let mut out = vec![0.0; frames];

        mixer.render_block(&mut out, 0);

        let input_tail = rms(&source[sample_rate as usize * 2..]);
        let output_tail = rms(&out[sample_rate as usize * 2..]);
        assert!(output_tail / input_tail < 0.05);
    }

    #[test]
    fn render_is_independent_of_block_segmentation() {
        let sample_rate = 48_000;
        let frames = 4_800_usize;
        let mut program = valid_program();
        program.sample_rate = sample_rate;
        program.resolve_at = frames as u64;
        program.swap_start = frames as u64 / 2;
        program.fade_start = program.swap_start;
        // Ramped EQ + deck gains: the automation shapes that used to be
        // quantized to block boundaries.
        program.automation = vec![
            AutomationEvent {
                param: Param::DeckGain(DeckId::A),
                start_sample: 0,
                end_sample: frames as u64,
                from: 1.0,
                to: 0.0,
                curve: Curve::EqualPowerOut,
            },
            AutomationEvent {
                param: Param::DeckGain(DeckId::B),
                start_sample: 0,
                end_sample: frames as u64,
                from: 0.0,
                to: 1.0,
                curve: Curve::EqualPowerIn,
            },
            AutomationEvent {
                param: Param::LowGain(DeckId::A),
                start_sample: 0,
                end_sample: frames as u64,
                from: 1.0,
                to: 0.05,
                curve: Curve::Cosine,
            },
            AutomationEvent {
                param: Param::LowGain(DeckId::B),
                start_sample: frames as u64 / 2,
                end_sample: frames as u64,
                from: 0.0,
                to: 1.0,
                curve: Curve::Cosine,
            },
        ];
        let deck_a = sine(80.0, frames, sample_rate);
        let deck_b = sine(220.0, frames, sample_rate);

        let render = |block_frames: usize| {
            let mut mixer = Mixer::new(
                program.clone(),
                DeckBuffer::new(deck_a.clone(), 1),
                DeckBuffer::new(deck_b.clone(), 1),
                frames,
            )
            .expect("mixer");
            let mut out = vec![0.0; frames];
            let mut master = 0_u64;
            for block in out.chunks_mut(block_frames) {
                mixer.render_block(block, master);
                master += block.len() as u64;
            }
            out
        };

        let whole = render(frames);
        let small_blocks = render(128);
        let odd_blocks = render(313);
        assert_eq!(whole, small_blocks);
        assert_eq!(whole, odd_blocks);
    }

    #[test]
    fn render_block_reuses_scratch_capacity() {
        let mut mixer = Mixer::new(
            valid_program(),
            DeckBuffer::new(vec![0.25; 16], 1),
            DeckBuffer::new(vec![0.25; 16], 1),
            16,
        )
        .expect("mixer");
        let capacities = mixer.scratch_capacities();
        let mut out = [0.0; 8];
        mixer.render_block(&mut out, 0);
        mixer.render_block(&mut out, 8);
        assert_eq!(mixer.scratch_capacities(), capacities);
    }
}
