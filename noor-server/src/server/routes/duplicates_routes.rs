use crate::library::duplicates as dup;
use crate::services::tidal::mutations as tidal_mutations;
use crate::{AppEvent, SharedState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{error, warn};

#[derive(Debug, Deserialize)]
pub(super) struct DuplicateListParams {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResolveGroupRequest {
    preferred_track_id: i64,
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
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

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
    // Get TIDAL tokens for unfavorite calls.
    let (tokens, http) = {
        let s = state.read().await;
        let tokens = s.tidal_tokens.clone();
        (tokens, s.http_client.clone())
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
        for tidal_id in &result.tidal_ids_to_unfavorite {
            if let Err(e) = tidal_mutations::remove_favorite_track(
                &http,
                &t.access_token,
                &t.user_id,
                *tidal_id,
                &t.country_code,
            )
            .await
            {
                // If it looks like a session expiry, try to refresh and retry once.
                if (e.to_string().contains("401")
                    || e.to_string().to_lowercase().contains("unauthorized"))
                    && let Ok(refreshed) = super::recover_tidal_session(&state, &http, &t).await
                {
                    if let Err(e2) = tidal_mutations::remove_favorite_track(
                        &http,
                        &refreshed.access_token,
                        &refreshed.user_id,
                        *tidal_id,
                        &refreshed.country_code,
                    )
                    .await
                    {
                        error!(
                            "Failed to unfavorite TIDAL track {tidal_id} after session refresh: {e2}"
                        );
                    }
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
    let s = state.read().await;
    s.db.with_conn(|conn| dup::dismiss_group(conn, group_id))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": "dismissed" })))
}
