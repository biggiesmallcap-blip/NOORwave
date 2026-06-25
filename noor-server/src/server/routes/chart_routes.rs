use crate::SharedState;
use crate::db::queries;
use crate::metadata::lastfm::LastFmClient;
use crate::services::tidal::client::TidalClient;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

// ─── Trending / Charts (Phase 5) ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(super) struct ChartParams {
    /// "lastfm" (default) or "tidal".
    source: Option<String>,
    /// Max entries to return (clamped 1..=100).
    limit: Option<u32>,
    /// Optional country (Last.fm only). Accepts either an ISO 3166-1 alpha-2
    /// code (e.g. "AU") which is mapped via `CURATED_COUNTRIES`, or the full
    /// English name (e.g. "United States") for legacy/free-form callers.
    country: Option<String>,
    /// Optional curated genre key (Last.fm only), e.g. "hip-hop".
    /// Mutually exclusive with `country`.
    tag: Option<String>,
    /// Last.fm chart kind: "tracks" (default), "artists", or "tags".
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChartSnapshotParams {
    source: Option<String>,
    period: Option<String>,
    region: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SpotifyDailyImportParams {
    region: Option<String>,
    date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ChartTidalPlayable {
    tidal_id: i64,
    title: String,
    artist_name: Option<String>,
    artist_tidal_id: Option<i64>,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
    track_id: Option<i64>,
    is_in_library: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ChartEntryDto {
    /// Local library Track when the chart entry was resolved to a known track.
    /// Frontend renders these via `<TrackRow>` and gets full menu support.
    local_track: Option<crate::db::models::Track>,
    /// Otherwise a TidalPlayable-shaped DTO that the frontend renders via
    /// `<TidalTrackRow>`. May be `None` only when both resolutions failed (rare).
    tidal_playable: Option<ChartTidalPlayable>,
    /// Optional preview image (mostly for Last.fm entries with no Tidal match).
    image_url: Option<String>,
    /// Source-tagged for the frontend ("lastfm" | "tidal").
    source: String,
    /// Top genre name for the resolved local track (None for Tidal-only entries
    /// where we have no genre data without an extra API call).
    genre: Option<String>,
    /// Entity shape for mixed Last.fm panels.
    entity_type: String,
    display_title: String,
    display_subtitle: Option<String>,
    metric_label: Option<String>,
}

const CHART_TRACK_SELECT_COLUMNS: &str =
    "SELECT t.id, t.title, t.artist_id, a.name as artist_name, t.album_id, al.title as album_title,
        t.disc_number, t.track_number, t.duration_ms, t.isrc, t.tidal_id, t.ytmusic_id,
        t.soundcloud_id, t.best_quality, t.best_source, t.fidelity_score, t.is_favorite,
        t.play_count, t.last_played_at, t.date_added, t.source, t.artwork_url
    FROM tracks t
    LEFT JOIN artists a ON t.artist_id = a.id
    LEFT JOIN albums al ON t.album_id = al.id";

/// Look up the most-confident genre name for each track id in a single query.
/// Returns a map keyed by track_id; tracks with no genre rows are absent.
fn fetch_top_genres_for_tracks(
    db: &crate::db::Database,
    track_ids: &[i64],
) -> HashMap<i64, String> {
    if track_ids.is_empty() {
        return HashMap::new();
    }
    db.with_conn(|conn| {
        let placeholders = std::iter::repeat_n("?", track_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        // Pick the highest-confidence genre per track. Ties broken by
        // alphabetical order so the result is stable.
        let sql = format!(
            "SELECT track_id, name FROM (
                SELECT tg.track_id, g.name,
                       ROW_NUMBER() OVER (
                           PARTITION BY tg.track_id
                           ORDER BY tg.confidence DESC, g.name ASC
                       ) AS rn
                FROM track_genres tg
                JOIN genres g ON g.id = tg.genre_id
                WHERE tg.track_id IN ({placeholders})
             ) WHERE rn = 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = track_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.insert(id, name);
        }
        Ok(map)
    })
    .unwrap_or_default()
}

/// Cached chart payload with insertion timestamp.
struct ChartCacheEntry {
    inserted_at: Instant,
    payload: serde_json::Value,
}

const MATRIX_PROVIDERS: [(&str, &str); 6] = [
    ("itunes_daily", "iTunes"),
    ("spotify_daily", "Spotify"),
    ("apple_music_daily", "Apple Music"),
    ("youtube_daily", "YouTube"),
    ("shazam_daily", "Shazam"),
    ("deezer_daily", "Deezer"),
];

const MAIN_MATRIX_REGIONS: [&str; 6] = ["global", "US", "UK", "AU", "CA", "NZ"];

fn chart_cache() -> &'static StdMutex<HashMap<String, ChartCacheEntry>> {
    static CACHE: OnceLock<StdMutex<HashMap<String, ChartCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// Charts don't move minute-to-minute and Last.fm rate-limits aggressively.
/// 2-hour TTL matches the frontend's in-memory cache so the trending shelf
/// stays static across page navigations within the window.
const CHART_CACHE_TTL: Duration = Duration::from_secs(2 * 60 * 60);

fn chart_cache_get(key: &str) -> Option<serde_json::Value> {
    let cache = chart_cache().lock().ok()?;
    let entry = cache.get(key)?;
    if entry.inserted_at.elapsed() > CHART_CACHE_TTL {
        return None;
    }
    Some(entry.payload.clone())
}

fn chart_cache_put(key: String, payload: serde_json::Value) {
    if let Ok(mut cache) = chart_cache().lock() {
        // Bound cache size to avoid unbounded growth from arbitrary
        // source/country/limit combos. 32 entries is plenty.
        if cache.len() >= 32 {
            cache.clear();
        }
        cache.insert(
            key,
            ChartCacheEntry {
                inserted_at: Instant::now(),
                payload,
            },
        );
    }
}

pub(super) async fn get_charts(
    State(state): State<SharedState>,
    Query(params): Query<ChartParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::charts::curated;

    let source = params
        .source
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "lastfm".to_string());
    let kind = params
        .kind
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "tracks".to_string());
    let limit = params.limit.unwrap_or(50).clamp(1, 100);

    if !matches!(kind.as_str(), "tracks" | "artists" | "tags") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown_chart_kind" })),
        ));
    }
    if source == "tidal" && kind != "tracks" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported_chart_kind" })),
        ));
    }

    let country_input = params
        .country
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let tag_input = params
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if country_input.is_some() && tag_input.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "tag_country_exclusive" })),
        ));
    }

    // Resolve country: ISO code or full name to curated entry. Two-char inputs
    // that aren't curated codes are rejected; longer strings that don't match
    // a curated `lastfm_name` pass through as free-form (legacy callers).
    // The cache token is always the ISO code when curated, so `?country=AU`
    // and `?country=Australia` collapse to one cache entry.
    let (country_resolved, country_cache_token): (Option<String>, Option<String>) =
        match country_input {
            None => (None, None),
            Some(s) if s.len() == 2 => match curated::find_country_by_code(s) {
                Some(entry) => (
                    Some(entry.lastfm_name.to_string()),
                    Some(entry.code.to_string()),
                ),
                None => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "unknown_country" })),
                    ));
                }
            },
            Some(s) => match curated::find_country_by_code_or_name(s) {
                Some(entry) => (
                    Some(entry.lastfm_name.to_string()),
                    Some(entry.code.to_string()),
                ),
                None => (Some(s.to_string()), Some(s.to_ascii_uppercase())),
            },
        };

    let tag_resolved: Option<&'static curated::GenreEntry> = match tag_input {
        Some(key) => match curated::find_genre(key) {
            Some(entry) => Some(entry),
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "unknown_genre" })),
                ));
            }
        },
        None => None,
    };

    let cache_key = format!(
        "{}|{}|{}|{}|{}",
        source,
        kind,
        limit,
        country_cache_token.as_deref().unwrap_or(""),
        tag_resolved.map(|g| g.key).unwrap_or("")
    );
    if let Some(cached) = chart_cache_get(&cache_key) {
        return Ok(Json(cached));
    }

    let entries: Vec<ChartEntryDto> = match source.as_str() {
        "tidal" => fetch_tidal_chart(&state, limit as i32).await.map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("tidal chart: {e}") })),
            )
        })?,
        _ => fetch_lastfm_chart(
            &state,
            limit,
            &kind,
            country_resolved.as_deref(),
            tag_resolved,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("lastfm chart: {e}") })),
            )
        })?,
    };

    let payload = json!({
        "source": source,
        "kind": kind,
        "limit": limit,
        "country": country_cache_token,
        "tag": tag_resolved.map(|g| g.key),
        "items": entries.clone(),
        "tracks": entries,
    });
    chart_cache_put(cache_key, payload.clone());
    Ok(Json(payload))
}

