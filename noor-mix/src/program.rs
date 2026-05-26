use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeckId {
    A,
    B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    SafeCrossfade,
    FullBlend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Param {
    DeckGain(DeckId),
    LowGain(DeckId),
    MidGain(DeckId),
    HighGain(DeckId),
    PlaybackRate(DeckId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    Linear,
    EqualPowerIn,
    EqualPowerOut,
    Cosine,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationEvent {
    pub param: Param,
    pub start_sample: u64,
    pub end_sample: u64,
    pub from: f32,
    pub to: f32,
    pub curve: Curve,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopRegion {
    pub deck: DeckId,
    #[serde(default, alias = "start_sample")]
    pub start_frame: u64,
    #[serde(default, alias = "end_sample")]
    pub end_frame: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionProgram {
    pub tier: Tier,
    pub template: String,
    pub sample_rate: u32,
    pub channels: u16,
    #[serde(default)]
    pub deck_a_start_frame: u64,
    #[serde(default)]
    pub deck_b_start_frame: u64,
    pub sync_start: u64,
    pub intro_start: u64,
    pub swap_start: u64,
    pub fade_start: u64,
    pub resolve_at: u64,
    pub loops: Vec<LoopRegion>,
    pub automation: Vec<AutomationEvent>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ProgramError {
    #[error("sample_rate must be greater than zero")]
    InvalidSampleRate,
    #[error("channels must be greater than zero")]
    InvalidChannels,
    #[error("phase markers must be monotonic")]
    NonMonotonicMarkers,
    #[error("automation event has start >= end")]
    EmptyAutomationEvent,
    #[error("automation value is outside the allowed range")]
    AutomationValueOutOfRange,
    #[error("playback-rate automation exceeds max stretch")]
    PlaybackRateOutOfRange,
    #[error("loop region is empty")]
    EmptyLoopRegion,
}

impl TransitionProgram {
    pub fn validate(&self) -> Result<(), ProgramError> {
        if self.sample_rate == 0 {
            return Err(ProgramError::InvalidSampleRate);
        }
        if self.channels == 0 {
            return Err(ProgramError::InvalidChannels);
        }
        if !(self.sync_start <= self.intro_start
            && self.intro_start <= self.swap_start
            && self.swap_start <= self.fade_start
            && self.fade_start <= self.resolve_at)
        {
            return Err(ProgramError::NonMonotonicMarkers);
        }
        for region in &self.loops {
            if region.start_frame >= region.end_frame {
                return Err(ProgramError::EmptyLoopRegion);
            }
        }
        for event in &self.automation {
            if event.start_sample >= event.end_sample {
                return Err(ProgramError::EmptyAutomationEvent);
            }
            if !value_allowed(event.param, event.from) || !value_allowed(event.param, event.to) {
                return Err(ProgramError::AutomationValueOutOfRange);
            }
            if matches!(event.param, Param::PlaybackRate(_))
                && (!rate_allowed(event.from) || !rate_allowed(event.to))
            {
                return Err(ProgramError::PlaybackRateOutOfRange);
            }
        }
        Ok(())
    }
}

fn value_allowed(param: Param, value: f32) -> bool {
    value.is_finite()
        && match param {
            Param::DeckGain(_) | Param::LowGain(_) | Param::MidGain(_) | Param::HighGain(_) => {
                (0.0..=1.25).contains(&value)
            }
            Param::PlaybackRate(_) => (0.97..=1.03).contains(&value),
        }
}

fn rate_allowed(value: f32) -> bool {
    value.is_finite() && (0.97..=1.03).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_program() -> TransitionProgram {
        TransitionProgram {
            tier: Tier::FullBlend,
            template: "BassSwap16".to_string(),
            sample_rate: 48_000,
            channels: 2,
            deck_a_start_frame: 0,
            deck_b_start_frame: 0,
            sync_start: 0,
            intro_start: 48_000,
            swap_start: 96_000,
            fade_start: 144_000,
            resolve_at: 192_000,
            loops: vec![],
            automation: vec![],
        }
    }

    #[test]
    fn validate_accepts_valid_program() {
        valid_program().validate().expect("valid program");
    }

    #[test]
    fn validate_rejects_non_monotonic_markers() {
        let mut p = valid_program();
        p.swap_start = p.intro_start - 1;
        assert_eq!(p.validate(), Err(ProgramError::NonMonotonicMarkers));
    }

    #[test]
    fn validate_rejects_empty_automation_event() {
        let mut p = valid_program();
        p.automation.push(AutomationEvent {
            param: Param::DeckGain(DeckId::A),
            start_sample: 10,
            end_sample: 10,
            from: 1.0,
            to: 0.0,
            curve: Curve::Linear,
        });
        assert_eq!(p.validate(), Err(ProgramError::EmptyAutomationEvent));
    }

    #[test]
    fn validate_rejects_gain_out_of_range() {
        let mut p = valid_program();
        p.automation.push(AutomationEvent {
            param: Param::LowGain(DeckId::A),
            start_sample: 10,
            end_sample: 20,
            from: 1.0,
            to: 2.0,
            curve: Curve::Cosine,
        });
        assert_eq!(p.validate(), Err(ProgramError::AutomationValueOutOfRange));
    }

    #[test]
    fn validate_rejects_rate_out_of_range() {
        let mut p = valid_program();
        p.automation.push(AutomationEvent {
            param: Param::PlaybackRate(DeckId::B),
            start_sample: 10,
            end_sample: 20,
            from: 1.0,
            to: 1.04,
            curve: Curve::Linear,
        });
        assert_eq!(p.validate(), Err(ProgramError::AutomationValueOutOfRange));
    }
}
