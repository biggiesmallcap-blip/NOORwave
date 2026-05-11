use crate::SharedState;
use crate::db::queries;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
pub(super) struct GenreTrackParams {
    include_descendants: Option<bool>,
    /// Galaxy display filter. See `crate::genre::filter::GalaxyFilterRule`.
    /// Tokens: `all` | `conf05` | `conf07` | `top2` | `top3` | `mb_only` |
    /// `primary`. Unknown or missing values use the default `conf05`.
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GenreListParams {
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GenreSnapshotParams {
    days: Option<i64>,
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GenreHeatParams {
    days: Option<i64>,
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GenreCoOccurrenceParams {
    days: Option<i64>,
    window_minutes: Option<i64>,
    min_count: Option<i64>,
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GenreCohortParams {
    days: Option<i64>,
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GenreEvolutionParams {
    days: Option<i64>,
}

pub(super) async fn get_genres(
    State(state): State<SharedState>,
    Query(params): Query<GenreListParams>,
) -> Result<Json<Value>, StatusCode> {
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let genres = queries::get_genre_tree_filtered(conn, filter)?;
            Ok(Json(json!({
                "genres": genres,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_genre_snapshot(
    State(state): State<SharedState>,
    Query(params): Query<GenreSnapshotParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    let started = Instant::now();
    let result = state
        .db
        .with_conn(|conn| {
            let genres = queries::get_genre_tree_filtered(conn, filter)?;
            let heat = queries::get_genre_heat_filtered(conn, days, filter)?;
            let cohorts = queries::get_genre_cohorts_filtered(conn, days, filter, false)?;
            let evolution = queries::get_genre_evolution(conn, days.max(7))?;
            let metrics = queries::get_genre_audio_metrics(conn)?;
            Ok(Json(json!({
                "genres": genres,
                "heat": heat,
                "cohorts": cohorts,
                "evolution": evolution,
                "metrics": metrics,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|err| {
            tracing::error!("genre snapshot query failed: {err:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let elapsed = started.elapsed();
    if elapsed > Duration::from_millis(1000) {
        tracing::warn!(
            elapsed_ms = elapsed.as_millis(),
            days,
            filter = %filter.label(),
            "genre snapshot query was slow"
        );
    }

    Ok(result)
}

pub(super) async fn get_genre_heat(
    State(state): State<SharedState>,
    Query(params): Query<GenreHeatParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let heat = queries::get_genre_heat_filtered(conn, days, filter)?;
            Ok(Json(json!({
                "heat": heat,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_genre_co_occurrence(
    State(state): State<SharedState>,
    Query(params): Query<GenreCoOccurrenceParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let window = params.window_minutes.unwrap_or(30).max(5);
    let min = params.min_count.unwrap_or(3).max(1);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let pairs = queries::get_genre_co_occurrence_filtered(conn, days, window, min, filter)
                .map_err(|e| {
                    tracing::error!("co-occurrence query failed: {e:#}");
                    anyhow::anyhow!("co-occurrence query failed: {e:#}")
                })?;
            Ok(Json(json!({
                "pairs": pairs,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|e| {
            tracing::error!("co-occurrence handler error: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

pub(super) async fn get_genre_cohorts(
    State(state): State<SharedState>,
    Query(params): Query<GenreCohortParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            // HTTP endpoint preserves strict semantics: `?filter` controls
            // exactly what's matched. Fallback rescue is internal-only.
            let cohorts = queries::get_genre_cohorts_filtered(conn, days, filter, false)?;
            Ok(Json(json!({
                "cohorts": cohorts,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_genre_evolution(
    State(state): State<SharedState>,
    Query(params): Query<GenreEvolutionParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(7);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let evolution = queries::get_genre_evolution(conn, days)?;
            Ok(Json(json!({ "evolution": evolution })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_genre_audio_metrics(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let metrics = queries::get_genre_audio_metrics(conn)?;
            Ok(Json(json!({ "metrics": metrics })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_genre_tracks(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(params): Query<GenreTrackParams>,
) -> Result<Json<Value>, StatusCode> {
    let include_descendants = params.include_descendants.unwrap_or(true);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks =
                queries::get_tracks_by_genre_filtered(conn, id, include_descendants, filter)?;
            Ok(Json(json!({
                "tracks": tracks,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
