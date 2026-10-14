//! Radio continuation. A queue built by song, album or artist radio keeps
//! topping up from that radio's seed and blend instead of handing over to
//! automix, whose queue-source chip and session anchors would steer it away
//! from the station the listener picked.
//!
//! The same watcher warms automix's "Include new" lane: when a track starts
//! under automix with it on, the Last.fm matches and TIDAL track mix of that
//! track and of the songs the weave seeds from next are fetched live if none
//! are stored yet, so the next woven song can use them. TIDAL's mix keeps the
//! lane working for listeners without Last.fm. New songs ahead that still
//! wait on TIDAL are resolved early, before playback reaches them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::broadcast;

use crate::services::radio::{RadioBlend, RadioCandidate};
use crate::{AppEvent, SharedState};

/// Top up when fewer than this many rows wait after the current one; the same
/// depth automix keeps.
const TOP_UP_BELOW: usize = crate::playback::automix::AUTOMIX_MIN_UPCOMING;
/// Picks asked for per top-up. The gate drops recent plays and anything
/// already queued, so this asks for more than it expects to add.
const TOP_UP_LIMIT: usize = 40;
const DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioSeedKind {
    Track,
    Album,
    Artist,
}

impl RadioSeedKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Artist => "artist",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "track" => Some(Self::Track),
            "album" => Some(Self::Album),
            "artist" => Some(Self::Artist),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioSeed {
    pub kind: RadioSeedKind,
    pub id: i64,
    pub blend: RadioBlend,
}

fn blend_str(blend: RadioBlend) -> &'static str {
    match blend {
        RadioBlend::Familiar => "familiar",
        RadioBlend::Mixed => "mixed",
        RadioBlend::Adventurous => "adventurous",
    }
}

fn parse_blend(value: &str) -> RadioBlend {
    match value {
        "familiar" => RadioBlend::Familiar,
        "adventurous" => RadioBlend::Adventurous,
        _ => RadioBlend::Mixed,
    }
}

/// Remember the radio that just built the queue.
pub fn remember_seed(conn: &Connection, seed: RadioSeed) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE playback_state SET radio_seed_kind = ?1, radio_seed_id = ?2, radio_blend = ?3
         WHERE id = 1",
        params![seed.kind.as_str(), seed.id, blend_str(seed.blend)],
    )?;
    Ok(())
}

/// Forget the radio seed: another builder replaced the queue, or the radio
/// ran out of picks and automix takes over. Whatever play context the old
/// queue was loaded with ("Include new" weaving) goes with it.
pub fn forget_seed(conn: &Connection) {
    crate::playback::automix::forget_weave_context(conn);
    // Best effort: a database without the columns has nothing to forget.
    let _ = conn.execute(
        "UPDATE playback_state
         SET radio_seed_kind = NULL, radio_seed_id = NULL, radio_blend = NULL
         WHERE id = 1",
        [],
    );
}

pub fn load_seed(conn: &Connection) -> Option<RadioSeed> {
    let (kind, id, blend) = conn
        .query_row(
            "SELECT radio_seed_kind, radio_seed_id, radio_blend FROM playback_state WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()?;
    Some(RadioSeed {
        kind: RadioSeedKind::parse(kind.as_deref()?)?,
        id: id.filter(|id| *id > 0)?,
        blend: parse_blend(blend.as_deref().unwrap_or_default()),
    })
}

/// The queue still belongs to the remembered radio: a seed is stored and the
/// queue holds radio rows. Tracks the listener adds keep it a radio; playing
/// an album or playlist replaces the rows and ends it.
pub fn radio_queue_active(conn: &Connection) -> bool {
    load_seed(conn).is_some()
        && conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM queue WHERE source IN ('radio', 'radio_pending'))",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false)
}

