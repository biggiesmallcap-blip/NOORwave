use crate::playback::dj_queue_ranker::{
    GeneratedCandidate, append_dj_reason, rank_generated_candidates,
};
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
    let candidates = candidates
        .into_iter()
        .filter(|c| seed_track_id != Some(c.track_id))
        .collect::<Vec<_>>();
    let candidates = rank_radio_candidates(conn, seed_track_id, candidates);

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

    for c in &candidates {
        insert_radio_candidate(&tx, c, pos)?;
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

/// One row of an explicitly-ordered mixed queue: a library track (plays from
/// the local library) or an unresolved external track (pending row, resolved
/// lazily by tidal id / artist+title at play time). The display metadata is
/// stored on the pending row so the queue renders artwork/album/duration
/// immediately, before the resolver imports a library track.
pub struct OrderedQueueCandidate {
    pub track_id: Option<i64>,
    pub tidal_id: Option<i64>,
    pub artist: String,
    pub title: String,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub artist_tidal_id: Option<i64>,
    pub album_tidal_id: Option<i64>,
}

/// Replace the queue with an explicitly-ordered mixed list. Unlike the radio
/// builders above, candidates are NOT re-ranked: the caller's order IS the
/// queue order (e.g. album track order). Library rows insert as source 'user'
/// (matching replace_queue_with_tracks); unresolved rows reuse 'radio_pending'
/// so the existing pending resolve/skip machinery applies unchanged - if TIDAL
/// is unavailable a pending row is skipped instead of stalling the queue.
pub fn replace_queue_with_ordered_candidates(
    conn: &rusqlite::Connection,
    candidates: &[OrderedQueueCandidate],
) -> rusqlite::Result<RadioQueueBuild> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM queue", [])?;

    let mut pending_item_ids = Vec::new();
    for (pos, c) in candidates.iter().enumerate() {
        let pos = pos as i32;
        match c.track_id.filter(|id| *id > 0) {
            Some(track_id) => {
                tx.execute(
                    "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'user', NULL)",
                    rusqlite::params![track_id, pos],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO queue (track_id, position, source, reason,
                                        pending_artist, pending_title, pending_at, tidal_id_hint,
                                        ephemeral_album_title, ephemeral_artwork_url,
                                        ephemeral_duration_ms, ephemeral_artist_tidal_id,
                                        ephemeral_album_tidal_id)
                     VALUES (NULL, ?1, 'radio_pending', NULL, ?2, ?3, datetime('now'), ?4,
                             ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        pos,
                        c.artist,
                        c.title,
                        c.tidal_id,
                        c.album_title,
                        c.artwork_url,
                        c.duration_ms,
                        c.artist_tidal_id,
                        c.album_tidal_id
                    ],
                )?;
                pending_item_ids.push(tx.last_insert_rowid());
            }
        }
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

pub fn append_radio_queue_from_candidates(
    conn: &rusqlite::Connection,
    candidates: Vec<RadioCandidate>,
) -> rusqlite::Result<RadioQueueBuild> {
    let candidates = rank_radio_candidates(conn, append_seed_track_id(conn), candidates);

    let tx = conn.unchecked_transaction()?;
    let mut pos: i32 = tx.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM queue",
        [],
        |row| row.get(0),
    )?;

    let mut pending_item_ids = Vec::new();
    for c in &candidates {
        if insert_radio_candidate(&tx, c, pos)? {
            pending_item_ids.push(tx.last_insert_rowid());
        }
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

fn insert_radio_candidate(
    conn: &rusqlite::Connection,
    candidate: &RadioCandidate,
    position: i32,
) -> rusqlite::Result<bool> {
    if candidate.is_in_library && candidate.track_id > 0 {
        conn.execute(
            "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'radio', ?3)",
            rusqlite::params![candidate.track_id, position, candidate.reason],
        )?;
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO queue (track_id, position, source, reason,
                            pending_artist, pending_title, pending_at, tidal_id_hint)
         VALUES (NULL, ?1, 'radio_pending', ?2, ?3, ?4, datetime('now'), ?5)",
        rusqlite::params![
            position,
            candidate.reason,
            candidate.artist_name,
            candidate.title,
            candidate.tidal_track_id
        ],
    )?;
    Ok(true)
}

fn append_seed_track_id(conn: &rusqlite::Connection) -> Option<i64> {
    conn.query_row(
        "SELECT current_track_id FROM playback_state WHERE id = 1",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .or_else(|| {
        conn.query_row(
            "SELECT track_id FROM queue WHERE track_id IS NOT NULL ORDER BY position DESC, id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    })
}

fn rank_radio_candidates(
    conn: &rusqlite::Connection,
    seed_track_id: Option<i64>,
    candidates: Vec<RadioCandidate>,
) -> Vec<RadioCandidate> {
    let Some(seed_track_id) = seed_track_id else {
        return candidates;
    };
    let generated = candidates
        .into_iter()
        .map(|candidate| GeneratedCandidate {
            track_id: (candidate.is_in_library && candidate.track_id > 0)
                .then_some(candidate.track_id),
            tidal_id: candidate.tidal_track_id,
            policy: Default::default(),
            item: candidate,
        })
        .collect::<Vec<_>>();
    let fallback = generated
        .iter()
        .map(|candidate| candidate.item.clone())
        .collect::<Vec<_>>();
    rank_generated_candidates(conn, seed_track_id, generated)
        .map(|ranked| {
            ranked
                .into_iter()
                .map(|ranked| {
                    let mut candidate = ranked.item;
                    candidate.reason =
                        append_dj_reason(&candidate.reason, ranked.score, &ranked.reasons);
                    candidate
                })
                .collect()
        })
        .unwrap_or(fallback)
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

    fn add_dsp_table(conn: &rusqlite::Connection) {
        conn.execute_batch(
            "
            CREATE TABLE audio_dsp_features (
                track_id INTEGER PRIMARY KEY,
                bpm REAL,
                key_signature TEXT,
                camelot_key TEXT,
                loudness_lufs REAL,
                energy REAL,
                danceability REAL,
                beat_strength REAL,
                spectral_centroid REAL,
                stereo_width REAL,
                is_instrumental INTEGER NOT NULL DEFAULT 0,
                analysis_source TEXT NOT NULL DEFAULT 'test',
                analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
                samples_analyzed INTEGER,
                analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
                analysis_version TEXT NOT NULL DEFAULT 'test'
            );
            ",
        )
        .unwrap();
    }

    fn insert_features(conn: &rusqlite::Connection, track_id: i64, bpm: f64, key: &str) {
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES (?1, ?2, ?3)",
            rusqlite::params![track_id, bpm, key],
        )
        .unwrap();
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
    fn radio_queue_ranks_generated_candidates_by_dj_fit_after_seed() {
        let conn = conn_with_queue();
        add_dsp_table(&conn);
        insert_features(&conn, 1, 124.0, "8A");
        insert_features(&conn, 10, 126.0, "8A");
        insert_features(&conn, 20, 145.0, "3B");

        build_radio_queue_from_candidates_with_seed(
            &conn,
            Some(1),
            vec![
                candidate(20, true, "Clash Artist", "Clash Track"),
                candidate(10, true, "Fit Artist", "Fit Track"),
            ],
        )
        .unwrap();

        let rows: Vec<(i32, Option<i64>, Option<String>)> = conn
            .prepare("SELECT position, track_id, reason FROM queue ORDER BY position ASC")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert_eq!(rows[0], (0, Some(1), None));
        assert_eq!(rows[1].1, Some(10));
        assert_eq!(rows[2].1, Some(20));
        assert!(
            rows[1]
                .2
                .as_deref()
                .is_some_and(|reason| reason.contains("dj: tempo inside 3 percent"))
        );
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
