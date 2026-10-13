//! A day of On Air: built lazily on the first request of the day for every
//! channel in one pass, frozen in video_channel_day (programs) and
//! server_config (the dial), then served by local time. Taste changes reach
//! tomorrow's build; turning the explore knob rebuilds today.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::ExploreMode;
use super::channels::{self, ChannelId, ChannelMeta, DayContext, SpotlightPick};
use super::clock::{self, DAY_S};
use super::programmer::{self, Program};
use super::taste::Taste;
use super::universe::{self, Universe};
use crate::services::video_sets::{VideoCandidate, VideoSetItem};
use crate::services::video_stations::{lineup, to_item};

/// Bump when a release changes what a day holds: a stored day from an older
/// planner is rebuilt on the next request.
pub const LINEUP_VERSION: u32 = 1;
const LINEUP_KEY_PREFIX: &str = "video_air.lineup:";
/// Days older than this many days before the built day are dropped.
const KEEP_DAYS: i64 = 2;
const CACHE_TTL: Duration = Duration::from_secs(600);
/// Concert Hall's tune-in queue runs on to finish the concert it ends in.
const CONCERT_QUEUE_SLACK: usize = 60;
pub const QUEUE: usize = 12;
/// Tomorrow's first programs that a long tail (a concert) may skip past.
const TOMORROW_SLACK: usize = 40;

/// The dial for one day, as stored next to its programs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lineup {
    pub version: u32,
    pub day: String,
    pub explore: ExploreMode,
    /// False when built from the catalog fallback (no intelligence yet).
    pub facets: bool,
    pub built_at: String,
    pub spotlight_artist: Option<i64>,
    pub spotlight_name: Option<String>,
    pub rewind_decade: Option<i32>,
    pub channels: Vec<ChannelMeta>,
}

impl Lineup {
    pub fn channel(&self, id: &str) -> Option<&ChannelMeta> {
        self.channels.iter().find(|channel| channel.id == id)
    }

    pub fn context(&self) -> DayContext {
        DayContext {
            explore: self.explore,
            spotlight: self.spotlight_artist.map(|artist| SpotlightPick {
                artist,
                name: self.spotlight_name.clone().unwrap_or_default(),
            }),
            rewind_decade: self.rewind_decade,
        }
    }
}

static BUILD_LOCK: Mutex<()> = Mutex::new(());
static CACHE: Mutex<Option<(Instant, Arc<Universe>, Arc<Taste>)>> = Mutex::new(None);

/// The universe and taste, shared by refills for ten minutes. Tests each own
/// a database, so they always load fresh.
pub fn shared(conn: &Connection) -> Result<(Arc<Universe>, Arc<Taste>)> {
    if !cfg!(test)
        && let Ok(cache) = CACHE.lock()
        && let Some((built, universe, taste)) = cache.as_ref()
        && built.elapsed() < CACHE_TTL
    {
        return Ok((universe.clone(), taste.clone()));
    }
    let universe = Arc::new(Universe::load(conn)?);
    let taste = Arc::new(Taste::load(conn, &universe)?);
    remember(&universe, &taste);
    Ok((universe, taste))
}

fn remember(universe: &Arc<Universe>, taste: &Arc<Taste>) {
    if !cfg!(test)
        && let Ok(mut cache) = CACHE.lock()
    {
        *cache = Some((Instant::now(), universe.clone(), taste.clone()));
    }
}

fn lineup_key(day: &str) -> String {
    format!("{LINEUP_KEY_PREFIX}{day}")
}

pub fn load_lineup(conn: &Connection, day: &str) -> Result<Option<Lineup>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [lineup_key(day)],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()))
}

fn is_current(lineup: &Lineup, explore: ExploreMode, facets_now: bool) -> bool {
    lineup.version == LINEUP_VERSION && lineup.explore == explore && (lineup.facets || !facets_now)
}

/// The day's lineup, building it (and its programs) when missing, from an
/// older planner, under another explore setting, or from the fallback while
/// the intelligence build has since landed.
pub fn ensure(conn: &Connection, day: &str, explore: ExploreMode) -> Result<Lineup> {
    if let Some(lineup) = load_lineup(conn, day)?
        && is_current(&lineup, explore, universe::facets_ready(conn)?)
    {
        return Ok(lineup);
    }
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(lineup) = load_lineup(conn, day)?
        && is_current(&lineup, explore, universe::facets_ready(conn)?)
    {
        return Ok(lineup);
    }
    let started = Instant::now();
    let lineup = build(conn, day, explore)?;
    tracing::info!(
        target: "noor.video_air",
        day,
        channels = lineup.channels.len(),
        on_air = lineup.channels.iter().filter(|c| !c.locked).count(),
        facets = lineup.facets,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "on air day built"
    );
    Ok(lineup)
}

