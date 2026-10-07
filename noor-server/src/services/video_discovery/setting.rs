//! The listener's choice of how much background video discovery to run.

use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const SETTING_KEY: &str = "video_discovery.setting";

/// `Full` crawls with the normal budget, `Limited` with a small one, and `Off`
/// runs no background discovery: only a station the listener starts is looked
/// up on demand, so radio keeps working.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoverySetting {
    #[default]
    Full,
    Limited,
    Off,
}

/// Call spacing and caps for one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub idle_spacing: Duration,
    pub active_spacing: Duration,
    pub idle_per_hour: usize,
    pub active_per_hour: usize,
    pub daily: usize,
}

pub const FULL_BUDGET: Budget = Budget {
    idle_spacing: Duration::from_secs(2),
    active_spacing: Duration::from_secs(12),
    idle_per_hour: 500,
    active_per_hour: 150,
    daily: 6000,
};

/// About a tenth of the full budget, spread evenly over the hour rather than
/// spent in a burst. Also caps on-demand station lookups when background
/// discovery is off (station work skips the spacing, never the caps).
pub const LIMITED_BUDGET: Budget = Budget {
    idle_spacing: Duration::from_secs(60),
    active_spacing: Duration::from_secs(180),
    idle_per_hour: 60,
    active_per_hour: 20,
    daily: 500,
};

impl DiscoverySetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Limited => "limited",
            Self::Off => "off",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "limited" => Some(Self::Limited),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn budget(self) -> Budget {
        match self {
            Self::Full => FULL_BUDGET,
            Self::Limited | Self::Off => LIMITED_BUDGET,
        }
    }

    /// Whether the crawler plans its own work, beyond stations the listener starts.
    pub fn background(self) -> bool {
        self != Self::Off
    }
}

/// An unknown stored value reads as the default rather than failing.
pub fn load(conn: &Connection) -> Result<DiscoverySetting> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [SETTING_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value
        .as_deref()
        .and_then(DiscoverySetting::parse)
        .unwrap_or_default())
}

pub fn save(conn: &Connection, setting: DiscoverySetting) -> Result<()> {
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SETTING_KEY, setting.as_str()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn the_setting_defaults_to_full_and_round_trips() {
        let conn = conn();
        assert_eq!(load(&conn).unwrap(), DiscoverySetting::Full);
        for setting in [
            DiscoverySetting::Off,
            DiscoverySetting::Limited,
            DiscoverySetting::Full,
        ] {
            save(&conn, setting).unwrap();
            assert_eq!(load(&conn).unwrap(), setting);
        }
        conn.execute(
            "UPDATE server_config SET value = 'turbo' WHERE key = ?1",
            [SETTING_KEY],
        )
        .unwrap();
        assert_eq!(load(&conn).unwrap(), DiscoverySetting::Full);
    }

    #[test]
    fn limited_and_off_share_the_small_budget() {
        assert_eq!(DiscoverySetting::Full.budget(), FULL_BUDGET);
        assert_eq!(DiscoverySetting::Limited.budget(), LIMITED_BUDGET);
        assert_eq!(DiscoverySetting::Off.budget(), LIMITED_BUDGET);
        assert!(!DiscoverySetting::Off.background());
        assert!(DiscoverySetting::Limited.background());
    }
}
