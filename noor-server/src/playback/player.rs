use crate::db::audio_settings::AudioQuality;
use crate::db::{
    models::{AudioDspFeatures, PlaybackState, QueueItem, Track},
    queries,
};
use crate::playback::gapless::{self, GaplessPlan, GaplessSettings};
use crate::playback::queue::{self, ShuffleMode};
use crate::playback::shuffle::{WeightedShuffleProfile, genre_shuffle, true_shuffle};
use crate::services::audio_analysis::compute_harmonic_multiplier;
use crate::services::tidal::stream::{self, StreamInfo, StreamRequest};
use crate::smart::taste_vector::adapters::from_session_profile;
use crate::smart::taste_vector::{SeedContext, TasteVector};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub queue: Vec<QueueItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackSourceRequest {
    LocalLibrary,
    TidalStream(StreamRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackSourceKind {
    LocalLibrary,
    TidalStream,
}

impl PlaybackSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalLibrary => "local",
            Self::TidalStream => "tidal",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedPlaybackJob {
    pub track: Track,
    pub source: PlaybackSourceRequest,
    pub gapless: GaplessPlan,
    pub generation: u64,
}

pub type PlaybackPreparation = PreparedPlaybackJob;

#[derive(Debug, Clone)]
pub struct ActiveListenSession {
    pub track_id: i64,
    pub started_at: DateTime<Utc>,
    pub accumulated_ms: i64,
    pub resumed_at: Option<DateTime<Utc>>,
    // Multi-track session context, captured at session start so it survives flush.
    pub session_id: String,
    pub source: crate::db::models::ListenSource,
    pub position_in_session: i32,
    pub transition_from_track_id: Option<i64>,
}

// Tracks the rolling state of the user's current listening session across multiple
// tracks. A session continues across tracks if the gap between flush time and the
// next track's start is under SESSION_GAP_THRESHOLD; otherwise a new session_id
// is minted. Lives on AppState; updated in flush_active_listen_session_locked
// after every successful listen_history write.
#[derive(Debug, Clone)]
pub struct LiveListenSession {
    pub session_id: String,
    pub last_track_id: i64,
    pub last_finished_at: DateTime<Utc>,
    pub position: i32,
}

const SESSION_GAP_MINUTES: i64 = 30;

pub const AUTOMIX_MIN_UPCOMING: usize = 8;
const AUTOMIX_BATCH_SIZE: usize = 12;
const SESSION_FEEDBACK_LIMIT: i64 = 60;
const TRUE_SHUFFLE_POOL_MULTIPLIER: usize = 12;

#[derive(Debug, Default)]
pub(crate) struct SessionTasteProfile {
    pub(crate) positive_artists: HashMap<i64, f64>,
    pub(crate) negative_artists: HashMap<i64, f64>,
    pub(crate) positive_genres: HashMap<String, f64>,
    pub(crate) negative_genres: HashMap<String, f64>,
    pub(crate) recent_track_ids: HashSet<i64>,
    pub(crate) skipped_track_ids: HashSet<i64>,
    pub(crate) current_artist_id: Option<i64>,
    pub(crate) current_album_id: Option<i64>,
    pub(crate) current_source: Option<String>,
    pub(crate) current_genres: HashSet<String>,
}

#[derive(Debug, Clone)]
struct ScoredTrack {
    track: Track,
    score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenSessionEndReason {
    Replaced,
    QueueEnded,
    Stopped,
}

impl PlaybackSourceRequest {
    pub fn kind(&self) -> PlaybackSourceKind {
        match self {
            Self::LocalLibrary => PlaybackSourceKind::LocalLibrary,
            Self::TidalStream(_) => PlaybackSourceKind::TidalStream,
        }
    }

    pub fn stream_request(&self) -> Option<&StreamRequest> {
        match self {
            Self::LocalLibrary => None,
            Self::TidalStream(request) => Some(request),
        }
    }
}

impl PreparedPlaybackJob {
    pub fn new(track: Track, source: PlaybackSourceRequest, gapless: GaplessPlan) -> Self {
        Self {
            track,
            source,
            gapless,
            generation: 0,
        }
    }

    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    pub fn source_kind(&self) -> PlaybackSourceKind {
        self.source.kind()
    }

    pub fn is_local(&self) -> bool {
        matches!(self.source, PlaybackSourceRequest::LocalLibrary)
    }

    pub fn is_tidal(&self) -> bool {
        matches!(self.source, PlaybackSourceRequest::TidalStream(_))
    }

    pub fn stream_request(&self) -> Option<&StreamRequest> {
        self.source.stream_request()
    }

    pub fn track_id(&self) -> i64 {
        self.track.id
    }

    pub fn source_label(&self) -> &'static str {
        self.source_kind().as_str()
    }
}

impl ActiveListenSession {
    pub fn start(
        track_id: i64,
        now: DateTime<Utc>,
        source: crate::db::models::ListenSource,
        prior: Option<&LiveListenSession>,
    ) -> Self {
        let (session_id, position, transition_from) = match prior {
            Some(ls) if (now - ls.last_finished_at).num_minutes() < SESSION_GAP_MINUTES => (
                ls.session_id.clone(),
                ls.position + 1,
                Some(ls.last_track_id),
            ),
            _ => (uuid::Uuid::new_v4().to_string(), 0, None),
        };
        Self {
            track_id,
            started_at: now,
            accumulated_ms: 0,
            resumed_at: Some(now),
            session_id,
            source,
            position_in_session: position,
            transition_from_track_id: transition_from,
        }
    }

    pub fn to_live_session(&self, finished_at: DateTime<Utc>) -> LiveListenSession {
        LiveListenSession {
            session_id: self.session_id.clone(),
            last_track_id: self.track_id,
            last_finished_at: finished_at,
            position: self.position_in_session,
        }
    }
}

// Reads the queue.source string of the currently-playing queue item and maps it
// to a ListenSource. Returns Unknown if no current queue item or the source
// label isn't one we recognize — those rows still get written, they just count
// at half confidence in the trainer.
pub fn lookup_current_listen_source(conn: &Connection) -> crate::db::models::ListenSource {
    use crate::db::models::ListenSource;
    let raw: Option<String> = conn
        .query_row(
            "SELECT q.source FROM playback_state ps
             JOIN queue q ON q.id = ps.current_queue_item_id
             WHERE ps.id = 1",
            [],
            |row| row.get(0),
        )
        .ok();
    match raw.as_deref() {
        Some("user") | Some("library") | Some("playback") => ListenSource::Manual,
        Some("radio") | Some("radio_pending") => ListenSource::Radio,
        Some("playlist") => ListenSource::Playlist,
        Some("album") => ListenSource::Album,
        Some("artist") => ListenSource::Artist,
        Some("search") => ListenSource::Search,
        Some("automix") | Some("automix-new") => ListenSource::Automix,
        _ => ListenSource::Unknown,
    }
}

impl ActiveListenSession {
    pub fn pause(&mut self, now: DateTime<Utc>) {
        if let Some(resumed_at) = self.resumed_at.take() {
            self.accumulated_ms += (now - resumed_at).num_milliseconds().max(0);
        }
    }

    pub fn resume(&mut self, now: DateTime<Utc>) {
        if self.resumed_at.is_none() {
            self.resumed_at = Some(now);
        }
    }

    pub fn listened_ms_at(&self, now: DateTime<Utc>) -> i64 {
        let live_ms = self
            .resumed_at
            .map(|resumed_at| (now - resumed_at).num_milliseconds().max(0))
            .unwrap_or(0);
        self.accumulated_ms + live_ms
    }
}

/// Canonical "give me everything the UI needs to render the player" loader.
/// Endpoints that mutate queue or playback_state should return this snapshot
/// (or call back into it via `get_playback_state`) so the UI never has to
/// stitch together partial responses. Returns `{state, queue}` together.
pub fn load_snapshot(conn: &Connection) -> Result<PlaybackSnapshot> {
    let state = load_state(conn)?;
    let queue = queue::load_queue(conn)?;
    Ok(PlaybackSnapshot { state, queue })
}

pub fn load_state(conn: &Connection) -> Result<PlaybackState> {
    let row = conn
        .query_row(
            "SELECT current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms, automix_discover_new, automix_use_learning, automix_allow_external, current_queue_item_id
             FROM playback_state
             WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, i32>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, bool>(9)?,
                    row.get::<_, bool>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow!("playback_state row missing"))?;

    let current_track = match row.0 {
        Some(track_id) => queue::get_track_by_id(conn, track_id)?,
        None => None,
    };

    Ok(PlaybackState {
        current_track,
        current_queue_item_id: row.11,
        position_ms: row.1,
        is_playing: row.2,
        volume: row.3,
        shuffle_mode: row.4,
        repeat_mode: row.5,
        automix_enabled: row.6,
        crossfade_ms: row.7,
        automix_discover_new: row.8,
        automix_use_learning: row.9,
        automix_allow_external: row.10,
    })
}

pub fn enqueue_track(conn: &Connection, track_id: i64, source: &str) -> Result<Vec<QueueItem>> {
    let track = queue::get_track_by_id(conn, track_id)?
        .ok_or_else(|| anyhow!("track {track_id} not found"))?;
    queue::append_tracks(conn, &[track], source)
}

pub fn replace_queue_with_tracks(
    conn: &Connection,
    track_ids: &[i64],
    source: &str,
) -> Result<Vec<QueueItem>> {
    let mut tracks = Vec::new();
    for track_id in track_ids {
        if let Some(track) = queue::get_track_by_id(conn, *track_id)? {
            tracks.push(track);
        }
    }
    queue::replace_queue(conn, &tracks, source)
}

/// Replace the queue with tracks plus optional per-row reasons.
///
/// `reasons` is index-aligned with `track_ids`. Missing or shorter
/// reasons lists default to `None` for the absent indices. Tracks that
/// fail to load are skipped (consistent with `replace_queue_with_tracks`).
pub fn replace_queue_with_reasons(
    conn: &Connection,
    track_ids: &[i64],
    reasons: &[Option<String>],
    source: &str,
) -> Result<Vec<QueueItem>> {
    let mut paired: Vec<(Track, Option<String>)> = Vec::with_capacity(track_ids.len());
    for (idx, track_id) in track_ids.iter().enumerate() {
        if let Some(track) = queue::get_track_by_id(conn, *track_id)? {
            let reason = reasons.get(idx).cloned().unwrap_or(None);
            paired.push((track, reason));
        }
    }
    queue::replace_queue_with_reasons(conn, &paired, source)
}

pub fn play_track_now(conn: &Connection, track_id: i64) -> Result<PlaybackSnapshot> {
    let track = queue::get_track_by_id(conn, track_id)?
        .ok_or_else(|| anyhow!("track {track_id} not found"))?;

    let current_ids = queue::queue_track_ids(conn)?;
    if !current_ids.contains(&track_id) {
        queue::append_tracks(conn, std::slice::from_ref(&track), "playback")?;
    }

    // Resolve to the actual queue row so the UI's "now playing" highlight
    // points at the right row (not just any row sharing the same track_id).
    let queue_item_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM queue WHERE track_id = ?1 ORDER BY position ASC, id ASC LIMIT 1",
            params![track_id],
            |row| row.get(0),
        )
        .optional()?;

    conn.execute(
        "UPDATE playback_state
         SET current_track_id = ?1,
             current_queue_item_id = ?2,
             position_ms = 0,
             is_playing = 1
         WHERE id = 1",
        params![track_id, queue_item_id],
    )?;

    load_snapshot(conn)
}

/// What changed during reconciliation. Callers use this to decide which
/// `AppEvent`s to broadcast and whether to stop the audio runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileOutcome {
    /// At least one queue row was deleted.
    pub queue_changed: bool,
    /// `playback_state.current_track_id` was updated (advanced or cleared).
    pub current_changed: bool,
    /// `is_playing` was set to 0 because no surviving track exists. Caller
    /// should also stop the audio runtime to release the output device.
    pub stopped_playback: bool,
    /// The new current track ID. `None` means playback was cleared.
    pub new_current_track_id: Option<i64>,
}

