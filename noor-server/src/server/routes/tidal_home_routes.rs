use crate::SharedState;
use crate::db::queries;
use crate::services::tidal::client::{TidalClient, TidalHomeModule};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

type TidalMoodCategoriesCache = Arc<Mutex<Option<(Instant, Duration, Vec<Value>)>>>;
type TidalPageModulesCache = Arc<Mutex<HashMap<String, (Instant, Vec<TidalHomeModule>)>>>;

const TIDAL_HOME_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const TIDAL_MOODS_FALLBACK_CACHE_TTL: Duration = Duration::from_secs(60);
const TIDAL_MOODS_PROBE_FAILURE_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const MOOD_THUMBNAIL_FETCH_CONCURRENCY: usize = 4;
const MOOD_THUMBNAIL_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const TIDAL_HOME_MODULES_PAGE_PATH: &str = "pages/home";
const TIDAL_MODULE_ITEMS_DEFAULT_LIMIT: u32 = 50;
const TIDAL_MODULE_ITEMS_MAX_LIMIT: u32 = 200;
// Real TIDAL `pages/*` module ids are base64-encoded JSON tokens (~150+ chars),
// not the short slugs the first cut of this validator assumed. Keep a generous
// upper bound so the value stays bounded without rejecting legitimate ids.
const TIDAL_MODULE_ID_MAX_LEN: usize = 512;
const TIDAL_MIX_ID_MAX_LEN: usize = 96;
const TIDAL_PAGE_ID_MAX_LEN: usize = 96;
const ROUTE_TIMING_INFO_THRESHOLD_MS: u128 = 500;
static TIDAL_MOODS_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static TIDAL_MOODS_PROBE_FAILURE_COOLDOWN_UNTIL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
const DEFAULT_TIDAL_MOOD_CATEGORIES: &[(&str, &str)] = &[
    ("mood_party", "Party"),
    ("mood_workout", "Workout"),
    ("mood_focus", "Focus"),
    ("mood_relax", "Relax"),
    ("mood_sleep", "Sleep"),
    ("mood_love", "Love"),
    ("m_happy", "Happy"),
    ("m_celebration", "Celebration"),
    ("mood_djselector", "DJ Selector"),
];

enum MoodProbeOutcome {
    Modules {
        slug: String,
        modules: Vec<TidalHomeModule>,
        cache_hit: bool,
    },
    Timeout {
        slug: String,
    },
    Error {
        slug: String,
    },
}

struct TidalMoodsRefreshGuard;

impl Drop for TidalMoodsRefreshGuard {
    fn drop(&mut self) {
        TIDAL_MOODS_REFRESH_IN_FLIGHT.store(false, Ordering::Release);
    }
}

fn try_begin_tidal_moods_refresh() -> Option<TidalMoodsRefreshGuard> {
    TIDAL_MOODS_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| TidalMoodsRefreshGuard)
}

fn mood_probe_failed_without_results(
    fetched: usize,
    cache_hits: usize,
    timeouts: usize,
    errors: usize,
) -> bool {
    fetched == 0 && cache_hits == 0 && (timeouts > 0 || errors > 0)
}

fn tidal_moods_probe_failure_cooldown() -> &'static Mutex<Option<Instant>> {
    TIDAL_MOODS_PROBE_FAILURE_COOLDOWN_UNTIL.get_or_init(|| Mutex::new(None))
}

fn tidal_moods_probe_cooldown_remaining(
    cooldown: &Mutex<Option<Instant>>,
    now: Instant,
) -> Option<Duration> {
    let Ok(mut guard) = cooldown.lock() else {
        return None;
    };

    match *guard {
        Some(until) if now < until => Some(until.duration_since(now)),
        Some(_) => {
            *guard = None;
            None
        }
        None => None,
    }
}

fn note_tidal_moods_probe_failure(cooldown: &Mutex<Option<Instant>>, now: Instant) {
    let Ok(mut guard) = cooldown.lock() else {
        return;
    };
    *guard = Some(now + TIDAL_MOODS_PROBE_FAILURE_COOLDOWN);
}

fn clear_tidal_moods_probe_failure(cooldown: &Mutex<Option<Instant>>) {
    let Ok(mut guard) = cooldown.lock() else {
        return;
    };
    *guard = None;
}

