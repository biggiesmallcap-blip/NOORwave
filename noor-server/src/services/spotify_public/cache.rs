//! TTL policy and convenience wrappers around the queries-layer reads/writes.
//!
//! The raw queries return whatever's in the cache. This module decides what
//! counts as "fresh enough" vs "stale" vs "negative-cached" vs "retry-zero".

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::queries::{
    CachedArtistStatsRow, CachedTrackStatsRow, clear_spotify_null_cache,
    get_cached_spotify_artist_stats, get_cached_spotify_track_stats_for_isrcs,
    get_spotify_artist_map, upsert_spotify_artist_map, upsert_spotify_artist_stats,
    upsert_spotify_isrc_map, upsert_spotify_null_cache, upsert_spotify_track_stats,
};

const STATS_TTL_SECS: i64 = 7 * 24 * 60 * 60;
const NULL_TTL_SECS: i64 = 24 * 60 * 60;
const ARTIST_MAP_TTL_SECS: i64 = 30 * 24 * 60 * 60;

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub enum TrackState {
    /// Fresh playcount available; just serve from cache.
    Fresh(i64),
    /// Map row exists + stats row exists but stats are >7 days old.
    StaleStats { spotify_track_id: String },
    /// Map row exists, no stats row OR `playcount = 0` (treat zero as null
    /// with 1-day retry per plan).
    NeedsStatsFetch { spotify_track_id: String },
    /// Negative-cached within the last 24h.
    NegativeFresh,
    /// Never seen, or negative cache has expired.
    NeedsResolution,
}

