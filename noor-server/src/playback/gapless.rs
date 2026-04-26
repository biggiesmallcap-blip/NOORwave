use crate::services::tidal::stream::StreamInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaplessSettings {
    pub enabled: bool,
    pub crossfade_ms: i32,
}

impl GaplessSettings {
    pub fn new(enabled: bool, crossfade_ms: i32) -> Self {
        Self {
            enabled,
            crossfade_ms: crossfade_ms.max(0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaplessPlan {
    pub enabled: bool,
    pub overlap_ms: i32,
    pub prebuffer_ms: i32,
    pub requires_stream_metadata: bool,
}

impl GaplessPlan {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            overlap_ms: 0,
            prebuffer_ms: 0,
            requires_stream_metadata: false,
        }
    }
}

/// Build a playback transition plan from the current stream metadata and player settings.
pub fn plan_from_stream(stream: Option<&StreamInfo>, settings: GaplessSettings) -> GaplessPlan {
    if !settings.enabled {
        // Even without gapless, we need a small prebuffer to avoid starting playback
        // with an empty buffer (causes stuttering). 500ms is enough to decode ~20+ packets.
        return GaplessPlan {
            enabled: false,
            overlap_ms: 0,
            prebuffer_ms: 500,
            requires_stream_metadata: false,
        };
    }

    let overlap_ms = settings.crossfade_ms;
    let stream_supports_gapless = stream.map_or(false, StreamInfo::supports_gapless);
    let enabled = stream_supports_gapless && overlap_ms > 0;
    // 500 ms of prebuffer is enough to cover decoder jitter for both the cold-
    // start and the pre-decoded-next paths. The earlier value of `overlap_ms +
    // 250` was conflating "fade-ramp duration" with "buffer-fill threshold" —
    // the consequence was that a pre-decoded next engine had to buffer the
    // entire crossfade window (~5 s) before is_ready() flipped, by which time
    // the crossfade window had already expired.
    let prebuffer_ms = 500;

    GaplessPlan {
        enabled,
        overlap_ms: if enabled { overlap_ms } else { 0 },
        prebuffer_ms,
        requires_stream_metadata: stream.is_some(),
    }
}

/// Convenience helper for callers that only have a crossfade value.
pub fn plan_from_crossfade(crossfade_ms: i32) -> GaplessPlan {
    plan_from_stream(None, GaplessSettings::new(true, crossfade_ms))
}

/// Clamp applied to any beat-aligned crossfade value (matches existing UI range).
const CROSSFADE_MAX_MS: u32 = 12_000;

/// Quantise a crossfade duration to the nearest whole number of beats at the given BPM.
/// If the BPM is outside a sane range, the original crossfade is returned unchanged.
pub fn align_crossfade_to_beat(crossfade_ms: u32, bpm: f64) -> u32 {
    if !(60.0..=220.0).contains(&bpm) {
        return crossfade_ms;
    }
    let beat_ms = 60_000.0 / bpm;
    let beats = (crossfade_ms as f64 / beat_ms).round().max(1.0);
    (beats * beat_ms).round() as u32
}

/// Build a playback transition plan, beat-aligning the crossfade when BPM metadata
/// is available for both the currently-playing track and the next track.
///
/// Fallback behaviour: if either track has no BPM (unanalyzed), the original
/// `crossfade_ms` is used as-is. The final value is clamped to `0..=CROSSFADE_MAX_MS`.
pub fn build_gapless_plan(
    stream: Option<&StreamInfo>,
    settings: GaplessSettings,
    current_bpm: Option<f64>,
    next_bpm: Option<f64>,
) -> GaplessPlan {
    let adjusted = match (current_bpm, next_bpm) {
        (Some(a), Some(b)) if a > 0.0 && b > 0.0 => {
            // Average the two BPMs so neither track's beat grid "wins" the alignment.
            let avg_bpm = (a + b) / 2.0;
            let base = settings.crossfade_ms.max(0) as u32;
            let aligned = align_crossfade_to_beat(base, avg_bpm).min(CROSSFADE_MAX_MS);
            GaplessSettings::new(settings.enabled, aligned as i32)
        }
        _ => settings,
    };

    plan_from_stream(stream, adjusted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tidal::stream::StreamInfo;

    fn stream(codec: &str) -> StreamInfo {
        StreamInfo {
            url: "https://example.com/stream.flac".to_string(),
            track_id: 1,
            audio_quality: "LOSSLESS".to_string(),
            codec: codec.to_string(),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
        }
    }

    #[test]
    fn disabled_when_player_crossfade_is_off() {
        let plan = plan_from_stream(Some(&stream("audio/flac")), GaplessSettings::new(true, 0));
        assert!(!plan.enabled);
        assert_eq!(plan.overlap_ms, 0);
        assert_eq!(plan.prebuffer_ms, 500); // still has startup buffer
    }

    #[test]
    fn enables_gapless_for_lossless_streams() {
        let plan = plan_from_stream(
            Some(&stream("audio/flac")),
            GaplessSettings::new(true, 1500),
        );
        assert!(plan.enabled);
        assert_eq!(plan.overlap_ms, 1500);
        assert_eq!(plan.prebuffer_ms, 500);
        assert!(plan.requires_stream_metadata);
    }

    #[test]
    fn falls_back_when_stream_format_is_not_gapless_friendly() {
        let plan = plan_from_stream(Some(&stream("audio/ogg")), GaplessSettings::new(true, 1500));
        assert!(!plan.enabled);
        assert_eq!(plan.overlap_ms, 0);
        assert_eq!(plan.prebuffer_ms, 500); // still has startup buffer
    }

    #[test]
    fn disabled_when_no_stream_metadata_is_available() {
        let plan = plan_from_crossfade(1500);
        assert!(!plan.enabled);
        assert_eq!(plan.overlap_ms, 0);
        assert_eq!(plan.prebuffer_ms, 500); // still has startup buffer
        assert!(!plan.requires_stream_metadata);
    }
}
