//! Verify rhythmic overlap using the PCM that will actually be mixed.
//! Opening-track grids are tempo hints, not proof of an outro's beat phase.
use crate::services::audio_analysis::onset::compute_onset_envelope_in_band;
use noor_mix::{AutomationEvent, Curve, DeckId, Param, TransitionProgram};

#[derive(Debug, Clone, Copy)]
pub(super) struct AudioBeatSync {
    pub rate: f32,
    pub cue_shift_frames: i64,
    pub confidence: f64,
    pub residual_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PulseRejection {
    IncompletePcm,
    InvalidAudio,
    InsufficientDynamics,
    TooFewOnsets,
    NoCoherentPulse {
        sparse: usize,
        coverage: usize,
        timing: usize,
    },
    WeakPulse,
    AmbiguousPhase,
}

impl std::fmt::Display for PulseRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompletePcm => f.write_str("complete decoded audio is not available"),
            Self::InvalidAudio => f.write_str("audio cannot be analysed reliably"),
            Self::InsufficientDynamics => f.write_str("bass dynamics do not support a beat lock"),
            Self::TooFewOnsets => f.write_str("too few bass onsets support the pulse"),
            Self::NoCoherentPulse {
                sparse,
                coverage,
                timing,
            } => write!(
                f,
                "no consistent pulse fit ({sparse} sparse, {coverage} coverage, {timing} timing rejections)"
            ),
            Self::WeakPulse => f.write_str("pulse strength is below the required support"),
            Self::AmbiguousPhase => f.write_str("competing offbeat pulses make phase ambiguous"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BeatSyncRejection {
    InvalidPlanTempo,
    WindowTooLong,
    Outgoing(PulseRejection),
    Incoming(PulseRejection),
    RateLimit,
    CueOutsideVerifiedWindow,
    DropTiming,
    ResidualLimit,
    AudioSafety,
}

impl std::fmt::Display for BeatSyncRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlanTempo => {
                f.write_str("the plan does not provide a supported pulse period")
            }
            Self::WindowTooLong => f.write_str("the analysis window exceeds its bounded duration"),
            Self::Outgoing(reason) => write!(f, "outgoing audio: {reason}"),
            Self::Incoming(reason) => write!(f, "incoming audio: {reason}"),
            Self::RateLimit => {
                f.write_str("matching the local tempos would exceed the 3% rate limit")
            }
            Self::CueOutsideVerifiedWindow => {
                f.write_str("the adjusted cue exceeds the verified audio window")
            }
            Self::DropTiming => f.write_str("beat correction would displace the verified drop cue"),
            Self::ResidualLimit => f.write_str("combined pulse timing exceeds the residual limit"),
            Self::AudioSafety => {
                f.write_str("the corrected plan does not pass audio safety checks")
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct BeatSyncFailures {
    attempts: Vec<(u64, BeatSyncRejection)>,
}

impl std::fmt::Display for BeatSyncFailures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, (duration_ms, reason)) in self.attempts.iter().take(3).enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{:.1}s window: {reason}", *duration_ms as f64 / 1_000.0)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Pulse {
    period: f64,
    phase: f64,
    confidence: f64,
    error: f64,
}

pub(super) fn required(program: &TransitionProgram) -> bool {
    (program.decision.is_some() || program.template == "DropPreview16")
        && program.resolve_at >= u64::from(program.sample_rate) * 6
        && matches!(
            program.template.as_str(),
            "BassSwap16"
                | "BassSwap32"
                | "ClubMix"
                | "QuickMix"
                | "LongHarmonicBlend"
                | "FilterSweep"
                | "EnergyLift"
                | "EnergyReset"
                | "DropSwap"
                | "DropPreview16"
        )
}

#[cfg(test)]
pub(super) fn synchronize(
    program: &mut TransitionProgram,
    outgoing: &[f32],
    incoming: &[f32],
) -> Option<AudioBeatSync> {
    synchronize_checked(program, outgoing, incoming).ok()
}

fn synchronize_checked(
    program: &mut TransitionProgram,
    outgoing: &[f32],
    incoming: &[f32],
) -> Result<AudioBeatSync, BeatSyncRejection> {
    let duration = program.resolve_at as f64 / f64::from(program.sample_rate.max(1));
    if duration > 30.0 {
        return Err(BeatSyncRejection::WindowTooLong);
    }
    let beats = f64::from(
        program
            .decision
            .as_ref()
            .ok_or(BeatSyncRejection::InvalidPlanTempo)?
            .duration_beats,
    );
    let expected_period = duration / beats;
    if !(0.25..=1.0).contains(&expected_period) {
        return Err(BeatSyncRejection::InvalidPlanTempo);
    }
    let old_rate = program
        .automation
        .iter()
        .find_map(|event| {
            (event.param == Param::PlaybackRate(DeckId::B)).then_some(f64::from(event.to))
        })
        .unwrap_or(1.0);
    let a = pulse_from_pcm_checked(
        outgoing,
        program.deck_a_start_frame,
        program.channels,
        program.sample_rate,
        duration,
        expected_period,
    )
    .map_err(BeatSyncRejection::Outgoing)?;
    // The old rate is a search hint, not a bound on corrected consumption.
    // Cover the maximum permitted rate and both directions of cue movement.
    let margin = expected_period * 1.06;
    let lead_frames = program
        .deck_b_start_frame
        .min((margin * f64::from(program.sample_rate)).ceil() as u64);
    let analysis_start = program.deck_b_start_frame - lead_frames;
    let lead_seconds = lead_frames as f64 / f64::from(program.sample_rate);
    let incoming_seconds = duration * 1.03 + margin + lead_seconds;
    if incoming_seconds > 33.0 {
        return Err(BeatSyncRejection::WindowTooLong);
    }
    let mut b = pulse_from_pcm_checked(
        incoming,
        analysis_start,
        program.channels,
        program.sample_rate,
        incoming_seconds,
        expected_period * old_rate,
    )
    .map_err(BeatSyncRejection::Incoming)?;
    b.phase = (b.phase - lead_seconds).rem_euclid(b.period);
    let rate = b.period / a.period;
    // Keep the established constant-B-rate handoff and its small nudge limits.
    if !(0.97..=1.03).contains(&rate) {
        return Err(BeatSyncRejection::RateLimit);
    }
    let mut shift =
        (b.phase - a.phase * rate + b.period / 2.0).rem_euclid(b.period) - b.period / 2.0;
    // Prefer the nearest beat, preserving phrase position when the cue has
    // decoded lead-in. At the start of a track only a forward shift is possible.
    if program.deck_b_start_frame as f64 / f64::from(program.sample_rate) + shift < 0.0 {
        shift += b.period;
    }
    // A verified drop cue must retain its structural arrival time.
    if program.drop_source.is_some() && shift.abs() > 0.03 {
        return Err(BeatSyncRejection::DropTiming);
    }
    let shift_frames = (shift * f64::from(program.sample_rate)).round() as i64;
    let start = program
        .deck_b_start_frame
        .checked_add_signed(shift_frames)
        .ok_or(BeatSyncRejection::CueOutsideVerifiedWindow)?;
    if let Some(drop) = program
        .decision
        .as_ref()
        .and_then(|decision| decision.incoming_drop_seconds)
    {
        let entry = f64::from(
            program
                .decision
                .as_ref()
                .ok_or(BeatSyncRejection::InvalidPlanTempo)?
                .incoming_entry_seconds,
        );
        let arrival =
            entry + shift + program.swap_start as f64 * rate / f64::from(program.sample_rate);
        if program.drop_source.is_some() && (arrival - f64::from(drop)).abs() > 0.03 {
            return Err(BeatSyncRejection::DropTiming);
        }
    }
    let consumed = (program.resolve_at as f64 * rate).ceil() as u64;
    let analysis_end = analysis_start
        .checked_add((incoming_seconds * f64::from(program.sample_rate)) as u64)
        .ok_or(BeatSyncRejection::CueOutsideVerifiedWindow)?;
    let consumed_end = start
        .checked_add(consumed + 1)
        .ok_or(BeatSyncRejection::CueOutsideVerifiedWindow)?;
    if start < analysis_start || consumed_end > analysis_end {
        return Err(BeatSyncRejection::CueOutsideVerifiedWindow);
    }
    if consumed_end > (incoming.len() / usize::from(program.channels.max(1))) as u64 {
        return Err(BeatSyncRejection::Incoming(PulseRejection::IncompletePcm));
    }
    // Check the entire decoded overlap, including one extra incoming beat
    // for the possible phase adjustment; do not project a short sample.
    let drift_bound = a.error + b.error / rate;
    if drift_bound > 0.04 {
        return Err(BeatSyncRejection::ResidualLimit);
    }
    program.deck_b_start_frame = start;
    program
        .automation
        .retain(|event| event.param != Param::PlaybackRate(DeckId::B));
    program.automation.push(AutomationEvent {
        param: Param::PlaybackRate(DeckId::B),
        start_sample: 0,
        end_sample: program.resolve_at,
        from: rate as f32,
        to: rate as f32,
        curve: Curve::Linear,
    });
    if let Some(decision) = program.decision.as_mut() {
        decision.incoming_entry_seconds += shift as f32;
        decision
            .reason
            .push_str("; beat phase and tempo verified in the decoded mix window");
    }
    Ok(AudioBeatSync {
        rate: rate as f32,
        cue_shift_frames: shift_frames,
        confidence: a.confidence.min(b.confidence),
        residual_ms: drift_bound * 1000.0,
    })
}

/// A long bass swap can retain its musical shape over a shorter verified
/// phrase. These are prefixes of the same source windows, never later cues.
pub(super) fn musical_prefixes(program: &TransitionProgram) -> Vec<TransitionProgram> {
    let Some(decision) = program.decision.as_ref() else {
        return vec![];
    };
    let beats = f64::from(decision.duration_beats);
    if program.template != "BassSwap32"
        || !beats.is_finite()
        || beats < 40.0
        || program.resolve_at < u64::from(program.sample_rate) * 12
        || !program.loops.is_empty()
        || program.drop_source.is_some()
        || decision.incoming_drop_seconds.is_some()
    {
        return vec![];
    }
    [32.0_f64, 16.0]
        .into_iter()
        .filter_map(|short_beats| {
            let frames = (program.resolve_at as f64 * short_beats / beats).round() as u64;
            if frames >= program.resolve_at || frames < u64::from(program.sample_rate) * 6 {
                return None;
            }
            let scale = |frame: u64| {
                (u128::from(frame) * u128::from(frames) / u128::from(program.resolve_at)) as u64
            };
            let mut prefix = program.clone();
            prefix.template = "BassSwap16".into();
            prefix.sync_start = scale(prefix.sync_start);
            prefix.intro_start = scale(prefix.intro_start);
            prefix.swap_start = scale(prefix.swap_start);
            prefix.fade_start = scale(prefix.fade_start);
            prefix.resolve_at = frames;
            for event in &mut prefix.automation {
                event.start_sample = scale(event.start_sample);
                event.end_sample = scale(event.end_sample);
            }
            let decision = prefix.decision.as_mut()?;
            decision.strategy = "BassSwap16".into();
            decision.duration_beats = short_beats as f32;
            prefix.validate().is_ok().then_some(prefix)
        })
        .collect()
}

/// Verify the full plan first. Each shorter alternative must independently
/// pass the same complete-window phase, tempo, coverage and safety checks.
pub(super) fn synchronize_or_shorten_checked(
    program: &mut TransitionProgram,
    outgoing: &[f32],
    incoming: &[f32],
) -> Result<AudioBeatSync, BeatSyncFailures> {
    let original = program.clone();
    let prefixes = musical_prefixes(&original);
    let mut attempts = Vec::with_capacity(3);
    for (index, mut candidate) in std::iter::once(original).chain(prefixes).enumerate() {
        let duration_ms =
            candidate.resolve_at.saturating_mul(1_000) / u64::from(candidate.sample_rate.max(1));
        let sync = match synchronize_checked(&mut candidate, outgoing, incoming) {
            Ok(sync) => sync,
            Err(reason) => {
                attempts.push((duration_ms, reason));
                continue;
            }
        };
        if noor_mix::planner::safety::validate_audio_safety(
            &candidate,
            &noor_mix::planner::safety::AudioSafetyPolicy::default(),
        )
        .is_err()
        {
            attempts.push((duration_ms, BeatSyncRejection::AudioSafety));
            continue;
        }
        if index > 0
            && let Some(decision) = candidate.decision.as_mut()
        {
            decision
                .reason
                .push_str("; shorter verified phrase avoids the unavailable or unstable tail");
        }
        *program = candidate;
        return Ok(sync);
    }
    Err(BeatSyncFailures { attempts })
}

#[cfg(test)]
fn pulse_from_pcm(
    samples: &[f32],
    start_frame: u64,
    channels: u16,
    sample_rate: u32,
    seconds: f64,
    expected_period: f64,
) -> Option<Pulse> {
    pulse_from_pcm_checked(
        samples,
        start_frame,
        channels,
        sample_rate,
        seconds,
        expected_period,
    )
    .ok()
}

fn pulse_from_pcm_checked(
    samples: &[f32],
    start_frame: u64,
    channels: u16,
    sample_rate: u32,
    seconds: f64,
    expected_period: f64,
) -> Result<Pulse, PulseRejection> {
    let channels = usize::from(channels.max(1));
    let start = usize::try_from(start_frame)
        .ok()
        .and_then(|frame| frame.checked_mul(channels))
        .ok_or(PulseRejection::InvalidAudio)?;
    let count = (seconds * f64::from(sample_rate)) as usize;
    let end = count
        .checked_mul(channels)
        .and_then(|count| start.checked_add(count))
        .ok_or(PulseRejection::InvalidAudio)?;
    let clip = match samples.get(start..end) {
        Some(clip) => clip,
        None => {
            tracing::info!(
                start_frame,
                required_frames = count,
                available_frames = samples.len() / channels,
                "DJ beat verification waiting for complete decoded overlap"
            );
            return Err(PulseRejection::IncompletePcm);
        }
    };
    // Use the existing spectral-flux analyser, bounded to 33 seconds and ~24kHz.
    // Bass emphasis reduces the risk of aligning an offbeat hat against a kick.
    let decimation = (sample_rate / 24_000).max(1) as usize;
    let rate = sample_rate / decimation as u32;
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * 180.0 / f64::from(rate)).exp();
    let (mut low_a, mut low_b) = (0.0, 0.0);
    let mono: Vec<f32> = clip
        .chunks(channels * decimation)
        .map(|chunk| {
            let input = chunk.iter().map(|s| f64::from(*s)).sum::<f64>() / chunk.len() as f64;
            low_a += alpha * (input - low_a);
            low_b += alpha * (low_a - low_b);
            low_b as f32
        })
        .collect();
    // Flux normalization can amplify floating-point noise in a steady tone
    // into apparent onsets. Require real dynamics in the underlying audio.
    let mut energies: Vec<f64> = mono
        .chunks((rate / 50).max(1) as usize)
        .skip(10)
        .map(|chunk| {
            (chunk
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>()
                / chunk.len() as f64)
                .sqrt()
        })
        .collect();
    if energies.len() < 16 || energies.iter().any(|value| !value.is_finite()) {
        return Err(PulseRejection::InvalidAudio);
    }
    energies.sort_by(f64::total_cmp);
    let low = energies[energies.len() / 10];
    let high = energies[energies.len() * 9 / 10];
    if high < 1e-7 || high < low * 1.4 {
        tracing::debug!(
            low,
            high,
            "DJ beat lock rejected: insufficient bass dynamics"
        );
        return Err(PulseRejection::InsufficientDynamics);
    }
    let envelope = compute_onset_envelope_in_band(&mono, rate, 30.0, 150.0)
        .ok_or(PulseRejection::TooFewOnsets)?;
    let peaks: Vec<(f64, f64)> = envelope
        .odf
        .windows(5)
        .enumerate()
        .filter_map(|(i, window)| {
            let peak = window[2];
            (peak >= 0.3 && window.iter().all(|v| *v <= peak))
                .then_some(((i + 2) as f64 * envelope.hop_seconds, peak))
        })
        .collect();
    fit_pulse_checked(&peaks, seconds, expected_period)
}

