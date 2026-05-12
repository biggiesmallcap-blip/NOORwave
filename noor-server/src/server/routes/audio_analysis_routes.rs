use crate::SharedState;
use crate::db::queries;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub(super) struct AudioAnalysisRequest {
    mode: String, // "preview" or "local"
    local_path: Option<String>,
}

pub(super) async fn start_audio_analysis(
    State(state): State<SharedState>,
    Json(payload): Json<AudioAnalysisRequest>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::audio_analysis::scanner;

    let mode = payload.mode.clone();
    let local_path = payload.local_path.clone();
    let (analysis_tx, cancel, running) = {
        let s = state.read().await;
        (
            s.analysis_tx.clone(),
            s.audio_analysis_cancel.clone(),
            s.audio_analysis_running.clone(),
        )
    };

    let Some(tx) = analysis_tx else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    // Reset cancel flag and mark as running before spawning
    cancel.store(false, std::sync::atomic::Ordering::Relaxed);
    running.store(true, std::sync::atomic::Ordering::Relaxed);

    let mode_for_spawn = mode.clone();
    tokio::spawn(async move {
        match mode_for_spawn.as_str() {
            "preview" => {
                scanner::run_preview_scan(state, tx, cancel).await;
            }
            "local" => {
                if let Some(raw) = local_path {
                    // Reject traversal sequences and resolve to a real absolute path
                    let candidate = std::path::PathBuf::from(&raw);
                    let resolved = match std::fs::canonicalize(&candidate) {
                        Ok(p) if p.is_dir() => p,
                        _ => {
                            tracing::warn!("local scan rejected invalid path: {:?}", raw);
                            return;
                        }
                    };
                    scanner::run_local_scan(state, tx, cancel, resolved, Default::default()).await;
                }
            }
            _ => {}
        }
        running.store(false, std::sync::atomic::Ordering::Relaxed);
    });

    Ok(Json(json!({ "status": "started", "mode": mode })))
}

pub(super) async fn stop_audio_analysis(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.audio_analysis_cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);
    s.audio_analysis_running
        .store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopped" })))
}

pub(super) async fn get_audio_analysis_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let analyzed =
        s.db.with_conn(queries::count_audio_dsp_features)
            .unwrap_or(0);
    Ok(Json(json!({
        "running": s.audio_analysis_running.load(std::sync::atomic::Ordering::Relaxed),
        "analyzed": analyzed,
    })))
}

pub(super) async fn get_passive_dsp(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let enabled =
        s.db.with_conn(|conn| Ok(crate::services::audio_analysis::is_passive_enabled(conn)))
            .unwrap_or(true);
    Ok(Json(json!({ "enabled": enabled })))
}

#[derive(Deserialize)]
pub(super) struct PassiveDspBody {
    enabled: bool,
}

pub(super) async fn set_passive_dsp(
    State(state): State<SharedState>,
    Json(payload): Json<PassiveDspBody>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.db.with_conn(|conn| {
        crate::services::audio_analysis::set_passive_enabled(conn, payload.enabled)
            .map_err(anyhow::Error::from)
    })
    .map_err(|e| {
        tracing::error!("failed to persist passive_dsp_enabled: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(json!({ "enabled": payload.enabled })))
}

pub(super) async fn get_track_audio_features(
    State(state): State<SharedState>,
    Path(track_id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let features =
        s.db.with_conn(|conn| queries::get_audio_dsp_features(conn, track_id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "features": features })))
}

pub(super) async fn get_audio_features_stats(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let stats =
        s.db.with_conn(queries::get_audio_features_stats)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "stats": stats })))
}

pub(super) async fn get_library_analytics(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let summary =
        s.db.with_conn(|conn| {
            let tracks = queries::get_all_tracks(conn)?;
            let playlists = queries::get_playlists(conn)?;
            let genre_paths = queries::get_track_genre_paths_with_fallback(conn)?;
            let mut context = crate::smart::analytics::AnalyticsContext::new();
            for (track_id, rows) in genre_paths {
                context =
                    context.with_track_genres(track_id, queries::ResolvedGenre::paths_only(&rows));
            }
            Ok(crate::smart::analytics::summarize_library(
                &tracks, &playlists, &context,
            ))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "analytics": summary })))
}

pub(super) async fn reset_audio_analysis(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.db.with_conn(queries::delete_all_audio_dsp_features)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": "reset" })))
}

/// GET /api/library/audio-features/quality - coverage / confidence breakdown.
pub(super) async fn get_audio_features_quality(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let q =
        s.db.with_conn(queries::get_audio_features_quality)
            .map_err(|e| {
                tracing::error!("audio-features/quality query failed: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    Ok(Json(json!({
        "total_tracks": q.total_tracks,
        "analyzed": q.analyzed,
        "analysis_current": q.analysis_current,
        "analysis_stale": q.analysis_stale,
        "low_confidence_bpm": q.low_confidence_bpm,
        "low_confidence_key": q.low_confidence_key,
        "no_preview_url": q.no_preview_url,
        "fingerprinted": q.fingerprinted,
    })))
}

/// GET /api/library/analyze/reanalyze-stale - re-queue every track whose
/// stored `analysis_version` is not the current `CURRENT_ANALYSIS_VERSION`
/// (see `crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION`). If the
/// analysis actor isn't wired we still return the count of stale tracks so the
/// caller can decide what to do next.
pub(super) async fn reanalyze_stale_tracks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (db, analysis_tx) = {
        let s = state.read().await;
        (s.db.clone(), s.analysis_tx.clone())
    };

    let stale_ids = db
        .with_conn(queries::get_stale_analysis_track_ids)
        .map_err(|e| {
            tracing::error!("reanalyze-stale query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let total = stale_ids.len();

    // Drop the DSP rows so the next scan picks them up, and optionally
    // nudge the analysis actor (it only accepts jobs with decoded samples,
    // so here we simply log the queue size - a fresh scan will actually
    // re-decode & re-analyse).
    if total > 0 {
        db.with_conn(|conn| -> anyhow::Result<()> {
            // CURRENT_ANALYSIS_VERSION is a compile-time constant - safe to interpolate.
            conn.execute(
                &format!(
                    "DELETE FROM audio_dsp_features WHERE analysis_version != '{}'",
                    crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
                ),
                [],
            )?;
            Ok(())
        })
        .map_err(|e| {
            tracing::error!("reanalyze-stale delete failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    let actor_configured = analysis_tx.is_some();

    Ok(Json(json!({
        "status": "queued",
        "stale_count": total,
        "actor_configured": actor_configured,
        "note": if actor_configured {
            "Stale analyses cleared. Run /api/library/analyze/audio-features to re-scan."
        } else {
            "Analysis actor not configured. Stale rows cleared but no scan queued."
        }
    })))
}
