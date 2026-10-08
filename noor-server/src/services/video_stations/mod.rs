//! Endless video stations built from the local video catalog. A daily
//! lineup decides which stations exist; every refill picks fresh videos
//! from the catalog. Nothing here calls TIDAL.

pub mod lineup;
pub mod pick;
pub mod pool;
pub mod settings;

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use anyhow::Result;
use rusqlite::Connection;

use pick::{Order, PickInput};
use pool::Listener;

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

/// Genre scenes for the Explore row. Unlike "Your genres" they do not depend
/// on listening history; each draws on a list of genre names (see
/// `pool::scene_genres`) and the listener can switch any of them off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scene {
    Latin,
    Reggaeton,
    Reggae,
    Afrobeats,
    Kpop,
    Metal,
    Punk,
    Classical,
    Jazz,
    Country,
    DiscoFunk,
}

impl Scene {
    /// Display order: Latin and Caribbean, global pop, heavier, timeless.
    pub const ALL: [Scene; 11] = [
        Scene::Latin,
        Scene::Reggaeton,
        Scene::Reggae,
        Scene::Afrobeats,
        Scene::Kpop,
        Scene::Metal,
        Scene::Punk,
        Scene::Classical,
        Scene::Jazz,
        Scene::Country,
        Scene::DiscoFunk,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Latin => "latin",
            Self::Reggaeton => "reggaeton",
            Self::Reggae => "reggae",
            Self::Afrobeats => "afrobeats",
            Self::Kpop => "k-pop",
            Self::Metal => "metal",
            Self::Punk => "punk",
            Self::Classical => "classical",
            Self::Jazz => "jazz",
            Self::Country => "country",
            Self::DiscoFunk => "disco-funk",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Latin => "Latin",
            Self::Reggaeton => "Reggaeton",
            Self::Reggae => "Reggae and dancehall",
            Self::Afrobeats => "Afrobeats",
            Self::Kpop => "K-pop",
            Self::Metal => "Metal",
            Self::Punk => "Punk",
            Self::Classical => "Classical",
            Self::Jazz => "Jazz",
            Self::Country => "Country and Americana",
            Self::DiscoFunk => "Disco and funk",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Latin => "Latin pop, salsa, bachata and cumbia",
            Self::Reggaeton => "Perreo, dembow and urbano",
            Self::Reggae => "Roots, reggae and dancehall",
            Self::Afrobeats => "Afrobeats and afrobeat",
            Self::Kpop => "Idol groups and K-pop soloists",
            Self::Metal => "Heavy, thrash, nu and prog metal",
            Self::Punk => "Punk rock, post-punk and pop punk",
            Self::Classical => "Orchestras, opera and solo piano",
            Self::Jazz => "From standards to the new scene",
            Self::Country => "Country, Americana and roots",
            Self::DiscoFunk => "Grooves for the floor",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scene| scene.slug() == raw)
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
    Scene(Scene),
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
                } else if let Some(scene) = raw.strip_prefix("scene:") {
                    Scene::parse(scene).map(Self::Scene)
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
            Self::Scene(scene) => format!("scene:{}", scene.slug()),
            Self::Duets => "duets".into(),
            Self::Live => "live".into(),
            Self::Spotlight(id) => format!("spotlight:{id}"),
            Self::Charts => "charts".into(),
        }
    }

    pub fn order(&self) -> Order {
        match self {
            Self::WildCard => Order::Rings,
            Self::Shuffle => Order::Uniform,
            Self::DeepCuts => Order::DeepCuts,
            Self::BigOnes | Self::Spotlight(_) => Order::Popularity,
            Self::Genre(_) | Self::Vibe(_) | Self::Scene(_) => Order::LeanMixed,
            Self::Duets => Order::ArtistHop,
            Self::Live => Order::Lean,
            Self::Charts => Order::Chart,
        }
    }

    /// A spotlight is one artist on purpose; everything else spaces artists.
    pub fn spacing(&self) -> bool {
        !matches!(self, Self::Spotlight(_))
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

pub const BATCH: usize = 12;

pub fn pick_input<'a>(
    station: &StationId,
    excluded: &'a HashSet<i64>,
    seed: u64,
    limit: usize,
) -> PickInput<'a> {
    PickInput {
        order: station.order(),
        unwatched_only: station.unwatched_only(),
        prefer_live_cuts: *station == StationId::Live,
        spacing: station.spacing(),
        excluded,
        seed,
        limit,
    }
}

/// Stable per station, day and listening session, so refills continue one
/// order while a replay tomorrow (or a new session) reshuffles.
pub fn seed_for(station: &StationId, day: &str, nonce: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    (station.as_string(), day, nonce).hash(&mut hasher);
    hasher.finish()
}

pub fn next_batch(
    conn: &Connection,
    listener: &Listener,
    station: &StationId,
    excluded: &HashSet<i64>,
    seed: u64,
) -> Result<Vec<VideoSetItem>> {
    let candidates = pool::candidates(conn, listener, station)?;
    let videos = pick::pick(candidates, &pick_input(station, excluded, seed, BATCH));
    Ok(videos.iter().map(to_item).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refill_skips_what_the_session_already_has() {
        let conn = pool::tests::conn();
        for id in 1..=20 {
            pool::tests::add_video(&conn, &pool::tests::video(id, 100 + id));
        }
        let listener = pool::load_listener(&conn).unwrap();
        let station = StationId::Shuffle;
        let seed = seed_for(&station, "2026-10-07", "abc");
        let first = next_batch(&conn, &listener, &station, &HashSet::new(), seed).unwrap();
        assert_eq!(first.len(), BATCH);
        let seen: HashSet<i64> = first.iter().map(|v| v.tidal_id).collect();
        let second = next_batch(&conn, &listener, &station, &seen, seed).unwrap();
        assert_eq!(second.len(), 8);
        assert!(second.iter().all(|v| !seen.contains(&v.tidal_id)));
        let third_seen: HashSet<i64> = seen
            .union(&second.iter().map(|v| v.tidal_id).collect())
            .copied()
            .collect();
        assert!(
            next_batch(&conn, &listener, &station, &third_seen, seed)
                .unwrap()
                .is_empty()
        );
    }

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
