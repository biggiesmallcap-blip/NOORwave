use crate::services::spotify;
use crate::{AppEvent, SharedState};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

pub(super) async fn start_musicbrainz_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;

    let (http_client, event_tx, running) = {
        let g = state.read().await;
        (
            g.http_client.clone(),
            g.event_tx.clone(),
            g.musicbrainz_enrich_running.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        return Ok(Json(json!({ "status": "already_running" })));
    }

    let total: usize = {
        let g = state.read().await;
        g.db.with_conn(crate::services::musicbrainz::count_unenriched_tracks)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    if total == 0 {
        return Ok(Json(
            json!({ "status": "already_complete", "remaining": 0 }),
        ));
    }

    running.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let result = crate::services::musicbrainz::run_enrichment(
            state,
            http_client,
            move |progress| {
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "musicbrainz".to_string(),
                    progress: progress.processed as f32 / progress.total.max(1) as f32,
                });
            },
            1,
        )
        .await;
        running.store(false, Ordering::SeqCst);
        match result {
            Ok(_) => {
                let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
                let _ = event_tx.send(AppEvent::LibrarySynced);
            }
            Err(err) => {
                warn!("MusicBrainz enrichment error: {err:?}");
            }
        }
    });

    Ok(Json(json!({ "status": "started", "remaining": total })))
}

pub(super) async fn get_musicbrainz_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let (total, checked, enriched) = state
        .db
        .with_conn(|conn| {
            let total: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
            let checked: i64 =
                conn.query_row("SELECT COUNT(*) FROM musicbrainz_checked", [], |r| r.get(0))?;
            let enriched: i64 = conn.query_row(
                "SELECT COUNT(DISTINCT track_id) FROM track_genres WHERE source = 'musicbrainz'",
                [],
                |r| r.get(0),
            )?;
            Ok((total, checked, enriched))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "total_tracks": total,
        "checked_tracks": checked,
        "enriched_tracks": enriched,
        "remaining": (total - checked).max(0),
        "complete": checked >= total
    })))
}

pub(super) async fn get_musicbrainz_portable_snapshot(
    State(_state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let snapshot =
        crate::services::musicbrainz::read_portable_snapshot_status().map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "NOOR couldn't read the portable MusicBrainz snapshot status.",
                    "details": error.to_string(),
                })),
            )
        })?;

    Ok(Json(json!({
        "exists": snapshot.exists,
        "path": snapshot.path,
        "generated_at": snapshot.generated_at,
        "checked_rows": snapshot.checked_rows,
        "genre_rows": snapshot.genre_rows,
        "lastfm_checked_rows": snapshot.lastfm_checked_rows,
        "context_tag_rows": snapshot.context_tag_rows,
    })))
}

pub(super) async fn export_musicbrainz_portable_snapshot(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(crate::services::musicbrainz::export_portable_snapshot)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "message": "NOOR couldn't write the portable MusicBrainz snapshot.",
                        "details": error.to_string(),
                    })),
                )
            })?
            .status
    };

    Ok(Json(json!({
        "status": "exported",
        "snapshot": {
            "exists": snapshot.exists,
            "path": snapshot.path,
            "generated_at": snapshot.generated_at,
            "checked_rows": snapshot.checked_rows,
            "genre_rows": snapshot.genre_rows,
            "lastfm_checked_rows": snapshot.lastfm_checked_rows,
            "context_tag_rows": snapshot.context_tag_rows,
        }
    })))
}

pub(super) async fn import_musicbrainz_portable_snapshot(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let imported = {
        let state = state.read().await;
        state
            .db
            .with_conn(crate::services::musicbrainz::import_portable_snapshot)
            .map_err(|error| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "message": "NOOR couldn't import the portable MusicBrainz snapshot.",
                        "details": error.to_string(),
                    })),
                )
            })?
    };

    {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::MusicBrainzEnriched);
        let _ = state.event_tx.send(AppEvent::LibrarySynced);
    }

    Ok(Json(json!({
        "status": "imported",
        "checked_inserted": imported.checked_inserted,
        "checked_skipped": imported.checked_skipped,
        "lastfm_checked_inserted": imported.lastfm_checked_inserted,
        "lastfm_checked_skipped": imported.lastfm_checked_skipped,
        "genre_inserted": imported.genre_inserted,
        "track_skipped": imported.track_skipped,
        "genre_skipped": imported.genre_skipped,
        "context_tag_inserted": imported.context_tag_inserted,
        "context_tag_skipped": imported.context_tag_skipped,
        "snapshot": {
            "exists": imported.status.exists,
            "path": imported.status.path,
            "generated_at": imported.status.generated_at,
            "checked_rows": imported.status.checked_rows,
            "genre_rows": imported.status.genre_rows,
            "lastfm_checked_rows": imported.status.lastfm_checked_rows,
            "context_tag_rows": imported.status.context_tag_rows,
        }
    })))
}

