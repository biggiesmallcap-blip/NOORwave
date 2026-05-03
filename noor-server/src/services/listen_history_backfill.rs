// One-shot backfill that populates session_id, source, position_in_session, and
// transition_from_track_id on historical listen_history rows that pre-date the
// schema additions in MIGRATION_023.
//
// Strategy: walk rows ordered by started_at ASC. Consecutive rows with gap
// < SESSION_GAP_MINUTES belong to the same synthetic session (matching the
// runtime session-continuation rule in player::ActiveListenSession::start).
// transition_from_track_id is the immediately-prior row in the same session;
// the first row of each session gets NULL. source is always Unknown — we
// can't reconstruct provenance after the fact, and the trainer downweights
// Unknown-source-supported edges via ListenSource::confidence_multiplier.
//
// Idempotent: skips rows that already have session_id, and writes a marker to
// server_config so it doesn't auto-re-run. Manual re-runs via the bin entry
// always work because the marker is only checked by run_if_needed.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use uuid::Uuid;

const SESSION_GAP_MINUTES: i64 = 30;
const BACKFILL_DONE_KEY: &str = "listen_history_backfill_done";

struct Row {
    id: i64,
    track_id: i64,
    started_at: DateTime<Utc>,
}

pub struct BackfillReport {
    pub rows_scanned: usize,
    pub rows_updated: usize,
    pub sessions_created: usize,
    pub already_populated: usize,
}

pub fn backfill_listen_history(conn: &Connection) -> Result<BackfillReport> {
    let rows = load_unbackfilled_rows(conn)?;
    let already_populated: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM listen_history WHERE session_id IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize;

    if rows.is_empty() {
        return Ok(BackfillReport {
            rows_scanned: 0,
            rows_updated: 0,
            sessions_created: 0,
            already_populated,
        });
    }

    let tx = conn.unchecked_transaction()?;
    let mut stmt = tx.prepare(
        "UPDATE listen_history
         SET session_id = ?1,
             source = ?2,
             position_in_session = ?3,
             transition_from_track_id = ?4
         WHERE id = ?5",
    )?;

    let mut sessions_created = 0usize;
    let mut rows_updated = 0usize;
    let mut current_session: Option<String> = None;
    let mut current_position: i32 = 0;
    let mut prior_row: Option<&Row> = None;

    for row in &rows {
        let continues = match prior_row {
            Some(prev) => (row.started_at - prev.started_at).num_minutes() < SESSION_GAP_MINUTES,
            None => false,
        };

        let (session_id, position, transition_from) = if continues {
            current_position += 1;
            (
                current_session.clone().expect("session set when continues"),
                current_position,
                prior_row.map(|p| p.track_id),
            )
        } else {
            let id = Uuid::new_v4().to_string();
            current_session = Some(id.clone());
            current_position = 0;
            sessions_created += 1;
            (id, 0, None)
        };

        stmt.execute(params![
            session_id,
            "unknown",
            position,
            transition_from,
            row.id,
        ])?;
        rows_updated += 1;
        prior_row = Some(row);
    }

    drop(stmt);
    tx.commit()?;

    Ok(BackfillReport {
        rows_scanned: rows.len(),
        rows_updated,
        sessions_created,
        already_populated,
    })
}

// Trainer-triggered entry point: runs the backfill once, sets a marker so
// subsequent training runs skip it. Returns Ok(None) if already done, Ok(Some)
// otherwise. Errors propagate so a transient DB issue retries on next call.
pub fn run_if_needed(conn: &Connection) -> Result<Option<BackfillReport>> {
    let already_done: bool = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            params![BACKFILL_DONE_KEY],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);

    if already_done {
        return Ok(None);
    }

    let report = backfill_listen_history(conn)?;
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, '1')",
        params![BACKFILL_DONE_KEY],
    )?;
    Ok(Some(report))
}

fn load_unbackfilled_rows(conn: &Connection) -> Result<Vec<Row>> {
    let mut stmt = conn.prepare(
        "SELECT id, track_id, started_at
         FROM listen_history
         WHERE session_id IS NULL AND started_at IS NOT NULL
         ORDER BY started_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let started_at_raw: String = row.get(2)?;
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, started_at_raw))
        })?
        .filter_map(|res| {
            let (id, track_id, raw) = res.ok()?;
            let parsed = parse_started_at(&raw)?;
            Some(Row {
                id,
                track_id,
                started_at: parsed,
            })
        })
        .collect::<Vec<_>>();
    Ok(rows)
}

