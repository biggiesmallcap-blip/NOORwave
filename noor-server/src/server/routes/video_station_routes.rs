//! Video stations: the daily lineup and endless refills from the local
//! catalog. Neither endpoint calls TIDAL.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::SharedState;
use crate::db::Database;
use crate::services::video_discovery::setting as discovery_setting;
use crate::services::video_stations::{self, Scene, StationId, lineup, pool, settings};

const RETRY_AFTER: Duration = Duration::from_secs(600);

static BUILDING: AtomicBool = AtomicBool::new(false);
static LAST_FAILURE: Mutex<Option<Instant>> = Mutex::new(None);

fn today() -> String {
    chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

/// Station work scans the whole catalog, so it runs on its own connection
/// and leaves the shared one free; in-memory test databases fall back to it.
fn with_own_conn<T>(
    db: &Database,
    f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    match db.open_isolated() {
        Ok(conn) => f(&conn),
        Err(_) => db.with_conn(f),
    }
}

/// One build at a time; a failed build waits ten minutes before retrying.
fn kick_build(db: Database, day: String) {
    let recently_failed = LAST_FAILURE
        .lock()
        .ok()
        .and_then(|guard| *guard)
        .is_some_and(|at| at.elapsed() < RETRY_AFTER);
    if recently_failed || BUILDING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let result = with_own_conn(&db, |conn| lineup::build(conn, &day));
        if let Ok(cards) = &result {
            tracing::info!(
                target: "noor.video_stations",
                stations = cards.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "station lineup built"
            );
        }
        if let Err(error) = result {
            tracing::warn!(target: "noor.video_stations", %error, "station lineup build failed");
            if let Ok(mut guard) = LAST_FAILURE.lock() {
                *guard = Some(Instant::now());
            }
        }
        BUILDING.store(false, Ordering::SeqCst);
    });
}

/// `GET /api/videos/stations`.
pub(super) async fn get_video_stations(State(state): State<SharedState>) -> Json<Value> {
    let db = { state.read().await.db.clone() };
    let day = today();
    // A lineup an older release built is treated as missing: it is still
    // shown below while today's is rebuilt with the current stations.
    let todays = db
        .with_conn(|conn| {
            if lineup::is_current(conn)? {
                lineup::load(conn, &day)
            } else {
                Ok(Vec::new())
            }
        })
        .unwrap_or_default();
    let (shown_day, stations) = if todays.is_empty() {
        kick_build(db.clone(), day.clone());
        match db.with_conn(lineup::latest).ok().flatten() {
            Some((previous, cards)) => (Some(previous), cards),
            None => (None, Vec::new()),
        }
    } else {
        (Some(day), todays)
    };
    let catalog_videos: i64 = db
        .with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM video_catalog", [], |row| row.get(0))?)
        })
        .unwrap_or(0);
    let setting = db.with_conn(discovery_setting::load).unwrap_or_default();
    Json(json!({
        "day": shown_day,
        "building": BUILDING.load(Ordering::SeqCst),
        "catalog_videos": catalog_videos,
        "discovery_setting": setting.as_str(),
        "stations": stations,
    }))
}

#[derive(Deserialize)]
pub(super) struct StationNextRequest {
    #[serde(default)]
    exclude_video_ids: Vec<i64>,
    #[serde(default)]
    recent_video_ids: Vec<i64>,
    #[serde(default)]
    session_nonce: String,
}

