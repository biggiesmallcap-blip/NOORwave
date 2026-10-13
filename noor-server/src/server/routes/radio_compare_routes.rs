//! Blind radio comparison. For one seed it returns the first tracks each
//! station design would play: today's station, Last.fm alone, TIDAL's own
//! track radio, and the hybrid prototype (`services::radio::hybrid`). Nothing
//! is queued or played and no diagnostics row is written.
//! Every list passes the same queue gate and the same TIDAL matching, so the
//! lists differ only in where their picks come from. Ratings live in one
//! `server_config` row so a session survives reloads.

use super::*;
use crate::db::models::Track;
use crate::playback::candidate_gate::CandidateGate;
use crate::services::radio::{Diagnostics, RadioBlend, RadioCandidate};
use futures::StreamExt;

/// Tracks per list: the first-20 fit the radio benchmark scores.
pub(super) const COMPARE_LIST_LEN: usize = 20;
/// Picks a list may look at (and match on TIDAL) to fill its 20.
const COMPARE_MAX_CANDIDATES: usize = 40;
/// Pending picks matched on TIDAL at once, per list.
const COMPARE_RESOLVE_WINDOW: usize = 8;
const COMPARE_RESOLVE_CONCURRENCY: usize = 4;
/// Station size the current design is built at, as `/api/radio/start` does.
const COMPARE_STATION_LIMIT: usize = 60;
pub(super) const COMPARE_SESSION_KEY: &str = "radio_compare_session";
const COMPARE_SESSION_MAX_BYTES: usize = 2 * 1024 * 1024;
const COMPARE_SUGGESTED_SEEDS: usize = 20;
const COMPARE_HARD_SEEDS: usize = 10;

/// One playable row, in the frontend's `TidalPlayable` shape.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(super) struct CompareTrack {
    tidal_id: i64,
    title: String,
    artist_name: Option<String>,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
    artist_tidal_id: Option<i64>,
    album_tidal_id: Option<i64>,
    local_id: Option<i64>,
    /// Why a suggested seed is a hard case; only on hard seed suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    seed_reason: Option<String>,
}

impl CompareTrack {
    fn from_track(track: &Track) -> Self {
        Self {
            tidal_id: track.tidal_id.unwrap_or(0),
            title: track.title.clone(),
            artist_name: track.artist_name.clone(),
            album_title: track.album_title.clone(),
            artwork_url: track.artwork_url.clone(),
            duration_ms: track.duration_ms,
            artist_tidal_id: track.artist_tidal_id,
            album_tidal_id: track.album_tidal_id,
            local_id: Some(track.id),
            seed_reason: None,
        }
    }