// ── Duplicate detection ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct SpotifyConfigRequest {
    client_id: String,
    client_secret: String,
}

pub(super) async fn spotify_save_config(
    State(state): State<SharedState>,
    Json(payload): Json<SpotifyConfigRequest>,
) -> Result<Json<Value>, StatusCode> {
    let http = state.read().await.http_client.clone();
    let creds = spotify::auth::SpotifyCredentials {
        client_id: payload.client_id.trim().to_string(),
        client_secret: payload.client_secret.trim().to_string(),
    };

    if creds.client_id.is_empty() || creds.client_secret.is_empty() {
        return Ok(Json(json!({
            "status": "error",
            "message": "Client ID and Client Secret are both required."
        })));
    }

    // Verify the credentials work by fetching a token before saving.
    match spotify::auth::fetch_app_token(&http, &creds).await {
        Ok(tokens) => {
            let _ = state.read().await.db.with_conn(|conn| {
                spotify::auth::save_credentials(conn, &creds)?;
                Ok(())
            });
            {
                let mut s = state.write().await;
                s.spotify_tokens = Some(tokens);
            }
            Ok(Json(json!({"status": "ok"})))
        }
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Spotify rejected the credentials: {}", e)
        }))),
    }
}

pub(super) async fn spotify_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let configured = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            Ok(spotify::auth::load_credentials(conn)
                .ok()
                .flatten()
                .is_some())
        })
        .unwrap_or(false);
    Ok(Json(json!({"configured": configured})))
}

pub(super) async fn spotify_clear_config(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let _ = state.read().await.db.with_conn(|conn| {
        spotify::auth::clear_credentials(conn)?;
        Ok(())
    });
    {
        let mut s = state.write().await;
        s.spotify_tokens = None;
    }
    Ok(Json(json!({"status": "cleared"})))
}

pub(super) async fn start_spotify_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;

    let (http, event_tx, running, total_atom, processed_atom) = {
        let s = state.read().await;
        (
            s.http_client.clone(),
            s.event_tx.clone(),
            s.spotify_enrich_running.clone(),
            s.spotify_enrich_total.clone(),
            s.spotify_enrich_processed.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        let total = total_atom.load(Ordering::SeqCst);
        let processed = processed_atom.load(Ordering::SeqCst);
        return Ok(Json(json!({
            "status": "already_running",
            "total": total,
            "processed": processed
        })));
    }

    // Require credentials and prime a fresh token before enqueueing work.
    let creds = state
        .read()
        .await
        .db
        .with_conn(|conn| Ok(spotify::auth::load_credentials(conn).ok().flatten()))
        .unwrap_or(None);
    let Some(creds) = creds else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Spotify credentials not configured."
        })));
    };

    match spotify::auth::fetch_app_token(&http, &creds).await {
        Ok(tokens) => {
            let mut s = state.write().await;
            s.spotify_tokens = Some(tokens);
        }
        Err(e) => {
            return Ok(Json(json!({
                "status": "error",
                "message": format!("Failed to fetch Spotify token: {}", e)
            })));
        }
    }

    let total: usize = state.read().await.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (SELECT 1 FROM spotify_checked sc WHERE sc.track_id = t.id)",
            [], |r| r.get(0)
        )?)
    }).unwrap_or(0);

    if total == 0 {
        return Ok(Json(json!({"status": "already_complete"})));
    }

    running.store(true, Ordering::SeqCst);
    total_atom.store(total, Ordering::SeqCst);
    processed_atom.store(0, Ordering::SeqCst);

    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let total_atom_cb = total_atom.clone();
        let processed_atom_cb = processed_atom.clone();
        let result = crate::services::spotify::enrichment::run_enrichment(
            state,
            http,
            move |current, total| {
                processed_atom_cb.store(current, Ordering::SeqCst);
                if total > 0 {
                    total_atom_cb.store(total, Ordering::SeqCst);
                }
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "spotify".to_string(),
                    progress: current as f32 / total.max(1) as f32,
                });
            },
        )
        .await;

        running.store(false, Ordering::SeqCst);
        if result.is_ok() {
            let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
        }
    });

    Ok(Json(json!({"status": "started", "total": total})))
}