fn spotlight(conn: &Connection, taste: &Taste, day: &str) -> Result<Option<SpotlightPick>> {
    let Some(card) = lineup::spotlight(conn, &taste.listener(), day)? else {
        return Ok(None);
    };
    Ok(card
        .id
        .strip_prefix("spotlight:")
        .and_then(|id| id.parse::<i64>().ok())
        .map(|artist| SpotlightPick {
            artist,
            name: card.title,
        }))
}

/// Build and store every channel's day. Always rebuilds.
pub fn build(conn: &Connection, day: &str, explore: ExploreMode) -> Result<Lineup> {
    let universe = Arc::new(Universe::load(conn)?);
    let taste = Arc::new(Taste::load(conn, &universe)?);
    remember(&universe, &taste);
    build_with(conn, day, explore, &universe, &taste)
}

pub fn build_with(
    conn: &Connection,
    day: &str,
    explore: ExploreMode,
    universe: &Universe,
    taste: &Taste,
) -> Result<Lineup> {
    let ctx = DayContext {
        explore,
        spotlight: spotlight(conn, taste, day)?,
        rewind_decade: channels::rewind_decade(universe, day),
    };
    let planned = channels::plan(universe, taste, &ctx);
    let mut schedules: Vec<(String, Vec<Program>)> = Vec::new();
    for channel in &planned {
        let (Some(pool), Some(id)) = (&channel.pool, ChannelId::parse(&channel.meta.id)) else {
            continue;
        };
        let mut programs = programmer::schedule_day(pool, &channel.meta.id, day);
        for program in programs.iter_mut().filter(|p| p.why.is_empty()) {
            program.why = channels::chip(&id, program.video_id, universe, taste);
        }
        schedules.push((channel.meta.id.clone(), programs));
    }
    let lineup = Lineup {
        version: LINEUP_VERSION,
        day: day.to_string(),
        explore,
        facets: universe.facets,
        built_at: chrono::Utc::now().to_rfc3339(),
        spotlight_artist: ctx.spotlight.as_ref().map(|pick| pick.artist),
        spotlight_name: ctx.spotlight.as_ref().map(|pick| pick.name.clone()),
        rewind_decade: ctx.rewind_decade,
        channels: planned.into_iter().map(|channel| channel.meta).collect(),
    };
    store(conn, &lineup, &schedules)?;
    Ok(lineup)
}

fn store(conn: &Connection, lineup: &Lineup, schedules: &[(String, Vec<Program>)]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM video_channel_day WHERE day = ?1",
        [&lineup.day],
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO video_channel_day (channel_id, day, slot, start_s, duration_s, tidal_video_id, why)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (channel, programs) in schedules {
            for program in programs {
                insert.execute(params![
                    channel,
                    lineup.day,
                    program.slot,
                    program.start_s,
                    program.duration_s,
                    program.video_id,
                    program.why,
                ])?;
            }
        }
    }
    if let Some(oldest) = clock::shift_day(&lineup.day, -KEEP_DAYS) {
        tx.execute("DELETE FROM video_channel_day WHERE day < ?1", [&oldest])?;
        tx.execute(
            "DELETE FROM server_config WHERE key LIKE 'video_air.lineup:%' AND key < ?1",
            [lineup_key(&oldest)],
        )?;
    }
    tx.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![lineup_key(&lineup.day), serde_json::to_string(lineup)?],
    )?;
    tx.commit()?;
    Ok(())
}

fn program_from_row(row: &rusqlite::Row) -> rusqlite::Result<Program> {
    Ok(Program {
        slot: row.get(0)?,
        start_s: row.get(1)?,
        duration_s: row.get(2)?,
        video_id: row.get(3)?,
        why: row.get(4)?,
    })
}

