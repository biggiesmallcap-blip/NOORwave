use crate::SharedState;
use crate::db::queries;
use crate::services::tidal::client::TidalClient;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};

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
            && stored_at.elapsed() < Duration::from_secs(6 * 60 * 60)
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
            && stored_at.elapsed() < Duration::from_secs(6 * 60 * 60)
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
    let (tokens, http_client, tidal_http_client) = {
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
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2)
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
    let modules = match client.get_home_modules().await {
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
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
        .min(200);

    let (tokens, http_client, tidal_http_client) = {
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
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2)
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

    let modules = match client.get_home_modules().await {
        Ok(m) => m,
        Err(e) if super::error_looks_like_auth(&e) => {
            let refreshed = super::recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client.clone(),
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_home_modules().await.map_err(|e| {
                tracing::warn!("get_home_modules failed after refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("get_home_modules failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

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
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let wire_path = resolve_page_path(&section, None)?;
    let debug_raw = query.get("debug").map(String::as_str) == Some("raw");
    fetch_page_modules(state, wire_path, debug_raw).await
}

pub(super) async fn get_tidal_page_modules_with_id(
    State(state): State<SharedState>,
    Path((section, id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let wire_path = resolve_page_path(&section, Some(id.as_str()))?;
    let debug_raw = query.get("debug").map(String::as_str) == Some("raw");
    fetch_page_modules(state, wire_path, debug_raw).await
}

// Whitelist + wire-path normalization. Returns 404 for anything not on the
// approved list so callers can't probe arbitrary TIDAL endpoints.
fn resolve_page_path(section: &str, id: Option<&str>) -> Result<String, StatusCode> {
    let section = section.trim_matches('/');
    let allowed_top = matches!(
        section,
        "charts" | "moods" | "genres" | "new-releases" | "new_releases"
    );
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
    debug_raw: bool,
) -> Result<Json<Value>, StatusCode> {
    let (tokens, http_client, tidal_http_client) = {
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
                let persisted = super::load_persisted_tidal_tokens(&state)
                    .await
                    .ok()
                    .flatten();
                (persisted, in_memory.1, in_memory.2)
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
    if debug_raw {
        let raw = client.get_page_raw(&page_path).await.map_err(|e| {
            tracing::warn!("TIDAL get_page_raw({page_path}) failed: {e}");
            StatusCode::BAD_GATEWAY
        })?;
        return Ok(Json(
            json!({ "raw": raw, "source": "tidal", "page": page_path }),
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
    Ok(Json(
        json!({ "modules": modules, "source": "tidal", "page": page_path }),
    ))
}

// ─── Last.fm scrobble auth (server-side web-auth flow) ──────────────────────
