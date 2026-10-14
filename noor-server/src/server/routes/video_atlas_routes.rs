//! The Atlas: the whole map in one columnar payload, lazy names, artist
//! and scene cards (an artist's with their collabs), probes into the
//! crawler, routes between artists and journeys somewhere new.
//!
//! Every read is local: the derived tables from services::video_intel plus
//! the listener's light, computed per request. Only a probe reaches the
//! crawler, and only through its existing urgent-station path. Wire shapes:
//! frontend/src/lib/api/video_explore_types.ts.
use std::collections::{HashMap, HashSet};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::SharedState;
use crate::db::Database;
use crate::services::video_air::day;
use crate::services::video_discovery::crawler::{self, Urgent};
use crate::services::video_discovery::setting::{self as discovery_setting, DiscoverySetting};
use crate::services::video_discovery::{artist_state, roots};
use crate::services::video_intel::journey;
use crate::services::video_intel::layout::MAP_SIZE;
use crate::services::video_intel::light::{self, LightMap};
use crate::services::video_intel::scenes::{self, MIN_PROGRAMMABLE_S, VARIOUS_ARTISTS_ID};
use crate::services::video_radio::video_song_key;
use crate::services::video_sets::{VideoCandidate, VideoSetItem};
use crate::services::video_stations::to_item;

pub(super) type ApiError = (StatusCode, Json<Value>);

const BEST_CUTS: usize = 8;
const NEIGHBORS: usize = 12;
const COLLABS: usize = 12;
const SCENE_TOP_ARTISTS: usize = 12;
const SCENE_SAMPLE: usize = 12;

fn error(status: StatusCode, message: &str) -> ApiError {
    (status, Json(json!({ "error": message })))
}

fn internal(err: anyhow::Error) -> ApiError {
    tracing::warn!(target: "noor.video_atlas", error = %err, "atlas read failed");
    error(StatusCode::INTERNAL_SERVER_ERROR, "atlas read failed")
}

