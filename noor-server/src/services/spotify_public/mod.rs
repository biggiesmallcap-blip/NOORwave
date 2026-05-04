//! Public Spotify stats via anonymous GraphQL.
//!
//! Off by default (cargo feature `spotify-public` + env `NOOR_SPOTIFY_PUBLIC_STATS=1`).
//! Returns rich playcount + monthly-listener data not exposed by the official
//! REST API. Schemas are private/undocumented; expect occasional breakage.
//!
//! V1 surface: the entry point [`fetch_artist_stats`] is wired up and the
//! SQLite cache tables are created by `MIGRATION_029`. The actual HTTP calls
//! to Spotify's anonymous endpoints are intentionally stubbed in V1 — wiring
//! them up is a fast follow-up once the schema has been verified against
//! current Spotify responses. While stubbed, the route returns clean empty
//! data (no errors, no toasts) so the frontend can ship the integration
//! without waiting on the backend implementation.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct ArtistStatsResult {
    pub monthly_listeners: Option<i64>,
    pub tracks: Vec<TrackStat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackStat {
    pub isrc: String,
    pub title: String,
    pub playcount: Option<i64>,
}

/// Fetch Spotify public stats for an artist. Always succeeds; on error or when
/// the feature is gated off, returns an empty result.
///
/// `tracks_with_isrc` should be the artist's top local tracks (ordered by play
/// count, capped at 10) that have non-null ISRCs.
pub async fn fetch_artist_stats(
    enabled: bool,
    _local_artist_name: &str,
    _tracks_with_isrc: &[(String, String)], // (isrc, title)
) -> ArtistStatsResult {
    if !enabled {
        return ArtistStatsResult::default();
    }

    // V1 stub: feature flag is on but no upstream calls implemented yet.
    // Real implementation: see plan Part 5b / 5c.
    //   1. Fetch anonymous token from open.spotify.com/get_access_token
    //   2. Resolve each ISRC via /v1/search?q=isrc:... -> spotify track id
    //   3. GraphQL getTrack -> playcount
    //   4. GraphQL queryArtistOverview -> monthlyListeners (with name-match guard)
    //   5. Cache via spotify_isrc_map / spotify_track_stats / spotify_artist_stats
    ArtistStatsResult::default()
}
