//! One ledger row per TIDAL artist: what the crawler knows about its video
//! catalog and when to look again.

use std::collections::HashMap;

use anyhow::Result;
use chrono::Duration;
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::names::name_key;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistState {
    pub artist_tidal_id: i64,
    pub name: String,
    pub popularity: Option<i32>,
    pub mix_id: Option<String>,
    pub seen_main: bool,
    pub seen_featured: bool,
    /// Items received from the artist's own video pages (highest offset reached).
    pub fetched_count: i64,
    /// TIDAL's `totalNumberOfItems` for the artist's video list.
    pub total_videos: Option<i64>,
    /// Days since the last catalog check; `None` when never checked.
    pub checked_age_days: Option<f64>,
    pub check_due: bool,
    pub empty_streak: i64,
    pub expand_due: bool,
    pub mix_age_days: Option<f64>,
}

impl ArtistState {
    pub fn has_videos(&self) -> bool {
        self.seen_main || self.fetched_count > 0
    }

    pub fn never_checked(&self) -> bool {
        self.checked_age_days.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckResult {
    Found { new_videos: i64 },
    Empty,
    Failed,
}

/// Non-priority cadence. Priority artists (liked, enjoyed, on air) are
/// re-checked weekly by the scheduler regardless of this date.
pub fn next_check_delay(result: CheckResult, streak: i64) -> Duration {
    match result {
        CheckResult::Found { .. } => match streak {
            s if s <= 0 => Duration::days(30),
            1 => Duration::days(60),
            _ => Duration::days(120),
        },
        CheckResult::Empty => match streak {
            s if s <= 1 => Duration::days(60),
            2 => Duration::days(120),
            _ => Duration::days(240),
        },
        CheckResult::Failed => failure_delay(streak),
    }
}

pub fn failure_delay(streak: i64) -> Duration {
    match streak {
        s if s <= 1 => Duration::hours(1),
        2 => Duration::hours(6),
        _ => Duration::hours(24),
    }
}

pub fn expand_delay(ok: bool, station: bool, fail_streak: i64) -> Duration {
    match (ok, station) {
        (true, true) => Duration::days(7),
        (true, false) => Duration::days(30),
        (false, _) => failure_delay(fail_streak),
    }
}

const SELECT_COLUMNS: &str = "artist_tidal_id, name, popularity, mix_id, seen_main,
    seen_featured, fetched_count, total_videos,
    julianday('now') - julianday(last_checked_at),
    next_check_at IS NULL OR next_check_at <= datetime('now'),
    empty_streak,
    next_expand_at IS NULL OR next_expand_at <= datetime('now'),
    julianday('now') - julianday(mix_checked_at)";

fn from_row(row: &Row) -> rusqlite::Result<ArtistState> {
    Ok(ArtistState {
        artist_tidal_id: row.get(0)?,
        name: row.get(1)?,
        popularity: row.get(2)?,
        mix_id: row.get(3)?,
        seen_main: row.get(4)?,
        seen_featured: row.get(5)?,
        fetched_count: row.get(6)?,
        total_videos: row.get(7)?,
        checked_age_days: row.get(8)?,
        check_due: row.get(9)?,
        empty_streak: row.get(10)?,
        expand_due: row.get(11)?,
        mix_age_days: row.get(12)?,
    })
}

pub fn get(conn: &Connection, artist_id: i64) -> Result<Option<ArtistState>> {
    Ok(conn
        .query_row(
            &format!("SELECT {SELECT_COLUMNS} FROM video_artist_state WHERE artist_tidal_id = ?1"),
            [artist_id],
            from_row,
        )
        .optional()?)
}

pub fn load_all(conn: &Connection) -> Result<HashMap<i64, ArtistState>> {
    let mut stmt = conn.prepare(&format!("SELECT {SELECT_COLUMNS} FROM video_artist_state"))?;
    let rows = stmt.query_map([], from_row)?;
    let mut out = HashMap::new();
    for row in rows {
        let state = row?;
        out.insert(state.artist_tidal_id, state);
    }
    Ok(out)
}

fn ensure(conn: &Connection, artist_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO video_artist_state (artist_tidal_id) VALUES (?1)",
        [artist_id],
    )?;
    Ok(())
}

/// Name, popularity and artist-mix id from any artist object TIDAL returns.
/// Missing facts never erase known ones.
pub fn upsert_identity(
    conn: &Connection,
    artist_id: i64,
    name: &str,
    popularity: Option<i32>,
    mix_id: Option<&str>,
) -> Result<()> {
    if artist_id <= 0 {
        return Ok(());
    }
    let name = name.trim();
    conn.execute(
        "INSERT INTO video_artist_state (artist_tidal_id, name, name_key, popularity, mix_id)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(artist_tidal_id) DO UPDATE SET
           name = CASE WHEN excluded.name <> '' THEN excluded.name ELSE video_artist_state.name END,
           name_key = CASE WHEN excluded.name_key <> '' THEN excluded.name_key
                           ELSE video_artist_state.name_key END,
           popularity = COALESCE(excluded.popularity, video_artist_state.popularity),
           mix_id = COALESCE(excluded.mix_id, video_artist_state.mix_id)",
        params![artist_id, name, name_key(name), popularity, mix_id],
    )?;
    Ok(())
}

/// The artist appeared on a harvested video, as its main or a featured artist.
pub fn mark_seen(conn: &Connection, artist_id: i64, name: &str, featured: bool) -> Result<()> {
    if artist_id <= 0 {
        return Ok(());
    }
    upsert_identity(conn, artist_id, name, None, None)?;
    let sql = if featured {
        "UPDATE video_artist_state SET seen_featured = 1 WHERE artist_tidal_id = ?1"
    } else {
        "UPDATE video_artist_state SET seen_main = 1 WHERE artist_tidal_id = ?1"
    };
    conn.execute(sql, [artist_id])?;
    Ok(())
}

pub fn record_check(conn: &Connection, artist_id: i64, result: CheckResult) -> Result<()> {
    ensure(conn, artist_id)?;
    let (checked_before, empty, nothing_new, fail): (bool, i64, i64, i64) = conn.query_row(
        "SELECT last_checked_at IS NOT NULL, empty_streak, nothing_new_streak, fail_streak
           FROM video_artist_state WHERE artist_tidal_id = ?1",
        [artist_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let (empty, nothing_new, fail, delay) = match result {
        CheckResult::Found { new_videos } => {
            let streak = if new_videos > 0 || !checked_before {
                0
            } else {
                nothing_new + 1
            };
            (0, streak, 0, next_check_delay(result, streak))
        }
        CheckResult::Empty => (empty + 1, 0, 0, next_check_delay(result, empty + 1)),
        CheckResult::Failed => (
            empty,
            nothing_new,
            fail + 1,
            next_check_delay(result, fail + 1),
        ),
    };
    conn.execute(
        "UPDATE video_artist_state SET empty_streak = ?2, nothing_new_streak = ?3, fail_streak = ?4,
             last_checked_at = CASE WHEN ?5 THEN datetime('now') ELSE last_checked_at END,
             next_check_at = datetime('now', ?6)
         WHERE artist_tidal_id = ?1",
        params![
            artist_id,
            empty,
            nothing_new,
            fail,
            !matches!(result, CheckResult::Failed),
            format!("+{} seconds", delay.num_seconds()),
        ],
    )?;
    Ok(())
}

/// One page of the artist's own video list arrived. `fetched_count` keeps the
/// deepest offset reached so a later page-one recheck never rewinds paging.
pub fn record_page(
    conn: &Connection,
    artist_id: i64,
    offset: i64,
    received: i64,
    total: Option<i64>,
) -> Result<()> {
    ensure(conn, artist_id)?;
    conn.execute(
        "UPDATE video_artist_state
            SET fetched_count = MAX(fetched_count, ?2),
                total_videos = COALESCE(?3, total_videos)
          WHERE artist_tidal_id = ?1",
        params![artist_id, offset + received, total],
    )?;
    Ok(())
}

pub fn record_expand(conn: &Connection, artist_id: i64, ok: bool, station: bool) -> Result<()> {
    ensure(conn, artist_id)?;
    let fail: i64 = conn.query_row(
        "SELECT expand_fail_streak FROM video_artist_state WHERE artist_tidal_id = ?1",
        [artist_id],
        |row| row.get(0),
    )?;
    let fail = if ok { 0 } else { fail + 1 };
    let delay = expand_delay(ok, station, fail);
    conn.execute(
        "UPDATE video_artist_state SET expand_fail_streak = ?2,
             last_expanded_at = CASE WHEN ?3 THEN datetime('now') ELSE last_expanded_at END,
             next_expand_at = datetime('now', ?4)
         WHERE artist_tidal_id = ?1",
        params![
            artist_id,
            fail,
            ok,
            format!("+{} seconds", delay.num_seconds())
        ],
    )?;
    Ok(())
}

pub fn record_mix_check(conn: &Connection, artist_id: i64) -> Result<()> {
    ensure(conn, artist_id)?;
    conn.execute(
        "UPDATE video_artist_state SET mix_checked_at = datetime('now') WHERE artist_tidal_id = ?1",
        [artist_id],
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

    fn days_until_next_check(conn: &Connection, id: i64) -> f64 {
        conn.query_row(
            "SELECT julianday(next_check_at) - julianday('now') FROM video_artist_state
              WHERE artist_tidal_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn backoff_follows_the_spec_table() {
        let found = CheckResult::Found { new_videos: 0 };
        assert_eq!(next_check_delay(found, 0), Duration::days(30));
        assert_eq!(next_check_delay(found, 1), Duration::days(60));
        assert_eq!(next_check_delay(found, 4), Duration::days(120));
        assert_eq!(next_check_delay(CheckResult::Empty, 1), Duration::days(60));
        assert_eq!(next_check_delay(CheckResult::Empty, 2), Duration::days(120));
        assert_eq!(next_check_delay(CheckResult::Empty, 3), Duration::days(240));
        assert_eq!(failure_delay(1), Duration::hours(1));
        assert_eq!(failure_delay(2), Duration::hours(6));
        assert_eq!(failure_delay(9), Duration::hours(24));
        assert_eq!(expand_delay(true, true, 0), Duration::days(7));
        assert_eq!(expand_delay(true, false, 0), Duration::days(30));
        assert_eq!(expand_delay(false, false, 2), Duration::hours(6));
    }

    #[test]
    fn empty_artists_back_off_and_a_find_resets_them() {
        let conn = conn();
        for expected in [60.0, 120.0, 240.0] {
            record_check(&conn, 5, CheckResult::Empty).unwrap();
            assert!((days_until_next_check(&conn, 5) - expected).abs() < 0.01);
        }
        record_check(&conn, 5, CheckResult::Found { new_videos: 3 }).unwrap();
        assert!((days_until_next_check(&conn, 5) - 30.0).abs() < 0.01);
        assert_eq!(get(&conn, 5).unwrap().unwrap().empty_streak, 0);
        record_check(&conn, 5, CheckResult::Found { new_videos: 0 }).unwrap();
        assert!((days_until_next_check(&conn, 5) - 60.0).abs() < 0.01);
    }

    #[test]
    fn a_first_check_with_only_known_videos_is_not_a_stale_streak() {
        let conn = conn();
        record_check(&conn, 6, CheckResult::Found { new_videos: 0 }).unwrap();
        assert!((days_until_next_check(&conn, 6) - 30.0).abs() < 0.01);
    }

    #[test]
    fn failures_retry_soon_and_do_not_count_as_a_check() {
        let conn = conn();
        record_check(&conn, 7, CheckResult::Failed).unwrap();
        let state = get(&conn, 7).unwrap().unwrap();
        assert!(state.never_checked());
        assert!((days_until_next_check(&conn, 7) * 24.0 - 1.0).abs() < 0.05);
        assert!(!state.check_due);
    }

    #[test]
    fn identity_updates_never_erase_known_facts() {
        let conn = conn();
        upsert_identity(&conn, 8, "Artist", Some(71), Some("mix8")).unwrap();
        upsert_identity(&conn, 8, "", None, None).unwrap();
        let state = get(&conn, 8).unwrap().unwrap();
        assert_eq!(
            (
                state.name.as_str(),
                state.popularity,
                state.mix_id.as_deref()
            ),
            ("Artist", Some(71), Some("mix8"))
        );
    }

    #[test]
    fn paging_never_rewinds_and_keeps_tidal_totals() {
        let conn = conn();
        record_page(&conn, 9, 0, 50, Some(120)).unwrap();
        record_page(&conn, 9, 50, 50, Some(120)).unwrap();
        record_page(&conn, 9, 0, 50, None).unwrap();
        let state = get(&conn, 9).unwrap().unwrap();
        assert_eq!((state.fetched_count, state.total_videos), (100, Some(120)));
        assert!(state.has_videos());
    }

    #[test]
    fn expansion_schedules_by_outcome() {
        let conn = conn();
        record_expand(&conn, 11, true, true).unwrap();
        assert!(!get(&conn, 11).unwrap().unwrap().expand_due);
        let days: f64 = conn
            .query_row(
                "SELECT julianday(next_expand_at) - julianday('now') FROM video_artist_state WHERE artist_tidal_id = 11",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((days - 7.0).abs() < 0.01);
        assert!(get(&conn, 12).unwrap().is_none());
    }
}
