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

    // 3. LUFS (computed before energy: the energy map is derived from it)
    let loudness_lufs = features::compute_lufs(samples, sample_rate);

    // 4. Energy: perceptual map of LUFS, RMS-dB fallback for sub-second clips
    let energy = Some(features::compute_energy(samples, loudness_lufs));

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

    // Refuse to persist empty rows: when the input clip is silence (e.g. a
    // DRM-encrypted TIDAL preview that decodes to zeros), every analyser
    // returns None / 0.0 and we'd otherwise upsert a v{CURRENT}-versioned row
    // with no signal. That row would then mark the track as "already analysed"
    // and prevent the playback-driven actor from retrying with real audio.
    if is_empty_analysis(&features) {
        tracing::debug!(track_id, source, "skipping save: clip decoded to silence");
        return None;
    }

    db.with_conn(|conn| crate::db::queries::upsert_audio_dsp_features(conn, &features))
        .map_err(|e| {
            tracing::debug!(track_id, "failed to save DSP features: {}", e);
        })
        .ok()?;

    Some(features)
}

fn is_empty_analysis(f: &AudioDspFeatures) -> bool {
    // LUFS must be None too: a real-but-very-quiet track (below the -30 LUFS
    // energy floor) legitimately maps energy to 0.0, while true silence
    // (e.g. a DRM-encrypted clip decoding to zeros) gates every LUFS block
    // and yields None. Without this check quiet ambient tracks with no
    // detectable bpm/key would be misread as silence and never persisted.
    f.bpm.is_none()
        && f.key_signature.is_none()
        && f.loudness_lufs.is_none()
        && f.energy.map(|e| e < 0.001).unwrap_or(true)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn features(
        bpm: Option<f64>,
        key_signature: Option<&str>,
        loudness_lufs: Option<f64>,
        energy: Option<f64>,
    ) -> AudioDspFeatures {
        AudioDspFeatures {
            track_id: 1,
            bpm,
            key_signature: key_signature.map(str::to_string),
            camelot_key: None,
            loudness_lufs,
            energy,
            danceability: None,
            beat_strength: None,
            spectral_centroid: None,
            stereo_width: Some(0.5),
            is_instrumental: false,
            analysis_source: "test".to_string(),
            analysis_offset_ms: 0,
            samples_analyzed: Some(0),
            analyzed_at: "2026-01-01T00:00:00Z".to_string(),
            analysis_version: super::super::CURRENT_ANALYSIS_VERSION.to_string(),
        }
    }

    #[test]
    fn quiet_real_track_with_zero_energy_is_not_empty() {
        // Below the -30 LUFS energy floor the map legitimately produces 0.0;
        // a measured LUFS proves there was real audio, so the row must save.
        let f = features(None, None, Some(-38.0), Some(0.0));
        assert!(!is_empty_analysis(&f));
    }

    #[test]
    fn drm_silence_is_empty() {
        // All-zero decode: no bpm, no key, no LUFS (every block gated), 0.0
        // energy from the RMS fallback.
        let f = features(None, None, None, Some(0.0));
        assert!(is_empty_analysis(&f));
    }

    #[test]
    fn analysis_with_any_signal_is_not_empty() {
        assert!(!is_empty_analysis(&features(
            Some(120.0),
            None,
            None,
            Some(0.0)
        )));
        assert!(!is_empty_analysis(&features(
            None,
            Some("Am"),
            None,
            Some(0.0)
        )));
        assert!(!is_empty_analysis(&features(None, None, None, Some(0.4))));
    }
}
