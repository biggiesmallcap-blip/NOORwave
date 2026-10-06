//! A bounded, inspectable extension of the existing planner and automation.
//! Risk gates run before scoring; preference and diversity never admit a risky plan.
use super::{MixIntent, Policy, TransitionSpeedBias, TransitionTemplate, scoring};
use crate::profile::DjProfile;
use crate::program::{
    AutomationEvent, Curve, DeckId, Param, TransitionCandidateScore, TransitionDecision,
    TransitionProgram, TransitionScoreComponent,
};

#[derive(Clone)]
struct Candidate {
    program: TransitionProgram,
    facts: TransitionCandidateScore,
    drop_seconds: Option<f32>,
    window: &'static str,
}

#[derive(Clone, Copy)]
struct Entry {
    seconds: f32,
    phrase: bool,
    breakdown: bool,
}

/// Canonical families deliberately count both bass-swap durations as one style.
pub(super) fn family(name: &str) -> &str {
    match name {
        "LongHarmonicBlend" | "SmoothBlend" | "smooth_blend" | "smooth" => "smooth",
        "ClubMix" | "club_mix" | "club" => "club",
        "QuickMix" | "quick_mix" | "quick" => "quick",
        "EnergyLift" | "energy_lift" => "energy_lift",
        "EnergyReset" | "energy_reset" => "energy_reset",
        "DropSwap" | "drop_swap" => "drop_swap",
        "BassSwap16" | "BassSwap32" | "bass_swap" => "bass_swap",
        "SlamCut" | "cut" | "slam_cut" => "cut",
        "FilterSweep" | "filter_sweep" => "filter_sweep",
        "SafeCrossfade" | "safe_crossfade" => "safe",
        other => other,
    }
}

