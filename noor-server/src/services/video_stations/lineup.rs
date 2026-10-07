//! The day's station lineup: which stations exist, each with a preview.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::pool::{self, Listener, VARIOUS_ARTISTS_ID};
use super::{BATCH, StationId, Vibe, pick, pick_input, seed_for, to_item};
use crate::services::video_discovery::graph;
use crate::services::video_sets::VideoSetItem;

pub const MIN_UNWATCHED: usize = 30;
pub const SPOTLIGHT_MIN_VIDEOS: usize = 8;
const MAX_GENRES: usize = 6;
const SPOTLIGHT_HISTORY_KEY: &str = "video_stations.spotlight_history";
const SPOTLIGHT_MEMORY: usize = 30;
/// Exclusions can leave a big catalog artist with few playable videos, so a
/// few runners-up are tried before the day goes without a spotlight.
const SPOTLIGHT_TRIES: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationCard {
    pub id: String,
    pub group: String,
    pub title: String,
    pub subtitle: String,
    pub unwatched_count: i64,
    pub preview: Vec<VideoSetItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SpotlightDay {
    artist_id: i64,
    day: String,
}

fn preview(candidates: Vec<pick::Candidate>, station: &StationId, day: &str) -> Vec<VideoSetItem> {
    let excluded = HashSet::new();
    pick::pick(
        candidates,
        &pick_input(station, &excluded, seed_for(station, day, "preview"), BATCH),
    )
    .iter()
    .map(to_item)
    .collect()
}

fn top_genres(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT g.slug, g.name FROM listen_history lh
           JOIN tracks t ON t.id = lh.track_id
           JOIN track_genres tg ON tg.track_id = t.id
           JOIN genres g ON g.id = tg.genre_id
          WHERE COALESCE(lh.source, '') NOT IN ('radio', 'automix')
          GROUP BY g.id ORDER BY COUNT(*) DESC, g.name LIMIT ?1",
    )?;
    let rows = stmt.query_map([MAX_GENRES as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(slug, _)| StationId::parse(&format!("genre:{slug}")).is_some())
        .collect())
}

fn spotlight_history(conn: &Connection) -> Result<Vec<SpotlightDay>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [SPOTLIGHT_HISTORY_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default())
}

fn remember_spotlight(
    conn: &Connection,
    mut history: Vec<SpotlightDay>,
    artist_id: i64,
    day: &str,
) -> Result<()> {
    history.push(SpotlightDay {
        artist_id,
        day: day.to_string(),
    });
    let keep = history.len().saturating_sub(SPOTLIGHT_MEMORY);
    let history = &history[keep..];
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SPOTLIGHT_HISTORY_KEY, serde_json::to_string(history)?],
    )?;
    Ok(())
}

/// Artists the listener has never watched or liked, with enough videos, not
/// spotlighted recently, closest to their taste first.
fn spotlight_choices(
    conn: &Connection,
    listener: &Listener,
    history: &[SpotlightDay],
) -> Result<Vec<i64>> {
    let recent: HashSet<i64> = history.iter().map(|entry| entry.artist_id).collect();
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id, COUNT(*) FROM video_catalog
          WHERE artist_tidal_id > 0 GROUP BY artist_tidal_id HAVING COUNT(*) >= ?1",
    )?;
    let rows = stmt.query_map([SPOTLIGHT_MIN_VIDEOS as i64], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut ranked: Vec<(f64, i64, i64)> = Vec::new();
    for row in rows {
        let (artist, count) = row?;
        if artist == VARIOUS_ARTISTS_ID
            || recent.contains(&artist)
            || listener.watched_artists.contains(&artist)
            || listener.liked.contains(&artist)
        {
            continue;
        }
        let relevance = listener.relevance.get(&artist).copied().unwrap_or(0.0);
        ranked.push((relevance, count, artist));
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    Ok(ranked
        .into_iter()
        .take(SPOTLIGHT_TRIES)
        .map(|(_, _, artist)| artist)
        .collect())
}

fn near_liked(conn: &Connection, listener: &Listener, artist: i64) -> Result<Vec<String>> {
    let graph = graph::cached(conn)?;
    let mut names = Vec::new();
    for (neighbor, _) in graph
        .neighbors(artist)
        .iter()
        .filter(|(id, _)| listener.liked.contains(id))
    {
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM video_artist_state WHERE artist_tidal_id = ?1 AND name <> ''",
                [neighbor],
                |row| row.get(0),
            )
            .optional()?;
        names.extend(name);
        if names.len() == 2 {
            break;
        }
    }
    Ok(names)
}