/// Reconcile the queue and playback state with a set of just-deleted tracks.
///
/// Run inside a single transaction so the queue and playback_state never
/// drift. Behavior:
/// 1. Delete queue rows pointing at any of `deleted_track_ids`. Pending rows
///    (track_id IS NULL) are unaffected — Last.fm radio neighbors don't
///    reference local track IDs.
/// 2. If the current track is in the deleted set, advance `current_track_id`
///    and `current_queue_item_id` to the next surviving queue row. The next
///    survivor is preferred from "after the current position"; if none
///    exists there, fall back to the first surviving row globally.
/// 3. If no survivor exists, clear current_*, set is_playing = 0, and signal
///    `stopped_playback` so the caller can also stop the audio runtime.
/// 4. Renormalise positions to be contiguous starting at 0.
pub fn reconcile_after_track_delete(
    conn: &Connection,
    deleted_track_ids: &[i64],
) -> Result<ReconcileOutcome> {
    if deleted_track_ids.is_empty() {
        return Ok(ReconcileOutcome::default());
    }

    let deleted_set: HashSet<i64> = deleted_track_ids.iter().copied().collect();
    let tx = conn.unchecked_transaction()?;

    // Snapshot the queue before deletion so we can pick the next survivor by
    // position — `current_queue_item_id` would be invalid after deletion.
    let rows: Vec<(i64, Option<i64>, i32)> = {
        let mut stmt =
            tx.prepare("SELECT id, track_id, position FROM queue ORDER BY position ASC, id ASC")?;
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let (current_track_id, current_qid): (Option<i64>, Option<i64>) = tx.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let current_pos = current_qid.and_then(|cqid| {
        rows.iter()
            .find(|(id, _, _)| *id == cqid)
            .map(|(_, _, p)| *p)
    });

    let is_survivor = |tid: &Option<i64>| -> bool {
        match tid {
            None => true,
            Some(t) => !deleted_set.contains(t),
        }
    };

    let new_current_row: Option<&(i64, Option<i64>, i32)> = if let Some(cp) = current_pos {
        rows.iter()
            .find(|(id, tid, p)| *p > cp && is_survivor(tid) && Some(*id) != current_qid)
            .or_else(|| {
                rows.iter()
                    .find(|(id, tid, _)| is_survivor(tid) && Some(*id) != current_qid)
            })
    } else {
        None
    };

    // Apply deletion now that survivor selection is decided.
    let placeholders = (1..=deleted_track_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let delete_sql = format!("DELETE FROM queue WHERE track_id IN ({})", placeholders);
    let deleted = tx.execute(
        &delete_sql,
        rusqlite::params_from_iter(deleted_track_ids.iter()),
    )?;
    let queue_changed = deleted > 0;

    let mut current_changed = false;
    let mut stopped_playback = false;
    let mut new_current_track_id = current_track_id;

    if let Some(ctid) = current_track_id
        && deleted_set.contains(&ctid) {
            current_changed = true;
            if let Some((qid, tid, _)) = new_current_row.copied() {
                tx.execute(
                    "UPDATE playback_state
                     SET current_track_id = ?1, current_queue_item_id = ?2, position_ms = 0
                     WHERE id = 1",
                    params![tid, qid],
                )?;
                new_current_track_id = tid;
            } else {
                tx.execute(
                    "UPDATE playback_state
                     SET current_track_id = NULL,
                         current_queue_item_id = NULL,
                         position_ms = 0,
                         is_playing = 0
                     WHERE id = 1",
                    [],
                )?;
                stopped_playback = true;
                new_current_track_id = None;
            }
        }

    if queue_changed {
        let surviving_ids: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM queue ORDER BY position ASC, id ASC")?;
            stmt.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (idx, qid) in surviving_ids.iter().enumerate() {
            tx.execute(
                "UPDATE queue SET position = ?1 WHERE id = ?2",
                params![idx as i32, qid],
            )?;
        }
    }

    tx.commit()?;
    Ok(ReconcileOutcome {
        queue_changed,
        current_changed,
        stopped_playback,
        new_current_track_id,
    })
}

pub fn pause(conn: &Connection) -> Result<PlaybackSnapshot> {
    conn.execute("UPDATE playback_state SET is_playing = 0 WHERE id = 1", [])?;
    load_snapshot(conn)
}

pub fn resume(conn: &Connection) -> Result<PlaybackSnapshot> {
    conn.execute("UPDATE playback_state SET is_playing = 1 WHERE id = 1", [])?;
    load_snapshot(conn)
}

pub fn set_position(conn: &Connection, position_ms: i64) -> Result<PlaybackSnapshot> {
    conn.execute(
        "UPDATE playback_state SET position_ms = ?1 WHERE id = 1",
        params![position_ms],
    )?;
    load_snapshot(conn)
}

pub fn set_volume(conn: &Connection, volume: f64) -> Result<PlaybackSnapshot> {
    let clamped = volume.clamp(0.0, 1.0);
    conn.execute(
        "UPDATE playback_state SET volume = ?1 WHERE id = 1",
        params![clamped],
    )?;
    load_snapshot(conn)
}

pub fn set_shuffle_mode(conn: &Connection, mode: ShuffleMode) -> Result<PlaybackSnapshot> {
    let current_queue_item_id: Option<i64> = conn.query_row(
        "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    conn.execute(
        "UPDATE playback_state SET shuffle_mode = ?1 WHERE id = 1",
        params![mode.as_str()],
    )?;
    queue::apply_shuffle(conn, mode, current_queue_item_id)?;
    load_snapshot(conn)
}

pub fn set_automix_enabled(conn: &Connection, enabled: bool) -> Result<PlaybackSnapshot> {
    conn.execute(
        "UPDATE playback_state SET automix_enabled = ?1 WHERE id = 1",
        params![enabled],
    )?;
    load_snapshot(conn)
}

pub fn set_automix_discover_new(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET automix_discover_new = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

pub fn set_automix_use_learning(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET automix_use_learning = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

pub fn set_automix_allow_external(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET automix_allow_external = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

pub fn set_repeat_mode(conn: &Connection, mode: &str) -> Result<PlaybackSnapshot> {
    let mode = match mode {
        "all" | "one" => mode,
        _ => "off",
    };
    conn.execute(
        "UPDATE playback_state SET repeat_mode = ?1 WHERE id = 1",
        params![mode],
    )?;
    load_snapshot(conn)
}

pub fn set_crossfade_ms(conn: &Connection, crossfade_ms: i32) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET crossfade_ms = ?1 WHERE id = 1",
        params![crossfade_ms.max(0)],
    )?;
    Ok(())
}

pub fn next_track(conn: &Connection, recently_cleared: bool) -> Result<PlaybackSnapshot> {
    let repeat_mode: String = conn.query_row(
        "SELECT repeat_mode FROM playback_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let queue_items = ensure_automix_queue_depth(conn, AUTOMIX_MIN_UPCOMING, recently_cleared)?;
    if queue_items.is_empty() {
        conn.execute(
            "UPDATE playback_state SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 0, position_ms = 0 WHERE id = 1",
            [],
        )?;
        return load_snapshot(conn);
    }

    // Prefer track_id lookup (library rows); fall back to queue item ID (pending rows
    // where current_track_id is NULL because the track isn't in the library yet).
    let current_index = current_track_id
        .and_then(|track_id| {
            queue_items
                .iter()
                .position(|item| item.track.id == track_id)
        })
        .or_else(|| {
            current_queue_item_id.and_then(|qid| queue_items.iter().position(|item| item.id == qid))
        });

    let next_track = match repeat_mode.as_str() {
        "one" => current_index
            .and_then(|idx| queue_items.get(idx))
            .or_else(|| queue_items.first()),
        _ => current_index
            .and_then(|idx| queue_items.get(idx + 1))
            .or_else(|| {
                if current_index.is_none() || repeat_mode == "all" {
                    queue_items.first()
                } else {
                    None
                }
            }),
    };

    if let Some(item) = next_track {
        // Pending rows have track.id == 0 (COALESCE sentinel); write NULL so the FK is not
        // violated. current_queue_item_id tracks position for the next advance.
        let new_track_id: Option<i64> = if item.track.id != 0 {
            Some(item.track.id)
        } else {
            None
        };
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = ?1, current_queue_item_id = ?2, position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![new_track_id, item.id],
        )?;
    } else {
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = NULL, position_ms = 0, is_playing = 0
             WHERE id = 1",
            [],
        )?;
    }

    load_snapshot(conn)
}

pub fn start_queue_from_beginning(
    conn: &Connection,
    recently_cleared: bool,
) -> Result<PlaybackSnapshot> {
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = NULL,
             current_queue_item_id = NULL,
             position_ms = 0
         WHERE id = 1",
        [],
    )?;
    next_track(conn, recently_cleared)
}

pub fn previous_track(conn: &Connection) -> Result<PlaybackSnapshot> {
    let (current_track_id, current_queue_item_id, position_ms): (Option<i64>, Option<i64>, i64) =
        conn.query_row(
            "SELECT current_track_id, current_queue_item_id, position_ms FROM playback_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

    let queue_items = queue::load_queue(conn)?;
    if queue_items.is_empty() {
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = NULL,
                 position_ms = 0, is_playing = 0
             WHERE id = 1",
            [],
        )?;
        return load_snapshot(conn);
    }

    // Restart in place when more than 3s into the track.
    if (current_track_id.is_some() || current_queue_item_id.is_some()) && position_ms >= 3_000 {
        conn.execute("UPDATE playback_state SET position_ms = 0 WHERE id = 1", [])?;
        return load_snapshot(conn);
    }

    // Locate current position by queue item id first (works for pending rows too),
    // falling back to track id for rows written before migration 021.
    let current_index = current_queue_item_id
        .and_then(|qid| queue_items.iter().position(|item| item.id == qid))
        .or_else(|| {
            current_track_id.and_then(|track_id| {
                queue_items
                    .iter()
                    .position(|item| item.track.id == track_id)
            })
        });

    if let Some(previous_item) = current_index
        .and_then(|idx| idx.checked_sub(1))
        .and_then(|idx| queue_items.get(idx))
    {
        let new_track_id: Option<i64> = if previous_item.track.id != 0 {
            Some(previous_item.track.id)
        } else {
            None
        };
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = ?1, current_queue_item_id = ?2,
                 position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![new_track_id, previous_item.id],
        )?;
        return load_snapshot(conn);
    }

    // Nothing was playing — jump to the first item rather than doing nothing.
    if current_index.is_none()
        && let Some(first_item) = queue_items.first() {
            let new_track_id: Option<i64> = if first_item.track.id != 0 {
                Some(first_item.track.id)
            } else {
                None
            };
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = ?1, current_queue_item_id = ?2,
                     position_ms = 0, is_playing = 1
                 WHERE id = 1",
                params![new_track_id, first_item.id],
            )?;
            return load_snapshot(conn);
        }

    // Already at the start of the queue — just restart position.
    conn.execute("UPDATE playback_state SET position_ms = 0 WHERE id = 1", [])?;
    load_snapshot(conn)
}

