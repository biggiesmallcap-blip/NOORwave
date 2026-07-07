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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_source: Option<String>,
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
    #[error("both decks carry the low band beyond the overlap limit")]
    LowBandOverlapExceeded,
}

impl TransitionProgram {
    /// Re-express every frame-denominated field at `sample_rate`. Programs are
    /// planned at a fixed planning rate but consumed against deck buffers
    /// decoded at the live device rate; interpreting frames across a rate
    /// mismatch shifts every marker (and deck B's sync start) by the ratio of
    /// the two rates. Scaling is round-to-nearest and monotonic, and events
    /// that were non-empty stay non-empty.
    pub fn rescaled_to(mut self, sample_rate: u32) -> Self {
        let to_rate = u64::from(sample_rate.max(1));
        let from_rate = u64::from(self.sample_rate.max(1));
        if to_rate == from_rate {
            self.sample_rate = sample_rate.max(1);
            return self;
        }
        let scale = |frame: u64| -> u64 {
            ((u128::from(frame) * u128::from(to_rate) + u128::from(from_rate) / 2)
                / u128::from(from_rate)) as u64
        };
        self.deck_a_start_frame = scale(self.deck_a_start_frame);
        self.deck_b_start_frame = scale(self.deck_b_start_frame);
        self.sync_start = scale(self.sync_start);
        self.intro_start = scale(self.intro_start);
        self.swap_start = scale(self.swap_start);
        self.fade_start = scale(self.fade_start);
        self.resolve_at = scale(self.resolve_at).max(1);
        for region in &mut self.loops {
            let was_non_empty = region.end_frame > region.start_frame;
            let start = scale(region.start_frame);
            let end = scale(region.end_frame);
            region.start_frame = start;
            region.end_frame = if was_non_empty {
                end.max(start + 1)
            } else {
                end
            };
        }
        for event in &mut self.automation {
            let was_non_empty = event.end_sample > event.start_sample;
            let start = scale(event.start_sample);
            let end = scale(event.end_sample);
            event.start_sample = start;
            event.end_sample = if was_non_empty {
                end.max(start + 1)
            } else {
                end
            };
        }
        self.sample_rate = sample_rate.max(1);
        self
    }

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
            drop_source: None,
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
    fn rescaled_to_scales_every_frame_field() {
        let mut p = valid_program();
        p.deck_b_start_frame = 384_000; // 8.0s at 48k
        p.loops = vec![LoopRegion {
            deck: DeckId::B,
            start_frame: 48_000,
            end_frame: 96_000,
        }];
        p.automation = vec![AutomationEvent {
            param: Param::DeckGain(DeckId::A),
            start_sample: 0,
            end_sample: 192_000,
            from: 1.0,
            to: 0.0,
            curve: Curve::EqualPowerOut,
        }];

        let scaled = p.rescaled_to(44_100);

        assert_eq!(scaled.sample_rate, 44_100);
        assert_eq!(scaled.deck_b_start_frame, 352_800); // still 8.0s
        assert_eq!(scaled.intro_start, 44_100);
        assert_eq!(scaled.swap_start, 88_200);
        assert_eq!(scaled.fade_start, 132_300);
        assert_eq!(scaled.resolve_at, 176_400);
        assert_eq!(scaled.loops[0].start_frame, 44_100);
        assert_eq!(scaled.loops[0].end_frame, 88_200);
        assert_eq!(scaled.automation[0].end_sample, 176_400);
        scaled.validate().expect("scaled program stays valid");
    }

    #[test]
    fn rescaled_to_same_rate_is_identity() {
        let p = valid_program();
        let scaled = p.clone().rescaled_to(48_000);
        assert_eq!(scaled, p);
    }

    #[test]
    fn rescaled_to_keeps_non_empty_events_non_empty() {
        let mut p = valid_program();
        p.automation = vec![AutomationEvent {
            param: Param::DeckGain(DeckId::A),
            start_sample: 100_000,
            end_sample: 100_001,
            from: 1.0,
            to: 0.0,
            curve: Curve::Linear,
        }];
        let scaled = p.rescaled_to(8_000);
        assert!(scaled.automation[0].end_sample > scaled.automation[0].start_sample);
        scaled.validate().expect("valid after downscale");
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