/// Returns the authenticated user's TIDAL mixes (Daily Discovery, My Mix N,
/// Master Mix, etc) for the home page Your Mixes shelf.
///
/// 503 when TIDAL is disconnected so the frontend can render its connect prompt.
pub(super) async fn get_tidal_mixes(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    // Persisted-tokens fallback covers the cold-boot race: the home page
    // mounts before `tidal_status` has rehydrated `state.tidal_tokens` from
    // disk, so a direct in-memory check returns 503 even though the user is
    // connected. Other TIDAL endpoints follow this same pattern.
    let (tokens, http_client, tidal_http_client, mixes_cache) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
                s.tidal_mixes_cache.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2, in_memory.3),
            None => {
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2, in_memory.3)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    // 6h TTL cache. TIDAL refreshes mixes daily; revisiting Home shouldn't
    // round-trip TIDAL each time. Cache is cleared on app restart.
    {
        let guard = mixes_cache.lock().unwrap();
        if let Some((stored_at, cached)) = guard.as_ref()
            && stored_at.elapsed() < TIDAL_HOME_CACHE_TTL
        {
            return Ok(Json(
                json!({ "mixes": cached, "source": "tidal", "cached": true }),
            ));
        }
    }
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let mixes = match client.get_my_mixes().await {
        Ok(mixes) => mixes,
        Err(e) if super::error_looks_like_auth(&e) => {
            let refreshed = super::recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_my_mixes().await.map_err(|e| {
                tracing::warn!("TIDAL get_my_mixes failed after token refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_my_mixes failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    {
        let mut guard = mixes_cache.lock().unwrap();
        *guard = Some((Instant::now(), mixes.clone()));
    }
    Ok(Json(json!({ "mixes": mixes, "source": "tidal" })))
}

// ─── TIDAL: Personal Radio Stations ──────────────────────────────────────────

/// Returns the user's personal TIDAL radio stations for the home shelf.
/// Same pattern as `get_tidal_mixes`: 503 when disconnected, 6h TTL cache.
pub(super) async fn get_tidal_radio_stations(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (tokens, http_client, tidal_http_client, radio_cache) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
                s.tidal_radio_stations_cache.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2, in_memory.3),
            None => {
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2, in_memory.3)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    {
        let guard = radio_cache.lock().unwrap();
        if let Some((stored_at, cached)) = guard.as_ref()
            && stored_at.elapsed() < TIDAL_HOME_CACHE_TTL
        {
            return Ok(Json(
                json!({ "stations": cached, "source": "tidal", "cached": true }),
            ));
        }
    }

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let stations = match client.get_my_radio_stations().await {
        Ok(s) => s,
        Err(e) if super::error_looks_like_auth(&e) => {
            let refreshed = super::recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_my_radio_stations().await.map_err(|e| {
                tracing::warn!("TIDAL get_my_radio_stations failed after token refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_my_radio_stations failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    {
        let mut guard = radio_cache.lock().unwrap();
        *guard = Some((Instant::now(), stations.clone()));
    }
    Ok(Json(json!({ "stations": stations, "source": "tidal" })))
}

// ─── TIDAL: Home discover modules ────────────────────────────────────────────

/// Returns the editorial modules from `pages/home` (what the TIDAL web client
/// renders as the "discover" surface: The Hits, New Tracks, New Albums,
/// Spotlighted Uploads, From our editors). 503 when TIDAL is disconnected so
/// the frontend can render its connect prompt instead of an error toast.
pub(super) async fn get_tidal_home_modules(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let started_at = Instant::now();
    let (tokens, http_client, tidal_http_client, page_modules_cache) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
                s.tidal_page_modules_cache.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2, in_memory.3),
            None => {
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2, in_memory.3)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    let (modules, cache_hit) = load_tidal_home_modules_cached(
        &state,
        &tokens,
        &http_client,
        tidal_http_client,
        &page_modules_cache,
    )
    .await?;
    let elapsed_ms = started_at.elapsed().as_millis();
    if elapsed_ms >= ROUTE_TIMING_INFO_THRESHOLD_MS {
        tracing::info!(
            route = "tidal_home_modules",
            elapsed_ms,
            cache_hit,
            module_count = modules.len(),
            "TIDAL home modules route complete"
        );
    } else {
        tracing::debug!(
            route = "tidal_home_modules",
            elapsed_ms,
            cache_hit,
            module_count = modules.len(),
            "TIDAL home modules route complete"
        );
    }
    Ok(Json(json!({ "modules": modules, "source": "tidal" })))
}

/// Returns the full item set for one home discover module, used by the
/// per-module "View all" detail route. The home preview only ships 5 items
/// for TRACK_LIST modules; this handler resolves the module id back to the
/// upstream `dataApiPath` and follows it to load the complete list.
pub(super) async fn get_tidal_discover_module_items(
    State(state): State<SharedState>,
    Path(module_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let started_at = Instant::now();
    let module_id = normalize_tidal_module_id(&module_id)?;
    let limit = normalize_tidal_module_items_limit(params.get("limit").map(String::as_str));

    let (tokens, http_client, tidal_http_client, page_modules_cache) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
                s.tidal_page_modules_cache.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2, in_memory.3),
            None => {
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2, in_memory.3)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    let (modules, home_cache_hit) = load_tidal_home_modules_cached(
        &state,
        &tokens,
        &http_client,
        tidal_http_client.clone(),
        &page_modules_cache,
    )
    .await?;

    let Some(module) = modules.into_iter().find(|m| m.id == module_id) else {
        return Err(StatusCode::NOT_FOUND);
    };
    let module_kind = module.kind.clone();
    let module_title = module.title.clone();
    // Modules without a `dataApiPath` (e.g. ALBUM_LIST already returning all
    // items inline) just echo back the preview items. That's the whole set.
    let items = if let Some(path) = module.more_path.as_deref() {
        let access_token = tokens.access_token.clone();
        let country_code = tokens.country_code.clone();
        let live = TidalClient::with_http(tidal_http_client, access_token, country_code);
        match live
            .get_module_items_via_path(path, &module_kind, limit)
            .await
        {
            Ok(items) if !items.is_empty() => items,
            Err(e) if super::error_looks_like_auth(&e) => {
                match super::recover_tidal_client(&state, &tokens).await {
                    Ok(retry_client) => match retry_client
                        .get_module_items_via_path(path, &module_kind, limit)
                        .await
                    {
                        Ok(items) if !items.is_empty() => items,
                        _ => module.items,
                    },
                    Err(refresh_err) => {
                        tracing::warn!(
                            ?refresh_err,
                            "TIDAL discover module refresh failed; serving preview items"
                        );
                        module.items
                    }
                }
            }
            _ => module.items, // fall back to the preview if the show-more call fails or returns 0
        }
    } else {
        module.items
    };

    let elapsed_ms = started_at.elapsed().as_millis();
    if elapsed_ms >= ROUTE_TIMING_INFO_THRESHOLD_MS {
        tracing::info!(
            route = "tidal_discover_module_items",
            module_id,
            limit,
            elapsed_ms,
            home_cache_hit,
            item_count = items.len(),
            "TIDAL discover module items route complete"
        );
    } else {
        tracing::debug!(
            route = "tidal_discover_module_items",
            module_id,
            limit,
            elapsed_ms,
            home_cache_hit,
            item_count = items.len(),
            "TIDAL discover module items route complete"
        );
    }

    Ok(Json(json!({
        "module": {
            "id": module_id,
            "title": module_title,
            "kind": module_kind,
            "items": items,
        },
        "source": "tidal",
    })))
}

/// Returns the playable tracks inside a TIDAL mix. Frontend calls this when
/// the user clicks a mix card on the home Your Mixes shelf, then queues +
/// plays the first track via the existing TIDAL playback path.
pub(super) async fn get_tidal_mix_tracks(
    State(state): State<SharedState>,
    Path(mix_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mix_id = normalize_tidal_mix_id(&mix_id)
        .map_err(|status| (status, Json(json!({ "error": "invalid TIDAL mix id" }))))?;

    let (tokens, tidal_http_client) = {
        let s = state.read().await;
        let tidal_http = s.tidal_http_client.clone();
        let in_memory = s.tidal_tokens.clone();
        drop(s);
        match in_memory {
            Some(t) => (Some(t), tidal_http),
            None => {
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, tidal_http)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };
    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let items = match client.get_mix_tracks(mix_id).await {
        Ok(items) => items,
        Err(e) if super::error_looks_like_auth(&e) => {
            let retry_client =
                super::recover_tidal_client(&state, &tokens)
                    .await
                    .map_err(|refresh_err| {
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({
                                "error": format!("TIDAL session refresh failed: {}", refresh_err)
                            })),
                        )
                    })?;
            retry_client.get_mix_tracks(mix_id).await.map_err(|e2| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": e2.to_string() })),
                )
            })?
        }
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    let tidal_ids: Vec<i64> = items.iter().map(|t| t.id).collect();
    let library_states = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_tidal_track_library_states(conn, &tidal_ids))
            .unwrap_or_default()
    };
    let tracks: Vec<Value> = items
        .into_iter()
        .map(|t| {
            let library_state = library_states.get(&t.id).copied();
            super::tidal_track_playable_json(t, library_state, 640)
        })
        .collect();

    Ok(Json(json!({ "tracks": tracks })))
}

/// Returns editorial modules for a whitelisted TIDAL `/v1/pages/{section}` or
/// `/v1/pages/{section}/{id}` endpoint. Routed via two siblings so we never
/// open the door to arbitrary upstream paths via a wildcard extractor.
pub(super) async fn get_tidal_page_modules(
    State(state): State<SharedState>,
    Path(section): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let wire_path = resolve_page_path(&section, None)?;
    fetch_page_modules(state, wire_path).await
}

pub(super) async fn get_tidal_page_modules_with_id(
    State(state): State<SharedState>,
    Path((section, id)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
    let wire_path = resolve_page_path(&section, Some(id.as_str()))?;
    fetch_page_modules(state, wire_path).await
}

// Whitelist + wire-path normalization. Returns 404 for anything not on the
// approved list so callers can't probe arbitrary TIDAL endpoints.
fn resolve_page_path(section: &str, id: Option<&str>) -> Result<String, StatusCode> {
    let section = section.trim_matches('/');
    let normalized_id = id.map(normalize_tidal_page_id).transpose()?;
    let wire_section = match (section, normalized_id.as_deref()) {
        ("explore", None) => "explore",
        ("hires", None) => "hires",
        ("videos", None) => "videos",
        ("genres" | "genre-page" | "genre_page", None) => "genre_page",
        ("genre-page-local" | "genre_page_local", None) => "genre_page_local",
        ("new-releases" | "new_releases" | "whatsnew", None) => "whatsnew",
        ("mood" | "genre", Some(_)) => section,
        _ => return Err(StatusCode::NOT_FOUND),
    };
    Ok(match normalized_id {
        Some(id) => format!("pages/{}/{}", wire_section, id),
        None => format!("pages/{}", wire_section),
    })
}

fn normalize_tidal_page_id(id: &str) -> Result<&str, StatusCode> {
    let trimmed = id.trim();
    if trimmed.is_empty()
        || trimmed.len() > TIDAL_PAGE_ID_MAX_LEN
        || trimmed.chars().any(|c| {
            c.is_ascii_whitespace()
                || c.is_ascii_control()
                || matches!(c, '/' | '\\' | '?' | '#' | '&' | '=')
        })
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(trimmed)
}

fn normalize_tidal_mix_id(id: &str) -> Result<&str, StatusCode> {
    let trimmed = id.trim();
    if trimmed.is_empty()
        || trimmed.len() > TIDAL_MIX_ID_MAX_LEN
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(trimmed)
}

fn normalize_tidal_module_id(id: &str) -> Result<&str, StatusCode> {
    // The id is an opaque lookup key only (matched against cached module ids;
    // the outbound fetch uses the separately-validated `dataApiPath`), so it
    // never needs to be URL/path safe. Real TIDAL ids are standard base64 JSON
    // tokens, so allow the base64 alphabet (`+`/`/`/`=` padding) plus URL-safe
    // `-`/`_`, while still rejecting surrounding whitespace, control chars, and
    // query/fragment separators.
    let trimmed = id.trim();
    if trimmed.len() != id.len()
        || trimmed.is_empty()
        || trimmed.len() > TIDAL_MODULE_ID_MAX_LEN
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '='))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(trimmed)
}

fn normalize_tidal_module_items_limit(value: Option<&str>) -> u32 {
    value
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(TIDAL_MODULE_ITEMS_DEFAULT_LIMIT)
        .clamp(1, TIDAL_MODULE_ITEMS_MAX_LIMIT)
}

async fn fetch_page_modules(
    state: SharedState,
    page_path: String,
) -> Result<Json<Value>, StatusCode> {
    let (tokens, http_client, tidal_http_client, page_modules_cache) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
                s.tidal_page_modules_cache.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2, in_memory.3),
            None => {
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2, in_memory.3)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let cache_key = tidal_page_modules_cache_key(&tokens.country_code, &page_path);
    if let Some(cached) = get_cached_tidal_page_modules(&page_modules_cache, &cache_key) {
        return Ok(Json(
            json!({ "modules": cached, "source": "tidal", "page": page_path, "cached": true }),
        ));
    }
    let modules = match client.get_page_modules(&page_path).await {
        Ok(m) => m,
        Err(e) if super::error_looks_like_auth(&e) => {
            let refreshed = super::recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_page_modules(&page_path).await.map_err(|e| {
                tracing::warn!("TIDAL get_page_modules({page_path}) failed after refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_page_modules({page_path}) failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    put_cached_tidal_page_modules(&page_modules_cache, cache_key, modules.clone());
    Ok(Json(
        json!({ "modules": modules, "source": "tidal", "page": page_path }),
    ))
}

async fn load_tidal_home_modules_cached(
    state: &SharedState,
    tokens: &crate::services::tidal::auth::TidalTokens,
    http_client: &reqwest::Client,
    tidal_http_client: reqwest::Client,
    page_modules_cache: &TidalPageModulesCache,
) -> Result<(Vec<TidalHomeModule>, bool), StatusCode> {
    let cache_key =
        tidal_page_modules_cache_key(&tokens.country_code, TIDAL_HOME_MODULES_PAGE_PATH);
    if let Some(cached) = get_cached_tidal_page_modules(page_modules_cache, &cache_key) {
        return Ok((cached, true));
    }

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let modules = match client.get_home_modules().await {
        Ok(m) => m,
        Err(e) if super::error_looks_like_auth(&e) => {
            let refreshed = super::recover_tidal_session(state, http_client, tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_home_modules().await.map_err(|e| {
                tracing::warn!("TIDAL get_home_modules failed after token refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_home_modules failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    put_cached_tidal_page_modules(page_modules_cache, cache_key, modules.clone());
    Ok((modules, false))
}

fn tidal_page_modules_cache_key(country_code: &str, page_path: &str) -> String {
    format!("{country_code}:{page_path}")
}

fn get_cached_tidal_page_modules(
    cache: &TidalPageModulesCache,
    key: &str,
) -> Option<Vec<TidalHomeModule>> {
    let mut guard = cache.lock().unwrap();
    if let Some((stored_at, cached)) = guard.get(key)
        && stored_at.elapsed() < TIDAL_HOME_CACHE_TTL
    {
        return Some(cached.clone());
    }
    guard.remove(key);
    None
}

fn put_cached_tidal_page_modules(
    cache: &TidalPageModulesCache,
    key: String,
    modules: Vec<TidalHomeModule>,
) {
    let mut guard = cache.lock().unwrap();
    // Sweep expired entries on insert so the map stays bounded to the live-TTL
    // working set instead of accumulating dead keys for the process lifetime.
    // Inserts only happen on a cache miss (after a network fetch), so the O(n)
    // scan is cheap and rare.
    guard.retain(|_, (stored_at, _)| stored_at.elapsed() < TIDAL_HOME_CACHE_TTL);
    guard.insert(key, (Instant::now(), modules));
}

/// Returns the TIDAL mood / activity category list, parsed out of the
/// PAGE_LINKS module on `/v1/pages/moods`. Each entry carries the upstream
/// slug (e.g. `mood_party`) which the `/api/tidal/mood-page/{slug}` route
/// then proxies as `pages/{slug}` for the drill-down content.
pub(super) async fn get_tidal_moods(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let started_at = Instant::now();
    let (tokens, http_client, tidal_http_client) = load_tidal_session(&state).await;
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    let (mood_cache, page_modules_cache) = {
        let s = state.read().await;
        (
            s.tidal_moods_cache.clone(),
            s.tidal_page_modules_cache.clone(),
        )
    };
    if let Some(cached) = get_cached_tidal_mood_categories(&mood_cache) {
        let elapsed_ms = started_at.elapsed().as_millis();
        if elapsed_ms >= ROUTE_TIMING_INFO_THRESHOLD_MS {
            tracing::info!(
                route = "tidal_moods",
                elapsed_ms,
                cache_hit = true,
                category_count = cached.len(),
                "TIDAL moods route complete"
            );
        } else {
            tracing::debug!(
                route = "tidal_moods",
                elapsed_ms,
                cache_hit = true,
                category_count = cached.len(),
                "TIDAL moods route complete"
            );
        }
        return Ok(Json(
            json!({ "categories": cached, "source": "tidal", "cached": true }),
        ));
    }
    let categories = default_tidal_mood_categories();
    let (response_categories, pending_probe_slugs, cached_probe_hits) =
        apply_cached_mood_category_probes(categories, &tokens.country_code, &page_modules_cache);
    let pending_probe_count = pending_probe_slugs.len();

    put_cached_tidal_mood_categories_with_ttl(
        &mood_cache,
        response_categories.clone(),
        TIDAL_MOODS_FALLBACK_CACHE_TTL,
    );

    if let Some(remaining) =
        tidal_moods_probe_cooldown_remaining(tidal_moods_probe_failure_cooldown(), Instant::now())
    {
        tracing::debug!(
            route = "tidal_moods",
            retry_in_ms = remaining.as_millis(),
            background_probe_slugs = pending_probe_count,
            "TIDAL moods refresh skipped during probe failure cooldown"
        );
    } else if let Some(refresh_guard) = try_begin_tidal_moods_refresh() {
        let state_bg = state.clone();
        let mood_cache_bg = mood_cache.clone();
        let page_modules_cache_bg = page_modules_cache.clone();
        let tokens_bg = tokens.clone();
        let http_client_bg = http_client;
        let tidal_http_client_bg = tidal_http_client.clone();
        tokio::spawn(async move {
            let _refresh_guard = refresh_guard;
            refresh_tidal_moods_cache(
                state_bg,
                mood_cache_bg,
                page_modules_cache_bg,
                tokens_bg,
                http_client_bg,
                tidal_http_client_bg,
            )
            .await;
        });
    } else if pending_probe_count > 0 {
        tracing::debug!(
            route = "tidal_moods",
            background_probe_slugs = pending_probe_count,
            "TIDAL moods refresh already running"
        );
    }

    let elapsed_ms = started_at.elapsed().as_millis();
    if elapsed_ms >= ROUTE_TIMING_INFO_THRESHOLD_MS {
        tracing::info!(
            route = "tidal_moods",
            elapsed_ms,
            cache_hit = false,
            category_count = response_categories.len(),
            cached_probe_hits,
            background_probe_slugs = pending_probe_count,
            "TIDAL moods route complete"
        );
    } else {
        tracing::debug!(
            route = "tidal_moods",
            elapsed_ms,
            cache_hit = false,
            category_count = response_categories.len(),
            cached_probe_hits,
            background_probe_slugs = pending_probe_count,
            "TIDAL moods route complete"
        );
    }
    Ok(Json(
        json!({ "categories": response_categories, "source": "tidal", "fallback": true }),
    ))
}

async fn refresh_tidal_moods_cache(
    state: SharedState,
    mood_cache: TidalMoodCategoriesCache,
    page_modules_cache: TidalPageModulesCache,
    tokens: crate::services::tidal::auth::TidalTokens,
    http_client: reqwest::Client,
    tidal_http_client: reqwest::Client,
) {
    let started_at = Instant::now();
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let raw = match client.get_page_raw("pages/moods").await {
        Ok(r) => r,
        Err(e) if super::error_looks_like_auth(&e) => {
            let Ok(refreshed) = super::recover_tidal_session(&state, &http_client, &tokens).await
            else {
                tracing::warn!("TIDAL get_tidal_moods refresh failed");
                return;
            };
            let retry = TidalClient::with_http(
                tidal_http_client.clone(),
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            match retry.get_page_raw("pages/moods").await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("TIDAL get_tidal_moods failed after refresh: {e}");
                    return;
                }
            }
        }
        Err(e) => {
            tracing::warn!("TIDAL get_tidal_moods failed: {e}");
            let probe_client = TidalClient::with_http(
                tidal_http_client.clone(),
                tokens.access_token.clone(),
                tokens.country_code.clone(),
            );
            cache_default_moods_with_thumbnails(
                mood_cache,
                page_modules_cache,
                probe_client,
                tokens.country_code.clone(),
            )
            .await;
            return;
        }
    };
    let live_categories = extract_page_links(&raw);
    if live_categories.is_empty() {
        tracing::warn!("TIDAL get_tidal_moods returned no PAGE_LINKS categories");
        let probe_client = TidalClient::with_http(
            tidal_http_client.clone(),
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );
        cache_default_moods_with_thumbnails(
            mood_cache,
            page_modules_cache,
            probe_client,
            tokens.country_code.clone(),
        )
        .await;
        return;
    }

    let (response_categories, pending_probe_slugs, cached_probe_hits) =
        apply_cached_mood_category_probes(
            live_categories,
            &tokens.country_code,
            &page_modules_cache,
        );
    put_cached_tidal_mood_categories(&mood_cache, response_categories.clone());

    if !pending_probe_slugs.is_empty() {
        let probe_client = TidalClient::with_http(
            tidal_http_client,
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );
        run_mood_thumbnail_probe_refresh(
            mood_cache.clone(),
            page_modules_cache,
            probe_client,
            tokens.country_code.clone(),
            response_categories.clone(),
            pending_probe_slugs,
        )
        .await;
    }

    tracing::info!(
        route = "tidal_moods_refresh",
        elapsed_ms = started_at.elapsed().as_millis(),
        cached_probe_hits,
        category_count = response_categories.len(),
        "TIDAL moods background refresh complete"
    );
}

/// Fallback for when the live `pages/moods` PAGE_LINKS can't be fetched or come
/// back empty. The curated default categories all map to real TIDAL pages, so
/// probe each one for a thumbnail and cache the defaults with artwork instead of
/// leaving the rail showing the thumbnail-less hardcoded fallbacks forever. Keeps
/// every default category even when its own probe yields nothing.
async fn cache_default_moods_with_thumbnails(
    mood_cache: TidalMoodCategoriesCache,
    page_modules_cache: TidalPageModulesCache,
    probe_client: TidalClient,
    country_code: String,
) {
    let categories = default_tidal_mood_categories();
    let slugs: Vec<String> = categories
        .iter()
        .filter_map(|c| c.get("slug").and_then(|s| s.as_str()).map(String::from))
        .collect();
    if slugs.is_empty() {
        return;
    }

    let fetches = slugs.into_iter().map(|slug| {
        let client = probe_client.clone();
        let cache = page_modules_cache.clone();
        let country = country_code.clone();
        async move {
            let page_path = format!("pages/{slug}");
            let cache_key = tidal_page_modules_cache_key(&country, &page_path);
            if let Some(modules) = get_cached_tidal_page_modules(&cache, &cache_key) {
                return (slug, mood_probe_from_modules(&modules).1);
            }
            match tokio::time::timeout(
                MOOD_THUMBNAIL_PROBE_TIMEOUT,
                client.get_page_modules(&page_path),
            )
            .await
            {
                Ok(Ok(modules)) => {
                    put_cached_tidal_page_modules(&cache, cache_key, modules.clone());
                    (slug, mood_probe_from_modules(&modules).1)
                }
                _ => (slug, None),
            }
        }
    });

    let outcomes = {
        use futures::StreamExt;

        futures::stream::iter(fetches)
            .buffer_unordered(MOOD_THUMBNAIL_FETCH_CONCURRENCY)
            .collect::<Vec<(String, Option<String>)>>()
            .await
    };

    let mut thumbnails: HashMap<String, String> = HashMap::new();
    for (slug, thumbnail) in outcomes {
        if let Some(url) = thumbnail {
            thumbnails.insert(slug, url);
        }
    }

    if thumbnails.is_empty() {
        // Nothing resolved (likely the same upstream problem that sank the live
        // fetch); leave any existing cache untouched rather than overwriting it.
        return;
    }

    let found = thumbnails.len();
    let merged = merge_default_mood_thumbnails(categories, &thumbnails);
    put_cached_tidal_mood_categories(&mood_cache, merged);
    tracing::info!(
        route = "tidal_moods_default_probe",
        thumbnails = found,
        "Cached default mood categories with probed thumbnails (live moods unavailable)"
    );
}

/// Attach probed thumbnails to the default mood categories by slug, keeping every
/// category whether or not its probe resolved an image.
fn merge_default_mood_thumbnails(
    categories: Vec<Value>,
    thumbnails: &HashMap<String, String>,
) -> Vec<Value> {
    categories
        .into_iter()
        .map(|mut category| {
            if let Some(slug) = category
                .get("slug")
                .and_then(|s| s.as_str())
                .map(String::from)
            {
                if let Some(url) = thumbnails.get(&slug) {
                    if let Some(obj) = category.as_object_mut() {
                        obj.insert("thumbnail".to_string(), Value::String(url.clone()));
                    }
                }
            }
            category
        })
        .collect()
}

fn apply_cached_mood_category_probes(
    categories: Vec<Value>,
    country_code: &str,
    page_modules_cache: &TidalPageModulesCache,
) -> (Vec<Value>, Vec<String>, usize) {
    let mut probe: HashMap<String, (bool, Option<String>)> = HashMap::new();
    let mut pending = std::collections::HashSet::new();
    let mut cache_hits = 0usize;

    for category in &categories {
        let Some(slug) = category.get("slug").and_then(|s| s.as_str()) else {
            continue;
        };
        let page_path = format!("pages/{slug}");
        let cache_key = tidal_page_modules_cache_key(country_code, &page_path);
        if let Some(modules) = get_cached_tidal_page_modules(page_modules_cache, &cache_key) {
            cache_hits += 1;
            probe.insert(slug.to_string(), mood_probe_from_modules(&modules));
        } else {
            pending.insert(slug.to_string());
        }
    }

    let merged = apply_mood_probe_results(categories, &probe);
    (merged, pending.into_iter().collect(), cache_hits)
}

fn apply_mood_probe_results(
    categories: Vec<Value>,
    probe: &HashMap<String, (bool, Option<String>)>,
) -> Vec<Value> {
    categories
        .into_iter()
        .filter_map(|mut category| {
            let slug = category
                .get("slug")
                .and_then(|s| s.as_str())
                .map(String::from)?;
            if let Some((is_empty, thumbnail)) = probe.get(&slug) {
                if *is_empty {
                    return None;
                }
                if let Some(url) = thumbnail {
                    if let Some(obj) = category.as_object_mut() {
                        obj.insert("thumbnail".to_string(), Value::String(url.clone()));
                    }
                }
            }
            Some(category)
        })
        .collect()
}

fn default_tidal_mood_categories() -> Vec<Value> {
    DEFAULT_TIDAL_MOOD_CATEGORIES
        .iter()
        .map(|(slug, title)| {
            json!({
                "slug": slug,
                "title": title,
                "icon": null,
                "imageId": null,
                "thumbnail": null,
            })
        })
        .collect()
}

fn mood_probe_from_modules(modules: &[TidalHomeModule]) -> (bool, Option<String>) {
    let thumbnail = modules
        .first()
        .and_then(|module| module.items.first())
        .and_then(|item| item.artwork_url.clone());
    (modules.is_empty(), thumbnail)
}

async fn run_mood_thumbnail_probe_refresh(
    mood_cache: TidalMoodCategoriesCache,
    page_modules_cache: TidalPageModulesCache,
    probe_client: TidalClient,
    country_code: String,
    categories: Vec<Value>,
    slugs: Vec<String>,
) {
    let started_at = Instant::now();
    let slugs_total = slugs.len();
    let fetches = slugs.into_iter().map(|slug| {
        let client = probe_client.clone();
        let cache = page_modules_cache.clone();
        let country = country_code.clone();
        async move {
            let page_path = format!("pages/{slug}");
            let cache_key = tidal_page_modules_cache_key(&country, &page_path);
            if let Some(modules) = get_cached_tidal_page_modules(&cache, &cache_key) {
                return MoodProbeOutcome::Modules {
                    slug,
                    modules,
                    cache_hit: true,
                };
            }
            match tokio::time::timeout(
                MOOD_THUMBNAIL_PROBE_TIMEOUT,
                client.get_page_modules(&page_path),
            )
            .await
            {
                Ok(Ok(modules)) => {
                    put_cached_tidal_page_modules(&cache, cache_key, modules.clone());
                    MoodProbeOutcome::Modules {
                        slug,
                        modules,
                        cache_hit: false,
                    }
                }
                Ok(Err(err)) => {
                    tracing::debug!(route = "tidal_moods_probe", slug, error = %err, "Mood probe failed");
                    MoodProbeOutcome::Error { slug }
                }
                Err(_) => MoodProbeOutcome::Timeout { slug },
            }
        }
    });

    let outcomes = {
        use futures::StreamExt;

        futures::stream::iter(fetches)
            .buffer_unordered(MOOD_THUMBNAIL_FETCH_CONCURRENCY)
            .collect::<Vec<MoodProbeOutcome>>()
            .await
    };

    let mut probe: HashMap<String, (bool, Option<String>)> = HashMap::new();
    let mut cache_hits = 0usize;
    let mut fetched = 0usize;
    let mut timeouts = 0usize;
    let mut errors = 0usize;
    for outcome in outcomes {
        match outcome {
            MoodProbeOutcome::Modules {
                slug,
                modules,
                cache_hit,
            } => {
                if cache_hit {
                    cache_hits += 1;
                } else {
                    fetched += 1;
                }
                probe.insert(slug, mood_probe_from_modules(&modules));
            }
            MoodProbeOutcome::Timeout { slug } => {
                timeouts += 1;
                tracing::debug!(route = "tidal_moods_probe", slug, "Mood probe timed out");
            }
            MoodProbeOutcome::Error { slug } => {
                errors += 1;
                tracing::debug!(route = "tidal_moods_probe", slug, "Mood probe error");
            }
        }
    }

    if mood_probe_failed_without_results(fetched, cache_hits, timeouts, errors) {
        note_tidal_moods_probe_failure(tidal_moods_probe_failure_cooldown(), Instant::now());
        tracing::warn!(
            route = "tidal_moods_probe",
            elapsed_ms = started_at.elapsed().as_millis(),
            slugs_total,
            timeouts,
            errors,
            timeout_ms = MOOD_THUMBNAIL_PROBE_TIMEOUT.as_millis(),
            "All mood probes failed, keeping mood cache to avoid retry storm"
        );
        return;
    }

    clear_tidal_moods_probe_failure(tidal_moods_probe_failure_cooldown());
    let refreshed_categories = apply_mood_probe_results(categories, &probe);
    put_cached_tidal_mood_categories(&mood_cache, refreshed_categories.clone());
    tracing::info!(
        route = "tidal_moods_probe",
        elapsed_ms = started_at.elapsed().as_millis(),
        slugs_total,
        cache_hits,
        fetched,
        timeouts,
        errors,
        category_count = refreshed_categories.len(),
        timeout_ms = MOOD_THUMBNAIL_PROBE_TIMEOUT.as_millis(),
        "TIDAL moods thumbnail/background probe complete"
    );
}

fn get_cached_tidal_mood_categories(cache: &TidalMoodCategoriesCache) -> Option<Vec<Value>> {
    let mut guard = cache.lock().unwrap();
    if let Some((stored_at, ttl, cached)) = guard.as_ref()
        && stored_at.elapsed() < *ttl
    {
        return Some(cached.clone());
    }
    *guard = None;
    None
}

fn put_cached_tidal_mood_categories(cache: &TidalMoodCategoriesCache, categories: Vec<Value>) {
    put_cached_tidal_mood_categories_with_ttl(cache, categories, TIDAL_HOME_CACHE_TTL);
}

fn put_cached_tidal_mood_categories_with_ttl(
    cache: &TidalMoodCategoriesCache,
    categories: Vec<Value>,
    ttl: Duration,
) {
    let mut guard = cache.lock().unwrap();
    *guard = Some((Instant::now(), ttl, categories));
}

/// Walks `rows[].modules[]` and pulls items from any module of `type ==
/// "PAGE_LINKS"`. TIDAL uses this shape for nav-style content (moods,
/// activity categories, sub-section links) -- the items are link metadata,
/// not playable rows.
fn extract_page_links(payload: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(rows) = payload.get("rows").and_then(|v| v.as_array()) else {
        return out;
    };
    for row in rows {
        let Some(modules) = row.get("modules").and_then(|v| v.as_array()) else {
            continue;
        };
        for module in modules {
            let kind = module.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if kind != "PAGE_LINKS" {
                continue;
            }
            let Some(items) = module
                .get("pagedList")
                .and_then(|p| p.get("items"))
                .and_then(|v| v.as_array())
            else {
                continue;
            };
            for item in items {
                let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let api_path = item.get("apiPath").and_then(|v| v.as_str()).unwrap_or("");
                if title.is_empty() || api_path.is_empty() {
                    continue;
                }
                let slug = api_path.strip_prefix("pages/").unwrap_or(api_path);
                out.push(json!({
                    "slug": slug,
                    "title": title,
                    "icon": item.get("icon").and_then(|v| v.as_str()),
                    "imageId": item.get("imageId").and_then(|v| v.as_str()),
                }));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, LazyLock, Mutex};

    static TIDAL_MOODS_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn reset_tidal_moods_refresh_guard_for_tests() {
        TIDAL_MOODS_REFRESH_IN_FLIGHT.store(false, Ordering::Release);
    }

    fn cooldown_state(value: Option<Instant>) -> Mutex<Option<Instant>> {
        Mutex::new(value)
    }

    #[test]
    fn merge_default_mood_thumbnails_keeps_all_and_attaches_by_slug() {
        let categories = default_tidal_mood_categories();
        let total = categories.len();
        let mut thumbnails = HashMap::new();
        thumbnails.insert(
            "mood_party".to_string(),
            "https://resources.tidal.com/images/abc/640x640.jpg".to_string(),
        );

        let merged = merge_default_mood_thumbnails(categories, &thumbnails);

        // Every default survives, even the ones with no probed thumbnail.
        assert_eq!(merged.len(), total);
        let party = merged
            .iter()
            .find(|c| c["slug"] == "mood_party")
            .expect("party kept");
        assert_eq!(
            party["thumbnail"],
            "https://resources.tidal.com/images/abc/640x640.jpg"
        );
        let workout = merged
            .iter()
            .find(|c| c["slug"] == "mood_workout")
            .expect("workout kept");
        assert!(
            workout["thumbnail"].is_null(),
            "unprobed default kept with a null thumbnail rather than dropped"
        );
    }

    #[test]
    fn resolve_page_path_allows_documented_tidal_editorial_sections() {
        let cases = [
            ("explore", None, "pages/explore"),
            ("hires", None, "pages/hires"),
            ("videos", None, "pages/videos"),
            ("genres", None, "pages/genre_page"),
            ("genre-page", None, "pages/genre_page"),
            ("genre-page-local", None, "pages/genre_page_local"),
            ("new-releases", None, "pages/whatsnew"),
            ("whatsnew", None, "pages/whatsnew"),
            ("mood", Some("abc"), "pages/mood/abc"),
            ("genre", Some("rock"), "pages/genre/rock"),
            ("genre", Some("  hip-hop  "), "pages/genre/hip-hop"),
        ];

        for (section, id, expected) in cases {
            assert_eq!(resolve_page_path(section, id).unwrap(), expected);
        }
    }

    #[test]
    fn resolve_page_path_rejects_unlisted_tidal_editorial_sections() {
        for (section, id) in [
            ("", None),
            ("home", None),
            ("charts", None),
            ("genres", Some("rock")),
            ("new-releases", Some("albums")),
            ("explore/deeper", None),
            ("mood", Some("")),
            ("mood", Some("../home")),
            ("mood", Some("party?debug=true")),
            ("mood", Some("party&limit=200")),
            ("mood", Some("party mode")),
        ] {
            assert_eq!(resolve_page_path(section, id), Err(StatusCode::NOT_FOUND));
        }
    }

    #[test]
    fn tidal_module_item_limit_is_bounded_for_show_more_requests() {
        assert_eq!(normalize_tidal_module_items_limit(None), 50);
        assert_eq!(normalize_tidal_module_items_limit(Some("bad")), 50);
        assert_eq!(normalize_tidal_module_items_limit(Some("0")), 1);
        assert_eq!(normalize_tidal_module_items_limit(Some("12")), 12);
        assert_eq!(normalize_tidal_module_items_limit(Some("9999")), 200);
    }

    #[test]
    fn tidal_mix_id_normalization_allows_known_safe_id_shapes() {
        assert_eq!(normalize_tidal_mix_id("abc123").unwrap(), "abc123");
        assert_eq!(
            normalize_tidal_mix_id("  daily_discovery-01  ").unwrap(),
            "daily_discovery-01"
        );
    }

    #[test]
    fn tidal_mix_id_normalization_rejects_url_control_characters() {
        for id in [
            "",
            "../home",
            "mix/tracks",
            "mix?limit=1",
            "mix&countryCode=US",
            "mix#fragment",
            "mix track",
        ] {
            assert_eq!(normalize_tidal_mix_id(id), Err(StatusCode::BAD_REQUEST));
        }
    }

    #[test]
    fn tidal_mood_category_cache_returns_fresh_entries_and_expires_stale_entries() {
        let cache = Arc::new(Mutex::new(None));
        let categories = vec![json!({ "slug": "mood_party", "title": "Party" })];

        put_cached_tidal_mood_categories(&cache, categories.clone());

        assert_eq!(
            get_cached_tidal_mood_categories(&cache),
            Some(categories.clone())
        );

        {
            let mut guard = cache.lock().unwrap();
            *guard = Some((
                Instant::now() - Duration::from_secs(6 * 60 * 60 + 1),
                TIDAL_HOME_CACHE_TTL,
                categories,
            ));
        }

        assert!(get_cached_tidal_mood_categories(&cache).is_none());
        assert!(cache.lock().unwrap().is_none());
    }

    #[test]
    fn tidal_moods_refresh_guard_allows_one_refresh_at_a_time() {
        let _guard = TIDAL_MOODS_TEST_LOCK.lock().expect("moods test lock");
        reset_tidal_moods_refresh_guard_for_tests();

        let guard = try_begin_tidal_moods_refresh().expect("first refresh starts");
        assert!(try_begin_tidal_moods_refresh().is_none());

        drop(guard);
        assert!(try_begin_tidal_moods_refresh().is_some());
        reset_tidal_moods_refresh_guard_for_tests();
    }

    #[test]
    fn failed_mood_probe_without_results_keeps_existing_cache() {
        assert!(mood_probe_failed_without_results(0, 0, 1, 0));
        assert!(mood_probe_failed_without_results(0, 0, 0, 1));
        assert!(!mood_probe_failed_without_results(1, 0, 1, 0));
        assert!(!mood_probe_failed_without_results(0, 1, 0, 1));
        assert!(!mood_probe_failed_without_results(0, 0, 0, 0));
    }

    #[test]
    fn mood_probe_failure_cooldown_reports_remaining_time() {
        let now = Instant::now();
        let cooldown = cooldown_state(Some(now + Duration::from_secs(60)));

        let remaining =
            tidal_moods_probe_cooldown_remaining(&cooldown, now).expect("cooldown active");

        assert_eq!(remaining, Duration::from_secs(60));
    }

    #[test]
    fn expired_mood_probe_failure_cooldown_clears_state() {
        let now = Instant::now();
        let cooldown = cooldown_state(Some(now));

        assert_eq!(tidal_moods_probe_cooldown_remaining(&cooldown, now), None);
        assert_eq!(*cooldown.lock().expect("lock cooldown"), None);
    }

    #[test]
    fn note_and_clear_mood_probe_failure_cooldown() {
        let now = Instant::now();
        let cooldown = cooldown_state(None);

        note_tidal_moods_probe_failure(&cooldown, now);
        assert!(tidal_moods_probe_cooldown_remaining(&cooldown, now).is_some());

        clear_tidal_moods_probe_failure(&cooldown);
        assert_eq!(tidal_moods_probe_cooldown_remaining(&cooldown, now), None);
    }

    #[test]
    fn fallback_mood_cache_uses_short_retry_ttl() {
        let cache = Arc::new(Mutex::new(None));
        let categories = vec![json!({ "slug": "mood_party", "title": "Party" })];

        put_cached_tidal_mood_categories_with_ttl(
            &cache,
            categories,
            TIDAL_MOODS_FALLBACK_CACHE_TTL,
        );

        {
            let mut guard = cache.lock().unwrap();
            let (_, ttl, cached) = guard.as_ref().expect("fallback cached").clone();
            *guard = Some((Instant::now() - ttl - Duration::from_millis(1), ttl, cached));
        }

        assert!(get_cached_tidal_mood_categories(&cache).is_none());
    }

    #[test]
    fn tidal_page_modules_cache_returns_fresh_entries_and_expires_stale_entries() {
        let cache = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let key = tidal_page_modules_cache_key("AU", "pages/m_happy");
        let modules = vec![TidalHomeModule {
            id: "happy".to_string(),
            title: "Happy".to_string(),
            kind: "PLAYLIST_LIST".to_string(),
            more_path: None,
            items: Vec::new(),
        }];

        put_cached_tidal_page_modules(&cache, key.clone(), modules.clone());

        let cached = get_cached_tidal_page_modules(&cache, &key).expect("fresh cache hit");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].title, "Happy");

        {
            let mut guard = cache.lock().unwrap();
            guard.insert(
                key.clone(),
                (
                    Instant::now() - Duration::from_secs(6 * 60 * 60 + 1),
                    modules,
                ),
            );
        }

        assert!(get_cached_tidal_page_modules(&cache, &key).is_none());
        assert!(!cache.lock().unwrap().contains_key(&key));
    }

    #[test]
    fn tidal_page_modules_cache_evicts_stale_keys_on_insert() {
        let cache = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let stale_key = tidal_page_modules_cache_key("AU", "pages/old");
        let fresh_key = tidal_page_modules_cache_key("AU", "pages/new");
        let modules = vec![TidalHomeModule {
            id: "x".to_string(),
            title: "X".to_string(),
            kind: "PLAYLIST_LIST".to_string(),
            more_path: None,
            items: Vec::new(),
        }];

        // Seed a stale entry that nobody will ever read again.
        cache.lock().unwrap().insert(
            stale_key.clone(),
            (
                Instant::now() - Duration::from_secs(6 * 60 * 60 + 1),
                modules.clone(),
            ),
        );

        // Inserting an unrelated fresh key must sweep the stale one so the map
        // can't grow without bound from never-revisited keys.
        put_cached_tidal_page_modules(&cache, fresh_key.clone(), modules);

        let guard = cache.lock().unwrap();
        assert!(!guard.contains_key(&stale_key), "stale key should be swept");
        assert!(guard.contains_key(&fresh_key));
        assert_eq!(guard.len(), 1);
    }

    #[test]
    fn mood_probe_results_add_thumbnails_and_filter_empty_categories() {
        let categories = vec![
            json!({ "slug": "mood_party", "title": "Party" }),
            json!({ "slug": "mood_empty", "title": "Empty" }),
        ];
        let mut probe = HashMap::new();
        probe.insert(
            "mood_party".to_string(),
            (false, Some("https://img.example/party.jpg".to_string())),
        );
        probe.insert("mood_empty".to_string(), (true, None));

        let merged = apply_mood_probe_results(categories, &probe);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0]["slug"], "mood_party");
        assert_eq!(merged[0]["thumbnail"], "https://img.example/party.jpg");
    }

    #[test]
    fn cached_mood_probes_use_page_module_cache_and_leave_misses_pending() {
        let cache = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let cached_slug = "mood_party";
        let cached_key = tidal_page_modules_cache_key("AU", &format!("pages/{cached_slug}"));
        let modules = vec![TidalHomeModule {
            id: "module_1".to_string(),
            title: "Party Picks".to_string(),
            kind: "PLAYLIST_LIST".to_string(),
            more_path: None,
            items: vec![crate::services::tidal::client::TidalHomeItem {
                kind: "playlist".to_string(),
                id: "abc".to_string(),
                title: "Party".to_string(),
                artist_name: None,
                artwork_url: Some("https://img.example/cached.jpg".to_string()),
                duration: None,
                artist_id: None,
                album_id: None,
                album_title: None,
                creator_name: Some("NOOR".to_string()),
            }],
        }];
        put_cached_tidal_page_modules(&cache, cached_key, modules);
        let categories = vec![
            json!({ "slug": cached_slug, "title": "Party" }),
            json!({ "slug": "mood_uncached", "title": "Uncached" }),
        ];

        let (merged, pending, cache_hits) =
            apply_cached_mood_category_probes(categories, "AU", &cache);

        assert_eq!(cache_hits, 1);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], "mood_uncached");
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["thumbnail"], "https://img.example/cached.jpg");
    }
}

/// Drill-down for one mood / activity category. `slug` is the path segment
/// returned by `/api/tidal/moods` (e.g. `mood_party`) and is proxied to
/// `pages/{slug}` on TIDAL. Slug pattern is restricted to lowercase
/// alphanumeric + underscores so callers can't escape the `pages/` namespace.
pub(super) async fn get_tidal_mood_page(
    State(state): State<SharedState>,
    Path(slug): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    if slug.is_empty()
        || slug.len() > 64
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let wire_path = format!("pages/{}", slug);
    fetch_page_modules(state, wire_path).await
}

// Shared TIDAL session loader -- mirrors the inline block other handlers use.
async fn load_tidal_session(
    state: &SharedState,
) -> (
    Option<crate::services::tidal::auth::TidalTokens>,
    reqwest::Client,
    reqwest::Client,
) {
    let in_memory = {
        let s = state.read().await;
        (
            s.tidal_tokens.clone(),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
        )
    };
    match in_memory.0 {
        Some(t) => (Some(t), in_memory.1, in_memory.2),
        None => {
            let persisted = super::load_persisted_tidal_tokens(state)
                .await
                .ok()
                .flatten();
            (persisted, in_memory.1, in_memory.2)
        }
    }
}

// ─── Last.fm scrobble auth (server-side web-auth flow) ──────────────────────