pub(super) fn plan(
    outgoing: &DjProfile,
    incoming: &DjProfile,
    policy: &Policy,
) -> TransitionProgram {
    if policy.safety_template_override.is_some() {
        return super::Planner::plan(outgoing, incoming, policy);
    }
    let confidence =
        finite_unit(outgoing.profile_confidence).min(finite_unit(incoming.profile_confidence));
    if outgoing.safe_crossfade_only || incoming.safe_crossfade_only || confidence < 0.65 {
        return safe(
            outgoing,
            incoming,
            policy,
            "Analysis confidence requires a protected crossfade",
        );
    }
    if policy.require_full_profile
        && (!outgoing.has_full_dj_profile() || !incoming.has_full_dj_profile())
    {
        return safe(
            outgoing,
            incoming,
            policy,
            "A complete profile is required by the current policy",
        );
    }
    let (Some(a_bpm), Some(b_bpm)) = (
        valid_bpm(outgoing.bpm).or_else(|| valid_bpm(outgoing.tempo_bpm)),
        valid_bpm(incoming.bpm).or_else(|| valid_bpm(incoming.tempo_bpm)),
    ) else {
        return safe(
            outgoing,
            incoming,
            policy,
            "Tempo evidence is insufficient for a rhythmic transition",
        );
    };
    let tempo_delta =
        scoring::bpm_delta_pct(a_bpm, scoring::nearest_tempo_family_bpm(a_bpm, b_bpm));
    let rate = super::small_tempo_nudge_rate(outgoing, incoming);
    let beat_confidence = [outgoing, incoming]
        .into_iter()
        .map(|p| p.beat_confidence.map(finite_unit).unwrap_or(0.62))
        .fold(1.0_f32, f32::min);
    let grid_ready = [outgoing, incoming].into_iter().all(|p| {
        !p.grid_is_synthetic
            && grid_tempo_is_credible(p)
            && p.beat_grid_seconds
                .iter()
                .filter(|v| v.is_finite() && **v >= 0.0)
                .count()
                >= 2
    });
    let rhythmic = rate.is_some() && grid_ready && beat_confidence >= 0.55;
    let phrase_ready = !phrase_seconds(outgoing).is_empty() && !phrase_seconds(incoming).is_empty();
    let advanced = rhythmic && phrase_ready && beat_confidence >= 0.72 && confidence >= 0.65;
    let harmonic = outgoing
        .camelot_key
        .as_deref()
        .zip(incoming.camelot_key.as_deref())
        .and_then(|(a, b)| scoring::camelot_distance(a, b))
        .is_some_and(|distance| matches!(distance, 0 | 1 | 7));
    let conservative = matches!(policy.mix_intent, MixIntent::Safe);
    let adventure = (match policy.mix_intent {
        MixIntent::Safe => 0.0,
        MixIntent::Balanced => 0.5,
        MixIntent::Bold => 1.0,
    } + bounded(policy.adventurousness_bias, -0.1, 0.1))
    .clamp(0.0, 1.0);
    let energy_change = outgoing
        .energy
        .zip(incoming.energy)
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .map(|(a, b)| (b - a).clamp(-1.0, 1.0));
    let energy_move = energy_change.unwrap_or(0.0);
    let entries = entries(incoming, adventure);
    let mut candidates = Vec::new();

    for entry in entries {
        let vocal_conflict = vocal_conflict(outgoing, incoming, entry.seconds, 8.0);
        let vocals_known = vocal_conflict.is_some();
        let clash = vocal_conflict.unwrap_or(0.25);
        let low_vocals = vocals_known && clash < 0.16;
        let short_exit = outgoing
            .intro_end_seconds
            .zip(outgoing.outro_start_seconds)
            .is_some_and(|(intro, outro)| {
                intro.is_finite() && outro.is_finite() && outro < intro + 8.0
            });
        if rhythmic {
            let bass_name = if phrase_ready
                && outgoing.phrase_bar_indices.len() >= 4
                && incoming.phrase_bar_indices.len() >= 4
                && clash < 0.35
                && !matches!(policy.transition_speed_bias, TransitionSpeedBias::Faster)
            {
                "BassSwap32"
            } else {
                "BassSwap16"
            };
            let bass_beats = if bass_name == "BassSwap32" {
                48.0
            } else {
                32.0
            };
            add(
                &mut candidates,
                outgoing,
                incoming,
                policy,
                entry,
                bass_name,
                speed_beats(bass_beats, policy, conservative),
                a_bpm,
                rate,
                0.76 + if phrase_ready { 0.05 } else { 0.0 } - clash * 0.1,
                "Rhythmic overlap with separate low-band ownership",
                None,
                "phrase_end",
                confidence,
                beat_confidence,
                harmonic,
                energy_move,
                clash,
                vocals_known,
            );
            if harmonic && clash < 0.3 && energy_move.abs() <= 0.18 {
                let quality = 0.79
                    + if low_vocals { 0.09 } else { 0.0 }
                    + if matches!(policy.transition_speed_bias, TransitionSpeedBias::Slower) {
                        0.035
                    } else {
                        0.0
                    };
                add(
                    &mut candidates,
                    outgoing,
                    incoming,
                    policy,
                    entry,
                    "LongHarmonicBlend",
                    speed_beats(48.0, policy, conservative),
                    a_bpm,
                    rate,
                    quality,
                    "Compatible keys and restrained energy suit a long blend",
                    None,
                    "outro_to_intro",
                    confidence,
                    beat_confidence,
                    harmonic,
                    energy_move,
                    clash,
                    vocals_known,
                );
            }
            if phrase_ready && (!conservative || beat_confidence >= 0.8) && clash < 0.45 {
                add(
                    &mut candidates,
                    outgoing,
                    incoming,
                    policy,
                    entry,
                    "ClubMix",
                    speed_beats(32.0, policy, conservative),
                    a_bpm,
                    rate,
                    0.79 + if entry.phrase { 0.025 } else { 0.0 }
                        + if low_vocals { 0.01 } else { -0.03 },
                    "A percussion lead-in and phrase handoff keep the groove moving",
                    None,
                    "phrase_end",
                    confidence,
                    beat_confidence,
                    harmonic,
                    energy_move,
                    clash,
                    vocals_known,
                );
            }
            if !conservative || clash > 0.35 || short_exit {
                add(
                    &mut candidates,
                    outgoing,
                    incoming,
                    policy,
                    entry,
                    "QuickMix",
                    speed_beats(8.0, policy, conservative),
                    a_bpm,
                    rate,
                    0.72 + clash * 0.23 + if short_exit { 0.06 } else { 0.0 },
                    if vocals_known && clash > 0.35 {
                        "A brief overlap avoids sustained vocal competition"
                    } else {
                        "A compact phrase mix keeps this handoff concise"
                    },
                    None,
                    "short_overlap",
                    confidence,
                    beat_confidence,
                    harmonic,
                    energy_move,
                    clash,
                    vocals_known,
                );
            }
            if advanced
                && !conservative
                && energy_move >= 0.08
                && energy_move <= 0.32 + adventure * 0.2
            {
                add(
                    &mut candidates,
                    outgoing,
                    incoming,
                    policy,
                    entry,
                    "EnergyLift",
                    speed_beats(16.0, policy, false),
                    a_bpm,
                    rate,
                    0.84 + energy_move.min(0.25) * 0.15,
                    "The incoming phrase raises intensity with a late bass arrival",
                    None,
                    "phrase_end",
                    confidence,
                    beat_confidence,
                    harmonic,
                    energy_move,
                    clash,
                    vocals_known,
                );
            }
            if advanced
                && !conservative
                && energy_move <= -0.08
                && energy_move >= -0.32 - adventure * 0.2
            {
                add(
                    &mut candidates,
                    outgoing,
                    incoming,
                    policy,
                    entry,
                    "EnergyReset",
                    speed_beats(16.0, policy, false),
                    a_bpm,
                    rate,
                    0.84 + if entry.breakdown { 0.06 } else { 0.0 },
                    "A calmer entry and earlier withdrawal create breathing room",
                    None,
                    "phrase_end",
                    confidence,
                    beat_confidence,
                    harmonic,
                    energy_move,
                    clash,
                    vocals_known,
                );
            }
        }
        // A cut uses an actual downbeat and a short click-safe envelope, without tempo stretch.
        let cut_ready = grid_ready
            && beat_confidence >= 0.7
            && incoming
                .downbeat_seconds
                .iter()
                .any(|downbeat| downbeat.is_finite() && (*downbeat - entry.seconds).abs() <= 0.04)
            && (!conservative || tempo_delta > 8.0);
        if cut_ready
            && (tempo_delta > 8.0 || clash > 0.6 || adventure > 0.75 && energy_move.abs() > 0.3)
        {
            add(
                &mut candidates,
                outgoing,
                incoming,
                policy,
                entry,
                "SlamCut",
                a_bpm / 60.0 * 0.04,
                a_bpm,
                None,
                0.79 + if tempo_delta > 8.0 { 0.1 } else { clash * 0.1 },
                "A clean downbeat cut avoids a mismatched or crowded blend",
                None,
                "downbeat_cut",
                confidence,
                beat_confidence,
                harmonic,
                energy_move,
                clash,
                vocals_known,
            );
        }
        if grid_ready && phrase_ready && beat_confidence >= 0.7 && adventure > 0.75 {
            add(
                &mut candidates,
                outgoing,
                incoming,
                policy,
                entry,
                "FilterSweep",
                16.0,
                a_bpm,
                rate,
                0.77 + if !harmonic { 0.025 } else { 0.0 },
                "A short tonal wash clears space before the incoming phrase",
                None,
                "phrase_end",
                confidence,
                beat_confidence,
                harmonic,
                energy_move,
                clash,
                vocals_known,
            );
        }
    }
    if advanced && !conservative {
        add_drop_candidates(
            &mut candidates,
            outgoing,
            incoming,
            policy,
            a_bpm,
            rate,
            confidence,
            beat_confidence,
            harmonic,
            energy_move,
            adventure,
        );
    }
    // A reliable tempo estimate is useful even when beat phase is unknown.
    // Keep this overlap short, start B at its opening, and use unity playback
    // rate: manufactured grids must not imply beat/phrase alignment.
    let mut decision_confidence = confidence.min(beat_confidence);
    let mut duration_bpm = a_bpm;
    if candidates.is_empty()
        && !conservative
        && let Some((tempo, tempo_confidence)) = compatible_independent_tempos(outgoing, incoming)
    {
        let seconds = match policy.transition_speed_bias {
            TransitionSpeedBias::Faster => 1.5,
            TransitionSpeedBias::Neutral => (8.0 * 60.0 / tempo).clamp(1.5, 4.0),
            TransitionSpeedBias::Slower => 4.0,
        };
        let entry = Entry {
            seconds: 0.0,
            phrase: false,
            breakdown: false,
        };
        add(
            &mut candidates,
            outgoing,
            incoming,
            policy,
            entry,
            "QuickMix",
            seconds * tempo / 60.0,
            tempo,
            None,
            0.74,
            "Compatible tempo evidence supports a short bass handoff; beat phase is unverified",
            None,
            "tempo_informed_short_overlap",
            confidence,
            0.0,
            harmonic,
            energy_move,
            0.25,
            false,
        );
        if energy_move.abs() >= 0.12 && energy_move.abs() <= 0.25 + adventure * 0.2 {
            let (style, reason) = if energy_move > 0.0 {
                (
                    "EnergyLift",
                    "Compatible tempo evidence and rising energy suit a short late-bass lift; beat phase is unverified",
                )
            } else {
                (
                    "EnergyReset",
                    "Compatible tempo evidence and calmer incoming energy suit an early bass withdrawal; beat phase is unverified",
                )
            };
            add(
                &mut candidates,
                outgoing,
                incoming,
                policy,
                entry,
                style,
                seconds * tempo / 60.0,
                tempo,
                None,
                0.77,
                reason,
                None,
                "tempo_informed_short_overlap",
                confidence,
                0.0,
                harmonic,
                energy_move,
                0.25,
                false,
            );
        }
        for candidate in &mut candidates {
            candidate
                .facts
                .components
                .push(component("independent_tempo", tempo_confidence, 0.08));
            candidate.facts.quality_score += tempo_confidence * 0.08;
        }
        decision_confidence = confidence.min(tempo_confidence);
        duration_bpm = tempo;
    }
    if candidates.is_empty() {
        return safe(
            outgoing,
            incoming,
            policy,
            "No musical candidate passes the rhythm and confidence gates",
        );
    }

    let best_quality = candidates
        .iter()
        .map(|c| c.facts.quality_score)
        .fold(0.0_f32, f32::max);
    for candidate in &mut candidates {
        let quality = candidate.facts.quality_score;
        let style = family(&candidate.facts.strategy);
        let preference = if family(&policy.preferred_strategy) == style {
            0.14
        } else {
            0.0
        };
        let feedback = policy
            .strategy_feedback
            .iter()
            .filter(|(name, _)| family(name) == style)
            .map(|(_, value)| bounded(*value, -0.06, 0.06))
            .sum::<f32>()
            .clamp(-0.06, 0.06);
        let repeats = policy
            .recent_templates
            .iter()
            .take(6)
            .enumerate()
            .filter(|(_, name)| family(name) == style)
            .map(|(index, _)| 0.022 / (index + 1) as f32)
            .sum::<f32>()
            .min(0.055);
        let variety = if quality >= best_quality - 0.08 && !conservative {
            -repeats
        } else {
            0.0
        };
        let wildcard = if policy.preferred_strategy == "wildcard"
            && quality >= best_quality - 0.08
            && matches!(
                style,
                "drop_swap" | "energy_lift" | "energy_reset" | "quick" | "cut" | "filter_sweep"
            ) {
            0.045
        } else {
            0.0
        };
        candidate.facts.score = quality + preference + feedback + variety + wildcard;
        for (name, value) in [
            ("preference", preference),
            ("feedback", feedback),
            ("recent_diversity", variety),
            ("wildcard", wildcard),
        ] {
            candidate.facts.components.push(component(name, value, 1.0));
        }
    }
    candidates.sort_by(|a, b| {
        b.facts
            .score
            .total_cmp(&a.facts.score)
            .then_with(|| a.facts.entry_seconds.total_cmp(&b.facts.entry_seconds))
    });
    let chosen = candidates[0].clone();
    let mut program = chosen.program;
    program.decision = Some(TransitionDecision {
        strategy: chosen.facts.strategy.clone(),
        confidence: decision_confidence,
        score: chosen.facts.score,
        reason: chosen.facts.reason.clone(),
        energy_direction: energy_direction(energy_change).to_string(),
        incoming_entry_seconds: chosen.facts.entry_seconds,
        incoming_drop_seconds: chosen.drop_seconds,
        outgoing_window: chosen.window.to_string(),
        duration_beats: chosen.facts.duration_seconds * duration_bpm / 60.0,
        candidates: candidates.into_iter().take(24).map(|c| c.facts).collect(),
    });
    program
}