fn closest(peaks: &[(f64, f64)], time: f64) -> Option<(f64, f64)> {
    let i = peaks.partition_point(|(t, _)| *t < time);
    [i.checked_sub(1), (i < peaks.len()).then_some(i)]
        .into_iter()
        .flatten()
        .map(|i| peaks[i])
        .min_by(|a, b| (a.0 - time).abs().total_cmp(&(b.0 - time).abs()))
}

#[cfg(test)]
fn fit_pulse(peaks: &[(f64, f64)], seconds: f64, expected: f64) -> Option<Pulse> {
    fit_pulse_checked(peaks, seconds, expected).ok()
}

fn fit_pulse_checked(
    peaks: &[(f64, f64)],
    seconds: f64,
    expected: f64,
) -> Result<Pulse, PulseRejection> {
    if peaks.len() < 8 {
        tracing::debug!(
            peaks = peaks.len(),
            "DJ beat lock rejected: too few bass onsets"
        );
        return Err(PulseRejection::TooFewOnsets);
    }
    let mut best: Option<(f64, Pulse)> = None;
    let mut candidates = Vec::new();
    let mut rejected = [0_usize; 3];
    for step in -40..=40 {
        let period = expected * (1.0 + f64::from(step) * 0.0005);
        for &(phase, _) in peaks.iter().take_while(|(t, _)| *t < expected * 1.1) {
            // A seed onset only proposes the pulse. Refine its phase and
            // period against the observed window before judging quality or
            // comparing it with another phase; the first onset can be noisy.
            match refined_pulse_candidate(peaks, seconds, expected, period, phase) {
                Ok((score, pulse)) => {
                    candidates.push((score, pulse));
                    if best.is_none_or(|(s, _)| score > s) {
                        best = Some((score, pulse));
                    }
                }
                Err(reason) => rejected[reason as usize] += 1,
            }
        }
    }
    let Some((score, pulse)) = best else {
        tracing::debug!(
            expected,
            sparse_candidates = rejected[0],
            coverage_rejections = rejected[1],
            drift_rejections = rejected[2],
            "DJ beat lock rejected: no coherent whole-window pulse"
        );
        return Err(PulseRejection::NoCoherentPulse {
            sparse: rejected[0],
            coverage: rejected[1],
            timing: rejected[2],
        });
    };
    if score < 0.5 {
        tracing::debug!(
            score,
            expected,
            "DJ beat lock rejected: weak periodic pulse"
        );
        return Err(PulseRejection::WeakPulse);
    }
    // Equally strong offbeat bass cannot certify which pulse is the kick.
    // Competing half-beat phases must have a meaningful separation in score.
    if candidates.iter().any(|(other_score, other)| {
        let distance = (other.phase - pulse.phase).abs().rem_euclid(pulse.period);
        (other.period / pulse.period - 1.0).abs() < 0.01
            && distance.min(pulse.period - distance) > pulse.period * 0.3
            && *other_score >= score * 0.9
    }) {
        tracing::debug!(
            score,
            period = pulse.period,
            phase = pulse.phase,
            "DJ beat lock rejected: competing offbeat phase"
        );
        return Err(PulseRejection::AmbiguousPhase);
    }
    Ok(pulse)
}

