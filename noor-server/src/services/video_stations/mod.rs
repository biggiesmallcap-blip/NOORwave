//! Endless video stations built from the local video catalog. A daily
//! lineup decides which stations exist; every refill picks fresh videos
//! from the catalog. Nothing here calls TIDAL.

pub mod pick;

use crate::services::video_sets::{VideoCandidate, VideoSetItem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Vibe {
    Psychedelic,
    Atmosphere,
    Dance,
    Mellow,
    Dark,
}

impl Vibe {
    pub const ALL: [Vibe; 5] = [
        Vibe::Psychedelic,
        Vibe::Atmosphere,
        Vibe::Dance,
        Vibe::Mellow,
        Vibe::Dark,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Psychedelic => "psychedelic",
            Self::Atmosphere => "atmosphere",
            Self::Dance => "dance",
            Self::Mellow => "mellow",
            Self::Dark => "dark",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Psychedelic => "Psychedelic and trippy",
            Self::Atmosphere => "Atmosphere",
            Self::Dance => "Dance",
            Self::Mellow => "Mellow",
            Self::Dark => "Dark",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Psychedelic => "Psychedelic rock, trip-hop and space rock",
            Self::Atmosphere => "Ambient, dream pop and shoegaze",
            Self::Dance => "Dance, club and party",
            Self::Mellow => "Mellow, beautiful and melancholy",
            Self::Dark => "Darkwave, gothic and industrial",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|vibe| vibe.slug() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StationId {
    WildCard,
    Shuffle,
    DeepCuts,
    BigOnes,
    Genre(String),
    Vibe(Vibe),
    Duets,
    Live,
    Spotlight(i64),
    Charts,
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 60
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl StationId {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "wild-card" => Some(Self::WildCard),
            "shuffle" => Some(Self::Shuffle),
            "deep-cuts" => Some(Self::DeepCuts),
            "big-ones" => Some(Self::BigOnes),
            "duets" => Some(Self::Duets),
            "live" => Some(Self::Live),
            "charts" => Some(Self::Charts),
            _ => {
                if let Some(slug) = raw.strip_prefix("genre:") {
                    valid_slug(slug).then(|| Self::Genre(slug.to_string()))
                } else if let Some(vibe) = raw.strip_prefix("vibe:") {
                    Vibe::parse(vibe).map(Self::Vibe)
                } else if let Some(id) = raw.strip_prefix("spotlight:") {
                    id.parse::<i64>()
                        .ok()
                        .filter(|id| *id > 0)
                        .map(Self::Spotlight)
                } else {
                    None
                }
            }
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::WildCard => "wild-card".into(),
            Self::Shuffle => "shuffle".into(),
            Self::DeepCuts => "deep-cuts".into(),
            Self::BigOnes => "big-ones".into(),
            Self::Genre(slug) => format!("genre:{slug}"),
            Self::Vibe(vibe) => format!("vibe:{}", vibe.slug()),
            Self::Duets => "duets".into(),
            Self::Live => "live".into(),
            Self::Spotlight(id) => format!("spotlight:{id}"),
            Self::Charts => "charts".into(),
        }
    }

    /// Stations that only ever play videos the listener has not watched.
    pub fn unwatched_only(&self) -> bool {
        matches!(
            self,
            Self::WildCard | Self::Shuffle | Self::DeepCuts | Self::BigOnes | Self::Spotlight(_)
        )
    }
}

/// The shape the player and the lineup store.
pub fn to_item(video: &VideoCandidate) -> VideoSetItem {
    VideoSetItem {
        tidal_id: video.tidal_id,
        title: video.title.clone(),
        duration_ms: video.duration_s.map(|seconds| seconds * 1000),
        artist_id: video.artist_id,
        artist_name: video.artist_name.clone(),
        album_tidal_id: video.album_tidal_id,
        artwork_url: video.artwork_url.clone(),
        quality: video.quality.clone(),
        explicit: video.explicit,
        kind: video
            .video_type
            .clone()
            .unwrap_or_else(|| "Music Video".into()),
        why: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_ids_round_trip() {
        for raw in [
            "wild-card",
            "shuffle",
            "deep-cuts",
            "big-ones",
            "genre:psychedelic-rock",
            "vibe:atmosphere",
            "duets",
            "live",
            "spotlight:5396",
            "charts",
        ] {
            assert_eq!(StationId::parse(raw).unwrap().as_string(), raw);
        }
    }

    #[test]
    fn malformed_station_ids_are_rejected() {
        for raw in [
            "",
            "genre:",
            "genre:Rock",
            "genre:a b",
            "vibe:loud",
            "spotlight:0",
            "spotlight:x",
            "everything",
        ] {
            assert!(StationId::parse(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn only_discovery_stations_are_unwatched_only() {
        assert!(StationId::WildCard.unwatched_only());
        assert!(StationId::Spotlight(1).unwatched_only());
        assert!(!StationId::Genre("rock".into()).unwatched_only());
        assert!(!StationId::Charts.unwatched_only());
    }
}