pub fn current_track_id(conn: &Connection) -> Result<Option<i64>> {
    let current_track_id = conn.query_row(
        "SELECT current_track_id FROM playback_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(current_track_id)
}

/// Returns the track that would play next **without** advancing the queue or
/// mutating any playback state. Used for gapless pre-buffering.
pub fn peek_next_track(conn: &Connection, recently_cleared: bool) -> Result<Option<Track>> {
    let (current_track_id, current_queue_item_id, repeat_mode): (Option<i64>, Option<i64>, String) =
        conn.query_row(
            "SELECT current_track_id, current_queue_item_id, repeat_mode FROM playback_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

    let queue_items = ensure_automix_queue_depth(conn, AUTOMIX_MIN_UPCOMING, recently_cleared)?;
    if queue_items.is_empty() {
        return Ok(None);
    }

    let current_index = current_track_id
        .and_then(|track_id| {
            queue_items
                .iter()
                .position(|item| item.track.id == track_id)
        })
        .or_else(|| {
            current_queue_item_id.and_then(|qid| queue_items.iter().position(|item| item.id == qid))
        });

    let next = match repeat_mode.as_str() {
        "one" => current_index
            .and_then(|idx| queue_items.get(idx))
            .or_else(|| queue_items.first()),
        _ => current_index
            .and_then(|idx| queue_items.get(idx + 1))
            .or_else(|| {
                if repeat_mode == "all" {
                    queue_items.first()
                } else {
                    None
                }
            }),
    };

    Ok(next.map(|item| item.track.clone()))
}

pub(crate) fn preferred_tidal_quality(track: &Track, user_pref: Option<AudioQuality>) -> String {
    if let Some(q) = user_pref {
        return q.as_tidal_str().to_string();
    }
    track
        .best_quality
        .clone()
        .unwrap_or_else(|| stream::DEFAULT_AUDIO_QUALITY.to_string())
}

pub fn build_tidal_stream_request(
    track: &Track,
    user_pref: Option<AudioQuality>,
) -> Option<StreamRequest> {
    track
        .tidal_id
        .map(|track_id| StreamRequest::new(track_id, preferred_tidal_quality(track, user_pref)))
}

pub fn build_playback_preparation(
    track: &Track,
    stream_info: Option<&StreamInfo>,
    crossfade_ms: i32,
    user_pref: Option<AudioQuality>,
) -> PlaybackPreparation {
    let source = build_tidal_stream_request(track, user_pref)
        .map(PlaybackSourceRequest::TidalStream)
        .unwrap_or(PlaybackSourceRequest::LocalLibrary);
    let gapless = gapless::plan_from_stream(stream_info, GaplessSettings::new(true, crossfade_ms));

    PreparedPlaybackJob::new(track.clone(), source, gapless)
}

pub fn playback_source_kind(track: &Track) -> &'static str {
    if track.tidal_id.is_some() {
        "tidal"
    } else {
        "local"
    }
}

pub fn listen_completion_threshold_ms(track: &Track) -> Option<i64> {
    track
        .duration_ms
        .map(|duration_ms| ((duration_ms as f64 * 0.9) as i64).min(240_000))
}

pub fn is_completed_listen(track: &Track, listened_ms: i64) -> bool {
    listen_completion_threshold_ms(track)
        .map(|threshold_ms| listened_ms >= threshold_ms)
        .unwrap_or(listened_ms >= 240_000)
}

pub fn ensure_automix_queue_depth(
    conn: &Connection,
    target_upcoming: usize,
    recently_cleared: bool,
) -> Result<Vec<QueueItem>> {
    let state = load_state(conn)?;
    let queue_items = queue::load_queue(conn)?;

    if !state.automix_enabled || state.repeat_mode == "one" {
        return Ok(queue_items);
    }

    // User just manually cleared the queue (within the suppression window);
    // refilling now would instantly negate that action. Caller resets the
    // window on any new user-driven play, so this only suppresses while the
    // user is actively in the "I cleared, I'm done" state.
    if recently_cleared {
        return Ok(queue_items);
    }

    let Some(current_track) = state.current_track.as_ref() else {
        return Ok(queue_items);
    };

    let current_index = queue_items
        .iter()
        .position(|item| item.track.id == current_track.id);

    // If the current track isn't found in the queue (e.g. queue was replaced or cleared),
    // treat upcoming count as 0 so automix still extends rather than bailing.
    let upcoming_count = current_index
        .map(|idx| queue_items.len().saturating_sub(idx + 1))
        .unwrap_or(0);

    if upcoming_count >= target_upcoming {
        return Ok(queue_items);
    }

    let needed = (target_upcoming - upcoming_count).max(AUTOMIX_BATCH_SIZE);
    let extension = build_automix_extension(
        conn,
        current_track,
        &queue_items,
        ShuffleMode::parse(&state.shuffle_mode),
        needed,
        state.automix_use_learning,
    )?;

    if extension.is_empty() {
        return Ok(queue_items);
    }

    queue::append_tracks(conn, &extension, "automix")?;
    queue::load_queue(conn)
}

fn build_automix_extension(
    conn: &Connection,
    current_track: &Track,
    queue_items: &[QueueItem],
    mode: ShuffleMode,
    needed: usize,
    use_learning: bool,
) -> Result<Vec<Track>> {
    if use_learning
        && let Some(model) = queries::get_active_embedding_model(conn).ok().flatten() {
            let excluded = queue_items
                .iter()
                .map(|item| item.track.id)
                .collect::<Vec<_>>();
            let neighbors = queries::get_track_neighbors(
                conn,
                model.id,
                current_track.id,
                (needed * 4).max(24) as i64,
                &excluded,
            )?;
            if !neighbors.is_empty() {
                let neighbor_ids = neighbors.iter().map(|row| row.track_id).collect::<Vec<_>>();
                let tracks = queue::get_tracks_by_ids(conn, &neighbor_ids)?;
                let track_map = tracks
                    .into_iter()
                    .map(|track| (track.id, track))
                    .collect::<HashMap<_, _>>();
                let ordered = neighbor_ids
                    .into_iter()
                    .filter_map(|track_id| track_map.get(&track_id).cloned())
                    .take(needed)
                    .collect::<Vec<_>>();
                if !ordered.is_empty() {
                    return Ok(ordered);
                }
            }
        }

    let session_profile = build_session_taste_profile(conn, current_track)?;
    let mut excluded_track_ids = queue_items
        .iter()
        .map(|item| item.track.id)
        .collect::<Vec<_>>();
    excluded_track_ids.extend(session_profile.recent_track_ids.iter().copied());
    // Convert once, after recent_track_ids has been read for exclusions, so
    // the move into TasteVector below doesn't force an extra clone.
    let (taste, seed) = from_session_profile(&session_profile);
    excluded_track_ids.sort_unstable();
    excluded_track_ids.dedup();

    // Load at most 500 candidates to keep memory bounded while still
    // providing enough diversity for scoring and genre shuffling.
    const MAX_CANDIDATES: usize = 500;

    // Preferred recall: precomputed track_similarity (co-album/artist/genre/duration).
    // Floor at `needed` so we always have at least the batch size to score and decluster;
    // below that, widen to the random pool. Library coverage is ~78% of seeds at this floor.
    let similar = queries::get_similar_tracks(
        conn,
        current_track.id,
        MAX_CANDIDATES as i64,
        &excluded_track_ids,
    )
    .unwrap_or_default();

    // Phase 2c hotfix: if we got here, the embedding fast-path produced
    // nothing usable (model missing, no neighbours, or filtered to
    // empty). If the precomputed similarity table is also empty for
    // this seed, the track has no recommendation signal at all.
    // Filling with a 500-track random library pool below would produce
    // a kitchen-sink queue that reads as "the system is broken" — most
    // commonly seen on fresh `tidal_stream` imports that haven't been
    // enriched yet. Skip the extension and let the queue end gracefully.
    // Seeds with sparse-but-non-empty signal still fall through to the
    // random pool below; only the truly-empty case is guarded.
    if similar.is_empty() {
        tracing::debug!(
            seed_track_id = current_track.id,
            "automix: skipping extension — seed has no recommendation signal (no embedding neighbours, no track_similarity rows)"
        );
        return Ok(Vec::new());
    }

    let mut candidates: Vec<Track> = if similar.len() >= needed {
        let similar_ids = similar.iter().map(|r| r.track_id).collect::<Vec<_>>();
        queue::get_tracks_by_ids(conn, &similar_ids)?
    } else {
        queries::get_tracks_excluding_with_limit(conn, &excluded_track_ids, MAX_CANDIDATES)?
    };
    if candidates.is_empty() {
        let queue_track_ids = queue_items
            .iter()
            .map(|item| item.track.id)
            .collect::<Vec<_>>();
        candidates =
            queries::get_tracks_excluding_with_limit(conn, &queue_track_ids, MAX_CANDIDATES)?;
    }

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_genres = queue::get_track_genres(conn, &candidates)?;

    // Load DSP features for seed + all candidates (ignore errors — fall back to behavioural score).
    let seed_features = queries::get_audio_dsp_features(conn, current_track.id)
        .ok()
        .flatten();
    let mut candidate_features: HashMap<i64, AudioDspFeatures> = HashMap::new();
    for track in &candidates {
        if let Ok(Some(features)) = queries::get_audio_dsp_features(conn, track.id) {
            candidate_features.insert(track.id, features);
        }
    }

    let ordered = order_automix_candidates(
        mode,
        candidates,
        &candidate_genres,
        &taste,
        &seed,
        needed,
        seed_features.as_ref(),
        &candidate_features,
    );
    let ordered = decluster_by_album(ordered);
    Ok(ordered.into_iter().take(needed).collect())
}

pub(crate) fn build_session_taste_profile(
    conn: &Connection,
    current_track: &Track,
) -> Result<SessionTasteProfile> {
    let mut profile = SessionTasteProfile {
        current_artist_id: Some(current_track.artist_id),
        current_album_id: current_track.album_id,
        current_source: Some(current_track.source.clone()),
        ..SessionTasteProfile::default()
    };

    if current_track.artist_id != 0 {
        *profile
            .positive_artists
            .entry(current_track.artist_id)
            .or_insert(0.0) += 3.0;
    }

    let current_track_genres = queue::get_track_genres(conn, std::slice::from_ref(current_track))?;
    for genre in current_track_genres
        .get(&current_track.id)
        .into_iter()
        .flat_map(|genres| genres.iter())
    {
        let normalized = normalize_genre_key(genre);
        profile.current_genres.insert(normalized.clone());
        *profile.positive_genres.entry(normalized).or_insert(0.0) += 2.2;
    }

    let mut stmt = conn.prepare(
        "SELECT track_id, completed
         FROM listen_history
         ORDER BY started_at DESC, id DESC
         LIMIT ?1",
    )?;
    let feedback_rows = stmt
        .query_map(params![SESSION_FEEDBACK_LIMIT], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut feedback_tracks = Vec::new();
    let mut feedback_entries = Vec::new();

    // Batch load all feedback tracks in a single query instead of N individual SELECTs.
    let feedback_track_ids: Vec<i64> = feedback_rows.iter().map(|(id, _)| *id).collect();
    let found_tracks = queue::get_tracks_by_ids(conn, &feedback_track_ids)?;
    let track_map: HashMap<i64, &Track> = found_tracks.iter().map(|t| (t.id, t)).collect();

    for (track_id, completed) in feedback_rows {
        profile.recent_track_ids.insert(track_id);
        if !completed {
            profile.skipped_track_ids.insert(track_id);
        }

        if let Some(track) = track_map.get(&track_id) {
            feedback_tracks.push((**track).clone());
            feedback_entries.push(((**track).clone(), completed));
        }
    }

    let feedback_genres = queue::get_track_genres(conn, &feedback_tracks)?;
    for (index, (track, completed)) in feedback_entries.iter().enumerate() {
        let recency = (SESSION_FEEDBACK_LIMIT - index as i64).max(1) as f64 / 6.0;
        let artist_weight = if *completed {
            0.8 + recency
        } else {
            1.1 + recency
        };
        let genre_weight = if *completed {
            0.7 + recency
        } else {
            1.0 + recency
        };

        let artist_buckets = if *completed {
            &mut profile.positive_artists
        } else {
            &mut profile.negative_artists
        };
        if track.artist_id != 0 {
            *artist_buckets.entry(track.artist_id).or_insert(0.0) += artist_weight;
        }

        let genre_buckets = if *completed {
            &mut profile.positive_genres
        } else {
            &mut profile.negative_genres
        };
        for genre in feedback_genres
            .get(&track.id)
            .into_iter()
            .flat_map(|genres| genres.iter())
        {
            let normalized = normalize_genre_key(genre);
            *genre_buckets.entry(normalized).or_insert(0.0) += genre_weight;
        }
    }

    Ok(profile)
}

// Eight inputs is one over clippy's default threshold, but `taste` and `seed`
// represent distinct concepts (per-user preference vs per-query seed track)
// and bundling them just to satisfy the lint would obscure that.
#[allow(clippy::too_many_arguments)]
fn order_automix_candidates(
    mode: ShuffleMode,
    candidates: Vec<Track>,
    candidate_genres: &HashMap<i64, Vec<String>>,
    taste: &TasteVector,
    seed: &SeedContext,
    needed: usize,
    seed_features: Option<&AudioDspFeatures>,
    candidate_features: &HashMap<i64, AudioDspFeatures>,
) -> Vec<Track> {
    let mut scored = candidates
        .into_iter()
        .map(|track| {
            let score = automix_score(
                &track,
                candidate_genres
                    .get(&track.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                taste,
                seed,
                seed_features,
                candidate_features.get(&track.id),
            );
            ScoredTrack { track, score }
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.track.title.cmp(&right.track.title))
    });

    match mode {
        ShuffleMode::Off => scored.into_iter().map(|entry| entry.track).collect(),
        ShuffleMode::True => {
            let pool_size = (needed * TRUE_SHUFFLE_POOL_MULTIPLIER).max(48);
            let pool = scored
                .into_iter()
                .take(pool_size)
                .map(|entry| entry.track)
                .collect::<Vec<_>>();
            true_shuffle(&pool)
        }
        ShuffleMode::Weighted => {
            let pool_size = (needed * TRUE_SHUFFLE_POOL_MULTIPLIER).max(48);
            let pool = scored.into_iter().take(pool_size).collect::<Vec<_>>();
            weighted_session_shuffle(&pool)
        }
        ShuffleMode::Genre => {
            let mut preferred = Vec::new();
            let mut fallback = Vec::new();

            for entry in scored {
                let genres = candidate_genres
                    .get(&entry.track.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if matches_preferred_genres(genres, taste, seed) {
                    preferred.push(entry.track);
                } else {
                    fallback.push(entry.track);
                }
            }

            // Interleave preferred and fallback at ~3:1 ratio so the queue
            // never becomes a solid wall of one genre type, but still leans
            // toward the current session's taste.
            let preferred_shuffled = genre_shuffle(&preferred, candidate_genres);
            let fallback_shuffled = genre_shuffle(&fallback, candidate_genres);
            let total = preferred_shuffled.len() + fallback_shuffled.len();
            let mut ordered = Vec::with_capacity(total);
            let mut pi = 0usize;
            let mut fi = 0usize;
            let mut streak = 0usize;
            while pi < preferred_shuffled.len() || fi < fallback_shuffled.len() {
                let take_pref =
                    pi < preferred_shuffled.len() && (fi >= fallback_shuffled.len() || streak < 3);
                if take_pref {
                    ordered.push(preferred_shuffled[pi].clone());
                    pi += 1;
                    streak += 1;
                } else {
                    ordered.push(fallback_shuffled[fi].clone());
                    fi += 1;
                    streak = 0;
                }
            }
            ordered
        }
    }
}

/// Spread tracks from the same album apart so they don't run consecutively.
/// Preserves the score ordering as much as possible while ensuring no two
/// adjacent tracks share the same album_id.
/// Uses a visited-set pattern instead of Vec::remove to avoid O(n²).
fn decluster_by_album(tracks: Vec<Track>) -> Vec<Track> {
    if tracks.len() <= 1 {
        return tracks;
    }
    let mut result = Vec::with_capacity(tracks.len());
    let mut visited = vec![false; tracks.len()];
    let mut last_album: Option<i64> = None;

    for _ in 0..tracks.len() {
        let pos = if let Some(last_id) = last_album {
            tracks
                .iter()
                .enumerate()
                .position(|(i, t)| !visited[i] && (t.album_id != Some(last_id)))
                .unwrap_or_else(|| {
                    // Fallback: pick the first unvisited track.
                    tracks
                        .iter()
                        .enumerate()
                        .find(|(i, _)| !visited[*i])
                        .map(|(i, _)| i)
                        .unwrap_or(0)
                })
        } else {
            0
        };
        visited[pos] = true;
        last_album = tracks[pos].album_id;
        result.push(tracks[pos].clone());
    }
    result
}

fn automix_score(
    track: &Track,
    genres: &[String],
    taste: &TasteVector,
    seed: &SeedContext,
    seed_features: Option<&AudioDspFeatures>,
    candidate_features: Option<&AudioDspFeatures>,
) -> f64 {
    let mut score = 1.0;

    // Hard suppression for recently skipped tracks
    if taste.skipped_track_ids.contains(&track.id) {
        score *= 0.1;
    }

    // Same-artist: gentle familiarity boost, not enough to cause artist runs.
    // Artist spread is handled at the queue level by decluster_by_album.
    if Some(track.artist_id) == seed.artist_id && track.artist_id != 0 {
        score *= 1.1;
    }

    if seed.source.as_deref() == Some(track.source.as_str()) {
        score *= 1.05;
    }

    if track.is_favorite {
        score *= 1.2;
    }

    // Unplayed tracks get a meaningful boost so they surface before heavily-played ones.
    if track.play_count == 0 {
        score *= 1.35;
    } else if let Some(last_played) = track.last_played_at.as_deref() {
        // Time-decay penalty: full suppression at <1 day, fades to zero by 14 days.
        let days_since = parse_days_since_last_played(last_played);
        if days_since < 14.0 {
            let penalty = 0.5 + 0.5 * (days_since / 14.0);
            score *= penalty;
        }
    }

    if track.artist_id != 0
        && let Some(affinity) = taste.artist_affinity.get(&track.artist_id) {
            score += affinity.pos * 0.5;
            score -= affinity.neg * 0.65;
        }

    let normalized_genres = genres.iter().map(|genre| normalize_genre_key(genre));
    for genre in normalized_genres {
        if seed.genres.contains(&genre) {
            score += 1.8;
        }
        if let Some(affinity) = taste.genre_affinity.get(&genre) {
            score += affinity.pos * 0.4;
            score -= affinity.neg * 0.5;
        }
    }

    score += (track.fidelity_score.max(0) as f64) * 0.003;

    // DSP harmonic/BPM/energy scoring — only applied when BOTH tracks have features.
    // Unanalyzed tracks are never penalised; they simply skip this pass.
    if let (Some(seed), Some(cand)) = (seed_features, candidate_features) {
        // Camelot + BPM multiplier (shared with radio post-scoring).
        score *= compute_harmonic_multiplier(
            seed.camelot_key.as_deref(),
            cand.camelot_key.as_deref(),
            seed.bpm,
            cand.bpm,
        );

        // Energy whiplash penalty.
        if let (Some(seed_energy), Some(cand_energy)) = (seed.energy, cand.energy)
            && (seed_energy - cand_energy).abs() > 0.5 {
                score *= 0.7;
            }
    }

    score.max(0.05)
}

/// Parse an ISO-8601 timestamp and return days elapsed since then.
/// Returns `f64::MAX` on failure so malformed timestamps get maximum recency penalty.
fn parse_days_since_last_played(timestamp: &str) -> f64 {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        return f64::MAX;
    };
    let elapsed = chrono::Utc::now().signed_duration_since(dt.with_timezone(&chrono::Utc));
    elapsed.num_seconds().max(0) as f64 / 86_400.0
}

fn matches_preferred_genres(genres: &[String], taste: &TasteVector, seed: &SeedContext) -> bool {
    genres
        .iter()
        .map(|genre| normalize_genre_key(genre))
        .any(|genre| {
            seed.genres.contains(&genre)
                || taste
                    .genre_affinity
                    .get(&genre)
                    .is_some_and(|affinity| affinity.pos > 0.0)
        })
}

fn weighted_session_shuffle(entries: &[ScoredTrack]) -> Vec<Track> {
    let profile = WeightedShuffleProfile::default();
    let mut rng = rand::thread_rng();
    let mut weighted = entries
        .iter()
        .map(|entry| {
            let weight = profile.weight_for(&entry.track) * entry.score.max(0.05);
            let uniform = rng.gen_range(f64::EPSILON..1.0);
            let key = -uniform.ln() / weight;
            (key, entry.track.clone())
        })
        .collect::<Vec<_>>();

    weighted.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
    weighted.into_iter().map(|(_, track)| track).collect()
}

fn normalize_genre_key(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, artwork_url TEXT);
            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                disc_number INTEGER,
                track_number INTEGER,
                duration_ms INTEGER,
                isrc TEXT,
                tidal_id INTEGER,
                ytmusic_id TEXT,
                soundcloud_id INTEGER,
                best_quality TEXT,
                best_source TEXT,
                fidelity_score INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played_at TEXT,
                date_added TEXT,
                source TEXT DEFAULT 'tidal'
            );
            CREATE TABLE queue (
                id               INTEGER PRIMARY KEY,
                track_id         INTEGER,
                position         INTEGER NOT NULL,
                source           TEXT    DEFAULT 'user',
                reason           TEXT,
                pending_artist   TEXT,
                pending_title    TEXT,
                pending_at       TIMESTAMP,
                resolving_at     TIMESTAMP,
                resolved_at      TIMESTAMP,
                tidal_match_score REAL,
                tidal_id_hint    INTEGER
            );
            CREATE TABLE genres (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                parent_id INTEGER
            );
            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT,
                confidence REAL DEFAULT 1.0
            );
            CREATE TABLE listen_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                duration_listened_ms INTEGER DEFAULT 0,
                completed INTEGER DEFAULT 0
            );
            CREATE TABLE embedding_models (
                id INTEGER PRIMARY KEY,
                model_key TEXT NOT NULL UNIQUE,
                family TEXT NOT NULL,
                dimension INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                is_active INTEGER NOT NULL DEFAULT 0,
                trained_at TEXT,
                config_json TEXT,
                metrics_json TEXT,
                created_at TEXT
            );
            CREATE TABLE track_similarity (
                track_a INTEGER NOT NULL,
                track_b INTEGER NOT NULL,
                similarity_score REAL NOT NULL DEFAULT 0,
                co_listen_score REAL DEFAULT 0,
                co_album_score REAL DEFAULT 0,
                co_artist_score REAL DEFAULT 0,
                genre_proximity REAL DEFAULT 0,
                duration_proximity REAL DEFAULT 0,
                era_proximity REAL DEFAULT 0,
                computed_at TEXT,
                PRIMARY KEY (track_a, track_b)
            );
            CREATE TABLE playback_state (
                id INTEGER PRIMARY KEY,
                current_track_id INTEGER,
                current_queue_item_id INTEGER,
                position_ms INTEGER NOT NULL DEFAULT 0,
                is_playing INTEGER NOT NULL DEFAULT 0,
                volume REAL NOT NULL DEFAULT 1.0,
                shuffle_mode TEXT NOT NULL DEFAULT 'off',
                repeat_mode TEXT NOT NULL DEFAULT 'off',
                automix_enabled INTEGER NOT NULL DEFAULT 0,
                crossfade_ms INTEGER NOT NULL DEFAULT 0,
                automix_discover_new INTEGER NOT NULL DEFAULT 0,
                automix_use_learning INTEGER NOT NULL DEFAULT 1,
                automix_allow_external INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .unwrap();

        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])
            .unwrap();
        for id in 1..=6 {
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc,
                    tidal_id, ytmusic_id, soundcloud_id, best_quality, best_source, fidelity_score,
                    is_favorite, play_count, last_played_at, date_added, source
                ) VALUES (?1, ?2, 1, NULL, 1, ?1, 180000, NULL, ?1, NULL, NULL, 'LOSSLESS', 'tidal', 10, 0, 0, NULL, '2025-01-01', 'tidal')",
                params![id, format!("Track {id}")],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO playback_state (
                id, current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms
            ) VALUES (1, NULL, 0, 0, 1.0, 'off', 'off', 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO track_similarity (track_a, track_b, similarity_score, co_artist_score)
             VALUES (2, 3, 0.95, 1.0)",
            [],
        )
        .unwrap();

        conn
    }

    fn load_tracks(conn: &Connection, ids: &[i64]) -> Vec<Track> {
        ids.iter()
            .map(|id| queue::get_track_by_id(conn, *id).unwrap().unwrap())
            .collect()
    }

    fn track_with_tidal_id(id: i64, tidal_id: Option<i64>, quality: Option<&str>) -> Track {
        Track {
            id,
            title: format!("Track {id}"),
            artist_id: 1,
            artist_name: Some("A".to_string()),
            album_id: None,
            album_title: None,
            disc_number: Some(1),
            track_number: Some(id as i32),
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: quality.map(|s| s.to_string()),
            best_source: tidal_id.map(|_| "tidal".to_string()),
            fidelity_score: 10,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: Some("2025-01-01".to_string()),
            source: if tidal_id.is_some() {
                "tidal".to_string()
            } else {
                "local".to_string()
            },
            artwork_url: None,
        }
    }

    #[test]
    fn previous_track_moves_back_when_under_threshold() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 2, position_ms = 2500, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = previous_track(&conn).unwrap();

        assert_eq!(snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(snapshot.state.position_ms, 0);
        assert!(snapshot.state.is_playing);
    }

    #[test]
    fn previous_track_restarts_current_track_when_over_threshold() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 2, position_ms = 3000, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = previous_track(&conn).unwrap();

        assert_eq!(snapshot.state.current_track.unwrap().id, 2);
        assert_eq!(snapshot.state.position_ms, 0);
    }

    #[test]
    fn previous_track_restarts_first_track_when_no_previous_exists() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, position_ms = 1000, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = previous_track(&conn).unwrap();

        assert_eq!(snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(snapshot.state.position_ms, 0);
    }

    #[test]
    fn previous_track_selects_first_queue_item_when_nothing_is_playing() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();

        let snapshot = previous_track(&conn).unwrap();

        assert_eq!(snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(snapshot.state.position_ms, 0);
        assert!(snapshot.state.is_playing);
    }

    #[test]
    fn previous_track_clears_state_when_queue_is_empty() {
        let conn = conn();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, position_ms = 1500, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = previous_track(&conn).unwrap();

        assert!(snapshot.state.current_track.is_none());
        assert_eq!(snapshot.state.position_ms, 0);
        assert!(!snapshot.state.is_playing);
        assert!(snapshot.queue.is_empty());
    }

    #[test]
    fn build_tidal_stream_request_uses_track_quality_or_defaults() {
        let track = track_with_tidal_id(42, Some(88), Some("HI_RES_LOSSLESS"));

        let request = build_tidal_stream_request(&track, None).unwrap();

        assert_eq!(request.track_id, 88);
        assert_eq!(request.audio_quality, "HI_RES_LOSSLESS");
        assert_eq!(request.playback_mode, "STREAM");
        assert_eq!(request.asset_presentation, "FULL");
    }

    #[test]
    fn build_playback_preparation_marks_local_tracks_as_local_library() {
        let track = track_with_tidal_id(7, None, None);

        let prep = build_playback_preparation(&track, None, 1500, None);

        assert!(prep.is_local());
        assert_eq!(prep.source_kind(), PlaybackSourceKind::LocalLibrary);
        assert!(prep.stream_request().is_none());
        assert!(!prep.gapless.enabled);
        assert_eq!(prep.track.id, 7);
    }

    #[test]
    fn build_playback_preparation_includes_tidal_stream_request() {
        let track = track_with_tidal_id(7, Some(77), Some("LOSSLESS"));
        let stream = StreamInfo {
            url: "https://example.com/stream.flac".to_string(),
            segment_urls: vec![],
            track_id: 77,
            audio_quality: "LOSSLESS".to_string(),
            codec: "audio/flac".to_string(),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
        };

        let prep = build_playback_preparation(&track, Some(&stream), 1500, None);

        assert!(prep.is_tidal());
        assert_eq!(prep.source_kind(), PlaybackSourceKind::TidalStream);
        let request = prep.stream_request().expect("expected a tidal request");
        assert_eq!(request.track_id, 77);
        assert_eq!(request.audio_quality, "LOSSLESS");
        assert!(prep.gapless.enabled);
        assert_eq!(prep.gapless.overlap_ms, 1500);
    }

    #[test]
    fn completed_listen_uses_ninety_percent_or_four_minute_cap() {
        let short_track = track_with_tidal_id(1, Some(42), Some("LOSSLESS"));
        assert!(is_completed_listen(&short_track, 162_000));
        assert!(!is_completed_listen(&short_track, 161_999));

        let long_track = Track {
            duration_ms: Some(600_000),
            ..track_with_tidal_id(2, Some(99), Some("LOSSLESS"))
        };
        assert!(is_completed_listen(&long_track, 240_000));
        assert!(!is_completed_listen(&long_track, 239_999));
    }

    #[test]
    fn next_track_extends_queue_when_automix_is_enabled() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = next_track(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.unwrap().id, 3);
        assert!(snapshot.queue.len() > 2);
        assert!(snapshot.queue.iter().any(|item| item.source == "automix"));
    }

    #[test]
    fn peek_next_track_can_see_generated_automix_track() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            [],
        )
        .unwrap();

        let next = peek_next_track(&conn, false)
            .unwrap()
            .expect("generated automix track");

        assert_eq!(next.id, 3);
        let queue_items = queue::load_queue(&conn).unwrap();
        assert!(queue_items.len() > 2);
    }

    #[test]
    fn ensure_automix_queue_depth_suppresses_refill_when_recently_cleared() {
        // Same setup as `next_track_extends_queue_when_automix_is_enabled`:
        // two tracks queued, automix on, current = 2. The non-suppressed
        // refill path is already covered by that sibling test; here we
        // verify the new gate alone — that with `recently_cleared = true`
        // the helper short-circuits before any extension work and returns
        // the existing queue unmodified.
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            [],
        )
        .unwrap();

        // Suppression on — must return the 2 existing items, never call the
        // extension path (which would otherwise touch tables not present in
        // this minimal test schema and panic).
        let suppressed = ensure_automix_queue_depth(&conn, AUTOMIX_MIN_UPCOMING, true).unwrap();
        assert_eq!(
            suppressed.len(),
            2,
            "suppressed call must not extend the queue"
        );
        assert!(
            !suppressed.iter().any(|item| item.source == "automix"),
            "suppressed call must not append automix rows"
        );
        let stored = queue::load_queue(&conn).unwrap();
        assert_eq!(
            stored.len(),
            2,
            "DB queue must be untouched while suppressed"
        );
    }

    #[test]
    fn play_track_now_sets_current_queue_item_id() {
        let conn = conn();
        // Seed two queue rows pointing at the same track so the "lowest
        // position" tiebreak is testable.
        let tracks = load_tracks(&conn, &[1, 1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let q = queue::load_queue(&conn).unwrap();

        play_track_now(&conn, 1).unwrap();
        let state = load_state(&conn).unwrap();
        assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
        // Of the two rows pointing at track 1, the lowest-position one wins.
        let expected_qid = q.iter().find(|i| i.track.id == 1).unwrap().id;
        assert_eq!(state.current_queue_item_id, Some(expected_qid));
        assert!(state.is_playing);
    }

    #[test]
    fn lookup_listen_source_maps_radio_pending_and_automix_new() {
        let conn = conn();
        for (source, expected) in [
            ("radio_pending", crate::db::models::ListenSource::Radio),
            ("automix-new", crate::db::models::ListenSource::Automix),
        ] {
            conn.execute("DELETE FROM queue", []).unwrap();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, ?1)",
                params![source],
            )
            .unwrap();
            let qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1
                 WHERE id = 1",
                params![qid],
            )
            .unwrap();

            assert_eq!(lookup_current_listen_source(&conn), expected);
        }
    }

    #[test]
    fn next_track_starts_first_queue_item_when_no_current_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();

        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = next_track(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(2));
        assert_eq!(
            snapshot.state.current_queue_item_id,
            snapshot.queue.first().map(|item| item.id)
        );
        assert!(snapshot.state.is_playing);
    }

    #[test]
    fn start_queue_from_beginning_ignores_previous_current_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let snapshot = start_queue_from_beginning(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(1));
        assert_eq!(
            snapshot.state.current_queue_item_id,
            Some(queue_items[0].id)
        );
        assert!(snapshot.state.is_playing);
    }

    #[test]
    fn reconcile_advances_current_when_deleted() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let queue_after_seed = queue::load_queue(&conn).unwrap();
        // Mark t1 as current.
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_after_seed[0].id],
        )
        .unwrap();

        let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
        assert!(outcome.queue_changed);
        assert!(outcome.current_changed);
        assert!(!outcome.stopped_playback);
        assert_eq!(outcome.new_current_track_id, Some(2));

        let state = load_state(&conn).unwrap();
        assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
        assert!(state.is_playing);

        let remaining = queue::load_queue(&conn).unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].position, 0);
        assert_eq!(remaining[1].position, 1);
    }

    #[test]
    fn reconcile_stops_playback_when_no_survivors() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let q = queue::load_queue(&conn).unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![q[0].id],
        )
        .unwrap();

        let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
        assert!(outcome.queue_changed);
        assert!(outcome.current_changed);
        assert!(outcome.stopped_playback);
        assert_eq!(outcome.new_current_track_id, None);

        let state = load_state(&conn).unwrap();
        assert!(state.current_track.is_none());
        assert!(!state.is_playing);
    }

    #[test]
    fn reconcile_skips_pending_rows() {
        let conn = conn();
        // Seed: one library row (track 1, current) followed by a pending row.
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
             VALUES (NULL, 1, 'radio_pending', 'Pending Artist', 'Pending Title', datetime('now'))",
            [],
        )
        .unwrap();
        let q = queue::load_queue(&conn).unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![q[0].id],
        )
        .unwrap();

        let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
        assert!(outcome.current_changed);
        assert!(!outcome.stopped_playback);
        // Pending rows always survive — they don't reference local track IDs.
        assert_eq!(outcome.new_current_track_id, None);

        let remaining = queue::load_queue(&conn).unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].is_pending);
    }

    #[test]
    fn reconcile_noop_when_current_not_in_deleted_set() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let q = queue::load_queue(&conn).unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![q[0].id],
        )
        .unwrap();

        // Delete track 3 — current (track 1) is unaffected.
        let outcome = reconcile_after_track_delete(&conn, &[3]).unwrap();
        assert!(outcome.queue_changed);
        assert!(!outcome.current_changed);
        assert!(!outcome.stopped_playback);
        assert_eq!(outcome.new_current_track_id, Some(1));

        let state = load_state(&conn).unwrap();
        assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
    }

    /// Build an isolated DB fixture with the full surface
    /// `build_automix_extension` needs (the standard `conn()` helper
    /// above lacks `embedding_models`, `track_embeddings`, and
    /// `track_similarity`). Returns a connection with one seed track
    /// inserted but **no** embedding row and **no** similarity rows —
    /// the "no recommendation signal" case the guard targets.
    fn empty_signal_conn() -> (Connection, Track) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, artwork_url TEXT);
            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                disc_number INTEGER,
                track_number INTEGER,
                duration_ms INTEGER,
                isrc TEXT,
                tidal_id INTEGER,
                ytmusic_id TEXT,
                soundcloud_id INTEGER,
                best_quality TEXT,
                best_source TEXT,
                fidelity_score INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played_at TEXT,
                date_added TEXT,
                source TEXT DEFAULT 'tidal'
            );
            CREATE TABLE queue (
                id               INTEGER PRIMARY KEY,
                track_id         INTEGER,
                position         INTEGER NOT NULL,
                source           TEXT    DEFAULT 'user',
                reason           TEXT,
                pending_artist   TEXT,
                pending_title    TEXT,
                pending_at       TIMESTAMP,
                resolving_at     TIMESTAMP,
                resolved_at      TIMESTAMP,
                tidal_match_score REAL,
                tidal_id_hint    INTEGER
            );
            CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL, parent_id INTEGER);
            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT,
                confidence REAL DEFAULT 1.0
            );
            CREATE TABLE listen_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                duration_listened_ms INTEGER DEFAULT 0,
                completed INTEGER DEFAULT 0
            );
            CREATE TABLE embedding_models (
                id INTEGER PRIMARY KEY,
                model_key TEXT NOT NULL UNIQUE,
                family TEXT NOT NULL,
                dimension INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                is_active INTEGER NOT NULL DEFAULT 0,
                trained_at TEXT,
                config_json TEXT,
                metrics_json TEXT,
                created_at TEXT
            );
            CREATE TABLE track_embeddings (
                model_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                vector_blob BLOB NOT NULL,
                PRIMARY KEY (model_id, track_id)
            );
            CREATE TABLE track_similarity (
                track_a INTEGER NOT NULL,
                track_b INTEGER NOT NULL,
                similarity_score REAL NOT NULL DEFAULT 0,
                co_listen_score REAL DEFAULT 0,
                co_album_score REAL DEFAULT 0,
                co_artist_score REAL DEFAULT 0,
                genre_proximity REAL DEFAULT 0,
                duration_proximity REAL DEFAULT 0,
                era_proximity REAL DEFAULT 0,
                computed_at TEXT,
                PRIMARY KEY (track_a, track_b)
            );
            ",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Unenriched Artist')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, source, fidelity_score)
             VALUES (1, 'Fresh Tidal Import', 1, 'tidal_stream', 0)",
            [],
        )
        .unwrap();

        let track = queue::get_track_by_id(&conn, 1).unwrap().unwrap();
        (conn, track)
    }

    /// Phase 2c hotfix: when both the embedding fast-path AND the
    /// precomputed similarity table are empty for a seed,
    /// `build_automix_extension` must return `Vec::new()` rather than
    /// falling back to a 500-track random library pool.
    ///
    /// Reproduces the Amy Shark "I Said Hi" symptom from the
    /// diagnostic: fresh `tidal_stream` import, no enrichment, no
    /// similarity rows, no embedding. Pre-fix behaviour was a queue
    /// full of unrelated library tracks (Mac Miller, Bob Marley,
    /// James Brown, etc.); post-fix the queue ends gracefully.
    #[test]
    fn build_automix_extension_returns_empty_when_seed_has_no_signal() {
        let (conn, seed) = empty_signal_conn();
        let extension = build_automix_extension(
            &conn,
            &seed,
            &[], // empty queue
            ShuffleMode::Off,
            12,   // typical needed
            true, // use_learning enabled — fast-path will run, find no model, fall through
        )
        .expect("extension call");
        assert!(
            extension.is_empty(),
            "expected empty extension for seed with no signal, got {} tracks",
            extension.len()
        );
    }

    /// Same fixture but with one similarity row for the seed → the
    /// guard should NOT fire. We deliberately don't assert on the
    /// extension's exact contents (that depends on scoring), only on
    /// the fact that the guard's early-return path didn't engage.
    /// Sparse-but-non-empty signal is the documented "still falls
    /// through to random pool below" case.
    #[test]
    fn build_automix_extension_does_not_skip_when_seed_has_some_signal() {
        let (conn, seed) = empty_signal_conn();
        // Add a second track + one similarity row from seed (id=1) to it.
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, source, fidelity_score)
             VALUES (2, 'Other Track', 1, 'tidal_stream', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO track_similarity (track_a, track_b, similarity_score)
             VALUES (1, 2, 0.5)",
            [],
        )
        .unwrap();

        let extension = build_automix_extension(&conn, &seed, &[], ShuffleMode::Off, 12, true)
            .expect("extension call");

        // We don't pin contents — only that the empty-signal guard
        // didn't bail. Sparse-signal seeds still walk through the
        // random-pool path which is intended legacy behaviour.
        assert!(
            !extension.is_empty(),
            "expected non-empty extension once a similarity row exists"
        );
    }
}

