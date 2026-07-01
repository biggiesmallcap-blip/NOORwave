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
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
pub enum VideoQualityMode {
    #[default]
    Max,
    Auto,
}

impl VideoQualityMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoQualityMode::Max => "MAX",
            VideoQualityMode::Auto => "AUTO",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "MAX" => Some(Self::Max),
            "AUTO" => Some(Self::Auto),
            _ => None,
        }
    }
}

fn default_video_quality_mode() -> VideoQualityMode {
    VideoQualityMode::default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
pub enum ExclusiveLatencyMode {
    #[default]
    Stable,
    LowLatency,
    UltraLowLatency,
}

impl ExclusiveLatencyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExclusiveLatencyMode::Stable => "STABLE",
            ExclusiveLatencyMode::LowLatency => "LOW_LATENCY",
            ExclusiveLatencyMode::UltraLowLatency => "ULTRA_LOW_LATENCY",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "STABLE" => Some(Self::Stable),
            "LOW_LATENCY" => Some(Self::LowLatency),
            "ULTRA_LOW_LATENCY" => Some(Self::UltraLowLatency),
            _ => None,
        }
    }
}

fn default_exclusive_latency_mode() -> ExclusiveLatencyMode {
    ExclusiveLatencyMode::default()
}

pub const DEFAULT_EXCLUSIVE_RELEASE_GRACE_SECS: u32 = 30;
pub const MIN_EXCLUSIVE_RELEASE_GRACE_SECS: u32 = 5;
pub const MAX_EXCLUSIVE_RELEASE_GRACE_SECS: u32 = 120;

fn default_exclusive_release_grace_secs() -> u32 {
    DEFAULT_EXCLUSIVE_RELEASE_GRACE_SECS
}

pub fn clamp_exclusive_release_grace_secs(v: u32) -> u32 {
    v.clamp(
        MIN_EXCLUSIVE_RELEASE_GRACE_SECS,
        MAX_EXCLUSIVE_RELEASE_GRACE_SECS,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSettings {
    pub quality: AudioQuality,
    /// `None` means "system default".
    pub output_device: Option<String>,
    pub exclusive_mode: bool,
    pub sample_rate_follow: bool,
    #[serde(default = "default_video_quality_mode")]
    pub video_quality_mode: VideoQualityMode,
    #[serde(default = "default_exclusive_latency_mode")]
    pub exclusive_latency_mode: ExclusiveLatencyMode,
    /// Seconds of continuous silence (paused / no audio) before the exclusive
    /// WASAPI render thread releases the device so other apps can use it. On
    /// next playback the runtime re-grabs exclusive automatically. Clamped to
    /// 5..=120 by the setter route.
    #[serde(default = "default_exclusive_release_grace_secs")]
    pub exclusive_release_grace_secs: u32,
    /// When true, an explicit user Pause frees the exclusive WASAPI device
    /// immediately (re-grabbed on the next Resume/Play) instead of waiting out
    /// `exclusive_release_grace_secs`, so other apps can take the DAC the moment
    /// you pause. Default false: only the idle grace releases the device.
    #[serde(default)]
    pub exclusive_release_on_pause: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            quality: AudioQuality::Lossless,
            output_device: None,
            exclusive_mode: false,
            sample_rate_follow: false,
            video_quality_mode: VideoQualityMode::Max,
            exclusive_latency_mode: ExclusiveLatencyMode::Stable,
            exclusive_release_grace_secs: DEFAULT_EXCLUSIVE_RELEASE_GRACE_SECS,
            exclusive_release_on_pause: false,
        }
    }
}

