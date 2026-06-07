use crate::PendingEphemeralTidalTrack;
use crate::db::models::Track;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const EPHEMERAL_TIDAL_MIX_NEXT_QUEUE_ITEM_ID: i64 = -1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DjMediaRef {
    LibraryTrack {
        track_id: i64,
    },
    TidalTrack {
        tidal_id: i64,
        track_id: Option<i64>,
    },
    PendingQueueItem {
        queue_item_id: i64,
        pending_artist: String,
        pending_title: String,
        tidal_id_hint: Option<i64>,
    },
}

impl DjMediaRef {
    pub fn profile_key(&self) -> crate::db::models::AudioDjProfileKey {
        match self {
            DjMediaRef::LibraryTrack { track_id } => crate::db::models::AudioDjProfileKey {
                media_ref_kind: "library_track".to_string(),
                media_ref_id: track_id.to_string(),
            },
            DjMediaRef::TidalTrack { tidal_id, .. } => crate::db::models::AudioDjProfileKey {
                media_ref_kind: "tidal_track".to_string(),
                media_ref_id: tidal_id.to_string(),
            },
            DjMediaRef::PendingQueueItem {
                queue_item_id,
                tidal_id_hint,
                ..
            } => {
                if let Some(tidal_id) = tidal_id_hint {
                    crate::db::models::AudioDjProfileKey {
                        media_ref_kind: "tidal_track".to_string(),
                        media_ref_id: tidal_id.to_string(),
                    }
                } else {
                    crate::db::models::AudioDjProfileKey {
                        media_ref_kind: "queue_item".to_string(),
                        media_ref_id: queue_item_id.to_string(),
                    }
                }
            }
        }
    }

    pub fn track_id(&self) -> Option<i64> {
        match self {
            DjMediaRef::LibraryTrack { track_id } => Some(*track_id),
            DjMediaRef::TidalTrack { track_id, .. } => *track_id,
            DjMediaRef::PendingQueueItem { .. } => None,
        }
    }

    pub fn tidal_id(&self) -> Option<i64> {
        match self {
            DjMediaRef::LibraryTrack { .. } => None,
            DjMediaRef::TidalTrack { tidal_id, .. } => Some(*tidal_id),
            DjMediaRef::PendingQueueItem { tidal_id_hint, .. } => *tidal_id_hint,
        }
    }

    pub fn queue_item_id(&self) -> Option<i64> {
        match self {
            DjMediaRef::LibraryTrack { .. } | DjMediaRef::TidalTrack { .. } => None,
            DjMediaRef::PendingQueueItem { queue_item_id, .. } => Some(*queue_item_id),
        }
    }
}

pub fn tidal_media_ref_for_track(track: &Track) -> Option<DjMediaRef> {
    let tidal_id = track.tidal_id?;
    Some(DjMediaRef::TidalTrack {
        tidal_id,
        track_id: (track.id > 0).then_some(track.id),
    })
}

pub fn build_ephemeral_tidal_mix_pair(
    current: &Track,
    pending: &[PendingEphemeralTidalTrack],
) -> Option<DjLookaheadPair> {
    let current_ref = tidal_media_ref_for_track(current)?;
    let next_ref = pending.first().map(|track| DjMediaRef::TidalTrack {
        tidal_id: track.tidal_track_id,
        track_id: None,
    });
    let next_queue_item_id = next_ref
        .is_some()
        .then_some(EPHEMERAL_TIDAL_MIX_NEXT_QUEUE_ITEM_ID);
    Some(DjLookaheadPair {
        current: Some(current_ref),
        next: next_ref,
        current_queue_item_id: None,
        next_queue_item_id,
        queue_generation: compute_ephemeral_tidal_mix_generation(current, pending),
    })
}

