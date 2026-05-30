pub mod policy;
pub mod safety;
pub mod scoring;
pub mod template;

use crate::profile::DjProfile;
use crate::program::{AutomationEvent, Curve, DeckId, Param, Tier, TransitionProgram};

pub use policy::{DJ_PLANNER_VERSION, MixIntent, Policy, TransitionSpeedBias};
pub use template::TransitionTemplate;

const SMALL_TEMPO_NUDGE_MIN: f32 = 0.97;
const SMALL_TEMPO_NUDGE_MAX: f32 = 1.03;
const PLAYBACK_RATE_EPSILON: f32 = 0.0001;
const DROP_TEASE_CONFIDENCE_FLOOR: f32 = 0.65;
const DROP_PREVIEW_GAIN: f32 = 0.65;
const PLANNER_SAMPLE_RATE: u32 = 48_000;

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
        let comparable_incoming_bpm = scoring::nearest_tempo_family_bpm(outgoing_bpm, incoming_bpm);
        let bpm_delta = scoring::bpm_delta_pct(outgoing_bpm, comparable_incoming_bpm);
        let bold_filter_candidate = matches!(policy.mix_intent, MixIntent::Bold)
            && !outgoing.phrase_bar_indices.is_empty()
            && !incoming.phrase_bar_indices.is_empty();

        if bpm_delta > 8.0 {
            if bold_filter_candidate {
                return TransitionTemplate::FilterSweep;
            }
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
        let harmonic_fit = matches!(camelot_distance, 0 | 1 | 7);
        if !harmonic_fit {
            if bold_filter_candidate {
                return TransitionTemplate::FilterSweep;
            }
            return TransitionTemplate::SafeCrossfade;
        }

        if matches!(policy.mix_intent, MixIntent::Safe)
            && !(bpm_delta <= 3.0
                && outgoing.has_full_dj_profile()
                && incoming.has_full_dj_profile()
                && outgoing.phrase_bar_indices.len() >= 2
                && incoming.phrase_bar_indices.len() >= 2
                && harmonic_fit)
        {
            return TransitionTemplate::SafeCrossfade;
        }

        let outgoing_phrases = outgoing.phrase_bar_indices.len();
        let incoming_phrases = incoming.phrase_bar_indices.len();
        if matches!(policy.transition_speed_bias, TransitionSpeedBias::Slower)
            && matches!(camelot_distance, 0 | 1 | 7)
            && bpm_delta <= 3.0
        {
            return TransitionTemplate::LongHarmonicBlend;
        }
        if outgoing_phrases >= 4 && incoming_phrases >= 4 && bpm_delta <= 3.0 {
            return TransitionTemplate::BassSwap32;
        }
        if outgoing_phrases >= 2 && incoming_phrases >= 2 && bpm_delta <= 3.0 {
            return TransitionTemplate::BassSwap16;
        }
        if harmonic_fit && bpm_delta <= 3.0 {
            return TransitionTemplate::LongHarmonicBlend;
        }
        if bold_filter_candidate {
            return TransitionTemplate::FilterSweep;
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
    let sample_rate = PLANNER_SAMPLE_RATE;
    let channels = 2;
    let bpm = outgoing.bpm.or(incoming.bpm).unwrap_or(120.0).max(1.0);
    let bar_samples = ((60.0 / bpm) * 4.0 * sample_rate as f32) as u64;
    let duration_samples = duration_samples(template, policy, bar_samples);
    let swap_start = duration_samples / 2;
    let fade_start = swap_start;
    let mut program = TransitionProgram {
        tier: tier_for_template(template),
        template: template_name(template).to_string(),
        drop_source: None,
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: 0,
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation: if matches!(template, TransitionTemplate::DropTease16) {
            drop_tease_overlay_automation(duration_samples)
        } else {
            deck_gain_automation(duration_samples)
        },
    };
    program.deck_b_start_frame = if matches!(template, TransitionTemplate::DropTease16) {
        incoming_drop_tease_start_frame(incoming, swap_start, sample_rate)
    } else {
        incoming_sync_start_frame(incoming, sample_rate)
    };
    if matches!(template, TransitionTemplate::DropTease16) {
        program.drop_source = Some(drop_source_for_profile(incoming).to_string());
    }

    if matches!(
        template,
        TransitionTemplate::BassSwap16 | TransitionTemplate::BassSwap32
    ) {
        program
            .automation
            .extend(bass_swap_eq_handoff(duration_samples));
    }

    if matches!(template, TransitionTemplate::LongHarmonicBlend) {
        program
            .automation
            .extend(long_harmonic_low_handoff(duration_samples));
    }

    if matches!(template, TransitionTemplate::FilterSweep) {
        program
            .automation
            .extend(filter_sweep_eq_wash(duration_samples));
    }

    if !matches!(template, TransitionTemplate::SlamCut) {
        if let Some(rate) = small_tempo_nudge_rate(outgoing, incoming) {
            if (rate - 1.0).abs() > PLAYBACK_RATE_EPSILON {
                program.automation.push(AutomationEvent {
                    param: Param::PlaybackRate(DeckId::B),
                    start_sample: 0,
                    end_sample: duration_samples,
                    from: rate,
                    to: rate,
                    curve: Curve::Linear,
                });
            }
        }
    }

    program
}

#[allow(dead_code)]
fn drop_tease_candidate_ready(outgoing: &DjProfile, incoming: &DjProfile, policy: &Policy) -> bool {
    let has_manual_drop = !incoming.manual_drop_seconds.is_empty();
    if !(matches!(policy.mix_intent, MixIntent::Bold)
        || (matches!(policy.mix_intent, MixIntent::Balanced) && has_manual_drop))
    {
        return false;
    }
    if !outgoing.has_full_dj_profile() || !incoming.has_full_dj_profile() {
        return false;
    }
    if outgoing.profile_confidence < DROP_TEASE_CONFIDENCE_FLOOR
        || incoming.profile_confidence < DROP_TEASE_CONFIDENCE_FLOOR
    {
        return false;
    }
    let Some(camelot_distance) = outgoing
        .camelot_key
        .as_deref()
        .zip(incoming.camelot_key.as_deref())
        .and_then(|(a, b)| scoring::camelot_distance(a, b))
    else {
        return false;
    };
    if !matches!(camelot_distance, 0 | 1 | 7) {
        return false;
    }
    if outgoing.downbeat_seconds.is_empty()
        || incoming.downbeat_seconds.is_empty()
        || outgoing.phrase_bar_indices.len() < 4
        || incoming.phrase_bar_indices.len() < 4
    {
        return false;
    }
    let Some(rate) = small_tempo_nudge_rate(outgoing, incoming) else {
        return false;
    };
    if !(SMALL_TEMPO_NUDGE_MIN..=SMALL_TEMPO_NUDGE_MAX).contains(&rate) {
        return false;
    }
    let Some(outgoing_bpm) = outgoing.bpm else {
        return false;
    };
    let min_drop_lead_frames = bar_samples_for_bpm(outgoing_bpm, PLANNER_SAMPLE_RATE) * 8;
    first_valid_drop_frame(incoming, PLANNER_SAMPLE_RATE)
        .is_some_and(|drop_frame| drop_frame >= min_drop_lead_frames)
}

fn bar_samples_for_bpm(bpm: f32, sample_rate: u32) -> u64 {
    ((60.0 / bpm.max(1.0)) * 4.0 * sample_rate as f32) as u64
}

fn incoming_sync_start_frame(incoming: &DjProfile, sample_rate: u32) -> u64 {
    incoming
        .downbeat_seconds
        .iter()
        .chain(incoming.beat_grid_seconds.iter())
        .copied()
        .find(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * sample_rate as f32).round() as u64)
        .unwrap_or(0)
}

