pub mod policy;
pub mod safety;
pub mod scoring;
pub mod template;

use crate::profile::DjProfile;
use crate::program::{AutomationEvent, Curve, DeckId, Param, Tier, TransitionProgram};

pub use policy::{DJ_PLANNER_VERSION, MixIntent, Policy, TransitionSpeedBias};
pub use template::TransitionTemplate;

pub struct Planner;

impl Planner {
    pub fn choose_template(
        outgoing: &DjProfile,
        incoming: &DjProfile,
        policy: &Policy,
    ) -> TransitionTemplate {
        if let Some(template) = policy.safety_template_override {
            return template;
        }
        if policy.require_full_profile
            && (!outgoing.has_full_dj_profile() || !incoming.has_full_dj_profile())
        {
            return TransitionTemplate::SafeCrossfade;
        }

        let (Some(outgoing_bpm), Some(incoming_bpm)) = (outgoing.bpm, incoming.bpm) else {
            return TransitionTemplate::SafeCrossfade;
        };
        let bpm_delta = scoring::bpm_delta_pct(outgoing_bpm, incoming_bpm);
        if matches!(policy.mix_intent, MixIntent::Bold)
            && !outgoing.phrase_bar_indices.is_empty()
            && !incoming.phrase_bar_indices.is_empty()
        {
            return TransitionTemplate::FilterSweep;
        }

        if bpm_delta > 8.0 {
            return TransitionTemplate::SlamCut;
        }

        let Some(camelot_distance) = outgoing
            .camelot_key
            .as_deref()
            .zip(incoming.camelot_key.as_deref())
            .and_then(|(a, b)| scoring::camelot_distance(a, b))
        else {
            return TransitionTemplate::SafeCrossfade;
        };
        if camelot_distance > 7 {
            return TransitionTemplate::SafeCrossfade;
        }

        if matches!(policy.mix_intent, MixIntent::Safe)
            && !(bpm_delta <= 3.0
                && outgoing.has_full_dj_profile()
                && incoming.has_full_dj_profile()
                && outgoing.phrase_bar_indices.len() >= 2
                && incoming.phrase_bar_indices.len() >= 2
                && matches!(camelot_distance, 0 | 1 | 7))
        {
            return TransitionTemplate::SafeCrossfade;
        }

        let outgoing_phrases = outgoing.phrase_bar_indices.len();
        let incoming_phrases = incoming.phrase_bar_indices.len();
        if outgoing_phrases >= 4 && incoming_phrases >= 4 && bpm_delta <= 3.0 {
            return TransitionTemplate::BassSwap32;
        }
        if outgoing_phrases >= 2 && incoming_phrases >= 2 && bpm_delta <= 3.0 {
            return TransitionTemplate::BassSwap16;
        }
        if matches!(camelot_distance, 0 | 1 | 7) && bpm_delta <= 3.0 {
            return TransitionTemplate::LongHarmonicBlend;
        }
        TransitionTemplate::FilterSweep
    }

    pub fn plan(outgoing: &DjProfile, incoming: &DjProfile, policy: &Policy) -> TransitionProgram {
        let template = Self::choose_template(outgoing, incoming, policy);
        let program = build_program(template, outgoing, incoming, policy);
        match program.validate() {
            Ok(()) => program,
            Err(_) => build_program(
                TransitionTemplate::SafeCrossfade,
                outgoing,
                incoming,
                policy,
            ),
        }
    }
}

