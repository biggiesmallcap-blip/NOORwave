//! Catalog intelligence reads: census, the Archive lens query, a song's
//! versions, a concert's setlist, and a manual rebuild trigger.
//!
//! Every handler is a local read over the derived tables (or a kick of the
//! background build); nothing here calls TIDAL. Reads run on a blocking
//! thread against the shared connection, so its warm page cache serves them.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::Serialize;
use serde_json::json;

use crate::SharedState;
use crate::db::Database;
use crate::services::video_intel::{archive, census, concerts, job, songs};

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Run a read on a blocking thread and serialize it; None is a 404.
async fn read<T: Serialize + Send + 'static>(
    db: Database,
    what: &'static str,
    f: impl FnOnce(&Database) -> anyhow::Result<Option<T>> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(move || f(&db)).await {
        Ok(Ok(Some(value))) => Json(value).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, &format!("{what} not found")),
        Ok(Err(err)) => {
            tracing::warn!(target: "noor.video_intel", what, error = %format!("{err:#}"), "read failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "read failed")
        }
        Err(err) => {
            tracing::warn!(target: "noor.video_intel", what, error = %err, "read task failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "read failed")
        }
    }
}

/// GET /api/videos/census
pub(super) async fn get_video_census(State(state): State<SharedState>) -> Response {
    let db = { state.read().await.db.clone() };
    read(db, "census", |db| {
        db.with_conn(|conn| census::read(conn, job::is_running()).map(Some))
    })
    .await
}

/// GET /api/videos/archive?scene&cut&era_from&era_to&depth&length&source&sort&q&offset&limit
pub(super) async fn get_video_archive(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let db = { state.read().await.db.clone() };
    let query = archive::Query::from_params(&params);
    read(db, "archive", move |db| {
        let index = archive::cached_for(db)?;
        db.with_conn(|conn| {
            let lights = archive::Lights::load(conn)?;
            archive::query(conn, &index, &query, &lights).map(Some)
        })
    })
    .await
}

/// GET /api/videos/songs/{key}/versions
pub(super) async fn get_video_song_versions(
    State(state): State<SharedState>,
    Path(key): Path<String>,
) -> Response {
    let db = { state.read().await.db.clone() };
    read(db, "song", move |db| {
        db.with_conn(|conn| songs::versions(conn, &key))
    })
    .await
}

/// GET /api/videos/concerts/{id}
pub(super) async fn get_video_concert(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Response {
    let db = { state.read().await.db.clone() };
    read(db, "concert", move |db| {
        db.with_conn(|conn| concerts::read(conn, id))
    })
    .await
}

/// POST /api/videos/intel/rebuild
pub(super) async fn post_video_intel_rebuild(State(state): State<SharedState>) -> Response {
    let db = { state.read().await.db.clone() };
    let started = job::kick(db, true);
    Json(json!({ "started": started, "running": true })).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::super::api_routes;
    use super::super::tests::{fresh_migrated_db, fresh_test_state};
    use crate::services::video_intel::build::build_all;
    use crate::services::video_sets::VideoCandidate;

    async fn get(db: &crate::db::Database, uri: &str) -> (StatusCode, Value) {
        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    use axum::http::StatusCode;

    fn seed(db: &crate::db::Database) {
        db.with_conn(|conn| {
            for (id, title) in [
                (100, "Song A (Live at Wembley 1986)"),
                (101, "Song B (Live at Wembley 1986)"),
                (102, "Song C (Live at Wembley 1986)"),
                (103, "Song A"),
            ] {
                let video = VideoCandidate {
                    tidal_id: id,
                    title: title.into(),
                    duration_s: Some(240),
                    artist_id: Some(7),
                    artist_name: Some("Queen".into()),
                    release_year: Some(1986),
                    ..Default::default()
                };
                conn.execute(
                    "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
                     VALUES (?1, 7, 'Queen', ?2)",
                    rusqlite::params![id, serde_json::to_string(&video)?],
                )?;
            }
            build_all(conn)?;
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn census_archive_versions_and_concerts_answer() {
        let db = fresh_migrated_db();
        seed(&db);
        let (status, census) = get(&db, "/api/videos/census").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(census["videos"], 4);
        assert_eq!(census["concerts"], 1);
        assert_eq!(census["cuts"]["live"], 3);

        let (status, archive) = get(&db, "/api/videos/archive?cut=live&sort=year").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(archive["total"], 3);
        assert_eq!(archive["items"][0]["best"]["type"], "Music Video");
        assert!(archive["facets"]["cut"].is_array());

        let (status, versions) = get(&db, "/api/videos/songs/id%3A7%3Asong%20a/versions").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(versions["versions"].as_array().unwrap().len(), 2);
        assert_eq!(versions["versions"][1]["concert"]["id"], 100);
        assert_eq!(versions["versions"][1]["label"], "Live at Wembley 1986");

        let (status, concert) = get(&db, "/api/videos/concerts/100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(concert["video_count"], 3);
        assert_eq!(concert["videos"][2]["tidal_id"], 102);

        let (status, _) = get(&db, "/api/videos/concerts/5").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = get(&db, "/api/videos/songs/nope/versions").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