pub fn load(conn: &Connection) -> rusqlite::Result<AudioSettings> {
    let mut s = AudioSettings::default();
    if let Some(v) = read_kv(conn, "audio.quality")?
        && let Some(q) = AudioQuality::from_tidal_str(&v)
    {
        s.quality = q;
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
    if let Some(v) = read_kv(conn, "video.quality_mode")?
        && let Some(mode) = VideoQualityMode::from_str(&v)
    {
        s.video_quality_mode = mode;
    }
    if let Some(v) = read_kv(conn, "audio.exclusive_latency_mode")?
        && let Some(mode) = ExclusiveLatencyMode::from_str(&v)
    {
        s.exclusive_latency_mode = mode;
    }
    if let Some(v) = read_kv(conn, "audio.exclusive_release_grace_secs")?
        && let Ok(parsed) = v.parse::<u32>()
    {
        s.exclusive_release_grace_secs = clamp_exclusive_release_grace_secs(parsed);
    }
    if let Some(v) = read_kv(conn, "audio.exclusive_release_on_pause")? {
        s.exclusive_release_on_pause = v == "true";
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
    write_kv(conn, "video.quality_mode", s.video_quality_mode.as_str())?;
    write_kv(
        conn,
        "audio.exclusive_latency_mode",
        s.exclusive_latency_mode.as_str(),
    )?;
    write_kv(
        conn,
        "audio.exclusive_release_grace_secs",
        &clamp_exclusive_release_grace_secs(s.exclusive_release_grace_secs).to_string(),
    )?;
    write_kv(
        conn,
        "audio.exclusive_release_on_pause",
        if s.exclusive_release_on_pause {
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
        assert_eq!(s.exclusive_latency_mode, ExclusiveLatencyMode::Stable);
        assert_eq!(s.video_quality_mode, VideoQualityMode::Max);
    }

    #[test]
    fn save_then_load_round_trips_all_fields() {
        let conn = fresh_conn();
        let want = AudioSettings {
            quality: AudioQuality::HiResLossless,
            output_device: Some("USB DAC #1".into()),
            exclusive_mode: true,
            sample_rate_follow: true,
            exclusive_latency_mode: ExclusiveLatencyMode::LowLatency,
            video_quality_mode: VideoQualityMode::Auto,
            exclusive_release_grace_secs: 60,
            exclusive_release_on_pause: true,
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
            exclusive_latency_mode: ExclusiveLatencyMode::UltraLowLatency,
            video_quality_mode: VideoQualityMode::Max,
            exclusive_release_grace_secs: 90,
            exclusive_release_on_pause: true,
        };
        save(&conn, &updated).unwrap();
        assert_eq!(load(&conn).unwrap(), updated);
    }

    #[test]
    fn grace_secs_is_clamped_on_load() {
        let conn = fresh_conn();
        write_kv(&conn, "audio.exclusive_release_grace_secs", "999").unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(
            s.exclusive_release_grace_secs,
            MAX_EXCLUSIVE_RELEASE_GRACE_SECS
        );
        write_kv(&conn, "audio.exclusive_release_grace_secs", "0").unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(
            s.exclusive_release_grace_secs,
            MIN_EXCLUSIVE_RELEASE_GRACE_SECS
        );
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

    #[test]
    fn video_quality_mode_serializes_to_setting_strings() {
        assert_eq!(VideoQualityMode::Max.as_str(), "MAX");
        assert_eq!(
            VideoQualityMode::from_str("AUTO"),
            Some(VideoQualityMode::Auto)
        );
        assert_eq!(VideoQualityMode::from_str("LOW"), None);
    }

    #[test]
    fn exclusive_latency_mode_serializes_to_setting_strings() {
        assert_eq!(ExclusiveLatencyMode::Stable.as_str(), "STABLE");
        assert_eq!(
            ExclusiveLatencyMode::from_str("LOW_LATENCY"),
            Some(ExclusiveLatencyMode::LowLatency)
        );
        assert_eq!(
            ExclusiveLatencyMode::from_str("ULTRA_LOW_LATENCY"),
            Some(ExclusiveLatencyMode::UltraLowLatency)
        );
        assert_eq!(ExclusiveLatencyMode::from_str("FAST"), None);
    }
}