    fn from_search(track: crate::services::tidal::client::TidalSearchTrack) -> Self {
        Self {
            tidal_id: track.id,
            title: track.title,
            artist_name: track.artist_name,
            album_title: track.album_title,
            artwork_url: track.artwork_url,
            duration_ms: Some(track.duration * 1000),
            artist_tidal_id: track.artist_id,
            album_tidal_id: track.album_id,
            local_id: None,
            seed_reason: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct CompareArm {
    /// Stable design id; the page hides it until reveal.
    arm: &'static str,
    tracks: Vec<CompareTrack>,
    /// Why a list is short or empty, in plain words.
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CompareRequest {
    seed_track_id: i64,
    #[serde(default)]
    blend: Option<RadioBlend>,
}

/// A pick on its way to a row: ready to show, or a Last.fm name still to be
/// matched on TIDAL.
#[derive(Debug, Clone)]
enum Slot {
    Ready(CompareTrack),
    Pending { artist: String, title: String },
}

pub(super) async fn radio_compare_seed(
    State(state): State<SharedState>,
    Json(payload): Json<CompareRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if payload.seed_track_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "seed_track_id must be a positive library id" })),
        ));
    }
    let blend = payload.blend.unwrap_or_default();
    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };
    let seed_id = payload.seed_track_id;
    let seed = db
        .with_conn(move |conn| crate::playback::queue::get_track_by_id(conn, seed_id))
        .ok()
        .flatten()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "seed track not found" })),
            )
        })?;
    let tidal = compare_tidal_client(&state).await;

    let (current, lastfm_only, tidal_radio) = tokio::join!(
        current_station(
            &state,
            &db,
            lastfm.as_ref(),
            &lastfm_similar_cache,
            &seed,
            blend
        ),
        lastfm_station(lastfm.as_ref(), &lastfm_similar_cache, &seed),
        tidal_station(&state, &db, &seed),
    );
    let hybrid = {
        let note = (lastfm_only.0.is_empty() && tidal_radio.0.is_empty()).then(|| {
            "Neither TIDAL nor Last.fm had picks, so only library tracks remain".to_string()
        });
        let picks = crate::services::radio::hybrid::hybrid_station(
            &db,
            seed.id,
            tidal_radio.0.clone(),
            lastfm_only.0.clone(),
            blend,
            COMPARE_MAX_CANDIDATES,
        )
        .await;
        (picks, note)
    };
    let lists = [
        ("current", current),
        ("lastfm", lastfm_only),
        ("tidal", tidal_radio),
        ("hybrid", hybrid),
    ];
    let arms = futures::future::join_all(lists.into_iter().map(|(arm, (picks, note))| {
        let db = db.clone();
        let tidal = tidal.as_ref();
        let seed = &seed;
        async move {
            let tracks = playable_list(&db, tidal, seed, picks).await;
            let note = note.or_else(|| {
                (tracks.len() < COMPARE_LIST_LEN && tidal.is_none())
                    .then(|| "TIDAL is not connected, so Last.fm picks cannot be matched".into())
            });
            CompareArm { arm, tracks, note }
        }
    }))
    .await;

    Ok(Json(json!({
        "seed": CompareTrack::from_track(&seed),
        "blend": blend,
        "arms": arms,
    })))
}

/// The TIDAL client every list matches with, or `None` without a session.
async fn compare_tidal_client(state: &SharedState) -> Option<TidalClient> {
    let tokens = {
        let s = state.read().await;
        s.tidal.tokens()
    };
    let tokens = match tokens {
        Some(tokens) => tokens,
        None => load_persisted_tidal_tokens(state).await.ok().flatten()?,
    };
    let session = state.read().await.tidal.clone();
    Some(TidalClient::for_session(session, &tokens.country_code))
}

/// Today's Song Radio, built as `/api/radio/start` builds it (TIDAL mix
/// fallback included), without writing a diagnostics row.
async fn current_station(
    state: &SharedState,
    db: &crate::db::Database,
    lastfm: Option<&crate::metadata::lastfm::LastFmClient>,
    cache: &crate::services::radio::LastFmSimilarCache,
    seed: &Track,
    blend: RadioBlend,
) -> (Vec<RadioCandidate>, Option<String>) {
    match crate::services::radio::orchestrate_song_with(
        db,
        lastfm,
        Some(cache),
        seed.id,
        blend,
        COMPARE_STATION_LIMIT,
        &[],
        Diagnostics::Skip,
    )
    .await
    {
        Ok(mut queue) => {
            add_tidal_mix_fallback(
                state,
                db,
                TidalMixSeed::Track(seed.id),
                &mut queue.tracks,
                COMPARE_STATION_LIMIT,
            )
            .await;
            (queue.tracks, None)
        }
        Err(error) => {
            tracing::warn!(seed_track_id = seed.id, %error, "radio compare: station failed");
            (Vec::new(), Some("The station could not be built".into()))
        }
    }
}