pub(super) async fn get_spotify_enrichment_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    let enriched: i64 =
        s.db.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(DISTINCT track_id) FROM track_genres WHERE source = 'spotify'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(0);
    let remaining: i64 = s.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (SELECT 1 FROM spotify_checked sc WHERE sc.track_id = t.id)",
            [],
            |r| r.get(0),
        )?)
    }).unwrap_or(0);
    let is_running = s.spotify_enrich_running.load(Ordering::SeqCst);
    let run_total = s.spotify_enrich_total.load(Ordering::SeqCst);
    let run_processed = s.spotify_enrich_processed.load(Ordering::SeqCst);

    Ok(Json(json!({
        "enriched_tracks": enriched,
        "remaining_tracks": remaining,
        "is_running": is_running,
        "run_total": run_total,
        "run_processed": run_processed,
    })))
}

// Wipes the spotify_checked table and any track_genres rows from source
// 'spotify'. Use after fixing rate-limiting bugs that may have wrongly
// stamped tracks as "checked" with no tags. Refuses while a run is active.
pub(super) async fn reset_spotify_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    if s.spotify_enrich_running.load(Ordering::SeqCst) {
        return Ok(Json(json!({
            "status": "error",
            "message": "Cannot reset while enrichment is running."
        })));
    }
    let result: anyhow::Result<(usize, usize)> = s.db.with_conn(|conn| {
        let checks = conn.execute("DELETE FROM spotify_checked", [])?;
        let tags = conn.execute("DELETE FROM track_genres WHERE source = 'spotify'", [])?;
        Ok((checks, tags))
    });
    match result {
        Ok((checks, tags)) => Ok(Json(json!({
            "status": "ok",
            "checks_cleared": checks,
            "tags_cleared": tags,
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Reset failed: {}", e),
        }))),
    }
}

// Manual cleanup: delete `tidal_stream` track rows that have no remaining
// references (no listen history, not favorited, not in any queue/playlist/etc).
// Safe to run any time. CASCADE FKs (track_neighbors, embeddings, transitions,
// audio_dsp_features, etc.) take care of trained-data cleanup automatically.
pub(super) async fn purge_orphan_tidal_stream_tracks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let result: anyhow::Result<usize> = s.db.with_conn(|conn| {
        // Filter against every non-CASCADE FK referencing tracks(id) so the
        // DELETE doesn't fail with a constraint violation.
        let deleted = conn.execute(
            "DELETE FROM tracks
             WHERE source = 'tidal_stream'
               AND is_favorite = 0
               AND id NOT IN (SELECT track_id FROM listen_history WHERE track_id IS NOT NULL)
               AND id NOT IN (SELECT track_id FROM queue WHERE track_id IS NOT NULL)
               AND id NOT IN (SELECT track_id FROM playlist_tracks)
               AND id NOT IN (SELECT current_track_id FROM playback_state WHERE current_track_id IS NOT NULL)
               AND id NOT IN (SELECT track_id FROM shuffle_state)
               AND id NOT IN (SELECT track_id FROM duplicate_group_members)
               AND id NOT IN (SELECT track_id FROM acrcloud_results)",
            [],
        )?;
        Ok(deleted)
    });
    match result {
        Ok(deleted) => {
            tracing::info!(deleted, "purge_orphan_tidal_stream_tracks");
            Ok(Json(json!({ "status": "ok", "deleted": deleted })))
        }
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Purge failed: {}", e),
        }))),
    }
}

// ── Last.fm Endpoints ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct LastFmConfigRequest {
    api_key: String,
    api_secret: Option<String>,
}