fn grid_tempo_is_credible(profile: &DjProfile) -> bool {
    let Some(tempo) = valid_bpm(profile.tempo_bpm) else {
        return true;
    };
    let Some(grid) = valid_bpm(profile.bpm) else {
        return false;
    };
    scoring::bpm_delta_pct(grid, scoring::nearest_tempo_family_bpm(grid, tempo)) <= 3.0
}

fn compatible_independent_tempos(a: &DjProfile, b: &DjProfile) -> Option<(f32, f32)> {
    let (a_tempo, b_tempo) = (valid_bpm(a.tempo_bpm)?, valid_bpm(b.tempo_bpm)?);
    let confidence = finite_unit(a.tempo_confidence?).min(finite_unit(b.tempo_confidence?));
    let corroborated = [a, b].into_iter().all(grid_tempo_is_credible);
    ((confidence >= 0.65 || (confidence >= 0.55 && corroborated))
        && scoring::bpm_delta_pct(a_tempo, scoring::nearest_tempo_family_bpm(a_tempo, b_tempo))
            <= 3.0)
        .then_some((a_tempo, confidence))
}

#[allow(clippy::too_many_arguments)]
fn add(
    candidates: &mut Vec<Candidate>,
    outgoing: &DjProfile,
    incoming: &DjProfile,
    policy: &Policy,
    entry: Entry,
    name: &str,
    beats: f32,
    bpm: f32,
    rate: Option<f32>,
    suitability: f32,
    reason: &str,
    drop_seconds: Option<f32>,
    window: &'static str,
    confidence: f32,
    beat_confidence: f32,
    harmonic: bool,
    energy_move: f32,
    clash: f32,
    vocals_known: bool,
) {
    // Structure/imported cues describe useful sections, not necessarily exact
    // phase. Rhythmic programs must enter on a measured marker. Verified drop
    // swaps retain their rate-aware lead so the requested drop stays exact;
    // phase-unverified short overlaps deliberately keep the raw opening.
    let entry = if name != "DropSwap" && (rate.is_some() || name == "SlamCut") {
        let Some(entry) = snap_rhythmic_entry(incoming, entry, name == "SlamCut") else {
            return;
        };
        entry
    } else {
        entry
    };
    let seconds = transition_seconds(name, beats, bpm);
    let duration = (seconds * super::PLANNER_SAMPLE_RATE as f32).round() as u64;
    let mut program = program(name, duration, outgoing, incoming, policy);
    program.deck_b_start_frame = (entry.seconds * program.sample_rate as f32).round() as u64;
    if let Some(rate) = rate.filter(|r| (r - 1.0).abs() > super::PLAYBACK_RATE_EPSILON) {
        program.automation.push(event(
            Param::PlaybackRate(DeckId::B),
            0,
            duration,
            rate,
            rate,
            Curve::Linear,
        ));
    }
    if super::safety::validate_audio_safety(&program, &super::safety::AudioSafetyPolicy::default())
        .is_err()
    {
        return;
    }
    let phrase_quality = if entry.phrase { 1.0 } else { 0.55 };
    let key_quality = if harmonic {
        1.0
    } else if matches!(
        name,
        "BassSwap16" | "BassSwap32" | "QuickMix" | "SlamCut" | "FilterSweep"
    ) {
        0.8
    } else {
        0.55
    };
    // Unknown vocals receive a neutral value, never the reward for confirmed instrumental material.
    let vocal_quality = if vocals_known { 1.0 - clash } else { 0.45 };
    let components = vec![
        component("suitability", suitability, 0.65),
        component("confidence", confidence, 0.12),
        component("rhythm", beat_confidence, 0.08),
        component("phrase", phrase_quality, 0.06),
        component("key", key_quality, 0.04),
        component("vocal_space", vocal_quality, 0.05),
    ];
    let quality_score = components.iter().map(|c| c.value * c.weight).sum::<f32>();
    // Later cues are useful only when they materially improve the transition; close ties keep the opening.
    let entry_penalty = (entry.seconds / 45.0 * 0.018).clamp(0.0, 0.018);
    let mut components = components;
    components.push(component("entry_skip_cost", -entry_penalty, 1.0));
    components.push(component("energy_movement", energy_move, 0.0));
    candidates.push(Candidate {
        program,
        facts: TransitionCandidateScore {
            strategy: name.to_string(),
            score: quality_score - entry_penalty,
            quality_score: quality_score - entry_penalty,
            entry_seconds: entry.seconds,
            duration_seconds: seconds,
            reason: reason.to_string(),
            components,
        },
        drop_seconds,
        window,
    });
}

