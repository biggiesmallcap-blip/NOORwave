//! The Atlas: the whole map in one columnar payload, lazy names, artist
//! and scene cards, probes into the crawler, and routes between artists.
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
use crate::services::video_discovery::crawler::{self, Urgent};
use crate::services::video_discovery::setting::{self as discovery_setting, DiscoverySetting};
use crate::services::video_discovery::{artist_state, roots};
use crate::services::video_intel::layout::MAP_SIZE;
use crate::services::video_intel::light::{self, LightMap};
use crate::services::video_intel::scenes::{self, MIN_PROGRAMMABLE_S, VARIOUS_ARTISTS_ID};
use crate::services::video_radio::video_song_key;
use crate::services::video_sets::{VideoCandidate, VideoSetItem};
use crate::services::video_stations::to_item;

type ApiError = (StatusCode, Json<Value>);

const BEST_CUTS: usize = 8;
const NEIGHBORS: usize = 12;
const SCENE_TOP_ARTISTS: usize = 12;
const SCENE_SAMPLE: usize = 12;
const ROUTE_MAX_HOPS: usize = 6;

fn error(status: StatusCode, message: &str) -> ApiError {
    (status, Json(json!({ "error": message })))
}

fn internal(err: anyhow::Error) -> ApiError {
    tracing::warn!(target: "noor.video_atlas", error = %err, "atlas read failed");
    error(StatusCode::INTERNAL_SERVER_ERROR, "atlas read failed")
}

/// Run a read on its own connection off the async runtime; in-memory test
/// databases fall back to the shared one.
async fn read<T, F>(state: &SharedState, f: F) -> Result<T, ApiError>
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
struct AtlasRouteStop {
    artist_id: i64,
    name: String,
    via: Option<&'static str>,
    video: Option<VideoSetItem>,
}

#[derive(Debug, Serialize)]
pub(super) struct AtlasRouteResponse {
    found: bool,
    stops: Vec<AtlasRouteStop>,
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
fn best_videos(
    conn: &Connection,
    artist_id: i64,
    limit: usize,
) -> anyhow::Result<Vec<VideoSetItem>> {
    if artist_id <= 0 || artist_id == VARIOUS_ARTISTS_ID || limit == 0 {
        return Ok(Vec::new());
    }
    let ready = scenes::facets_ready(conn)?;
    let sql = if ready {
        "SELECT c.item_json FROM video_facets f
           JOIN video_catalog c ON c.tidal_video_id = f.tidal_video_id
          WHERE f.artist_tidal_id = ?1 AND f.programmable = 1
          ORDER BY f.quality DESC, COALESCE(f.popularity, -1) DESC, f.tidal_video_id DESC"
    } else {
        "SELECT item_json FROM video_catalog WHERE artist_tidal_id = ?1
          ORDER BY COALESCE(json_extract(item_json, '$.popularity'), -1) DESC, tidal_video_id DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([artist_id])?;
    let mut songs = HashSet::new();
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let json: String = row.get(0)?;
        let Ok(video) = serde_json::from_str::<VideoCandidate>(&json) else {
            continue;
        };
        if !ready && video.duration_s.is_some_and(|d| d < MIN_PROGRAMMABLE_S) {
            continue;
        }
        let key = video_song_key(video.artist_id, video.artist_name.as_deref(), &video.title);
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
fn playable(conn: &Connection, artist_id: i64) -> anyhow::Result<(i64, f64)> {
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
fn artist_name(conn: &Connection, artist_id: i64) -> anyhow::Result<Option<String>> {
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
    }))
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

pub(super) fn route(conn: &Connection, from: i64, to: i64) -> anyhow::Result<AtlasRouteResponse> {
    let graph = scenes::cached_graph(conn)?;
    let on_map: HashSet<i64> = conn
        .prepare("SELECT artist_tidal_id FROM video_artist_facets")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let Some(path) = graph.route(from, to, ROUTE_MAX_HOPS, |id| on_map.contains(&id)) else {
        return Ok(AtlasRouteResponse {
            found: false,
            stops: Vec::new(),
        });
    };
    let mut stops = Vec::with_capacity(path.len());
    for (artist_id, via) in path {
        stops.push(AtlasRouteStop {
            artist_id,
            name: artist_name(conn, artist_id)?.unwrap_or_default(),
            via,
            video: best_videos(conn, artist_id, 1)?.into_iter().next(),
        });
    }
    Ok(AtlasRouteResponse { found: true, stops })
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

#[derive(Debug, Deserialize)]
pub(super) struct RouteQuery {
    from: i64,
    to: i64,
}

/// GET /api/videos/atlas/route?from&to
pub(super) async fn get_video_atlas_route(
    State(state): State<SharedState>,
    Query(query): Query<RouteQuery>,
) -> Result<Json<AtlasRouteResponse>, ApiError> {
    if query.from <= 0 || query.to <= 0 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "from and to must be artist ids",
        ));
    }
    read(&state, move |conn| route(conn, query.from, query.to))
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

    #[tokio::test]
    async fn routes_walk_the_graph_with_one_video_per_stop() {
        let db = seeded_db();
        let (status, route) = call(
            &db,
            "GET",
            "/api/videos/atlas/route?from=1002&to=2003",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(route["found"], true);
        let stops = route["stops"].as_array().unwrap();
        let ids: Vec<i64> = stops
            .iter()
            .map(|s| s["artist_id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![1002, 1004, 2001, 2003]);
        assert_eq!(stops[0]["via"], Value::Null);
        assert_eq!(stops[2]["via"], "featured");
        assert!(
            stops
                .iter()
                .all(|s| s["video"]["artist_id"] == s["artist_id"])
        );
        let (_, lost) = call(
            &db,
            "GET",
            "/api/videos/atlas/route?from=1001&to=3002",
            None,
        )
        .await;
        assert_eq!(lost, json!({ "found": false, "stops": [] }));
        let (status, _) = call(&db, "GET", "/api/videos/atlas/route?from=1001", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
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
        for (from, to) in [(1588i64, 8027279i64), (8812, 4377), (159, 3618971)] {
            let started = std::time::Instant::now();
            let (_, bytes) = get(format!("/api/videos/atlas/route?from={from}&to={to}")).await;
            time(&format!("route {from} -> {to}"), started, bytes.len());
            let route: Value = serde_json::from_slice(&bytes).unwrap();
            let names: Vec<String> = route["stops"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    format!(
                        "{} [{}] {}",
                        s["name"].as_str().unwrap_or(""),
                        s["via"].as_str().unwrap_or("-"),
                        s["video"]["title"].as_str().unwrap_or("(no video)")
                    )
                })
                .collect();
            eprintln!("  {}", names.join(" -> "));
        }
    }
}