pub(super) async fn lastfm_save_config(
    State(state): State<SharedState>,
    Json(payload): Json<LastFmConfigRequest>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let mut api_key = payload.api_key.trim().to_string();
    if api_key.is_empty() {
        api_key = state
            .read()
            .await
            .db
            .with_conn(|conn| {
                Ok::<_, anyhow::Error>(
                    lastfm::auth::load_credentials(conn)?
                        .map(|creds| creds.api_key)
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
    }
    if api_key.is_empty() {
        return Ok(Json(json!({
            "status": "error",
            "message": "API key is required before saving a Last.fm secret."
        })));
    }

    // Verify the key works by hitting a free, parameterless endpoint.
    let http = state.read().await.http_client.clone();
    let probe = http
        .get("https://ws.audioscrobbler.com/2.0/")
        .query(&[
            ("method", "tag.getTopTags"),
            ("api_key", &api_key),
            ("format", "json"),
        ])
        .send()
        .await;
    match probe {
        Ok(resp) if resp.status().is_success() => {
            let body_text = resp.text().await.unwrap_or_default();
            if body_text.contains("\"error\"") {
                return Ok(Json(json!({
                    "status": "error",
                    "message": format!("Last.fm rejected the key: {}",
                        body_text.chars().take(200).collect::<String>())
                })));
            }
            let api_secret = payload.api_secret.as_deref().map(str::trim).unwrap_or("");
            let master_key = state.read().await.master_key.clone();
            let _ = state.read().await.db.with_conn(|conn| {
                let mut creds = lastfm::auth::load_credentials(conn)?.unwrap_or_default();
                creds.api_key = api_key.clone();
                lastfm::auth::save_credentials(conn, &creds)?;
                if !api_secret.is_empty() {
                    lastfm::auth::save_api_secret(conn, &master_key, api_secret)?;
                }
                Ok(())
            });
            Ok(Json(json!({"status": "ok"})))
        }
        Ok(resp) => Ok(Json(json!({
            "status": "error",
            "message": format!("Last.fm rejected the key: HTTP {}", resp.status())
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Could not reach Last.fm: {}", e)
        }))),
    }
}

pub(super) async fn lastfm_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let (creds, has_secret, pending, failed) = {
        let s = state.read().await;
        let result = s.db.with_conn(|conn| {
            let creds = lastfm::auth::load_credentials(conn).ok().flatten();
            let has_db_secret = lastfm::auth::has_api_secret(conn).unwrap_or(false);
            let (pending, failed) =
                crate::services::scrobbling::outbox_status(conn).unwrap_or((0, 0));
            Ok::<_, anyhow::Error>((
                creds,
                has_db_secret || s.lastfm_api_secret.is_some(),
                pending,
                failed,
            ))
        });
        result.unwrap_or((None, s.lastfm_api_secret.is_some(), 0, 0))
    };
    let enrichment = creds
        .as_ref()
        .map(|c| !c.api_key.is_empty())
        .unwrap_or(false);
    let user = creds.as_ref().and_then(|c| c.session_user.clone());
    let scrobbling = enrichment && has_secret && user.is_some();
    Ok(Json(json!({
        // Legacy field kept for backward compat with any existing caller of
        // /api/lastfm/status: equivalent to `enrichment`.
        "configured": enrichment,
        "enrichment": enrichment,
        "api_key_configured": enrichment,
        "api_secret_configured": has_secret,
        "scrobbling": scrobbling,
        "scrobble_available": has_secret,
        "recommendations": scrobbling,
        "pending_submissions": pending,
        "failed_submissions": failed,
        "user": user,
    })))
}

#[derive(Deserialize)]
pub(super) struct ListenBrainzConfigRequest {
    token: String,
}

pub(super) async fn listenbrainz_save_config(
    State(state): State<SharedState>,
    Json(payload): Json<ListenBrainzConfigRequest>,
) -> Result<Json<Value>, StatusCode> {
    let token = payload.token.trim().to_string();
    if token.is_empty() {
        return Ok(Json(json!({
            "status": "error",
            "message": "ListenBrainz token is required."
        })));
    }
    let (http, master_key) = {
        let s = state.read().await;
        (s.http_client.clone(), s.master_key.clone())
    };
    let validation = match crate::services::listenbrainz::validate_token(&http, &token).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(Json(json!({
                "status": "error",
                "message": format!("Could not validate ListenBrainz token: {e}")
            })));
        }
    };
    if !validation.valid {
        return Ok(Json(json!({
            "status": "error",
            "message": "ListenBrainz rejected that token."
        })));
    }
    let Some(user_name) = validation.user_name else {
        return Ok(Json(json!({
            "status": "error",
            "message": "ListenBrainz did not return a username for that token."
        })));
    };
    let result = state.read().await.db.with_conn(|conn| {
        crate::services::listenbrainz::save_credentials(conn, &master_key, &token, &user_name)?;
        Ok::<_, anyhow::Error>(())
    });
    if let Err(e) = result {
        tracing::warn!("Failed to save ListenBrainz credentials: {e:#}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(Json(json!({
        "status": "ok",
        "user": user_name,
    })))
}