/// Last.fm's similar tracks for the seed, in Last.fm's order.
async fn lastfm_station(
    lastfm: Option<&crate::metadata::lastfm::LastFmClient>,
    cache: &crate::services::radio::LastFmSimilarCache,
    seed: &Track,
) -> (Vec<RadioCandidate>, Option<String>) {
    let Some(client) = lastfm else {
        return (Vec::new(), Some("Last.fm is not connected".into()));
    };
    let Some(artist) = seed.artist_name.as_deref().filter(|a| !a.is_empty()) else {
        return (Vec::new(), Some("The seed has no artist name".into()));
    };
    let picks = crate::services::radio::lastfm_similar_candidates(
        client,
        Some(cache),
        artist,
        &seed.title,
        COMPARE_MAX_CANDIDATES,
    )
    .await;
    let note = picks
        .is_empty()
        .then(|| "Last.fm has no similar tracks for this seed".to_string());
    (picks, note)
}

/// TIDAL's own track radio for the seed, in TIDAL's order.
async fn tidal_station(
    state: &SharedState,
    db: &crate::db::Database,
    seed: &Track,
) -> (Vec<RadioCandidate>, Option<String>) {
    match fetch_seed_tidal_mix(state, db, TidalMixSeed::Track(seed.id)).await {
        Ok((mix, seed_tidal_id)) => {
            let picks = db
                .with_conn(move |conn| {
                    Ok(crate::services::radio::tidal_mix_candidates(
                        conn,
                        mix,
                        seed_tidal_id,
                        &[],
                        COMPARE_MAX_CANDIDATES,
                    ))
                })
                .unwrap_or_default();
            (picks, None)
        }
        Err(reason) => (Vec::new(), Some(sentence(reason))),
    }
}