/// Rows queued after the current one.
fn upcoming_count(conn: &Connection) -> usize {
    conn.query_row(
        "SELECT COUNT(*) FROM queue
         WHERE position > COALESCE(
             (SELECT q.position FROM queue q
              JOIN playback_state ps ON ps.current_queue_item_id = q.id
              WHERE ps.id = 1),
             -1)",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count.max(0) as usize)
    .unwrap_or(usize::MAX)
}

fn queued_track_ids(conn: &Connection) -> Vec<i64> {
    conn.prepare("SELECT track_id FROM queue WHERE track_id IS NOT NULL")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// The remembered radio when it needs more rows, with what is queued so far.
fn due_top_up(conn: &Connection) -> Option<(RadioSeed, Vec<i64>)> {
    if !radio_queue_active(conn) || upcoming_count(conn) >= TOP_UP_BELOW {
        return None;
    }
    Some((load_seed(conn)?, queued_track_ids(conn)))
}

fn queued_row_count(conn: &Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM queue", [], |row| row.get::<_, i64>(0))
        .map(|count| count.max(0) as usize)
        .unwrap_or(0)
}

static TOPPING_UP: AtomicBool = AtomicBool::new(false);

/// Add the next batch from the remembered radio. Returns rows appended. When
/// the radio has nothing left, the seed is forgotten so automix carries on.
pub async fn top_up(state: &SharedState) -> anyhow::Result<usize> {
    if TOPPING_UP.swap(true, Ordering::AcqRel) {
        return Ok(0);
    }
    let result = top_up_inner(state).await;
    TOPPING_UP.store(false, Ordering::Release);
    result
}

async fn top_up_inner(state: &SharedState) -> anyhow::Result<usize> {
    let (db, lastfm, cache, event_tx) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (
            g.db.clone(),
            lastfm,
            g.lastfm_similar_cache.clone(),
            g.event_tx.clone(),
        )
    };
    let Some((seed, exclude)) = db.with_conn(|conn| Ok(due_top_up(conn)))? else {
        return Ok(0);
    };
    // The listener's chosen station source, with the station's own blend.
    let preferences = crate::server::routes::radio_preferences(&db);
    let station = crate::server::routes::radio_station::StationContext {
        state,
        db: &db,
        lastfm: lastfm.as_ref(),
        cache: &cache,
    };
    let station_seed = match seed.kind {
        RadioSeedKind::Track => crate::server::routes::radio_station::StationSeed::Track(seed.id),
        RadioSeedKind::Album => crate::server::routes::radio_station::StationSeed::Album(seed.id),
        RadioSeedKind::Artist => crate::server::routes::radio_station::StationSeed::Artist(seed.id),
    };
    let tracks: Vec<RadioCandidate> = crate::server::routes::radio_station::build_station(
        &station,
        station_seed,
        seed.blend,
        TOP_UP_LIMIT,
        &exclude,
        preferences.source,
    )
    .await
    .map(|queue| queue.tracks)
    .unwrap_or_default();
    let (appended, pending_item_ids) = db.with_conn(move |conn| {
        let before = queued_row_count(conn);
        let build =
            crate::server::radio_pipeline::append_radio_queue_from_candidates(conn, tracks)?;
        let added = queued_row_count(conn).saturating_sub(before);
        if added == 0 {
            // Nothing new left for this station: automix takes over.
            forget_seed(conn);
        }
        Ok((added, build.pending_item_ids))
    })?;
    tracing::info!(
        target: "noor.radio",
        seed_kind = seed.kind.as_str(),
        seed_id = seed.id,
        appended,
        "radio continuation topped up the queue"
    );
    if appended > 0 {
        let _ = event_tx.send(AppEvent::QueueUpdated);
    }
    crate::server::routes::spawn_pending_resolvers_for_queue_items(
        state,
        &db,
        pending_item_ids,
        "radio_top_up_pending_resolved",
    )
    .await;
    Ok(appended)
}

/// Automix (not a radio) with "Include new" on: the playing track's Last.fm
/// matches and TIDAL mix feed the next top-up.
fn wants_new_matches(conn: &Connection) -> bool {
    let flags = conn
        .query_row(
            "SELECT automix_enabled, automix_discover_new OR automix_allow_external
             FROM playback_state WHERE id = 1",
            [],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
        )
        .unwrap_or((false, false));
    flags == (true, true) && !radio_queue_active(conn)
}

/// Warm the "Include new" lane for the track that just started and for the
/// songs the weave seeds its next pick from, one song at a time so a track
/// change costs a handful of lookups at most.
async fn warm_new_matches(state: &SharedState, track_id: i64) {
    let db = state.read().await.db.clone();
    let Some(ahead) = db
        .with_conn(|conn| Ok(wants_new_matches(conn).then(|| warm_ahead_track_ids(conn))))
        .ok()
        .flatten()
    else {
        return;
    };
    let tracks = std::iter::once(track_id).chain(ahead.into_iter().filter(|id| *id != track_id));
    for track_id in tracks {
        // Independent lanes: one failing or missing never blocks the other.
        tokio::join!(
            warm_lastfm_matches(state, &db, track_id),
            warm_tidal_mix(state, &db, track_id)
        );
    }
}

async fn warm_lastfm_matches(state: &SharedState, db: &crate::db::Database, track_id: i64) {
    if recently_missed(track_id, Lane::LastFm) {
        return;
    }
    let lastfm = {
        let g = state.read().await;
        crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db)
    };
    let Some(lastfm) = lastfm else { return };
    match crate::services::learning::refresh_lastfm_sightings_for_track(db, &lastfm, track_id).await
    {
        Ok(stored) if stored > 0 => {
            tracing::info!(target: "noor.automix", track_id, stored, "stored Last.fm matches for automix")
        }
        // Nothing new (or fresh matches already stored): no need to ask again soon.
        Ok(_) => note_miss(track_id, Lane::LastFm),
        Err(error) => {
            tracing::debug!(target: "noor.automix", track_id, %error, "Last.fm match lookup failed")
        }
    }
}