/// Run a read on its own connection off the async runtime; in-memory test
/// databases fall back to the shared one.
pub(super) async fn read<T, F>(state: &SharedState, f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
{
    let db: Database = state.read().await.db.clone();
    tokio::task::spawn_blocking(move || match db.open_isolated() {
        Ok(conn) => f(&conn),
        Err(_) => db.with_conn(f),
    })
    .await
    .map_err(|err| internal(err.into()))?
    .map_err(internal)
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

// --------------------------------------------------------------- shapes

#[derive(Debug, Serialize)]
pub(super) struct AtlasScene {
    id: i64,
    name: String,
    exemplars: Vec<String>,
    tags: Vec<String>,
    hue: f64,
    x: f64,
    y: f64,
    r: f64,
    videos: i64,
    artists: i64,
    hours: f64,
    channel_no: Option<i64>,
    lit_artists: i64,
    seen_videos: i64,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct AtlasArtists {
    ids: Vec<i64>,
    x: Vec<f32>,
    y: Vec<f32>,
    scene: Vec<i64>,
    videos: Vec<i64>,
    light: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub(super) struct AtlasResponse {
    built_at: Option<String>,
    building: bool,
    size: u32,
    scenes: Vec<AtlasScene>,
    artists: AtlasArtists,
    recent_artist_ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
pub(super) struct AtlasNamesResponse {
    ids: Vec<i64>,
    names: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AtlasNeighbor {
    id: i64,
    name: String,
    source: &'static str,
    weight: f64,
    light: u8,
}

/// Someone the artist shares a video with, and the best such video.
#[derive(Debug, Serialize)]
struct AtlasCollab {
    id: i64,
    name: String,
    light: u8,
    video: Option<VideoSetItem>,
}

#[derive(Debug, Serialize)]
pub(super) struct AtlasArtistCard {
    id: i64,
    name: String,
    scene_id: Option<i64>,
    scene_name: Option<String>,
    videos: i64,
    playable: i64,
    hours: f64,
    light: u8,
    charted: bool,
    genres: Vec<String>,
    best: Vec<VideoSetItem>,
    neighbors: Vec<AtlasNeighbor>,
    /// The best `COLLABS` collaborators, best video first.
    collabs: Vec<AtlasCollab>,
    collabs_total: usize,
}

#[derive(Debug, Serialize)]
struct SceneArtist {
    id: i64,
    name: String,
    videos: i64,
    light: u8,
}

#[derive(Debug, Serialize)]
pub(super) struct AtlasSceneCard {
    scene: AtlasScene,
    top_artists: Vec<SceneArtist>,
    sample: Vec<VideoSetItem>,
}

#[derive(Debug, Serialize)]
pub(super) struct AtlasProbeResponse {
    queued: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct JourneyStop {
    artist_id: i64,
    name: String,
    light: u8,
    video: Option<VideoSetItem>,
    why: String,
}

#[derive(Debug, Serialize)]
struct JourneyDestination {
    scene_id: i64,
    name: String,
}

/// `AtlasJourneyResponse` on the wire: every field always present.
#[derive(Debug, Serialize)]
pub(super) struct AtlasJourneyResponse {
    found: bool,
    kind: &'static str,
    title: String,
    /// Somewhere new only.
    destination: Option<JourneyDestination>,
    stops: Vec<JourneyStop>,
    reason: Option<String>,
}

impl AtlasJourneyResponse {
    fn lost(kind: &'static str, title: String, reason: impl Into<String>) -> Self {
        Self {
            found: false,
            kind,
            title,
            destination: None,
            stops: Vec::new(),
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum JourneyRequest {
    New { from: i64, scene: Option<i64> },
}

impl JourneyRequest {
    /// kind=new&from[&scene]; Err is the 400 message.
    fn parse(params: &HashMap<String, String>) -> Result<Self, &'static str> {
        // Absent is None, anything but a positive integer is an error.
        let id = |name: &str| -> Result<Option<i64>, ()> {
            match params.get(name).map(|v| v.trim()).filter(|v| !v.is_empty()) {
                None => Ok(None),
                Some(value) => value
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .map(Some)
                    .ok_or(()),
            }
        };
        let kind = params.get("kind").map(String::as_str);
        if kind != Some("new") {
            return Err("kind must be new");
        }
        let from = id("from")
            .ok()
            .flatten()
            .ok_or("from must be an artist id")?;
        let scene = id("scene").map_err(|_| "scene must be a scene id")?;
        Ok(Self::New { from, scene })
    }
}

// --------------------------------------------------------------- reads

/// The listener's light over the cached graph.
fn light_map(conn: &Connection) -> anyhow::Result<LightMap> {
    let graph = scenes::cached_graph(conn)?;
    light::load(conn, &graph)
}

/// Distinct watched videos per artist.
fn seen_per_artist(conn: &Connection) -> anyhow::Result<HashMap<i64, i64>> {
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id, COUNT(DISTINCT tidal_video_id) FROM video_history
          WHERE artist_tidal_id > 0 GROUP BY artist_tidal_id",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<HashMap<i64, i64>, _>>()?;
    Ok(rows)
}

struct SceneRow {
    id: i64,
    name: String,
    exemplars: String,
    tags: String,
    hue: f64,
    x: f64,
    y: f64,
    r: f64,
    videos: i64,
    artists: i64,
    hours: f64,
    channel_no: Option<i64>,
}

const SCENE_COLUMNS: &str = "scene_id, name, exemplars_json, tags_json, hue, x, y, r,
    video_count, artist_count, hours, channel_no";

fn scene_row(row: &rusqlite::Row) -> rusqlite::Result<SceneRow> {
    Ok(SceneRow {
        id: row.get(0)?,
        name: row.get(1)?,
        exemplars: row.get(2)?,
        tags: row.get(3)?,
        hue: row.get(4)?,
        x: row.get(5)?,
        y: row.get(6)?,
        r: row.get(7)?,
        videos: row.get(8)?,
        artists: row.get(9)?,
        hours: row.get(10)?,
        channel_no: row.get(11)?,
    })
}

fn to_scene(row: SceneRow, lit_artists: i64, seen_videos: i64) -> AtlasScene {
    AtlasScene {
        id: row.id,
        name: row.name,
        exemplars: serde_json::from_str(&row.exemplars).unwrap_or_default(),
        tags: serde_json::from_str(&row.tags).unwrap_or_default(),
        hue: round1(row.hue),
        x: round1(row.x),
        y: round1(row.y),
        r: round1(row.r),
        videos: row.videos,
        artists: row.artists,
        hours: round1(row.hours),
        channel_no: row.channel_no,
        lit_artists,
        seen_videos,
    }
}

/// Artists the crawler checked or expanded in the last hour.
fn recently_touched(conn: &Connection) -> anyhow::Result<HashSet<i64>> {
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id FROM video_artist_state
          WHERE last_checked_at >= datetime('now', '-1 hour')
             OR last_expanded_at >= datetime('now', '-1 hour')",
    )?;
    let rows = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<HashSet<i64>, _>>()?;
    Ok(rows)
}

pub(super) fn atlas_payload(conn: &Connection) -> anyhow::Result<AtlasResponse> {
    let scene_rows: Vec<SceneRow> = conn
        .prepare(&format!(
            "SELECT {SCENE_COLUMNS} FROM video_scenes ORDER BY scene_id"
        ))?
        .query_map([], scene_row)?
        .collect::<Result<_, _>>()?;
    let built_at: Option<String> =
        conn.query_row("SELECT MAX(built_at) FROM video_scenes", [], |row| {
            row.get(0)
        })?;
    let light = light_map(conn)?;
    let seen = seen_per_artist(conn)?;
    let recent = recently_touched(conn)?;

    let mut artists = AtlasArtists::default();
    let mut lit_per_scene: HashMap<i64, i64> = HashMap::new();
    let mut seen_per_scene: HashMap<i64, i64> = HashMap::new();
    let mut recent_artist_ids = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id, x, y, scene_id, videos, charted FROM video_artist_facets
          WHERE x IS NOT NULL AND y IS NOT NULL ORDER BY artist_tidal_id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let scene: Option<i64> = row.get(3)?;
        let charted = row.get::<_, i64>(5)? != 0;
        let level = light.light(id, charted);
        if let Some(scene) = scene {
            if level == light::LIT {
                *lit_per_scene.entry(scene).or_default() += 1;
            }
            *seen_per_scene.entry(scene).or_default() += seen.get(&id).copied().unwrap_or(0);
        }
        if recent.contains(&id) {
            recent_artist_ids.push(id);
        }
        artists.ids.push(id);
        artists.x.push(round1(row.get(1)?) as f32);
        artists.y.push(round1(row.get(2)?) as f32);
        artists.scene.push(scene.unwrap_or(-1));
        artists.videos.push(row.get(4)?);
        artists.light.push(level);
    }
    let scenes: Vec<AtlasScene> = scene_rows
        .into_iter()
        .map(|row| {
            let (lit, seen) = (
                lit_per_scene.get(&row.id).copied().unwrap_or(0),
                seen_per_scene.get(&row.id).copied().unwrap_or(0),
            );
            to_scene(row, lit, seen)
        })
        .collect();
    Ok(AtlasResponse {
        built_at,
        building: scenes.is_empty(),
        size: MAP_SIZE as u32,
        scenes,
        artists,
        recent_artist_ids,
    })
}

/// Up to `limit` programmable videos, best first, one per song. Uses the
/// catalog stage's cuts and quality once they exist; before that, the most
/// popular videos of 90 s or more.
pub(super) fn best_videos(
    conn: &Connection,
    artist_id: i64,
    limit: usize,
) -> anyhow::Result<Vec<VideoSetItem>> {
    if artist_id <= 0 || artist_id == VARIOUS_ARTISTS_ID || limit == 0 {
        return Ok(Vec::new());
    }
    let ready = scenes::facets_ready(conn)?;
    let sql = if ready {
        "SELECT c.item_json, f.song_key FROM video_facets f
           JOIN video_catalog c ON c.tidal_video_id = f.tidal_video_id
          WHERE f.artist_tidal_id = ?1 AND f.programmable = 1
          ORDER BY f.quality DESC, COALESCE(f.popularity, -1) DESC, f.tidal_video_id DESC"
    } else {
        "SELECT item_json, NULL FROM video_catalog WHERE artist_tidal_id = ?1
          ORDER BY COALESCE(json_extract(item_json, '$.popularity'), -1) DESC, tidal_video_id DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([artist_id])?;
    let mut songs = HashSet::new();
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let json: String = row.get(0)?;
        let song_key: Option<String> = row.get(1)?;
        let Ok(video) = serde_json::from_str::<VideoCandidate>(&json) else {
            continue;
        };
        if !ready && video.duration_s.is_some_and(|d| d < MIN_PROGRAMMABLE_S) {
            continue;
        }
        let key = song_key.unwrap_or_else(|| {
            video_song_key(video.artist_id, video.artist_name.as_deref(), &video.title)
        });
        if songs.insert(key) {
            out.push(to_item(&video));
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

/// (programmable videos, hours of them) for one artist.
pub(super) fn playable(conn: &Connection, artist_id: i64) -> anyhow::Result<(i64, f64)> {
    if artist_id == VARIOUS_ARTISTS_ID {
        return Ok((0, 0.0));
    }
    let (count, seconds): (i64, f64) = if scenes::facets_ready(conn)? {
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(COALESCE(duration_s, 0)), 0) FROM video_facets
              WHERE artist_tidal_id = ?1 AND programmable = 1",
            [artist_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
    } else {
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(COALESCE(d, 0)), 0)
               FROM (SELECT json_extract(item_json, '$.duration_s') AS d FROM video_catalog
                      WHERE artist_tidal_id = ?1)
              WHERE d IS NULL OR d >= ?2",
            params![artist_id, MIN_PROGRAMMABLE_S],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
    };
    Ok((count, round1(seconds / 3600.0)))
}

/// Facet row of one artist: name, scene, videos, charted, genres.
type FacetRow = (String, Option<i64>, i64, bool, String);

fn facet(conn: &Connection, artist_id: i64) -> anyhow::Result<Option<FacetRow>> {
    Ok(conn
        .query_row(
            "SELECT name, scene_id, videos, charted, genres_json FROM video_artist_facets
              WHERE artist_tidal_id = ?1",
            [artist_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get(4)?,
                ))
            },
        )
        .optional()?)
}

/// A name for any artist id: facets, then the catalog, then the ledger.
pub(super) fn artist_name(conn: &Connection, artist_id: i64) -> anyhow::Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT COALESCE(
                (SELECT name FROM video_artist_facets WHERE artist_tidal_id = ?1 AND name <> ''),
                (SELECT artist_name FROM video_catalog WHERE artist_tidal_id = ?1 LIMIT 1),
                (SELECT name FROM video_artist_state WHERE artist_tidal_id = ?1 AND name <> ''))",
            [artist_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

pub(super) fn artist_card(
    conn: &Connection,
    artist_id: i64,
) -> anyhow::Result<Option<AtlasArtistCard>> {
    let catalog_videos: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_catalog WHERE artist_tidal_id = ?1",
        [artist_id],
        |row| row.get(0),
    )?;
    let facet = facet(conn, artist_id)?;
    if facet.is_none() && catalog_videos == 0 {
        return Ok(None);
    }
    let (name, scene_id, videos, charted, genres) = match facet {
        Some(row) => row,
        None => (
            artist_name(conn, artist_id)?.unwrap_or_default(),
            None,
            catalog_videos,
            false,
            "[]".to_string(),
        ),
    };
    let scene_name: Option<String> = match scene_id {
        Some(scene) => conn
            .query_row(
                "SELECT name FROM video_scenes WHERE scene_id = ?1",
                [scene],
                |row| row.get(0),
            )
            .optional()?,
        None => None,
    };
    let graph = scenes::cached_graph(conn)?;
    let light = light::load(conn, &graph)?;
    let (playable, hours) = playable(conn, artist_id)?;
    // Neighbours on the map only: anything else could not be flown to.
    let mut neighbors = Vec::new();
    let mut stmt =
        conn.prepare("SELECT name, charted FROM video_artist_facets WHERE artist_tidal_id = ?1")?;
    let mut candidates: Vec<(i64, f64, &'static str)> = graph.neighbors(artist_id).collect();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    for (id, weight, source) in candidates {
        let Some((name, charted)) = stmt
            .query_row([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
            })
            .optional()?
        else {
            continue;
        };
        neighbors.push(AtlasNeighbor {
            id,
            name,
            source,
            weight: (weight * 1000.0).round() / 1000.0,
            light: light.light(id, charted),
        });
        if neighbors.len() >= NEIGHBORS {
            break;
        }
    }
    let (collabs, collabs_total) = collabs(conn, artist_id, &light)?;
    Ok(Some(AtlasArtistCard {
        id: artist_id,
        name,
        scene_id,
        scene_name,
        videos,
        playable,
        hours,
        light: light.light(artist_id, charted),
        charted,
        genres: serde_json::from_str(&genres).unwrap_or_default(),
        best: best_videos(conn, artist_id, BEST_CUTS)?,
        neighbors,
        collabs,
        collabs_total,
    }))
}

/// Everyone the artist shares a video with (best video first, at most
/// `COLLABS`) and how many there are in all.
fn collabs(
    conn: &Connection,
    artist_id: i64,
    light: &LightMap,
) -> anyhow::Result<(Vec<AtlasCollab>, usize)> {
    let all = journey::cached_collab(conn)?.collaborators(artist_id);
    if all.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let facts = journey::cached_facts(conn)?;
    let top = &all[..all.len().min(COLLABS)];
    let ids: Vec<i64> = top.iter().map(|(_, link)| link.video_id).collect();
    let videos = day::catalog_videos(conn, &ids)?;
    let mut out = Vec::with_capacity(top.len());
    for (id, link) in top {
        let name = artist_name(conn, *id)?.unwrap_or_default();
        out.push(AtlasCollab {
            id: *id,
            light: light.light(*id, facts.charted(*id)),
            video: videos
                .get(&link.video_id)
                .map(|entry| day::item(entry, &format!("With {name}"))),
            name,
        });
    }
    Ok((out, all.len()))
}

pub(super) fn scene_card(
    conn: &Connection,
    scene_id: i64,
) -> anyhow::Result<Option<AtlasSceneCard>> {
    let Some(row) = conn
        .query_row(
            &format!("SELECT {SCENE_COLUMNS} FROM video_scenes WHERE scene_id = ?1"),
            [scene_id],
            scene_row,
        )
        .optional()?
    else {
        return Ok(None);
    };
    let light = light_map(conn)?;
    let seen = seen_per_artist(conn)?;
    let members: Vec<(i64, String, i64)> = conn
        .prepare(
            "SELECT artist_tidal_id, name, videos FROM video_artist_facets
              WHERE scene_id = ?1 ORDER BY videos DESC, artist_tidal_id",
        )?
        .query_map([scene_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    let lit_artists = members
        .iter()
        .filter(|(id, _, _)| light.is_lit(*id))
        .count() as i64;
    let seen_videos = members
        .iter()
        .map(|(id, _, _)| seen.get(id).copied().unwrap_or(0))
        .sum();
    let top_artists: Vec<SceneArtist> = members
        .iter()
        .take(SCENE_TOP_ARTISTS)
        .map(|(id, name, videos)| SceneArtist {
            id: *id,
            name: name.clone(),
            videos: *videos,
            light: light.light(*id, true),
        })
        .collect();
    // The hubs in the scene's exemplar order lead; one best cut per artist,
    // so the strip is spaced by artist.
    let exemplars: Vec<String> = serde_json::from_str(&row.exemplars).unwrap_or_default();
    let mut order: Vec<i64> = Vec::new();
    for name in &exemplars {
        if let Some((id, _, _)) = members.iter().find(|(_, n, _)| n == name) {
            order.push(*id);
        }
    }
    for (id, _, _) in &members {
        if !order.contains(id) {
            order.push(*id);
        }
    }
    let mut sample = Vec::new();
    for id in order.iter().take(SCENE_SAMPLE * 2) {
        if let Some(video) = best_videos(conn, *id, 1)?.into_iter().next() {
            sample.push(video);
            if sample.len() >= SCENE_SAMPLE {
                break;
            }
        }
    }
    Ok(Some(AtlasSceneCard {
        scene: to_scene(row, lit_artists, seen_videos),
        top_artists,
        sample,
    }))
}

/// An edge provider as the Archive's stage words it (frontend viaLabel).
fn via_label(source: Option<&str>) -> &'static str {
    match source {
        Some("tidal") => "TIDAL related",
        Some("lastfm") => "Last.fm similar",
        Some("featured") => "Featured together",
        Some("colist") => "Listed together",
        _ => "Nearby",
    }
}

pub(super) fn journey(
    conn: &Connection,
    request: JourneyRequest,
) -> anyhow::Result<AtlasJourneyResponse> {
    match request {
        JourneyRequest::New { from, scene } => new_journey(conn, from, scene),
    }
}

/// From `from` to a scene the listener has barely seen (or `scene`),
/// through artists new to them; each stop plays its best cut not yet
/// watched.
fn new_journey(
    conn: &Connection,
    from: i64,
    scene: Option<i64>,
) -> anyhow::Result<AtlasJourneyResponse> {
    const KIND: &str = "new";
    let Some(from_name) = artist_name(conn, from)? else {
        return Ok(AtlasJourneyResponse::lost(
            KIND,
            String::new(),
            "unknown artist",
        ));
    };
    let graph = scenes::cached_graph(conn)?;
    let facts = journey::cached_facts(conn)?;
    let light = light::load(conn, &graph)?;
    let scene_rows = journey::scenes_seen(conn)?;
    let scene_name = |id: i64| {
        scene_rows
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
    };
    let trip = match journey::plan_new(&graph, &facts, &light, &scene_rows, from, scene) {
        Ok(trip) => trip,
        Err(reason) => {
            let title = scene
                .and_then(scene_name)
                .map(|name| format!("{from_name} to {name}"))
                .unwrap_or_default();
            return Ok(AtlasJourneyResponse::lost(KIND, title, reason));
        }
    };
    let destination = scene_name(trip.scene).unwrap_or_default();
    let watched = journey::watched_videos(conn)?;
    let never = HashSet::new();
    let picks: Vec<Option<i64>> = trip
        .path
        .iter()
        .map(|(id, _)| {
            Ok(journey::pick_video(
                &journey::programmable_videos(conn, *id)?,
                &watched,
                &never,
            ))
        })
        .collect::<anyhow::Result<_>>()?;
    let ids: Vec<i64> = picks.iter().flatten().copied().collect();
    let videos = day::catalog_videos(conn, &ids)?;
    let last = trip.path.len() - 1;
    let mut stops = Vec::with_capacity(trip.path.len());
    for (at, ((artist_id, via), pick)) in trip.path.iter().zip(&picks).enumerate() {
        let name = artist_name(conn, *artist_id)?.unwrap_or_default();
        let level = light.light(*artist_id, facts.charted(*artist_id));
        let why = if at == 0 {
            format!("Start: {name}")
        } else if at == last {
            format!("Arriving in {destination}")
        } else if level == light::LIT {
            format!("You know {name}")
        } else {
            format!("New to you, via {}", via_label(*via))
        };
        stops.push(JourneyStop {
            artist_id: *artist_id,
            name,
            light: level,
            video: pick
                .and_then(|id| videos.get(&id))
                .map(|entry| day::item(entry, &why)),
            why,
        });
    }
    Ok(AtlasJourneyResponse {
        found: true,
        kind: KIND,
        title: format!("{from_name} to {destination}"),
        destination: Some(JourneyDestination {
            scene_id: trip.scene,
            name: destination,
        }),
        stops,
        reason: None,
    })
}

/// Whether a probe can and should go to the crawler.
pub(super) fn probe_decision(
    setting: DiscoverySetting,
    building: bool,
    state: Option<&artist_state::ArtistState>,
) -> AtlasProbeResponse {
    if setting == DiscoverySetting::Off {
        return AtlasProbeResponse {
            queued: false,
            reason: Some("discovery off"),
        };
    }
    if building {
        return AtlasProbeResponse {
            queued: true,
            reason: None,
        };
    }
    if let Some(state) = state
        && !state.never_checked()
        && !state.check_due
        && !state.expand_due
    {
        return AtlasProbeResponse {
            queued: false,
            reason: Some("already checked"),
        };
    }
    AtlasProbeResponse {
        queued: true,
        reason: None,
    }
}

// --------------------------------------------------------------- handlers

/// GET /api/videos/atlas
pub(super) async fn get_video_atlas(
    State(state): State<SharedState>,
) -> Result<Json<AtlasResponse>, ApiError> {
    read(&state, atlas_payload).await.map(Json)
}

/// GET /api/videos/atlas/names
pub(super) async fn get_video_atlas_names(
    State(state): State<SharedState>,
) -> Result<Json<AtlasNamesResponse>, ApiError> {
    read(&state, |conn| {
        let mut ids = Vec::new();
        let mut names = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, name FROM video_artist_facets
              WHERE x IS NOT NULL ORDER BY artist_tidal_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
            names.push(row.get(1)?);
        }
        Ok(AtlasNamesResponse { ids, names })
    })
    .await
    .map(Json)
}

/// GET /api/videos/atlas/artists/{id}
pub(super) async fn get_video_atlas_artist(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<AtlasArtistCard>, ApiError> {
    if id <= 0 {
        return Err(error(StatusCode::BAD_REQUEST, "invalid artist id"));
    }
    read(&state, move |conn| artist_card(conn, id))
        .await?
        .map(Json)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "artist not on the atlas"))
}

/// GET /api/videos/atlas/scenes/{id}
pub(super) async fn get_video_atlas_scene(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<AtlasSceneCard>, ApiError> {
    read(&state, move |conn| scene_card(conn, id))
        .await?
        .map(Json)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "scene not found"))
}

#[derive(Debug, Deserialize)]
pub(super) struct ProbeRequest {
    artist_id: i64,
}

/// POST /api/videos/atlas/probe
///
/// Registers the artist as a station root (its neighbourhood becomes
/// relevant to the background planner for two hours) and asks the crawler
/// for urgent work on it, exactly like a station running thin.
pub(super) async fn post_video_atlas_probe(
    State(state): State<SharedState>,
    Json(body): Json<ProbeRequest>,
) -> Result<Json<AtlasProbeResponse>, ApiError> {
    let id = body.artist_id;
    if id <= 0 || id == VARIOUS_ARTISTS_ID {
        return Err(error(StatusCode::BAD_REQUEST, "invalid artist id"));
    }
    let (setting, ledger) = read(&state, move |conn| {
        Ok((discovery_setting::load(conn)?, artist_state::get(conn, id)?))
    })
    .await?;
    let decision = probe_decision(
        setting,
        crawler::is_building(Urgent::Station(id)),
        ledger.as_ref(),
    );
    if setting != DiscoverySetting::Off {
        roots::touch_station(id);
    }
    if decision.queued {
        crawler::request(Urgent::Station(id));
    }
    Ok(Json(decision))
}

/// GET /api/videos/atlas/journey?kind=collab&from&to or ?kind=new&from[&scene]
pub(super) async fn get_video_atlas_journey(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<AtlasJourneyResponse>, ApiError> {
    let request = JourneyRequest::parse(&params)
        .map_err(|message| error(StatusCode::BAD_REQUEST, message))?;
    read(&state, move |conn| journey(conn, request))
        .await
        .map(Json)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{Router, body::Body, http::Request};
    use tower::ServiceExt;

    use super::super::api_routes;
    use super::super::tests::{fresh_migrated_db, fresh_test_state};
    use super::*;
    use crate::services::video_intel::{layout, spread};

    fn app(db: Database) -> Router {
        api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(db))))
    }

    async fn call(
        db: &Database,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(value) => {
                request = request.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let response = app(db.clone())
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Two scenes of four artists (80 videos each) joined by one weak edge,
    /// two uncharted artists, one watched artist and one library artist.
    fn seeded_db() -> Database {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            {
                let mut video = tx.prepare(
                    "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
                     VALUES (?1, ?2, ?3, ?4)",
                )?;
                let mut add = |id: i64, artist: i64, name: &str, title: &str, duration: i64, popularity: i64| {
                    let json = json!({
                        "tidal_id": id, "title": title, "duration_s": duration,
                        "artist_id": artist, "artist_name": name, "popularity": popularity,
                    });
                    video.execute(params![id, artist, name, json.to_string()])
                };
                for artist in [1001i64, 1002, 1003, 1004, 2001, 2002, 2003, 2004] {
                    for v in 0..80 {
                        add(artist * 1000 + v, artist, &format!("Artist {artist}"), &format!("Song {v}"), 200, v)?;
                    }
                }
                // Two cuts of one song and a teaser: the card shows the song once.
                add(1_001_900, 1001, "Artist 1001", "Hit", 210, 99)?;
                add(1_001_901, 1001, "Artist 1001", "Hit (Live)", 260, 98)?;
                add(1_001_902, 1001, "Artist 1001", "Teaser", 30, 100)?;
                for artist in [3001i64, 3002] {
                    for v in 0..3 {
                        add(artist * 1000 + v, artist, &format!("Loner {artist}"), &format!("Song {v}"), 200, v)?;
                    }
                }
                let mut edge = tx.prepare(
                    "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                     VALUES (?1, ?2, 'x', ?3, ?4)",
                )?;
                for group in [[1001i64, 1002, 1003, 1004], [2001, 2002, 2003, 2004]] {
                    for (i, a) in group.iter().enumerate() {
                        for b in &group[i + 1..] {
                            edge.execute(params![a, b, "lastfm", 0.9])?;
                        }
                    }
                }
                edge.execute(params![1004, 2001, "featured", 0.3])?;
            }
            tx.execute_batch(
                "INSERT INTO video_history (tidal_video_id, artist_tidal_id, artist_name) VALUES
                     (1001000, 1001, 'Artist 1001'), (1001001, 1001, 'Artist 1001'), (1001000, 1001, 'Artist 1001');
                 INSERT INTO artists (id, tidal_id, name) VALUES (7, 3001, 'Loner 3001');
                 INSERT INTO tracks (id, artist_id, title) VALUES (70, 7, 'Library track');
                 INSERT INTO video_artist_state (artist_tidal_id, name, last_checked_at, next_check_at, next_expand_at)
                     VALUES (2002, 'Artist 2002', datetime('now'), datetime('now', '+30 days'), datetime('now', '+30 days'));",
            )?;
            tx.commit()?;
            scenes::build(conn)?;
            spread::build(conn)?;
            layout::build(conn)?;
            Ok(())
        })
        .unwrap();
        db
    }

    fn column<'a>(atlas: &'a Value, name: &str) -> &'a Vec<Value> {
        atlas["artists"][name].as_array().unwrap()
    }

    #[tokio::test]
    async fn the_atlas_is_columnar_with_light_and_scene_counts() {
        let db = seeded_db();
        let (status, atlas) = call(&db, "GET", "/api/videos/atlas", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(atlas["building"], false);
        assert_eq!(atlas["size"], 1000);
        assert_eq!(atlas["scenes"].as_array().unwrap().len(), 2);
        let ids = column(&atlas, "ids");
        assert_eq!(ids.len(), 10);
        for name in ["x", "y", "scene", "videos", "light"] {
            assert_eq!(column(&atlas, name).len(), 10, "{name}");
        }
        let at = |id: i64| ids.iter().position(|v| v.as_i64() == Some(id)).unwrap();
        let light = |id: i64| column(&atlas, "light")[at(id)].as_i64().unwrap();
        let scene = |id: i64| column(&atlas, "scene")[at(id)].as_i64().unwrap();
        assert_eq!(light(1001), 3, "watched");
        assert_eq!(light(1002), 2, "one hop from watched");
        assert_eq!(light(2003), 1, "charted, untouched");
        assert_eq!(light(3001), 2, "library artist on the ring");
        assert_eq!(light(3002), 0, "uncharted");
        assert_eq!(scene(3002), -1);
        assert_ne!(scene(1001), scene(2001));
        let lit_scene = atlas["scenes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"].as_i64() == Some(scene(1001)))
            .unwrap();
        assert_eq!(lit_scene["lit_artists"], 1);
        assert_eq!(lit_scene["seen_videos"], 2);
        assert_eq!(lit_scene["artists"], 4);
        assert!(lit_scene["r"].as_f64().unwrap() > 0.0);
        assert_eq!(atlas["recent_artist_ids"], json!([2002]));

        let (status, names) = call(&db, "GET", "/api/videos/atlas/names", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(names["ids"].as_array().unwrap().len(), 10);
        assert_eq!(names["names"][0], "Artist 1001");
    }

    #[tokio::test]
    async fn an_empty_atlas_reports_building() {
        let db = fresh_migrated_db();
        let (status, atlas) = call(&db, "GET", "/api/videos/atlas", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(atlas["building"], true);
        assert_eq!(atlas["built_at"], Value::Null);
        assert!(column(&atlas, "ids").is_empty());
    }

    #[tokio::test]
    async fn artist_cards_show_best_cuts_once_per_song_and_lit_neighbours() {
        let db = seeded_db();
        let (status, card) = call(&db, "GET", "/api/videos/atlas/artists/1001", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(card["name"], "Artist 1001");
        assert_eq!(card["light"], 3);
        assert_eq!(card["charted"], true);
        assert!(card["scene_name"].as_str().is_some());
        let best = card["best"].as_array().unwrap();
        assert_eq!(best.len(), 8);
        assert_eq!(
            best[0]["title"], "Hit",
            "most popular programmable cut first"
        );
        assert!(
            best.iter()
                .all(|v| v["title"] != "Hit (Live)" && v["title"] != "Teaser")
        );
        assert_eq!(best[0]["duration_ms"], 210_000);
        assert_eq!(card["playable"], 82);
        let neighbors = card["neighbors"].as_array().unwrap();
        assert_eq!(neighbors.len(), 3);
        assert!(
            neighbors
                .iter()
                .all(|n| n["light"] == 2 && n["source"] == "lastfm")
        );

        let (status, ring) = call(&db, "GET", "/api/videos/atlas/artists/3002", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (ring["charted"].clone(), ring["light"].clone()),
            (json!(false), json!(0))
        );
        assert_eq!(ring["scene_id"], Value::Null);
        let (status, _) = call(&db, "GET", "/api/videos/atlas/artists/99", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn scene_cards_list_top_artists_and_a_sample_spaced_by_artist() {
        let db = seeded_db();
        let scene_id: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT scene_id FROM video_artist_facets WHERE artist_tidal_id = 2001",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        let (status, card) = call(
            &db,
            "GET",
            &format!("/api/videos/atlas/scenes/{scene_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(card["scene"]["id"], scene_id);
        assert_eq!(card["top_artists"].as_array().unwrap().len(), 4);
        let sample = card["sample"].as_array().unwrap();
        assert_eq!(sample.len(), 4);
        let artists: HashSet<i64> = sample
            .iter()
            .map(|v| v["artist_id"].as_i64().unwrap())
            .collect();
        assert_eq!(artists.len(), 4, "one video per artist");
        let (status, _) = call(&db, "GET", "/api/videos/atlas/scenes/999", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn probes_respect_the_setting_and_the_ledger() {
        let db = seeded_db();
        let (status, queued) = call(
            &db,
            "POST",
            "/api/videos/atlas/probe",
            Some(json!({ "artist_id": 3002 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(queued, json!({ "queued": true }));
        assert!(crawler::is_building(Urgent::Station(3002)));
        assert!(roots::station_active(3002));
        let (_, checked) = call(
            &db,
            "POST",
            "/api/videos/atlas/probe",
            Some(json!({ "artist_id": 2002 })),
        )
        .await;
        assert_eq!(
            checked,
            json!({ "queued": false, "reason": "already checked" })
        );
        db.with_conn(|conn| discovery_setting::save(conn, DiscoverySetting::Off))
            .unwrap();
        let (_, off) = call(
            &db,
            "POST",
            "/api/videos/atlas/probe",
            Some(json!({ "artist_id": 3001 })),
        )
        .await;
        assert_eq!(off, json!({ "queued": false, "reason": "discovery off" }));
        assert!(!crawler::is_building(Urgent::Station(3001)));
        let (status, _) = call(
            &db,
            "POST",
            "/api/videos/atlas/probe",
            Some(json!({ "artist_id": 0 })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// A catalog video, optionally featuring others.
    fn add_video(
        conn: &Connection,
        id: i64,
        artist: i64,
        featured: &[i64],
        popularity: i64,
    ) -> rusqlite::Result<usize> {
        let json = json!({
            "tidal_id": id, "title": format!("Video {id}"), "duration_s": 200,
            "artist_id": artist, "artist_name": format!("Artist {artist}"),
            "popularity": popularity, "featured_artist_ids": featured,
        });
        conn.execute(
            "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, artist, format!("Artist {artist}"), json.to_string()],
        )
    }

    /// An artist on the map, in `scene` (charted) or on the ring.
    fn add_artist(
        conn: &Connection,
        id: i64,
        scene: Option<i64>,
        videos: i64,
    ) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO video_artist_facets (artist_tidal_id, name, scene_id, x, y, videos,
                    playable, charted)
             VALUES (?1, ?2, ?3, 1, 1, ?4, ?4, ?5)",
            params![
                id,
                format!("Artist {id}"),
                scene,
                videos,
                scene.is_some() as i64
            ],
        )
    }

    /// Collab links only through the catalog's featured artists:
    /// 11 -> 12 -> 13 (unpopular), 11 -> 14 -> 13 (popular), 11 -> 21 -> 22
    /// -> 13 (a step longer), 11 -> 15 -> 13 (most popular, but 15 is off the
    /// map); 51 features 52 and 53 on one video and 53 again on a weaker one;
    /// 31 -> 32 -> ... -> 40 is a nine-step line. A Last.fm edge joins 11 and
    /// 13 directly, and 11 is watched.
    fn collab_db() -> Database {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            for (id, artist, featured, popularity) in [
                (1100i64, 11i64, vec![], 30i64),
                (1101, 11, vec![12], 1),
                (1102, 11, vec![14], 50),
                (1103, 11, vec![21], 40),
                (1104, 11, vec![15], 45),
                (1300, 13, vec![], 5),
                (1301, 13, vec![12], 1),
                (1302, 13, vec![14], 50),
                (1303, 13, vec![15], 99),
                (1200, 12, vec![], 5),
                (1400, 14, vec![], 5),
                (2101, 21, vec![22], 40),
                (2201, 22, vec![13], 40),
                (5101, 51, vec![52, 53], 80),
                (5102, 51, vec![53], 10),
                (5200, 52, vec![], 5),
                (5300, 53, vec![], 5),
            ] {
                add_video(&tx, id, artist, &featured, popularity)?;
            }
            for step in 1..=9i64 {
                add_video(&tx, 3100 + step, 30 + step, &[31 + step], 5)?;
            }
            for id in [11i64, 12, 13, 14, 21, 22, 51, 52, 53] {
                add_artist(&tx, id, Some(1), 2)?;
            }
            for id in 31..=40i64 {
                add_artist(&tx, id, Some(1), 1)?;
            }
            tx.execute_batch(
                "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                     VALUES (11, 13, 'x', 'lastfm', 0.9);
                 INSERT INTO video_history (tidal_video_id, artist_tidal_id, artist_name)
                     VALUES (1100, 11, 'Artist 11');",
            )?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
        db
    }

    fn stop_ids(journey: &Value) -> Vec<i64> {
        journey["stops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["artist_id"].as_i64().unwrap())
            .collect()
    }

    /// (main artist, featured artists) of a catalog video.
    fn credits(db: &Database, video: i64) -> (i64, Vec<i64>) {
        db.with_conn(|conn| {
            let json: String = conn.query_row(
                "SELECT item_json FROM video_catalog WHERE tidal_video_id = ?1",
                [video],
                |row| row.get(0),
            )?;
            let item: VideoCandidate = serde_json::from_str(&json)?;
            Ok((item.artist_id.unwrap(), item.featured_artist_ids))
        })
        .unwrap()
    }

    #[tokio::test]
    async fn artist_cards_list_collabs_best_video_first() {
        let db = collab_db();
        let (status, card) = call(&db, "GET", "/api/videos/atlas/artists/11", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(card["collabs_total"], 4);
        let collabs = card["collabs"].as_array().unwrap();
        let ids: Vec<i64> = collabs.iter().map(|c| c["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![14, 15, 21, 12], "most popular shared video first");
        let videos: Vec<i64> = collabs
            .iter()
            .map(|c| c["video"]["tidal_id"].as_i64().unwrap())
            .collect();
        assert_eq!(videos, vec![1102, 1104, 1103, 1101]);
        for collab in collabs {
            let (main, featured) = credits(&db, collab["video"]["tidal_id"].as_i64().unwrap());
            let other = collab["id"].as_i64().unwrap();
            assert!(main == 11 && featured.contains(&other), "both are on it");
        }
        assert_eq!(collabs[0]["name"], "Artist 14");
        assert_eq!(collabs[0]["video"]["why"], "With Artist 14");

        // A video by 51 features 52 and 53: each is a collab of 51 only.
        let (_, hub) = call(&db, "GET", "/api/videos/atlas/artists/52", None).await;
        assert_eq!(hub["collabs_total"], 1);
        assert_eq!(hub["collabs"][0]["id"], 51);
        let (_, twice) = call(&db, "GET", "/api/videos/atlas/artists/12", None).await;
        assert_eq!(twice["collabs_total"], 2);
    }

    /// Scenes for somewhere new, every artist on the map with programmable
    /// videos (quality falls with the index, so `a * 10` is a's best):
    /// 1 Home 101..108, 2 Seen 201..208 (201 and 202 watched), 3 Fresh
    /// 301..312 (311 and 312 rank last), 4 Tiny 401..403, 5 Far 501..508,
    /// ring artists 150 and 601..605. Cliques inside scenes; 101 - 201 -
    /// 312 strong, 101 - 150 - 312 a little weaker, 101 - 401, and a line
    /// 310 - 601 - ... - 605 - 501 that puts Far out of reach.
    fn new_db() -> Database {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut members: Vec<(i64, Option<i64>, i64)> = Vec::new();
            for (scene, ids) in [
                (1i64, (101..=108).collect::<Vec<i64>>()),
                (2, (201..=208).collect()),
                (3, (301..=312).collect()),
                (4, (401..=403).collect()),
                (5, (501..=508).collect()),
            ] {
                for id in ids {
                    let videos = if id == 311 || id == 312 { 1 } else { 4 };
                    members.push((id, Some(scene), videos));
                }
            }
            for id in [150i64, 601, 602, 603, 604, 605] {
                members.push((id, None, 2));
            }
            for (id, scene, videos) in &members {
                add_artist(&tx, *id, *scene, *videos)?;
                for v in 0..*videos {
                    let video = id * 10 + v;
                    add_video(&tx, video, *id, &[], 10)?;
                    tx.execute(
                        "INSERT INTO video_facets (tidal_video_id, artist_tidal_id, cut, programmable,
                                song_key, quality, popularity, duration_s)
                         VALUES (?1, ?2, 'official', 1, ?3, ?4, 10, 200)",
                        params![video, id, format!("{id}-{v}"), 1.0 - v as f64 * 0.1],
                    )?;
                }
            }
            for (scene, name) in [(1i64, "Home"), (2, "Seen"), (3, "Fresh"), (4, "Tiny"), (5, "Far")] {
                let (videos, artists) = members
                    .iter()
                    .filter(|m| m.1 == Some(scene))
                    .fold((0, 0), |(v, a), m| (v + m.2, a + 1));
                tx.execute(
                    "INSERT INTO video_scenes (scene_id, name, video_count, artist_count)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![scene, name, videos, artists],
                )?;
            }
            let mut edge = tx.prepare(
                "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                 VALUES (?1, ?2, 'x', ?3, ?4)",
            )?;
            for scene in 1..=5i64 {
                let ids: Vec<i64> = members
                    .iter()
                    .filter(|m| m.1 == Some(scene))
                    .map(|m| m.0)
                    .collect();
                for (i, a) in ids.iter().enumerate() {
                    for b in &ids[i + 1..] {
                        edge.execute(params![a, b, "lastfm", 0.9])?;
                    }
                }
            }
            for (a, b, weight) in [
                (101i64, 201i64, 0.9),
                (201, 312, 0.9),
                (101, 150, 0.8),
                (150, 312, 0.8),
                (101, 401, 0.9),
                (310, 601, 0.9),
                (601, 602, 0.9),
                (602, 603, 0.9),
                (603, 604, 0.9),
                (604, 605, 0.9),
                (605, 501, 0.9),
            ] {
                edge.execute(params![a, b, "tidal", weight])?;
            }
            drop(edge);
            tx.execute_batch(
                "INSERT INTO video_history (tidal_video_id, artist_tidal_id, artist_name) VALUES
                     (2010, 201, 'Artist 201'), (2020, 202, 'Artist 202');",
            )?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
        db
    }

    #[tokio::test]
    async fn somewhere_new_goes_to_the_least_seen_scene_in_reach_through_new_artists() {
        let db = new_db();
        let (status, trip) = call(
            &db,
            "GET",
            "/api/videos/atlas/journey?kind=new&from=101",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(trip["found"], true);
        assert_eq!(trip["kind"], "new");
        assert_eq!(trip["reason"], Value::Null);
        // Home is the start's own scene, Tiny too small, Far out of reach,
        // Seen already watched: Fresh.
        assert_eq!(
            trip["destination"],
            json!({ "scene_id": 3, "name": "Fresh" })
        );
        assert_eq!(trip["title"], "Artist 101 to Fresh");
        let ids = stop_ids(&trip);
        assert!((4..=8).contains(&ids.len()), "{ids:?}");
        assert_eq!(&ids[..3], &[101, 150, 312], "around the watched 201");
        let end = *ids.last().unwrap();
        assert!((301..=310).contains(&end), "ends at a top artist: {end}");
        let stops = trip["stops"].as_array().unwrap();
        let why: Vec<&str> = stops.iter().map(|s| s["why"].as_str().unwrap()).collect();
        assert_eq!(
            why,
            vec![
                "Start: Artist 101",
                "New to you, via TIDAL related",
                "New to you, via TIDAL related",
                "Arriving in Fresh"
            ]
        );
        let lights: Vec<i64> = stops.iter().map(|s| s["light"].as_i64().unwrap()).collect();
        assert_eq!(lights, vec![2, 0, 2, 1]);
        assert!(
            stops
                .iter()
                .all(|s| s["video"]["artist_id"] == s["artist_id"]
                    && s["video"]["tidal_id"].as_i64()
                        == Some(s["artist_id"].as_i64().unwrap() * 10)),
            "each artist's best cut"
        );
        assert_eq!(stops[0]["video"]["cut"], "official");

        // From a watched artist: Seen is home now, Home is the least seen in
        // reach, and the start plays its best cut not yet watched.
        let (_, back) = call(
            &db,
            "GET",
            "/api/videos/atlas/journey?kind=new&from=201",
            None,
        )
        .await;
        assert_eq!(back["destination"]["name"], "Home");
        let first = &back["stops"][0];
        assert_eq!(
            (first["light"].clone(), first["why"].clone()),
            (json!(3), json!("Start: Artist 201"))
        );
        assert_eq!(first["video"]["tidal_id"], 2011);
        let end = *stop_ids(&back).last().unwrap();
        assert!((101..=108).contains(&end));
    }

    #[tokio::test]
    async fn somewhere_new_honours_the_scene_and_validates_params() {
        let db = new_db();
        let (_, seen) = call(
            &db,
            "GET",
            "/api/videos/atlas/journey?kind=new&from=101&scene=2",
            None,
        )
        .await;
        assert_eq!(seen["found"], true);
        assert_eq!(
            seen["destination"],
            json!({ "scene_id": 2, "name": "Seen" })
        );
        let ids = stop_ids(&seen);
        assert!((4..=8).contains(&ids.len()), "{ids:?}");
        assert!((201..=208).contains(ids.last().unwrap()));
        assert_eq!(seen["stops"][ids.len() - 1]["why"], "Arriving in Seen");

        let (_, far) = call(
            &db,
            "GET",
            "/api/videos/atlas/journey?kind=new&from=101&scene=5",
            None,
        )
        .await;
        assert_eq!(far["found"], false);
        assert_eq!(far["title"], "Artist 101 to Far");
        assert_eq!(far["reason"], "no route to Far within 8 stops");
        let (_, unknown) = call(
            &db,
            "GET",
            "/api/videos/atlas/journey?kind=new&from=101&scene=99",
            None,
        )
        .await;
        assert_eq!(unknown["reason"], "unknown scene");
        let (_, nobody) = call(
            &db,
            "GET",
            "/api/videos/atlas/journey?kind=new&from=999",
            None,
        )
        .await;
        assert_eq!(nobody["reason"], "unknown artist");

        for uri in [
            "/api/videos/atlas/journey?from=101",
            "/api/videos/atlas/journey?kind=route&from=101",
            "/api/videos/atlas/journey?kind=new",
            "/api/videos/atlas/journey?kind=new&from=abc",
            "/api/videos/atlas/journey?kind=new&from=101&scene=x",
            "/api/videos/atlas/journey?kind=collab&from=101&to=102",
        ] {
            let (status, body) = call(&db, "GET", uri, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
            assert!(body["error"].is_string(), "{uri}");
        }
    }

    #[test]
    fn best_cuts_follow_catalog_facets_once_they_exist() {
        let db = seeded_db();
        db.with_conn(|conn| {
            // Quality ranks; the interview is not programmable; two cuts of
            // one song collapse to the better one.
            conn.execute_batch(
                "INSERT INTO video_facets (tidal_video_id, artist_tidal_id, cut, programmable, song_key, quality, duration_s) VALUES
                     (1001005, 1001, 'official', 1, 'k5', 0.9, 200),
                     (1001006, 1001, 'live', 1, 'k5', 0.8, 200),
                     (1001007, 1001, 'interview', 0, 'k7', 1.0, 900),
                     (1001008, 1001, 'official', 1, 'k8', 0.5, 200);",
            )?;
            let best = best_videos(conn, 1001, 8)?;
            let ids: Vec<i64> = best.iter().map(|v| v.tidal_id).collect();
            assert_eq!(ids, vec![1001005, 1001008]);
            assert_eq!(playable(conn, 1001)?, (3, 0.2));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn probe_decisions_cover_every_branch() {
        use artist_state::ArtistState;
        let checked = ArtistState {
            artist_tidal_id: 1,
            name: String::new(),
            popularity: None,
            mix_id: None,
            seen_main: true,
            seen_featured: false,
            fetched_count: 3,
            total_videos: Some(3),
            checked_age_days: Some(1.0),
            check_due: false,
            empty_streak: 0,
            expand_due: false,
            mix_age_days: None,
        };
        let full = DiscoverySetting::Full;
        assert!(probe_decision(full, false, None).queued);
        assert_eq!(
            probe_decision(full, false, Some(&checked)).reason,
            Some("already checked")
        );
        assert!(
            probe_decision(full, true, Some(&checked)).queued,
            "already in flight"
        );
        let due = ArtistState {
            expand_due: true,
            ..checked.clone()
        };
        assert!(probe_decision(DiscoverySetting::Limited, false, Some(&due)).queued);
        assert_eq!(
            probe_decision(DiscoverySetting::Off, false, None).reason,
            Some("discovery off")
        );
    }

    /// Journey latency and examples over a copy of a real database that
    /// already holds a build, opened read-only (a debug build is fine):
    ///   NOOR_INTEL_DB=<copy> cargo test -p noor-server --bin noor-server real_catalog_journeys -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_catalog_journeys() {
        use rusqlite::OpenFlags;
        let Ok(path) = std::env::var("NOOR_INTEL_DB") else {
            eprintln!("NOOR_INTEL_DB not set");
            return;
        };
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        // The caches a cold first journey builds, one at a time.
        let started = std::time::Instant::now();
        scenes::cached_graph(&conn).unwrap();
        eprintln!("artist graph: {} ms", started.elapsed().as_millis());
        let started = std::time::Instant::now();
        journey::cached_facts(&conn).unwrap();
        eprintln!("map facts: {} ms", started.elapsed().as_millis());
        let started = std::time::Instant::now();
        light_map(&conn).unwrap();
        eprintln!("light: {} ms", started.elapsed().as_millis());
        let id_of = |name: &str| -> i64 {
            conn.query_row(
                "SELECT artist_tidal_id FROM video_artist_facets WHERE name = ?1
                  ORDER BY videos DESC LIMIT 1",
                [name],
                |row| row.get(0),
            )
            .unwrap()
        };
        let run = |label: String, request: JourneyRequest, rounds: usize| {
            let mut times = Vec::new();
            let mut last = None;
            for _ in 0..rounds {
                let started = std::time::Instant::now();
                let response = journey(&conn, request).unwrap();
                let bytes = serde_json::to_vec(&response).unwrap().len();
                times.push(started.elapsed().as_millis());
                last = Some((response, bytes));
            }
            let (response, bytes) = last.unwrap();
            eprintln!(
                "{label}: {times:?} ms, {bytes} bytes, found {} \"{}\" {}",
                response.found,
                response.title,
                response.reason.as_deref().unwrap_or("")
            );
            for stop in &response.stops {
                eprintln!(
                    "    {} [{}] {} | {}",
                    stop.name,
                    stop.light,
                    stop.why,
                    stop.video
                        .as_ref()
                        .map_or("(no video)", |v| v.title.as_str())
                );
            }
        };
        // JAY-Z's TIDAL name is not plain ASCII; 7804 is his id.
        for (from, id) in [
            ("James Blake", id_of("James Blake")),
            ("JAY-Z", 7804),
            ("Elvis Presley", id_of("Elvis Presley")),
            ("Metallica", id_of("Metallica")),
        ] {
            let request = JourneyRequest::New {
                from: id,
                scene: None,
            };
            run(format!("new from {from}"), request, 3);
        }
    }

    /// Payload size and endpoint latency over a copy of a real database that
    /// already holds a build (run real_catalog_atlas_build first).
    ///   NOOR_INTEL_DB=<copy> cargo test -p noor-server real_catalog_atlas_endpoints -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn real_catalog_atlas_endpoints() {
        let Ok(path) = std::env::var("NOOR_INTEL_DB") else {
            eprintln!("NOOR_INTEL_DB not set");
            return;
        };
        let db = Database::open(&path).unwrap();
        db.with_conn(|conn| {
            let started = std::time::Instant::now();
            let graph = scenes::ArtistGraph::load(conn)?;
            eprintln!("graph load: {} ms", started.elapsed().as_millis());
            let started = std::time::Instant::now();
            light::load(conn, &graph)?;
            eprintln!("light: {} ms", started.elapsed().as_millis());
            Ok(())
        })
        .unwrap();
        let time = |label: &str, started: std::time::Instant, bytes: usize| {
            eprintln!(
                "{label}: {} ms, {} bytes",
                started.elapsed().as_millis(),
                bytes
            );
        };
        let get = |uri: String| {
            let db = db.clone();
            async move {
                let response = app(db)
                    .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (status, bytes)
            }
        };
        for round in 0..3 {
            let started = std::time::Instant::now();
            let (status, bytes) = get("/api/videos/atlas".into()).await;
            assert_eq!(status, StatusCode::OK);
            time(&format!("atlas #{round}"), started, bytes.len());
        }
        let started = std::time::Instant::now();
        let (_, bytes) = get("/api/videos/atlas/names".into()).await;
        time("names", started, bytes.len());
        let atlas: Value =
            serde_json::from_slice(&get("/api/videos/atlas".into()).await.1).unwrap();
        let lights = column(&atlas, "light");
        let mut by_level = [0usize; 4];
        for level in lights {
            by_level[level.as_u64().unwrap() as usize] += 1;
        }
        eprintln!("artists {} light {by_level:?}", lights.len());
        let scene = atlas["scenes"][0]["id"].as_i64().unwrap();
        let started = std::time::Instant::now();
        let (_, bytes) = get(format!("/api/videos/atlas/scenes/{scene}")).await;
        time("scene card", started, bytes.len());
        for artist in [8812i64, 4377, 3121] {
            let started = std::time::Instant::now();
            let (status, bytes) = get(format!("/api/videos/atlas/artists/{artist}")).await;
            time(
                &format!("artist card {artist} ({status})"),
                started,
                bytes.len(),
            );
        }
    }
}