fn snap_rhythmic_entry(profile: &DjProfile, entry: Entry, downbeat_only: bool) -> Option<Entry> {
    let grid = if downbeat_only {
        &profile.downbeat_seconds
    } else {
        &profile.beat_grid_seconds
    };
    let scope = profile
        .analysis_scope_seconds
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(45.0)
        .min(45.0);
    let nearest = grid
        .iter()
        .copied()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0 && *seconds <= scope)
        .min_by(|left, right| {
            (left - entry.seconds)
                .abs()
                .total_cmp(&(right - entry.seconds).abs())
        })?;
    let beat_seconds = 60.0 / valid_bpm(profile.bpm)?;
    let max_shift = beat_seconds * if downbeat_only { 2.1 } else { 0.6 };
    if (nearest - entry.seconds).abs() > max_shift {
        return None;
    }
    Some(Entry {
        seconds: nearest,
        phrase: phrase_seconds(profile)
            .iter()
            .any(|seconds| (*seconds - nearest).abs() < 0.1),
        breakdown: entry.breakdown,
    })
}

fn program(
    name: &str,
    duration: u64,
    outgoing: &DjProfile,
    incoming: &DjProfile,
    policy: &Policy,
) -> TransitionProgram {
    let mut program = super::build_program(
        TransitionTemplate::SafeCrossfade,
        outgoing,
        incoming,
        policy,
    );
    program.template = name.to_string();
    program.tier = if name == "SlamCut" {
        crate::program::Tier::SafeCrossfade
    } else {
        crate::program::Tier::FullBlend
    };
    program.resolve_at = duration;
    program.swap_start = duration / 2;
    program.fade_start = duration / 2;
    program.automation = match name {
        "ClubMix" => club_envelope(duration),
        "EnergyLift" => lift_envelope(duration),
        "EnergyReset" => reset_envelope(duration),
        "DropSwap" => drop_envelope(duration),
        "SlamCut" | "QuickMix" => cosine_envelope(duration),
        _ => super::deck_gain_automation(duration),
    };
    match name {
        "LongHarmonicBlend" => program
            .automation
            .extend(super::long_harmonic_low_handoff(duration)),
        "FilterSweep" => program
            .automation
            .extend(super::filter_sweep_eq_wash(duration)),
        "SlamCut" => {
            program.swap_start = duration / 2;
            program.fade_start = program.swap_start;
        }
        "DropSwap" => {
            program.swap_start = duration * 3 / 4;
            program.fade_start = program.swap_start;
            program
                .automation
                .extend(low_handoff(duration, program.swap_start));
        }
        "EnergyLift" => {
            program.swap_start = duration * 3 / 4;
            program.fade_start = program.swap_start;
            program
                .automation
                .extend(low_handoff(duration, program.swap_start));
        }
        "EnergyReset" => {
            program.swap_start = duration * 2 / 5;
            program.fade_start = program.swap_start;
            program
                .automation
                .extend(low_handoff(duration, program.swap_start));
            program.automation.push(event(
                Param::HighGain(DeckId::A),
                0,
                duration,
                1.0,
                0.25,
                Curve::Cosine,
            ));
        }
        "QuickMix" => program
            .automation
            .extend(low_handoff(duration, duration / 3)),
        _ => program
            .automation
            .extend(super::bass_swap_eq_handoff(duration)),
    }
    program
}

fn cosine_envelope(end: u64) -> Vec<AutomationEvent> {
    vec![
        event(Param::DeckGain(DeckId::A), 0, end, 1.0, 0.0, Curve::Cosine),
        event(Param::DeckGain(DeckId::B), 0, end, 0.0, 1.0, Curve::Cosine),
    ]
}

fn club_envelope(end: u64) -> Vec<AutomationEvent> {
    let lead = end / 4;
    let handoff = end * 5 / 8;
    vec![
        event(
            Param::DeckGain(DeckId::A),
            0,
            handoff,
            1.0,
            1.0,
            Curve::Linear,
        ),
        event(
            Param::DeckGain(DeckId::A),
            handoff,
            end,
            1.0,
            0.0,
            Curve::EqualPowerOut,
        ),
        event(
            Param::DeckGain(DeckId::B),
            0,
            lead,
            0.0,
            0.35,
            Curve::Cosine,
        ),
        event(
            Param::DeckGain(DeckId::B),
            lead,
            handoff,
            0.35,
            0.35,
            Curve::Linear,
        ),
        event(
            Param::DeckGain(DeckId::B),
            handoff,
            end,
            0.35,
            1.0,
            Curve::EqualPowerIn,
        ),
    ]
}

fn lift_envelope(end: u64) -> Vec<AutomationEvent> {
    let arrival = end * 3 / 4;
    vec![
        event(
            Param::DeckGain(DeckId::A),
            0,
            arrival,
            1.0,
            0.85,
            Curve::Cosine,
        ),
        event(
            Param::DeckGain(DeckId::A),
            arrival,
            end,
            0.85,
            0.0,
            Curve::EqualPowerOut,
        ),
        event(
            Param::DeckGain(DeckId::B),
            0,
            arrival,
            0.0,
            0.5,
            Curve::Cosine,
        ),
        event(
            Param::DeckGain(DeckId::B),
            arrival,
            end,
            0.5,
            1.0,
            Curve::EqualPowerIn,
        ),
    ]
}

fn reset_envelope(end: u64) -> Vec<AutomationEvent> {
    let withdrawal = end * 3 / 5;
    vec![
        event(
            Param::DeckGain(DeckId::A),
            0,
            withdrawal,
            1.0,
            0.0,
            Curve::EqualPowerOut,
        ),
        event(
            Param::DeckGain(DeckId::B),
            0,
            withdrawal,
            0.0,
            0.7,
            Curve::EqualPowerIn,
        ),
        event(
            Param::DeckGain(DeckId::B),
            withdrawal,
            end,
            0.7,
            1.0,
            Curve::Cosine,
        ),
    ]
}

fn drop_envelope(end: u64) -> Vec<AutomationEvent> {
    let arrival = end * 3 / 4;
    let punch_end = (arrival + u64::from(super::PLANNER_SAMPLE_RATE) / 25).min(end);
    vec![
        event(
            Param::DeckGain(DeckId::A),
            0,
            arrival,
            1.0,
            1.0,
            Curve::Linear,
        ),
        event(
            Param::DeckGain(DeckId::A),
            arrival,
            punch_end,
            1.0,
            0.0,
            Curve::Cosine,
        ),
        event(
            Param::DeckGain(DeckId::B),
            0,
            arrival,
            0.0,
            0.32,
            Curve::Cosine,
        ),
        event(
            Param::DeckGain(DeckId::B),
            arrival,
            punch_end,
            0.32,
            1.0,
            Curve::Cosine,
        ),
    ]
}

fn low_handoff(end: u64, swap: u64) -> Vec<AutomationEvent> {
    let start = swap.saturating_sub((end / 12).max(1));
    let finish = (swap + (end / 12).max(1)).min(end);
    vec![
        event(
            Param::LowGain(DeckId::A),
            start,
            swap,
            1.0,
            0.05,
            Curve::Cosine,
        ),
        event(Param::LowGain(DeckId::B), 0, swap, 0.0, 0.0, Curve::Linear),
        event(
            Param::LowGain(DeckId::B),
            swap,
            finish,
            0.0,
            1.0,
            Curve::Cosine,
        ),
    ]
}