pub(super) async fn get_chart_snapshots(
    State(state): State<SharedState>,
    Query(params): Query<ChartSnapshotParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let source_key = params
        .source
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "spotify_daily".to_string());
    let period = params
        .period
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "daily".to_string());
    let region = params
        .region
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.eq_ignore_ascii_case("global") {
                "global".to_string()
            } else {
                s.to_ascii_uppercase()
            }
        })
        .unwrap_or_else(|| "global".to_string());
    let limit = params.limit.unwrap_or(50).clamp(1, 200);

    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let snapshot = db
        .with_conn(|conn| {
            queries::get_latest_chart_snapshot(conn, &source_key, &region, &period, limit)
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("chart snapshots: {e}") })),
            )
        })?;

    match snapshot {
        Some(snapshot) => Ok(Json(json!({
            "source": source_key,
            "period": period,
            "region": region,
            "limit": limit,
            "snapshot": snapshot.snapshot,
            "entries": snapshot.entries,
        }))),
        None => Ok(Json(json!({
            "source": source_key,
            "period": period,
            "region": region,
            "limit": limit,
            "snapshot": Value::Null,
            "entries": [],
        }))),
    }
}

pub(super) async fn import_spotify_daily_snapshot(
    State(state): State<SharedState>,
    Query(params): Query<SpotifyDailyImportParams>,
    body: String,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let region = normalize_chart_region(params.region.as_deref());
    let now = chrono::Utc::now();
    let chart_date = params
        .date
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| now.date_naive().to_string());
    let fetched_at = now.timestamp();
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let snapshot_id = db
        .with_conn(|conn| {
            crate::services::charts::spotify_daily::ingest_spotify_daily_csv(
                conn,
                &region,
                &chart_date,
                fetched_at,
                &body,
            )
        })
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("spotify daily import: {e}") })),
            )
        })?;

    Ok(Json(json!({
        "source": "spotify_daily",
        "period": "daily",
        "region": region,
        "chart_date": chart_date,
        "snapshot_id": snapshot_id,
    })))
}