#[derive(Clone, Copy)]
enum PulseCandidateRejection {
    Sparse,
    Coverage,
    Drift,
}

fn pulse_matches(
    peaks: &[(f64, f64)],
    count: usize,
    period: f64,
    phase: f64,
) -> Vec<(f64, f64, f64)> {
    (0..count)
        .filter_map(|beat| {
            let predicted = phase + beat as f64 * period;
            let (time, weight) = closest(peaks, predicted)?;
            ((time - predicted).abs() <= 0.035).then_some((beat as f64, time, weight))
        })
        .collect()
}

fn refined_pulse_candidate(
    peaks: &[(f64, f64)],
    seconds: f64,
    expected: f64,
    period: f64,
    phase: f64,
) -> Result<(f64, Pulse), PulseCandidateRejection> {
    let count = ((seconds - phase) / period).floor() as usize;
    let coarse = pulse_matches(peaks, count, period, phase);
    if coarse.len() < 8 {
        return Err(PulseCandidateRejection::Sparse);
    }
    let n = coarse.len() as f64;
    let x = coarse.iter().map(|(x, _, _)| x).sum::<f64>() / n;
    let y = coarse.iter().map(|(_, y, _)| y).sum::<f64>() / n;
    let slope = coarse
        .iter()
        .map(|(a, b, _)| (a - x) * (b - y))
        .sum::<f64>()
        / coarse.iter().map(|(a, _, _)| (a - x).powi(2)).sum::<f64>();
    let origin = y - slope * x;
    let coarse_error = coarse
        .iter()
        .map(|(a, b, _)| (b - origin - slope * a).abs())
        .fold(0.0_f64, f64::max);
    // Refinement stays inside the original association radius and tempo
    // bound. Every seed match must still meet the existing residual limit.
    if !slope.is_finite()
        || !origin.is_finite()
        || (slope / expected - 1.0).abs() > 0.025
        || (origin - phase).abs() > 0.035
        || coarse_error > 0.02
    {
        return Err(PulseCandidateRejection::Drift);
    }
    let phase = origin.rem_euclid(slope);
    let count = ((seconds - phase) / slope).floor() as usize;
    // Reassociate against the fitted pulse, then enforce all coverage and
    // residual gates over the full window. A good local fit is insufficient.
    let matched = pulse_matches(peaks, count, slope, phase);
    if matched.len() < 8 || matched.len() * 4 < count * 3 {
        return Err(PulseCandidateRejection::Sparse);
    }
    // A good opening must not hide drift or a breakdown in the last quarter.
    // Evidence must span the seam and the end, with at most two missed beats.
    if matched[0].0 > 1.0
        || matched[matched.len() - 1].0 < count.saturating_sub(2) as f64
        || matched.windows(2).any(|pair| pair[1].0 - pair[0].0 > 3.0)
    {
        return Err(PulseCandidateRejection::Coverage);
    }
    for segment_start in (0..count).step_by(8) {
        let segment_end = (segment_start + 8).min(count);
        let covered = matched
            .iter()
            .filter(|(beat, _, _)| *beat >= segment_start as f64 && *beat < segment_end as f64)
            .count();
        if covered * 4 < (segment_end - segment_start) * 3 {
            return Err(PulseCandidateRejection::Coverage);
        }
    }
    let error = matched
        .iter()
        .map(|(beat, time, _)| (time - phase - slope * beat).abs())
        .fold(0.0_f64, f64::max);
    // Irregular/drifting percussion is not a trustworthy fixed-rate beat lock.
    if error > 0.02 {
        return Err(PulseCandidateRejection::Drift);
    }
    let strength = matched
        .iter()
        .map(|(beat, time, weight)| {
            weight * (1.0 - (time - phase - slope * beat).abs() / 0.04).max(0.0)
        })
        .sum::<f64>();
    let score = strength / count as f64 - phase * 0.001;
    Ok((
        score,
        Pulse {
            period: slope,
            phase,
            confidence: score.min(1.0) * matched.len() as f64 / count as f64,
            error,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rhythmic_program(seconds: u32, old_rate: f32) -> TransitionProgram {
        let mut program = noor_mix::planner::bass_swap_16_program(24_000, 1, seconds * 1000);
        program.automation.push(AutomationEvent {
            param: Param::PlaybackRate(DeckId::B),
            start_sample: 0,
            end_sample: program.resolve_at,
            from: old_rate,
            to: old_rate,
            curve: Curve::Linear,
        });
        program.decision = Some(noor_mix::program::TransitionDecision {
            strategy: "BassSwap16".into(),
            confidence: 0.65,
            score: 0.8,
            reason: "Rhythmic overlap".into(),
            energy_direction: "steady".into(),
            incoming_entry_seconds: 0.0,
            incoming_drop_seconds: None,
            outgoing_window: "phrase_end".into(),
            duration_beats: seconds as f32 * 2.0,
            candidates: vec![],
        });
        program
    }

    fn kicks(period: f64, seconds: u32, late_jump: Option<f64>) -> Vec<f32> {
        let mut samples = vec![0.0; seconds as usize * 24_000];
        let mut time = 0.02;
        while time < f64::from(seconds) {
            let actual = time
                + if late_jump.is_some_and(|after| time >= after) {
                    0.17
                } else {
                    0.0
                };
            let start = (actual * 24_000.0).round() as usize;
            for frame in 0..1200 {
                let t = frame as f64 / 24_000.0;
                if let Some(sample) = samples.get_mut(start + frame) {
                    *sample +=
                        ((2.0 * std::f64::consts::PI * 60.0 * t).cos() * (-t * 80.0).exp()) as f32;
                }
            }
            time += period;
        }
        samples
    }

    #[test]
    fn checked_verification_distinguishes_missing_audio_and_unsupported_dynamics() {
        let complete = kicks(0.5, 16, None);
        let partial = kicks(0.5, 3, None);
        assert_eq!(
            synchronize_checked(&mut rhythmic_program(12, 1.0), &partial, &complete).err(),
            Some(BeatSyncRejection::Outgoing(PulseRejection::IncompletePcm))
        );
        assert_eq!(
            synchronize_checked(&mut rhythmic_program(12, 1.0), &complete, &partial).err(),
            Some(BeatSyncRejection::Incoming(PulseRejection::IncompletePcm))
        );
        assert_eq!(
            synchronize_checked(
                &mut rhythmic_program(12, 1.0),
                &complete,
                &vec![0.0; 24_000 * 16],
            )
            .err(),
            Some(BeatSyncRejection::Incoming(
                PulseRejection::InsufficientDynamics
            ))
        );
    }

    #[test]
    fn checked_verification_keeps_each_bounded_prefix_failure() {
        let partial = kicks(0.5, 3, None);
        let mut program = rhythmic_program(24, 1.0);
        program.template = "BassSwap32".into();
        let failures = synchronize_or_shorten_checked(&mut program, &partial, &partial)
            .expect_err("unavailable audio must not certify a shorter phrase");
        assert_eq!(failures.attempts.len(), 3);
        assert_eq!(
            failures
                .attempts
                .iter()
                .map(|(duration, _)| *duration)
                .collect::<Vec<_>>(),
            vec![24_000, 16_000, 8_000]
        );
        assert!(failures.attempts.iter().all(|(_, failure)| {
            *failure == BeatSyncRejection::Outgoing(PulseRejection::IncompletePcm)
        }));
        let reason = failures.to_string();
        assert!(
            reason
                .contains("24.0s window: outgoing audio: complete decoded audio is not available")
        );
        assert!(reason.contains("8.0s window:"));
        assert!(reason.len() < 400);
        assert_eq!(program.template, "BassSwap32");
        assert_eq!(program.deck_b_start_frame, 0);
    }

    #[test]
    fn checked_fit_reports_weak_and_ambiguous_support_separately() {
        let weak: Vec<_> = (0..32)
            .map(|beat| (0.03 + beat as f64 * 0.5, 0.49))
            .collect();
        assert_eq!(
            fit_pulse_checked(&weak, 16.0, 0.5).err(),
            Some(PulseRejection::WeakPulse)
        );
        let ambiguous: Vec<_> = (0..32)
            .flat_map(|beat| {
                [
                    (0.03 + beat as f64 * 0.5, 1.0),
                    (0.28 + beat as f64 * 0.5, 1.0),
                ]
            })
            .collect();
        assert_eq!(
            fit_pulse_checked(&ambiguous, 16.0, 0.5).err(),
            Some(PulseRejection::AmbiguousPhase)
        );
    }

    #[test]
    fn corrected_consumption_cannot_hide_drift_beyond_the_old_rate_window() {
        let a = kicks(0.491, 32, None);
        let b = kicks(0.493, 32, None);
        let mut program = rhythmic_program(28, 0.97);
        assert!(synchronize(&mut program, &a, &b).is_some());
        let drifted = kicks(0.493, 32, Some(27.7));
        let mut program = rhythmic_program(28, 0.97);
        assert!(synchronize(&mut program, &a, &drifted).is_none());
    }

    #[test]
    fn drop_arrival_uses_source_time_and_rejects_rate_induced_displacement() {
        let a = kicks(0.5, 18, None);
        let b = kicks(0.5, 18, None);
        let mut program = rhythmic_program(12, 1.0);
        program.drop_source = Some("manual".into());
        program.deck_b_start_frame = 48_000; // local 2s; source buffer begins at 100s
        let decision = program.decision.as_mut().unwrap();
        decision.incoming_entry_seconds = 102.0;
        decision.incoming_drop_seconds = Some(102.0 + program.swap_start as f32 / 24_000.0);
        assert!(synchronize(&mut program.clone(), &a, &b).is_some());
        let slower = kicks(0.505, 18, None);
        assert!(synchronize(&mut program, &a, &slower).is_none());
    }
    #[test]
    fn rejects_silence_and_irregular_percussion() {
        assert!(pulse_from_pcm(&vec![0.0; 48_000 * 8], 0, 1, 48_000, 8.0, 0.5).is_none());
        let peaks: Vec<_> = (0..24)
            .map(|i| (i as f64 * 0.5 + if i % 3 == 0 { 0.17 } else { 0.0 }, 1.0))
            .collect();
        assert!(fit_pulse(&peaks, 12.0, 0.5).is_none());
    }

    #[test]
    fn rejects_a_late_phase_jump_or_missing_last_quarter() {
        let shifted: Vec<_> = (0..32)
            .map(|beat| {
                (
                    0.03 + beat as f64 * 0.5 + if beat >= 24 { 0.17 } else { 0.0 },
                    1.0,
                )
            })
            .collect();
        assert!(fit_pulse(&shifted, 16.0, 0.5).is_none());
        assert!(fit_pulse(&shifted[..24], 16.0, 0.5).is_none());
    }
    #[test]
    fn coherent_pulse_is_scored_after_refining_a_jittered_first_onset() {
        // Pinning every candidate to the first onset gave this coherent
        // moderate-strength pulse a best coarse score of about .485, below
        // the unchanged .5 floor. Its fitted whole-window pulse clears .5.
        let peaks: Vec<_> = (0..32)
            .map(|beat| {
                (
                    0.173 + beat as f64 * 0.5 + if beat == 0 { 0.019 } else { 0.0 },
                    0.6,
                )
            })
            .collect();
        let pulse = fit_pulse(&peaks, 16.0, 0.5)
            .expect("a bounded whole-window fit must remove seed-onset scoring bias");
        assert!((pulse.period - 0.5).abs() < 0.0002);
        assert!((pulse.phase - 0.173).abs() < 0.003);
        assert!(pulse.error <= 0.02);
        assert!(pulse.confidence >= 0.5);

        // Refining phase cannot turn genuinely weak onsets into confidence.
        let weak: Vec<_> = peaks.iter().map(|(time, _)| (*time, 0.49)).collect();
        assert!(fit_pulse(&weak, 16.0, 0.5).is_none());
    }

    #[test]
    fn fits_tempo_and_phase_without_following_offbeat_subdivisions() {
        let peaks: Vec<_> = (0..32)
            .flat_map(|i| {
                let t = 0.173 + i as f64 * 60.0 / 121.0;
                [(t, 1.0), (t + 0.2, 0.32)]
            })
            .collect();
        let pulse = fit_pulse(&peaks, 16.0, 0.5).unwrap();
        assert!((pulse.period - 60.0 / 121.0).abs() < 0.0001);
        assert!((pulse.phase - 0.173).abs() < 0.0001);
    }

    #[test]
    fn high_frequency_offbeats_do_not_move_the_verified_bass_phase() {
        let mut samples = kicks(0.5, 12, None);
        let reference = pulse_from_pcm(&samples, 0, 1, 24_000, 12.0, 0.5).unwrap();
        for beat in 0..24 {
            let start = ((0.27 + beat as f64 * 0.5) * 24_000.0).round() as usize;
            for frame in 0..480 {
                let t = frame as f64 / 24_000.0;
                let window = (std::f64::consts::PI * frame as f64 / 480.0).sin().powi(2);
                if let Some(sample) = samples.get_mut(start + frame) {
                    *sample +=
                        (4.0 * window * (2.0 * std::f64::consts::PI * 3000.0 * t).sin()) as f32;
                }
            }
        }
        let pulse = pulse_from_pcm(&samples, 0, 1, 24_000, 12.0, 0.5)
            .expect("strong hats must not veto or replace a stable bass pulse");
        assert!((pulse.phase - reference.phase).abs() < 0.02);
        assert!((pulse.period - reference.period).abs() < 0.001);
    }

    #[test]
    fn broadband_noise_hats_do_not_replace_the_verified_kick_phase() {
        use rustfft::num_complex::Complex;

        const SAMPLE_RATE: u32 = 24_000;
        const SECONDS: usize = 12;
        let frames = SAMPLE_RATE as usize * SECONDS;
        let mut seed = 5_u64;
        // Unit-variance deterministic white noise, high-passed before the
        // hat envelope. A broad transient exercises every surviving FFT bin;
        // a single high-frequency tone does not expose the log-flux bias.
        let mut noise: Vec<Complex<f32>> = (0..frames)
            .map(|_| {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let value =
                    ((seed >> 32) as u32 as f64 / f64::from(u32::MAX) * 2.0 - 1.0) * 3.0_f64.sqrt();
                Complex::new(value as f32, 0.0)
            })
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(frames).process(&mut noise);
        for (bin, value) in noise.iter_mut().enumerate() {
            let frequency = bin.min(frames - bin) as f64 * f64::from(SAMPLE_RATE) / frames as f64;
            if frequency < 3_000.0 {
                *value = Complex::new(0.0, 0.0);
            }
        }
        planner.plan_fft_inverse(frames).process(&mut noise);

        let mut plain_kicks = vec![0.0; frames];
        let mut mixed = vec![0.0; frames];
        for beat in 0..24 {
            let kick_start = ((0.12 + beat as f64 * 0.5) * f64::from(SAMPLE_RATE)).round() as usize;
            for frame in 0..(SAMPLE_RATE as usize * 12 / 100) {
                if let Some(sample) = plain_kicks.get_mut(kick_start + frame) {
                    let t = frame as f64 / f64::from(SAMPLE_RATE);
                    *sample += (0.4
                        * (2.0 * std::f64::consts::PI * 60.0 * t).cos()
                        * (-80.0 * t).exp()) as f32;
                }
            }
            let hat_start = ((0.37 + beat as f64 * 0.5) * f64::from(SAMPLE_RATE)).round() as usize;
            for frame in 0..(SAMPLE_RATE as usize * 75 / 1_000) {
                let index = hat_start + frame;
                if let Some(sample) = mixed.get_mut(index) {
                    let t = frame as f64 / f64::from(SAMPLE_RATE);
                    *sample += (0.4 * f64::from(noise[index].re) / frames as f64
                        * (-140.0 * t).exp()) as f32;
                }
            }
        }
        for (sample, kick) in mixed.iter_mut().zip(&plain_kicks) {
            *sample += kick;
        }
        let reference = pulse_from_pcm(&plain_kicks, 0, 1, SAMPLE_RATE, SECONDS as f64, 0.5)
            .expect("plain kicks must establish the reference phase");
        let pulse = pulse_from_pcm(&mixed, 0, 1, SAMPLE_RATE, SECONDS as f64, 0.5)
            .expect("noise hats must not veto the supported quarter-beat kick pulse");
        let phase_difference = (pulse.phase - reference.phase).rem_euclid(reference.period);
        assert!(
            phase_difference.min(reference.period - phase_difference) < 0.02,
            "noise hats replaced the kick phase: reference={}, fitted={}",
            reference.phase,
            pulse.phase
        );
        assert!((pulse.period - reference.period).abs() < 0.001);
    }

    #[test]
    fn equally_strong_offbeats_cannot_certify_kick_phase() {
        let peaks: Vec<_> = (0..32)
            .flat_map(|beat| {
                [
                    (0.03 + beat as f64 * 0.5, 1.0),
                    (0.28 + beat as f64 * 0.5, 1.0),
                ]
            })
            .collect();
        assert!(fit_pulse(&peaks, 16.0, 0.5).is_none());
    }
}