/// TIDAL's track mix for the playing track, stored as external picks. Best
/// effort: no TIDAL session, a non-TIDAL track or no mix just stores nothing.
async fn warm_tidal_mix(state: &SharedState, db: &crate::db::Database, track_id: i64) {
    use crate::services::learning::{has_fresh_tidal_mix_sightings, persist_tidal_mix_sightings};
    if recently_missed(track_id, Lane::TidalMix)
        || db
            .with_conn(|conn| Ok(has_fresh_tidal_mix_sightings(conn, track_id)))
            .unwrap_or(true)
    {
        return;
    }
    let seed = crate::server::routes::TidalMixSeed::Track(track_id);
    let (mix, seed_tidal_id) = match crate::server::routes::fetch_seed_tidal_mix(state, db, seed)
        .await
    {
        Ok(found) => found,
        Err(reason) => {
            tracing::debug!(target: "noor.automix", track_id, reason, "no TIDAL mix for automix");
            // A missing session or a failed request is worth retrying; a
            // track that is not on TIDAL or has no mix is not, for a while.
            if !matches!(
                reason,
                "TIDAL is not connected" | "the TIDAL radio request failed"
            ) {
                note_miss(track_id, Lane::TidalMix);
            }
            return;
        }
    };
    let now = chrono::Utc::now().naive_utc();
    match db
        .with_conn(move |conn| persist_tidal_mix_sightings(conn, track_id, seed_tidal_id, mix, now))
    {
        Ok(stored) if stored > 0 => {
            tracing::info!(target: "noor.automix", track_id, stored, "stored TIDAL mix for automix")
        }
        Ok(_) => note_miss(track_id, Lane::TidalMix),
        Err(error) => {
            tracing::warn!(target: "noor.automix", track_id, %error, "storing TIDAL mix failed")
        }
    }
}

/// Position of the playing row, or -1 when nothing is playing.
const CURRENT_POSITION_SQL: &str = "COALESCE(
    (SELECT q.position FROM queue q
     JOIN playback_state ps ON ps.current_queue_item_id = q.id
     WHERE ps.id = 1),
    -1)";

/// The songs the weave seeds its next pick from: the second to fifth after
/// the playing one, whichever pair borders the slot when it next looks.
/// Empty unless "Include new" is on outside a radio.
fn warm_ahead_track_ids(conn: &Connection) -> Vec<i64> {
    if !wants_new_matches(conn) {
        return Vec::new();
    }
    let sql = format!(
        "SELECT track_id FROM queue WHERE position > {CURRENT_POSITION_SQL}
         ORDER BY position LIMIT 4 OFFSET 1"
    );
    conn.prepare(&sql)
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, Option<i64>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map(|ids| ids.into_iter().flatten().collect())
        .unwrap_or_default()
}