pub(super) async fn get_chart_matrix(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let region_group = params
        .get("region_group")
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".to_string());

    if region_group != "main" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown_region_group" })),
        ));
    }

    let provider_keys: Vec<&str> = MATRIX_PROVIDERS.iter().map(|(key, _)| *key).collect();
    let providers: Vec<_> = MATRIX_PROVIDERS
        .iter()
        .map(|(source_key, label)| json!({ "source_key": source_key, "label": label }))
        .collect();

    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let rows = db
        .with_conn(|conn| {
            queries::get_chart_matrix(conn, &MAIN_MATRIX_REGIONS, &provider_keys, "daily")
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("chart matrix: {e}") })),
            )
        })?;

    Ok(Json(json!({
        "region_group": region_group,
        "period": "daily",
        "providers": providers,
        "rows": rows,
    })))
}

pub(super) async fn refresh_chart_matrix(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (http, db) = {
        let s = state.read().await;
        (s.http_client.clone(), s.db.clone())
    };

    let pages = crate::services::charts::kworb_matrix::fetch_kworb_chart_pages(&http)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("kworb matrix fetch: {e}") })),
            )
        })?;
    let now = chrono::Utc::now();
    let chart_date = now.date_naive().to_string();
    let fetched_at = now.timestamp();
    let report = db
        .with_conn(|conn| {
            crate::services::charts::kworb_matrix::ingest_kworb_chart_pages(
                conn,
                &chart_date,
                fetched_at,
                &pages,
            )
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("kworb matrix ingest: {e}") })),
            )
        })?;

    Ok(Json(json!({
        "source": "kworb",
        "chart_date": chart_date,
        "fetched_at": fetched_at,
        "report": report,
    })))
}

