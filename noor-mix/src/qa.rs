use crate::deck::DeckBuffer;
use crate::program::TransitionProgram;
use crate::render::Mixer;

#[derive(Debug, Clone, Copy)]
pub struct TransitionQaThresholds {
    pub peak_ceiling: f32,
    pub click_jump: f32,
    pub loudness_jump_db: f32,
    pub dc_offset: f32,
}

impl Default for TransitionQaThresholds {
    fn default() -> Self {
        Self {
            peak_ceiling: 1.0,
            click_jump: 0.7,
            loudness_jump_db: 6.0,
            dc_offset: 0.05,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionQaReport {
    pub sample_count: usize,
    pub finite: bool,
    pub peak: f32,
    pub peak_ok: bool,
    pub max_derivative_jump: f32,
    pub click_risk: bool,
    pub loudness_jump_db: f32,
    pub loudness_jump_ok: bool,
    pub dc_offset: f32,
    pub dc_offset_ok: bool,
    pub deterministic_hash: u64,
    pub deterministic: bool,
}

impl TransitionQaReport {
    pub fn passed(&self) -> bool {
        self.sample_count > 0
            && self.finite
            && self.peak_ok
            && !self.click_risk
            && self.loudness_jump_ok
            && self.dc_offset_ok
            && self.deterministic
    }
}

pub fn render_transition_qa(
    program: &TransitionProgram,
    deck_a: &[f32],
    deck_b: &[f32],
) -> anyhow::Result<TransitionQaReport> {
    let first = render_program(program, deck_a, deck_b)?;
    let second = render_program(program, deck_a, deck_b)?;
    Ok(analyze_rendered_transition(
        &first,
        Some(&second),
        usize::from(program.channels.max(1)),
        TransitionQaThresholds::default(),
    ))
}

pub fn analyze_rendered_transition(
    samples: &[f32],
    repeated_render: Option<&[f32]>,
    channels: usize,
    thresholds: TransitionQaThresholds,
) -> TransitionQaReport {
    let channels = channels.max(1);
    let finite = samples.iter().all(|sample| sample.is_finite());
    let peak = samples
        .iter()
        .fold(0.0_f32, |acc, sample| acc.max(sample.abs()));
    let max_derivative_jump = derivative_jump(samples, channels);
    let loudness_jump_db = loudness_jump_db(samples);
    let dc_offset = mean(samples).abs();
    let deterministic_hash = stable_hash(samples);
    let deterministic = repeated_render
        .map(|other| other.len() == samples.len() && stable_hash(other) == deterministic_hash)
        .unwrap_or(true);

    TransitionQaReport {
        sample_count: samples.len(),
        finite,
        peak,
        peak_ok: finite && peak <= thresholds.peak_ceiling,
        max_derivative_jump,
        click_risk: !finite || max_derivative_jump > thresholds.click_jump,
        loudness_jump_db,
        loudness_jump_ok: finite && loudness_jump_db <= thresholds.loudness_jump_db,
        dc_offset,
        dc_offset_ok: finite && dc_offset <= thresholds.dc_offset,
        deterministic_hash,
        deterministic,
    }
}

fn render_program(
    program: &TransitionProgram,
    deck_a: &[f32],
    deck_b: &[f32],
) -> anyhow::Result<Vec<f32>> {
    let channels = usize::from(program.channels.max(1));
    let frames = program.resolve_at.max(1) as usize;
    let sample_count = frames.saturating_mul(channels);
    let mut mixer = Mixer::new(
        program.clone(),
        DeckBuffer::new(deck_a.to_vec(), program.channels),
        DeckBuffer::new(deck_b.to_vec(), program.channels),
        sample_count,
    )?;
    let mut out = vec![0.0; sample_count];
    mixer.render_block(&mut out, 0);
    Ok(out)
}

fn derivative_jump(samples: &[f32], channels: usize) -> f32 {
    if samples.len() <= channels {
        return 0.0;
    }
    let mut max_jump = 0.0_f32;
    for frame in samples
        .chunks(channels)
        .zip(samples.chunks(channels).skip(1))
    {
        let (current, next) = frame;
        for (a, b) in current.iter().zip(next.iter()) {
            max_jump = max_jump.max((b - a).abs());
        }
    }
    max_jump
}

fn loudness_jump_db(samples: &[f32]) -> f32 {
    if samples.len() < 4 {
        return 0.0;
    }
    let window = (samples.len() / 4).max(1);
    let first = rms(&samples[..window]).max(1e-9);
    let last = rms(&samples[samples.len() - window..]).max(1e-9);
    (20.0 * (last / first).log10()).abs()
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn mean(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f32>() / samples.len() as f32
}

fn stable_hash(samples: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in samples {
        hash ^= u64::from(sample.to_bits());
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{AutomationEvent, Curve, DeckId, Param, Tier};

    fn safe_crossfade_program(
        sample_rate: u32,
        channels: u16,
        duration_ms: u32,
    ) -> TransitionProgram {
        let duration_samples = u64::from(duration_ms) * u64::from(sample_rate) / 1_000;
        TransitionProgram {
            tier: Tier::SafeCrossfade,
            template: "SafeCrossfade".to_string(),
            sample_rate,
            channels,
            deck_a_start_frame: 0,
            deck_b_start_frame: 0,
            sync_start: 0,
            intro_start: 0,
            swap_start: duration_samples / 2,
            fade_start: duration_samples / 2,
            resolve_at: duration_samples,
            loops: vec![],
            automation: vec![
                AutomationEvent {
                    param: Param::DeckGain(DeckId::A),
                    start_sample: 0,
                    end_sample: duration_samples,
                    from: 1.0,
                    to: 0.0,
                    curve: Curve::EqualPowerIn,
                },
                AutomationEvent {
                    param: Param::DeckGain(DeckId::B),
                    start_sample: 0,
                    end_sample: duration_samples,
                    from: 0.0,
                    to: 1.0,
                    curve: Curve::EqualPowerIn,
                },
            ],
        }
    }

    fn sine(freq: f32, frames: usize, sample_rate: u32) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin() * 0.35
            })
            .collect()
    }

    #[test]
    fn safe_crossfade_qa_passes_for_generated_sines() {
        let sample_rate = 48_000;
        let program = safe_crossfade_program(sample_rate, 1, 1_000);
        let frames = program.resolve_at as usize + sample_rate as usize;
        let report = render_transition_qa(
            &program,
            &sine(220.0, frames, sample_rate),
            &sine(330.0, frames, sample_rate),
        )
        .expect("qa render");

        assert!(report.passed(), "{report:?}");
        assert!(report.peak <= 1.0);
        assert!(report.deterministic);
    }

    #[test]
    fn qa_fails_peak_ceiling_breach() {
        let report = analyze_rendered_transition(
            &[0.0, 1.2, -1.1],
            None,
            1,
            TransitionQaThresholds::default(),
        );

        assert!(!report.peak_ok);
        assert!(!report.passed());
    }

    #[test]
    fn qa_fails_non_finite_or_empty_render() {
        let non_finite = analyze_rendered_transition(
            &[0.0, f32::NAN],
            None,
            1,
            TransitionQaThresholds::default(),
        );
        let empty = analyze_rendered_transition(&[], None, 1, TransitionQaThresholds::default());

        assert!(!non_finite.finite);
        assert!(!non_finite.passed());
        assert_eq!(empty.sample_count, 0);
        assert!(!empty.passed());
    }

    #[test]
    fn qa_flags_click_sized_derivative_jump() {
        let report = analyze_rendered_transition(
            &[0.0, 0.0, 1.0, 1.0],
            None,
            1,
            TransitionQaThresholds::default(),
        );

        assert!(report.click_risk);
        assert!(!report.passed());
    }

    #[test]
    fn qa_flags_dc_offset() {
        let report =
            analyze_rendered_transition(&[0.2; 64], None, 1, TransitionQaThresholds::default());

        assert!(!report.dc_offset_ok);
        assert!(!report.passed());
    }

    #[test]
    fn qa_flags_loudness_jump() {
        let samples = [vec![0.01; 32], vec![0.9; 32]].concat();
        let report =
            analyze_rendered_transition(&samples, None, 1, TransitionQaThresholds::default());

        assert!(!report.loudness_jump_ok);
        assert!(!report.passed());
    }

    #[test]
    fn qa_flags_non_deterministic_repeat_render() {
        let first = [0.0, 0.25, 0.5, 0.25];
        let second = [0.0, 0.25, 0.51, 0.25];
        let report = analyze_rendered_transition(
            &first,
            Some(&second),
            1,
            TransitionQaThresholds::default(),
        );

        assert!(!report.deterministic);
        assert!(!report.passed());
    }
}
