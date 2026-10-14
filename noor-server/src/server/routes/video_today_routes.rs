//! GET /api/videos/today: the three short pieces under the dial
//! (services::video_air::today writes them). The picks and the catalog facts
//! come from the database on each request; a biography (stored, else TIDAL,
//! else Last.fm) and the origin from Wikidata are looked up once per artist
//! and kept for a month (a failed lookup is retried after a day).

use std::collections::HashSet;
use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::Json};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::video_atlas_routes::{artist_name, best_videos, playable};
use crate::SharedState;
use crate::metadata::lastfm::LastFmClient;
use crate::services::tidal::client::TidalClient;
use crate::services::video_air::today::{self, Facts, Kind, Origin};
use crate::services::video_air::{self, clock, day};
use crate::services::video_intel::{census, journey, scenes, you};
use crate::services::video_sets::{VideoSetItem, build_seed};

type Reply = (StatusCode, Json<Value>);

const ENRICH_KEY: &str = "video_air.today.enrich:";
const ENRICH_KEEP_S: i64 = 30 * 86_400;
const ENRICH_RETRY_S: i64 = 86_400;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(6);
/// Picks are drawn from this many of the best candidates, by the day.
const PICK_FROM: usize = 6;
/// "From your archive": watched at least this often, not in this many days.
const ARCHIVE_MIN_PLAYS: i64 = 2;
const ARCHIVE_QUIET_DAYS: i64 = 21;

/// What was looked up about an artist outside the catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Enrichment {
    bio: Option<String>,
    bio_source: Option<String>,
    origin: Option<Origin>,
    fetched_at: i64,
}

#[derive(Debug, Serialize)]
struct BioOut {
    text: String,
    source: String,
}

#[derive(Debug, Serialize)]
struct PieceOut {
    kind: Kind,
    kicker: &'static str,
    artist_id: i64,
    name: String,
    paragraphs: Vec<String>,
    bio: Option<BioOut>,
    photo_url: Option<String>,
    scene_id: Option<i64>,
    videos: Vec<VideoSetItem>,
}

#[derive(Debug, Serialize)]
struct TodayOut {
    day: String,
    pieces: Vec<PieceOut>,
}

/// One pick with everything the database knows about it.
struct Gathered {
    kind: Kind,
    artist_id: i64,
    facts: Facts,
    photo_url: Option<String>,
    scene_id: Option<i64>,
    videos: Vec<VideoSetItem>,
    stored_bio: Option<String>,
    enrichment: Option<Enrichment>,
}

fn seeded<T: Copy>(day: &str, salt: &str, items: &[T]) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    Some(items[(build_seed(day, salt) % items.len() as u64) as usize])
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Today's three artists: Spotlight's, someone new next to what is watched,
/// and someone from the archive (else from a scene rarely visited).
fn picks(conn: &Connection, day: &str) -> anyhow::Result<Vec<(Kind, i64, Option<i64>)>> {
    let mut out: Vec<(Kind, i64, Option<i64>)> = Vec::new();
    let mut taken: HashSet<i64> = HashSet::new();
    let explore = video_air::load_explore(conn)?;
    if let Some(artist) = day::ensure(conn, day, explore)?.spotlight_artist {
        out.push((Kind::Today, artist, None));
        taken.insert(artist);
    }

    let frontier = you::read(conn, now_unix())?.frontier;
    let mut fresh: Vec<(i64, i64)> = Vec::new();
    for entry in frontier {
        if taken.contains(&entry.artist_id) || playable(conn, entry.artist_id)?.0 == 0 {
            continue;
        }
        fresh.push((entry.artist_id, entry.via));
        if fresh.len() >= PICK_FROM {
            break;
        }
    }
    if let Some((artist, via)) = seeded(day, "today:new", &fresh) {
        out.push((Kind::New, artist, Some(via)));
        taken.insert(artist);
    }

    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id FROM video_history
          WHERE artist_tidal_id > 0
          GROUP BY artist_tidal_id
         HAVING COUNT(*) >= ?1 AND MAX(started_at) < datetime('now', ?2)
          ORDER BY COUNT(*) DESC, MAX(started_at) DESC
          LIMIT 20",
    )?;
    let quiet = format!("-{ARCHIVE_QUIET_DAYS} days");
    let archived: Vec<i64> = stmt
        .query_map(params![ARCHIVE_MIN_PLAYS, quiet], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?
        .into_iter()
        .filter(|id| !taken.contains(id))
        .take(PICK_FROM)
        .collect();
    if let Some(artist) = seeded(day, "today:archive", &archived) {
        out.push((Kind::Archive, artist, None));
        return Ok(out);
    }

    // No archive yet: the leading artist of a scene barely visited.
    let seen = census::seen_per_scene(conn)?;
    let mut scenes: Vec<(i64, i64)> = conn
        .prepare(
            "SELECT scene_id FROM video_scenes WHERE channel_no IS NOT NULL AND artist_count >= 8",
        )?
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<i64>, _>>()?
        .into_iter()
        .map(|scene| (seen.get(&scene).copied().unwrap_or(0), scene))
        .collect();
    scenes.sort();
    let quiet_scenes: Vec<i64> = scenes
        .into_iter()
        .take(PICK_FROM)
        .map(|(_, scene)| scene)
        .collect();
    if let Some(scene) = seeded(day, "today:elsewhere", &quiet_scenes) {
        let lead: Option<i64> = conn
            .query_row(
                "SELECT artist_tidal_id FROM video_artist_facets
                  WHERE scene_id = ?1 AND playable > 0
                  ORDER BY videos DESC, artist_tidal_id LIMIT 1",
                [scene],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(artist) = lead.filter(|artist| !taken.contains(artist)) {
            out.push((Kind::Elsewhere, artist, None));
        }
    }
    Ok(out)
}

fn names(
    conn: &Connection,
    ids: impl IntoIterator<Item = i64>,
    limit: usize,
) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    for id in ids {
        if let Some(name) = artist_name(conn, id)?.filter(|name| !name.trim().is_empty())
            && !out.contains(&name)
        {
            out.push(name);
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

fn load_enrichment(conn: &Connection, artist: i64) -> anyhow::Result<Option<Enrichment>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [format!("{ENRICH_KEY}{artist}")],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()))
}

