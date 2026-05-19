use crate::services::radio::RadioCandidate;
use rusqlite::OptionalExtension;

pub struct RadioQueueBuild {
    pub first_item: Option<(i64, Option<i64>)>,
    pub pending_item_ids: Vec<i64>,
}

pub fn build_radio_queue_from_candidates(
    conn: &rusqlite::Connection,
    seed_track_id: i64,
    candidates: Vec<RadioCandidate>,
) -> rusqlite::Result<RadioQueueBuild> {
    build_radio_queue_from_candidates_with_seed(conn, Some(seed_track_id), candidates)
}

pub fn build_radio_queue_from_candidates_with_seed(
    conn: &rusqlite::Connection,
    seed_track_id: Option<i64>,
    candidates: Vec<RadioCandidate>,
) -> rusqlite::Result<RadioQueueBuild> {
    // Seed track leads the queue, matching the user's explicit radio seed.
    // orchestrate_song already excludes it; this filter is a defensive guard.
    let (library_cands, pending_cands): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .filter(|c| seed_track_id != Some(c.track_id))
        .partition(|c| c.is_in_library && c.track_id > 0);

    let tx = conn.unchecked_transaction()?;

    tx.execute("DELETE FROM queue", [])?;

    let mut pos = 0i32;
    if let Some(seed_track_id) = seed_track_id {
        tx.execute(
            "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'radio', NULL)",
            rusqlite::params![seed_track_id, pos],
        )?;
        pos += 1;
    }

    for c in &library_cands {
        tx.execute(
            "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'radio', ?3)",
            rusqlite::params![c.track_id, pos, c.reason],
        )?;
        pos += 1;
    }

    for c in &pending_cands {
        tx.execute(
            "INSERT INTO queue (track_id, position, source, reason,
                                pending_artist, pending_title, pending_at, tidal_id_hint)
             VALUES (NULL, ?1, 'radio_pending', ?2, ?3, ?4, datetime('now'), ?5)",
            rusqlite::params![pos, c.reason, c.artist_name, c.title, c.tidal_track_id],
        )?;
        pos += 1;
    }

    tx.commit()?;

    let first_item: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, track_id FROM queue ORDER BY position ASC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let pending_item_ids: Vec<i64> = conn
        .prepare("SELECT id FROM queue WHERE track_id IS NULL AND pending_at IS NOT NULL")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(RadioQueueBuild {
        first_item,
        pending_item_ids,
    })
}

pub fn append_radio_queue_from_candidates(
    conn: &rusqlite::Connection,
    candidates: Vec<RadioCandidate>,
) -> rusqlite::Result<RadioQueueBuild> {
    let (library_cands, pending_cands): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|c| c.is_in_library && c.track_id > 0);

    let tx = conn.unchecked_transaction()?;
    let mut pos: i32 = tx.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM queue",
        [],
        |row| row.get(0),
    )?;

    for c in &library_cands {
        tx.execute(
            "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'radio', ?3)",
            rusqlite::params![c.track_id, pos, c.reason],
        )?;
        pos += 1;
    }

    let mut pending_item_ids = Vec::new();
    for c in &pending_cands {
        tx.execute(
            "INSERT INTO queue (track_id, position, source, reason,
                                pending_artist, pending_title, pending_at, tidal_id_hint)
             VALUES (NULL, ?1, 'radio_pending', ?2, ?3, ?4, datetime('now'), ?5)",
            rusqlite::params![pos, c.reason, c.artist_name, c.title, c.tidal_track_id],
        )?;
        pending_item_ids.push(tx.last_insert_rowid());
        pos += 1;
    }

    tx.commit()?;

    let first_item: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, track_id FROM queue ORDER BY position ASC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    Ok(RadioQueueBuild {
        first_item,
        pending_item_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with_queue() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                track_id INTEGER,
                position INTEGER NOT NULL,
                source TEXT,
                reason TEXT,
                pending_artist TEXT,
                pending_title TEXT,
                pending_at TEXT,
                tidal_id_hint INTEGER
            );
            ",
        )
        .unwrap();
        conn
    }

    fn candidate(
        track_id: i64,
        is_in_library: bool,
        artist_name: &str,
        title: &str,
    ) -> RadioCandidate {
        RadioCandidate {
            track_id,
            tidal_track_id: None,
            title: title.to_string(),
            artist_name: artist_name.to_string(),
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            isrc: None,
            is_in_library,
            source: crate::services::radio::RadioSource::Lastfm,
            reason: "test candidate".to_string(),
            similarity_score: 0.8,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        }
    }

    fn tidal_candidate(tidal_track_id: i64, artist_name: &str, title: &str) -> RadioCandidate {
        RadioCandidate {
            tidal_track_id: Some(tidal_track_id),
            ..candidate(0, false, artist_name, title)
        }
    }

    #[test]
    fn optional_seed_builds_queue_from_candidates_without_seed_row() {
        let conn = conn_with_queue();

        let build = build_radio_queue_from_candidates_with_seed(
            &conn,
            None,
            vec![
                candidate(10, true, "Library Artist", "Library Track"),
                candidate(0, false, "Pending Artist", "Pending Track"),
            ],
        )
        .unwrap();

        assert_eq!(build.first_item, Some((1, Some(10))));
        assert_eq!(build.pending_item_ids, vec![2]);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn pending_external_candidate_preserves_tidal_hint() {
        let conn = conn_with_queue();

        build_radio_queue_from_candidates_with_seed(
            &conn,
            None,
            vec![tidal_candidate(9001, "External Artist", "External Track")],
        )
        .unwrap();

        let hint: Option<i64> = conn
            .query_row(
                "SELECT tidal_id_hint FROM queue WHERE position = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hint, Some(9001));
    }

    #[test]
    fn append_candidates_preserves_existing_queue_rows() {
        let conn = conn_with_queue();
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (42, 0, 'user_queue')",
            [],
        )
        .unwrap();

        let build = append_radio_queue_from_candidates(
            &conn,
            vec![tidal_candidate(9002, "External Artist", "External Track")],
        )
        .unwrap();

        assert_eq!(build.first_item, Some((1, Some(42))));
        assert_eq!(build.pending_item_ids, vec![2]);
        let rows: Vec<(i32, Option<i64>, Option<i64>)> = conn
            .prepare("SELECT position, track_id, tidal_id_hint FROM queue ORDER BY position ASC")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows, vec![(0, Some(42), None), (1, None, Some(9002))]);
    }

    #[test]
    fn replace_candidates_clears_existing_queue_rows() {
        let conn = conn_with_queue();
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (42, 0, 'user_queue')",
            [],
        )
        .unwrap();

        let build = build_radio_queue_from_candidates_with_seed(
            &conn,
            None,
            vec![tidal_candidate(9003, "External Artist", "External Track")],
        )
        .unwrap();

        assert_eq!(build.first_item, Some((2, None)));
        assert_eq!(build.pending_item_ids, vec![2]);
        let rows: Vec<(i32, Option<i64>, Option<i64>)> = conn
            .prepare("SELECT position, track_id, tidal_id_hint FROM queue ORDER BY position ASC")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows, vec![(0, None, Some(9003))]);
    }
}