pub fn build_external_current_queue_pair(
    conn: &Connection,
    current: &Track,
) -> Result<Option<DjLookaheadPair>> {
    let current_ref = tidal_media_ref_for_track(current).or_else(|| {
        (current.id > 0).then_some(DjMediaRef::LibraryTrack {
            track_id: current.id,
        })
    });
    let Some(current_ref) = current_ref else {
        return Ok(None);
    };
    let next_row = load_first_queue_ref(conn)?;
    Ok(Some(DjLookaheadPair {
        current: Some(current_ref.clone()),
        next: next_row.as_ref().map(|row| row.media_ref.clone()),
        current_queue_item_id: None,
        next_queue_item_id: next_row.as_ref().map(|row| row.queue_item_id),
        queue_generation: compute_external_current_queue_generation(conn, &current_ref)?,
    }))
}

fn compute_ephemeral_tidal_mix_generation(
    current: &Track,
    pending: &[PendingEphemeralTidalTrack],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "ephemeral_tidal_mix".hash(&mut hasher);
    current.tidal_id.hash(&mut hasher);
    current.id.hash(&mut hasher);
    for track in pending {
        track.tidal_track_id.hash(&mut hasher);
        track.title.hash(&mut hasher);
        track.artist_name.hash(&mut hasher);
    }
    hasher.finish()
}

