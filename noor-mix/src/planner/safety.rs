use crate::program::{Param, ProgramError, TransitionProgram};

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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{AutomationEvent, Curve, DeckId, Param, Tier};

    fn valid_safe_crossfade() -> TransitionProgram {
        TransitionProgram {
            tier: Tier::SafeCrossfade,
            template: "SafeCrossfade".to_string(),
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
}