fn normalize_chart_region(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.eq_ignore_ascii_case("global") {
                "global".to_string()
            } else {
                s.to_ascii_uppercase()
            }
        })
        .unwrap_or_else(|| "global".to_string())
}

pub(super) async fn list_lastfm_genres() -> Json<Value> {
    use crate::services::charts::curated::{CURATED_GENRES, DEFAULT_GENRE_KEY};
    let genres: Vec<_> = CURATED_GENRES
        .iter()
        .map(|g| json!({ "key": g.key, "label": g.label }))
        .collect();
    Json(json!({ "genres": genres, "default_genre": DEFAULT_GENRE_KEY }))
}

pub(super) async fn list_lastfm_countries() -> Json<Value> {
    use crate::services::charts::curated::{CURATED_COUNTRIES, DEFAULT_COUNTRY_CODE};
    let countries: Vec<_> = CURATED_COUNTRIES
        .iter()
        .map(|c| json!({ "code": c.code, "label": c.label }))
        .collect();
    Json(json!({ "countries": countries, "default_country": DEFAULT_COUNTRY_CODE }))
}

async fn fetch_lastfm_chart(
    state: &SharedState,
    limit: u32,
    kind: &str,
    country: Option<&str>,
    genre: Option<&'static crate::services::charts::curated::GenreEntry>,
) -> anyhow::Result<Vec<ChartEntryDto>> {
    let (http, db) = {
        let s = state.read().await;
        (s.http_client.clone(), s.db.clone())
    };
    let client = LastFmClient::load(http, &db)
        .ok_or_else(|| anyhow::anyhow!("Last.fm API key not configured"))?;

    match kind {
        "artists" => return fetch_lastfm_artist_chart(&client, limit, country, genre).await,
        "tags" => return fetch_lastfm_tag_chart(&client, limit).await,
        _ => {}
    }

    let tracks = load_lastfm_track_chart(&client, limit, country, genre).await?;

    // Resolve each (artist, title) to a local library track when present.
    // We do this in a single DB call by collecting all (artist, title) pairs
    // and matching case-insensitively.
    let pairs: Vec<(String, String)> = tracks
        .iter()
        .map(|t| (t.artist.clone(), t.title.clone()))
        .collect();
    let local_map = resolve_chart_pairs_to_local(&db, &pairs).unwrap_or_default();
    let local_ids: Vec<i64> = local_map.values().map(|t| t.id).collect();
    let genre_map = fetch_top_genres_for_tracks(&db, &local_ids);

    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        let key = format!(
            "{}\u{0001}{}",
            t.artist.to_ascii_lowercase(),
            t.title.to_ascii_lowercase()
        );
        let local_track = local_map.get(&key).cloned();
        let genre = local_track
            .as_ref()
            .and_then(|lt| genre_map.get(&lt.id).cloned());
        let tidal_playable = if local_track.is_none() {
            // No local match; expose a TidalPlayable-shaped placeholder. The
            // frontend will resolve to a real Tidal id via search if the user
            // clicks play (existing ephemeral-play flow does this).
            Some(ChartTidalPlayable {
                tidal_id: 0,
                title: t.title.clone(),
                artist_name: Some(t.artist.clone()),
                artist_tidal_id: None,
                album_title: None,
                artwork_url: t.image_url.clone(),
                duration_ms: None,
                track_id: None,
                is_in_library: false,
            })
        } else {
            None
        };
        out.push(ChartEntryDto {
            display_title: t.title.clone(),
            display_subtitle: Some(t.artist.clone()),
            metric_label: metric_label(t.listeners, "listeners")
                .or_else(|| metric_label(t.playcount, "plays")),
            entity_type: "track".to_string(),
            local_track,
            tidal_playable,
            image_url: t.image_url,
            source: "lastfm".to_string(),
            genre,
        });
    }
    Ok(out)
}