fn incoming_drop_tease_start_frame(
    incoming: &DjProfile,
    drop_alignment_sample: u64,
    sample_rate: u32,
) -> u64 {
    first_valid_drop_frame(incoming, sample_rate)
        .map(|drop_frame| drop_frame.saturating_sub(drop_alignment_sample))
        .unwrap_or_else(|| incoming_sync_start_frame(incoming, sample_rate))
}

fn first_valid_drop_frame(incoming: &DjProfile, sample_rate: u32) -> Option<u64> {
    drop_candidates(incoming)
        .copied()
        .find(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * sample_rate as f32).round() as u64)
}

fn drop_candidates(incoming: &DjProfile) -> impl Iterator<Item = &f32> {
    if incoming.manual_drop_seconds.is_empty() {
        incoming.drop_seconds.iter()
    } else {
        incoming.manual_drop_seconds.iter()
    }
}

fn drop_source_for_profile(incoming: &DjProfile) -> &'static str {
    if incoming.manual_drop_seconds.is_empty() {
        "profile_drop_candidate"
    } else {
        "manual_drop_cue"
    }
}

fn small_tempo_nudge_rate(outgoing: &DjProfile, incoming: &DjProfile) -> Option<f32> {
    let outgoing_bpm = outgoing.bpm?.max(1.0);
    let incoming_bpm = incoming.bpm?.max(1.0);
    let comparable_incoming_bpm = scoring::nearest_tempo_family_bpm(outgoing_bpm, incoming_bpm);
    let rate = outgoing_bpm / comparable_incoming_bpm;
    (rate.is_finite() && (SMALL_TEMPO_NUDGE_MIN..=SMALL_TEMPO_NUDGE_MAX).contains(&rate))
        .then_some(rate)
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
        drop_source: None,
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: 0,
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
        drop_source: None,
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: 0,
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

pub fn slam_cut_program(sample_rate: u32, channels: u16, duration_ms: u32) -> TransitionProgram {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let duration_samples =
        (u64::from(duration_ms).saturating_mul(u64::from(sample_rate)) / 1_000).max(1);
    TransitionProgram {
        tier: Tier::SafeCrossfade,
        template: "SlamCut".to_string(),
        drop_source: None,
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: 0,
        sync_start: 0,
        intro_start: 0,
        swap_start: duration_samples,
        fade_start: duration_samples,
        resolve_at: duration_samples,
        loops: vec![],
        automation: deck_gain_automation(duration_samples),
    }
}

pub fn long_harmonic_blend_program(
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
    rate: f32,
) -> TransitionProgram {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let duration_samples =
        (u64::from(duration_ms).saturating_mul(u64::from(sample_rate)) / 1_000).max(1);
    let swap_start = duration_samples / 2;
    let mut program = TransitionProgram {
        tier: Tier::FullBlend,
        template: "LongHarmonicBlend".to_string(),
        drop_source: None,
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: 0,
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start: swap_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation: deck_gain_automation(duration_samples),
    };
    program.automation.push(AutomationEvent {
        param: Param::PlaybackRate(DeckId::B),
        start_sample: 0,
        end_sample: duration_samples,
        from: rate.clamp(0.97, 1.03),
        to: rate.clamp(0.97, 1.03),
        curve: Curve::Linear,
    });
    program
        .automation
        .extend(long_harmonic_low_handoff(duration_samples));
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
        drop_source: None,
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: 0,
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

pub fn drop_tease_16_program(
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
) -> TransitionProgram {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let duration_samples =
        (u64::from(duration_ms).saturating_mul(u64::from(sample_rate)) / 1_000).max(1);
    let swap_start = duration_samples / 2;
    TransitionProgram {
        tier: Tier::FullBlend,
        template: "DropTease16".to_string(),
        drop_source: None,
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: 0,
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start: swap_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation: drop_tease_overlay_automation(duration_samples),
    }
}

pub fn drop_preview_16_program(
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
    outgoing: &DjProfile,
    incoming: &DjProfile,
) -> Option<TransitionProgram> {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let duration_samples =
        (u64::from(duration_ms).saturating_mul(u64::from(sample_rate)) / 1_000).max(1);
    let swap_start = duration_samples / 2;
    let drop_frame = first_valid_preview_drop_frame(incoming, sample_rate)?;
    let rate = drop_preview_autosync_rate(outgoing, incoming)?;
    let mut automation = drop_preview_overlay_automation(duration_samples);
    if (rate - 1.0).abs() > PLAYBACK_RATE_EPSILON {
        automation.push(AutomationEvent {
            param: Param::PlaybackRate(DeckId::B),
            start_sample: 0,
            end_sample: duration_samples,
            from: rate,
            to: rate,
            curve: Curve::Linear,
        });
    }
    Some(TransitionProgram {
        tier: Tier::FullBlend,
        template: "DropPreview16".to_string(),
        drop_source: Some(drop_source_for_profile(incoming).to_string()),
        sample_rate,
        channels,
        deck_a_start_frame: 0,
        deck_b_start_frame: drop_frame.saturating_sub(swap_start),
        sync_start: 0,
        intro_start: 0,
        swap_start,
        fade_start: swap_start,
        resolve_at: duration_samples,
        loops: vec![],
        automation,
    })
}

fn drop_preview_autosync_rate(outgoing: &DjProfile, incoming: &DjProfile) -> Option<f32> {
    let outgoing_bpm = outgoing.bpm?.max(1.0);
    let incoming_bpm = incoming.bpm?.max(1.0);
    let comparable_incoming_bpm = scoring::nearest_tempo_family_bpm(outgoing_bpm, incoming_bpm);
    if scoring::bpm_delta_pct(outgoing_bpm, comparable_incoming_bpm) > 3.0 {
        return None;
    }
    let rate = outgoing_bpm / comparable_incoming_bpm;
    (rate.is_finite() && (SMALL_TEMPO_NUDGE_MIN..=SMALL_TEMPO_NUDGE_MAX).contains(&rate))
        .then_some(rate)
}

fn duration_samples(template: TransitionTemplate, policy: &Policy, bar_samples: u64) -> u64 {
    match template {
        TransitionTemplate::BassSwap32 => bar_samples * 32,
        TransitionTemplate::BassSwap16 => bar_samples * 16,
        TransitionTemplate::LongHarmonicBlend => bar_samples * 16,
        TransitionTemplate::FilterSweep => bar_samples * 8,
        TransitionTemplate::DropTease16 => bar_samples * 16,
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
        | TransitionTemplate::FilterSweep
        | TransitionTemplate::DropTease16 => Tier::FullBlend,
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
        TransitionTemplate::DropTease16 => "DropTease16",
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

fn drop_tease_overlay_automation(duration_samples: u64) -> Vec<AutomationEvent> {
    let end_sample = duration_samples.max(1);
    let fade_in_end = (end_sample / 4).max(1);
    let fade_out_start = (end_sample * 3 / 4).max(fade_in_end);
    vec![
        AutomationEvent {
            param: Param::DeckGain(DeckId::A),
            start_sample: 0,
            end_sample,
            from: 0.0,
            to: 0.0,
            curve: Curve::Linear,
        },
        AutomationEvent {
            param: Param::DeckGain(DeckId::B),
            start_sample: 0,
            end_sample: fade_in_end,
            from: 0.0,
            to: 1.0,
            curve: Curve::EqualPowerIn,
        },
        AutomationEvent {
            param: Param::DeckGain(DeckId::B),
            start_sample: fade_in_end,
            end_sample: fade_out_start.max(fade_in_end + 1),
            from: 1.0,
            to: 1.0,
            curve: Curve::Linear,
        },
        AutomationEvent {
            param: Param::DeckGain(DeckId::B),
            start_sample: fade_out_start,
            end_sample,
            from: 1.0,
            to: 0.0,
            curve: Curve::EqualPowerIn,
        },
    ]
}

fn drop_preview_overlay_automation(duration_samples: u64) -> Vec<AutomationEvent> {
    let end_sample = duration_samples.max(1);
    let fade_in_end = (end_sample / 4).max(1);
    let fade_out_start = (end_sample * 3 / 4).max(fade_in_end);
    vec![
        AutomationEvent {
            param: Param::DeckGain(DeckId::A),
            start_sample: 0,
            end_sample,
            from: 0.0,
            to: 0.0,
            curve: Curve::Linear,
        },
        AutomationEvent {
            param: Param::DeckGain(DeckId::B),
            start_sample: 0,
            end_sample: fade_in_end,
            from: 0.0,
            to: DROP_PREVIEW_GAIN,
            curve: Curve::EqualPowerIn,
        },
        AutomationEvent {
            param: Param::DeckGain(DeckId::B),
            start_sample: fade_in_end,
            end_sample: fade_out_start.max(fade_in_end + 1),
            from: DROP_PREVIEW_GAIN,
            to: DROP_PREVIEW_GAIN,
            curve: Curve::Linear,
        },
        AutomationEvent {
            param: Param::DeckGain(DeckId::B),
            start_sample: fade_out_start,
            end_sample,
            from: DROP_PREVIEW_GAIN,
            to: 0.0,
            curve: Curve::EqualPowerIn,
        },
    ]
}

fn first_valid_preview_drop_frame(incoming: &DjProfile, sample_rate: u32) -> Option<u64> {
    incoming
        .manual_drop_seconds
        .iter()
        .chain(incoming.drop_seconds.iter())
        .copied()
        .find(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * sample_rate as f32).round() as u64)
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

fn long_harmonic_low_handoff(duration_samples: u64) -> Vec<AutomationEvent> {
    let end_sample = duration_samples.max(1);
    if end_sample < 8 {
        return vec![
            AutomationEvent {
                param: Param::LowGain(DeckId::A),
                start_sample: 0,
                end_sample,
                from: 1.0,
                to: 0.25,
                curve: Curve::Cosine,
            },
            AutomationEvent {
                param: Param::LowGain(DeckId::B),
                start_sample: 0,
                end_sample,
                from: 0.15,
                to: 1.0,
                curve: Curve::Cosine,
            },
        ];
    }

    let swap_point = end_sample / 2;
    let outgoing_low_start = end_sample / 4;
    let incoming_low_end = end_sample * 3 / 4;
    vec![
        AutomationEvent {
            param: Param::LowGain(DeckId::A),
            start_sample: outgoing_low_start,
            end_sample: swap_point.max(outgoing_low_start + 1),
            from: 1.0,
            to: 0.25,
            curve: Curve::Cosine,
        },
        AutomationEvent {
            param: Param::LowGain(DeckId::B),
            start_sample: 0,
            end_sample: swap_point.max(1),
            from: 0.15,
            to: 0.15,
            curve: Curve::Linear,
        },
        AutomationEvent {
            param: Param::LowGain(DeckId::B),
            start_sample: swap_point,
            end_sample: incoming_low_end.max(swap_point + 1),
            from: 0.15,
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
            manual_drop_seconds: vec![],
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
    fn choose_template_treats_half_time_grid_as_compatible() {
        assert_eq!(
            Planner::choose_template(
                &profile(Some(124.0), Some("8A"), 2),
                &profile(Some(62.0), Some("8A"), 2),
                &Policy::default()
            ),
            TransitionTemplate::BassSwap16
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
    fn choose_template_rejects_distant_same_mode_camelot_for_balanced_intent() {
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 4),
                &profile(Some(121.0), Some("3A"), 4),
                &Policy::default()
            ),
            TransitionTemplate::SafeCrossfade
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
    fn choose_template_bold_preserves_bass_swap_32_for_compatible_profiles() {
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
            TransitionTemplate::BassSwap32
        );
    }

    #[test]
    fn choose_template_slower_bias_prefers_long_harmonic_blend() {
        let policy = Policy {
            transition_speed_bias: TransitionSpeedBias::Slower,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 4),
                &profile(Some(121.0), Some("8A"), 4),
                &policy
            ),
            TransitionTemplate::LongHarmonicBlend
        );
    }

    #[test]
    fn choose_template_bold_preserves_bass_swap_16_for_medium_phrases() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 2),
                &profile(Some(121.0), Some("8A"), 2),
                &policy
            ),
            TransitionTemplate::BassSwap16
        );
    }

    #[test]
    fn choose_template_bold_preserves_long_harmonic_blend_for_short_compatible_profiles() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        assert_eq!(
            Planner::choose_template(
                &profile(Some(120.0), Some("8A"), 1),
                &profile(Some(121.0), Some("9A"), 1),
                &policy
            ),
            TransitionTemplate::LongHarmonicBlend
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
    fn planner_applies_small_tempo_nudge_inside_three_percent() {
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 2),
            &profile(Some(122.0), Some("8A"), 2),
            &Policy::default(),
        );
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == Param::PlaybackRate(DeckId::B))
            .expect("rate automation");

        assert_eq!(rate.from, rate.to);
        assert!((rate.to - 120.0 / 122.0).abs() < 0.0001);
    }

    #[test]
    fn planner_omits_tempo_nudge_outside_three_percent() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::LongHarmonicBlend),
            ..Policy::default()
        };
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(126.0), Some("8A"), 4),
            &policy,
        );

        assert!(
            !program
                .automation
                .iter()
                .any(|event| event.param == Param::PlaybackRate(DeckId::B))
        );
    }

    #[test]
    fn planner_sets_incoming_start_frame_from_downbeat() {
        let mut incoming = profile(Some(121.0), Some("8A"), 2);
        incoming.downbeat_seconds = vec![8.0];

        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 2),
            &incoming,
            &Policy::default(),
        );

        assert_eq!(program.deck_b_start_frame, 384_000);
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
    fn slam_cut_program_uses_requested_duration() {
        let program = slam_cut_program(48_000, 2, 200);

        assert_eq!(program.template, "SlamCut");
        assert_eq!(program.resolve_at, 9_600);
        assert_eq!(program.tier, Tier::SafeCrossfade);
        program.validate().expect("slam cut");
        assert!(
            program
                .automation
                .iter()
                .any(|event| event.param == Param::DeckGain(DeckId::A))
        );
        assert!(program.automation.iter().all(|event| !matches!(
            event.param,
            Param::LowGain(_) | Param::MidGain(_) | Param::HighGain(_)
        )));
    }

    #[test]
    fn long_harmonic_blend_omits_large_rate_delta() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::LongHarmonicBlend),
            ..Policy::default()
        };
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(126.0), Some("8A"), 4),
            &policy,
        );
        assert!(
            !program
                .automation
                .iter()
                .any(|event| event.param == Param::PlaybackRate(DeckId::B))
        );
    }

    #[test]
    fn long_harmonic_blend_skips_noop_half_time_rate() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::LongHarmonicBlend),
            ..Policy::default()
        };
        let program = Planner::plan(
            &profile(Some(124.0), Some("8A"), 1),
            &profile(Some(62.0), Some("8A"), 1),
            &policy,
        );
        assert!(
            !program
                .automation
                .iter()
                .any(|event| event.param == Param::PlaybackRate(DeckId::B))
        );
    }

    #[test]
    fn long_harmonic_blend_program_uses_requested_duration() {
        let program = long_harmonic_blend_program(48_000, 2, 16_000, 0.985);

        assert_eq!(program.template, "LongHarmonicBlend");
        assert_eq!(program.resolve_at, 768_000);
        assert_eq!(program.tier, Tier::FullBlend);
        program.validate().expect("long harmonic blend");
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == Param::PlaybackRate(DeckId::B))
            .expect("rate automation")
            .to;
        assert_eq!(rate, 0.985);
        let outgoing_low = program
            .automation
            .iter()
            .find(|event| event.param == Param::LowGain(DeckId::A))
            .expect("outgoing low handoff");
        let incoming_low = program
            .automation
            .iter()
            .find(|event| event.param == Param::LowGain(DeckId::B) && event.to == 1.0)
            .expect("incoming low rise");
        assert_eq!(outgoing_low.end_sample, program.swap_start);
        assert_eq!(incoming_low.start_sample, program.swap_start);
        assert_eq!(outgoing_low.to, 0.25);
        assert_eq!(incoming_low.from, 0.15);
        assert!(
            program
                .automation
                .iter()
                .all(|event| !matches!(event.param, Param::MidGain(_) | Param::HighGain(_)))
        );
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
    fn choose_template_bold_keeps_drop_tease_out_of_end_transition() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(121.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];

        assert_eq!(
            Planner::choose_template(&outgoing, &incoming, &policy),
            TransitionTemplate::BassSwap32
        );
    }

    #[test]
    fn choose_template_drop_tease_rejects_safe_and_unverified_balanced_intents() {
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(121.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];

        let safe = Policy {
            mix_intent: MixIntent::Safe,
            ..Policy::default()
        };
        assert_ne!(
            Planner::choose_template(&outgoing, &incoming, &safe),
            TransitionTemplate::DropTease16
        );

        let balanced = Policy {
            mix_intent: MixIntent::Balanced,
            ..Policy::default()
        };
        assert_ne!(
            Planner::choose_template(&outgoing, &incoming, &balanced),
            TransitionTemplate::DropTease16
        );
    }

    #[test]
    fn choose_template_balanced_selects_drop_tease_for_manual_drop_candidate() {
        let policy = Policy {
            mix_intent: MixIntent::Balanced,
            ..Policy::default()
        };
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(121.0), Some("8A"), 4);
        incoming.manual_drop_seconds = vec![32.0];

        assert_eq!(
            Planner::choose_template(&outgoing, &incoming, &policy),
            TransitionTemplate::DropTease16
        );
    }

    #[test]
    fn choose_template_drop_tease_requires_harmonic_fit() {
        let policy = Policy {
            mix_intent: MixIntent::Balanced,
            ..Policy::default()
        };
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(121.0), Some("3B"), 4);
        incoming.manual_drop_seconds = vec![32.0];

        assert_ne!(
            Planner::choose_template(&outgoing, &incoming, &policy),
            TransitionTemplate::DropTease16
        );
    }

    #[test]
    fn choose_template_drop_tease_rejects_unsafe_candidates() {
        let policy = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        let outgoing = profile(Some(120.0), Some("8A"), 4);

        let mut missing_drop = profile(Some(121.0), Some("8A"), 4);
        missing_drop.drop_seconds.clear();
        assert_ne!(
            Planner::choose_template(&outgoing, &missing_drop, &policy),
            TransitionTemplate::DropTease16
        );

        let mut low_confidence = profile(Some(121.0), Some("8A"), 4);
        low_confidence.drop_seconds = vec![32.0];
        low_confidence.profile_confidence = 0.5;
        assert_ne!(
            Planner::choose_template(&outgoing, &low_confidence, &policy),
            TransitionTemplate::DropTease16
        );

        let mut unsafe_tempo = profile(Some(130.0), Some("8A"), 4);
        unsafe_tempo.drop_seconds = vec![32.0];
        assert_ne!(
            Planner::choose_template(&outgoing, &unsafe_tempo, &policy),
            TransitionTemplate::DropTease16
        );

        let mut early_drop = profile(Some(121.0), Some("8A"), 4);
        early_drop.drop_seconds = vec![4.0];
        assert_ne!(
            Planner::choose_template(&outgoing, &early_drop, &policy),
            TransitionTemplate::DropTease16
        );
    }

    #[test]
    fn drop_tease_plan_aligns_incoming_drop_to_overlay_swap() {
        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::DropTease16),
            ..Policy::default()
        };
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(121.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];

        let program = Planner::plan(&outgoing, &incoming, &policy);

        assert_eq!(program.template, "DropTease16");
        assert_eq!(
            program.drop_source.as_deref(),
            Some("profile_drop_candidate")
        );
        assert_eq!(program.swap_start, 768_000);
        assert_eq!(program.deck_b_start_frame, 768_000);
    }

    #[test]
    fn drop_tease_plan_prefers_manual_drop_alignment() {
        let policy = Policy {
            mix_intent: MixIntent::Balanced,
            ..Policy::default()
        };
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(121.0), Some("8A"), 4);
        incoming.drop_seconds = vec![40.0];
        incoming.manual_drop_seconds = vec![32.0];

        let program = Planner::plan(&outgoing, &incoming, &policy);

        assert_eq!(program.template, "DropTease16");
        assert_eq!(program.drop_source.as_deref(), Some("manual_drop_cue"));
        assert_eq!(program.deck_b_start_frame, 768_000);
    }

    #[test]
    fn drop_tease_16_override_builds_even_without_auto_candidate() {
        let bold = Policy {
            mix_intent: MixIntent::Bold,
            ..Policy::default()
        };
        let selected = Planner::choose_template(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(121.0), Some("8A"), 4),
            &bold,
        );
        assert_eq!(selected, TransitionTemplate::BassSwap32);

        let policy = Policy {
            safety_template_override: Some(TransitionTemplate::DropTease16),
            ..Policy::default()
        };
        let program = Planner::plan(
            &profile(Some(120.0), Some("8A"), 4),
            &profile(Some(121.0), Some("8A"), 4),
            &policy,
        );

        assert_eq!(program.template, "DropTease16");
        assert_eq!(program.resolve_at, 16 * 96_000);
        assert!(program.automation.iter().any(|event| {
            event.param == Param::DeckGain(DeckId::A) && event.from == 0.0 && event.to == 0.0
        }));
        assert!(program.automation.iter().any(|event| {
            event.param == Param::DeckGain(DeckId::B) && event.from == 1.0 && event.to == 0.0
        }));
        program.validate().expect("drop tease guardrail program");
    }

    #[test]
    fn drop_tease_16_program_uses_overlay_gain_shape() {
        let program = drop_tease_16_program(48_000, 2, 16_000);

        assert_eq!(program.template, "DropTease16");
        assert_eq!(program.resolve_at, 768_000);
        assert_eq!(program.tier, Tier::FullBlend);
        program.validate().expect("drop tease program");
        assert!(program.automation.iter().any(|event| {
            event.param == Param::DeckGain(DeckId::A) && event.from == 0.0 && event.to == 0.0
        }));
        assert!(program.automation.iter().any(|event| {
            event.param == Param::DeckGain(DeckId::B)
                && event.start_sample == program.resolve_at * 3 / 4
                && event.to == 0.0
        }));
    }

    #[test]
    fn drop_preview_16_program_aligns_profile_drop_to_preview_midpoint() {
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(120.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];

        let program =
            drop_preview_16_program(48_000, 2, 16_000, &outgoing, &incoming).expect("drop preview");

        assert_eq!(program.template, "DropPreview16");
        assert_eq!(program.resolve_at, 768_000);
        assert_eq!(program.swap_start, 384_000);
        assert_eq!(program.deck_b_start_frame, 1_152_000);
        program.validate().expect("drop preview program");
    }

    #[test]
    fn drop_preview_16_program_prefers_manual_drop_marker() {
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(120.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];
        incoming.manual_drop_seconds = vec![24.0];

        let program =
            drop_preview_16_program(48_000, 2, 16_000, &outgoing, &incoming).expect("drop preview");

        assert_eq!(program.deck_b_start_frame, 768_000);
    }

    #[test]
    fn drop_preview_16_program_rejects_missing_drop_marker() {
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let incoming = profile(Some(120.0), Some("8A"), 4);

        assert!(drop_preview_16_program(48_000, 2, 16_000, &outgoing, &incoming).is_none());
    }

    #[test]
    fn drop_preview_16_program_caps_preview_gain() {
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(120.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];

        let program =
            drop_preview_16_program(48_000, 2, 16_000, &outgoing, &incoming).expect("drop preview");

        assert!(program.automation.iter().any(|event| {
            event.param == Param::DeckGain(DeckId::A) && event.from == 0.0 && event.to == 0.0
        }));
        assert!(program.automation.iter().any(|event| {
            event.param == Param::DeckGain(DeckId::B) && event.from == 0.65 && event.to == 0.65
        }));
        assert!(!program.automation.iter().any(|event| {
            event.param == Param::DeckGain(DeckId::B) && (event.from == 1.0 || event.to == 1.0)
        }));
    }

    #[test]
    fn drop_preview_16_program_autosyncs_small_tempo_delta() {
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(122.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];

        let program =
            drop_preview_16_program(48_000, 2, 16_000, &outgoing, &incoming).expect("drop preview");
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == Param::PlaybackRate(DeckId::B))
            .expect("incoming playback rate automation");

        assert!((rate.from - 120.0 / 122.0).abs() < 0.0001);
        assert_eq!(rate.from, rate.to);
    }

    #[test]
    fn drop_preview_16_program_rejects_unsyncable_tempo_delta() {
        let outgoing = profile(Some(120.0), Some("8A"), 4);
        let mut incoming = profile(Some(130.0), Some("8A"), 4);
        incoming.drop_seconds = vec![32.0];

        assert!(drop_preview_16_program(48_000, 2, 16_000, &outgoing, &incoming).is_none());
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
