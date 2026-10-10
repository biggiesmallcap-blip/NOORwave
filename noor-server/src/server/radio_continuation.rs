//! Radio continuation. A queue built by song, album or artist radio keeps
//! topping up from that radio's seed and blend instead of handing over to
//! automix, whose queue-source chip and session anchors would steer it away
//! from the station the listener picked.
//!
//! The same watcher warms automix's Last.fm lane: when a track starts under
//! automix with "Include new" on, its Last.fm matches are fetched live if
//! none are stored yet, so the next top-up can use them.

use std::sync::atomic::{AtomicBool, Ordering};
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
/// ran out of picks and automix takes over.
pub fn forget_seed(conn: &Connection) {
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
    use crate::services::radio::{orchestrate_album, orchestrate_artist, orchestrate_song};

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
    let orchestrated = match seed.kind {
        RadioSeedKind::Track => {
            orchestrate_song(
                &db,
                lastfm.as_ref(),
                Some(&cache),
                seed.id,
                seed.blend,
                TOP_UP_LIMIT,
                &exclude,
            )
            .await
        }
        RadioSeedKind::Album => {
            orchestrate_album(
                &db,
                lastfm.as_ref(),
                Some(&cache),
                seed.id,
                seed.blend,
                TOP_UP_LIMIT,
                &exclude,
            )
            .await
        }
        RadioSeedKind::Artist => {
            orchestrate_artist(
                &db,
                lastfm.as_ref(),
                Some(&cache),
                seed.id,
                seed.blend,
                TOP_UP_LIMIT,
                &exclude,
            )
            .await
        }
    };
    let mut tracks: Vec<RadioCandidate> =
        orchestrated.map(|queue| queue.tracks).unwrap_or_default();
    let mix_seed = match seed.kind {
        RadioSeedKind::Track => Some(crate::server::routes::TidalMixSeed::Track(seed.id)),
        RadioSeedKind::Artist => Some(crate::server::routes::TidalMixSeed::Artist(seed.id)),
        RadioSeedKind::Album => None,
    };
    if let Some(mix_seed) = mix_seed {
        crate::server::routes::add_tidal_mix_fallback(
            state,
            &db,
            mix_seed,
            &mut tracks,
            TOP_UP_LIMIT,
        )
        .await;
    }
    let appended = db.with_conn(move |conn| {
        let before = queued_row_count(conn);
        crate::server::radio_pipeline::append_radio_queue_from_candidates(conn, tracks)?;
        let added = queued_row_count(conn).saturating_sub(before);
        if added == 0 {
            // Nothing new left for this station: automix takes over.
            forget_seed(conn);
        }
        Ok(added)
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
    Ok(appended)
}

/// Automix (not a radio) with "Include new" on: the playing track's Last.fm
/// matches feed the next top-up.
fn wants_lastfm_matches(conn: &Connection) -> bool {
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

async fn warm_lastfm_matches(state: &SharedState, track_id: i64) {
    let (db, lastfm) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm)
    };
    let Some(lastfm) = lastfm else { return };
    if !db
        .with_conn(|conn| Ok(wants_lastfm_matches(conn)))
        .unwrap_or(false)
    {
        return;
    }
    match crate::services::learning::refresh_lastfm_sightings_for_track(&db, &lastfm, track_id)
        .await
    {
        Ok(stored) if stored > 0 => {
            tracing::info!(target: "noor.automix", track_id, stored, "stored Last.fm matches for automix")
        }
        Ok(_) => {}
        Err(error) => {
            tracing::debug!(target: "noor.automix", track_id, %error, "Last.fm match lookup failed")
        }
    }
}

/// Watch queue and track changes: top the radio up when it runs low, and warm
/// automix's Last.fm lane for the track that just started.
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
                        tokio::spawn(async move { warm_lastfm_matches(&state, track_id).await });
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
}