fn build_program(
    template: TransitionTemplate,
    outgoing: &DjProfile,
    incoming: &DjProfile,
    policy: &Policy,
) -> TransitionProgram {
    let sample_rate = 48_000;
    let channels = 2;
    let bpm = outgoing.bpm.or(incoming.bpm).unwrap_or(120.0).max(1.0);
    let bar_samples = ((60.0 / bpm) * 4.0 * sample_rate as f32) as u64;
    let duration_samples = duration_samples(template, policy, bar_samples);
    let swap_start = duration_samples / 2;
    let fade_start = swap_start;
    let mut program = TransitionProgram {
        tier: tier_for_template(template),
        template: template_name(template).to_string(),
        sample_rate,
        channels,
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation: deck_gain_automation(duration_samples),
    };

    if matches!(
        template,
        TransitionTemplate::BassSwap16 | TransitionTemplate::BassSwap32
    ) {
        program
            .automation
            .extend(bass_swap_eq_handoff(duration_samples));
    }

    if matches!(template, TransitionTemplate::FilterSweep) {
        program
            .automation
            .extend(filter_sweep_eq_wash(duration_samples));
    }

    if matches!(template, TransitionTemplate::LongHarmonicBlend)
        && outgoing.bpm.zip(incoming.bpm).is_some()
    {
        let rate = incoming.bpm.map(|b| bpm / b).unwrap_or(1.0);
        program.automation.push(AutomationEvent {
            param: Param::PlaybackRate(DeckId::B),
            start_sample: 0,
            end_sample: duration_samples,
            from: 1.0,
            to: rate.clamp(0.97, 1.03),
            curve: Curve::Linear,
        });
    }

    program
}

pub fn bass_swap_16_program(
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
) -> TransitionProgram {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let duration_samples =
        (u64::from(duration_ms).saturating_mul(u64::from(sample_rate)) / 1_000).max(1);
    let swap_start = duration_samples / 2;
    let mut program = TransitionProgram {
        tier: Tier::FullBlend,
        template: "BassSwap16".to_string(),
        sample_rate,
        channels,
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start: swap_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation: deck_gain_automation(duration_samples),
    };
    program
        .automation
        .extend(bass_swap_eq_handoff(duration_samples));
    program
}

pub fn bass_swap_32_program(
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
) -> TransitionProgram {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let duration_samples =
        (u64::from(duration_ms).saturating_mul(u64::from(sample_rate)) / 1_000).max(1);
    let swap_start = duration_samples / 2;
    let mut program = TransitionProgram {
        tier: Tier::FullBlend,
        template: "BassSwap32".to_string(),
        sample_rate,
        channels,
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start: swap_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation: deck_gain_automation(duration_samples),
    };
    program
        .automation
        .extend(bass_swap_eq_handoff(duration_samples));
    program
}

pub fn filter_sweep_eq_wash_program(
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
) -> TransitionProgram {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let duration_samples =
        (u64::from(duration_ms).saturating_mul(u64::from(sample_rate)) / 1_000).max(1);
    let swap_start = duration_samples / 2;
    let mut program = TransitionProgram {
        tier: Tier::FullBlend,
        template: "FilterSweep".to_string(),
        sample_rate,
        channels,
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start: swap_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation: deck_gain_automation(duration_samples),
    };
    program
        .automation
        .extend(filter_sweep_eq_wash(duration_samples));
    program
}

fn duration_samples(template: TransitionTemplate, policy: &Policy, bar_samples: u64) -> u64 {
    match template {
        TransitionTemplate::BassSwap32 => bar_samples * 32,
        TransitionTemplate::BassSwap16 => bar_samples * 16,
        TransitionTemplate::LongHarmonicBlend => bar_samples * 16,
        TransitionTemplate::FilterSweep => bar_samples * 8,
        TransitionTemplate::SlamCut => bar_samples.max(1),
        TransitionTemplate::SafeCrossfade => {
            u64::from(policy.default_crossfade_ms) * 48_000 / 1_000
        }
    }
    .max(1)
}

fn tier_for_template(template: TransitionTemplate) -> Tier {
    match template {
        TransitionTemplate::SafeCrossfade | TransitionTemplate::SlamCut => Tier::SafeCrossfade,
        TransitionTemplate::BassSwap16
        | TransitionTemplate::BassSwap32
        | TransitionTemplate::LongHarmonicBlend
        | TransitionTemplate::FilterSweep => Tier::FullBlend,
    }
}

fn template_name(template: TransitionTemplate) -> &'static str {
    match template {
        TransitionTemplate::SafeCrossfade => "SafeCrossfade",
        TransitionTemplate::SlamCut => "SlamCut",
        TransitionTemplate::BassSwap16 => "BassSwap16",
        TransitionTemplate::BassSwap32 => "BassSwap32",
        TransitionTemplate::LongHarmonicBlend => "LongHarmonicBlend",
        TransitionTemplate::FilterSweep => "FilterSweep",
    }
}