fn compute_external_current_queue_generation(
    conn: &Connection,
    current_ref: &DjMediaRef,
) -> Result<u64> {
    let mut hasher = DefaultHasher::new();
    "external_current_queue".hash(&mut hasher);
    let key = current_ref.profile_key();
    key.media_ref_kind.hash(&mut hasher);
    key.media_ref_id.hash(&mut hasher);
    compute_queue_generation(conn)?.hash(&mut hasher);
    Ok(hasher.finish())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DjLookaheadPair {
    pub current: Option<DjMediaRef>,
    pub next: Option<DjMediaRef>,
    pub current_queue_item_id: Option<i64>,
    pub next_queue_item_id: Option<i64>,
    pub queue_generation: u64,
}

pub fn load_dj_lookahead_pair(conn: &Connection) -> Result<DjLookaheadPair> {
    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or((None, None));

    let current_row = if let Some(queue_item_id) = current_queue_item_id {
        match load_queue_ref_by_id(conn, queue_item_id)? {
            Some(row) if row.track_id == current_track_id => Some(row),
            _ => match current_track_id {
                Some(track_id) => load_queue_ref_by_track_id(conn, track_id)?,
                None => None,
            },
        }
    } else if let Some(track_id) = current_track_id {
        load_queue_ref_by_track_id(conn, track_id)?
    } else {
        None
    };

    let next_row = if let Some(current_row) = current_row.as_ref() {
        load_next_queue_ref_after(conn, current_row.position, current_row.queue_item_id)?
    } else {
        load_first_queue_ref(conn)?
    };

    Ok(DjLookaheadPair {
        current: current_row.as_ref().map(|row| row.media_ref.clone()),
        next: next_row.as_ref().map(|row| row.media_ref.clone()),
        current_queue_item_id: current_row.as_ref().map(|row| row.queue_item_id),
        next_queue_item_id: next_row.as_ref().map(|row| row.queue_item_id),
        queue_generation: compute_queue_generation(conn)?,
    })
}

#[derive(Debug, Clone)]
struct QueueRefRow {
    queue_item_id: i64,
    position: i64,
    track_id: Option<i64>,
    media_ref: DjMediaRef,
}

fn load_queue_ref_by_id(conn: &Connection, queue_item_id: i64) -> Result<Option<QueueRefRow>> {
    load_queue_ref(
        conn,
        "WHERE q.id = ?1 ORDER BY q.position ASC, q.id ASC LIMIT 1",
        params![queue_item_id],
    )
}

fn load_queue_ref_by_track_id(conn: &Connection, track_id: i64) -> Result<Option<QueueRefRow>> {
    load_queue_ref(
        conn,
        "WHERE q.track_id = ?1 ORDER BY q.position ASC, q.id ASC LIMIT 1",
        params![track_id],
    )
}

fn load_next_queue_ref_after(
    conn: &Connection,
    position: i64,
    queue_item_id: i64,
) -> Result<Option<QueueRefRow>> {
    load_queue_ref(
        conn,
        "WHERE q.position > ?1 OR (q.position = ?1 AND q.id > ?2)
         ORDER BY q.position ASC, q.id ASC LIMIT 1",
        params![position, queue_item_id],
    )
}

fn load_first_queue_ref(conn: &Connection) -> Result<Option<QueueRefRow>> {
    load_queue_ref(conn, "ORDER BY q.position ASC, q.id ASC LIMIT 1", [])
}

fn load_queue_ref<P>(conn: &Connection, clause: &str, params: P) -> Result<Option<QueueRefRow>>
where
    P: rusqlite::Params,
{
    let sql = format!(
        "SELECT q.id, q.position, q.track_id, t.tidal_id, q.pending_artist,
            q.pending_title, q.tidal_id_hint
         FROM queue q
         LEFT JOIN tracks t ON t.id = q.track_id
         {clause}"
    );
    conn.query_row(&sql, params, |row| {
        let queue_item_id: i64 = row.get(0)?;
        let position: i64 = row.get(1)?;
        let track_id: Option<i64> = row.get(2)?;
        let tidal_id: Option<i64> = row.get(3)?;
        let pending_artist: Option<String> = row.get(4)?;
        let pending_title: Option<String> = row.get(5)?;
        let tidal_id_hint: Option<i64> = row.get(6)?;
        let media_ref = match (track_id, tidal_id) {
            (Some(track_id), Some(tidal_id)) => DjMediaRef::TidalTrack {
                tidal_id,
                track_id: Some(track_id),
            },
            (Some(track_id), None) => DjMediaRef::LibraryTrack { track_id },
            (None, _) => DjMediaRef::PendingQueueItem {
                queue_item_id,
                pending_artist: pending_artist.unwrap_or_default(),
                pending_title: pending_title.unwrap_or_default(),
                tidal_id_hint,
            },
        };
        Ok(QueueRefRow {
            queue_item_id,
            position,
            track_id,
            media_ref,
        })
    })
    .optional()
    .map_err(Into::into)
}

fn compute_queue_generation(conn: &Connection) -> Result<u64> {
    let mut hasher = DefaultHasher::new();
    let state: (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or((None, None));
    state.hash(&mut hasher);

    let mut stmt = conn.prepare(
        "SELECT id, position, track_id, source, pending_artist, pending_title, tidal_id_hint
         FROM queue
         ORDER BY position ASC, id ASC",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let position: i64 = row.get(1)?;
        let track_id: Option<i64> = row.get(2)?;
        let source: Option<String> = row.get(3)?;
        let pending_artist: Option<String> = row.get(4)?;
        let pending_title: Option<String> = row.get(5)?;
        let tidal_id_hint: Option<i64> = row.get(6)?;
        (
            id,
            position,
            track_id,
            source,
            pending_artist,
            pending_title,
            tidal_id_hint,
        )
            .hash(&mut hasher);
    }
    Ok(hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().expect("db");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL REFERENCES artists(id),
                tidal_id INTEGER
             );
             CREATE TABLE queue (
                id INTEGER PRIMARY KEY,
                track_id INTEGER REFERENCES tracks(id),
                position INTEGER NOT NULL,
                source TEXT DEFAULT 'user',
                pending_artist TEXT,
                pending_title TEXT,
                tidal_id_hint INTEGER
             );
             CREATE TABLE playback_state (
                id INTEGER PRIMARY KEY,
                current_track_id INTEGER,
                current_queue_item_id INTEGER
             );
             INSERT INTO playback_state (id) VALUES (1);
             INSERT INTO artists (id, name) VALUES (1, 'Artist');",
        )
        .expect("schema");
        conn
    }

    fn seed_track(conn: &Connection, id: i64, tidal_id: Option<i64>) {
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id) VALUES (?1, ?2, 1, ?3)",
            params![id, format!("Track {id}"), tidal_id],
        )
        .expect("track");
    }

    fn seed_queue_track(conn: &Connection, position: i64, track_id: i64) -> i64 {
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (?1, ?2, 'user')",
            params![track_id, position],
        )
        .expect("queue");
        conn.last_insert_rowid()
    }

    fn ephemeral_track(id: i64, tidal_id: i64) -> Track {
        Track {
            id,
            title: "Current".to_string(),
            artist_id: 0,
            artist_name: Some("Artist".to_string()),
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: Some(tidal_id),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some("tidal".to_string()),
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal_ephemeral".to_string(),
            artwork_url: None,
        }
    }

    fn pending_tidal(id: i64, title: &str) -> PendingEphemeralTidalTrack {
        PendingEphemeralTidalTrack {
            tidal_track_id: id,
            title: title.to_string(),
            artist_name: Some("Next Artist".to_string()),
            album_title: None,
            artwork_url: None,
            duration_ms: Some(180_000),
        }
    }

    #[test]
    fn ephemeral_tidal_mix_pair_uses_tidal_refs_without_negative_track_ids() {
        let current = ephemeral_track(-101, 101);
        let pending = vec![pending_tidal(202, "Next")];

        let pair = build_ephemeral_tidal_mix_pair(&current, &pending).expect("pair");

        assert_eq!(
            pair.current,
            Some(DjMediaRef::TidalTrack {
                tidal_id: 101,
                track_id: None
            })
        );
        assert_eq!(
            pair.next,
            Some(DjMediaRef::TidalTrack {
                tidal_id: 202,
                track_id: None
            })
        );
        assert_eq!(pair.current_queue_item_id, None);
        assert_eq!(
            pair.next_queue_item_id,
            Some(EPHEMERAL_TIDAL_MIX_NEXT_QUEUE_ITEM_ID)
        );
    }

    #[test]
    fn ephemeral_tidal_mix_generation_changes_when_pending_queue_changes() {
        let current = ephemeral_track(-101, 101);
        let before = build_ephemeral_tidal_mix_pair(&current, &[pending_tidal(202, "Next")])
            .expect("before")
            .queue_generation;
        let after = build_ephemeral_tidal_mix_pair(
            &current,
            &[pending_tidal(202, "Next"), pending_tidal(303, "Third")],
        )
        .expect("after")
        .queue_generation;

        assert_ne!(before, after);
    }

    #[test]
    fn external_current_queue_pair_uses_first_persisted_queue_item() {
        let conn = conn();
        seed_track(&conn, 2, Some(202));
        let next = seed_queue_track(&conn, 0, 2);
        let current = ephemeral_track(-101, 101);

        let pair = build_external_current_queue_pair(&conn, &current)
            .unwrap()
            .expect("pair");

        assert_eq!(
            pair.current,
            Some(DjMediaRef::TidalTrack {
                tidal_id: 101,
                track_id: None
            })
        );
        assert_eq!(
            pair.next,
            Some(DjMediaRef::TidalTrack {
                tidal_id: 202,
                track_id: Some(2)
            })
        );
        assert_eq!(pair.current_queue_item_id, None);
        assert_eq!(pair.next_queue_item_id, Some(next));
    }

    #[test]
    fn external_current_queue_generation_includes_current_ref() {
        let conn = conn();
        seed_track(&conn, 2, Some(202));
        seed_queue_track(&conn, 0, 2);

        let first = build_external_current_queue_pair(&conn, &ephemeral_track(-101, 101))
            .unwrap()
            .expect("first")
            .queue_generation;
        let second = build_external_current_queue_pair(&conn, &ephemeral_track(-303, 303))
            .unwrap()
            .expect("second")
            .queue_generation;

        assert_ne!(first, second);
    }

    #[test]
    fn library_track_profile_key_uses_track_id() {
        let key = DjMediaRef::LibraryTrack { track_id: 42 }.profile_key();
        assert_eq!(key.media_ref_kind, "library_track");
        assert_eq!(key.media_ref_id, "42");
        assert_eq!(
            DjMediaRef::LibraryTrack { track_id: 42 }.track_id(),
            Some(42)
        );
    }

    #[test]
    fn tidal_track_profile_key_uses_tidal_id() {
        let key = DjMediaRef::TidalTrack {
            tidal_id: 55,
            track_id: Some(42),
        }
        .profile_key();
        assert_eq!(key.media_ref_kind, "tidal_track");
        assert_eq!(key.media_ref_id, "55");
        let media_ref = DjMediaRef::TidalTrack {
            tidal_id: 55,
            track_id: Some(42),
        };
        assert_eq!(media_ref.tidal_id(), Some(55));
        assert_eq!(media_ref.track_id(), Some(42));
    }

    #[test]
    fn pending_queue_item_prefers_tidal_hint() {
        let key = DjMediaRef::PendingQueueItem {
            queue_item_id: 1,
            pending_artist: "A".to_string(),
            pending_title: "T".to_string(),
            tidal_id_hint: Some(99),
        }
        .profile_key();
        assert_eq!(key.media_ref_kind, "tidal_track");
        assert_eq!(key.media_ref_id, "99");
    }

    #[test]
    fn pending_queue_item_without_hint_uses_queue_item_key() {
        let key = DjMediaRef::PendingQueueItem {
            queue_item_id: 1,
            pending_artist: "A".to_string(),
            pending_title: "T".to_string(),
            tidal_id_hint: None,
        }
        .profile_key();
        assert_eq!(key.media_ref_kind, "queue_item");
        assert_eq!(key.media_ref_id, "1");
        assert_eq!(
            DjMediaRef::PendingQueueItem {
                queue_item_id: 1,
                pending_artist: "A".to_string(),
                pending_title: "T".to_string(),
                tidal_id_hint: None,
            }
            .queue_item_id(),
            Some(1)
        );
    }

    #[test]
    fn lookahead_pair_uses_current_queue_item_id() {
        let conn = conn();
        seed_track(&conn, 1, None);
        seed_track(&conn, 2, None);
        let first = seed_queue_track(&conn, 0, 1);
        let second = seed_queue_track(&conn, 1, 2);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![first],
        )
        .unwrap();

        let pair = load_dj_lookahead_pair(&conn).unwrap();
        assert_eq!(pair.current_queue_item_id, Some(first));
        assert_eq!(pair.next_queue_item_id, Some(second));
    }

    #[test]
    fn lookahead_pair_falls_back_to_current_track_id() {
        let conn = conn();
        seed_track(&conn, 1, None);
        seed_track(&conn, 2, None);
        let first = seed_queue_track(&conn, 0, 1);
        let second = seed_queue_track(&conn, 1, 2);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = NULL WHERE id = 1",
            [],
        )
        .unwrap();

        let pair = load_dj_lookahead_pair(&conn).unwrap();
        assert_eq!(pair.current_queue_item_id, Some(first));
        assert_eq!(pair.next_queue_item_id, Some(second));
    }

    #[test]
    fn lookahead_pair_repairs_mismatched_current_queue_item_id() {
        let conn = conn();
        seed_track(&conn, 1, None);
        seed_track(&conn, 2, None);
        seed_track(&conn, 3, None);
        let first = seed_queue_track(&conn, 0, 1);
        let mismatched = seed_queue_track(&conn, 1, 2);
        let third = seed_queue_track(&conn, 2, 3);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![mismatched],
        )
        .unwrap();

        let pair = load_dj_lookahead_pair(&conn).unwrap();
        assert_eq!(pair.current_queue_item_id, Some(first));
        assert_eq!(pair.next_queue_item_id, Some(mismatched));
        assert_ne!(pair.next_queue_item_id, Some(third));
    }

    #[test]
    fn lookahead_pair_accepts_pending_current_queue_item_id() {
        let conn = conn();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_artist, pending_title)
             VALUES (NULL, 0, 'radio_pending', 'Pending Artist', 'Pending Title')",
            [],
        )
        .unwrap();
        let pending = conn.last_insert_rowid();
        seed_track(&conn, 2, None);
        let next = seed_queue_track(&conn, 1, 2);
        conn.execute(
            "UPDATE playback_state SET current_track_id = NULL, current_queue_item_id = ?1 WHERE id = 1",
            params![pending],
        )
        .unwrap();

        let pair = load_dj_lookahead_pair(&conn).unwrap();
        assert_eq!(pair.current_queue_item_id, Some(pending));
        assert_eq!(pair.next_queue_item_id, Some(next));
        assert!(matches!(
            pair.current,
            Some(DjMediaRef::PendingQueueItem {
                pending_artist,
                pending_title,
                ..
            }) if pending_artist == "Pending Artist" && pending_title == "Pending Title"
        ));
    }

    #[test]
    fn lookahead_pair_returns_pending_next_item() {
        let conn = conn();
        seed_track(&conn, 1, None);
        let first = seed_queue_track(&conn, 0, 1);
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_artist, pending_title)
             VALUES (NULL, 1, 'radio_pending', 'Pending Artist', 'Pending Title')",
            [],
        )
        .unwrap();
        let pending_id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![first],
        )
        .unwrap();

        let pair = load_dj_lookahead_pair(&conn).unwrap();
        assert_eq!(pair.next_queue_item_id, Some(pending_id));
        assert!(matches!(
            pair.next,
            Some(DjMediaRef::PendingQueueItem {
                pending_artist,
                pending_title,
                ..
            }) if pending_artist == "Pending Artist" && pending_title == "Pending Title"
        ));
    }

    #[test]
    fn lookahead_pair_updates_after_queue_reorder() {
        let conn = conn();
        seed_track(&conn, 1, None);
        seed_track(&conn, 2, None);
        seed_track(&conn, 3, None);
        let first = seed_queue_track(&conn, 0, 1);
        let second = seed_queue_track(&conn, 1, 2);
        let third = seed_queue_track(&conn, 2, 3);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![first],
        )
        .unwrap();
        assert_eq!(
            load_dj_lookahead_pair(&conn).unwrap().next_queue_item_id,
            Some(second)
        );
        conn.execute(
            "UPDATE queue SET position = 1 WHERE id = ?1",
            params![third],
        )
        .unwrap();
        conn.execute(
            "UPDATE queue SET position = 2 WHERE id = ?1",
            params![second],
        )
        .unwrap();
        assert_eq!(
            load_dj_lookahead_pair(&conn).unwrap().next_queue_item_id,
            Some(third)
        );
    }

    #[test]
    fn lookahead_pair_ignores_second_upcoming_item() {
        let conn = conn();
        seed_track(&conn, 1, None);
        seed_track(&conn, 2, None);
        seed_track(&conn, 3, None);
        let first = seed_queue_track(&conn, 0, 1);
        let second = seed_queue_track(&conn, 1, 2);
        seed_queue_track(&conn, 2, 3);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![first],
        )
        .unwrap();
        let pair = load_dj_lookahead_pair(&conn).unwrap();
        assert_eq!(pair.next_queue_item_id, Some(second));
    }

    #[test]
    fn lookahead_pair_generation_changes_after_queue_edit() {
        let conn = conn();
        seed_track(&conn, 1, None);
        seed_track(&conn, 2, None);
        let first = seed_queue_track(&conn, 0, 1);
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![first],
        )
        .unwrap();
        let before = load_dj_lookahead_pair(&conn).unwrap().queue_generation;
        seed_queue_track(&conn, 1, 2);
        let after = load_dj_lookahead_pair(&conn).unwrap().queue_generation;
        assert_ne!(before, after);
    }

    #[test]
    fn lookahead_pair_generation_changes_after_pending_resolution() {
        let conn = conn();
        seed_track(&conn, 1, None);
        let first = seed_queue_track(&conn, 0, 1);
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_artist, pending_title)
             VALUES (NULL, 1, 'radio_pending', 'Pending Artist', 'Pending Title')",
            [],
        )
        .unwrap();
        let pending = conn.last_insert_rowid();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
            params![first],
        )
        .unwrap();
        let before = load_dj_lookahead_pair(&conn).unwrap().queue_generation;
        conn.execute(
            "UPDATE queue SET tidal_id_hint = 99 WHERE id = ?1",
            params![pending],
        )
        .unwrap();
        let after = load_dj_lookahead_pair(&conn).unwrap().queue_generation;
        assert_ne!(before, after);
    }
}