impl TrackState {
    pub fn from_row(row: &CachedTrackStatsRow, now: i64) -> Self {
        // Negative cache wins iff it's still fresh.
        if let Some(cached_at) = row.null_cached_at
            && now - cached_at < NULL_TTL_SECS
        {
            return TrackState::NegativeFresh;
        }

        match (&row.spotify_track_id, row.playcount, row.stats_fetched_at) {
            (Some(tid), Some(pc), Some(fa)) => {
                if pc == 0 && now - fa >= NULL_TTL_SECS {
                    TrackState::NeedsStatsFetch {
                        spotify_track_id: tid.clone(),
                    }
                } else if now - fa >= STATS_TTL_SECS {
                    TrackState::StaleStats {
                        spotify_track_id: tid.clone(),
                    }
                } else if pc == 0 {
                    // Zero within the 1-day window: treat as "negative for now".
                    TrackState::NegativeFresh
                } else {
                    TrackState::Fresh(pc)
                }
            }
            (Some(tid), _, _) => TrackState::NeedsStatsFetch {
                spotify_track_id: tid.clone(),
            },
            _ => TrackState::NeedsResolution,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ArtistMapState {
    Resolved(String),
    NegativeFresh,
    Missing,
}

pub fn read_track_states(
    conn: &Connection,
    isrcs: &[String],
) -> Result<HashMap<String, TrackState>> {
    let now = now_secs();
    let raw = get_cached_spotify_track_stats_for_isrcs(conn, isrcs)?;
    Ok(raw
        .into_iter()
        .map(|(isrc, row)| (isrc, TrackState::from_row(&row, now)))
        .collect())
}

pub fn read_artist_stats(
    conn: &Connection,
    spotify_artist_id: &str,
) -> Result<Option<(CachedArtistStatsRow, bool)>> {
    let now = now_secs();
    let row = get_cached_spotify_artist_stats(conn, spotify_artist_id)?;
    Ok(row.map(|r| {
        let stale = now - r.fetched_at >= STATS_TTL_SECS;
        (r, stale)
    }))
}

pub fn read_artist_map(conn: &Connection, tidal_artist_id: &str) -> Result<ArtistMapState> {
    let now = now_secs();
    match get_spotify_artist_map(conn, tidal_artist_id)? {
        None => Ok(ArtistMapState::Missing),
        Some((None, resolved_at)) => {
            if now - resolved_at < NULL_TTL_SECS {
                Ok(ArtistMapState::NegativeFresh)
            } else {
                Ok(ArtistMapState::Missing)
            }
        }
        Some((Some(spid), resolved_at)) => {
            if now - resolved_at >= ARTIST_MAP_TTL_SECS {
                // Map row very old; treat as missing so the resolver re-runs.
                Ok(ArtistMapState::Missing)
            } else {
                Ok(ArtistMapState::Resolved(spid))
            }
        }
    }
}

pub fn write_track_resolution(
    conn: &Connection,
    isrc: &str,
    spotify_track_id: &str,
) -> Result<()> {
    let now = now_secs();
    upsert_spotify_isrc_map(conn, isrc, spotify_track_id, now)?;
    clear_spotify_null_cache(conn, isrc)?;
    Ok(())
}

pub fn write_track_playcount(
    conn: &Connection,
    spotify_track_id: &str,
    playcount: i64,
) -> Result<()> {
    let now = now_secs();
    upsert_spotify_track_stats(conn, spotify_track_id, playcount, now)
}

pub fn write_track_negative(conn: &Connection, isrc: &str) -> Result<()> {
    let now = now_secs();
    upsert_spotify_null_cache(conn, isrc, now)
}

pub fn write_artist_map(
    conn: &Connection,
    tidal_artist_id: &str,
    spotify_artist_id: Option<&str>,
) -> Result<()> {
    let now = now_secs();
    upsert_spotify_artist_map(conn, tidal_artist_id, spotify_artist_id, now)
}

pub fn write_artist_stats(
    conn: &Connection,
    spotify_artist_id: &str,
    monthly_listeners: Option<i64>,
    followers: Option<i64>,
    world_rank: Option<i64>,
    top_cities_json: Option<&str>,
) -> Result<()> {
    let now = now_secs();
    upsert_spotify_artist_stats(
        conn,
        spotify_artist_id,
        monthly_listeners,
        followers,
        world_rank,
        top_cities_json,
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_playcount_returns_fresh() {
        let now = 1_000_000_000_i64;
        let row = CachedTrackStatsRow {
            spotify_track_id: Some("abc".into()),
            playcount: Some(1234),
            stats_fetched_at: Some(now - 60),
            null_cached_at: None,
        };
        assert!(matches!(TrackState::from_row(&row, now), TrackState::Fresh(1234)));
    }

    #[test]
    fn stale_stats_after_seven_days() {
        let now = 1_000_000_000_i64;
        let row = CachedTrackStatsRow {
            spotify_track_id: Some("abc".into()),
            playcount: Some(1234),
            stats_fetched_at: Some(now - STATS_TTL_SECS - 1),
            null_cached_at: None,
        };
        assert!(matches!(
            TrackState::from_row(&row, now),
            TrackState::StaleStats { .. }
        ));
    }

    #[test]
    fn negative_cache_wins_when_fresh() {
        let now = 1_000_000_000_i64;
        let row = CachedTrackStatsRow {
            spotify_track_id: None,
            playcount: None,
            stats_fetched_at: None,
            null_cached_at: Some(now - 60),
        };
        assert!(matches!(
            TrackState::from_row(&row, now),
            TrackState::NegativeFresh
        ));
    }

    #[test]
    fn zero_playcount_within_window_is_negative() {
        let now = 1_000_000_000_i64;
        let row = CachedTrackStatsRow {
            spotify_track_id: Some("abc".into()),
            playcount: Some(0),
            stats_fetched_at: Some(now - 60),
            null_cached_at: None,
        };
        assert!(matches!(
            TrackState::from_row(&row, now),
            TrackState::NegativeFresh
        ));
    }

    #[test]
    fn zero_playcount_after_one_day_retries_stats() {
        let now = 1_000_000_000_i64;
        let row = CachedTrackStatsRow {
            spotify_track_id: Some("abc".into()),
            playcount: Some(0),
            stats_fetched_at: Some(now - NULL_TTL_SECS - 1),
            null_cached_at: None,
        };
        assert!(matches!(
            TrackState::from_row(&row, now),
            TrackState::NeedsStatsFetch { .. }
        ));
    }

    #[test]
    fn missing_map_row_means_needs_resolution() {
        let now = 1_000_000_000_i64;
        let row = CachedTrackStatsRow::default();
        assert!(matches!(
            TrackState::from_row(&row, now),
            TrackState::NeedsResolution
        ));
    }
}