pub(super) async fn listenbrainz_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (creds, has_token, pending, failed) = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let creds = crate::services::listenbrainz::load_credentials(conn)
                .ok()
                .flatten();
            let has_token = crate::services::listenbrainz::has_token(conn).unwrap_or(false);
            let (pending, failed) =
                crate::services::scrobbling::outbox_status(conn).unwrap_or((0, 0));
            Ok::<_, anyhow::Error>((creds, has_token, pending, failed))
        })
        .unwrap_or((None, false, 0, 0))
    };
    let user = creds.as_ref().and_then(|c| c.user_name.clone());
    let configured = has_token && user.is_some();
    Ok(Json(json!({
        "configured": configured,
        "scrobbling": configured && creds.as_ref().is_some_and(|c| c.scrobbling_enabled),
        "recommendations": configured && creds.as_ref().is_some_and(|c| c.recommendations_enabled),
        "pending_submissions": pending,
        "failed_submissions": failed,
        "user": user,
    })))
}

pub(super) async fn listenbrainz_clear_config(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let _ = state.read().await.db.with_conn(|conn| {
        crate::services::listenbrainz::clear_credentials(conn)?;
        Ok::<_, anyhow::Error>(())
    });
    Ok(Json(json!({"status": "cleared"})))
}

pub(super) async fn lastfm_clear_config(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let _ = state.read().await.db.with_conn(|conn| {
        lastfm::auth::clear_credentials(conn)?;
        Ok(())
    });
    Ok(Json(json!({"status": "cleared"})))
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LastfmEnrichmentStartParams {
    mode: Option<String>,
    refresh: Option<bool>,
}

impl LastfmEnrichmentStartParams {
    fn mode(&self) -> crate::services::lastfm::enrichment::EnrichmentMode {
        if self.refresh == Some(true)
            || self.mode.as_deref().is_some_and(|mode| {
                mode.eq_ignore_ascii_case("refresh") || mode.eq_ignore_ascii_case("refresh_all")
            })
        {
            crate::services::lastfm::enrichment::EnrichmentMode::RefreshAll
        } else if self.mode.as_deref().is_some_and(|mode| {
            mode.eq_ignore_ascii_case("retry_untagged")
                || mode.eq_ignore_ascii_case("untagged")
                || mode.eq_ignore_ascii_case("missing")
        }) {
            crate::services::lastfm::enrichment::EnrichmentMode::RetryUntagged
        } else {
            crate::services::lastfm::enrichment::EnrichmentMode::Pending
        }
    }
}

pub(super) async fn start_lastfm_enrichment(
    State(state): State<SharedState>,
    Query(params): Query<LastfmEnrichmentStartParams>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    use crate::services::lastfm::enrichment::EnrichmentMode;
    use std::sync::atomic::Ordering;

    let (
        http,
        event_tx,
        running,
        cancel,
        total_atom,
        processed_atom,
        prefetch_total_atom,
        prefetch_done_atom,
        started_at_atom,
    ) = {
        let s = state.read().await;
        (
            s.http_client.clone(),
            s.event_tx.clone(),
            s.lastfm_enrich_running.clone(),
            s.lastfm_enrich_cancel.clone(),
            s.lastfm_enrich_total.clone(),
            s.lastfm_enrich_processed.clone(),
            s.lastfm_prefetch_total.clone(),
            s.lastfm_prefetch_done.clone(),
            s.lastfm_enrich_started_at.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        let total = total_atom.load(Ordering::SeqCst);
        let processed = processed_atom.load(Ordering::SeqCst);
        return Ok(Json(json!({
            "status": "already_running",
            "total": total,
            "processed": processed
        })));
    }

    let creds = state
        .read()
        .await
        .db
        .with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
        .unwrap_or(None);
    let Some(creds) = creds else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Last.fm API key not configured."
        })));
    };

    let mode = params.mode();
    let total: usize = state
        .read()
        .await
        .db
        .with_conn(|conn| lastfm::enrichment::count_tracks_to_enrich(conn, mode))
        .unwrap_or(0);

    if total == 0 {
        return Ok(Json(json!({
            "status": if mode == EnrichmentMode::RefreshAll {
                "no_eligible_tracks"
            } else {
                "already_complete"
            }
        })));
    }

    cancel.store(false, Ordering::SeqCst);
    running.store(true, Ordering::SeqCst);
    total_atom.store(total, Ordering::SeqCst);
    processed_atom.store(0, Ordering::SeqCst);
    prefetch_total_atom.store(0, Ordering::SeqCst);
    prefetch_done_atom.store(0, Ordering::SeqCst);
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    started_at_atom.store(now_secs, Ordering::SeqCst);

    let api_key = creds.api_key.clone();
    let started_at_atom_cleanup = started_at_atom.clone();
    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let artist_tx = event_tx.clone();
        let total_atom_cb = total_atom.clone();
        let processed_atom_cb = processed_atom.clone();
        let prefetch_total_cb = prefetch_total_atom.clone();
        let prefetch_done_cb = prefetch_done_atom.clone();
        let result = lastfm::enrichment::run_enrichment(
            state,
            http,
            api_key,
            mode,
            cancel,
            move |done, artist_total| {
                prefetch_total_cb.store(artist_total, Ordering::SeqCst);
                prefetch_done_cb.store(done, Ordering::SeqCst);
                let _ = artist_tx.send(AppEvent::SyncProgress {
                    service: "lastfm".to_string(),
                    progress: done as f32 / artist_total.max(1) as f32,
                });
            },
            move |current, total| {
                processed_atom_cb.store(current, Ordering::SeqCst);
                if total > 0 {
                    total_atom_cb.store(total, Ordering::SeqCst);
                }
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "lastfm".to_string(),
                    progress: current as f32 / total.max(1) as f32,
                });
            },
        )
        .await;
        running.store(false, Ordering::SeqCst);
        started_at_atom_cleanup.store(0, Ordering::SeqCst);
        if result.is_ok() {
            let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
        }
    });

    Ok(Json(json!({"status": "started", "total": total})))
}

