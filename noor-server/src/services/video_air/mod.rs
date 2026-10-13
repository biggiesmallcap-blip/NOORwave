//! On Air: the Programmer and numbered channels. Taste is borrowed from the
//! music library (roots), shaped per scene (appetite) and per session (zaps,
//! holds, likes). Channels are filters over video_facets with a daily clock
//! schedule frozen at build time (video_channel_day). Nothing here calls TIDAL.
//!
//! Layers, each pure over the one below where it can be:
//! - `universe`: the programmable catalog with its facets, songs, concerts
//!   and scenes, loaded once per build (catalog fallback before the first
//!   intelligence build).
//! - `taste`: roots from the library, relevance over the artist graph, scene
//!   appetite and watch history.
//! - `channels`: the fixed channels 1-8 and the scene channels, their pools,
//!   lock rules and copy.
//! - `programmer`: artist-first sampling with rings, frontier picks, format
//!   rules and why chips. No database access.
//! - `clock`: four 6-hour blocks a day, seeds, and where "now" falls.
//! - `day`: builds, stores and serves a day of every channel.
//! - `session`: refills past the schedule from the session's signals.

pub mod channels;
pub mod clock;
pub mod day;
pub mod programmer;
pub mod session;
pub mod taste;
pub mod universe;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const EXPLORE_KEY: &str = "video_air.explore";

/// The explore knob: how far from the listener's taste picks may wander.
/// Same three bands as Sound Space's coherence control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExploreMode {
    Familiar,
    #[default]
    Mixed,
    Adventurous,
}

impl ExploreMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "familiar" => Some(Self::Familiar),
            "mixed" => Some(Self::Mixed),
            "adventurous" => Some(Self::Adventurous),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Familiar => "familiar",
            Self::Mixed => "mixed",
            Self::Adventurous => "adventurous",
        }
    }

    /// Share of picks drawn from the near, mid and far rings.
    pub fn rings(self) -> [f64; 3] {
        match self {
            Self::Familiar => [0.80, 0.18, 0.02],
            Self::Mixed => [0.60, 0.30, 0.10],
            Self::Adventurous => [0.35, 0.40, 0.25],
        }
    }

    /// One band closer to home (Your Channel leans this way).
    pub fn closer(self) -> Self {
        match self {
            Self::Adventurous => Self::Mixed,
            _ => Self::Familiar,
        }
    }

    /// One band further out (Wild Card leans this way).
    pub fn wider(self) -> Self {
        match self {
            Self::Familiar => Self::Mixed,
            _ => Self::Adventurous,
        }
    }
}

/// The stored knob; mixed when never set or unreadable.
pub fn load_explore(conn: &Connection) -> Result<ExploreMode> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [EXPLORE_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .as_deref()
        .and_then(ExploreMode::parse)
        .unwrap_or_default())
}

pub fn save_explore(conn: &Connection, mode: ExploreMode) -> Result<()> {
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![EXPLORE_KEY, mode.as_str()],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explore_defaults_to_mixed_and_persists() {
        let conn = test_support::conn();
        assert_eq!(load_explore(&conn).unwrap(), ExploreMode::Mixed);
        save_explore(&conn, ExploreMode::Adventurous).unwrap();
        assert_eq!(load_explore(&conn).unwrap(), ExploreMode::Adventurous);
        save_explore(&conn, ExploreMode::Familiar).unwrap();
        assert_eq!(load_explore(&conn).unwrap(), ExploreMode::Familiar);
        conn.execute(
            "UPDATE server_config SET value = 'loud' WHERE key = ?1",
            [EXPLORE_KEY],
        )
        .unwrap();
        assert_eq!(load_explore(&conn).unwrap(), ExploreMode::Mixed);
    }

    #[test]
    fn explore_bands_round_trip_and_shift() {
        for mode in [
            ExploreMode::Familiar,
            ExploreMode::Mixed,
            ExploreMode::Adventurous,
        ] {
            assert_eq!(ExploreMode::parse(mode.as_str()), Some(mode));
            let total: f64 = mode.rings().iter().sum();
            assert!((total - 1.0).abs() < 1e-9);
        }
        assert_eq!(ExploreMode::Mixed.closer(), ExploreMode::Familiar);
        assert_eq!(ExploreMode::Familiar.closer(), ExploreMode::Familiar);
        assert_eq!(ExploreMode::Mixed.wider(), ExploreMode::Adventurous);
        assert_eq!(ExploreMode::Adventurous.wider(), ExploreMode::Adventurous);
        assert_eq!(ExploreMode::parse("loud"), None);
    }
}
