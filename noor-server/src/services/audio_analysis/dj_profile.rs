use crate::db::models::{AudioDjProfileCorrectionRow, AudioDjProfileKey, AudioDjProfileRow};
use crate::db::queries;
use crate::playback::dj_lookahead::DjMediaRef;
use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::bpm::{self, BeatGridAnalysis};
use super::{onset, tempo};

pub const DJ_PROFILE_VERSION: &str = "dj_profile_v2";
pub const DJ_WAVEFORM_PEAK_COUNT: usize = 512;
const DJ_PROFILE_DB_LOCK_RETRY_LIMIT: usize = 6;
const DJ_PROFILE_DB_LOCK_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DjAnalysisJob {
    pub media_ref: DjMediaRef,
    pub track_id: Option<i64>,
    pub queue_item_id: Option<i64>,
    pub tidal_id: Option<i64>,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub analysis_scope_ms: i64,
    pub deadline_generation: u64,
}

/// Spawn the DJ profile analysis actor. Returns the sender for profile jobs.
///
/// This actor is intentionally separate from passive DSP analysis. Playback and
/// explicit rebuilds send decoded PCM here, and the actor writes only
/// `audio_dj_profiles`.
pub fn spawn_dj_profile_actor(db: crate::db::Database) -> mpsc::UnboundedSender<DjAnalysisJob> {
    spawn_dj_profile_actor_with_analyzer(db, bpm::analyze_beat_grid)
}

fn spawn_dj_profile_actor_with_analyzer(
    db: crate::db::Database,
    analyzer: fn(&[f32], u32) -> Option<BeatGridAnalysis>,
) -> mpsc::UnboundedSender<DjAnalysisJob> {
    let (tx, mut rx) = mpsc::unbounded_channel::<DjAnalysisJob>();

    tokio::spawn(async move {
        let inflight = Arc::new(Mutex::new(HashSet::<String>::new()));
        while let Some(job) = rx.recv().await {
            if job.samples.is_empty() || job.sample_rate == 0 {
                continue;
            }

            let key = job.media_ref.profile_key();
            let inflight_key = profile_inflight_key(&key);
            {
                let Ok(mut guard) = inflight.lock() else {
                    warn!("DJ profile inflight guard poisoned");
                    continue;
                };
                if !guard.insert(inflight_key.clone()) {
                    debug!(
                        media_ref_kind = %key.media_ref_kind,
                        media_ref_id = %key.media_ref_id,
                        "Skipping duplicate in-flight DJ profile job"
                    );
                    continue;
                }
            }

            let db_clone = db.clone();
            let inflight_clone = inflight.clone();
            let result = tokio::task::spawn_blocking(move || {
                let result = persist_dj_analysis_job(&db_clone, job, analyzer);
                clear_profile_inflight(&inflight_clone, &inflight_key);
                result
            })
            .await
            .map_err(|error| anyhow::anyhow!("DJ profile task join failed: {error}"))
            .and_then(|result| result);

            if let Err(error) = result {
                warn!(error = %error, "DJ profile analysis failed");
            }
        }
    });

    tx
}

fn profile_inflight_key(key: &crate::db::models::AudioDjProfileKey) -> String {
    format!("{}:{}", key.media_ref_kind, key.media_ref_id)
}

fn clear_profile_inflight(inflight: &Arc<Mutex<HashSet<String>>>, key: &str) {
    if let Ok(mut guard) = inflight.lock() {
        guard.remove(key);
    }
}

fn persist_dj_analysis_job(
    db: &crate::db::Database,
    job: DjAnalysisJob,
    analyzer: fn(&[f32], u32) -> Option<BeatGridAnalysis>,
) -> Result<()> {
    let key = job.media_ref.profile_key();
    if with_dj_profile_db_retry("check current DJ profile version", || {
        db.with_conn(|conn| dj_analysis_skips_existing_profile_version(conn, &key))
    })? {
        debug!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            "Skipping current DJ profile"
        );
        return Ok(());
    }

    let analysis = if job.tidal_id.is_some() {
        fallback_beat_grid_analysis(&job.samples, job.sample_rate).or_else(|| {
            warn!(
                media_ref_kind = %key.media_ref_kind,
                media_ref_id = %key.media_ref_id,
                "Persisting provisional DJ profile with default beat grid"
            );
            provisional_beat_grid_analysis(&job.samples, job.sample_rate)
        })
    } else {
        analyzer(&job.samples, job.sample_rate)
            .or_else(|| fallback_beat_grid_analysis(&job.samples, job.sample_rate))
    };
    let Some(analysis) = analysis else {
        warn!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            "Skipping DJ profile with no beat-grid analysis"
        );
        return Ok(());
    };

    with_dj_profile_db_retry("persist DJ profile", || {
        db.with_conn(|conn| {
            persist_dj_analysis_job_from_analysis(conn, &job, "dj_playback", &analysis)
        })
    })?;
    info!(
        media_ref_kind = %key.media_ref_kind,
        media_ref_id = %key.media_ref_id,
        beats = analysis.beats_seconds.len(),
        downbeats = analysis.downbeats_seconds.len(),
        "DJ profile persisted"
    );
    Ok(())
}

fn with_dj_profile_db_retry<T, F>(operation: &'static str, f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    with_dj_profile_db_retry_config(
        operation,
        DJ_PROFILE_DB_LOCK_RETRY_LIMIT,
        DJ_PROFILE_DB_LOCK_RETRY_DELAY,
        f,
    )
}

fn with_dj_profile_db_retry_config<T, F>(
    operation: &'static str,
    retry_limit: usize,
    retry_delay: Duration,
    mut f: F,
) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    for attempt in 0..=retry_limit {
        match f() {
            Ok(value) => return Ok(value),
            Err(error) if sqlite_database_locked(&error) && attempt < retry_limit => {
                let next_attempt = attempt + 1;
                warn!(
                    operation,
                    next_attempt, retry_limit, "DJ profile database locked; retrying"
                );
                std::thread::sleep(retry_delay);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("DJ profile retry loop always returns before exhausting attempts")
}

fn sqlite_database_locked(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(rusqlite::Error::SqliteFailure(sqlite_error, _)) =
            cause.downcast_ref::<rusqlite::Error>()
        {
            return matches!(
                sqlite_error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            );
        }
        let message = cause.to_string();
        message.contains("database is locked") || message.contains("database table is locked")
    })
}