fn event(param: Param, start: u64, end: u64, from: f32, to: f32, curve: Curve) -> AutomationEvent {
    AutomationEvent {
        param,
        start_sample: start,
        end_sample: end.max(start + 1),
        from,
        to,
        curve,
    }
}

fn component(name: &str, value: f32, weight: f32) -> TransitionScoreComponent {
    TransitionScoreComponent {
        name: name.to_string(),
        value,
        weight,
    }
}

fn speed_beats(beats: f32, policy: &Policy, conservative: bool) -> f32 {
    match policy.transition_speed_bias {
        TransitionSpeedBias::Faster => (beats / 2.0).max(4.0),
        TransitionSpeedBias::Slower => (beats * 1.5).min(if conservative { 48.0 } else { 64.0 }),
        TransitionSpeedBias::Neutral => beats,
    }
}

fn transition_seconds(name: &str, beats: f32, bpm: f32) -> f32 {
    let beats = if name == "SlamCut" {
        beats
    } else {
        beats.min((28.0 * bpm / 60.0 / 4.0).floor() * 4.0).max(4.0)
    };
    (beats * 60.0 / bpm).clamp(0.04, 28.0)
}

fn phrase_seconds(profile: &DjProfile) -> Vec<f32> {
    profile
        .phrase_bar_indices
        .iter()
        .filter_map(|bar| {
            profile
                .downbeat_seconds
                .get(*bar as usize)
                .or_else(|| profile.beat_grid_seconds.get(*bar as usize * 4))
        })
        .copied()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .collect()
}

fn entries(profile: &DjProfile, adventure: f32) -> Vec<Entry> {
    let scope = profile
        .analysis_scope_seconds
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(45.0);
    let intro_limit = profile
        .intro_end_seconds
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or(8.0)
        + 8.0;
    let limit = scope.min(45.0).min(if adventure > 0.75 {
        45.0
    } else {
        intro_limit.min(24.0)
    });
    let phrases = phrase_seconds(profile);
    let first = profile
        .downbeat_seconds
        .iter()
        .chain(profile.beat_grid_seconds.iter())
        .copied()
        .find(|s| s.is_finite() && *s >= 0.0 && *s <= limit)
        .unwrap_or(0.0);
    let mut times = vec![first];
    times.extend(profile.mix_in_seconds.iter().copied());
    times.extend(phrases.iter().copied());
    if adventure > 0.0 {
        times.extend(profile.breakdown_seconds.iter().copied());
    }
    times.retain(|s| s.is_finite() && *s >= first && *s <= limit);
    times.sort_by(f32::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 0.1);
    times
        .into_iter()
        .take(8)
        .map(|seconds| Entry {
            seconds,
            phrase: phrases.iter().any(|p| (*p - seconds).abs() < 0.1),
            breakdown: profile
                .breakdown_seconds
                .iter()
                .any(|p| (*p - seconds).abs() < 1.0),
        })
        .collect()
}

fn vocal_conflict(
    outgoing: &DjProfile,
    incoming: &DjProfile,
    entry: f32,
    seconds: f32,
) -> Option<f32> {
    if !outgoing.vocals_known || !incoming.vocals_known {
        return None;
    }
    let a = if !outgoing.vocal_density_by_bar.is_empty() {
        &outgoing.vocal_density_by_bar
    } else {
        &outgoing.vocal_presence_by_bar
    };
    let b = if !incoming.vocal_density_by_bar.is_empty() {
        &incoming.vocal_density_by_bar
    } else {
        &incoming.vocal_presence_by_bar
    };
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let bar_seconds = 240.0 / valid_bpm(incoming.bpm)?;
    let origin = incoming.downbeat_seconds.first().copied().unwrap_or(0.0);
    let index = ((entry - origin).max(0.0) / bar_seconds).floor() as usize;
    let span = (seconds / bar_seconds).ceil().max(1.0) as usize;
    let a_tail = &a[a.len().saturating_sub(span)..];
    let b_entry = b.get(index..(index + span).min(b.len()))?;
    if b_entry.is_empty() {
        return None;
    }
    let avg =
        |values: &[f32]| values.iter().map(|v| finite_unit(*v)).sum::<f32>() / values.len() as f32;
    Some((avg(a_tail) * avg(b_entry)).clamp(0.0, 1.0))
}

#[allow(clippy::too_many_arguments)]
fn add_drop_candidates(
    candidates: &mut Vec<Candidate>,
    outgoing: &DjProfile,
    incoming: &DjProfile,
    policy: &Policy,
    bpm: f32,
    rate: Option<f32>,
    confidence: f32,
    beat_confidence: f32,
    harmonic: bool,
    energy_move: f32,
    adventure: f32,
) {
    let manual = !incoming.manual_drop_seconds.is_empty();
    let drops = if manual {
        &incoming.manual_drop_seconds
    } else {
        &incoming.drop_seconds
    };
    let scope = incoming
        .analysis_scope_seconds
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(45.0);
    let limit = if manual {
        scope.max(45.0)
    } else {
        scope.min(45.0)
    };
    let duration_seconds = transition_seconds("DropSwap", 16.0, bpm);
    let lead = duration_seconds * 0.75 * rate.unwrap_or(1.0);
    for drop in drops
        .iter()
        .copied()
        .filter(|d| d.is_finite() && *d >= lead && *d <= limit)
        .take(3)
    {
        let supported_build = incoming
            .breakdown_seconds
            .iter()
            .any(|s| s.is_finite() && *s < drop && drop - *s <= 24.0);
        let phrase = phrase_seconds(incoming)
            .iter()
            .any(|s| (*s - drop).abs() < 0.25);
        // Automatic markers are energy heuristics: a marker alone never proves a drop swap.
        let section = (drop / (32.0 * 60.0 / bpm)).floor() as usize;
        let contour_build = section
            .checked_sub(1)
            .and_then(|previous| incoming.energy_contour.get(previous..=section))
            .is_some_and(|pair| {
                pair[0].is_finite() && pair[1].is_finite() && pair[1] - pair[0] > 0.12
            });
        if !manual
            && !(confidence >= 0.85
                && beat_confidence >= 0.8
                && supported_build
                && phrase
                && contour_build)
        {
            continue;
        }
        let entry = Entry {
            seconds: drop - lead,
            phrase: true,
            breakdown: supported_build,
        };
        let clash =
            vocal_conflict(outgoing, incoming, entry.seconds, duration_seconds).unwrap_or(0.4);
        if clash > 0.55 || !harmonic && adventure < 0.75 {
            continue;
        }
        let before = candidates.len();
        add(
            candidates,
            outgoing,
            incoming,
            policy,
            entry,
            "DropSwap",
            16.0,
            bpm,
            rate,
            0.92,
            "A verified build reaches the incoming drop at the bass handoff",
            Some(drop),
            "build_to_drop",
            confidence,
            beat_confidence,
            harmonic,
            energy_move,
            clash,
            outgoing.vocals_known && incoming.vocals_known,
        );
        if candidates.len() > before {
            candidates.last_mut().unwrap().program.drop_source = Some(
                if manual {
                    "manual_drop_cue"
                } else {
                    "profile_drop_candidate"
                }
                .to_string(),
            );
        }
    }
}