pub(super) async fn get_lastfm_enrichment_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    let stats =
        s.db.with_conn(crate::services::lastfm::enrichment::enrichment_stats)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let is_running = s.lastfm_enrich_running.load(Ordering::SeqCst);
    let run_total = s.lastfm_enrich_total.load(Ordering::SeqCst);
    let run_processed = s.lastfm_enrich_processed.load(Ordering::SeqCst);
    let prefetch_total = s.lastfm_prefetch_total.load(Ordering::SeqCst);
    let prefetch_done = s.lastfm_prefetch_done.load(Ordering::SeqCst);
    let run_started_at = s.lastfm_enrich_started_at.load(Ordering::SeqCst);
    Ok(Json(json!({
        "total_tracks": stats.total_tracks,
        "checked_tracks": stats.checked_tracks,
        "enriched_tracks": stats.enriched_tracks,
        "remaining_tracks": stats.remaining_tracks,
        "is_running": is_running,
        "run_total": run_total,
        "run_processed": run_processed,
        "prefetch_total": prefetch_total,
        "prefetch_done": prefetch_done,
        "run_started_at": run_started_at,
    })))
}

pub(super) async fn reset_lastfm_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    if s.lastfm_enrich_running.load(Ordering::SeqCst) {
        return Ok(Json(json!({
            "status": "error",
            "message": "Cannot reset while enrichment is running."
        })));
    }
    let result: anyhow::Result<(usize, usize)> = s.db.with_conn(|conn| {
        let checks = conn.execute("DELETE FROM lastfm_checked", [])?;
        let tags = conn.execute("DELETE FROM track_genres WHERE source = 'lastfm'", [])?;
        conn.execute("DELETE FROM lastfm_artist_cache", [])?;
        Ok((checks, tags))
    });
    match result {
        Ok((checks, tags)) => Ok(Json(json!({
            "status": "ok",
            "checks_cleared": checks,
            "tags_cleared": tags,
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Reset failed: {}", e),
        }))),
    }
}

pub(super) async fn stop_lastfm_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    s.lastfm_enrich_cancel.store(true, Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopping" })))
}