// listen_history.started_at can be either RFC3339 (live writes) or
// SQLite's datetime('now') format ("YYYY-MM-DD HH:MM:SS"). Parse both;
// drop rows we can't parse.
fn parse_started_at(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn open_in_memory() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn
    }

    fn insert_track(conn: &Connection, id: i64) {
        conn.execute("INSERT INTO artists (id, name) VALUES (?1, 'A')", params![id]).unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, source) VALUES (?1, 't', ?1, 'tidal')",
            params![id],
        )
        .unwrap();
    }

    fn insert_listen(conn: &Connection, track_id: i64, started_at: &str) {
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
             VALUES (?1, ?2, 60000, 1)",
            params![track_id, started_at],
        )
        .unwrap();
    }

    #[test]
    fn groups_within_30_minutes_into_one_session() {
        let conn = open_in_memory();
        for id in 1..=3 { insert_track(&conn, id); }
        insert_listen(&conn, 1, "2026-01-01T10:00:00Z");
        insert_listen(&conn, 2, "2026-01-01T10:05:00Z");
        insert_listen(&conn, 3, "2026-01-01T10:25:00Z");

        let report = backfill_listen_history(&conn).expect("backfill ok");
        assert_eq!(report.rows_updated, 3);
        assert_eq!(report.sessions_created, 1);

        let session_ids: Vec<String> = conn
            .prepare("SELECT session_id FROM listen_history ORDER BY started_at")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(session_ids.len(), 3);
        assert_eq!(session_ids[0], session_ids[1]);
        assert_eq!(session_ids[1], session_ids[2]);
    }

    #[test]
    fn splits_on_long_gap() {
        let conn = open_in_memory();
        for id in 1..=2 { insert_track(&conn, id); }
        insert_listen(&conn, 1, "2026-01-01T10:00:00Z");
        insert_listen(&conn, 2, "2026-01-01T11:00:00Z");

        let report = backfill_listen_history(&conn).expect("backfill ok");
        assert_eq!(report.sessions_created, 2);
    }

    #[test]
    fn position_increments_within_session() {
        let conn = open_in_memory();
        for id in 1..=3 { insert_track(&conn, id); }
        insert_listen(&conn, 1, "2026-01-01T10:00:00Z");
        insert_listen(&conn, 2, "2026-01-01T10:05:00Z");
        insert_listen(&conn, 3, "2026-01-01T10:10:00Z");
        backfill_listen_history(&conn).expect("backfill ok");

        let positions: Vec<i32> = conn
            .prepare("SELECT position_in_session FROM listen_history ORDER BY started_at")
            .unwrap()
            .query_map([], |r| r.get::<_, i32>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[test]
    fn transition_from_is_prior_track_in_session() {
        let conn = open_in_memory();
        for id in [7, 11, 13] { insert_track(&conn, id); }
        insert_listen(&conn, 7, "2026-01-01T10:00:00Z");
        insert_listen(&conn, 11, "2026-01-01T10:05:00Z");
        insert_listen(&conn, 13, "2026-01-01T10:10:00Z");
        backfill_listen_history(&conn).expect("backfill ok");

        let transitions: Vec<Option<i64>> = conn
            .prepare("SELECT transition_from_track_id FROM listen_history ORDER BY started_at")
            .unwrap()
            .query_map([], |r| r.get::<_, Option<i64>>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(transitions, vec![None, Some(7), Some(11)]);
    }

    #[test]
    fn idempotent_skips_already_populated_rows() {
        let conn = open_in_memory();
        insert_track(&conn, 1);
        insert_listen(&conn, 1, "2026-01-01T10:00:00Z");
        let first = backfill_listen_history(&conn).expect("first backfill");
        assert_eq!(first.rows_updated, 1);

        let second = backfill_listen_history(&conn).expect("second backfill");
        assert_eq!(second.rows_updated, 0);
    }

    #[test]
    fn run_if_needed_writes_marker() {
        let conn = open_in_memory();
        insert_track(&conn, 1);
        insert_listen(&conn, 1, "2026-01-01T10:00:00Z");

        let first = run_if_needed(&conn).expect("first run").expect("ran");
        assert_eq!(first.rows_updated, 1);

        let second = run_if_needed(&conn).expect("second run");
        assert!(second.is_none(), "marker should suppress second run");
    }

    #[test]
    fn source_is_unknown_for_backfilled_rows() {
        let conn = open_in_memory();
        insert_track(&conn, 1);
        insert_listen(&conn, 1, "2026-01-01T10:00:00Z");
        backfill_listen_history(&conn).expect("backfill ok");
        let source: String = conn
            .query_row("SELECT source FROM listen_history LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(source, "unknown");
    }
}