async fn load_lastfm_track_chart(
    client: &LastFmClient,
    limit: u32,
    country: Option<&str>,
    genre: Option<&'static crate::services::charts::curated::GenreEntry>,
) -> anyhow::Result<Vec<crate::metadata::lastfm::LastFmChartTrack>> {
    if let Some(genre) = genre {
        if genre.lastfm_tags.len() == 1 {
            return client
                .get_top_tracks_by_tag(genre.lastfm_tags[0], limit)
                .await;
        }
        use futures::future::join_all;
        let fan_limit = limit.saturating_mul(2).min(100);
        let calls = genre.lastfm_tags.iter().map(|tag| {
            let c = client.clone();
            let t = (*tag).to_string();
            async move { c.get_top_tracks_by_tag(&t, fan_limit).await }
        });
        let results = join_all(calls).await;
        let mut merged: Vec<crate::metadata::lastfm::LastFmChartTrack> = Vec::new();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        for res in results {
            let list = match res {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!("tag track fan-out leg failed: {}", e);
                    continue;
                }
            };
            for t in list {
                let key = crate::services::radio::normalize_for_dedup(&t.artist, &t.title);
                if key.is_empty() {
                    continue;
                }
                if let Some(&idx) = by_key.get(&key) {
                    let existing = &mut merged[idx];
                    merge_chart_counts(&mut existing.listeners, t.listeners);
                    merge_chart_counts(&mut existing.playcount, t.playcount);
                    if existing.image_url.as_deref().unwrap_or("").is_empty() {
                        existing.image_url = t.image_url;
                    }
                    if existing.mbid.is_none() {
                        existing.mbid = t.mbid;
                    }
                } else {
                    by_key.insert(key, merged.len());
                    merged.push(t);
                }
            }
        }
        merged.sort_by(|a, b| b.playcount.unwrap_or(0).cmp(&a.playcount.unwrap_or(0)));
        merged.truncate(limit as usize);
        return Ok(merged);
    }
    client.get_top_chart(limit, country).await
}

async fn fetch_lastfm_artist_chart(
    client: &LastFmClient,
    limit: u32,
    country: Option<&str>,
    genre: Option<&'static crate::services::charts::curated::GenreEntry>,
) -> anyhow::Result<Vec<ChartEntryDto>> {
    let artists = if let Some(genre) = genre {
        if genre.lastfm_tags.len() == 1 {
            client
                .get_top_artists_by_tag(genre.lastfm_tags[0], limit)
                .await?
        } else {
            use futures::future::join_all;
            let fan_limit = limit.saturating_mul(2).min(100);
            let calls = genre.lastfm_tags.iter().map(|tag| {
                let c = client.clone();
                let t = (*tag).to_string();
                async move { c.get_top_artists_by_tag(&t, fan_limit).await }
            });
            let results = join_all(calls).await;
            let mut merged: Vec<crate::metadata::lastfm::LastFmChartArtist> = Vec::new();
            let mut by_key: HashMap<String, usize> = HashMap::new();
            for res in results {
                let list = match res {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!("tag artist fan-out leg failed: {}", e);
                        continue;
                    }
                };
                for artist in list {
                    let key = artist.name.trim().to_ascii_lowercase();
                    if key.is_empty() {
                        continue;
                    }
                    if let Some(&idx) = by_key.get(&key) {
                        let existing = &mut merged[idx];
                        merge_chart_counts(&mut existing.listeners, artist.listeners);
                        merge_chart_counts(&mut existing.playcount, artist.playcount);
                        if existing.image_url.as_deref().unwrap_or("").is_empty() {
                            existing.image_url = artist.image_url;
                        }
                        if existing.mbid.is_none() {
                            existing.mbid = artist.mbid;
                        }
                    } else {
                        by_key.insert(key, merged.len());
                        merged.push(artist);
                    }
                }
            }
            merged.sort_by(|a, b| b.listeners.unwrap_or(0).cmp(&a.listeners.unwrap_or(0)));
            merged.truncate(limit as usize);
            merged
        }
    } else {
        client.get_top_artists(limit, country).await?
    };

    Ok(artists
        .into_iter()
        .map(|artist| ChartEntryDto {
            local_track: None,
            tidal_playable: None,
            image_url: artist.image_url,
            source: "lastfm".to_string(),
            genre: None,
            entity_type: "artist".to_string(),
            display_title: artist.name,
            display_subtitle: Some("Artist".to_string()),
            metric_label: metric_label(artist.listeners, "listeners")
                .or_else(|| metric_label(artist.playcount, "plays")),
        })
        .collect())
}

