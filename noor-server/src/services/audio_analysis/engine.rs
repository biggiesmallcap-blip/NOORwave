/// Audio analysis orchestrator.
///
/// Combines BPM detection, key detection, energy, LUFS, spectral centroid,
/// instrumental detection, and danceability into a single analysis pass.
///
/// ALWAYS use `analyze_clip_safe` (which wraps with panic catching) — never call `analyze_clip` directly.
use crate::db::models::AudioDspFeatures;
use std::panic::AssertUnwindSafe;

use super::bpm;
use super::features;
use super::key;

/// Analyze 30 seconds of mono audio samples.
/// Returns `AudioDspFeatures` with all computed values.
///
/// Individual analysis steps may return `None` if confidence is too low —
/// those fields will be `None` in the returned struct.
///
/// `offset_ms` records how many milliseconds were skipped before `samples`
/// starts (e.g. 10 000 when the first 10 s of a preview clip were dropped).
pub fn analyze_clip(
    samples: &[f32],
    sample_rate: u32,
    source: &str,
    track_id: i64,
    offset_ms: i64,
) -> AudioDspFeatures {
    // 1. BPM detection
    let (bpm, beat_strength) = detect_bpm(samples, sample_rate);

    // 2. Key detection
    let (key_signature, camelot_key) = detect_key(samples, sample_rate);

    // 3. Energy
    let energy = Some(features::compute_energy(samples));

    // 4. LUFS
    let loudness_lufs = features::compute_lufs(samples, sample_rate);

    // 5-6, 8. One STFT pass feeds spectral_centroid + instrumental + danceability.
    let stft = features::compute_stft_features(samples, sample_rate);
    let spectral_centroid = stft.as_ref().and_then(|s| s.centroid_hz);
    let is_instrumental = stft.as_ref().and_then(features::detect_instrumental_from);
    let danceability = stft
        .as_ref()
        .and_then(|s| features::compute_danceability_from(s, bpm, beat_strength));

    // 7. Stereo width — placeholder; pipeline currently consumes mono samples only.
    let stereo_width = Some(0.5);

    let now = chrono::Utc::now().to_rfc3339();

    AudioDspFeatures {
        track_id,
        bpm,
        key_signature,
        camelot_key,
        loudness_lufs,
        energy,
        danceability,
        beat_strength,
        spectral_centroid,
        stereo_width,
        is_instrumental: is_instrumental.unwrap_or(false),
        analysis_source: source.to_string(),
        analysis_offset_ms: offset_ms,
        samples_analyzed: Some(samples.len() as i64),
        analyzed_at: now,
        analysis_version: super::CURRENT_ANALYSIS_VERSION.to_string(),
    }
}

/// Safe wrapper that catches panics.
///
/// ALWAYS call this — never `analyze_clip` directly.
/// Panics are logged at WARN level and return `None`.
///
/// `offset_ms` is forwarded to `analyze_clip` and recorded on the returned
/// `AudioDspFeatures::analysis_offset_ms` field.
pub fn analyze_clip_safe(
    samples: &[f32],
    sample_rate: u32,
    source: &str,
    track_id: i64,
    offset_ms: i64,
) -> Option<AudioDspFeatures> {
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        analyze_clip(samples, sample_rate, source, track_id, offset_ms)
    }))
    .map_err(|e| {
        let msg = if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        tracing::warn!(track_id, "analyze_clip panicked: {}", msg);
    })
    .ok()
}

/// Analyze and save to database.
///
/// Calls `analyze_clip_safe`, then upserts into `audio_dsp_features`.
///
/// Pass `offset_ms = 0` when no intro was skipped (e.g. passive / local-file
/// analysis).  Pass the real offset in milliseconds when the caller already
/// dropped leading samples (e.g. the preview scan skips the first 10 s →
/// `offset_ms = 10_000`).
pub fn analyze_and_save(
    db: &crate::db::Database,
    samples: &[f32],
    sample_rate: u32,
    source: &str,
    track_id: i64,
    offset_ms: i64,
) -> Option<AudioDspFeatures> {
    let features = analyze_clip_safe(samples, sample_rate, source, track_id, offset_ms)?;

    db.with_conn(|conn| crate::db::queries::upsert_audio_dsp_features(conn, &features))
        .map_err(|e| {
            tracing::debug!(track_id, "failed to save DSP features: {}", e);
        })
        .ok()?;

    Some(features)
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn detect_bpm(samples: &[f32], sample_rate: u32) -> (Option<f64>, Option<f64>) {
    bpm::detect_bpm(samples, sample_rate)
        .map(|(bpm, strength)| (Some(bpm), Some(strength)))
        .unwrap_or((None, None))
}

fn detect_key(samples: &[f32], sample_rate: u32) -> (Option<String>, Option<String>) {
    key::detect_key(samples, sample_rate)
        .map(|(sig, camelot)| (Some(sig), Some(camelot)))
        .unwrap_or((None, None))
}