fn store_enrichment(conn: &Connection, artist: i64, enrichment: &Enrichment) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![
            format!("{ENRICH_KEY}{artist}"),
            serde_json::to_string(enrichment)?
        ],
    )?;
    Ok(())
}

/// Everything the database knows for one pick.
fn gather(
    conn: &Connection,
    kind: Kind,
    artist: i64,
    via: Option<i64>,
) -> anyhow::Result<Option<Gathered>> {
    let Some(name) = artist_name(conn, artist)?.filter(|name| !name.trim().is_empty()) else {
        return Ok(None);
    };
    let (scene_id, videos): (Option<i64>, i64) = conn
        .query_row(
            "SELECT scene_id, videos FROM video_artist_facets WHERE artist_tidal_id = ?1",
            [artist],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or((None, 0));
    // Before the intelligence build has faceted an artist, count the catalog.
    let videos = if videos > 0 {
        videos
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM video_catalog WHERE artist_tidal_id = ?1",
            [artist],
            |row| row.get(0),
        )?
    };
    let (_, hours) = playable(conn, artist)?;

    let years: Vec<i32> = conn
        .prepare(
            "SELECT song_year FROM video_songs
              WHERE artist_tidal_id = ?1 AND era_conf >= 0.5 AND song_year IS NOT NULL",
        )?
        .query_map([artist], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let mut cuts = (0i64, 0i64);
    {
        let mut stmt = conn.prepare(
            "SELECT cut, COUNT(*) FROM video_facets
              WHERE artist_tidal_id = ?1 AND programmable = 1 GROUP BY cut",
        )?;
        let rows = stmt.query_map([artist], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            match row? {
                (cut, n) if cut == "live" => cuts.0 += n,
                (cut, n) if cut == "session" => cuts.1 += n,
                _ => {}
            }
        }
    }
    let concerts: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_concerts WHERE artist_tidal_id = ?1",
        [artist],
        |row| row.get(0),
    )?;
    let collabs = names(
        conn,
        journey::cached_collab(conn)?
            .collaborators(artist)
            .into_iter()
            .map(|(id, _)| id),
        3,
    )?;
    let scene = match scene_id {
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
    let mut near: Vec<(i64, f64)> = graph
        .neighbors(artist)
        .map(|(id, weight, _)| (id, weight))
        .collect();
    near.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let neighbours: Vec<String> = names(conn, near.into_iter().map(|(id, _)| id), 4)?
        .into_iter()
        .filter(|name| !collabs.contains(name))
        .take(2)
        .collect();
    let (plays, days_since): (i64, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), CAST(julianday('now') - julianday(MAX(started_at)) AS INTEGER)
           FROM video_history WHERE artist_tidal_id = ?1",
        [artist],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let via = match via {
        Some(id) => artist_name(conn, id)?,
        None => None,
    };
    let (photo_url, stored_bio): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT photo_url, biography FROM artists WHERE tidal_id = ?1",
            [artist],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or((None, None));
    let facts = Facts {
        name,
        videos,
        hours,
        first_year: years.iter().min().copied(),
        last_year: years.iter().max().copied(),
        peak_decade: today::peak_decade(&years, 4, 0.5),
        live: cuts.0,
        sessions: cuts.1,
        concerts,
        collabs,
        scene,
        neighbours,
        plays,
        days_since,
        via,
        origin: None,
    };
    Ok(Some(Gathered {
        kind,
        artist_id: artist,
        facts,
        photo_url,
        scene_id,
        videos: best_videos(conn, artist, 3)?,
        stored_bio,
        enrichment: load_enrichment(conn, artist)?,
    }))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(LOOKUP_TIMEOUT)
        .user_agent("NOORwave/1.0 (music app; daily artist notes)")
        .build()
        .unwrap_or_default()
}

/// Where and when an artist began, from Wikidata; None when not found or
/// ambiguous.
async fn wikidata_origin(http: &reqwest::Client, name: &str) -> Option<Origin> {
    let response = http
        .get("https://query.wikidata.org/sparql")
        .header(reqwest::header::ACCEPT, "application/sparql-results+json")
        .query(&[
            ("query", today::wikidata_query(name)),
            ("format", "json".to_string()),
        ])
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let json: Value = response.json().await.ok()?;
    today::parse_wikidata(&json)
}

/// A biography: TIDAL's, else Last.fm's.
async fn fetch_bio(
    state: &SharedState,
    http: &reqwest::Client,
    artist: i64,
    name: &str,
) -> Option<(String, String)> {
    let (tokens, session) = {
        let s = state.read().await;
        (s.tidal.tokens(), s.tidal.clone())
    };
    let tokens = match tokens {
        Some(tokens) => Some(tokens),
        None => super::tidal_auth_routes::load_persisted_tidal_tokens(state)
            .await
            .ok()
            .flatten(),
    };
    if let Some(tokens) = tokens {
        let client = TidalClient::for_session(session, &tokens.country_code);
        if let Ok(Ok(bio)) =
            tokio::time::timeout(LOOKUP_TIMEOUT, client.get_artist_bio(artist)).await
            && let Some(text) = bio
                .text
                .or(bio.summary)
                .filter(|text| !text.trim().is_empty())
        {
            return Some((text, "TIDAL".to_string()));
        }
    }
    let db = state.read().await.db.clone();
    let lastfm = LastFmClient::load(http.clone(), &db)?;
    let text = lastfm.artist_bio(name).await.ok().flatten()?;
    Some((text, "Last.fm".to_string()))
}

/// Looks up what is missing or stale, keeps it, and returns it.
async fn enrich(state: &SharedState, http: &reqwest::Client, gathered: &Gathered) -> Enrichment {
    let now = now_unix();
    if let Some(known) = &gathered.enrichment {
        let fresh = known.fetched_at + ENRICH_KEEP_S > now;
        let nothing_found = known.bio.is_none() && known.origin.is_none();
        if fresh && !(nothing_found && known.fetched_at + ENRICH_RETRY_S < now) {
            return known.clone();
        }
    }
    let mut enrichment = Enrichment {
        fetched_at: now,
        ..Enrichment::default()
    };
    if cfg!(test) {
        return enrichment;
    }
    let name = gathered.facts.name.clone();
    let stored = gathered
        .stored_bio
        .clone()
        .filter(|bio| !bio.trim().is_empty());
    let (origin, bio) = tokio::join!(wikidata_origin(http, &name), async {
        match stored {
            Some(bio) => Some((bio, "TIDAL".to_string())),
            None => fetch_bio(state, http, gathered.artist_id, &name).await,
        }
    });
    enrichment.origin = origin;
    if let Some((text, source)) = bio {
        enrichment.bio = Some(text);
        enrichment.bio_source = Some(source);
    }
    let db = state.read().await.db.clone();
    let artist = gathered.artist_id;
    let keep = enrichment.clone();
    let _ = tokio::task::spawn_blocking(move || {
        db.with_conn(|conn| store_enrichment(conn, artist, &keep))
    })
    .await;
    enrichment
}

/// GET /api/videos/today
pub(super) async fn get_video_today(State(state): State<SharedState>) -> Reply {
    let db = { state.read().await.db.clone() };
    let (today_s, _) = clock::local_now();
    let day = today_s.clone();
    let gathered = tokio::task::spawn_blocking(move || {
        let conn = db.open_isolated().ok();
        let run = |conn: &Connection| -> anyhow::Result<Vec<Gathered>> {
            let mut out = Vec::new();
            for (kind, artist, via) in picks(conn, &day)? {
                if let Some(piece) = gather(conn, kind, artist, via)? {
                    out.push(piece);
                }
            }
            Ok(out)
        };
        match conn {
            Some(conn) => run(&conn),
            None => db.with_conn(run),
        }
    })
    .await;
    let gathered = match gathered {
        Ok(Ok(gathered)) => gathered,
        Ok(Err(error)) => return failed(error),
        Err(error) => return failed(anyhow::anyhow!("today task failed: {error}")),
    };

    let http = http();
    let enrichments =
        futures::future::join_all(gathered.iter().map(|piece| enrich(&state, &http, piece))).await;
    let pieces = gathered
        .into_iter()
        .zip(enrichments)
        .map(|(mut piece, enrichment)| {
            piece.facts.origin = enrichment.origin.clone();
            let bio = enrichment
                .bio
                .as_deref()
                .and_then(today::bio_excerpt)
                .map(|text| BioOut {
                    text,
                    source: enrichment
                        .bio_source
                        .clone()
                        .unwrap_or_else(|| "TIDAL".to_string()),
                });
            PieceOut {
                kind: piece.kind,
                kicker: piece.kind.kicker(),
                artist_id: piece.artist_id,
                paragraphs: today::write(piece.kind, &piece.facts),
                name: piece.facts.name,
                bio,
                photo_url: piece.photo_url,
                scene_id: piece.scene_id,
                videos: piece.videos,
            }
        })
        .collect();
    match serde_json::to_value(TodayOut {
        day: today_s,
        pieces,
    }) {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => failed(error.into()),
    }
}

fn failed(error: anyhow::Error) -> Reply {
    tracing::warn!(target: "noor.video_air", %error, "today failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "today failed" })),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::Database;
    use crate::services::video_air::test_support;

    fn seeded() -> Database {
        let db = crate::server::routes::tests::fresh_migrated_db();
        db.with_conn(|conn| {
            for artist in 0..40 {
                for n in 0..6 {
                    test_support::add_video(
                        conn,
                        10_000 + artist * 100 + n,
                        500 + artist,
                        "official",
                        &format!("song {n}"),
                        240,
                    );
                }
            }
            // Artist 501, watched twice a month ago: from the archive.
            conn.execute_batch(
                "INSERT INTO video_history (tidal_video_id, artist_tidal_id, artist_name, started_at, completed)
                 VALUES (10100, 501, 'Artist 501', datetime('now', '-30 days'), 1),
                        (10101, 501, 'Artist 501', datetime('now', '-31 days'), 1);",
            )?;
            Ok(())
        })
        .unwrap();
        db
    }

    #[tokio::test]
    async fn today_writes_its_pieces_from_the_catalog_without_the_network() {
        let db = seeded();
        let state: SharedState = Arc::new(tokio::sync::RwLock::new(
            crate::server::routes::tests::fresh_test_state(db),
        ));
        let (status, Json(body)) = get_video_today(State(state)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["day"].is_string());
        let pieces = body["pieces"].as_array().unwrap();
        let archive = pieces
            .iter()
            .find(|p| p["kind"] == "archive")
            .expect("the artist watched a month ago comes back");
        assert_eq!(archive["artist_id"], 501);
        assert_eq!(archive["kicker"], "From your archive");
        let lead = archive["paragraphs"][0].as_str().unwrap();
        assert!(
            lead.starts_with("You watched them twice, the last "),
            "{lead}"
        );
        assert!(
            archive["paragraphs"][1]
                .as_str()
                .unwrap()
                .starts_with("6 videos here")
        );
        assert!(archive["bio"].is_null(), "no lookups in tests");
        for piece in pieces {
            let videos = piece["videos"].as_array().unwrap();
            assert!(!videos.is_empty() && videos.len() <= 3);
        }
        let ids: HashSet<i64> = pieces
            .iter()
            .map(|p| p["artist_id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids.len(), pieces.len(), "never the same artist twice");
    }
}
