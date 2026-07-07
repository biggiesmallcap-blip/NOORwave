use crate::automation::param_value_at;
use crate::program::{DeckId, Param, ProgramError, Tier, TransitionProgram};

#[derive(Debug, Clone)]
pub struct AudioSafetyPolicy {
    pub max_pitch_shift_pct: f32,
    pub peak_ceiling_dbfs: f32,
    pub max_loudness_jump_lu: f32,
    pub max_low_band_overlap: f32,
}

impl Default for AudioSafetyPolicy {
    fn default() -> Self {
        Self {
            max_pitch_shift_pct: 3.0,
            peak_ceiling_dbfs: -1.0,
            max_loudness_jump_lu: 4.0,
            max_low_band_overlap: 1.25,
        }
    }
}

pub fn validate_audio_safety(
    program: &TransitionProgram,
    policy: &AudioSafetyPolicy,
) -> Result<(), ProgramError> {
    program.validate()?;
    let max_delta = policy.max_pitch_shift_pct / 100.0;
    for event in &program.automation {
        if matches!(event.param, Param::PlaybackRate(_))
            && ((event.from - 1.0).abs() > max_delta || (event.to - 1.0).abs() > max_delta)
        {
            return Err(ProgramError::PlaybackRateOutOfRange);
        }
    }
    if matches!(program.tier, Tier::FullBlend)
        && max_low_band_overlap(program) > policy.max_low_band_overlap + 1e-3
    {
        return Err(ProgramError::LowBandOverlapExceeded);
    }
    Ok(())
}

/// Peak simultaneous low-band ownership across the transition: at each probe
/// point the effective low contribution of a deck is deck_gain * low_gain, and
/// the overlap is the energy sum sqrt(a^2 + b^2). An equal-power crossfade
/// with untouched EQ measures 1.0; two decks playing full low end at full gain
/// measure sqrt(2) and must be rejected as mud.
fn max_low_band_overlap(program: &TransitionProgram) -> f32 {
    const UNIFORM_PROBES: u64 = 128;
    let last_sample = program.resolve_at.saturating_sub(1);
    let mut probes = vec![0, last_sample];
    for event in &program.automation {
        probes.push(event.start_sample.min(last_sample));
        probes.push(event.end_sample.min(last_sample));
        probes.push(event.end_sample.saturating_sub(1).min(last_sample));
    }
    for step in 0..=UNIFORM_PROBES {
        probes.push(program.resolve_at.saturating_mul(step) / UNIFORM_PROBES.max(1));
    }
    probes.sort_unstable();
    probes.dedup();

    probes
        .into_iter()
        .map(|sample| {
            let low_a = effective_low(program, DeckId::A, sample);
            let low_b = effective_low(program, DeckId::B, sample);
            (low_a * low_a + low_b * low_b).sqrt()
        })
        .fold(0.0_f32, f32::max)
}