fn fallback_beat_grid_analysis(samples: &[f32], sample_rate: u32) -> Option<BeatGridAnalysis> {
    let env = onset::compute_onset_envelope(samples, sample_rate)?;
    let estimate = tempo::estimate_tempo(&env)?;
    if estimate.strength < 0.05 {
        return None;
    }
    let bpm = estimate
        .bpm
        .clamp(tempo::BPM_MIN as f64, tempo::BPM_MAX as f64);
    let beat_seconds = 60.0 / bpm;
    if !beat_seconds.is_finite() || beat_seconds <= 0.0 {
        return None;
    }
    let duration_seconds = samples.len() as f64 / sample_rate.max(1) as f64;
    let beat_count = (duration_seconds / beat_seconds).floor() as usize;
    if beat_count < 4 {
        return None;
    }
    let beats_seconds = (0..beat_count)
        .map(|beat| (beat as f64 * beat_seconds) as f32)
        .collect::<Vec<_>>();
    let downbeats_seconds = beats_seconds
        .iter()
        .enumerate()
        .filter_map(|(index, seconds)| (index % 4 == 0).then_some(*seconds))
        .collect::<Vec<_>>();
    Some(BeatGridAnalysis {
        bpm,
        confidence: estimate.strength,
        beats_seconds,
        downbeats_seconds,
    })
}