async fn fetch_lastfm_tag_chart(
    client: &LastFmClient,
    limit: u32,
) -> anyhow::Result<Vec<ChartEntryDto>> {
    let tags = client.get_top_tags(limit).await?;
    Ok(tags
        .into_iter()
        .map(|tag| ChartEntryDto {
            local_track: None,
            tidal_playable: None,
            image_url: None,
            source: "lastfm".to_string(),
            genre: None,
            entity_type: "tag".to_string(),
            display_title: tag.name,
            display_subtitle: Some("Tag".to_string()),
            metric_label: metric_label(tag.reach, "reach")
                .or_else(|| metric_label(tag.count, "uses")),
        })
        .collect())
}

fn merge_chart_counts(target: &mut Option<u64>, incoming: Option<u64>) {
    *target = match (*target, incoming) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, b) => a.or(b),
    };
}

fn metric_label(value: Option<u64>, label: &str) -> Option<String> {
    value.map(|n| format!("{} {}", n, label))
}

async fn fetch_tidal_chart(state: &SharedState, limit: i32) -> anyhow::Result<Vec<ChartEntryDto>> {
    let (tokens_opt, http, db, tidal_http_client) = {
        let s = state.read().await;
        (
            s.tidal_tokens.clone(),
            s.http_client.clone(),
            s.db.clone(),
            s.tidal_http_client.clone(),
        )
    };
    let persisted = super::load_persisted_tidal_tokens(state).await?;
    let tokens = tokens_opt.or(persisted);
    let Some(tokens) = tokens else {
        // Tidal not connected; degrade to empty list rather than failing.
        tracing::warn!("Tidal chart requested but Tidal not connected");
        return Ok(Vec::new());
    };
    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let tracks = match client.get_editorial_top_tracks(limit).await {
        Ok(t) => t,
        Err(e) if super::error_looks_like_auth(&e) => {
            match super::recover_tidal_client(state, &tokens).await {
                Ok(retry_client) => retry_client
                    .get_editorial_top_tracks(limit)
                    .await
                    .unwrap_or_else(|retry_err| {
                        tracing::warn!("Tidal editorial chart retry failed: {}", retry_err);
                        Vec::new()
                    }),
                Err(refresh_err) => {
                    tracing::warn!("Tidal session refresh failed for chart: {}", refresh_err);
                    Vec::new()
                }
            }
        }
        Err(e) => {
            tracing::warn!("Tidal editorial chart failed: {}", e);
            Vec::new()
        }
    };
    if tracks.is_empty() {
        return Ok(Vec::new());
    }
    let _ = http; // currently unused; reserved for future fallback paths

    // Resolve to local tracks via tidal_id batch lookup.
    let tidal_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let known: HashMap<i64, i64> = db
        .with_conn(|conn| queries::get_tidal_track_local_ids(conn, &tidal_ids))
        .unwrap_or_default();

    // Pull full Track rows for any local matches.
    let local_ids: Vec<i64> = known.values().copied().collect();
    let local_tracks: HashMap<i64, crate::db::models::Track> = db
        .with_conn(|conn| {
            let mut map: HashMap<i64, crate::db::models::Track> = HashMap::new();
            if local_ids.is_empty() {
                return Ok(map);
            }
            let mut stmt = conn.prepare(
                &(CHART_TRACK_SELECT_COLUMNS.to_string()
                    + "
                    WHERE t.id = ?1
                    LIMIT 1"),
            )?;
            for id in &local_ids {
                let mut rows = stmt.query(rusqlite::params![id])?;
                if let Some(row) = rows.next()? {
                    let track = chart_track_from_joined_row(row)?;
                    map.insert(*id, track);
                }
            }
            Ok(map)
        })
        .unwrap_or_default();

    let resolved_local_ids: Vec<i64> = local_tracks.values().map(|t| t.id).collect();
    let genre_map = fetch_top_genres_for_tracks(&db, &resolved_local_ids);

    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        let local_track = known
            .get(&t.id)
            .and_then(|lid| local_tracks.get(lid))
            .cloned();
        let genre = local_track
            .as_ref()
            .and_then(|lt| genre_map.get(&lt.id).cloned());
        let tidal_playable = if local_track.is_none() {
            Some(ChartTidalPlayable {
                tidal_id: t.id,
                title: t.title.clone(),
                artist_name: t.artist_name.clone(),
                artist_tidal_id: t.artist_id,
                album_title: t.album_title.clone(),
                artwork_url: t.artwork_url.clone(),
                duration_ms: Some(t.duration * 1000),
                track_id: None,
                is_in_library: false,
            })
        } else {
            None
        };
        out.push(ChartEntryDto {
            image_url: t.artwork_url.clone(),
            local_track,
            tidal_playable,
            source: "tidal".to_string(),
            genre,
            entity_type: "track".to_string(),
            display_title: t.title.clone(),
            display_subtitle: t.artist_name.clone(),
            metric_label: None,
        });
    }
    Ok(out)
}