fn safe(
    outgoing: &DjProfile,
    incoming: &DjProfile,
    policy: &Policy,
    reason: &str,
) -> TransitionProgram {
    let mut safe_policy = policy.clone();
    safe_policy.safety_template_override = Some(TransitionTemplate::SafeCrossfade);
    let mut program = super::Planner::plan(outgoing, incoming, &safe_policy);
    program.decision = Some(TransitionDecision {
        strategy: "SafeCrossfade".to_string(),
        confidence: finite_unit(outgoing.profile_confidence)
            .min(finite_unit(incoming.profile_confidence)),
        score: 0.0,
        reason: reason.to_string(),
        energy_direction: energy_direction(
            outgoing.energy.zip(incoming.energy).map(|(a, b)| b - a),
        )
        .to_string(),
        incoming_entry_seconds: program.deck_b_start_frame as f32 / program.sample_rate as f32,
        incoming_drop_seconds: None,
        outgoing_window: "safe_tail".to_string(),
        duration_beats: valid_bpm(outgoing.bpm)
            .map(|bpm| program.resolve_at as f32 / program.sample_rate as f32 * bpm / 60.0)
            .unwrap_or(0.0),
        candidates: vec![],
    });
    program
}

fn energy_direction(change: Option<f32>) -> &'static str {
    match change.filter(|v| v.is_finite()) {
        Some(v) if v > 0.06 => "lift",
        Some(v) if v < -0.06 => "reset",
        Some(_) => "steady",
        None => "unknown",
    }
}

