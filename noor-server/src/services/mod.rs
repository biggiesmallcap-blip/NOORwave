pub mod discovery;
pub mod musicbrainz;
pub mod tidal;

/// Trait that all streaming services implement
/// Designed for extensibility: TIDAL now, YTM + SoundCloud + Spotify + Bandcamp later
pub trait ServiceProvider: Send + Sync {
    fn name(&self) -> &str;
    fn is_authenticated(&self) -> bool;
}
