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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type TidalMoodCategoriesCache = Arc<Mutex<Option<(Instant, Vec<Value>)>>>;
type TidalPageModulesCache = Arc<Mutex<HashMap<String, (Instant, Vec<TidalHomeModule>)>>>;

const TIDAL_HOME_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const MOOD_THUMBNAIL_FETCH_CONCURRENCY: usize = 4;
const MOOD_THUMBNAIL_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const TIDAL_HOME_MODULES_PAGE_PATH: &str = "pages/home";
const ROUTE_TIMING_INFO_THRESHOLD_MS: u128 = 500;

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
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
        .min(200);

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
    let items = client.get_mix_tracks(&mix_id).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

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
    // `charts` and `genres`/`new_releases` slugs aren't valid TIDAL endpoints
    // (verified live: all 404 with subStatus 2001 "Not found"). `moods` now
    // has its own dedicated route at /api/tidal/moods + /api/tidal/mood-page/{slug}
    // because its modules are PAGE_LINKS, not the usual TRACK_LIST/etc shape.
    // Empty top-level whitelist for now — this generic route is kept for
    // future slugs (pages/explore, pages/hires, pages/videos, etc).
    let allowed_top = matches!(section, "");
    let allowed_with_id = matches!(section, "mood" | "genre");
    let valid = (id.is_none() && allowed_top) || (id.is_some() && allowed_with_id);
    if !valid {
        return Err(StatusCode::NOT_FOUND);
    }
    // TIDAL uses `new_releases` on the wire; normalize the dash form callers
    // may use.
    let wire_section = if section == "new-releases" {
        "new_releases"
    } else {
        section
    };
    Ok(match id {
        Some(id) => format!("pages/{}/{}", wire_section, id),
        None => format!("pages/{}", wire_section),
    })
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
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let raw = match client.get_page_raw("pages/moods").await {
        Ok(r) => r,
        Err(e) if super::error_looks_like_auth(&e) => {
            let refreshed = super::recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client.clone(),
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_page_raw("pages/moods").await.map_err(|e| {
                tracing::warn!("TIDAL get_tidal_moods failed after refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_tidal_moods failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    let categories = extract_page_links(&raw);
    let (response_categories, pending_probe_slugs, cached_probe_hits) =
        apply_cached_mood_category_probes(categories, &tokens.country_code, &page_modules_cache);
    let pending_probe_count = pending_probe_slugs.len();

    put_cached_tidal_mood_categories(&mood_cache, response_categories.clone());

    if !pending_probe_slugs.is_empty() {
        let mood_cache_bg = mood_cache.clone();
        let page_modules_cache_bg = page_modules_cache.clone();
        let probe_seed_categories = response_categories.clone();
        let probe_client = TidalClient::with_http(
            tidal_http_client,
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );
        let country_code = tokens.country_code.clone();
        tokio::spawn(async move {
            run_mood_thumbnail_probe_refresh(
                mood_cache_bg,
                page_modules_cache_bg,
                probe_client,
                country_code,
                probe_seed_categories,
                pending_probe_slugs,
            )
            .await;
        });
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
        json!({ "categories": response_categories, "source": "tidal" }),
    ))
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

    if fetched == 0 && cache_hits == 0 && (timeouts > 0 || errors > 0) {
        // All background probes failed, so do not lock in a 6h stale mood
        // cache without thumbnails. Clearing forces a fresh retry on next call.
        let mut guard = mood_cache.lock().unwrap();
        *guard = None;
        tracing::warn!(
            route = "tidal_moods_probe",
            elapsed_ms = started_at.elapsed().as_millis(),
            slugs_total,
            timeouts,
            errors,
            timeout_ms = MOOD_THUMBNAIL_PROBE_TIMEOUT.as_millis(),
            "All mood probes failed, clearing mood cache for retry"
        );
        return;
    }

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
    if let Some((stored_at, cached)) = guard.as_ref()
        && stored_at.elapsed() < TIDAL_HOME_CACHE_TTL
    {
        return Some(cached.clone());
    }
    *guard = None;
    None
}

fn put_cached_tidal_mood_categories(cache: &TidalMoodCategoriesCache, categories: Vec<Value>) {
    let mut guard = cache.lock().unwrap();
    *guard = Some((Instant::now(), categories));
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
    use std::sync::{Arc, Mutex};

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
                categories,
            ));
        }

        assert!(get_cached_tidal_mood_categories(&cache).is_none());
        assert!(cache.lock().unwrap().is_none());
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