/// `POST /api/videos/stations/{id}/next`. The next 12 videos; never one the
/// session already queued or played.
pub(super) async fn post_video_station_next(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<StationNextRequest>,
) -> Json<Value> {
    let Some(station) = StationId::parse(&id) else {
        return Json(json!({ "items": [], "exhausted": true }));
    };
    let db = { state.read().await.db.clone() };
    let excluded: HashSet<i64> = body
        .exclude_video_ids
        .into_iter()
        .take(256)
        .chain(body.recent_video_ids.into_iter().take(128))
        .filter(|id| *id > 0)
        .collect();
    let nonce: String = body.session_nonce.chars().take(64).collect();
    let day = today();
    let result = tokio::task::spawn_blocking(move || {
        with_own_conn(&db, |conn| {
            let listener = pool::load_listener(conn)?;
            let seed = video_stations::seed_for(&station, &day, &nonce);
            video_stations::next_batch(conn, &listener, &station, &excluded, seed)
        })
    })
    .await;
    let items = match result {
        Ok(Ok(items)) => items,
        Ok(Err(error)) => {
            tracing::warn!(target: "noor.video_stations", %error, "station refill failed");
            Vec::new()
        }
        Err(error) => {
            tracing::warn!(target: "noor.video_stations", %error, "station refill task failed");
            Vec::new()
        }
    };
    Json(json!({ "exhausted": items.is_empty(), "items": items }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::services::video_sets::VideoCandidate;

    fn state(db: Database) -> SharedState {
        Arc::new(tokio::sync::RwLock::new(
            crate::server::routes::tests::fresh_test_state(db),
        ))
    }

    fn add_videos(db: &Database, count: i64) {
        db.with_conn(|conn| {
            for id in 1..=count {
                let video = VideoCandidate {
                    tidal_id: id,
                    title: format!("Song {id}"),
                    duration_s: Some(200),
                    artist_id: Some(500 + id),
                    artist_name: Some(format!("Artist {id}")),
                    ..Default::default()
                };
                conn.execute(
                    "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![id, 500 + id, video.artist_name, serde_json::to_string(&video)?],
                )?;
            }
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn an_unknown_station_is_empty_and_exhausted() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        let response = post_video_station_next(
            State(state(db)),
            Path("everything".into()),
            Json(StationNextRequest {
                exclude_video_ids: Vec::new(),
                recent_video_ids: Vec::new(),
                session_nonce: String::new(),
            }),
        )
        .await;
        assert_eq!(response.0["items"], json!([]));
        assert_eq!(response.0["exhausted"], json!(true));
    }

    #[tokio::test]
    async fn a_station_refills_from_the_catalog_without_repeats() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        add_videos(&db, 20);
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            post_video_station_next(
                State(state(db)),
                Path("shuffle".into()),
                Json(StationNextRequest {
                    exclude_video_ids: vec![1, 2, 3],
                    recent_video_ids: vec![4],
                    session_nonce: "n".into(),
                }),
            ),
        )
        .await
        .expect("answers without TIDAL");
        let ids: Vec<i64> = response.0["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["tidal_id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids.len(), 12);
        assert!(ids.iter().all(|id| *id > 4));
        assert_eq!(response.0["exhausted"], json!(false));
    }

    #[tokio::test]
    async fn the_lineup_reports_building_when_today_is_missing() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        let response = get_video_stations(State(state(db))).await;
        assert!(response.0["stations"].as_array().unwrap().is_empty());
        assert_eq!(response.0["catalog_videos"], json!(0));
        assert_eq!(response.0["discovery_setting"], json!("full"));
        assert!(response.0["building"].is_boolean());
    }
}

fn explore_settings_json(explore: &settings::ExploreSettings) -> Value {
    let scenes: Vec<Value> = Scene::ALL
        .iter()
        .map(|scene| {
            json!({
                "slug": scene.slug(),
                "title": scene.title(),
                "subtitle": scene.subtitle(),
            })
        })
        .collect();
    json!({ "enabled": explore.enabled, "hidden": explore.hidden, "scenes": scenes })
}

/// `GET /api/videos/stations/settings`. The Explore scenes and which are on.
pub(super) async fn get_video_station_settings(
    State(state): State<SharedState>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    let db = { state.read().await.db.clone() };
    let explore = db
        .with_conn(settings::load)
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(explore_settings_json(&explore)))
}

/// `PUT /api/videos/stations/settings`. Saves the choice and drops today's
/// lineup, so the next visit rebuilds it with only the scenes left on.
pub(super) async fn put_video_station_settings(
    State(state): State<SharedState>,
    Json(body): Json<settings::ExploreSettings>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    let db = { state.read().await.db.clone() };
    let day = today();
    let saved = db
        .with_conn(|conn| {
            let saved = settings::save(conn, &body)?;
            lineup::clear_day(conn, &day)?;
            Ok(saved)
        })
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(explore_settings_json(&saved)))
}