// ── Audio Analysis Endpoints ─────────────────────────────────────────────────

/// Reasoning lives in `services/lastfm/scrobble.rs` and the plan file. Short
/// version: the user goes Settings, selects "Connect Last.fm account", opens
/// the Last.fm auth URL in a new tab, clicks "Yes, allow access", returns to
/// NOORwave, and clicks "I've authorized". Then /complete redeems the token
/// for a session_key encrypted on disk.

pub(super) async fn lastfm_auth_start(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;

    let (http, api_secret, api_key) = {
        let s = state.read().await;
        let result = s.db.with_conn(|conn| {
            let creds = lastfm::auth::load_credentials(conn).ok().flatten();
            let api_secret = lastfm::auth::load_api_secret(conn, &s.master_key)
                .ok()
                .flatten()
                .or_else(|| s.lastfm_api_secret.clone());
            Ok::<_, anyhow::Error>((api_secret, creds.map(|c| c.api_key)))
        });
        let (api_secret, api_key) = result.unwrap_or((s.lastfm_api_secret.clone(), None));
        (s.http_client.clone(), api_secret, api_key)
    };
    let Some(api_secret) = api_secret else {
        return Err(StatusCode::NOT_IMPLEMENTED);
    };
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Save a Last.fm API key first."
        })));
    };

    let token = match lastfm::scrobble::get_token(&http, &api_key, &api_secret).await {
        Ok(t) => t,
        Err(e) => {
            return Ok(Json(json!({
                "status": "error",
                "message": format!("auth.getToken failed: {e}")
            })));
        }
    };

    // Stash the pending token server-side so /complete doesn't have to trust
    // the client to round-trip it.
    let stash_result = state.read().await.db.with_conn(|conn| {
        lastfm::auth::set_pending_token(conn, &token)?;
        Ok(())
    });
    if let Err(e) = stash_result {
        tracing::warn!("Failed to stash Last.fm pending_token: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let auth_url = format!(
        "https://www.last.fm/api/auth/?api_key={}&token={}",
        urlencoding::encode(&api_key),
        urlencoding::encode(&token)
    );
    Ok(Json(json!({
        "status": "awaiting",
        "auth_url": auth_url,
    })))
}

pub(super) async fn lastfm_auth_complete(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;

    let (http, api_secret, api_key, pending_token, master_key) = {
        let s = state.read().await;
        let creds =
            s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
                .ok()
                .flatten();
        let api_secret =
            s.db.with_conn(|conn| lastfm::auth::load_api_secret(conn, &s.master_key))
                .ok()
                .flatten()
                .or_else(|| s.lastfm_api_secret.clone());
        (
            s.http_client.clone(),
            api_secret,
            creds.as_ref().map(|c| c.api_key.clone()),
            creds.and_then(|c| c.pending_token),
            s.master_key.clone(),
        )
    };
    let Some(api_secret) = api_secret else {
        return Err(StatusCode::NOT_IMPLEMENTED);
    };
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Last.fm API key not configured."
        })));
    };
    let Some(token) = pending_token else {
        return Ok(Json(json!({
            "status": "error",
            "message": "No pending auth. Call /api/lastfm/auth/start first."
        })));
    };

    let session = match lastfm::scrobble::get_session(&http, &api_key, &api_secret, &token).await {
        Ok(s) => s,
        Err(e) => {
            // Don't drop the pending_token on a "not yet authorized" error.
            // the user might just need a few more seconds in the browser.
            // The user can retry by clicking the button again.
            return Ok(Json(json!({
                "status": "not_yet_authorized",
                "message": format!("auth.getSession failed: {e}")
            })));
        }
    };

    let persist_result = state.read().await.db.with_conn(|conn| {
        lastfm::auth::save_session_key(
            conn,
            &master_key,
            &session.session_key,
            &session.user_name,
        )?;
        Ok(())
    });
    if let Err(e) = persist_result {
        tracing::warn!("Failed to persist Last.fm session: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(json!({
        "status": "connected",
        "user": session.user_name,
    })))
}

pub(super) async fn lastfm_auth_disconnect(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let _ = state.read().await.db.with_conn(|conn| {
        lastfm::auth::clear_session(conn)?;
        Ok(())
    });
    Ok(Json(json!({"status": "disconnected"})))
}
