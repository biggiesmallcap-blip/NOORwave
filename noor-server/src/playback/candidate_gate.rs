//! One gate for every recommendation source.
//!
//! Automix (learned, fallback and external picks), radio and resolved pending
//! rows all ask the same questions before a track reaches the queue: is it
//! hidden (AI filter), played in the last few hours, skipped early recently,
//! marked "Not for me" (track or artist), or already queued under another id,
//! ISRC or version ("Song" vs "Song (2011 Remaster)")? Each check reads data
//! once per refill; a missing table reads as "nothing to exclude", so a gate
//! problem can thin a queue but never break playback.

use crate::db::models::{QueueItem, Track};
use rusqlite::Connection;
use std::collections::HashSet;

/// Tracks played in this window are not suggested again.
const RECENT_PLAY_HOURS: i64 = 6;
/// An early skip in this window keeps a track out until a later full listen.
const EARLY_SKIP_DAYS: i64 = 14;

/// Words that mark a release version of the same recording, not a new song.
/// "mix" and "remix" are deliberately absent: a remix is a different track.
const VERSION_WORDS: [&str; 15] = [
    "remaster",
    "remastered",
    "live",
    "edit",
    "mono",
    "stereo",
    "demo",
    "deluxe",
    "version",
    "single",
    "bonus",
    "anniversary",
    "explicit",
    "clean",
    "radio",
];

/// Lowercase alphanumeric words of `text`.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(String::from)
        .collect()
}

fn is_version_note(text: &str) -> bool {
    words(text)
        .iter()
        .any(|word| VERSION_WORDS.contains(&word.as_str()))
}

/// Identity of a recording across versions: artist plus the title with
/// version notes removed, whether bracketed ("Song (2011 Remaster)",
/// "Song [Live]") or dashed ("Song - Radio Edit").
pub(crate) fn base_title_key(artist: &str, title: &str) -> String {
    let mut kept = String::new();
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest[open..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(close_rel) = rest[open..].find(close_char) else {
            break;
        };
        let inner = &rest[open + 1..open + close_rel];
        kept.push_str(&rest[..open]);
        if !is_version_note(inner) {
            kept.push(' ');
            kept.push_str(inner);
        }
        rest = &rest[open + close_rel + 1..];
    }
    kept.push_str(rest);
    let base = match kept.find(" - ") {
        Some(dash) if is_version_note(&kept[dash + 3..]) => kept[..dash].to_string(),
        _ => kept,
    };
    format!("{}|{}", words(artist).join(" "), words(&base).join(" "))
}

#[derive(Debug, Default)]
pub(crate) struct CandidateGate {
    hidden_tidal_ids: HashSet<i64>,
    recent_track_ids: HashSet<i64>,
    early_skipped_ids: HashSet<i64>,
    not_for_me_tracks: HashSet<i64>,
    not_for_me_artist_ids: HashSet<i64>,
    not_for_me_artist_names: HashSet<String>,
    seen_track_ids: HashSet<i64>,
    seen_tidal_ids: HashSet<i64>,
    seen_isrcs: HashSet<String>,
    seen_titles: HashSet<String>,
}

fn id_set(conn: &Connection, sql: &str) -> HashSet<i64> {
    let read = || -> rusqlite::Result<HashSet<i64>> {
        conn.prepare(sql)?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect()
    };
    read().unwrap_or_else(|error| {
        tracing::debug!(%error, "candidate gate: check skipped");
        HashSet::new()
    })
}

