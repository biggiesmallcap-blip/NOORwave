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
    // Seed track leads the queue, matching the user's explicit radio seed.
    // orchestrate_song already excludes it; this filter is a defensive guard.
    let (library_cands, pending_cands): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .filter(|c| c.track_id != seed_track_id)
        .partition(|c| c.is_in_library && c.track_id > 0);

    let tx = conn.unchecked_transaction()?;

    tx.execute("DELETE FROM queue", [])?;

    let mut pos = 0i32;
    tx.execute(
        "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'radio', NULL)",
        rusqlite::params![seed_track_id, pos],
    )?;
    pos += 1;

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
                                pending_artist, pending_title, pending_at)
             VALUES (NULL, ?1, 'radio_pending', ?2, ?3, ?4, datetime('now'))",
            rusqlite::params![pos, c.reason, c.artist_name, c.title],
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
