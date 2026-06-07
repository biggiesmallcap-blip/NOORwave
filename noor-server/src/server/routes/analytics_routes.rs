use crate::SharedState;
use crate::db::{models::AnalyticsDashboard, queries, signals};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub(super) struct ListenHistoryParams {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnalyticsDashboardParams {
    recent_limit: Option<i64>,
    top_limit: Option<i64>,
    days: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnalyticsSignalsParams {
    days: Option<i64>,
}

pub(super) async fn get_analytics_overview(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let overview = queries::get_analytics_overview(conn)?;
            Ok(Json(json!({ "overview": overview })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_recent_listens(
    State(state): State<SharedState>,
    Query(params): Query<ListenHistoryParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(25).clamp(1, 200);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let listens = queries::get_recent_listens(conn, limit)?;
            Ok(Json(json!({ "listens": listens })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_analytics_dashboard(
    State(state): State<SharedState>,
    Query(params): Query<AnalyticsDashboardParams>,
) -> Result<Json<Value>, StatusCode> {
    let recent_limit = params.recent_limit.unwrap_or(12).clamp(1, 50);
    let top_limit = params.top_limit.unwrap_or(8).clamp(1, 20);
    // Keep the wide range so the analytics page's "All" pill reaches the
    // legacy dashboard endpoint without silent truncation.
    let days = params.days.unwrap_or(14).clamp(1, 36500);

    let state = state.read().await;
    let dashboard = state
        .db
        .with_conn(|conn| {
            Ok(AnalyticsDashboard {
                overview: queries::get_analytics_overview(conn)?,
                recent_listens: queries::get_recent_listens(conn, recent_limit)?,
                top_tracks: queries::get_top_tracks_by_history(conn, top_limit)?,
                top_artists: queries::get_top_artists_by_history(conn, top_limit)?,
                top_genres: queries::get_top_genres_by_history(conn, top_limit)?,
                activity: queries::get_listen_activity(conn, days)?,
                behavior: queries::get_behavior_metrics(conn)?,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "dashboard": dashboard })))
}

pub(super) async fn get_analytics_signals(
    State(state): State<SharedState>,
    Query(params): Query<AnalyticsSignalsParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(30).clamp(1, 36500);

    let state = state.read().await;
    let signals = state
        .db
        .with_conn(|conn| signals::Signals::compute(conn, days))
        .map_err(|err| {
            tracing::error!(?err, days, "get_analytics_signals failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(json!({ "signals": signals })))
}
