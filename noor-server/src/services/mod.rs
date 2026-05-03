pub mod acrcloud;
pub mod audio_analysis;
pub mod charts;
pub mod discovery;
pub mod discovery_trainer;
pub mod lastfm;
pub mod learning;
pub mod listen_history_backfill;
pub mod musicbrainz;
pub mod radio;
pub mod radio_config;
pub mod rss_feeds;
pub mod spotify;
pub mod tidal;

/// Trait that all streaming services implement
/// Designed for extensibility: TIDAL now, YTM + SoundCloud + Spotify + Bandcamp later
pub trait ServiceProvider: Send + Sync {
    fn name(&self) -> &str;
    fn is_authenticated(&self) -> bool;
}
