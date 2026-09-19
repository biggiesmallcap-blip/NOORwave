use crate::library::duplicates as dup;
use crate::services::tidal::client::TidalClient;
use crate::{AppEvent, SharedState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{error, warn};

const DUPLICATE_LIST_LIMIT_MAX: i64 = 100;

#[derive(Debug, Deserialize)]
pub(super) struct DuplicateListParams {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResolveGroupRequest {
    preferred_track_id: i64,
}

fn duplicate_list_bounds(params: DuplicateListParams) -> Result<(i64, i64), StatusCode> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    if !(1..=DUPLICATE_LIST_LIMIT_MAX).contains(&limit) || offset < 0 {
        return Err(StatusCode::BAD_REQUEST);
    }

    Ok((limit, offset))
}

fn require_positive_id(id: i64) -> Result<(), StatusCode> {
    if id <= 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

/// Scan the library for duplicates. Runs synchronously (usually <5s for 32k tracks).
pub(super) async fn scan_duplicates(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let stats = {
        let s = state.read().await;
        s.db.with_conn(dup::scan)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(json!({
        "groups_found": stats.groups_found,
        "tracks_affected": stats.tracks_affected,
        "isrc_matches": stats.isrc_matches,
        "title_matches": stats.title_matches,
    })))
}

/// List pending duplicate groups with full track data (paginated).
pub(super) async fn get_duplicates(
    State(state): State<SharedState>,
    Query(params): Query<DuplicateListParams>,
) -> Result<Json<Value>, StatusCode> {
    let (limit, offset) = duplicate_list_bounds(params)?;

    let s = state.read().await;
    s.db.with_conn(|conn| {
        let total = dup::count_pending_groups(conn)?;
        let groups = dup::load_groups(conn, limit, offset)?;
        Ok(Json(json!({ "groups": groups, "total": total })))
    })
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Keep `preferred_track_id`, delete the rest from DB, return TIDAL IDs to unfavorite.
pub(super) async fn resolve_duplicate_group(
    State(state): State<SharedState>,
    Path(group_id): Path<i64>,
    Json(payload): Json<ResolveGroupRequest>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_id(group_id)?;
    require_positive_id(payload.preferred_track_id)?;

    // Get TIDAL tokens for unfavorite calls.
    let (tokens, tidal_http_client) = {
        let s = state.read().await;
        let tokens = s.tidal_tokens.clone();
        (tokens, s.tidal_http_client.clone())
    };

    let result = {
        let s = state.read().await;
        s.db.with_conn(|conn| dup::resolve_group(conn, group_id, payload.preferred_track_id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    // Broadcast queue / playback / library events based on the reconcile outcome.
    {
        let s = state.read().await;
        if result.reconcile.queue_changed {
            let _ = s.event_tx.send(AppEvent::QueueUpdated);
        }
        if result.reconcile.current_changed {
            let _ = s.event_tx.send(AppEvent::PlaybackStateChanged);
        }
        let _ = s.event_tx.send(AppEvent::LibrarySynced);
    }

    // Best-effort unfavorite on TIDAL with session refresh retry.
    if let Some(t) = tokens.clone() {
        let mut client = TidalClient::with_http(
            tidal_http_client,
            t.access_token.clone(),
            t.country_code.clone(),
        );
        let mut user_id = t.user_id.clone();
        for tidal_id in &result.tidal_ids_to_unfavorite {
            if let Err(e) = client.remove_favorite_track(&user_id, *tidal_id).await {
                // If it looks like a session expiry, try to refresh and retry once.
                if super::error_looks_like_auth(&e)
                    && let Ok((retry_client, refreshed)) =
                        super::recover_tidal_client_with_tokens(&state, &t).await
                {
                    if let Err(e2) = retry_client
                        .remove_favorite_track(&refreshed.user_id, *tidal_id)
                        .await
                    {
                        error!(
                            "Failed to unfavorite TIDAL track {tidal_id} after session refresh: {e2}"
                        );
                    }
                    client = retry_client;
                    user_id = refreshed.user_id;
                    continue;
                }
                warn!("Failed to unfavorite TIDAL track {tidal_id}: {e}");
            }
        }
    }

    Ok(Json(json!({
        "removed": result.removed_track_ids,
        "unfavorited_tidal": result.tidal_ids_to_unfavorite,
    })))
}

/// Dismiss a duplicate group without deleting anything.
pub(super) async fn dismiss_duplicate_group(
    State(state): State<SharedState>,
    Path(group_id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_id(group_id)?;

    let s = state.read().await;
    s.db.with_conn(|conn| dup::dismiss_group(conn, group_id))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": "dismissed" })))
}