fn deck_gain_automation(duration_samples: u64) -> Vec<AutomationEvent> {
    vec![
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
    ]
}

fn bass_swap_eq_handoff(duration_samples: u64) -> Vec<AutomationEvent> {
    let end_sample = duration_samples.max(1);
    if end_sample < 8 {
        return vec![
            AutomationEvent {
                param: Param::LowGain(DeckId::A),
                start_sample: 0,
                end_sample,
                from: 1.0,
                to: 0.05,
                curve: Curve::Cosine,
            },
            AutomationEvent {
                param: Param::LowGain(DeckId::B),
                start_sample: 0,
                end_sample,
                from: 0.0,
                to: 1.0,
                curve: Curve::Cosine,
            },
        ];
    }

    let swap_point = end_sample / 2;
    let outgoing_low_start = end_sample * 3 / 8;
    let incoming_low_end = (end_sample * 5 / 8).max(swap_point + 1);
    vec![
        AutomationEvent {
            param: Param::LowGain(DeckId::A),
            start_sample: outgoing_low_start,
            end_sample: swap_point.max(outgoing_low_start + 1),
            from: 1.0,
            to: 0.05,
            curve: Curve::Cosine,
        },
        AutomationEvent {
            param: Param::LowGain(DeckId::B),
            start_sample: 0,
            end_sample: swap_point.max(1),
            from: 0.0,
            to: 0.0,
            curve: Curve::Linear,
        },
        AutomationEvent {
            param: Param::LowGain(DeckId::B),
            start_sample: swap_point,
            end_sample: incoming_low_end,
            from: 0.0,
            to: 1.0,
            curve: Curve::Cosine,
        },
        AutomationEvent {
            param: Param::MidGain(DeckId::A),
            start_sample: swap_point,
            end_sample,
            from: 1.0,
            to: 0.35,
            curve: Curve::Cosine,
        },
        AutomationEvent {
            param: Param::HighGain(DeckId::A),
            start_sample: swap_point,
            end_sample,
            from: 1.0,
            to: 0.35,
            curve: Curve::Cosine,
        },
        AutomationEvent {
            param: Param::MidGain(DeckId::B),
            start_sample: 0,
            end_sample,
            from: 0.55,
            to: 1.0,
            curve: Curve::Cosine,
        },
        AutomationEvent {
            param: Param::HighGain(DeckId::B),
            start_sample: 0,
            end_sample,
            from: 0.45,
            to: 1.0,
            curve: Curve::Cosine,
        },
    ]
}

