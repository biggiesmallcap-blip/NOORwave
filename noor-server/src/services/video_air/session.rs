//! Refills after the schedule: once a tuned-in session has played through
//! what the clock queued, the channel keeps going live from the same pool,
//! shaped by what this session did (zaps, holds, saves in the last hours of
//! video_history) and the +/- presses the client sends. Never repeats a
//! video or song the session already has; concerts finish before new ones.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rusqlite::Connection;
use serde::Deserialize;

use super::ExploreMode;
use super::channels::{self, Boosts, ChannelId};
use super::day::{self, QUEUE};
use super::programmer::{self, Picker, SPACING};
use super::taste::{Taste, is_hold, is_zap};
use super::universe::Universe;
use crate::services::video_sets::{VideoSetItem, build_seed};

/// How far back video_history counts as this session.
pub const SESSION_HOURS: i64 = 3;
const MAX_EXCLUDED: usize = 512;
const MAX_ARTIST_IDS: usize = 64;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NextRequest {
    #[serde(default)]
    pub exclude_video_ids: Vec<i64>,
    #[serde(default)]
    pub session_nonce: String,
    #[serde(default)]
    pub more_artist_ids: Vec<i64>,
    #[serde(default)]
    pub less_artist_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Signal {
    pub zaps: u32,
    pub holds: u32,
    pub saves: u32,
}

/// What the session did lately, per artist, plus the videos it zapped.
#[derive(Debug, Clone, Default)]
pub struct SessionSignals {
    pub artists: HashMap<i64, Signal>,
    pub zapped: HashSet<i64>,
}

impl SessionSignals {
    pub fn load(conn: &Connection) -> Result<Self> {
        let mut signals = Self::default();
        let mut stmt = conn.prepare(&format!(
            "SELECT tidal_video_id, artist_tidal_id, duration_watched_ms, video_duration_ms, completed
               FROM video_history
              WHERE started_at >= datetime('now', '-{SESSION_HOURS} hours')
                AND duration_watched_ms IS NOT NULL"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<bool>>(4)?.unwrap_or(false),
            ))
        })?;
        for row in rows {
            let (video, artist, watched_ms, duration_ms, completed) = row?;
            let zap = is_zap(watched_ms, duration_ms, completed);
            if zap {
                signals.zapped.insert(video);
            }
            if let Some(artist) = artist.filter(|a| *a > 0) {
                let signal = signals.artists.entry(artist).or_default();
                signal.zaps += u32::from(zap);
                signal.holds += u32::from(is_hold(watched_ms, duration_ms, completed));
            }
        }
        let mut stmt = conn.prepare(&format!(
            "SELECT CAST(json_extract(item_json, '$.artist_id') AS INTEGER) FROM saved_videos
              WHERE saved_at >= datetime('now', '-{SESSION_HOURS} hours')
                AND json_type(item_json, '$.artist_id') = 'integer'"
        ))?;
        for artist in stmt.query_map([], |row| row.get::<_, i64>(0))? {
            let artist = artist?;
            if artist > 0 {
                signals.artists.entry(artist).or_default().saves += 1;
            }
        }
        Ok(signals)
    }
}

/// Zap -0.2 and hold +0.5 on the artist, save +1; the scene moves by a
/// fifth as much (-0.05, +0.1, +0.2). More doubles an artist, less halves.
pub fn boosts(signals: &SessionSignals, universe: &Universe, more: &[i64], less: &[i64]) -> Boosts {
    let mut out = Boosts {
        skip_videos: signals.zapped.clone(),
        ..Default::default()
    };
    let mut scene_delta: HashMap<i64, f64> = HashMap::new();
    let scene_of = |artist: i64| {
        let start = universe.videos.partition_point(|v| v.artist < artist);
        universe
            .videos
            .get(start)
            .filter(|v| v.artist == artist)
            .and_then(|v| v.scene)
    };
    let mut artists: Vec<(&i64, &Signal)> = signals.artists.iter().collect();
    artists.sort_by_key(|(artist, _)| **artist);
    for (artist, signal) in artists {
        let delta =
            0.5 * f64::from(signal.holds) - 0.2 * f64::from(signal.zaps) + f64::from(signal.saves);
        out.artist.insert(*artist, (1.0 + delta).clamp(0.1, 3.0));
        if let Some(scene) = scene_of(*artist) {
            *scene_delta.entry(scene).or_default() += 0.1 * f64::from(signal.holds)
                - 0.05 * f64::from(signal.zaps)
                + 0.2 * f64::from(signal.saves);
        }
    }
    out.scene = scene_delta
        .into_iter()
        .map(|(scene, delta)| (scene, (1.0 + delta).clamp(0.5, 2.0)))
        .collect();
    for artist in more.iter().take(MAX_ARTIST_IDS) {
        *out.artist.entry(*artist).or_insert(1.0) *= 2.0;
    }
    for artist in less.iter().take(MAX_ARTIST_IDS) {
        *out.artist.entry(*artist).or_insert(1.0) *= 0.5;
    }
    out
}