fn spotlight(conn: &Connection, listener: &Listener, day: &str) -> Result<Option<StationCard>> {
    let history = spotlight_history(conn)?;
    let todays = history
        .iter()
        .find(|entry| entry.day == day)
        .map(|entry| entry.artist_id);
    let choices = match todays {
        Some(artist) => vec![artist],
        None => spotlight_choices(conn, listener, &history)?,
    };
    let Some((artist, candidates, unwatched)) = choices
        .into_iter()
        .find_map(|artist| {
            let station = StationId::Spotlight(artist);
            match pool::candidates(conn, listener, &station) {
                Ok(candidates) => {
                    let unwatched = pick::unwatched_songs(&candidates);
                    (unwatched >= SPOTLIGHT_MIN_VIDEOS)
                        .then_some(Ok((artist, candidates, unwatched)))
                }
                Err(err) => Some(Err(err)),
            }
        })
        .transpose()?
    else {
        return Ok(None);
    };
    let station = StationId::Spotlight(artist);
    if todays.is_none() {
        remember_spotlight(conn, history, artist, day)?;
    }
    let name = candidates
        .first()
        .and_then(|c| c.video.artist_name.clone())
        .unwrap_or_default();
    let near = near_liked(conn, listener, artist)?;
    let subtitle = if near.is_empty() {
        format!("{unwatched} videos you haven't seen")
    } else {
        format!(
            "Near {}. {unwatched} videos you haven't seen.",
            near.join(" and ")
        )
    };
    Ok(Some(StationCard {
        id: station.as_string(),
        group: "spotlight".into(),
        title: name,
        subtitle,
        unwatched_count: unwatched as i64,
        preview: preview(candidates, &station, day),
    }))
}

fn planned(conn: &Connection) -> Result<Vec<(StationId, &'static str, String, String)>> {
    let mut stations = vec![
        (
            StationId::WildCard,
            "for_you",
            "Wild card".to_string(),
            "Mostly near your taste, some far".to_string(),
        ),
        (
            StationId::Shuffle,
            "for_you",
            "Pure shuffle".into(),
            "Anything you haven't seen".into(),
        ),
        (
            StationId::DeepCuts,
            "for_you",
            "Deep cuts".into(),
            "Artists you like, videos you haven't seen".into(),
        ),
        (
            StationId::BigOnes,
            "for_you",
            "Big ones you missed".into(),
            "The most popular videos you haven't watched".into(),
        ),
    ];
    for (slug, name) in top_genres(conn)? {
        stations.push((
            StationId::Genre(slug),
            "genres",
            name,
            "Artists you like and ones you don't know yet".into(),
        ));
    }
    for vibe in Vibe::ALL {
        stations.push((
            StationId::Vibe(vibe),
            "vibes",
            vibe.title().into(),
            vibe.subtitle().into(),
        ));
    }
    stations.push((
        StationId::Duets,
        "themes",
        "Duets and features".into(),
        "Hop from artist to artist".into(),
    ));
    stations.push((
        StationId::Live,
        "themes",
        "Live and acoustic".into(),
        "On stage, unplugged and in session".into(),
    ));
    stations.push((
        StationId::Charts,
        "charts",
        "Charts on camera".into(),
        "Today's charts, as videos".into(),
    ));
    Ok(stations)
}

/// Build, store and return the lineup for `day` (local date, YYYY-MM-DD).
pub fn build(conn: &Connection, day: &str) -> Result<Vec<StationCard>> {
    let listener = pool::load_listener(conn)?;
    let mut cards = Vec::new();
    if let Some(card) = spotlight(conn, &listener, day)? {
        cards.push(card);
    }
    for (station, group, title, subtitle) in planned(conn)? {
        let candidates = pool::candidates(conn, &listener, &station)?;
        let unwatched = pick::unwatched_songs(&candidates);
        if unwatched < MIN_UNWATCHED {
            continue;
        }
        cards.push(StationCard {
            id: station.as_string(),
            group: group.into(),
            title,
            subtitle,
            unwatched_count: unwatched as i64,
            preview: preview(candidates, &station, day),
        });
    }
    save(conn, day, &cards)?;
    Ok(cards)
}