/// New songs ahead still waiting on TIDAL that no resolver holds. Resolving
/// them early keeps playback (and gapless preloading) from waiting on one.
fn unresolved_new_picks(conn: &Connection) -> Vec<i64> {
    let sql = format!(
        "SELECT id FROM queue
         WHERE source = 'automix-new' AND track_id IS NULL AND resolving_at IS NULL
           AND position > {CURRENT_POSITION_SQL}
         ORDER BY position"
    );
    conn.prepare(&sql)
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

async fn resolve_new_picks_ahead(state: &SharedState) {
    let db = state.read().await.db.clone();
    let pending = db
        .with_conn(|conn| Ok(unresolved_new_picks(conn)))
        .unwrap_or_default();
    crate::server::routes::spawn_pending_resolvers_for_queue_items(
        state,
        &db,
        pending,
        "automix_new_pending_resolved",
    )
    .await;
}

/// How long a lookup that found nothing is trusted before asking again.
const MISS_TTL: Duration = Duration::from_secs(12 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Lane {
    LastFm,
    TidalMix,
}

/// Lookups that found nothing. Warming ahead visits a song several times as
/// it moves up the queue, and only stored picks mark it done, so without this
/// a song with no Last.fm match or TIDAL mix is fetched again on every visit.
/// Kept in memory: after a restart each song is asked once more.
#[derive(Default)]
struct LookupMisses {
    at: HashMap<(i64, Lane), Instant>,
}

impl LookupMisses {
    fn recent(&self, track_id: i64, lane: Lane, now: Instant) -> bool {
        self.at
            .get(&(track_id, lane))
            .is_some_and(|at| now.saturating_duration_since(*at) <= MISS_TTL)
    }

    fn note(&mut self, track_id: i64, lane: Lane, now: Instant) {
        self.at
            .retain(|_, at| now.saturating_duration_since(*at) <= MISS_TTL);
        self.at.insert((track_id, lane), now);
    }
}

static LOOKUP_MISSES: LazyLock<Mutex<LookupMisses>> = LazyLock::new(Default::default);

fn recently_missed(track_id: i64, lane: Lane) -> bool {
    LOOKUP_MISSES
        .lock()
        .map(|misses| misses.recent(track_id, lane, Instant::now()))
        .unwrap_or(false)
}

fn note_miss(track_id: i64, lane: Lane) {
    if let Ok(mut misses) = LOOKUP_MISSES.lock() {
        misses.note(track_id, lane, Instant::now());
    }
}

/// Watch queue and track changes: top the radio up when it runs low, warm
/// automix's "Include new" lane for the track that just started and the songs
/// ahead, and resolve new songs before playback reaches them.
pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        let mut event_rx = {
            let s = state.read().await;
            s.event_tx.subscribe()
        };
        let mut deadline: Option<Instant> = None;
        loop {
            let wait = deadline
                .map(|d| d.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(3600));
            tokio::select! {
                msg = event_rx.recv() => match msg {
                    Ok(AppEvent::TrackChanged { track_id }) => {
                        deadline.get_or_insert(Instant::now() + DEBOUNCE);
                        let state = state.clone();
                        tokio::spawn(async move { warm_new_matches(&state, track_id).await });
                    }
                    Ok(AppEvent::QueueUpdated) => {
                        deadline.get_or_insert(Instant::now() + DEBOUNCE);
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                    _ => {}
                },
                _ = tokio::time::sleep(wait), if deadline.is_some() => {
                    deadline = None;
                    if let Err(error) = top_up(&state).await {
                        tracing::warn!(target: "noor.radio", %error, "radio continuation failed");
                    }
                    resolve_new_picks_ahead(&state).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'A');
             INSERT INTO tracks (id, title, artist_id) VALUES (1, 'One', 1), (2, 'Two', 1), (3, 'Three', 1);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_radio_queue_tops_up_from_its_own_seed_until_it_is_replaced() {
        let conn = conn();
        assert!(load_seed(&conn).is_none());
        let seed = RadioSeed {
            kind: RadioSeedKind::Track,
            id: 1,
            blend: RadioBlend::Adventurous,
        };
        remember_seed(&conn, seed).unwrap();
        assert_eq!(load_seed(&conn), Some(seed));
        // No radio rows yet: nothing to continue.
        assert!(!radio_queue_active(&conn));

        conn.execute_batch(
            "INSERT INTO queue (id, track_id, position, source) VALUES
                (1, 1, 0, 'radio'), (2, 2, 1, 'radio'), (3, 3, 2, 'user');
             UPDATE playback_state SET current_queue_item_id = 1 WHERE id = 1;",
        )
        .unwrap();
        assert!(radio_queue_active(&conn));
        let (due_seed, queued) = due_top_up(&conn).expect("two upcoming is below the depth");
        assert_eq!(due_seed, seed);
        assert_eq!(queued, vec![1, 2, 3]);

        // Playing an album replaces the rows: the radio is over.
        conn.execute_batch(
            "DELETE FROM queue;
             INSERT INTO queue (id, track_id, position, source) VALUES (4, 1, 0, 'user');",
        )
        .unwrap();
        assert!(!radio_queue_active(&conn));
        assert!(due_top_up(&conn).is_none());

        forget_seed(&conn);
        assert!(load_seed(&conn).is_none());
    }

    #[test]
    fn automix_stands_aside_for_a_radio_until_it_runs_dry() {
        use crate::playback::automix::ensure_automix_queue_depth;
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO tracks (id, title, artist_id) VALUES (4, 'Four', 1), (5, 'Five', 1);
             INSERT INTO track_similarity (track_a, track_b, similarity_score)
             VALUES (1, 4, 0.9), (1, 5, 0.8);
             INSERT INTO queue (id, track_id, position, source) VALUES (1, 1, 0, 'radio'), (2, 2, 1, 'radio');
             UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = 1, automix_enabled = 1,
                 automix_use_learning = 0
             WHERE id = 1;",
        )
        .unwrap();
        remember_seed(
            &conn,
            RadioSeed {
                kind: RadioSeedKind::Track,
                id: 1,
                blend: RadioBlend::Mixed,
            },
        )
        .unwrap();
        // One radio row still waits: the radio tops itself up, automix waits.
        assert_eq!(
            ensure_automix_queue_depth(&conn, 8, false).unwrap().len(),
            2
        );

        // At the last row automix steps in so playback never stops.
        conn.execute("UPDATE playback_state SET current_track_id = 2, current_queue_item_id = 2 WHERE id = 1", [])
            .unwrap();
        conn.execute_batch(
            "INSERT INTO track_similarity (track_a, track_b, similarity_score) VALUES (2, 4, 0.9), (2, 5, 0.8);",
        )
        .unwrap();
        assert!(ensure_automix_queue_depth(&conn, 8, false).unwrap().len() > 2);
    }

    /// Seven queued songs (tracks 1..=7), the first playing, automix on.
    fn seven_song_session(conn: &Connection, include_new: bool) {
        conn.execute_batch(
            "INSERT INTO tracks (id, title, artist_id) VALUES
                (4, 'Four', 1), (5, 'Five', 1), (6, 'Six', 1), (7, 'Seven', 1);
             INSERT INTO queue (id, track_id, position, source) VALUES
                (1, 1, 0, 'user'), (2, 2, 1, 'user'), (3, 3, 2, 'user'), (4, 4, 3, 'user'),
                (5, 5, 4, 'user'), (6, 6, 5, 'user'), (7, 7, 6, 'user');
             UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = 1, automix_enabled = 1
             WHERE id = 1;",
        )
        .unwrap();
        conn.execute(
            "UPDATE playback_state SET automix_discover_new = ?1 WHERE id = 1",
            [include_new],
        )
        .unwrap();
    }

    #[test]
    fn include_new_warms_the_songs_around_the_next_slot() {
        let conn = conn();
        seven_song_session(&conn, true);
        // The second to fifth songs ahead: whichever pair borders the slot
        // when the weave next looks.
        assert_eq!(warm_ahead_track_ids(&conn), vec![3, 4, 5, 6]);

        conn.execute("UPDATE playback_state SET automix_discover_new = 0", [])
            .unwrap();
        assert!(warm_ahead_track_ids(&conn).is_empty());
    }

    #[test]
    fn new_songs_ahead_are_resolved_before_playback_reaches_them() {
        let conn = conn();
        seven_song_session(&conn, true);
        conn.execute_batch(
            "INSERT INTO queue (id, track_id, position, source, pending_artist, pending_title,
                                pending_at, resolving_at) VALUES
                (8, NULL, 7, 'automix-new', 'X', 'Waiting', datetime('now'), NULL),
                (9, NULL, 8, 'automix-new', 'X', 'Resolving', datetime('now'), datetime('now')),
                (10, NULL, 9, 'radio_pending', 'X', 'Album row', datetime('now'), NULL),
                (11, 3, 10, 'automix-new', NULL, NULL, NULL, NULL);",
        )
        .unwrap();

        assert_eq!(unresolved_new_picks(&conn), vec![8]);
    }

    #[test]
    fn a_lookup_that_found_nothing_waits_before_asking_again() {
        let start = Instant::now();
        let mut misses = LookupMisses::default();
        assert!(!misses.recent(1, Lane::TidalMix, start));

        misses.note(1, Lane::TidalMix, start);
        assert!(misses.recent(1, Lane::TidalMix, start + Duration::from_secs(60)));
        assert!(!misses.recent(1, Lane::LastFm, start), "lanes are separate");
        assert!(!misses.recent(2, Lane::TidalMix, start));
        assert!(!misses.recent(1, Lane::TidalMix, start + MISS_TTL + Duration::from_secs(1)));
    }
}
