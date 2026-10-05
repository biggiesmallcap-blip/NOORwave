use super::template::TransitionTemplate;

pub const DJ_PLANNER_VERSION: &str = "dj_planner_v2";

#[derive(Debug, Clone)]
pub struct Policy {
    pub max_pitch_shift_pct: f32,
    pub energy_step_max: f32,
    pub default_crossfade_ms: u32,
    pub transition_speed_bias: TransitionSpeedBias,
    pub mix_intent: MixIntent,
    pub safety_template_override: Option<TransitionTemplate>,
    pub require_full_profile: bool,
    pub preferred_strategy: String,
    pub recent_templates: Vec<String>,
    pub strategy_feedback: Vec<(String, f32)>,
    pub adventurousness_bias: f32,
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
            preferred_strategy: "adaptive".to_string(),
            recent_templates: Vec::new(),
            strategy_feedback: Vec::new(),
            adventurousness_bias: 0.0,
        }
    }
}
