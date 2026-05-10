//! Writeback for world playcount + monthly-listener stats lifted from
//! Sportify responses into the existing `spotify_track_stats` /
//! `spotify_artist_stats` tables (`MIGRATION_029`).
//!
//! Why these tables and not new `sportify_*_stats`: the existing artist page
//! already reads `spotify_artist_stats` via
//! `/api/artists/{id}/spotify-stats`, so funnelling Sportify-sourced numbers
//! through the same table means stats appear automatically on the legacy
//! library page with zero frontend wiring. Tracks resolve to TIDAL anyway
//! (the resolver writes `spotify_track_id → tidal_track_id` mappings), so a
//! library track's `spotify_track_id` lookup → `spotify_track_stats` lookup
//! gives us "1.2B plays" labels for free.
//!
//! Failure policy: every writeback is best-effort. Sportify is upstream and
//! subject to breakage; a missing `playcount` or a malformed value must
//! never bubble up an error that fails the surrounding request. Each helper
//! takes a connection and returns `()` — write what we can, log what we
//! can't, move on.

use rusqlite::{Connection, params};
use std::time::{SystemTime, UNIX_EPOCH};

use super::models::{SportifyArtist, SportifyTrack};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// If the Sportify track payload carries a `playcount`, upsert it into
/// `spotify_track_stats`. No-op when playcount is missing, zero, or negative.
pub fn write_track_playcount(conn: &Connection, track: &SportifyTrack) {
    let Some(spotify_id) = track.id.as_deref() else {
        return;
    };
    let Some(playcount) = track.playcount.filter(|c| *c > 0) else {
        return;
    };
    if let Err(e) = conn.execute(
        "INSERT INTO spotify_track_stats (spotify_track_id, playcount, fetched_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(spotify_track_id) DO UPDATE SET
            playcount = excluded.playcount,
            fetched_at = excluded.fetched_at",
        params![spotify_id, playcount, now_secs()],
    ) {
        tracing::warn!(
            "spotify_track_stats writeback for {} failed: {}",
            spotify_id,
            e
        );
    }

    // ISRC mapping is permanent (Spotify track IDs don't change), so opportunistically
    // populate the index when Sportify gives it to us. Cheap to keep; lets the
    // existing Spotify-stats path reuse the same lookup.
    if let Some(isrc) = track
        .external_ids
        .as_ref()
        .and_then(|e| e.isrc.as_deref())
        .filter(|s| !s.is_empty())
        && let Err(e) = conn.execute(
            "INSERT INTO spotify_isrc_map (isrc, spotify_track_id, resolved_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(isrc) DO UPDATE SET
                spotify_track_id = excluded.spotify_track_id,
                resolved_at = excluded.resolved_at",
            params![isrc, spotify_id, now_secs()],
        )
    {
        tracing::warn!("spotify_isrc_map writeback for {} failed: {}", isrc, e);
    }
}

/// If the Sportify artist payload carries `monthly_listeners`, upsert it
/// into `spotify_artist_stats`. No-op on missing/zero/negative values.
pub fn write_artist_monthly_listeners(conn: &Connection, artist: &SportifyArtist) {
    let Some(spotify_id) = artist.id.as_deref() else {
        return;
    };
    let Some(monthly) = artist.monthly_listeners.filter(|c| *c > 0) else {
        return;
    };
    if let Err(e) = conn.execute(
        "INSERT INTO spotify_artist_stats (spotify_artist_id, monthly_listeners, fetched_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(spotify_artist_id) DO UPDATE SET
            monthly_listeners = excluded.monthly_listeners,
            fetched_at = excluded.fetched_at",
        params![spotify_id, monthly, now_secs()],
    ) {
        tracing::warn!(
            "spotify_artist_stats writeback for {} failed: {}",
            spotify_id,
            e
        );
    }
}

/// Bulk variant — write every track in a batch, ignoring individual failures.
pub fn write_track_playcounts(conn: &Connection, tracks: &[SportifyTrack]) {
    for t in tracks {
        write_track_playcount(conn, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::run_migrations;
    use crate::services::sportify::models::{SportifyExternalIds, SportifyTrack};

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn writes_playcount_when_present() {
        let conn = fresh_db();
        let track = SportifyTrack {
            id: Some("abc".into()),
            playcount: Some(1_234_567),
            ..Default::default()
        };
        write_track_playcount(&conn, &track);
        let count: i64 = conn
            .query_row(
                "SELECT playcount FROM spotify_track_stats WHERE spotify_track_id = 'abc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1_234_567);
    }

    #[test]
    fn skips_playcount_when_missing_or_zero() {
        let conn = fresh_db();
        let no_count = SportifyTrack {
            id: Some("abc".into()),
            playcount: None,
            ..Default::default()
        };
        let zero = SportifyTrack {
            id: Some("def".into()),
            playcount: Some(0),
            ..Default::default()
        };
        write_track_playcount(&conn, &no_count);
        write_track_playcount(&conn, &zero);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM spotify_track_stats", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn writes_isrc_map_when_present() {
        let conn = fresh_db();
        let track = SportifyTrack {
            id: Some("abc".into()),
            playcount: Some(100),
            external_ids: Some(SportifyExternalIds {
                isrc: Some("USRC17607839".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        write_track_playcount(&conn, &track);
        let mapped: String = conn
            .query_row(
                "SELECT spotify_track_id FROM spotify_isrc_map WHERE isrc = 'USRC17607839'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mapped, "abc");
    }

    #[test]
    fn writes_monthly_listeners_when_present() {
        let conn = fresh_db();
        let artist = SportifyArtist {
            id: Some("artistX".into()),
            monthly_listeners: Some(47_000_000),
            ..Default::default()
        };
        write_artist_monthly_listeners(&conn, &artist);
        let n: i64 = conn
            .query_row(
                "SELECT monthly_listeners FROM spotify_artist_stats WHERE spotify_artist_id = 'artistX'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n, 47_000_000);
    }

    #[test]
    fn upsert_replaces_stale_value() {
        let conn = fresh_db();
        let t1 = SportifyTrack {
            id: Some("abc".into()),
            playcount: Some(100),
            ..Default::default()
        };
        let t2 = SportifyTrack {
            id: Some("abc".into()),
            playcount: Some(200),
            ..Default::default()
        };
        write_track_playcount(&conn, &t1);
        write_track_playcount(&conn, &t2);
        let count: i64 = conn
            .query_row(
                "SELECT playcount FROM spotify_track_stats WHERE spotify_track_id = 'abc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 200);
    }
}