/// Up to `limit` programs of one channel's day still airing after `after_s`.
pub fn programs_after(
    conn: &Connection,
    channel: &str,
    day: &str,
    after_s: i64,
    limit: usize,
) -> Result<Vec<Program>> {
    let mut stmt = conn.prepare_cached(
        "SELECT slot, start_s, duration_s, tidal_video_id, why FROM video_channel_day
          WHERE channel_id = ?1 AND day = ?2 AND start_s + duration_s > ?3
          ORDER BY slot LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        params![channel, day, after_s, limit as i64],
        program_from_row,
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// One channel's programs overlapping [from_s, to_s) of its day.
pub fn programs_between(
    conn: &Connection,
    channel: &str,
    day: &str,
    from_s: i64,
    to_s: i64,
) -> Result<Vec<Program>> {
    let mut stmt = conn.prepare_cached(
        "SELECT slot, start_s, duration_s, tidal_video_id, why FROM video_channel_day
          WHERE channel_id = ?1 AND day = ?2 AND start_s + duration_s > ?3 AND start_s < ?4
          ORDER BY slot",
    )?;
    let rows = stmt.query_map(params![channel, day, from_s, to_s], program_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// What airs from `now_s` on, crossing into tomorrow's schedule (built on
/// demand) when today's runs out. Tomorrow's programs are shifted onto
/// today's clock (start_s >= 86400).
pub fn upcoming(
    conn: &Connection,
    channel: &str,
    day: &str,
    now_s: i64,
    count: usize,
    explore: ExploreMode,
) -> Result<Vec<Program>> {
    let today = programs_after(conn, channel, day, now_s, count)?;
    if today.len() >= count {
        return Ok(today);
    }
    let Some(tomorrow) = clock::shift_day(day, 1) else {
        return Ok(today);
    };
    // Fewer than asked: 	oday holds the tail of the day.
    ensure(conn, &tomorrow, explore)?;
    let next_day = programs_after(conn, channel, &tomorrow, 0, count + TOMORROW_SLACK)?;
    Ok(clock::join(today, next_day)
        .into_iter()
        .filter(|program| program.end_s() > now_s)
        .take(count)
        .collect())
}

/// Catalog rows for the given videos, by id.
pub fn catalog_videos(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, VideoCandidate>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT tidal_video_id, item_json FROM video_catalog
          WHERE tidal_video_id IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt.query_map([serde_json::to_string(ids)?], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, json) = row?;
        if let Ok(video) = serde_json::from_str::<VideoCandidate>(&json) {
            out.insert(id, video);
        }
    }
    Ok(out)
}

pub fn item(video: &VideoCandidate, why: &str) -> VideoSetItem {
    let mut item = to_item(video);
    item.why = why.to_string();
    item
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgramOut {
    pub slot: i64,
    pub start_s: i64,
    pub duration_s: i64,
    pub video: VideoSetItem,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NowOut {
    #[serde(flatten)]
    pub program: ProgramOut,
    pub offset_s: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelOut {
    #[serde(flatten)]
    pub meta: ChannelMeta,
    pub now: Option<NowOut>,
    pub next: Option<ProgramOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelsOut {
    pub day: String,
    pub now_s: i64,
    pub explore: ExploreMode,
    pub building: bool,
    pub channels: Vec<ChannelOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GuideOut {
    pub channel_id: String,
    pub day: String,
    pub programs: Vec<ProgramOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TuneOut {
    pub channel: ChannelOut,
    pub video: VideoSetItem,
    pub offset_s: i64,
    pub queue: Vec<VideoSetItem>,
}

#[derive(Debug)]
pub enum Tune {
    On(Box<TuneOut>),
    Unknown,
    Locked,
    OffAir,
}

fn program_out(program: &Program, videos: &HashMap<i64, VideoCandidate>) -> Option<ProgramOut> {
    let video = videos.get(&program.video_id)?;
    Some(ProgramOut {
        slot: program.slot,
        start_s: program.start_s,
        duration_s: program.duration_s,
        video: item(video, &program.why),
        why: program.why.clone(),
    })
}

/// Now and next for one channel from its upcoming programs.
fn channel_out(
    meta: &ChannelMeta,
    upcoming: &[Program],
    now_s: i64,
    videos: &HashMap<i64, VideoCandidate>,
) -> ChannelOut {
    let (now, next) = match clock::locate(upcoming, now_s) {
        Some((index, offset)) => (Some((&upcoming[index], offset)), upcoming.get(index + 1)),
        None => (
            None,
            upcoming.iter().find(|program| program.start_s > now_s),
        ),
    };
    ChannelOut {
        meta: meta.clone(),
        now: now.and_then(|(program, offset_s)| {
            program_out(program, videos).map(|out| NowOut {
                program: out,
                offset_s,
            })
        }),
        next: next.and_then(|program| program_out(program, videos)),
    }
}

/// Every channel with what is on now (and how far in) and what is next.
pub fn channels_at(
    conn: &Connection,
    day: &str,
    now_s: i64,
    explore: ExploreMode,
) -> Result<ChannelsOut> {
    let lineup = ensure(conn, day, explore)?;
    let mut upcoming_by_channel: Vec<Vec<Program>> = Vec::with_capacity(lineup.channels.len());
    for meta in &lineup.channels {
        upcoming_by_channel.push(if meta.locked {
            Vec::new()
        } else {
            upcoming(conn, &meta.id, day, now_s, 2, explore)?
        });
    }
    let ids: Vec<i64> = upcoming_by_channel
        .iter()
        .flatten()
        .map(|program| program.video_id)
        .collect();
    let videos = catalog_videos(conn, &ids)?;
    let channels = lineup
        .channels
        .iter()
        .zip(&upcoming_by_channel)
        .map(|(meta, upcoming)| channel_out(meta, upcoming, now_s, &videos))
        .collect();
    Ok(ChannelsOut {
        day: lineup.day.clone(),
        now_s,
        explore: lineup.explore,
        building: !lineup.facets,
        channels,
    })
}

/// A window of one channel's programs. `to_s` past midnight reaches into
/// tomorrow's schedule (shifted onto today's clock).
pub fn guide_at(
    conn: &Connection,
    id: &str,
    day: &str,
    from_s: i64,
    to_s: i64,
    explore: ExploreMode,
) -> Result<Option<GuideOut>> {
    let Some(channel) = ChannelId::parse(id) else {
        return Ok(None);
    };
    let id = channel.as_string();
    let lineup = ensure(conn, day, explore)?;
    let Some(meta) = lineup.channel(&id) else {
        return Ok(None);
    };
    let mut programs = Vec::new();
    if !meta.locked {
        programs = programs_between(conn, &id, day, from_s, to_s)?;
        if to_s > DAY_S
            && let Some(tomorrow) = clock::shift_day(day, 1)
        {
            ensure(conn, &tomorrow, explore)?;
            let tail_end = programs_between(conn, &id, day, 0, DAY_S * 2)?
                .last()
                .map_or(DAY_S, Program::end_s)
                .max(DAY_S);
            let next = programs_between(
                conn,
                &id,
                &tomorrow,
                (from_s - DAY_S).max(tail_end - DAY_S),
                to_s - DAY_S,
            )?;
            programs.extend(next.into_iter().map(|mut program| {
                program.start_s += DAY_S;
                program
            }));
        }
    }
    let videos = catalog_videos(
        conn,
        &programs.iter().map(|p| p.video_id).collect::<Vec<_>>(),
    )?;
    Ok(Some(GuideOut {
        channel_id: id,
        day: lineup.day.clone(),
        programs: programs
            .iter()
            .filter_map(|program| program_out(program, &videos))
            .collect(),
    }))
}

/// Tune in: the scheduled video, where in it to start, and what follows.
/// Concert Hall's queue runs on to the end of the concert it reaches.
pub fn tune_at(
    conn: &Connection,
    id: &str,
    day: &str,
    now_s: i64,
    explore: ExploreMode,
) -> Result<Tune> {
    let Some(channel) = ChannelId::parse(id) else {
        return Ok(Tune::Unknown);
    };
    let id = channel.as_string();
    let lineup = ensure(conn, day, explore)?;
    let Some(meta) = lineup.channel(&id) else {
        return Ok(Tune::Unknown);
    };
    if meta.locked {
        return Ok(Tune::Locked);
    }
    let slack = if channel == ChannelId::Concerts {
        CONCERT_QUEUE_SLACK
    } else {
        0
    };
    let programs = upcoming(conn, &id, day, now_s, 1 + QUEUE + slack, explore)?;
    let videos = catalog_videos(
        conn,
        &programs.iter().map(|p| p.video_id).collect::<Vec<_>>(),
    )?;
    let playable: Vec<&Program> = programs
        .iter()
        .filter(|program| videos.contains_key(&program.video_id))
        .collect();
    let Some(first) = playable.first() else {
        return Ok(Tune::OffAir);
    };
    let mut queue_len = QUEUE.min(playable.len() - 1);
    if slack > 0 && queue_len > 0 {
        // Same artist and same concert label: still the same concert.
        let concert_of = |program: &Program| {
            (
                videos.get(&program.video_id).and_then(|v| v.artist_id),
                program.why.clone(),
            )
        };
        let last = concert_of(playable[queue_len]);
        while queue_len + 1 < playable.len() && concert_of(playable[queue_len + 1]) == last {
            queue_len += 1;
        }
    }
    let queue = playable[1..=queue_len]
        .iter()
        .map(|program| item(&videos[&program.video_id], &program.why))
        .collect();
    let owned: Vec<Program> = playable.iter().take(2).map(|p| (*p).clone()).collect();
    let channel_out = channel_out(meta, &owned, now_s, &videos);
    Ok(Tune::On(Box::new(TuneOut {
        channel: channel_out,
        video: item(&videos[&first.video_id], &first.why),
        offset_s: (now_s - first.start_s).max(0),
        queue,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_air::test_support::*;

    /// 60 artists x 12 songs of 4 minutes: 48 h of distinct songs.
    fn seed_catalog(conn: &Connection) {
        for artist in 0..60 {
            for n in 0..12 {
                let id = 10_000 + artist * 100 + n;
                add_video(
                    conn,
                    id,
                    500 + artist,
                    "official",
                    &format!("song {n}"),
                    240,
                );
            }
        }
    }

    #[test]
    fn a_day_is_built_once_and_frozen() {
        let conn = conn();
        seed_catalog(&conn);
        let first = ensure(&conn, "2026-10-14", ExploreMode::Mixed).unwrap();
        let wild = first.channel("wild").unwrap();
        assert!(!wild.locked);
        let programs = programs_between(&conn, "wild", "2026-10-14", 0, DAY_S * 2).unwrap();
        assert!(programs.last().unwrap().end_s() >= DAY_S);
        let built_at = first.built_at.clone();
        let again = ensure(&conn, "2026-10-14", ExploreMode::Mixed).unwrap();
        assert_eq!(again.built_at, built_at, "served from the store");
        let rebuilt = build(&conn, "2026-10-14", ExploreMode::Mixed).unwrap();
        let replay = programs_between(&conn, "wild", "2026-10-14", 0, DAY_S * 2).unwrap();
        assert_eq!(replay, programs, "same seeds, same day");
        assert_eq!(rebuilt.channels, first.channels);
    }

    #[test]
    fn turning_the_knob_rebuilds_today() {
        let conn = conn();
        seed_catalog(&conn);
        let mixed = ensure(&conn, "2026-10-14", ExploreMode::Mixed).unwrap();
        let adventurous = ensure(&conn, "2026-10-14", ExploreMode::Adventurous).unwrap();
        assert_eq!(adventurous.explore, ExploreMode::Adventurous);
        assert_ne!(adventurous.built_at, mixed.built_at);
    }

    #[test]
    fn a_fallback_day_is_rebuilt_once_the_intelligence_lands() {
        let conn = conn();
        for artist in 0..60 {
            for n in 0..12 {
                add_catalog(
                    &conn,
                    10_000 + artist * 100 + n,
                    500 + artist,
                    &format!("Song {n}"),
                    240,
                );
            }
        }
        let fallback = ensure(&conn, "2026-10-14", ExploreMode::Mixed).unwrap();
        assert!(!fallback.facets);
        let out = channels_at(&conn, "2026-10-14", 3_600, ExploreMode::Mixed).unwrap();
        assert!(out.building);
        assert!(
            !out.channels
                .iter()
                .find(|c| c.meta.id == "wild")
                .unwrap()
                .meta
                .locked
        );
        add_facet(&conn, 10_000, 500, "official", "id:500:song 0", 0.5);
        let built = ensure(&conn, "2026-10-14", ExploreMode::Mixed).unwrap();
        assert!(built.facets);
    }

    #[test]
    fn old_days_are_pruned() {
        let conn = conn();
        seed_catalog(&conn);
        for day in [
            "2026-10-10",
            "2026-10-11",
            "2026-10-12",
            "2026-10-13",
            "2026-10-14",
        ] {
            ensure(&conn, day, ExploreMode::Mixed).unwrap();
        }
        let days: Vec<String> = conn
            .prepare("SELECT DISTINCT day FROM video_channel_day ORDER BY day")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(days, vec!["2026-10-12", "2026-10-13", "2026-10-14"]);
        assert!(load_lineup(&conn, "2026-10-11").unwrap().is_none());
        assert!(load_lineup(&conn, "2026-10-12").unwrap().is_some());
    }

    #[test]
    fn channels_report_now_with_offset_and_next() {
        let conn = conn();
        seed_catalog(&conn);
        let out = channels_at(&conn, "2026-10-14", 30_000, ExploreMode::Mixed).unwrap();
        assert_eq!(out.now_s, 30_000);
        let wild = out.channels.iter().find(|c| c.meta.id == "wild").unwrap();
        let now = wild.now.as_ref().unwrap();
        assert!(
            now.program.start_s <= 30_000 && 30_000 < now.program.start_s + now.program.duration_s
        );
        assert_eq!(now.offset_s, 30_000 - now.program.start_s);
        let next = wild.next.as_ref().unwrap();
        assert_eq!(next.start_s, now.program.start_s + now.program.duration_s);
        let json = serde_json::to_value(&out).unwrap();
        let channel = &json["channels"][1];
        for key in [
            "id", "no", "title", "subtitle", "hue", "scene_id", "locked", "unlock", "now", "next",
        ] {
            assert!(channel.get(key).is_some(), "{key}");
        }
        assert_eq!(channel["now"]["offset_s"], now.offset_s);
        assert!(channel["now"]["video"]["tidal_id"].is_i64());
        assert!(channel["now"]["video"]["type"].is_string());
        assert_eq!(json["explore"], "mixed");
        let locked = json["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "live")
            .unwrap();
        assert_eq!(locked["locked"], true);
        assert!(locked["now"].is_null());
    }

    #[test]
    fn tuning_in_lands_mid_video_and_queues_what_follows() {
        let conn = conn();
        seed_catalog(&conn);
        let Tune::On(tune) =
            tune_at(&conn, "wild", "2026-10-14", 21_650, ExploreMode::Mixed).unwrap()
        else {
            panic!("on air");
        };
        let programs = programs_between(&conn, "wild", "2026-10-14", 0, DAY_S * 2).unwrap();
        let (index, offset) = clock::locate(&programs, 21_650).unwrap();
        assert_eq!(tune.video.tidal_id, programs[index].video_id);
        assert_eq!(tune.offset_s, offset);
        let queued: Vec<i64> = tune.queue.iter().map(|v| v.tidal_id).collect();
        let expected: Vec<i64> = programs[index + 1..index + 1 + QUEUE]
            .iter()
            .map(|p| p.video_id)
            .collect();
        assert_eq!(queued, expected);
        assert!(matches!(
            tune_at(&conn, "live", "2026-10-14", 100, ExploreMode::Mixed).unwrap(),
            Tune::Locked
        ));
        assert!(matches!(
            tune_at(&conn, "nope", "2026-10-14", 100, ExploreMode::Mixed).unwrap(),
            Tune::Unknown
        ));
        assert!(matches!(
            tune_at(&conn, "scene:99", "2026-10-14", 100, ExploreMode::Mixed).unwrap(),
            Tune::Unknown
        ));
    }

    #[test]
    fn tuning_in_before_midnight_queues_into_tomorrow() {
        let conn = conn();
        seed_catalog(&conn);
        let today = "2026-10-14";
        ensure(&conn, today, ExploreMode::Mixed).unwrap();
        let programs = programs_between(&conn, "wild", today, 0, DAY_S * 2).unwrap();
        let last = programs.last().unwrap().clone();
        let t = last.start_s + 5;
        let Tune::On(tune) = tune_at(&conn, "wild", today, t, ExploreMode::Mixed).unwrap() else {
            panic!("on air");
        };
        assert_eq!(tune.video.tidal_id, last.video_id);
        assert_eq!(tune.offset_s, 5);
        assert_eq!(tune.queue.len(), QUEUE);
        let tomorrow = programs_between(&conn, "wild", "2026-10-15", 0, DAY_S * 2).unwrap();
        let timeline = clock::join(programs.clone(), tomorrow);
        let (index, _) = clock::locate(&timeline, t).unwrap();
        let expected: Vec<i64> = timeline[index + 1..index + 1 + QUEUE]
            .iter()
            .map(|p| p.video_id)
            .collect();
        let queued: Vec<i64> = tune.queue.iter().map(|v| v.tidal_id).collect();
        assert_eq!(queued, expected);
        let next = tune.channel.next.as_ref().unwrap();
        assert!(
            next.start_s >= DAY_S,
            "next airs after midnight on today's clock"
        );
        let out = channels_at(&conn, today, t, ExploreMode::Mixed).unwrap();
        let wild = out.channels.iter().find(|c| c.meta.id == "wild").unwrap();
        assert_eq!(wild.next.as_ref().unwrap().start_s, next.start_s);
    }

    #[test]
    fn the_guide_returns_a_window_and_reaches_past_midnight() {
        let conn = conn();
        seed_catalog(&conn);
        let guide = guide_at(
            &conn,
            "wild",
            "2026-10-14",
            3_600,
            7_200,
            ExploreMode::Mixed,
        )
        .unwrap()
        .unwrap();
        assert!(!guide.programs.is_empty());
        assert!(
            guide
                .programs
                .iter()
                .all(|p| p.start_s < 7_200 && p.start_s + p.duration_s > 3_600)
        );
        let late = guide_at(
            &conn,
            "wild",
            "2026-10-14",
            DAY_S - 600,
            DAY_S + 1_800,
            ExploreMode::Mixed,
        )
        .unwrap()
        .unwrap();
        assert!(late.programs.iter().any(|p| p.start_s >= DAY_S));
        for pair in late.programs.windows(2) {
            assert!(pair[0].start_s < pair[1].start_s);
        }
        assert!(
            guide_at(&conn, "nope", "2026-10-14", 0, 10, ExploreMode::Mixed)
                .unwrap()
                .is_none()
        );
    }

    /// Timing and a sample day on a copy of a real database with the derived
    /// tables filled in. Writes to that copy only:
    /// `NOOR_AIR_DB=<copy> cargo test --release real_db -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_db_day_builds_and_serves_fast() {
        let Ok(path) = std::env::var("NOOR_AIR_DB") else {
            return;
        };
        let conn = Connection::open(path).unwrap();
        let (day, now_s) = clock::local_now();
        let explore = crate::services::video_air::load_explore(&conn).unwrap();
        let started = Instant::now();
        let universe = Universe::load(&conn).unwrap();
        let loaded = started.elapsed();
        let taste = Taste::load(&conn, &universe).unwrap();
        let tasted = started.elapsed();
        let lineup = build_with(&conn, &day, explore, &universe, &taste).unwrap();
        println!(
            "build: universe {} ms, taste {} ms, total {} ms ({} videos, {} roots, {} channels, {} on air)",
            loaded.as_millis(),
            (tasted - loaded).as_millis(),
            started.elapsed().as_millis(),
            universe.videos.len(),
            taste.roots.len(),
            lineup.channels.len(),
            lineup.channels.iter().filter(|c| !c.locked).count()
        );
        for _ in 0..2 {
            let started = Instant::now();
            let universe = Universe::load(&conn).unwrap();
            let t_universe = started.elapsed();
            let graph = crate::services::video_discovery::graph::Graph::load(&conn).unwrap();
            let t_graph = started.elapsed();
            drop(graph);
            let taste = Taste::load(&conn, &universe).unwrap();
            let t_taste = started.elapsed();
            let ctx = DayContext {
                explore,
                spotlight: spotlight(&conn, &taste, &day).unwrap(),
                rewind_decade: channels::rewind_decade(&universe, &day),
            };
            let t_ctx = started.elapsed();
            let planned = channels::plan(&universe, &taste, &ctx);
            let t_plan = started.elapsed();
            let schedules: Vec<(String, Vec<Program>)> = planned
                .iter()
                .filter_map(|c| {
                    c.pool.as_ref().map(|pool| {
                        (
                            c.meta.id.clone(),
                            programmer::schedule_day(pool, &c.meta.id, &day),
                        )
                    })
                })
                .collect();
            let t_schedule = started.elapsed();
            let mut stored = lineup.clone();
            stored.built_at = chrono::Utc::now().to_rfc3339();
            store(&conn, &stored, &schedules).unwrap();
            let t_store = started.elapsed();
            println!(
                "phases ms: universe {} | graph alone {} | taste (with graph) {} | spotlight+decade {} | plan {} | schedule {} | store {} | total {}",
                t_universe.as_millis(),
                (t_graph - t_universe).as_millis(),
                (t_taste - t_graph).as_millis(),
                (t_ctx - t_taste).as_millis(),
                (t_plan - t_ctx).as_millis(),
                (t_schedule - t_plan).as_millis(),
                (t_store - t_schedule).as_millis(),
                t_store.as_millis()
            );
        }
        let ctx = lineup.context();
        let mut costs: Vec<(u128, String)> = lineup
            .channels
            .iter()
            .filter_map(|meta| channels::ChannelId::parse(&meta.id))
            .map(|id| {
                let started = Instant::now();
                let planned = channels::plan_one(&id, &universe, &taste, &ctx);
                let units: usize = planned
                    .pool
                    .as_ref()
                    .map_or(0, |pool| pool.buckets.iter().map(|b| b.units.len()).sum());
                (
                    started.elapsed().as_micros(),
                    format!("{} ({units} units)", id.as_string()),
                )
            })
            .collect();
        costs.sort_by_key(|(us, _)| std::cmp::Reverse(*us));
        println!(
            "slowest plans: {}",
            costs
                .iter()
                .take(8)
                .map(|(us, id)| format!("{id} {} ms", us / 1000))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let started = Instant::now();
        let full = build(&conn, &day, explore).unwrap();
        println!(
            "cold build via build(): {} ms",
            started.elapsed().as_millis()
        );
        assert_eq!(full.channels.len(), lineup.channels.len());
        for _ in 0..3 {
            let started = Instant::now();
            let out = channels_at(&conn, &day, now_s, explore).unwrap();
            println!(
                "channels: {} ms ({} channels)",
                started.elapsed().as_millis(),
                out.channels.len()
            );
        }
        let started = Instant::now();
        let mut tuned = 0;
        for meta in lineup.channels.iter().filter(|c| !c.locked) {
            if let Tune::On(_) = tune_at(&conn, &meta.id, &day, now_s, explore).unwrap() {
                tuned += 1;
            }
        }
        println!(
            "tune: {} channels in {} ms",
            tuned,
            started.elapsed().as_millis()
        );
        let started = Instant::now();
        let guide = guide_at(&conn, "wild", &day, 0, DAY_S, explore)
            .unwrap()
            .unwrap();
        println!(
            "guide (whole day): {} programs in {} ms",
            guide.programs.len(),
            started.elapsed().as_millis()
        );
        let started = Instant::now();
        let request = crate::services::video_air::session::NextRequest {
            session_nonce: "perf".into(),
            ..Default::default()
        };
        let items = crate::services::video_air::session::refill(
            &conn, "wild", &day, explore, &request, &universe, &taste,
        )
        .unwrap();
        println!(
            "refill (warm universe): {} items in {} ms",
            items.len(),
            started.elapsed().as_millis()
        );
        for meta in &lineup.channels {
            println!(
                "{:>3} {:<28} {:<10} {}",
                meta.no,
                meta.title,
                if meta.locked {
                    format!(
                        "locked {}",
                        meta.unlock
                            .map(|u| format!("{}/{} h", u.hours, u.need_hours))
                            .unwrap_or_default()
                    )
                } else {
                    "on air".into()
                },
                meta.subtitle
            );
        }
        for id in [
            "your",
            "wild",
            "premieres",
            "live",
            "sessions",
            "concerts",
            "rewind",
            "spotlight",
        ]
        .into_iter()
        .map(String::from)
        .chain(
            lineup
                .channels
                .iter()
                .filter(|c| c.scene_id.is_some() && !c.locked)
                .take(2)
                .map(|c| c.id.clone()),
        ) {
            let programs = upcoming(&conn, &id, &day, now_s, 6, explore).unwrap();
            let videos = catalog_videos(
                &conn,
                &programs.iter().map(|p| p.video_id).collect::<Vec<_>>(),
            )
            .unwrap();
            println!("--- {id}");
            for program in programs {
                let video = &videos[&program.video_id];
                println!(
                    "  {:02}:{:02} {:>4}s  {} - {}  [{}]",
                    program.start_s / 3600 % 24,
                    program.start_s / 60 % 60,
                    program.duration_s,
                    video.artist_name.as_deref().unwrap_or("?"),
                    video.title,
                    program.why
                );
            }
        }
    }

    #[test]
    fn concert_hall_queues_whole_concerts() {
        let conn = conn();
        // 40 artists x 2 concerts x 8 songs x 5 min: 53 h of concerts.
        let mut concert = 1;
        for artist in 0..40 {
            for c in 0..2 {
                let ids: Vec<i64> = (0..8)
                    .map(|n| 100_000 + artist * 100 + c * 10 + n)
                    .collect();
                for (n, id) in ids.iter().enumerate() {
                    add_video(
                        &conn,
                        *id,
                        700 + artist,
                        "live",
                        &format!("live {c} {n}"),
                        300,
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
        let lineup = ensure(&conn, "2026-10-14", ExploreMode::Mixed).unwrap();
        assert!(!lineup.channel("concerts").unwrap().locked);
        let programs = programs_between(&conn, "concerts", "2026-10-14", 0, DAY_S * 2).unwrap();
        let mut index = 0;
        while index < programs.len() {
            let label = programs[index].why.clone();
            let run = programs[index..]
                .iter()
                .take_while(|p| p.why == label)
                .count();
            assert_eq!(run, 8, "{label} plays whole");
            let ids: Vec<i64> = programs[index..index + run]
                .iter()
                .map(|p| p.video_id)
                .collect();
            let mut sorted = ids.clone();
            sorted.sort();
            assert_eq!(ids, sorted, "setlist order");
            index += run;
        }
        let Tune::On(tune) =
            tune_at(&conn, "concerts", "2026-10-14", 10, ExploreMode::Mixed).unwrap()
        else {
            panic!("on air");
        };
        let all: Vec<&str> = std::iter::once(tune.video.why.as_str())
            .chain(tune.queue.iter().map(|v| v.why.as_str()))
            .collect();
        let last = *all.last().unwrap();
        let tail = all.iter().rev().take_while(|w| **w == last).count();
        assert_eq!(tail, 8, "the queue ends on a whole concert");
    }
}