impl CandidateGate {
    /// Load the gate for a refill. `queue_items` are the rows already queued;
    /// `skip_queue_item_id` leaves one row out (the pending row being checked).
    pub(crate) fn load(
        conn: &Connection,
        queue_items: &[QueueItem],
        skip_queue_item_id: Option<i64>,
    ) -> Self {
        let early_skip = "(COALESCE(lh.completed, 0) = 0
              AND (COALESCE(lh.duration_listened_ms, 0) < 30000
                   OR (t.duration_ms > 0 AND COALESCE(lh.duration_listened_ms, 0) * 4 < t.duration_ms)))";
        let mut gate = Self {
            hidden_tidal_ids: crate::db::tidal_content::blocked_ids(conn).unwrap_or_default(),
            recent_track_ids: id_set(
                conn,
                &format!(
                    "SELECT DISTINCT track_id FROM listen_history
                     WHERE julianday(started_at) >= julianday('now', '-{RECENT_PLAY_HOURS} hours')"
                ),
            ),
            early_skipped_ids: id_set(
                conn,
                &format!(
                    "SELECT lh.track_id FROM listen_history lh
                     LEFT JOIN tracks t ON t.id = lh.track_id
                     WHERE julianday(lh.started_at) >= julianday('now', '-{EARLY_SKIP_DAYS} days')
                       AND {early_skip}
                     EXCEPT
                     SELECT track_id FROM listen_history
                     WHERE completed = 1
                       AND julianday(started_at) >= julianday('now', '-{EARLY_SKIP_DAYS} days')"
                ),
            ),
            not_for_me_tracks: id_set(
                conn,
                "SELECT entity_id FROM recommendation_feedback WHERE kind = 'track'",
            ),
            not_for_me_artist_ids: id_set(
                conn,
                "SELECT entity_id FROM recommendation_feedback WHERE kind = 'artist'",
            ),
            ..Self::default()
        };
        let names = (|| -> rusqlite::Result<HashSet<String>> {
            conn.prepare(
                "SELECT a.name FROM recommendation_feedback f
                 JOIN artists a ON a.id = f.entity_id
                 WHERE f.kind = 'artist'",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect()
        })()
        .unwrap_or_default();
        gate.not_for_me_artist_names = names.iter().map(|name| words(name).join(" ")).collect();
        for item in queue_items {
            if Some(item.id) == skip_queue_item_id {
                continue;
            }
            gate.note_queued(&item.track);
        }
        gate
    }

    /// Remember a track as already queued (the radio seed, for instance).
    pub(crate) fn note_queued(&mut self, track: &Track) {
        if track.id > 0 {
            self.seen_track_ids.insert(track.id);
        }
        if let Some(tidal_id) = track.tidal_id.filter(|id| *id > 0) {
            self.seen_tidal_ids.insert(tidal_id);
        }
        if let Some(isrc) = track.isrc.as_deref().filter(|isrc| !isrc.is_empty()) {
            self.seen_isrcs.insert(isrc.to_ascii_uppercase());
        }
        self.seen_titles.insert(base_title_key(
            track.artist_name.as_deref().unwrap_or_default(),
            &track.title,
        ));
    }

    /// Every check for one candidate, library or external.
    #[allow(clippy::too_many_arguments)]
    fn allows(
        &self,
        track_id: Option<i64>,
        artist_id: Option<i64>,
        tidal_id: Option<i64>,
        isrc: Option<&str>,
        artist_name: &str,
        title: &str,
    ) -> bool {
        let track_id = track_id.filter(|id| *id > 0);
        let tidal_id = tidal_id.filter(|id| *id > 0);
        if tidal_id.is_some_and(|id| self.hidden_tidal_ids.contains(&id)) {
            return false;
        }
        if let Some(id) = track_id
            && (self.recent_track_ids.contains(&id)
                || self.early_skipped_ids.contains(&id)
                || self.not_for_me_tracks.contains(&id)
                || self.seen_track_ids.contains(&id))
        {
            return false;
        }
        if artist_id.is_some_and(|id| self.not_for_me_artist_ids.contains(&id))
            || self
                .not_for_me_artist_names
                .contains(&words(artist_name).join(" "))
        {
            return false;
        }
        if tidal_id.is_some_and(|id| self.seen_tidal_ids.contains(&id)) {
            return false;
        }
        if isrc
            .filter(|isrc| !isrc.is_empty())
            .is_some_and(|isrc| self.seen_isrcs.contains(&isrc.to_ascii_uppercase()))
        {
            return false;
        }
        !self
            .seen_titles
            .contains(&base_title_key(artist_name, title))
    }

    pub(crate) fn allows_track(&self, track: &Track) -> bool {
        self.allows(
            Some(track.id),
            (track.artist_id != 0).then_some(track.artist_id),
            track.tidal_id,
            track.isrc.as_deref(),
            track.artist_name.as_deref().unwrap_or_default(),
            &track.title,
        )
    }

    /// `allows_track`, then remember the track so later picks dedupe against it.
    pub(crate) fn admit_track(&mut self, track: &Track) -> bool {
        if !self.allows_track(track) {
            return false;
        }
        self.note_queued(track);
        true
    }