/// Replace `day`'s rows and keep only that day and the day before.
pub fn save(conn: &Connection, day: &str, cards: &[StationCard]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM video_station_lineup WHERE day = ?1", [day])?;
    for (position, card) in cards.iter().enumerate() {
        tx.execute(
            "INSERT INTO video_station_lineup
                 (day, station_id, position, grp, title, subtitle, unwatched_count, preview_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                day,
                card.id,
                position as i64,
                card.group,
                card.title,
                card.subtitle,
                card.unwatched_count,
                serde_json::to_string(&card.preview)?,
            ],
        )?;
    }
    tx.execute(
        "DELETE FROM video_station_lineup WHERE day < date(?1, '-1 day')",
        [day],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn load(conn: &Connection, day: &str) -> Result<Vec<StationCard>> {
    let mut stmt = conn.prepare(
        "SELECT station_id, grp, title, subtitle, unwatched_count, preview_json
           FROM video_station_lineup WHERE day = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([day], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut cards = Vec::new();
    for row in rows {
        let (id, group, title, subtitle, unwatched_count, preview_json) = row?;
        cards.push(StationCard {
            id,
            group,
            title,
            subtitle,
            unwatched_count,
            preview: serde_json::from_str(&preview_json).unwrap_or_default(),
        });
    }
    Ok(cards)
}

/// The most recent stored lineup, for while today's is being built.
pub fn latest(conn: &Connection) -> Result<Option<(String, Vec<StationCard>)>> {
    let day: Option<String> =
        conn.query_row("SELECT MAX(day) FROM video_station_lineup", [], |row| {
            row.get(0)
        })?;
    match day {
        Some(day) => {
            let cards = load(conn, &day)?;
            Ok(Some((day, cards)))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_stations::pool::tests::{add_video, conn, video};

    fn seed_catalog(conn: &Connection) {
        for id in 1..=40 {
            add_video(conn, &video(id, 1000 + id));
        }
        for id in 101..=110 {
            add_video(conn, &video(id, 77));
        }
        for id in 201..=210 {
            add_video(conn, &video(id, 88));
        }
    }

    #[test]
    fn only_stations_with_enough_unwatched_videos_are_listed() {
        let conn = conn();
        seed_catalog(&conn);
        let cards = build(&conn, "2026-10-07").unwrap();
        let ids: Vec<&str> = cards.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"wild-card"));
        assert!(ids.contains(&"shuffle"));
        assert!(!ids.contains(&"deep-cuts"), "no liked artists");
        assert!(!ids.contains(&"big-ones"), "no popularity known");
        let shuffle = cards.iter().find(|c| c.id == "shuffle").unwrap();
        assert_eq!(shuffle.preview.len(), BATCH);
        assert_eq!(shuffle.unwatched_count, 60);
    }

    #[test]
    fn the_spotlight_holds_for_the_day_and_rotates_after() {
        let conn = conn();
        seed_catalog(&conn);
        let first = build(&conn, "2026-10-07").unwrap();
        let today = first
            .iter()
            .find(|c| c.group == "spotlight")
            .unwrap()
            .id
            .clone();
        let again = build(&conn, "2026-10-07").unwrap();
        assert_eq!(
            again.iter().find(|c| c.group == "spotlight").unwrap().id,
            today
        );
        let tomorrow = build(&conn, "2026-10-08").unwrap();
        let next = &tomorrow.iter().find(|c| c.group == "spotlight").unwrap().id;
        assert_ne!(next, &today);
        assert!(today == "spotlight:77" || today == "spotlight:88");
    }

    #[test]
    fn a_spotlight_artist_without_playable_videos_passes_to_the_next() {
        let conn = conn();
        for id in 101..=110 {
            let mut clip = video(id, 77);
            clip.duration_s = Some(30);
            add_video(&conn, &clip);
        }
        for id in 201..=210 {
            add_video(&conn, &video(id, 88));
        }
        let cards = build(&conn, "2026-10-07").unwrap();
        let spotlight = cards.iter().find(|c| c.group == "spotlight").unwrap();
        assert_eq!(spotlight.id, "spotlight:88");
    }

    #[test]
    fn only_today_and_yesterday_are_kept() {
        let conn = conn();
        let card = StationCard {
            id: "shuffle".into(),
            group: "for_you".into(),
            title: "Pure shuffle".into(),
            subtitle: String::new(),
            unwatched_count: 40,
            preview: Vec::new(),
        };
        for day in ["2026-10-05", "2026-10-06", "2026-10-07"] {
            save(&conn, day, std::slice::from_ref(&card)).unwrap();
        }
        assert!(load(&conn, "2026-10-05").unwrap().is_empty());
        assert_eq!(load(&conn, "2026-10-06").unwrap().len(), 1);
        assert_eq!(latest(&conn).unwrap().unwrap().0, "2026-10-07");
    }
}