fn effective_low(program: &TransitionProgram, deck: DeckId, sample: u64) -> f32 {
    param_value_at(&program.automation, Param::DeckGain(deck), sample)
        * param_value_at(&program.automation, Param::LowGain(deck), sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{AutomationEvent, Curve, DeckId, Param, Tier};

    fn valid_safe_crossfade() -> TransitionProgram {
        TransitionProgram {
            tier: Tier::SafeCrossfade,
            template: "SafeCrossfade".to_string(),
            drop_source: None,
            sample_rate: 48_000,
            channels: 2,
            deck_a_start_frame: 0,
            deck_b_start_frame: 0,
            sync_start: 0,
            intro_start: 0,
            swap_start: 96_000,
            fade_start: 96_000,
            resolve_at: 192_000,
            loops: vec![],
            automation: vec![AutomationEvent {
                param: Param::PlaybackRate(DeckId::B),
                start_sample: 0,
                end_sample: 192_000,
                from: 1.0,
                to: 1.01,
                curve: Curve::Linear,
            }],
        }
    }

    #[test]
    fn audio_safety_rejects_pitch_shift_above_policy_cap() {
        let program = valid_safe_crossfade();
        let policy = AudioSafetyPolicy {
            max_pitch_shift_pct: 0.5,
            ..AudioSafetyPolicy::default()
        };
        assert_eq!(
            validate_audio_safety(&program, &policy),
            Err(ProgramError::PlaybackRateOutOfRange)
        );
    }

    #[test]
    fn audio_safety_rejects_invalid_automation_range() {
        let mut program = valid_safe_crossfade();
        program.automation[0].to = 1.04;
        assert_eq!(
            validate_audio_safety(&program, &AudioSafetyPolicy::default()),
            Err(ProgramError::AutomationValueOutOfRange)
        );
    }

    #[test]
    fn audio_safety_rejects_non_deterministic_render_fixture() {
        let mut program = valid_safe_crossfade();
        program.automation[0].from = f32::NAN;
        assert_eq!(
            validate_audio_safety(&program, &AudioSafetyPolicy::default()),
            Err(ProgramError::AutomationValueOutOfRange)
        );
    }

    #[test]
    fn audio_safety_accepts_valid_safe_crossfade() {
        validate_audio_safety(&valid_safe_crossfade(), &AudioSafetyPolicy::default())
            .expect("valid safe crossfade");
    }

    #[test]
    fn audio_safety_rejects_full_blend_with_unmanaged_low_end() {
        // Both decks at unity deck gain with untouched low bands: sqrt(2)
        // overlap for the whole program.
        let mut program = valid_safe_crossfade();
        program.tier = Tier::FullBlend;
        program.template = "BassSwap16".to_string();
        program.automation = vec![
            AutomationEvent {
                param: Param::DeckGain(DeckId::A),
                start_sample: 0,
                end_sample: 192_000,
                from: 1.0,
                to: 1.0,
                curve: Curve::Linear,
            },
            AutomationEvent {
                param: Param::DeckGain(DeckId::B),
                start_sample: 0,
                end_sample: 192_000,
                from: 1.0,
                to: 1.0,
                curve: Curve::Linear,
            },
        ];
        assert_eq!(
            validate_audio_safety(&program, &AudioSafetyPolicy::default()),
            Err(ProgramError::LowBandOverlapExceeded)
        );
    }

    #[test]
    fn audio_safety_accepts_planned_full_blend_templates() {
        use crate::planner::{Planner, Policy, TransitionTemplate};
        use crate::profile::{DjProfile, TransitionWindow};

        let profile = |bpm: f32, phrases: usize| DjProfile {
            bpm: Some(bpm),
            camelot_key: Some("8A".to_string()),
            energy: Some(0.5),
            beat_grid_seconds: vec![0.0, 0.5],
            downbeat_seconds: vec![0.0],
            phrase_bar_indices: (0..phrases as u32).collect(),
            mix_in_seconds: vec![0.0],
            mix_out_seconds: vec![60.0],
            intro_end_seconds: Some(16.0),
            outro_start_seconds: Some(180.0),
            breakdown_seconds: vec![],
            drop_seconds: vec![],
            manual_drop_seconds: vec![],
            safe_transition_windows: vec![TransitionWindow {
                start_seconds: 0.0,
                end_seconds: 8.0,
                confidence: 1.0,
            }],
            vocal_presence_by_bar: vec![0.0; phrases.max(1)],
            vocal_density_by_bar: vec![0.0; phrases.max(1)],
            lufs_loud_body: Some(-12.0),
            true_peak_dbtp: Some(-1.0),
            profile_confidence: 1.0,
            safe_crossfade_only: false,
            profile_version: "test".to_string(),
        };

        for template in [
            TransitionTemplate::BassSwap16,
            TransitionTemplate::BassSwap32,
            TransitionTemplate::LongHarmonicBlend,
            TransitionTemplate::FilterSweep,
            TransitionTemplate::DropTease16,
        ] {
            let policy = Policy {
                safety_template_override: Some(template),
                ..Policy::default()
            };
            let program = Planner::plan(&profile(120.0, 4), &profile(121.0, 4), &policy);
            validate_audio_safety(&program, &AudioSafetyPolicy::default())
                .unwrap_or_else(|error| panic!("{template:?} violated audio safety: {error:?}"));
        }
    }
}