    /// Gate and remember a candidate known only by name and ids (Last.fm and
    /// external rows, radio candidates).
    pub(crate) fn admit_candidate(
        &mut self,
        track_id: Option<i64>,
        tidal_id: Option<i64>,
        isrc: Option<&str>,
        artist_name: &str,
        title: &str,
    ) -> bool {
        if !self.allows(track_id, None, tidal_id, isrc, artist_name, title) {
            return false;
        }
        if let Some(id) = track_id.filter(|id| *id > 0) {
            self.seen_track_ids.insert(id);
        }
        if let Some(id) = tidal_id.filter(|id| *id > 0) {
            self.seen_tidal_ids.insert(id);
        }
        if let Some(isrc) = isrc.filter(|isrc| !isrc.is_empty()) {
            self.seen_isrcs.insert(isrc.to_ascii_uppercase());
        }
        self.seen_titles.insert(base_title_key(artist_name, title));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    #[test]
    fn base_title_key_collapses_versions_but_not_remixes() {
        let key = |title: &str| base_title_key("The Artist", title);
        assert_eq!(key("Song"), key("Song (2011 Remaster)"));
        assert_eq!(key("Song"), key("Song [Live]"));
        assert_eq!(key("Song"), key("Song - Radio Edit"));
        assert_eq!(key("Song"), key("Song - Remastered 2009"));
        assert_ne!(key("Song"), key("Song (Remix)"));
        assert_ne!(key("Song"), base_title_key("Other Artist", "Song"));
    }

    fn track(id: i64, artist_id: i64, artist: &str, title: &str) -> Track {
        Track {
            id,
            title: title.to_string(),
            artist_id,
            artist_name: Some(artist.to_string()),
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: Some(200_000),
            isrc: None,
            tidal_id: None,
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: None,
            best_source: None,
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Liked'), (2, 'Not For Me');
             INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES
                (1, 'Recent', 1, 200000), (2, 'Skipped', 1, 200000),
                (3, 'Skipped Then Loved', 1, 200000), (4, 'Fresh', 1, 200000),
                (5, 'Disliked', 1, 200000), (6, 'Any', 2, 200000);
             INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed) VALUES
                (1, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-1 hours'), 200000, 1),
                (2, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-3 days'), 9000, 0),
                (3, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-4 days'), 9000, 0),
                (3, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-2 days'), 200000, 1);
             INSERT INTO recommendation_feedback (kind, entity_id) VALUES ('track', 5), ('artist', 2);",
        )
        .expect("seed");
        conn
    }

    #[test]
    fn gate_rejects_recent_skipped_and_not_for_me() {
        let conn = db();
        let gate = CandidateGate::load(&conn, &[], None);
        assert!(!gate.allows_track(&track(1, 1, "Liked", "Recent")));
        assert!(!gate.allows_track(&track(2, 1, "Liked", "Skipped")));
        assert!(gate.allows_track(&track(3, 1, "Liked", "Skipped Then Loved")));
        assert!(gate.allows_track(&track(4, 1, "Liked", "Fresh")));
        assert!(!gate.allows_track(&track(5, 1, "Liked", "Disliked")));
        assert!(!gate.allows_track(&track(6, 2, "Not For Me", "Any")));
        let mut gate = gate;
        assert!(!gate.admit_candidate(None, None, None, "Not For Me", "Unseen Song"));
    }

    #[test]
    fn gate_dedupes_versions_ids_and_isrcs() {
        let conn = db();
        let mut gate = CandidateGate::load(&conn, &[], None);
        assert!(gate.admit_candidate(None, Some(900), Some("usabc1"), "Band", "Song"));
        assert!(!gate.admit_candidate(None, None, None, "Band", "Song (2011 Remaster)"));
        assert!(!gate.admit_candidate(None, Some(900), None, "Band", "Different Title"));
        assert!(!gate.admit_candidate(None, None, Some("USABC1"), "Band", "Other"));
        assert!(gate.admit_candidate(None, None, None, "Band", "Song (Remix)"));
    }

    #[test]
    fn gate_survives_a_database_without_its_tables() {
        let conn = Connection::open_in_memory().expect("db");
        let gate = CandidateGate::load(&conn, &[], None);
        assert!(gate.allows_track(&track(1, 1, "Anyone", "Anything")));
    }
}