#[cfg(test)]
mod quality_precedence_tests {
    use super::*;
    use crate::db::audio_settings::AudioQuality;

    fn track_with_best(best: Option<&str>) -> Track {
        Track {
            id: 1,
            title: "T".to_string(),
            artist_id: 1,
            artist_name: None,
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: None,
            isrc: None,
            tidal_id: Some(1),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: best.map(String::from),
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

    #[test]
    fn user_pref_overrides_track_best_quality() {
        let t = track_with_best(Some("HI_RES_LOSSLESS"));
        let got = preferred_tidal_quality(&t, Some(AudioQuality::Lossless));
        assert_eq!(got, "LOSSLESS");
    }

    #[test]
    fn falls_back_to_track_when_no_user_pref() {
        let t = track_with_best(Some("HI_RES_LOSSLESS"));
        let got = preferred_tidal_quality(&t, None);
        assert_eq!(got, "HI_RES_LOSSLESS");
    }

    #[test]
    fn falls_back_to_default_when_neither_set() {
        let t = track_with_best(None);
        let got = preferred_tidal_quality(&t, None);
        assert_eq!(got, crate::services::tidal::stream::DEFAULT_AUDIO_QUALITY);
    }
}

#[cfg(test)]
mod parity_tests {
    //! Parity gate for the TasteVector migration (Phase 1).
    //!
    //! Runs the live `automix_score` against a frozen snapshot
    //! (`automix_score_old`) on a fixed synthetic fixture and asserts the
    //! top-30 candidate ordering matches exactly. In the scaffolding commit
    //! the snapshot is byte-identical to the live function so the gate
    //! passes trivially; in the migration commit the live function is
    //! rewritten to consume `TasteVector`/`SeedContext` while the snapshot
    //! stays frozen, so divergence indicates a migration bug rather than a
    //! tuning miss.
    //!
    //! Fixture is built in-memory with no database — `automix_score` only
    //! consumes plain data, so a DB round-trip would add noise without
    //! adding signal.
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Frozen snapshot of `automix_score` taken at Phase 1 start. The body
    /// is a verbatim copy of `super::automix_score` and must not be
    /// modified by the migration commit — the whole point is that this
    /// path keeps producing the original numbers while the live function
    /// changes shape underneath it.
    fn automix_score_old(
        track: &Track,
        genres: &[String],
        taste: &SessionTasteProfile,
        seed_features: Option<&AudioDspFeatures>,
        candidate_features: Option<&AudioDspFeatures>,
    ) -> f64 {
        let mut score = 1.0;

        if taste.skipped_track_ids.contains(&track.id) {
            score *= 0.1;
        }

        if Some(track.artist_id) == taste.current_artist_id && track.artist_id != 0 {
            score *= 1.1;
        }

        if taste.current_source.as_deref() == Some(track.source.as_str()) {
            score *= 1.05;
        }

        if track.is_favorite {
            score *= 1.2;
        }

        if track.play_count == 0 {
            score *= 1.35;
        } else if let Some(last_played) = track.last_played_at.as_deref() {
            let days_since = parse_days_since_last_played(last_played);
            if days_since < 14.0 {
                let penalty = 0.5 + 0.5 * (days_since / 14.0);
                score *= penalty;
            }
        }

        if track.artist_id != 0 {
            score += taste
                .positive_artists
                .get(&track.artist_id)
                .copied()
                .unwrap_or(0.0)
                * 0.5;
            score -= taste
                .negative_artists
                .get(&track.artist_id)
                .copied()
                .unwrap_or(0.0)
                * 0.65;
        }

        let normalized_genres = genres.iter().map(|genre| normalize_genre_key(genre));
        for genre in normalized_genres {
            if taste.current_genres.contains(&genre) {
                score += 1.8;
            }
            score += taste.positive_genres.get(&genre).copied().unwrap_or(0.0) * 0.4;
            score -= taste.negative_genres.get(&genre).copied().unwrap_or(0.0) * 0.5;
        }

        score += (track.fidelity_score.max(0) as f64) * 0.003;

        if let (Some(seed), Some(cand)) = (seed_features, candidate_features) {
            score *= compute_harmonic_multiplier(
                seed.camelot_key.as_deref(),
                cand.camelot_key.as_deref(),
                seed.bpm,
                cand.bpm,
            );

            if let (Some(seed_energy), Some(cand_energy)) = (seed.energy, cand.energy) {
                if (seed_energy - cand_energy).abs() > 0.5 {
                    score *= 0.7;
                }
            }
        }

        score.max(0.05)
    }

    struct CandidateInput {
        track: Track,
        genres: Vec<String>,
        features: Option<AudioDspFeatures>,
    }

    struct Fixture {
        profile: SessionTasteProfile,
        seed_features: Option<AudioDspFeatures>,
        candidates: Vec<CandidateInput>,
    }

    fn make_track(
        id: i64,
        artist_id: i64,
        is_favorite: bool,
        play_count: i32,
        fidelity_score: i32,
        source: &str,
    ) -> Track {
        Track {
            id,
            title: format!("Track {id}"),
            artist_id,
            artist_name: Some(format!("Artist {artist_id}")),
            album_id: Some(artist_id * 10),
            album_title: Some(format!("Album {artist_id}")),
            disc_number: Some(1),
            track_number: Some(1),
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: Some(id),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some(source.to_string()),
            fidelity_score,
            is_favorite,
            play_count,
            last_played_at: None,
            date_added: Some("2025-01-01T00:00:00Z".to_string()),
            source: source.to_string(),
            artwork_url: None,
        }
    }

    fn make_features(camelot: &str, bpm: f64, energy: f64) -> AudioDspFeatures {
        AudioDspFeatures {
            track_id: 0,
            bpm: Some(bpm),
            key_signature: None,
            camelot_key: Some(camelot.to_string()),
            loudness_lufs: None,
            energy: Some(energy),
            danceability: None,
            beat_strength: None,
            spectral_centroid: None,
            stereo_width: None,
            is_instrumental: false,
            analysis_source: "test".to_string(),
            analysis_offset_ms: 0,
            samples_analyzed: None,
            analyzed_at: "2026-01-01T00:00:00Z".to_string(),
            analysis_version: "test".to_string(),
        }
    }

    /// Synthetic fixture exercising every branch in `automix_score`:
    /// hard suppression (skipped track), same-artist boost, same-source
    /// boost, favourite multiplier, unplayed boost, positive/negative
    /// artist signal, positive/negative genre signal, current-genre
    /// proximity bonus, fidelity tilt, harmonic multiplier, and energy
    /// whiplash penalty.
    fn build_fixture() -> Fixture {
        let mut profile = SessionTasteProfile {
            current_artist_id: Some(1),
            current_album_id: Some(10),
            current_source: Some("tidal".to_string()),
            ..SessionTasteProfile::default()
        };
        profile.current_genres.insert("house".to_string());
        for (id, weight) in [(1, 8.5_f64), (2, 5.2), (3, 3.0), (4, 1.5), (5, 0.8)] {
            profile.positive_artists.insert(id, weight);
        }
        for (id, weight) in [(2, 1.1_f64), (3, 2.4)] {
            profile.negative_artists.insert(id, weight);
        }
        for (genre, weight) in [("house", 6.4_f64), ("techno", 4.2), ("ambient", 2.0)] {
            profile.positive_genres.insert(genre.to_string(), weight);
        }
        for (genre, weight) in [("jazz", 1.8_f64), ("ambient", 0.4)] {
            profile.negative_genres.insert(genre.to_string(), weight);
        }
        for id in [300, 301, 302, 305] {
            profile.recent_track_ids.insert(id);
        }
        profile.skipped_track_ids.insert(305);

        let seed_features = Some(make_features("8A", 124.0, 0.7));

        let candidates = vec![
            CandidateInput {
                track: make_track(100, 1, true, 0, 80, "tidal"),
                genres: vec!["House".to_string()],
                features: Some(make_features("8A", 125.0, 0.65)),
            },
            CandidateInput {
                track: make_track(101, 1, false, 2, 60, "tidal"),
                genres: vec!["Techno".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(102, 2, false, 0, 70, "tidal"),
                genres: vec!["House".to_string(), "Techno".to_string()],
                features: Some(make_features("9A", 128.0, 0.75)),
            },
            CandidateInput {
                track: make_track(103, 3, false, 5, 40, "tidal"),
                genres: vec!["Jazz".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(104, 4, true, 0, 50, "tidal"),
                genres: vec!["Ambient".to_string()],
                features: Some(make_features("3B", 110.0, 0.2)),
            },
            CandidateInput {
                track: make_track(105, 5, false, 1, 30, "tidal"),
                genres: vec!["House".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(106, 6, false, 0, 70, "tidal"),
                genres: vec!["Techno".to_string()],
                features: Some(make_features("8A", 124.0, 0.7)),
            },
            CandidateInput {
                track: make_track(107, 7, false, 3, 20, "local"),
                genres: vec!["Jazz".to_string(), "Ambient".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(108, 2, false, 0, 10, "tidal"),
                genres: vec![],
                features: None,
            },
            CandidateInput {
                track: make_track(109, 1, false, 0, 90, "tidal"),
                genres: vec!["House".to_string(), "Ambient".to_string()],
                features: Some(make_features("2A", 130.0, 0.85)),
            },
            CandidateInput {
                track: make_track(110, 3, true, 4, 85, "tidal"),
                genres: vec!["Techno".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(305, 4, false, 0, 60, "tidal"),
                genres: vec!["House".to_string()],
                features: None,
            },
        ];

        Fixture {
            profile,
            seed_features,
            candidates,
        }
    }

    /// Builds a `TasteVector` + `SeedContext` from the fixture's
    /// `SessionTasteProfile` via `from_session_profile`, then calls the
    /// migrated `automix_score`. Conversion happens once per call (not per
    /// candidate) to mirror the production call site.
    fn score_with_new_path(fixture: &Fixture) -> Vec<(i64, f64)> {
        let (taste, seed) =
            crate::smart::taste_vector::adapters::from_session_profile(&fixture.profile);
        fixture
            .candidates
            .iter()
            .map(|cand| {
                let score = automix_score(
                    &cand.track,
                    &cand.genres,
                    &taste,
                    &seed,
                    fixture.seed_features.as_ref(),
                    cand.features.as_ref(),
                );
                (cand.track.id, score)
            })
            .collect()
    }

    fn score_with_old_path(fixture: &Fixture) -> Vec<(i64, f64)> {
        fixture
            .candidates
            .iter()
            .map(|cand| {
                let score = automix_score_old(
                    &cand.track,
                    &cand.genres,
                    &fixture.profile,
                    fixture.seed_features.as_ref(),
                    cand.features.as_ref(),
                );
                (cand.track.id, score)
            })
            .collect()
    }

    fn top_n_ids(scores: &[(i64, f64)], n: usize) -> Vec<i64> {
        let mut sorted: Vec<(i64, f64)> = scores.to_vec();
        sorted.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        sorted.into_iter().take(n).map(|(id, _)| id).collect()
    }

    fn kendall_tau_top_n(new_scores: &[(i64, f64)], old_scores: &[(i64, f64)], n: usize) -> f64 {
        let new_top = top_n_ids(new_scores, n);
        let old_top = top_n_ids(old_scores, n);
        let new_rank: HashMap<i64, usize> =
            new_top.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let old_rank: HashMap<i64, usize> =
            old_top.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let union: HashSet<i64> = new_top.iter().chain(old_top.iter()).copied().collect();
        let items: Vec<i64> = union.into_iter().collect();

        let mut concordant = 0i64;
        let mut discordant = 0i64;
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let (a, b) = (items[i], items[j]);
                if let (Some(&na), Some(&nb), Some(&oa), Some(&ob)) = (
                    new_rank.get(&a),
                    new_rank.get(&b),
                    old_rank.get(&a),
                    old_rank.get(&b),
                ) {
                    let new_order = na.cmp(&nb);
                    let old_order = oa.cmp(&ob);
                    if new_order == old_order {
                        concordant += 1;
                    } else {
                        discordant += 1;
                    }
                }
            }
        }
        let total = concordant + discordant;
        if total == 0 {
            1.0
        } else {
            (concordant - discordant) as f64 / total as f64
        }
    }

    /// Per-track structured table emitted on parity failure. Sorted by
    /// `|new_score - old_score|` descending so tiny diffs at the top
    /// suggest float precision while large diffs at the top point to a
    /// logic bug. Free triage signal.
    fn emit_divergence_table(new_scores: &[(i64, f64)], old_scores: &[(i64, f64)]) {
        let new_lookup: HashMap<i64, f64> = new_scores.iter().copied().collect();
        let old_lookup: HashMap<i64, f64> = old_scores.iter().copied().collect();
        let new_full = top_n_ids(new_scores, new_scores.len());
        let old_full = top_n_ids(old_scores, old_scores.len());
        let new_rank: HashMap<i64, usize> = new_full
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        let old_rank: HashMap<i64, usize> = old_full
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();

        let mut rows: Vec<(i64, usize, usize, f64, f64)> = new_scores
            .iter()
            .map(|(id, new_s)| {
                let old_s = old_lookup.get(id).copied().unwrap_or(f64::NAN);
                let nr = *new_rank.get(id).unwrap_or(&usize::MAX);
                let or = *old_rank.get(id).unwrap_or(&usize::MAX);
                (*id, or, nr, old_s, *new_s)
            })
            .collect();
        rows.sort_by(|a, b| {
            (b.4 - b.3)
                .abs()
                .partial_cmp(&(a.4 - a.3).abs())
                .unwrap_or(Ordering::Equal)
        });

        eprintln!(
            "{:>8}  {:>8}  {:>8}  {:>10}  {:>11}  {:>11}  {:>13}",
            "track_id",
            "old_rank",
            "new_rank",
            "rank_delta",
            "old_score",
            "new_score",
            "score_delta"
        );
        for (id, or, nr, old_s, new_s) in rows {
            let rank_delta = (nr as i64) - (or as i64);
            eprintln!(
                "{:>8}  {:>8}  {:>8}  {:>10}  {:>11.6}  {:>11.6}  {:>13.9}",
                id,
                or,
                nr,
                rank_delta,
                old_s,
                new_s,
                new_s - old_s
            );
        }
        // Note: per-signal contribution breakdown is intentionally not
        // emitted here. If parity fails, the score-delta column above plus
        // a quick read of `automix_score` against the snapshot is faster
        // than maintaining a parallel breakdown that itself can drift.
        let _ = new_lookup;
    }

    #[test]
    fn automix_score_parity_top_30() {
        let fixture = build_fixture();
        let new_scores = score_with_new_path(&fixture);
        let old_scores = score_with_old_path(&fixture);

        // Pre-assertion guards: same length, no NaN/Inf on either side.
        // Catches NaN-from-zero-division or shape mismatches before we
        // attempt to interpret rankings.
        assert_eq!(
            old_scores.len(),
            new_scores.len(),
            "score vector lengths differ"
        );
        assert!(
            old_scores.iter().all(|(_, s)| s.is_finite()),
            "old path produced NaN/Inf"
        );
        assert!(
            new_scores.iter().all(|(_, s)| s.is_finite()),
            "new path produced NaN/Inf"
        );

        let n = 30.min(fixture.candidates.len());
        let new_top = top_n_ids(&new_scores, n);
        let old_top = top_n_ids(&old_scores, n);

        let tau = kendall_tau_top_n(&new_scores, &old_scores, n);
        eprintln!("automix parity: kendall_tau_top_{n} = {tau:.6}");

        if new_top != old_top {
            eprintln!("\nautomix parity divergence (sorted by |score_delta| desc):");
            emit_divergence_table(&new_scores, &old_scores);
            panic!("top-{n} ranking diverged.\n  old: {old_top:?}\n  new: {new_top:?}");
        }
    }
}