/// Resolve (artist, title) pairs to local Track rows, case-insensitively.
/// Uses a single SQL query with a fold-table; falls back to empty map on error.
fn resolve_chart_pairs_to_local(
    db: &crate::db::Database,
    pairs: &[(String, String)],
) -> anyhow::Result<HashMap<String, crate::db::models::Track>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let mut out = HashMap::new();
    db.with_conn(|conn| {
        // Match by lower(artist_name) + lower(title). Library is small enough
        // that one round-trip per pair is acceptable; a single OR-chained
        // query also works but is harder to map back to pairs.
        let mut stmt = conn.prepare(
            &(CHART_TRACK_SELECT_COLUMNS.to_string()
                + "
                WHERE LOWER(a.name) = LOWER(?1) AND LOWER(t.title) = LOWER(?2)
                LIMIT 1"),
        )?;
        for (artist, title) in pairs {
            let mut rows = stmt.query(rusqlite::params![artist, title])?;
            if let Some(row) = rows.next()? {
                let track = chart_track_from_joined_row(row)?;
                let key = format!(
                    "{}\u{0001}{}",
                    artist.to_ascii_lowercase(),
                    title.to_ascii_lowercase()
                );
                out.insert(key, track);
            }
        }
        Ok(())
    })?;
    Ok(out)
}

fn chart_track_from_joined_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<crate::db::models::Track> {
    Ok(crate::db::models::Track {
        id: row.get(0)?,
        title: row.get(1)?,
        artist_id: row.get(2)?,
        artist_name: row.get(3)?,
        album_id: row.get(4)?,
        album_title: row.get(5)?,
        disc_number: row.get(6)?,
        track_number: row.get(7)?,
        duration_ms: row.get(8)?,
        isrc: row.get(9)?,
        tidal_id: row.get(10)?,
        ytmusic_id: row.get(11)?,
        soundcloud_id: row.get(12)?,
        best_quality: row.get(13)?,
        best_source: row.get(14)?,
        fidelity_score: row.get(15)?,
        is_favorite: row.get::<_, i64>(16)? != 0,
        play_count: row.get(17)?,
        last_played_at: row.get(18)?,
        date_added: row.get(19)?,
        source: row.get(20)?,
        artwork_url: row.get(21)?,
    })
}