fn sentence(reason: &str) -> String {
    let mut chars = reason.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The first `COMPARE_LIST_LEN` playable rows of a list: the queue gate a real
/// station applies (recent plays, early skips, Not for me, the seed, versions
/// already listed), then TIDAL matching for Last.fm names. A name with no
/// same-artist match is dropped, as the queue drops it.
async fn playable_list(
    db: &crate::db::Database,
    tidal: Option<&TidalClient>,
    seed: &Track,
    picks: Vec<RadioCandidate>,
) -> Vec<CompareTrack> {
    let seed_for_gate = seed.clone();
    let mut gate = db
        .with_conn(move |conn| {
            let mut gate = CandidateGate::load(conn, &[], None);
            gate.note_queued(&seed_for_gate);
            Ok(gate)
        })
        .unwrap_or_default();
    let mut slots = Vec::new();
    for pick in picks {
        if slots.len() >= COMPARE_MAX_CANDIDATES {
            break;
        }
        let library_id = (pick.is_in_library && pick.track_id > 0).then_some(pick.track_id);
        if !gate.admit_candidate(
            library_id,
            pick.tidal_track_id,
            pick.isrc.as_deref(),
            &pick.artist_name,
            &pick.title,
        ) {
            continue;
        }
        let slot = if let Some(id) = library_id {
            match db
                .with_conn(move |conn| crate::playback::queue::get_track_by_id(conn, id))
                .ok()
                .flatten()
            {
                Some(track) => Slot::Ready(CompareTrack::from_track(&track)),
                None => continue,
            }
        } else if let Some(tidal_id) = pick.tidal_track_id {
            Slot::Ready(CompareTrack {
                tidal_id,
                title: pick.title,
                artist_name: Some(pick.artist_name),
                album_title: pick.album_title,
                artwork_url: pick.artwork_url,
                duration_ms: pick.duration_ms,
                artist_tidal_id: None,
                album_tidal_id: None,
                local_id: None,
                seed_reason: None,
            })
        } else {
            Slot::Pending {
                artist: pick.artist_name,
                title: pick.title,
            }
        };
        slots.push(slot);
    }

    let mut rows = Vec::new();
    let mut seen_tidal_ids = HashSet::new();
    for window in slots.chunks(COMPARE_RESOLVE_WINDOW) {
        if rows.len() >= COMPARE_LIST_LEN {
            break;
        }
        let resolved: Vec<Option<CompareTrack>> = futures::stream::iter(window.iter().cloned())
            .map(|slot| resolve_slot(tidal, slot))
            .buffered(COMPARE_RESOLVE_CONCURRENCY)
            .collect()
            .await;
        for row in resolved.into_iter().flatten() {
            if row.tidal_id > 0
                && (Some(row.tidal_id) == seed.tidal_id || !seen_tidal_ids.insert(row.tidal_id))
            {
                continue;
            }
            rows.push(row);
            if rows.len() >= COMPARE_LIST_LEN {
                break;
            }
        }
    }
    rows
}

async fn resolve_slot(tidal: Option<&TidalClient>, slot: Slot) -> Option<CompareTrack> {
    match slot {
        Slot::Ready(row) => Some(row),
        Slot::Pending { artist, title } => {
            let client = tidal?;
            let results = client
                .search(&format!("{artist} {title}"), TIDAL_RESOLVE_POOL)
                .await
                .ok()?;
            select_best_tidal_match(&artist, &title, results)
                .map(|(_, track)| CompareTrack::from_search(track))
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct CompareSeedsQuery {
    /// `hard` for seeds where station designs should differ most.
    #[serde(default)]
    set: Option<String>,
}

/// Seeds worth comparing. The default set is the most played TIDAL-backed
/// library tracks, one per top-level genre first and never two by one artist,
/// then the most played of the rest. `?set=hard` returns the hard cases.
pub(super) async fn radio_compare_seeds(
    State(state): State<SharedState>,
    Query(query): Query<CompareSeedsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let db = state.read().await.db.clone();
    let hard = query.set.as_deref() == Some("hard");
    let seeds = db
        .with_conn(move |conn| {
            if hard {
                hard_compare_seeds(conn, COMPARE_HARD_SEEDS)
            } else {
                suggested_compare_seeds(conn, COMPARE_SUGGESTED_SEEDS)
            }
        })
        .map_err(internal)?;
    Ok(Json(json!({ "seeds": seeds })))
}

fn track_ids(
    conn: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> rusqlite::Result<Vec<i64>> {
    conn.prepare(sql)?
        .query_map(params, |row| row.get(0))?
        .collect()
}

/// Hard cases for a station, where the designs should differ most, taken in
/// turn from each source and never two by one artist:
/// - stations you started that had nothing learned to go on (no library lane),
/// - played tracks your listening model does not know yet,
/// - the most played track of each rare genre held with confidence,
/// - tracks added recently and not played yet.
fn hard_compare_seeds(
    conn: &rusqlite::Connection,
    count: usize,
) -> anyhow::Result<Vec<CompareTrack>> {
    let tagged = |list: Vec<i64>, reason: &str| -> std::collections::VecDeque<(i64, String)> {
        list.into_iter()
            .map(|id| (id, reason.to_string()))
            .collect()
    };

    let cold = track_ids(
        conn,
        "SELECT d.seed_track_id FROM radio_diagnostics d
         JOIN tracks t ON t.id = d.seed_track_id
         WHERE d.actual_library_count = 0 AND t.tidal_id > 0
         GROUP BY d.seed_track_id
         ORDER BY MAX(d.id) DESC
         LIMIT 20",
        [],
    )?;
    let unknown = match queries::get_selected_discovery_embedding_model(conn)? {
        Some(model) => track_ids(
            conn,
            "SELECT t.id FROM tracks t
             WHERE t.play_count > 0 AND t.tidal_id > 0
               AND NOT EXISTS (SELECT 1 FROM track_neighbors n
                               WHERE n.model_id = ?1 AND n.track_id = t.id)
             ORDER BY t.last_played_at DESC, t.id DESC
             LIMIT 20",
            [model.id],
        )?,
        None => Vec::new(),
    };
    let rare: std::collections::VecDeque<(i64, String)> = conn
        .prepare(
            "SELECT g.name,
                    (SELECT t2.id FROM track_genres tg2 JOIN tracks t2 ON t2.id = tg2.track_id
                     WHERE tg2.genre_id = g.id AND tg2.confidence >= 0.5 AND t2.tidal_id > 0
                     ORDER BY t2.play_count DESC, t2.id ASC LIMIT 1) AS pick
             FROM track_genres tg
             JOIN genres g ON g.id = tg.genre_id
             JOIN tracks t ON t.id = tg.track_id
             WHERE t.tidal_id > 0 AND tg.confidence >= 0.5
             GROUP BY g.id
             HAVING COUNT(DISTINCT tg.track_id) BETWEEN 3 AND 40
             ORDER BY COUNT(DISTINCT tg.track_id) ASC, g.name ASC
             LIMIT 30",
        )?
        .query_map([], |row| {
            Ok((row.get::<_, Option<i64>>(1)?, row.get::<_, String>(0)?))
        })?
        .filter_map(|row| row.ok())
        .filter_map(|(id, genre)| id.map(|id| (id, format!("Rare genre: {genre}"))))
        .collect();
    let fresh = track_ids(
        conn,
        "SELECT id FROM tracks
         WHERE play_count = 0 AND is_library = 1 AND tidal_id > 0
         ORDER BY date_added DESC, id DESC
         LIMIT 20",
        [],
    )?;

    let mut sources = [
        tagged(cold, "A station you started had nothing learned to go on"),
        tagged(unknown, "Your listening model doesn't know it yet"),
        rare,
        tagged(fresh, "Added recently, not played yet"),
    ];
    let all_ids: Vec<i64> = sources
        .iter()
        .flat_map(|source| source.iter().map(|(id, _)| *id))
        .collect();
    let tracks: HashMap<i64, Track> = crate::playback::queue::get_tracks_by_ids(conn, &all_ids)?
        .into_iter()
        .map(|track| (track.id, track))
        .collect();

    let mut picked: Vec<CompareTrack> = Vec::new();
    let mut picked_ids = HashSet::new();
    let mut artists = HashSet::new();
    while picked.len() < count && sources.iter().any(|source| !source.is_empty()) {
        for source in sources.iter_mut() {
            if picked.len() >= count {
                break;
            }
            while let Some((id, reason)) = source.pop_front() {
                let Some(track) = tracks.get(&id) else {
                    continue;
                };
                if picked_ids.contains(&id) || artists.contains(&track.artist_id) {
                    continue;
                }
                picked_ids.insert(id);
                artists.insert(track.artist_id);
                let mut seed = CompareTrack::from_track(track);
                seed.seed_reason = Some(reason);
                picked.push(seed);
                break;
            }
        }
    }
    Ok(picked)
}

fn suggested_compare_seeds(
    conn: &rusqlite::Connection,
    count: usize,
) -> anyhow::Result<Vec<CompareTrack>> {
    let ids: Vec<i64> = conn
        .prepare(
            "SELECT id FROM tracks
             WHERE play_count > 0 AND tidal_id IS NOT NULL AND tidal_id > 0
             ORDER BY play_count DESC, id ASC
             LIMIT 400",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let genres = queries::get_genres_for_tracks_with_fallback(conn, &ids)?;
    let tracks: HashMap<i64, Track> = crate::playback::queue::get_tracks_by_ids(conn, &ids)?
        .into_iter()
        .map(|track| (track.id, track))
        .collect();
    let root_genre = |id: i64| -> Option<String> {
        genres.get(&id)?.iter().find_map(|row| {
            let root = row.path.split(" > ").next()?.trim();
            (!root.is_empty()).then(|| root.to_string())
        })
    };

    let mut picked: Vec<CompareTrack> = Vec::new();
    let mut picked_ids = HashSet::new();
    let mut roots = HashSet::new();
    let mut artists = HashSet::new();
    for id in &ids {
        if picked.len() >= count {
            break;
        }
        let (Some(track), Some(root)) = (tracks.get(id), root_genre(*id)) else {
            continue;
        };
        if roots.contains(&root) || artists.contains(&track.artist_id) {
            continue;
        }
        roots.insert(root);
        artists.insert(track.artist_id);
        picked_ids.insert(*id);
        picked.push(CompareTrack::from_track(track));
    }
    for id in &ids {
        if picked.len() >= count {
            break;
        }
        let Some(track) = tracks.get(id) else {
            continue;
        };
        if picked_ids.contains(id) || !artists.insert(track.artist_id) {
            continue;
        }
        picked_ids.insert(*id);
        picked.push(CompareTrack::from_track(track));
    }
    Ok(picked)
}

pub(super) async fn get_radio_compare_session(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let db = state.read().await.db.clone();
    let raw: Option<String> = db
        .with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM server_config WHERE key = ?1",
                    [COMPARE_SESSION_KEY],
                    |row| row.get(0),
                )
                .optional()?)
        })
        .map_err(internal)?;
    let session = raw
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or(Value::Null);
    Ok(Json(json!({ "session": session })))
}

pub(super) async fn put_radio_compare_session(
    State(state): State<SharedState>,
    Json(session): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let raw = session.to_string();
    if raw.len() > COMPARE_SESSION_MAX_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "comparison session is too large" })),
        ));
    }
    let db = state.read().await.db.clone();
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO server_config (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![COMPARE_SESSION_KEY, raw],
        )?;
        Ok(())
    })
    .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::tests::{fresh_migrated_db, fresh_test_state};
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn seed_library(db: &crate::db::Database) {
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Ram Dass'), (2, 'Ashana'), (3, 'Enya');
                 INSERT INTO tracks (id, title, artist_id, play_count) VALUES
                     (1, 'The Water Poem', 1, 30), (2, 'Jai Ma', 2, 20), (3, 'Orinoco Flow', 3, 10);",
            )?;
            let model = queries::create_embedding_model(
                conn,
                "discovery-fusion-v2:compare",
                queries::DISCOVERY_ENGINE_V2_FAMILY,
                8,
                "ready",
                None,
            )?;
            queries::activate_embedding_model(conn, model.id)?;
            conn.execute(
                "INSERT INTO track_neighbors (track_id, neighbor_track_id, model_id, rank, score)
                 VALUES (1, 2, ?1, 1, 0.9), (1, 3, ?1, 2, 0.8)",
                [model.id],
            )?;
            Ok(())
        })
        .unwrap();
    }

    async fn post_json(state: &SharedState, uri: &str, body: Value) -> (StatusCode, Value) {
        let response = api_routes(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
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

    #[tokio::test]
    async fn compare_lists_each_design_without_touching_playback() {
        let db = fresh_migrated_db();
        seed_library(&db);
        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));

        let (status, body) =
            post_json(&state, "/api/radio/compare", json!({ "seed_track_id": 1 })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let arms = body["arms"].as_array().unwrap();
        let arm = |id: &str| arms.iter().find(|a| a["arm"] == id).unwrap().clone();

        let current: Vec<i64> = arm("current")["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["local_id"].as_i64().unwrap())
            .collect();
        assert_eq!(current, vec![2, 3], "learned neighbors, seed left out");
        assert_eq!(arm("lastfm")["note"], "Last.fm is not connected");
        assert_eq!(arm("tidal")["note"], "This seed is not on TIDAL");
        assert_eq!(
            arm("hybrid")["note"],
            "Neither TIDAL nor Last.fm had picks, so only library tracks remain"
        );

        let (queued, diagnostics): (i64, i64) = db
            .with_conn(|conn| {
                Ok((
                    conn.query_row("SELECT COUNT(*) FROM queue", [], |r| r.get(0))?,
                    conn.query_row("SELECT COUNT(*) FROM radio_diagnostics", [], |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!((queued, diagnostics), (0, 0));
    }

    #[test]
    fn suggested_seeds_spread_across_genres_and_artists() {
        let db = fresh_migrated_db();
        db.seed_genres_from_taxonomy().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'A'), (2, 'B'), (3, 'C');
                 INSERT INTO tracks (id, title, artist_id, play_count, tidal_id) VALUES
                     (1, 'House 1', 1, 50, 101), (2, 'House 2', 2, 40, 102),
                     (3, 'Jazz 1', 1, 30, 103), (4, 'Jazz 2', 3, 20, 104),
                     (5, 'Local only', 3, 99, NULL);
                 INSERT INTO track_genres (track_id, genre_id, source)
                     SELECT t.id, g.id, 'lastfm' FROM tracks t, genres g
                     WHERE (t.title LIKE 'House%' AND g.name = 'House')
                        OR (t.title LIKE 'Jazz%' AND g.name = 'Jazz');",
            )?;
            let picked: Vec<i64> = suggested_compare_seeds(conn, 3)?
                .into_iter()
                .map(|seed| seed.local_id.unwrap())
                .collect();
            // Electronic (House 1), then Jazz by a new artist (Jazz 2), then
            // the most played remaining artist (House 2). Never the local file.
            assert_eq!(picked, vec![1, 4, 2]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn hard_seeds_cover_cold_stations_unknown_tracks_rare_genres_and_new_additions() {
        let db = fresh_migrated_db();
        db.seed_genres_from_taxonomy().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES
                     (1, 'Ram Dass'), (2, 'B'), (3, 'C'), (4, 'D'), (5, 'E'), (7, 'F'), (8, 'G');
                 INSERT INTO tracks (id, title, artist_id, play_count, tidal_id, is_library) VALUES
                     (1, 'The Water Poem', 1, 2, 101, 1),
                     (2, 'Unknown To Model', 2, 3, 102, 1),
                     (3, 'Dub One', 3, 5, 103, 1),
                     (4, 'New Addition', 4, 0, 104, 1),
                     (5, 'Popular', 5, 50, 105, 1),
                     (7, 'Dub Two', 7, 1, 107, 1),
                     (8, 'Dub Three', 8, 1, 108, 1);
                 INSERT INTO track_genres (track_id, genre_id, source, confidence)
                     SELECT t.id, g.id, 'musicbrainz', 0.8 FROM tracks t, genres g
                     WHERE t.title LIKE 'Dub%' AND g.name = 'Dub Techno';",
            )?;
            let model = queries::create_embedding_model(
                conn,
                "discovery-fusion-v2:hard",
                queries::DISCOVERY_ENGINE_V2_FAMILY,
                8,
                "ready",
                None,
            )?;
            queries::activate_embedding_model(conn, model.id)?;
            conn.execute(
                "INSERT INTO track_neighbors (track_id, neighbor_track_id, model_id, rank, score)
                 VALUES (5, 3, ?1, 1, 0.9)",
                [model.id],
            )?;
            crate::services::radio_config::log_radio_diagnostics(
                conn,
                &crate::services::radio_config::RadioDiagnosticsRow {
                    seed_track_id: Some(1),
                    profile_name: "mixed".into(),
                    ..Default::default()
                },
            )?;
            let seeds = hard_compare_seeds(conn, 10)?;
            let picked: Vec<i64> = seeds.iter().map(|s| s.local_id.unwrap()).collect();
            for id in [1, 2, 3, 4] {
                assert!(picked.contains(&id), "{id} missing from {picked:?}");
            }
            assert!(!picked.contains(&5), "a well-known favourite is not hard");
            let reason = |id: i64| {
                seeds
                    .iter()
                    .find(|s| s.local_id == Some(id))
                    .and_then(|s| s.seed_reason.clone())
                    .unwrap()
            };
            assert_eq!(
                reason(1),
                "A station you started had nothing learned to go on"
            );
            assert_eq!(reason(3), "Rare genre: Dub Techno");
            assert_eq!(reason(4), "Added recently, not played yet");
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn session_round_trips_and_rejects_oversized_bodies() {
        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            fresh_migrated_db(),
        )));
        let put = |body: String| {
            let state = state.clone();
            async move {
                api_routes(state)
                    .oneshot(
                        Request::builder()
                            .method("PUT")
                            .uri("/api/radio/compare/session")
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                    .status()
            }
        };
        assert_eq!(
            put(json!({ "seeds": [1] }).to_string()).await,
            StatusCode::OK
        );
        let response = api_routes(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/radio/compare/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["session"]["seeds"], json!([1]));

        let huge = json!({ "pad": "x".repeat(COMPARE_SESSION_MAX_BYTES) }).to_string();
        assert_eq!(put(huge).await, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