fn valid_bpm(bpm: Option<f32>) -> Option<f32> {
    bpm.filter(|v| v.is_finite() && *v >= 30.0 && *v <= 300.0)
}
fn finite_unit(value: f32) -> f32 {
    bounded(value, 0.0, 1.0)
}
fn bounded(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn safe_crossfade_speed_changes_executable_duration() {
        let profile = profile();
        let mut policy = Policy {
            safety_template_override: Some(TransitionTemplate::SafeCrossfade),
            ..Policy::default()
        };
        let neutral = plan(&profile, &profile, &policy);
        policy.transition_speed_bias = TransitionSpeedBias::Slower;
        let slower = plan(&profile, &profile, &policy);
        policy.transition_speed_bias = TransitionSpeedBias::Faster;
        let faster = plan(&profile, &profile, &policy);
        assert!(slower.resolve_at > neutral.resolve_at);
        assert!(faster.resolve_at < neutral.resolve_at);
        for p in [neutral, slower, faster] {
            crate::planner::safety::validate_audio_safety(
                &p,
                &crate::planner::safety::AudioSafetyPolicy::default(),
            )
            .unwrap();
        }
    }
    use super::*;
    use crate::automation::param_value_at;

    fn profile() -> DjProfile {
        DjProfile {
            bpm: Some(120.0),
            camelot_key: Some("8A".into()),
            energy: Some(0.5),
            beat_grid_seconds: (0..240).map(|beat| beat as f32 * 0.5).collect(),
            downbeat_seconds: (0..60).map(|bar| bar as f32 * 2.0).collect(),
            phrase_bar_indices: vec![0, 8, 16, 24, 32, 40, 48, 56],
            mix_in_seconds: vec![0.0, 2.0, 4.0],
            mix_out_seconds: vec![108.0, 112.0],
            intro_end_seconds: Some(16.0),
            outro_start_seconds: Some(104.0),
            breakdown_seconds: vec![],
            drop_seconds: vec![],
            manual_drop_seconds: vec![],
            safe_transition_windows: vec![crate::profile::TransitionWindow {
                start_seconds: 0.0,
                end_seconds: 16.0,
                confidence: 0.9,
            }],
            vocal_presence_by_bar: vec![0.0; 60],
            vocal_density_by_bar: vec![0.0; 60],
            lufs_loud_body: Some(-12.0),
            true_peak_dbtp: Some(-1.0),
            profile_confidence: 0.95,
            safe_crossfade_only: false,
            profile_version: "test".into(),
            beat_confidence: Some(0.9),
            tempo_bpm: None,
            tempo_confidence: None,
            grid_is_synthetic: false,
            energy_contour: vec![0.2, 0.2, 0.7],
            analysis_scope_seconds: Some(120.0),
            vocals_known: true,
        }
    }

    #[test]
    fn rhythmic_candidates_snap_imported_cues_to_measured_beats() {
        let a = profile();
        let mut b = profile();
        b.beat_grid_seconds = (0..240).map(|beat| 0.13 + beat as f32 * 0.5).collect();
        b.downbeat_seconds = (0..60).map(|bar| 0.13 + bar as f32 * 2.0).collect();
        b.mix_in_seconds = vec![0.23, 2.23];
        b.breakdown_seconds = vec![1.23];
        let planned = crate::Planner::plan_adaptive(
            &a,
            &b,
            &Policy {
                preferred_strategy: "club_mix".into(),
                ..Policy::default()
            },
        );
        let decision = planned.decision.unwrap();
        let club = decision
            .candidates
            .iter()
            .filter(|candidate| candidate.strategy == "ClubMix")
            .collect::<Vec<_>>();
        assert!(club.len() >= 2);
        for candidate in club {
            assert!(
                b.beat_grid_seconds
                    .iter()
                    .any(|beat| (*beat - candidate.entry_seconds).abs() < 0.0001),
                "off-beat cue {}",
                candidate.entry_seconds
            );
        }
    }

    #[test]
    fn adaptive_compares_multiple_entries_and_styles_without_randomness() {
        let a = profile();
        let b = profile();
        let policy = Policy::default();
        let first = crate::Planner::plan_adaptive(&a, &b, &policy);
        assert_eq!(first, crate::Planner::plan_adaptive(&a, &b, &policy));
        let decision = first.decision.as_ref().unwrap();
        assert!(decision.candidates.len() > 3);
        assert!(decision.candidates.iter().any(|c| c.entry_seconds > 0.0));
        assert_eq!(first.template, "LongHarmonicBlend");
        for candidate in &decision.candidates {
            let sum = candidate
                .components
                .iter()
                .map(|c| c.value * c.weight)
                .sum::<f32>();
            assert!((sum - candidate.score).abs() < 1e-5);
        }
    }

    #[test]
    fn musical_pairs_select_lift_reset_quick_cut_and_verified_drop() {
        let a = profile();
        for (energy, name) in [(0.75, "EnergyLift"), (0.25, "EnergyReset")] {
            let mut b = profile();
            b.energy = Some(energy);
            let planned = crate::Planner::plan_adaptive(&a, &b, &Policy::default());
            assert_eq!(planned.template, name);
            planned.validate().unwrap();
        }
        let mut vocals = profile();
        vocals.vocal_density_by_bar.fill(1.0);
        assert_eq!(
            crate::Planner::plan_adaptive(&vocals, &vocals, &Policy::default()).template,
            "QuickMix"
        );
        let mut fast = profile();
        fast.bpm = Some(150.0);
        assert_eq!(
            crate::Planner::plan_adaptive(&a, &fast, &Policy::default()).template,
            "SlamCut"
        );
        let mut drop = profile();
        drop.manual_drop_seconds = vec![32.0];
        let planned = crate::Planner::plan_adaptive(&a, &drop, &Policy::default());
        assert_eq!(planned.template, "DropSwap");
        let rate = param_value_at(&planned.automation, Param::PlaybackRate(DeckId::B), 0);
        let source_at_swap = (planned.deck_b_start_frame as f32 + planned.swap_start as f32 * rate)
            / planned.sample_rate as f32;
        assert!((source_at_swap - 32.0).abs() < 0.001);
    }

    #[test]
    fn measured_partial_rhythm_remains_useful_and_provisional_grid_stays_safe() {
        let mut a = profile();
        a.camelot_key = None;
        a.phrase_bar_indices.clear();
        a.safe_transition_windows.clear();
        a.profile_confidence = 0.65;
        let good = crate::Planner::plan_adaptive(&a, &a, &Policy::default());
        assert!(matches!(good.template.as_str(), "BassSwap16" | "QuickMix"));
        a.beat_confidence = Some(0.05);
        assert_eq!(
            crate::Planner::plan_adaptive(&a, &a, &Policy::default()).template,
            "SafeCrossfade"
        );
        a.beat_confidence = Some(0.9);
        a.safe_crossfade_only = true;
        assert_eq!(
            crate::Planner::plan_adaptive(
                &a,
                &a,
                &Policy {
                    preferred_strategy: "wildcard".into(),
                    mix_intent: MixIntent::Bold,
                    ..Policy::default()
                }
            )
            .template,
            "SafeCrossfade"
        );
    }

    #[test]
    fn reported_synthetic_grids_use_independent_tempo_without_claiming_phase() {
        let mut dorado = profile();
        dorado.bpm = Some(115.0);
        dorado.beat_confidence = Some(1.0);
        dorado.tempo_bpm = Some(171.43);
        dorado.tempo_confidence = Some(0.851);
        dorado.grid_is_synthetic = true;
        dorado.profile_confidence = 0.65;
        let mut diversion = dorado.clone();
        diversion.bpm = Some(58.0);
        diversion.beat_confidence = Some(0.378);
        diversion.tempo_bpm = Some(174.04);
        diversion.tempo_confidence = Some(0.714);
        let planned = crate::Planner::plan_adaptive(&dorado, &diversion, &Policy::default());
        assert_eq!(planned.template, "QuickMix");
        assert_eq!(planned.deck_b_start_frame, 0);
        assert!(planned.resolve_at < 4 * u64::from(planned.sample_rate));
        assert!(
            !planned
                .automation
                .iter()
                .any(|e| matches!(e.param, Param::PlaybackRate(_)))
        );
        assert!(
            planned
                .automation
                .iter()
                .any(|e| matches!(e.param, Param::LowGain(_)))
        );
        let decision = planned.decision.unwrap();
        assert_eq!(decision.outgoing_window, "tempo_informed_short_overlap");
        assert!(decision.reason.contains("phase is unverified"));
        assert!(decision.candidates.iter().all(|c| c.strategy == "QuickMix"));
        assert_eq!(
            crate::Planner::plan_adaptive(
                &dorado,
                &diversion,
                &Policy {
                    mix_intent: MixIntent::Safe,
                    ..Policy::default()
                }
            )
            .template,
            "SafeCrossfade"
        );

        let mut photek = dorado.clone();
        photek.bpm = Some(63.0);
        photek.beat_confidence = Some(0.437);
        photek.tempo_bpm = Some(125.11);
        photek.tempo_confidence = Some(0.533);
        for preference in ["adaptive", "club_mix", "drop_swap", "cut", "wildcard"] {
            assert_eq!(
                crate::Planner::plan_adaptive(
                    &photek,
                    &dorado,
                    &Policy {
                        preferred_strategy: preference.into(),
                        mix_intent: MixIntent::Bold,
                        ..Policy::default()
                    }
                )
                .template,
                "SafeCrossfade"
            );
        }
    }

    #[test]
    fn contradictory_grid_confidence_cannot_certify_a_phrase_or_drop() {
        let mut p = profile();
        p.beat_confidence = Some(1.0);
        p.tempo_bpm = Some(180.0); // 3:2 disagrees with the 120 BPM grid.
        p.tempo_confidence = Some(0.9);
        p.manual_drop_seconds = vec![32.0];
        let plan = crate::Planner::plan_adaptive(
            &p,
            &p,
            &Policy {
                preferred_strategy: "drop_swap".into(),
                ..Policy::default()
            },
        );
        assert_eq!(plan.template, "QuickMix");
        p.tempo_confidence = Some(0.2);
        assert_eq!(
            crate::Planner::plan_adaptive(&p, &p, &Policy::default()).template,
            "SafeCrossfade"
        );
        p.tempo_bpm = Some(240.0); // Normal half/double agreement remains valid.
        p.tempo_confidence = Some(0.9);
        assert!(!matches!(
            crate::Planner::plan_adaptive(&p, &p, &Policy::default())
                .template
                .as_str(),
            "SafeCrossfade" | "QuickMix"
        ));
    }

    #[test]
    fn partial_tempo_uses_distinct_short_energy_envelopes_without_phase_claims() {
        let mut a = profile();
        a.grid_is_synthetic = true;
        a.beat_confidence = Some(0.5);
        a.tempo_bpm = Some(174.0);
        a.tempo_confidence = Some(0.8);
        let mut renders = Vec::new();
        for (energy, expected) in [
            (0.5, "QuickMix"),
            (0.65, "EnergyLift"),
            (0.35, "EnergyReset"),
        ] {
            let mut b = a.clone();
            b.energy = Some(energy);
            let plan = crate::Planner::plan_adaptive(&a, &b, &Policy::default());
            assert_eq!(plan.template, expected);
            assert_eq!(plan.deck_b_start_frame, 0);
            assert!(plan.resolve_at <= u64::from(plan.sample_rate) * 4);
            assert!(
                !plan
                    .automation
                    .iter()
                    .any(|e| matches!(e.param, Param::PlaybackRate(_)))
            );
            assert!(
                plan.decision
                    .as_ref()
                    .unwrap()
                    .reason
                    .contains("phase is unverified")
            );
            let mut p = plan.rescaled_to(8_000);
            p.channels = 1;
            let signal = |hz: f32| {
                (0..p.resolve_at as usize + 8_000)
                    .map(|i| (i as f32 * hz * std::f32::consts::TAU / 8_000.0).sin() * 0.2)
                    .collect::<Vec<_>>()
            };
            let qa = crate::qa::render_transition_qa(&p, &signal(180.0), &signal(270.0)).unwrap();
            assert!(qa.passed(), "{expected}: {qa:?}");
            let mut mixer = crate::Mixer::new(
                p.clone(),
                crate::deck::DeckBuffer::new(signal(180.0), 1),
                crate::deck::DeckBuffer::new(signal(270.0), 1),
                p.resolve_at as usize,
            )
            .unwrap();
            let mut audio = vec![0.0; p.resolve_at as usize];
            mixer.render_block(&mut audio, 0);
            renders.push(audio);
        }
        for pair in renders.windows(2) {
            let difference = pair[0]
                .iter()
                .zip(&pair[1])
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / pair[0].len() as f32;
            assert!(difference > 0.005, "actual PCM should differ: {difference}");
        }
    }

    #[test]
    fn a_measured_incoming_grid_does_not_disqualify_corroborated_partial_tempo() {
        let mut a = profile();
        a.grid_is_synthetic = true;
        a.beat_confidence = Some(0.5);
        a.bpm = Some(121.0);
        a.tempo_bpm = Some(122.009);
        a.tempo_confidence = Some(0.567618);
        let mut b = a.clone();
        b.grid_is_synthetic = false;
        b.beat_confidence = Some(0.65);
        b.tempo_bpm = Some(122.2335);
        b.tempo_confidence = Some(0.92);
        let plan = crate::Planner::plan_adaptive(&a, &b, &Policy::default());
        assert_ne!(plan.template, "SafeCrossfade");
        assert!(
            plan.decision
                .as_ref()
                .unwrap()
                .reason
                .contains("phase is unverified")
        );
        assert!(plan.resolve_at <= 4 * 48_000);
        a.bpm = Some(100.0);
        assert_eq!(
            crate::Planner::plan_adaptive(&a, &b, &Policy::default()).template,
            "SafeCrossfade"
        );
    }

    #[test]
    fn personality_changes_viable_styles_and_both_speed_biases_change_duration() {
        let a = profile();
        let mut b = profile();
        b.energy = Some(0.75);
        let safe = crate::Planner::plan_adaptive(
            &a,
            &b,
            &Policy {
                mix_intent: MixIntent::Safe,
                ..Policy::default()
            },
        );
        assert!(
            !safe
                .decision
                .unwrap()
                .candidates
                .iter()
                .any(|c| c.strategy == "EnergyLift")
        );
        let mut policy = Policy {
            preferred_strategy: "club_mix".into(),
            ..Policy::default()
        };
        let normal = crate::Planner::plan_adaptive(&a, &a, &policy);
        assert_eq!(normal.template, "ClubMix");
        policy.transition_speed_bias = TransitionSpeedBias::Faster;
        let fast = crate::Planner::plan_adaptive(&a, &a, &policy);
        policy.transition_speed_bias = TransitionSpeedBias::Slower;
        let slow = crate::Planner::plan_adaptive(&a, &a, &policy);
        assert!(fast.resolve_at < normal.resolve_at && normal.resolve_at < slow.resolve_at);
    }

    #[test]
    fn repetition_and_feedback_are_bounded_and_share_bass_swap_family() {
        let mut a = profile();
        a.vocals_known = false;
        let policy = Policy {
            recent_templates: vec!["LongHarmonicBlend".into(); 6],
            ..Policy::default()
        };
        let first = crate::Planner::plan_adaptive(&a, &a, &policy);
        assert_ne!(first.template, "LongHarmonicBlend");
        assert_eq!(family("BassSwap16"), family("BassSwap32"));
        let feedback = Policy {
            strategy_feedback: vec![("ClubMix".into(), 0.03); 100],
            ..Policy::default()
        };
        let chosen = crate::Planner::plan_adaptive(&a, &a, &feedback);
        assert_eq!(chosen.template, "ClubMix");
        assert!(
            chosen
                .decision
                .unwrap()
                .candidates
                .iter()
                .flat_map(|c| &c.components)
                .filter(|c| c.name == "feedback")
                .all(|c| c.value.abs() <= 0.06)
        );
    }

    #[test]
    fn automatic_drop_requires_local_structure_and_known_vocals_are_windowed() {
        let a = profile();
        let mut b = profile();
        b.drop_seconds = vec![32.0];
        b.breakdown_seconds = vec![24.0];
        assert_eq!(
            crate::Planner::plan_adaptive(&a, &b, &Policy::default()).template,
            "DropSwap"
        );
        b.energy_contour = vec![0.2, 0.7, 0.7];
        assert_ne!(
            crate::Planner::plan_adaptive(&a, &b, &Policy::default()).template,
            "DropSwap"
        );
        b.vocal_density_by_bar.fill(1.0);
        b.vocal_density_by_bar[..8].fill(0.0);
        let mut vocals = profile();
        vocals.vocal_density_by_bar.fill(1.0);
        assert_eq!(vocal_conflict(&vocals, &b, 0.0, 8.0), Some(0.0));
        assert_eq!(vocal_conflict(&vocals, &b, 16.0, 8.0), Some(1.0));
        b.vocals_known = false;
        assert_eq!(vocal_conflict(&vocals, &b, 0.0, 8.0), None);
    }

    #[test]
    fn distinct_styles_render_different_audio_and_pass_existing_qa() {
        let a = profile();
        let mut renders = Vec::new();
        for style in [
            "smooth_blend",
            "club_mix",
            "quick_mix",
            "bass_swap",
            "energy_lift",
            "energy_reset",
            "drop_swap",
            "cut",
        ] {
            let mut b = profile();
            if style == "energy_lift" {
                b.energy = Some(0.75);
            }
            if style == "energy_reset" {
                b.energy = Some(0.25);
            }
            if style == "drop_swap" {
                b.manual_drop_seconds = vec![32.0];
            }
            if style == "cut" {
                b.bpm = Some(150.0);
            }
            let planned = crate::Planner::plan_adaptive(
                &a,
                &b,
                &Policy {
                    preferred_strategy: style.into(),
                    ..Policy::default()
                },
            );
            assert_eq!(family(&planned.template), family(style));
            let mut p = planned.rescaled_to(8_000);
            p.channels = 1;
            let frames = (p.deck_b_start_frame + p.resolve_at + 8_000) as usize;
            let sine = |freq: f32| {
                (0..frames)
                    .map(|i| (i as f32 * freq * std::f32::consts::TAU / 8_000.0).sin() * 0.2)
                    .collect::<Vec<_>>()
            };
            let report = crate::qa::render_transition_qa(&p, &sine(220.0), &sine(330.0)).unwrap();
            assert!(report.passed(), "{style}: {report:?}");
            let mut mixer = crate::Mixer::new(
                p.clone(),
                crate::deck::DeckBuffer::new(sine(220.0), 1),
                crate::deck::DeckBuffer::new(sine(330.0), 1),
                p.resolve_at as usize,
            )
            .unwrap();
            let mut audio = vec![0.0; p.resolve_at as usize];
            mixer.render_block(&mut audio, 0);
            renders.push(audio);
        }
        // This exercises rendered PCM, not just template labels or event lists.
        for pair in renders.windows(2) {
            let count = pair[0].len().min(pair[1].len());
            let difference = pair[0][..count]
                .iter()
                .zip(&pair[1][..count])
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / count as f32;
            assert!(
                difference > 0.005,
                "styles should differ audibly: {difference}"
            );
        }
    }
}
