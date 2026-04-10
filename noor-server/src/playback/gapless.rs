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
    let prebuffer_ms = if enabled {
        overlap_ms.saturating_add(250)
    } else {
        500
    };

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
        assert_eq!(plan.prebuffer_ms, 1750);
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
