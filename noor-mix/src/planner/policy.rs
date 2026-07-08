use super::template::TransitionTemplate;

pub const DJ_PLANNER_VERSION: &str = "dj_planner_v1";

#[derive(Debug, Clone)]
pub struct Policy {
    pub max_pitch_shift_pct: f32,
    pub energy_step_max: f32,
    pub default_crossfade_ms: u32,
    pub transition_speed_bias: TransitionSpeedBias,
    pub mix_intent: MixIntent,
    pub safety_template_override: Option<TransitionTemplate>,
    pub require_full_profile: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixIntent {
    Safe,
    Balanced,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionSpeedBias {
    Slower,
    Neutral,
    Faster,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_pitch_shift_pct: 3.0,
            energy_step_max: 0.15,
            // SafeCrossfade duration. 12s of two full-spectrum tracks
            // overlapping reads as mud; 6s is long enough to feel mixed and
            // short enough that the tracks stop fighting.
            default_crossfade_ms: 6_000,
            transition_speed_bias: TransitionSpeedBias::Neutral,
            mix_intent: MixIntent::Balanced,
            safety_template_override: None,
            require_full_profile: false,
        }
    }
}