fn filter_sweep_eq_wash(duration_samples: u64) -> Vec<AutomationEvent> {
    let end_sample = duration_samples.max(1);
    let mut events = Vec::new();
    if end_sample < 8 {
        events.push(AutomationEvent {
            param: Param::LowGain(DeckId::A),
            start_sample: 0,
            end_sample,
            from: 1.0,
            to: 0.05,
            curve: Curve::Cosine,
        });
        events.push(AutomationEvent {
            param: Param::LowGain(DeckId::B),
            start_sample: 0,
            end_sample,
            from: 0.0,
            to: 1.0,
            curve: Curve::Cosine,
        });
        events.push(AutomationEvent {
            param: Param::HighGain(DeckId::A),
            start_sample: 0,
            end_sample,
            from: 1.0,
            to: 0.35,
            curve: Curve::Cosine,
        });
        events.push(AutomationEvent {
            param: Param::HighGain(DeckId::B),
            start_sample: 0,
            end_sample,
            from: 0.35,
            to: 1.0,
            curve: Curve::Cosine,
        });
    } else {
        let bass_handoff_start = end_sample * 3 / 8;
        let bass_handoff_end = (end_sample * 5 / 8).max(bass_handoff_start + 1);
        events.push(AutomationEvent {
            param: Param::LowGain(DeckId::A),
            start_sample: bass_handoff_start,
            end_sample: bass_handoff_end,
            from: 1.0,
            to: 0.05,
            curve: Curve::Cosine,
        });
        events.push(AutomationEvent {
            param: Param::LowGain(DeckId::B),
            start_sample: 0,
            end_sample: bass_handoff_start.max(1),
            from: 0.0,
            to: 0.0,
            curve: Curve::Linear,
        });
        events.push(AutomationEvent {
            param: Param::LowGain(DeckId::B),
            start_sample: bass_handoff_start,
            end_sample: bass_handoff_end,
            from: 0.0,
            to: 1.0,
            curve: Curve::Cosine,
        });
        events.push(AutomationEvent {
            param: Param::HighGain(DeckId::A),
            start_sample: bass_handoff_start,
            end_sample: bass_handoff_end,
            from: 1.0,
            to: 0.35,
            curve: Curve::Cosine,
        });
        events.push(AutomationEvent {
            param: Param::HighGain(DeckId::B),
            start_sample: 0,
            end_sample: bass_handoff_start.max(1),
            from: 0.35,
            to: 0.35,
            curve: Curve::Linear,
        });
        events.push(AutomationEvent {
            param: Param::HighGain(DeckId::B),
            start_sample: bass_handoff_start,
            end_sample: bass_handoff_end,
            from: 0.35,
            to: 1.0,
            curve: Curve::Cosine,
        });
    }

    events.extend([
        AutomationEvent {
            param: Param::MidGain(DeckId::A),
            start_sample: 0,
            end_sample,
            from: 1.0,
            to: 0.3,
            curve: Curve::Cosine,
        },
        AutomationEvent {
            param: Param::MidGain(DeckId::B),
            start_sample: 0,
            end_sample,
            from: 0.45,
            to: 1.0,
            curve: Curve::Cosine,
        },
    ]);
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::TransitionWindow;

    fn profile(bpm: Option<f32>, key: Option<&str>, phrase_count: usize) -> DjProfile {
        DjProfile {
            bpm,
            camelot_key: key.map(str::to_string),
            energy: Some(0.5),
            beat_grid_seconds: vec![0.0, 0.5],
            downbeat_seconds: vec![0.0],
            phrase_bar_indices: (0..phrase_count as u32).collect(),
            mix_in_seconds: vec![0.0],
            mix_out_seconds: vec![60.0],
            intro_end_seconds: Some(16.0),
            outro_start_seconds: Some(180.0),
            breakdown_seconds: vec![],
            drop_seconds: vec![],
            safe_transition_windows: vec![TransitionWindow {
                start_seconds: 0.0,
                end_seconds: 8.0,
                confidence: 1.0,
            }],
            vocal_presence_by_bar: vec![0.0; phrase_count.max(1)],
            vocal_density_by_bar: vec![0.0; phrase_count.max(1)],
            lufs_loud_body: Some(-12.0),
            true_peak_dbtp: Some(-1.0),
            profile_confidence: 1.0,
            safe_crossfade_only: false,
            profile_version: "test".to_string(),
        }
    }

    #[test]
    fn choose_template_uses_safety_override() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::SafeCrossfade),
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 32),
                &profile(Some(120.0), Some("8A"), 32),
                &policy
            ),
            TransitionTemplate::SafeCrossfade
        );
    }

    #[test]
    fn choose_template_requires_full_profile_when_configured() {
        let mut incomplete = profile(Some(120.0), Some("8A"), 32);
        incomplete.mix_in_seconds.clear();
        let policy = Policy {
            require_full_profile: true,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(&incomplete, &profile(Some(120.0), Some("8A"), 32), &policy),
            TransitionTemplate::SafeCrossfade
        );
    }

    #[test]
    fn choose_template_missing_bpm_is_safe_crossfade() {
        assert_eq!(
            Planner::choose_template(
                &profile(None, Some("8A"), 32),
                &profile(Some(120.0), Some("8A"), 32),
                &Policy::default()
            ),
            TransitionTemplate::SafeCrossfade
        );
    }

    #[test]
    fn choose_template_large_bpm_delta_is_slam_cut() {
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 32),
                &profile(Some(140.0), Some("8A"), 32),
                &Policy::default()
            ),
            TransitionTemplate::SlamCut
        );
    }

    #[test]
    fn choose_template_missing_camelot_is_safe_crossfade() {
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), None, 32),
                &profile(Some(120.0), Some("8A"), 32),
                &Policy::default()
            ),
            TransitionTemplate::SafeCrossfade
        );
    }

    #[test]
    fn choose_template_safe_intent_prefers_safe_crossfade() {
        let policy = Policy {
            mix_intent: MixIntent::Safe,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 1),
                &profile(Some(120.0), Some("8A"), 1),
                &policy
            ),
            TransitionTemplate::SafeCrossfade
        );
    }

    #[test]
    fn choose_template_uses_bass_swap_32_for_long_phrases() {
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 4),
                &profile(Some(121.0), Some("8A"), 4),
                &Policy::default()
            ),
            TransitionTemplate::BassSwap32
        );
    }

    #[test]
    fn choose_template_uses_bass_swap_16_for_medium_phrases() {
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 2),
                &profile(Some(121.0), Some("8A"), 2),
                &Policy::default()
            ),
            TransitionTemplate::BassSwap16
        );
    }

    #[test]
    fn choose_template_uses_long_harmonic_blend_for_compatible_short_profiles() {
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 1),
                &profile(Some(121.0), Some("9A"), 1),
                &Policy::default()
            ),
            TransitionTemplate::LongHarmonicBlend
        );
    }

    #[test]
    fn choose_template_bold_allows_filter_sweep_for_weaker_harmonic_fit() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 4),
                &profile(Some(124.0), Some("11A"), 4),
                &policy
            ),
            TransitionTemplate::FilterSweep
        );
    }

    #[test]
    fn choose_template_bold_prefers_filter_sweep_for_compatible_profiles() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 32),
                &profile(Some(121.0), Some("8A"), 32),
                &policy
            ),
            TransitionTemplate::FilterSweep
        );
    }

    #[test]
    fn choose_template_bold_prefers_filter_sweep_over_slam_cut_bpm_delta() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 32),
                &profile(Some(140.0), Some("8A"), 32),
                &policy
            ),
            TransitionTemplate::FilterSweep
        );
    }

    #[test]
    fn safe_crossfade_validates() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::SafeCrossfade),
            ..Policy::default()
        };
        Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(120.0), Some("8A"), 4),
            &policy,
        )
        .validate()
        .expect("safe crossfade");
    }

    #[test]
    fn slam_cut_validates() {
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(140.0), Some("8A"), 4),
            &Policy::default(),
        );
        assert_eq!(program.template, "SlamCut");
        program.validate().expect("slam cut");
    }

    #[test]
    fn bass_swap_16_low_band_crosses_at_swap_start() {
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 2),
            &profile(Some(121.0), Some("8A"), 2),
            &Policy::default(),
        );
        assert_eq!(program.template, "BassSwap16");
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::LowGain(DeckId::A)
                    && event.start_sample <= program.swap_start
                    && event.end_sample >= program.swap_start)
        );
    }

    #[test]
    fn bass_swap_16_shapes_clean_low_ownership_handoff() {
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 2),
            &profile(Some(121.0), Some("8A"), 2),
            &Policy::default(),
        );

        let outgoing_low = program
            .automation
            .iter()
            .find(|event| event.param == Param::LowGain(DeckId::A))
            .expect("outgoing low handoff");
        let incoming_low_hold = program
            .automation
            .iter()
            .find(|event| event.param == Param::LowGain(DeckId::B) && event.to == 0.0)
            .expect("incoming low hold");
        let incoming_low_rise = program
            .automation
            .iter()
            .find(|event| event.param == Param::LowGain(DeckId::B) && event.to == 1.0)
            .expect("incoming low rise");

        assert_eq!(outgoing_low.end_sample, program.swap_start);
        assert_eq!(incoming_low_hold.end_sample, program.swap_start);
        assert_eq!(incoming_low_rise.start_sample, program.swap_start);
        assert_eq!(outgoing_low.to, 0.05);
        assert_eq!(incoming_low_hold.from, 0.0);
        assert_eq!(incoming_low_hold.to, 0.0);
        assert_eq!(incoming_low_rise.from, 0.0);
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::MidGain(DeckId::A)
                    && event.start_sample == program.swap_start
                    && event.to < 1.0)
        );
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::MidGain(DeckId::B)
                    && event.from < 1.0
                    && event.to == 1.0)
        );
    }

    #[test]
    fn bass_swap_16_program_uses_requested_duration() {
        let program = bass_swap_16_program(48_000, 2, 16_000);

        assert_eq!(program.template, "BassSwap16");
        assert_eq!(program.resolve_at, 768_000);
        assert_eq!(program.tier, Tier::FullBlend);
        program.validate().expect("bass swap 16");
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::LowGain(DeckId::B)
                    && event.start_sample == program.swap_start
                    && event.to == 1.0)
        );
    }

    #[test]
    fn bass_swap_32_resolves_after_32_bars() {
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(121.0), Some("8A"), 4),
            &Policy::default(),
        );
        assert_eq!(program.template, "BassSwap32");
        assert_eq!(program.resolve_at, 32 * 96_000);
    }

    #[test]
    fn bass_swap_32_program_uses_requested_duration() {
        let program = bass_swap_32_program(48_000, 2, 32_000);

        assert_eq!(program.template, "BassSwap32");
        assert_eq!(program.resolve_at, 1_536_000);
        assert_eq!(program.tier, Tier::FullBlend);
        program.validate().expect("bass swap 32");
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::LowGain(DeckId::B)
                    && event.start_sample == program.swap_start
                    && event.to == 1.0)
        );
    }

    #[test]
    fn long_harmonic_blend_rejects_large_rate_delta() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::LongHarmonicBlend),
            ..Policy::default()
        };
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(126.0), Some("8A"), 4),
            &policy,
        );
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == Param::PlaybackRate(DeckId::B))
            .expect("rate automation")
            .to;
        assert_eq!(rate, 0.97);
    }

    #[test]
    fn filter_sweep_validates() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(124.0), Some("11A"), 4),
            &policy,
        );
        assert_eq!(program.template, "FilterSweep");
        program.validate().expect("filter sweep");
        let outgoing_low = program
            .automation
            .iter()
            .find(|event| event.param == Param::LowGain(DeckId::A))
            .expect("outgoing low automation");
        let outgoing_high = program
            .automation
            .iter()
            .find(|event| event.param == Param::HighGain(DeckId::A))
            .expect("outgoing high automation");
        let incoming_low = program
            .automation
            .iter()
            .find(|event| event.param == Param::LowGain(DeckId::B) && event.to == 1.0)
            .expect("incoming low rise automation");
        let incoming_high = program
            .automation
            .iter()
            .find(|event| event.param == Param::HighGain(DeckId::B) && event.to == 1.0)
            .expect("incoming high rise automation");

        assert_eq!(outgoing_low.to, 0.05);
        assert!(outgoing_low.start_sample < program.swap_start);
        assert!(outgoing_low.end_sample > program.swap_start);
        assert_eq!(outgoing_high.to, 0.35);
        assert!(outgoing_high.start_sample < program.swap_start);
        assert!(outgoing_high.end_sample > program.swap_start);
        assert_eq!(incoming_low.from, 0.0);
        assert!(incoming_low.start_sample < program.swap_start);
        assert!(incoming_low.end_sample > program.swap_start);
        assert_eq!(incoming_high.from, 0.35);
        assert!(incoming_high.start_sample < program.swap_start);
        assert!(incoming_high.end_sample > program.swap_start);
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::HighGain(DeckId::A))
        );
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::LowGain(DeckId::B))
        );
    }

    #[test]
    fn filter_sweep_eq_wash_program_uses_requested_duration() {
        let program = filter_sweep_eq_wash_program(48_000, 2, 10_000);

        assert_eq!(program.template, "FilterSweep");
        assert_eq!(program.resolve_at, 480_000);
        assert_eq!(program.tier, Tier::FullBlend);
        program.validate().expect("filter sweep eq wash");
    }

    #[test]
    fn planner_falls_back_to_safe_crossfade_on_invalid_program() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::SafeCrossfade),
            default_crossfade_ms: 0,
            ..Policy::default()
        };
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(120.0), Some("8A"), 4),
            &policy,
        );
        assert_eq!(program.template, "SafeCrossfade");
        program.validate().expect("fallback");
    }
}
