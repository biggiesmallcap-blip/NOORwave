use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AudioQuality {
    Low,
    High,
    Lossless,
    HiResLossless,
}

impl AudioQuality {
    pub fn as_tidal_str(&self) -> &'static str {
        match self {
            AudioQuality::Low => "LOW",
            AudioQuality::High => "HIGH",
            AudioQuality::Lossless => "LOSSLESS",
            AudioQuality::HiResLossless => "HI_RES_LOSSLESS",
        }
    }

    pub fn from_tidal_str(s: &str) -> Option<Self> {
        match s {
            "LOW" => Some(Self::Low),
            "HIGH" => Some(Self::High),
            "LOSSLESS" => Some(Self::Lossless),
            "HI_RES_LOSSLESS" => Some(Self::HiResLossless),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSettings {
    pub quality: AudioQuality,
    /// `None` means "system default".
    pub output_device: Option<String>,
    pub exclusive_mode: bool,
    pub sample_rate_follow: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            quality: AudioQuality::Lossless,
            output_device: None,
            exclusive_mode: false,
            sample_rate_follow: false,
        }
    }
}

pub fn load(conn: &Connection) -> rusqlite::Result<AudioSettings> {
    let mut s = AudioSettings::default();
    if let Some(v) = read_kv(conn, "audio.quality")? {
        if let Some(q) = AudioQuality::from_tidal_str(&v) {
            s.quality = q;
        }
    }
    if let Some(v) = read_kv(conn, "audio.output_device")? {
        s.output_device = if v == "default" { None } else { Some(v) };
    }
    if let Some(v) = read_kv(conn, "audio.exclusive_mode")? {
        s.exclusive_mode = v == "true";
    }
    if let Some(v) = read_kv(conn, "audio.sample_rate_follow")? {
        s.sample_rate_follow = v == "true";
    }
    Ok(s)
}

pub fn save(conn: &Connection, s: &AudioSettings) -> rusqlite::Result<()> {
    write_kv(conn, "audio.quality", s.quality.as_tidal_str())?;
    write_kv(
        conn,
        "audio.output_device",
        s.output_device.as_deref().unwrap_or("default"),
    )?;
    write_kv(
        conn,
        "audio.exclusive_mode",
        if s.exclusive_mode { "true" } else { "false" },
    )?;
    write_kv(
        conn,
        "audio.sample_rate_follow",
        if s.sample_rate_follow {
            "true"
        } else {
            "false"
        },
    )?;
    Ok(())
}

fn read_kv(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM server_config WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
}

fn write_kv(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE server_config (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn load_returns_defaults_on_empty_db() {
        let conn = fresh_conn();
        let s = load(&conn).unwrap();
        assert_eq!(s, AudioSettings::default());
        assert_eq!(s.quality, AudioQuality::Lossless);
        assert_eq!(s.output_device, None);
        assert!(!s.exclusive_mode);
        assert!(!s.sample_rate_follow);
    }

    #[test]
    fn save_then_load_round_trips_all_fields() {
        let conn = fresh_conn();
        let want = AudioSettings {
            quality: AudioQuality::HiResLossless,
            output_device: Some("USB DAC #1".into()),
            exclusive_mode: true,
            sample_rate_follow: true,
        };
        save(&conn, &want).unwrap();
        let got = load(&conn).unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn save_overwrites_previous_values() {
        let conn = fresh_conn();
        save(&conn, &AudioSettings::default()).unwrap();
        let updated = AudioSettings {
            quality: AudioQuality::High,
            output_device: Some("Other".into()),
            exclusive_mode: true,
            sample_rate_follow: false,
        };
        save(&conn, &updated).unwrap();
        assert_eq!(load(&conn).unwrap(), updated);
    }

    #[test]
    fn quality_serializes_to_tidal_strings() {
        assert_eq!(
            AudioQuality::HiResLossless.as_tidal_str(),
            "HI_RES_LOSSLESS"
        );
        assert_eq!(
            AudioQuality::from_tidal_str("LOSSLESS"),
            Some(AudioQuality::Lossless)
        );
        assert_eq!(AudioQuality::from_tidal_str("MQA"), None);
    }
}