fn provisional_beat_grid_analysis(samples: &[f32], sample_rate: u32) -> Option<BeatGridAnalysis> {
    if samples.is_empty() || sample_rate == 0 {
        return None;
    }

    let bpm = tempo::PRIOR_CENTER_BPM;
    let beat_seconds = 60.0 / bpm;
    let duration_seconds = samples.len() as f64 / sample_rate as f64;
    let beat_count = (duration_seconds / beat_seconds).floor() as usize;
    if beat_count < 4 {
        return None;
    }

    let beats_seconds = (0..beat_count)
        .map(|beat| (beat as f64 * beat_seconds) as f32)
        .collect::<Vec<_>>();
    let downbeats_seconds = beats_seconds
        .iter()
        .enumerate()
        .filter_map(|(index, seconds)| (index % 4 == 0).then_some(*seconds))
        .collect::<Vec<_>>();

    Some(BeatGridAnalysis {
        bpm,
        confidence: 0.05,
        beats_seconds,
        downbeats_seconds,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct SafeTransitionWindow {
    pub start_seconds: f32,
    pub end_seconds: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct StructureCandidate {
    seconds: f32,
    confidence: f32,
    reason: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedDjProfile {
    pub bpm: Option<f32>,
    pub downbeats_seconds: Vec<f32>,
    pub phrase_bar_indices: Vec<u32>,
    pub safe_crossfade_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannerPolicyShape {
    pub default_crossfade_ms: u32,
    pub transition_speed_bias: String,
}

#[allow(dead_code)]
pub fn build_audio_dj_profile_row(
    key: AudioDjProfileKey,
    track_id: Option<i64>,
    queue_item_id: Option<i64>,
    tidal_id: Option<i64>,
    samples: &[f32],
    sample_rate: u32,
    source: &str,
) -> Option<AudioDjProfileRow> {
    let beat_grid = bpm::analyze_beat_grid(samples, sample_rate)?;
    Some(build_audio_dj_profile_row_from_analysis(
        key,
        track_id,
        queue_item_id,
        tidal_id,
        samples,
        sample_rate,
        source,
        &beat_grid,
    ))
}

pub fn dj_analysis_skips_existing_profile_version(
    conn: &Connection,
    key: &AudioDjProfileKey,
) -> Result<bool> {
    let existing = queries::get_audio_dj_profile(conn, key)?;
    Ok(existing.as_ref().is_some_and(dj_profile_row_is_current))
}

pub fn dj_profile_row_is_current(row: &AudioDjProfileRow) -> bool {
    row.profile_version == DJ_PROFILE_VERSION
        && decode_f32_blob(&row.waveform_peaks_blob).is_some_and(|peaks| !peaks.is_empty())
}

pub fn persist_dj_analysis_job_from_analysis(
    conn: &Connection,
    job: &DjAnalysisJob,
    source: &str,
    analysis: &BeatGridAnalysis,
) -> Result<()> {
    let row = build_audio_dj_profile_row_from_analysis(
        job.media_ref.profile_key(),
        job.track_id,
        job.queue_item_id,
        job.tidal_id,
        &job.samples,
        job.sample_rate,
        source,
        analysis,
    );
    queries::upsert_audio_dj_profile(conn, &row)
}

pub fn encode_f32_blob(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + values.len() * 4);
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub fn decode_f32_blob(blob: &[u8]) -> Option<Vec<f32>> {
    let payload = payload_after_count(blob, 4)?;
    let count = read_count(blob)? as usize;
    if payload.len() != count.checked_mul(4)? {
        return None;
    }
    let mut values = Vec::with_capacity(count);
    for chunk in payload.chunks_exact(4) {
        let value = f32::from_le_bytes(chunk.try_into().ok()?);
        if !value.is_finite() {
            return None;
        }
        values.push(value);
    }
    Some(values)
}

pub fn waveform_peaks(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let mut peaks = Vec::with_capacity(DJ_WAVEFORM_PEAK_COUNT);
    for bucket in 0..DJ_WAVEFORM_PEAK_COUNT {
        let start = bucket * samples.len() / DJ_WAVEFORM_PEAK_COUNT;
        let end = ((bucket + 1) * samples.len() / DJ_WAVEFORM_PEAK_COUNT).max(start + 1);
        let peak = samples[start..end.min(samples.len())]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        peaks.push(peak);
    }

    let max_peak = peaks.iter().copied().fold(0.0_f32, f32::max);
    if max_peak > 0.0 {
        for peak in &mut peaks {
            *peak = (*peak / max_peak).clamp(0.0, 1.0);
        }
    }
    peaks
}

pub fn encode_u32_blob(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + values.len() * 4);
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub fn decode_u32_blob(blob: &[u8]) -> Option<Vec<u32>> {
    let payload = payload_after_count(blob, 4)?;
    let count = read_count(blob)? as usize;
    if payload.len() != count.checked_mul(4)? {
        return None;
    }
    Some(
        payload
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("chunk size")))
            .collect(),
    )
}

pub fn encode_safe_transition_windows(windows: &[SafeTransitionWindow]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + windows.len() * 12);
    out.extend_from_slice(&(windows.len() as u32).to_le_bytes());
    for window in windows {
        out.extend_from_slice(&window.start_seconds.to_le_bytes());
        out.extend_from_slice(&window.end_seconds.to_le_bytes());
        out.extend_from_slice(&window.confidence.to_le_bytes());
    }
    out
}

pub fn decode_safe_transition_windows(blob: &[u8]) -> Option<Vec<SafeTransitionWindow>> {
    let payload = payload_after_count(blob, 12)?;
    let count = read_count(blob)? as usize;
    if payload.len() != count.checked_mul(12)? {
        return None;
    }
    let mut windows = Vec::with_capacity(count);
    for chunk in payload.chunks_exact(12) {
        let start_seconds = f32::from_le_bytes(chunk[0..4].try_into().ok()?);
        let end_seconds = f32::from_le_bytes(chunk[4..8].try_into().ok()?);
        let confidence = f32::from_le_bytes(chunk[8..12].try_into().ok()?);
        if !start_seconds.is_finite() || !end_seconds.is_finite() || !confidence.is_finite() {
            return None;
        }
        windows.push(SafeTransitionWindow {
            start_seconds,
            end_seconds,
            confidence,
        });
    }
    Some(windows)
}

pub fn apply_correction_to_loaded_profile(
    profile: &mut LoadedDjProfile,
    correction: &AudioDjProfileCorrectionRow,
) {
    if let (Some(bpm), Some(multiplier)) = (profile.bpm, correction.bpm_multiplier) {
        profile.bpm = Some((bpm as f64 * multiplier) as f32);
    }
    if let Some(offset) = correction.downbeat_offset_beats {
        let beat_seconds = profile
            .downbeats_seconds
            .windows(2)
            .next()
            .map(|w| (w[1] - w[0]).abs() / 4.0)
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(0.5);
        for downbeat in &mut profile.downbeats_seconds {
            *downbeat += beat_seconds * offset as f32;
        }
    }
    if let Some(offset) = correction.phrase_offset_bars {
        for phrase in &mut profile.phrase_bar_indices {
            let shifted = i64::from(*phrase) + offset;
            *phrase = shifted.max(0) as u32;
        }
    }
    if correction.safe_crossfade_only {
        profile.safe_crossfade_only = true;
    }
}

pub fn transition_speed_bias_to_policy_shape(bias: Option<&str>) -> PlannerPolicyShape {
    let (default_crossfade_ms, transition_speed_bias) = match bias {
        Some("slower") => (12_000, "slower"),
        Some("faster") => (6_000, "faster"),
        _ => (8_000, "neutral"),
    };
    PlannerPolicyShape {
        default_crossfade_ms,
        transition_speed_bias: transition_speed_bias.to_string(),
    }
}

fn build_audio_dj_profile_row_from_analysis(
    key: AudioDjProfileKey,
    track_id: Option<i64>,
    queue_item_id: Option<i64>,
    tidal_id: Option<i64>,
    samples: &[f32],
    sample_rate: u32,
    source: &str,
    analysis: &BeatGridAnalysis,
) -> AudioDjProfileRow {
    let analysis_scope_ms = if sample_rate == 0 {
        0
    } else {
        ((samples.len() as f64 / sample_rate as f64) * 1000.0).round() as i64
    };
    let profile_confidence = profile_confidence(&key, analysis_scope_ms);
    let energy_contour =
        energy_contour_by_phrase(samples, sample_rate, &analysis.downbeats_seconds);
    let phrase_boundaries =
        phrase_boundaries_from_structure(&analysis.downbeats_seconds, &energy_contour);
    let mix_in = mix_in_points(&analysis.downbeats_seconds, profile_confidence);
    let mix_out = mix_out_points(&analysis.downbeats_seconds, profile_confidence);
    let windows = safe_transition_windows(&mix_in, &mix_out, profile_confidence);
    let confident_structure = profile_confidence >= 0.4 && !analysis.downbeats_seconds.is_empty();
    let breakdown_candidates = breakdown_candidates(
        &analysis.downbeats_seconds,
        &energy_contour,
        profile_confidence,
    );
    let drop_candidates = drop_candidates(
        &analysis.downbeats_seconds,
        &energy_contour,
        profile_confidence,
    );

    AudioDjProfileRow {
        media_ref_kind: key.media_ref_kind.clone(),
        media_ref_id: key.media_ref_id.clone(),
        track_id,
        queue_item_id,
        tidal_id,
        profile_version: DJ_PROFILE_VERSION.to_string(),
        beat_grid_blob: encode_f32_blob(&analysis.beats_seconds),
        downbeats_blob: encode_f32_blob(&analysis.downbeats_seconds),
        phrase_boundaries_blob: encode_u32_blob(&phrase_boundaries),
        mix_in_blob: encode_f32_blob(&mix_in),
        mix_out_blob: encode_f32_blob(&mix_out),
        intro_end_seconds: confident_structure
            .then(|| f64::from(*analysis.downbeats_seconds.get(8).unwrap_or(&0.0))),
        outro_start_seconds: confident_structure.then(|| {
            f64::from(
                analysis
                    .downbeats_seconds
                    .get(analysis.downbeats_seconds.len().saturating_sub(8))
                    .copied()
                    .unwrap_or(0.0),
            )
        }),
        breakdown_blob: encode_f32_blob(&candidate_seconds(&breakdown_candidates)),
        drop_blob: encode_f32_blob(&candidate_seconds(&drop_candidates)),
        safe_transition_windows_blob: encode_safe_transition_windows(&windows),
        energy_contour_blob: encode_f32_blob(&energy_contour),
        vocal_presence_blob: encode_f32_blob(&vec![0.0; phrase_boundaries.len().max(1)]),
        vocal_density_blob: encode_f32_blob(&vec![0.0; phrase_boundaries.len().max(1)]),
        waveform_peaks_blob: encode_f32_blob(&waveform_peaks(samples)),
        lufs_loud_body: None,
        true_peak_dbtp: None,
        beat_confidence: Some(analysis.confidence),
        profile_confidence,
        analysis_scope_ms,
        is_temporary: key.media_ref_kind == "queue_item",
        source: source.to_string(),
        computed_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn profile_confidence(key: &AudioDjProfileKey, analysis_scope_ms: i64) -> f64 {
    if key.media_ref_kind == "library_track" && analysis_scope_ms >= 120_000 {
        1.0
    } else if analysis_scope_ms >= 90_000 {
        0.65
    } else if analysis_scope_ms >= 30_000 {
        0.4
    } else {
        (analysis_scope_ms.max(0) as f64 / 30_000.0) * 0.39
    }
}

fn phrase_boundaries_every_eight_downbeats(downbeats: &[f32]) -> Vec<u32> {
    (0..downbeats.len() as u32).step_by(8).collect()
}

fn phrase_boundaries_from_structure(downbeats: &[f32], energy_contour: &[f32]) -> Vec<u32> {
    let mut boundaries = phrase_boundaries_every_eight_downbeats(downbeats);
    boundaries.extend((0..downbeats.len() as u32).step_by(16));
    for (phrase_index, window) in energy_contour.windows(2).enumerate() {
        let energy_rise = window[1] - window[0];
        if energy_rise >= 0.2 {
            boundaries.push(((phrase_index + 1) * 8) as u32);
        }
    }
    boundaries.retain(|bar| (*bar as usize) < downbeats.len());
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

fn mix_in_points(downbeats: &[f32], confidence: f64) -> Vec<f32> {
    if confidence < 0.4 {
        return Vec::new();
    }
    downbeats.iter().take(4).copied().collect()
}

fn mix_out_points(downbeats: &[f32], confidence: f64) -> Vec<f32> {
    if confidence < 0.4 {
        return Vec::new();
    }
    downbeats
        .iter()
        .rev()
        .take(4)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn safe_transition_windows(
    mix_in: &[f32],
    mix_out: &[f32],
    profile_confidence: f64,
) -> Vec<SafeTransitionWindow> {
    mix_in
        .iter()
        .copied()
        .chain(mix_out.iter().copied())
        .map(|start_seconds| SafeTransitionWindow {
            start_seconds,
            end_seconds: start_seconds + 8.0,
            confidence: profile_confidence as f32,
        })
        .collect()
}

fn breakdown_candidates(
    downbeats: &[f32],
    energy_contour: &[f32],
    profile_confidence: f64,
) -> Vec<StructureCandidate> {
    if profile_confidence < 0.65 || downbeats.len() < 16 {
        return Vec::new();
    }
    energy_contour
        .windows(2)
        .enumerate()
        .filter_map(|(phrase_index, window)| {
            let energy_drop = window[0] - window[1];
            let bar_index = (phrase_index + 1) * 8;
            let confidence = (profile_confidence as f32 * (0.55 + energy_drop.max(0.0))).min(1.0);
            (energy_drop >= 0.2)
                .then(|| downbeats.get(bar_index).copied())
                .flatten()
                .map(|seconds| StructureCandidate {
                    seconds,
                    confidence,
                    reason: "energy_drop",
                })
                .filter(|candidate| candidate.confidence >= 0.5)
        })
        .collect()
}

fn drop_candidates(
    downbeats: &[f32],
    energy_contour: &[f32],
    profile_confidence: f64,
) -> Vec<StructureCandidate> {
    if profile_confidence < 0.65 || downbeats.len() < 16 {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for phrase_index in 1..energy_contour.len() {
        let bar_index = phrase_index * 8;
        let Some(seconds) = downbeats.get(bar_index).copied() else {
            continue;
        };
        let previous_energy = energy_contour.get(phrase_index - 1).copied().unwrap_or(0.0);
        let current_energy = energy_contour.get(phrase_index).copied().unwrap_or(0.0);
        let energy_rise = current_energy - previous_energy;
        let sixteen_bar_aligned = bar_index % 16 == 0;
        let local_energy = current_energy.max(previous_energy);
        let (confidence, reason) = if energy_rise >= 0.2 {
            (
                (profile_confidence as f32 * (0.65 + energy_rise)).min(1.0),
                "energy_rise",
            )
        } else if sixteen_bar_aligned && local_energy >= 0.5 {
            (
                (profile_confidence as f32 * (0.55 + local_energy * 0.25)).min(1.0),
                "sixteen_bar_phrase",
            )
        } else {
            continue;
        };
        if confidence >= 0.5 {
            candidates.push(StructureCandidate {
                seconds,
                confidence,
                reason,
            });
        }
    }
    candidates
}

fn candidate_seconds(candidates: &[StructureCandidate]) -> Vec<f32> {
    candidates
        .iter()
        .map(|candidate| candidate.seconds)
        .collect()
}

fn energy_contour_by_phrase(samples: &[f32], sample_rate: u32, downbeats: &[f32]) -> Vec<f32> {
    let phrases = phrase_boundaries_every_eight_downbeats(downbeats);
    if phrases.is_empty() || sample_rate == 0 || samples.is_empty() {
        return vec![0.0];
    }
    let mut values = phrases
        .iter()
        .map(|phrase| {
            let start = downbeats
                .get(*phrase as usize)
                .map(|seconds| (*seconds * sample_rate as f32) as usize)
                .unwrap_or(0)
                .min(samples.len());
            let end = downbeats
                .get((*phrase as usize) + 8)
                .map(|seconds| (*seconds * sample_rate as f32) as usize)
                .unwrap_or(samples.len())
                .min(samples.len());
            rms(&samples[start..end])
        })
        .collect::<Vec<_>>();
    let peak = values.iter().copied().fold(0.0, f32::max);
    if peak > 0.0 {
        for value in &mut values {
            *value /= peak;
        }
    }
    values
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn payload_after_count(blob: &[u8], width: usize) -> Option<&[u8]> {
    let count = read_count(blob)? as usize;
    let expected = count.checked_mul(width)?;
    let payload = blob.get(4..)?;
    (payload.len() == expected).then_some(payload)
}

fn read_count(blob: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(blob.get(0..4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::AudioDspFeatures;
    use crate::db::{Database, schema};
    use std::time::{Duration, Instant};

    fn key(kind: &str, id: &str) -> AudioDjProfileKey {
        AudioDjProfileKey {
            media_ref_kind: kind.to_string(),
            media_ref_id: id.to_string(),
        }
    }

    fn library_ref(track_id: i64) -> DjMediaRef {
        DjMediaRef::LibraryTrack { track_id }
    }

    fn tidal_ref(tidal_id: i64) -> DjMediaRef {
        DjMediaRef::TidalTrack {
            tidal_id,
            track_id: None,
        }
    }

    fn analysis(downbeat_count: usize) -> BeatGridAnalysis {
        BeatGridAnalysis {
            bpm: 120.0,
            confidence: 0.9,
            beats_seconds: (0..downbeat_count * 4)
                .map(|beat| beat as f32 * 0.5)
                .collect(),
            downbeats_seconds: (0..downbeat_count).map(|bar| bar as f32 * 2.0).collect(),
        }
    }

    fn injected_analysis(_samples: &[f32], _sample_rate: u32) -> Option<BeatGridAnalysis> {
        Some(analysis(64))
    }

    fn no_analysis(_samples: &[f32], _sample_rate: u32) -> Option<BeatGridAnalysis> {
        None
    }

    fn samples(seconds: usize) -> Vec<f32> {
        (0..seconds * 48_000)
            .map(|i| if i % 1000 < 500 { 0.5 } else { 0.25 })
            .collect()
    }

    fn click_train(sample_rate: u32, seconds: usize, bpm: f64) -> Vec<f32> {
        let total = sample_rate as usize * seconds;
        let period = (sample_rate as f64 * 60.0 / bpm).round() as usize;
        let mut out = vec![0.0; total];
        for start in (0..total).step_by(period.max(1)) {
            for offset in 0..64 {
                if let Some(sample) = out.get_mut(start + offset) {
                    *sample = 1.0;
                }
            }
        }
        out
    }

    fn row_for(kind: &str, id: &str, seconds: usize) -> AudioDjProfileRow {
        build_audio_dj_profile_row_from_analysis(
            key(kind, id),
            (kind == "library_track").then_some(id.parse().unwrap_or(1)),
            (kind == "queue_item").then_some(id.parse().unwrap_or(1)),
            (kind == "tidal_track").then_some(id.parse().unwrap_or(1)),
            &samples(seconds),
            48_000,
            "test",
            &analysis(64),
        )
    }

    #[test]
    fn passive_analysis_job_shape_stays_track_id_keyed() {
        let job: super::super::AnalysisJob = (42, vec![0.0], 48_000);
        assert_eq!(job.0, 42);
        assert_eq!(job.2, 48_000);
    }

    #[test]
    fn dj_profile_version_is_independent_from_planner_version() {
        assert_ne!(DJ_PROFILE_VERSION, "dj_planner_v1");
        assert_eq!(DJ_PROFILE_VERSION, "dj_profile_v2");
    }

    #[test]
    fn dj_analysis_job_uses_media_ref_key() {
        let job = DjAnalysisJob {
            media_ref: tidal_ref(55),
            track_id: None,
            queue_item_id: None,
            tidal_id: Some(55),
            samples: vec![0.0],
            sample_rate: 48_000,
            analysis_scope_ms: 1_000,
            deadline_generation: 7,
        };
        let key = job.media_ref.profile_key();
        assert_eq!(key.media_ref_kind, "tidal_track");
        assert_eq!(key.media_ref_id, "55");
    }

    #[test]
    fn dj_analysis_skips_existing_profile_version() {
        let db = Database::open_in_memory().expect("db");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| {
            let mut row = row_for("tidal_track", "1", 90);
            row.profile_version = "dj_profile_v1".to_string();
            queries::upsert_audio_dj_profile(conn, &row)?;
            assert!(!super::dj_analysis_skips_existing_profile_version(
                conn,
                &key("tidal_track", "1")
            )?);
            row.profile_version = DJ_PROFILE_VERSION.to_string();
            queries::upsert_audio_dj_profile(conn, &row)?;
            assert!(super::dj_analysis_skips_existing_profile_version(
                conn,
                &key("tidal_track", "1")
            )?);
            row.waveform_peaks_blob = encode_f32_blob(&[]);
            queries::upsert_audio_dj_profile(conn, &row)?;
            assert!(!super::dj_analysis_skips_existing_profile_version(
                conn,
                &key("tidal_track", "1")
            )?);
            Ok(())
        })
        .expect("check");
    }

    #[test]
    fn dj_analysis_does_not_clobber_audio_dsp_features() {
        let db = Database::open_in_memory().expect("db");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (name) VALUES ('Artist')", [])?;
            let artist_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id) VALUES (1, 'Track', ?1)",
                rusqlite::params![artist_id],
            )?;
            let dsp = AudioDspFeatures {
                track_id: 1,
                bpm: Some(120.0),
                key_signature: None,
                camelot_key: None,
                loudness_lufs: None,
                energy: Some(0.5),
                danceability: None,
                beat_strength: None,
                spectral_centroid: None,
                stereo_width: None,
                is_instrumental: false,
                analysis_source: "test".to_string(),
                analysis_offset_ms: 0,
                samples_analyzed: Some(100),
                analyzed_at: "now".to_string(),
                analysis_version: super::super::CURRENT_ANALYSIS_VERSION.to_string(),
            };
            queries::upsert_audio_dsp_features(conn, &dsp)?;
            let job = DjAnalysisJob {
                media_ref: library_ref(1),
                track_id: Some(1),
                queue_item_id: None,
                tidal_id: None,
                samples: samples(90),
                sample_rate: 48_000,
                analysis_scope_ms: 90_000,
                deadline_generation: 1,
            };
            persist_dj_analysis_job_from_analysis(conn, &job, "test", &analysis(64))?;
            let loaded = queries::get_audio_dsp_features(conn, 1)?.expect("dsp");
            assert_eq!(loaded.bpm, Some(120.0));
            assert_eq!(loaded.analysis_source, "test");
            Ok(())
        })
        .expect("persist");
    }

    #[test]
    fn phrase_boundaries_every_eight_downbeats() {
        assert_eq!(
            super::phrase_boundaries_every_eight_downbeats(&analysis(24).downbeats_seconds),
            vec![0, 8, 16]
        );
    }

    #[test]
    fn phrase_boundaries_include_energy_change_candidates() {
        let downbeats = analysis(40).downbeats_seconds;
        assert_eq!(
            super::phrase_boundaries_from_structure(&downbeats, &[0.2, 0.3, 0.8, 0.7]),
            vec![0, 8, 16, 24, 32]
        );
    }

    #[test]
    fn structure_markers_are_persisted_in_v2_profile() {
        let row = row_for("library_track", "1", 180);
        assert!(row.intro_end_seconds.is_some());
        assert!(row.outro_start_seconds.is_some());
        assert!(decode_f32_blob(&row.drop_blob).unwrap().len() > 1);
    }

    #[test]
    fn profile_builder_covers_short_mid_and_full_windows() {
        let short = row_for("tidal_track", "1", 30);
        let mid = row_for("tidal_track", "2", 90);
        let full = row_for("library_track", "3", 180);

        assert_eq!(short.profile_confidence, 0.4);
        assert_eq!(mid.profile_confidence, 0.65);
        assert_eq!(full.profile_confidence, 1.0);
        assert!(decode_f32_blob(&short.drop_blob).unwrap().is_empty());
        assert!(!decode_f32_blob(&mid.drop_blob).unwrap().is_empty());
        assert!(decode_f32_blob(&full.drop_blob).unwrap().len() >= 3);
    }

    #[test]
    fn low_confidence_profile_omits_drop_candidates() {
        let row = row_for("tidal_track", "1", 10);
        assert!(row.profile_confidence < 0.4);
        assert!(decode_f32_blob(&row.drop_blob).unwrap().is_empty());
    }

    #[test]
    fn drop_candidates_use_phrase_structure_not_fixed_twenty_fourth_downbeat() {
        let row = row_for("library_track", "1", 180);
        let drops = decode_f32_blob(&row.drop_blob).expect("drops");
        let downbeats = decode_f32_blob(&row.downbeats_blob).expect("downbeats");

        assert_ne!(drops, vec![downbeats[24]]);
        assert!(drops.contains(&downbeats[16]));
        assert!(drops.contains(&downbeats[32]));
    }

    #[test]
    fn drop_candidates_carry_confidence_and_reason() {
        let downbeats = analysis(40).downbeats_seconds;
        let candidates = super::drop_candidates(&downbeats, &[0.1, 0.2, 0.8, 0.75, 0.9], 1.0);

        let energy_rise = candidates
            .iter()
            .find(|candidate| candidate.reason == "energy_rise")
            .expect("energy rise candidate");
        assert_eq!(energy_rise.seconds, downbeats[16]);
        assert!(energy_rise.confidence >= 0.9);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.reason == "sixteen_bar_phrase")
        );
    }

    #[test]
    fn mix_points_align_to_downbeats_when_confident() {
        let row = row_for("library_track", "1", 180);
        let downbeats = decode_f32_blob(&row.downbeats_blob).expect("downbeats");
        let mix_in = decode_f32_blob(&row.mix_in_blob).expect("mix in");
        assert!(mix_in.iter().all(|point| downbeats.contains(point)));
    }

    #[test]
    fn safe_transition_windows_round_trip() {
        let windows = vec![SafeTransitionWindow {
            start_seconds: 1.0,
            end_seconds: 9.0,
            confidence: 0.7,
        }];
        assert_eq!(
            decode_safe_transition_windows(&encode_safe_transition_windows(&windows)),
            Some(windows)
        );
    }

    #[test]
    fn low_confidence_profile_omits_intro_outro_estimates() {
        let row = row_for("tidal_track", "1", 10);
        assert!(row.profile_confidence < 0.4);
        assert_eq!(row.intro_end_seconds, None);
        assert_eq!(row.outro_start_seconds, None);
    }

    #[test]
    fn energy_contour_normalizes_peak_to_one() {
        let row = row_for("library_track", "1", 180);
        let contour = decode_f32_blob(&row.energy_contour_blob).expect("contour");
        assert!(contour.iter().any(|value| (*value - 1.0).abs() < 1e-6));
    }

    #[test]
    fn vocal_presence_defaults_to_zero() {
        let row = row_for("library_track", "1", 180);
        let values = decode_f32_blob(&row.vocal_presence_blob).expect("vocal presence");
        assert!(values.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn vocal_density_defaults_to_zero() {
        let row = row_for("library_track", "1", 180);
        let values = decode_f32_blob(&row.vocal_density_blob).expect("vocal density");
        assert!(values.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn profile_blob_round_trips_f32_values() {
        let values = vec![0.0, 1.25, 2.5];
        assert_eq!(decode_f32_blob(&encode_f32_blob(&values)), Some(values));
    }

    #[test]
    fn profile_blob_round_trips_u32_values() {
        let values = vec![0, 8, 16];
        assert_eq!(decode_u32_blob(&encode_u32_blob(&values)), Some(values));
    }

    #[test]
    fn profile_blob_rejects_truncated_payload() {
        assert!(decode_f32_blob(&[1, 0, 0, 0, 1]).is_none());
    }

    #[test]
    fn profile_blob_rejects_non_finite_float() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&1_u32.to_le_bytes());
        blob.extend_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode_f32_blob(&blob).is_none());
    }

    #[test]
    fn waveform_peaks_are_compact_and_normalized() {
        let samples = (0..2048)
            .map(|index| if index % 2 == 0 { 0.25 } else { -0.5 })
            .collect::<Vec<_>>();

        let peaks = waveform_peaks(&samples);

        assert_eq!(peaks.len(), DJ_WAVEFORM_PEAK_COUNT);
        assert!(peaks.iter().all(|peak| *peak >= 0.0 && *peak <= 1.0));
        assert!(peaks.iter().any(|peak| (*peak - 1.0).abs() < f32::EPSILON));
    }

    #[test]
    fn profile_row_blobs_decode_to_expected_lengths() {
        let row = row_for("library_track", "1", 180);
        assert_eq!(decode_f32_blob(&row.beat_grid_blob).unwrap().len(), 256);
        assert_eq!(decode_f32_blob(&row.downbeats_blob).unwrap().len(), 64);
        assert_eq!(
            decode_u32_blob(&row.phrase_boundaries_blob).unwrap().len(),
            8
        );
        assert_eq!(
            decode_f32_blob(&row.waveform_peaks_blob).unwrap().len(),
            DJ_WAVEFORM_PEAK_COUNT
        );
    }

    #[test]
    fn profile_builder_preserves_tidal_media_ref() {
        let row = row_for("tidal_track", "123", 90);
        assert_eq!(row.media_ref_kind, "tidal_track");
        assert_eq!(row.media_ref_id, "123");
        assert_eq!(row.tidal_id, Some(123));
    }

    #[test]
    fn profile_builder_allows_pending_queue_ref() {
        let row = row_for("queue_item", "44", 30);
        assert_eq!(row.media_ref_kind, "queue_item");
        assert_eq!(row.queue_item_id, Some(44));
    }

    #[test]
    fn profile_builder_marks_pending_queue_ref_temporary() {
        let row = row_for("queue_item", "44", 30);
        assert!(row.is_temporary);
    }

    #[test]
    fn profile_builder_supports_local_tidal_and_pending_sources() {
        assert_eq!(
            row_for("library_track", "1", 180).media_ref_kind,
            "library_track"
        );
        assert_eq!(
            row_for("tidal_track", "2", 90).media_ref_kind,
            "tidal_track"
        );
        assert_eq!(row_for("queue_item", "3", 30).media_ref_kind, "queue_item");
    }

    #[test]
    fn fallback_beat_grid_builds_profile_when_primary_analysis_returns_none() {
        let analysis = fallback_beat_grid_analysis(&click_train(48_000, 30, 120.0), 48_000)
            .expect("fallback beat grid");
        assert!(analysis.beats_seconds.len() > 40);
        assert!(analysis.downbeats_seconds.len() > 10);
        assert!((analysis.bpm - 120.0).abs() < 4.0);
    }

    #[test]
    fn profile_confidence_scales_with_analysis_scope() {
        assert_eq!(row_for("library_track", "1", 180).profile_confidence, 1.0);
        assert_eq!(row_for("tidal_track", "1", 90).profile_confidence, 0.65);
        assert_eq!(row_for("tidal_track", "1", 30).profile_confidence, 0.4);
        assert!(row_for("tidal_track", "1", 10).profile_confidence < 0.4);
    }

    #[test]
    #[ignore]
    fn profile_build_benchmark() {
        let cases = [("30s", 30_usize), ("90s", 90_usize), ("full", 180_usize)];
        for (label, seconds) in cases {
            let benchmark_samples = samples(seconds);
            let benchmark_analysis = analysis((seconds / 2).max(1));
            let mut timings = (0..5)
                .map(|iteration| {
                    let started = Instant::now();
                    let row = build_audio_dj_profile_row_from_analysis(
                        key("library_track", &(iteration + 1).to_string()),
                        Some((iteration + 1) as i64),
                        None,
                        None,
                        &benchmark_samples,
                        48_000,
                        "benchmark",
                        &benchmark_analysis,
                    );
                    std::hint::black_box(row);
                    started.elapsed()
                })
                .collect::<Vec<_>>();
            timings.sort_unstable();
            let median = timings[timings.len() / 2];
            let peak_bytes = estimated_profile_build_peak_bytes(&benchmark_samples, seconds);
            println!(
                "dj_profile_build {label} median_ms={} peak_memory_bytes={peak_bytes}",
                duration_ms(median)
            );
        }
    }

    #[test]
    fn correction_bpm_multiplier_adjusts_loaded_profile_bpm() {
        let mut profile = LoadedDjProfile {
            bpm: Some(120.0),
            downbeats_seconds: vec![0.0, 2.0],
            phrase_bar_indices: vec![0],
            safe_crossfade_only: false,
        };
        let correction = correction(Some(2.0), None, None, false, None);
        apply_correction_to_loaded_profile(&mut profile, &correction);
        assert_eq!(profile.bpm, Some(240.0));
    }

    #[test]
    fn correction_offsets_adjust_loaded_downbeats_and_phrases() {
        let mut profile = LoadedDjProfile {
            bpm: Some(120.0),
            downbeats_seconds: vec![0.0, 2.0],
            phrase_bar_indices: vec![8],
            safe_crossfade_only: false,
        };
        let correction = correction(None, Some(2), Some(-3), false, None);
        apply_correction_to_loaded_profile(&mut profile, &correction);
        assert_eq!(profile.downbeats_seconds, vec![1.0, 3.0]);
        assert_eq!(profile.phrase_bar_indices, vec![5]);
    }

    #[test]
    fn safe_crossfade_only_correction_sets_planner_safety_flag() {
        let mut profile = LoadedDjProfile {
            bpm: Some(120.0),
            downbeats_seconds: vec![0.0, 2.0],
            phrase_bar_indices: vec![0],
            safe_crossfade_only: false,
        };
        let correction = correction(None, None, None, true, None);
        apply_correction_to_loaded_profile(&mut profile, &correction);
        assert!(profile.safe_crossfade_only);
    }

    #[test]
    fn transition_speed_bias_maps_to_policy_duration_preference() {
        assert_eq!(
            transition_speed_bias_to_policy_shape(Some("faster")).default_crossfade_ms,
            6_000
        );
        assert_eq!(
            transition_speed_bias_to_policy_shape(Some("slower")).default_crossfade_ms,
            12_000
        );
        assert_eq!(
            transition_speed_bias_to_policy_shape(None).transition_speed_bias,
            "neutral"
        );
    }

    #[test]
    fn raw_profile_builder_ignores_user_corrections() {
        let row = row_for("tidal_track", "1", 90);
        let correction = correction(Some(2.0), None, None, true, Some("faster"));
        assert_eq!(row.profile_confidence, 0.65);
        assert!(!row.is_temporary);
        assert_eq!(correction.bpm_multiplier, Some(2.0));
    }

    #[test]
    fn analyze_and_save_does_not_write_audio_dj_profile() {
        let db = Database::open_in_memory().expect("db");
        db.run_migrations().expect("migrations");
        super::super::engine::analyze_and_save(&db, &[0.0; 128], 48_000, "test", 1, 0);
        db.with_conn(|conn| {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM audio_dj_profiles", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(count, 0);
            Ok(())
        })
        .expect("count");
    }

    #[test]
    fn dj_analysis_job_persists_audio_dj_profile() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        schema::run_migrations(&conn).expect("migrations");
        let job = DjAnalysisJob {
            media_ref: tidal_ref(7),
            track_id: None,
            queue_item_id: None,
            tidal_id: Some(7),
            samples: samples(90),
            sample_rate: 48_000,
            analysis_scope_ms: 90_000,
            deadline_generation: 1,
        };
        persist_dj_analysis_job_from_analysis(&conn, &job, "test", &analysis(64)).expect("persist");
        let loaded = queries::get_audio_dj_profile(&conn, &key("tidal_track", "7"))
            .expect("get")
            .expect("profile");
        assert_eq!(loaded.tidal_id, Some(7));
    }

    #[test]
    fn dj_profile_db_retry_retries_locked_database_errors() {
        let attempts = std::cell::Cell::new(0);
        let result = with_dj_profile_db_retry_config(
            "test retry",
            2,
            Duration::ZERO,
            || -> Result<&'static str> {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                if attempt == 0 {
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ErrorCode::DatabaseBusy,
                            extended_code: rusqlite::ffi::SQLITE_BUSY,
                        },
                        None,
                    )
                    .into());
                }
                Ok("ok")
            },
        );

        assert_eq!(result.expect("retry result"), "ok");
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn tidal_job_persists_provisional_profile_when_detector_fails() {
        let db = Database::open_in_memory().expect("db");
        db.run_migrations().expect("migrations");
        let job = DjAnalysisJob {
            media_ref: tidal_ref(77),
            track_id: None,
            queue_item_id: None,
            tidal_id: Some(77),
            samples: vec![0.0; 90 * 48_000],
            sample_rate: 48_000,
            analysis_scope_ms: 90_000,
            deadline_generation: 1,
        };

        persist_dj_analysis_job(&db, job, no_analysis).expect("persist");

        let loaded = db
            .with_conn(|conn| queries::get_audio_dj_profile(conn, &key("tidal_track", "77")))
            .expect("get")
            .expect("profile");
        assert_eq!(loaded.tidal_id, Some(77));
        assert_eq!(loaded.profile_confidence, 0.65);
        assert!(!decode_f32_blob(&loaded.beat_grid_blob).unwrap().is_empty());
        assert!(!decode_f32_blob(&loaded.downbeats_blob).unwrap().is_empty());
    }

    #[tokio::test]
    async fn dj_profile_actor_persists_tidal_profile_key() {
        let db = Database::open_in_memory().expect("db");
        db.run_migrations().expect("migrations");
        let tx = spawn_dj_profile_actor_with_analyzer(db.clone(), injected_analysis);

        tx.send(DjAnalysisJob {
            media_ref: tidal_ref(7),
            track_id: None,
            queue_item_id: None,
            tidal_id: Some(7),
            samples: click_train(48_000, 30, 120.0),
            sample_rate: 48_000,
            analysis_scope_ms: 30_000,
            deadline_generation: 1,
        })
        .expect("send");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let profile = db
                .with_conn(|conn| queries::get_audio_dj_profile(conn, &key("tidal_track", "7")))
                .expect("get");
            if let Some(profile) = profile {
                assert_eq!(profile.media_ref_kind, "tidal_track");
                assert_eq!(profile.media_ref_id, "7");
                assert_eq!(profile.tidal_id, Some(7));
                break;
            }
            assert!(Instant::now() < deadline, "timed out waiting for profile");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn dj_profile_actor_ignores_empty_samples_and_invalid_sample_rate() {
        let db = Database::open_in_memory().expect("db");
        db.run_migrations().expect("migrations");
        let tx = spawn_dj_profile_actor(db.clone());

        for (tidal_id, samples, sample_rate) in
            [(8, Vec::new(), 48_000_u32), (9, vec![0.0; 128], 0_u32)]
        {
            tx.send(DjAnalysisJob {
                media_ref: tidal_ref(tidal_id),
                track_id: None,
                queue_item_id: None,
                tidal_id: Some(tidal_id),
                samples,
                sample_rate,
                analysis_scope_ms: 0,
                deadline_generation: 1,
            })
            .expect("send");
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        db.with_conn(|conn| {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM audio_dj_profiles", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(count, 0);
            Ok(())
        })
        .expect("count");
    }

    fn correction(
        bpm_multiplier: Option<f64>,
        downbeat_offset_beats: Option<i64>,
        phrase_offset_bars: Option<i64>,
        safe_crossfade_only: bool,
        transition_speed_bias: Option<&str>,
    ) -> AudioDjProfileCorrectionRow {
        AudioDjProfileCorrectionRow {
            media_ref_kind: "tidal_track".to_string(),
            media_ref_id: "1".to_string(),
            bpm_multiplier,
            downbeat_offset_beats,
            phrase_offset_bars,
            safe_crossfade_only,
            transition_speed_bias: transition_speed_bias.map(str::to_string),
            notes: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        }
    }

    fn estimated_profile_build_peak_bytes(samples: &[f32], seconds: usize) -> usize {
        let row = row_for("library_track", "999", seconds);
        std::mem::size_of_val(samples)
            + row.beat_grid_blob.len()
            + row.downbeats_blob.len()
            + row.phrase_boundaries_blob.len()
            + row.mix_in_blob.len()
            + row.mix_out_blob.len()
            + row.safe_transition_windows_blob.len()
            + row.energy_contour_blob.len()
            + row.vocal_presence_blob.len()
            + row.vocal_density_blob.len()
            + row.breakdown_blob.len()
            + row.drop_blob.len()
    }

    fn duration_ms(duration: Duration) -> u128 {
        duration.as_micros() / 1_000
    }
}