/// The next videos for a tuned-in session. Empty when the channel is
/// unknown, locked or spent.
pub fn refill(
    conn: &Connection,
    id: &str,
    day: &str,
    explore: ExploreMode,
    request: &NextRequest,
    universe: &Universe,
    taste: &Taste,
) -> Result<Vec<VideoSetItem>> {
    let Some(channel) = ChannelId::parse(id) else {
        return Ok(Vec::new());
    };
    let id = channel.as_string();
    let lineup = day::ensure(conn, day, explore)?;
    if lineup.channel(&id).is_none_or(|meta| meta.locked) {
        return Ok(Vec::new());
    }
    let excluded: Vec<i64> = request
        .exclude_video_ids
        .iter()
        .copied()
        .filter(|id| *id > 0)
        .take(MAX_EXCLUDED)
        .collect();
    let signals = SessionSignals::load(conn)?;
    let mut shaping = boosts(
        &signals,
        universe,
        &request.more_artist_ids,
        &request.less_artist_ids,
    );
    shaping.skip_videos.extend(excluded.iter().copied());
    let ctx = lineup.context();
    let built = channels::build_pool(&channel, universe, taste, &ctx, &shaping);
    let pool = built.pool;

    let concerts = channel == ChannelId::Concerts;
    let mut picks: Vec<(i64, String)> = Vec::new();
    let mut used: HashSet<u64> = HashSet::new();
    for video in &excluded {
        let Some(row) = universe.video(*video) else {
            continue;
        };
        used.insert(row.song);
        if concerts && let Some(concert) = row.concert {
            used.insert(channels::concert_key(concert));
        }
    }
    // A concert the session is part way through finishes first.
    if concerts
        && let Some(last) = excluded.last().and_then(|id| universe.video(*id))
        && let Some(concert_id) = last.concert
        && let Ok(index) = universe
            .concerts
            .binary_search_by_key(&concert_id, |c| c.id)
    {
        let concert = &universe.concerts[index];
        let excluded_set: HashSet<i64> = excluded.iter().copied().collect();
        if let Some(position) = concert.video_ids.iter().position(|id| *id == last.id) {
            picks.extend(
                concert.video_ids[position + 1..]
                    .iter()
                    .filter(|id| !excluded_set.contains(id) && universe.video(**id).is_some())
                    .map(|id| (*id, concert.label.clone())),
            );
        }
    }
    let recent: Vec<i64> = excluded
        .iter()
        .rev()
        .filter_map(|id| universe.video(*id).map(|row| row.artist))
        .take(SPACING.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut picker = Picker::session(&pool, &recent, used);
    let nonce: String = request.session_nonce.chars().take(64).collect();
    let mut rng = StdRng::seed_from_u64(build_seed(
        &id,
        &format!("{day}/{nonce}/{}", excluded.len()),
    ));
    if picks.len() < QUEUE {
        let wanted = QUEUE - picks.len();
        picks.extend(programmer::refill(&mut picker, &mut rng, wanted));
    }
    for (video, why) in picks.iter_mut().filter(|(_, why)| why.is_empty()) {
        *why = channels::chip(&channel, *video, universe, taste);
    }
    let ids: Vec<i64> = picks.iter().map(|(id, _)| *id).collect();
    let videos = day::catalog_videos(conn, &ids)?;
    Ok(picks
        .iter()
        .filter_map(|(id, why)| videos.get(id).map(|video| day::item(video, why)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_air::test_support::*;

    fn seed(conn: &Connection) {
        for artist in 0..60 {
            for n in 0..12 {
                add_video(
                    conn,
                    10_000 + artist * 100 + n,
                    500 + artist,
                    "official",
                    &format!("song {n}"),
                    240,
                );
            }
        }
    }

    fn request(exclude: Vec<i64>) -> NextRequest {
        NextRequest {
            exclude_video_ids: exclude,
            session_nonce: "abc".into(),
            ..Default::default()
        }
    }

    fn next(conn: &Connection, id: &str, request: &NextRequest) -> Vec<VideoSetItem> {
        let universe = Universe::load(conn).unwrap();
        let taste = Taste::load(conn, &universe).unwrap();
        refill(
            conn,
            id,
            "2026-10-14",
            ExploreMode::Mixed,
            request,
            &universe,
            &taste,
        )
        .unwrap()
    }

    #[test]
    fn a_refill_never_repeats_the_session_and_spaces_artists() {
        let conn = conn();
        seed(&conn);
        let excluded: Vec<i64> = (0..11).map(|a| 10_000 + a * 100).collect();
        let items = next(&conn, "wild", &request(excluded.clone()));
        assert_eq!(items.len(), QUEUE);
        assert!(
            items.iter().all(|v| v.cut.as_deref() == Some("official")),
            "refills carry the cut for the lower third"
        );
        let recent_artists: HashSet<i64> = (0..11).map(|a| 500 + a).collect();
        assert!(items.iter().all(|v| !excluded.contains(&v.tidal_id)));
        assert!(
            !recent_artists.contains(&items[0].artist_id.unwrap()),
            "the session's last eleven artists wait"
        );
        let artists: HashSet<i64> = items.iter().filter_map(|v| v.artist_id).collect();
        assert_eq!(artists.len(), QUEUE, "no artist twice in a refill");
        let again = next(&conn, "wild", &request(excluded));
        let ids = |items: &[VideoSetItem]| items.iter().map(|v| v.tidal_id).collect::<Vec<_>>();
        assert_eq!(ids(&items), ids(&again), "seeded per session");
    }

    #[test]
    fn more_and_less_shape_the_refill() {
        let conn = conn();
        seed(&conn);
        let favoured = |shaped: bool| {
            let mut total = 0;
            for nonce in 0..40 {
                let mut req = request(Vec::new());
                req.session_nonce = format!("n{nonce}");
                if shaped {
                    req.more_artist_ids = vec![500, 501, 502];
                    req.less_artist_ids = (503..560).collect();
                }
                total += next(&conn, "wild", &req)
                    .iter()
                    .filter(|v| matches!(v.artist_id, Some(500..=502)))
                    .count();
            }
            total
        };
        let (plain, shaped) = (favoured(false), favoured(true));
        assert!(shaped > plain + 15, "plain {plain}, shaped {shaped}");
    }

    #[test]
    fn session_zaps_and_holds_move_artists_and_ban_the_zapped_video() {
        let conn = conn();
        seed(&conn);
        add_watch(&conn, 10_000, 500, 3, 240, 1);
        add_watch(&conn, 10_100, 501, 240, 240, 1);
        let signals = SessionSignals::load(&conn).unwrap();
        assert!(signals.zapped.contains(&10_000));
        let universe = Universe::load(&conn).unwrap();
        let shaping = boosts(&signals, &universe, &[], &[502]);
        assert!(shaping.artist[&500] < 1.0);
        assert!(shaping.artist[&501] > 1.0);
        assert_eq!(shaping.artist[&502], 0.5);
        for nonce in 0..10 {
            let mut req = request(Vec::new());
            req.session_nonce = format!("z{nonce}");
            assert!(
                next(&conn, "wild", &req)
                    .iter()
                    .all(|v| v.tidal_id != 10_000)
            );
        }
    }

    #[test]
    fn concerts_finish_before_new_ones_start() {
        let conn = conn();
        let mut concert = 1;
        for artist in 0..30 {
            for c in 0..2 {
                let ids: Vec<i64> = (0..6)
                    .map(|n| 100_000 + artist * 100 + c * 10 + n)
                    .collect();
                for (n, id) in ids.iter().enumerate() {
                    add_video(
                        &conn,
                        *id,
                        700 + artist,
                        "live",
                        &format!("live {c} {n}"),
                        400,
                    );
                }
                add_concert(
                    &conn,
                    concert,
                    700 + artist,
                    &format!("Live {concert}"),
                    &ids,
                );
                concert += 1;
            }
        }
        let items = next(&conn, "concerts", &request(vec![100_000, 100_001]));
        let first: Vec<i64> = items.iter().take(4).map(|v| v.tidal_id).collect();
        assert_eq!(first, vec![100_002, 100_003, 100_004, 100_005]);
        assert!(items.len() >= QUEUE);
        let rest = &items[4..];
        for chunk in rest.chunks(6) {
            assert_eq!(chunk.len(), 6, "whole concerts");
            assert!(chunk.iter().all(|v| v.why == chunk[0].why));
        }
    }

    #[test]
    fn unknown_and_locked_channels_refill_nothing() {
        let conn = conn();
        seed(&conn);
        assert!(next(&conn, "nope", &request(Vec::new())).is_empty());
        assert!(next(&conn, "live", &request(Vec::new())).is_empty());
    }
}
