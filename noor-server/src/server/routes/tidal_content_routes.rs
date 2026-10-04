use crate::{AppEvent, SharedState, db::tidal_content};
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{Json, Response},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    hide_ai_generated: bool,
}

pub(super) async fn get_settings(
    State(state): State<SharedState>,
) -> Result<Json<Settings>, StatusCode> {
    let db = state.read().await.db.clone();
    let hide_ai_generated = db
        .with_conn(|conn| Ok(tidal_content::enabled(conn)?))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(Settings { hide_ai_generated }))
}

pub(super) async fn put_settings(
    State(state): State<SharedState>,
    Json(settings): Json<Settings>,
) -> Result<Json<Settings>, StatusCode> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| {
            Ok(tidal_content::set_enabled(
                conn,
                settings.hide_ai_generated,
            )?)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = guard.event_tx.send(AppEvent::TidalContentSettingsChanged);
    Ok(Json(settings))
}

fn is_browse_path(path: &str) -> bool {
    if path.contains("/video") {
        return false;
    }
    [
        "/api/tidal/",
        "/api/discovery/",
        "/api/home",
        "/api/charts",
        "/api/search",
        "/api/tracks",
        "/api/albums",
        "/api/artists",
        "/api/playlists",
        "/api/radio/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

/// Apply the current preference after cached/raw responses are projected. Upstream pages stay
/// untouched so album pagination and library sync never truncate when a page contains AI tracks.
pub(super) async fn filter_browse(
    State(state): State<SharedState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if !is_browse_path(&path)
        || !response.status().is_success()
        || !response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes().starts_with(b"application/json"))
    {
        return Ok(response);
    }
    let db = state.read().await.db.clone();
    let blocked = db
        .with_conn(|conn| Ok(tidal_content::blocked_ids(conn)?))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if blocked.is_empty() {
        return Ok(response);
    }
    let (mut parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Ok(mut payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(Response::from_parts(parts, Body::from(bytes)));
    };
    tidal_content::filter_json(&mut payload, &blocked, path.starts_with("/api/tidal/"));
    let bytes = serde_json::to_vec(&payload).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    parts
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    parts.headers.remove(header::ETAG);
    Ok(Response::from_parts(parts, Body::from(bytes)))
}
