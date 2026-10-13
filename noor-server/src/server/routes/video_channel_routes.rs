//! On Air: numbered channels on a daily clock, the guide, tune-in at the
//! scheduled offset, refills, and the explore knob. Local reads only; the
//! first request of a day builds every channel's schedule (under 2 s) on its
//! own connection, later ones read the stored day.
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::SharedState;
use crate::db::Database;
use crate::services::video_air::{
    self, ExploreMode,
    channels::ChannelId,
    clock,
    day::{self, Tune},
    session::{self, NextRequest},
    steer,
};

type Reply = (StatusCode, Json<Value>);

/// Schedule work can scan the whole catalog, so it runs on its own
/// connection and leaves the shared one free; in-memory test databases fall
/// back to it.
fn with_own_conn<T>(
    db: &Database,
    f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    match db.open_isolated() {
        Ok(conn) => f(&conn),
        Err(_) => db.with_conn(f),
    }
}

async fn blocking<T: Send + 'static>(
    db: Database,
    f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(move || with_own_conn(&db, f))
        .await
        .map_err(|error| anyhow::anyhow!("on air task failed: {error}"))?
}

fn failed(error: anyhow::Error, what: &str) -> Reply {
    tracing::warn!(target: "noor.video_air", %error, "{what} failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": format!("{what} failed") })),
    )
}

fn ok(value: impl serde::Serialize) -> Reply {
    match serde_json::to_value(value) {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => failed(error.into(), "serialize"),
    }
}

/// GET /api/videos/channels
pub(super) async fn get_video_channels(State(state): State<SharedState>) -> Reply {
    let db = { state.read().await.db.clone() };
    let (today, now_s) = clock::local_now();
    let result = blocking(db, move |conn| {
        let explore = video_air::load_explore(conn)?;
        day::channels_at(conn, &today, now_s, explore)
    })
    .await;
    match result {
        Ok(out) => ok(out),
        Err(error) => failed(error, "channels"),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct GuideQuery {
    from_s: Option<i64>,
    to_s: Option<i64>,
}

/// GET /api/videos/channels/{id}/guide?from_s&to_s. Defaults to the whole
/// day; `to_s` past 86400 reaches into tomorrow.
pub(super) async fn get_video_channel_guide(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(query): Query<GuideQuery>,
) -> Reply {
    let db = { state.read().await.db.clone() };
    let (today, _) = clock::local_now();
    let from_s = query.from_s.unwrap_or(0).clamp(0, 2 * clock::DAY_S);
    let to_s = query
        .to_s
        .unwrap_or(clock::DAY_S)
        .clamp(from_s, 2 * clock::DAY_S);
    let result = blocking(db, move |conn| {
        let explore = video_air::load_explore(conn)?;
        day::guide_at(conn, &id, &today, from_s, to_s, explore)
    })
    .await;
    match result {
        Ok(Some(out)) => ok(out),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown channel" })),
        ),
        Err(error) => failed(error, "guide"),
    }
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct TuneRequest {
    #[serde(default)]
    explore: Option<String>,
}

/// POST /api/videos/channels/{id}/tune. The scheduled video, the offset to
/// start at, and the next 12 on the schedule. `explore` (optional) is
/// validated, but the stored knob decides the schedule: a client that has
/// not loaded the knob yet must not reset it. PUT /api/videos/explore turns
/// it.
pub(super) async fn post_video_channel_tune(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Reply {
    let request: TuneRequest = if body.iter().all(u8::is_ascii_whitespace) {
        TuneRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "invalid body" })),
                );
            }
        }
    };
    if request
        .explore
        .as_deref()
        .is_some_and(|raw| ExploreMode::parse(raw).is_none())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "explore must be familiar, mixed or adventurous" })),
        );
    }
    let db = { state.read().await.db.clone() };
    let (today, now_s) = clock::local_now();
    let result = blocking(db, move |conn| {
        let explore = video_air::load_explore(conn)?;
        day::tune_at(conn, &id, &today, now_s, explore)
    })
    .await;
    match result {
        Ok(Tune::On(out)) => ok(out),
        Ok(Tune::Unknown) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown channel" })),
        ),
        Ok(Tune::Locked) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "channel locked" })),
        ),
        Ok(Tune::OffAir) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "nothing on air" })),
        ),
        Err(error) => failed(error, "tune"),
    }
}

/// POST /api/videos/channels/{id}/next. A refill after the schedule for a
/// tuned-in session: never a video or song in `exclude_video_ids`, shaped by
/// the session's zaps and holds and its +/- artist presses.
pub(super) async fn post_video_channel_next(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<NextRequest>,
) -> Reply {
    let db = { state.read().await.db.clone() };
    let (today, _) = clock::local_now();
    let result = blocking(db, move |conn| {
        let explore = video_air::load_explore(conn)?;
        let (universe, taste) = day::shared(conn)?;
        session::refill(conn, &id, &today, explore, &body, &universe, &taste)
    })
    .await;
    match result {
        Ok(items) => ok(json!({ "exhausted": items.is_empty(), "items": items })),
        Err(error) => failed(error, "refill"),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct SteerBody {
    artist_id: i64,
    #[serde(default)]
    artist_name: Option<String>,
    more: bool,
    /// Everything the session has queued or played, so More lines up
    /// nothing it already has.
    #[serde(default)]
    exclude_video_ids: Vec<i64>,
}

/// POST /api/videos/channels/{id}/steer. More or Less like the artist on
/// screen. Remembered as a small lean that fades within four weeks
/// (steer.rs); for More, also the videos to line up right after the playing
/// one: a neighbour, the artist, another neighbour. Less lines up nothing:
/// the client takes the artist's videos out of its queue.
pub(super) async fn post_video_channel_steer(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<SteerBody>,
) -> Reply {
    if body.artist_id <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "artist_id must be a TIDAL artist id" })),
        );
    }
    if ChannelId::parse(&id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown channel" })),
        );
    }
    let db = { state.read().await.db.clone() };
    let now = chrono::Utc::now().timestamp();
    let result = blocking(db, move |conn| {
        steer::record(
            conn,
            body.artist_id,
            body.artist_name.as_deref(),
            body.more,
            now,
        )?;
        if !body.more {
            return Ok(Vec::new());
        }
        let (universe, taste) = day::shared(conn)?;
        let queued: Vec<i64> = body
            .exclude_video_ids
            .iter()
            .copied()
            .filter(|id| *id > 0)
            .take(512)
            .collect();
        let name = body
            .artist_name
            .clone()
            .or_else(|| taste.names.get(&body.artist_id).cloned())
            .unwrap_or_default();
        let picks = steer::more_picks(&universe, &taste.graph, body.artist_id, &name, &queued);
        let ids: Vec<i64> = picks.iter().map(|(id, _)| *id).collect();
        let videos = day::catalog_videos(conn, &ids)?;
        Ok(picks
            .iter()
            .filter_map(|(id, why)| videos.get(id).map(|video| day::item(video, why)))
            .collect::<Vec<_>>())
    })
    .await;
    match result {
        Ok(items) => ok(json!({ "items": items, "remembered": true })),
        Err(error) => failed(error, "steer"),
    }
}

/// GET /api/videos/steers. Every remembered More and Less still in force,
/// newest first, with how strong it still is and when it will be gone.
pub(super) async fn get_video_steers(State(state): State<SharedState>) -> Reply {
    let db = { state.read().await.db.clone() };
    let now = chrono::Utc::now().timestamp();
    match db.with_conn(|conn| steer::load(conn, now)) {
        Ok(steers) => ok(json!({
            "items": steers
                .iter()
                .map(|steer| json!({
                    "artist_id": steer.artist,
                    "name": steer.name,
                    "more": steer.more,
                    "at": steer.at,
                    "strength": (steer.strength(now) * 100.0).round() / 100.0,
                    "forgotten_at": steer.forgotten_at(),
                }))
                .collect::<Vec<_>>(),
        })),
        Err(error) => failed(error, "steers"),
    }
}

/// DELETE /api/videos/steers/{artist_id}. Undo a remembered press.
pub(super) async fn delete_video_steer(
    State(state): State<SharedState>,
    Path(artist_id): Path<i64>,
) -> Reply {
    let db = { state.read().await.db.clone() };
    let now = chrono::Utc::now().timestamp();
    match db.with_conn(|conn| steer::forget(conn, artist_id, now)) {
        Ok(forgotten) => ok(json!({ "forgotten": forgotten })),
        Err(error) => failed(error, "steer undo"),
    }
}

/// GET /api/videos/explore
pub(super) async fn get_video_explore(State(state): State<SharedState>) -> Reply {
    let db = { state.read().await.db.clone() };
    match db.with_conn(video_air::load_explore) {
        Ok(mode) => ok(json!({ "explore": mode })),
        Err(error) => failed(error, "explore"),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ExploreBody {
    explore: String,
}

/// PUT /api/videos/explore. Saved in server_config; today's schedule is
/// rebuilt with the new lean on the next channels request.
pub(super) async fn put_video_explore(
    State(state): State<SharedState>,
    Json(body): Json<ExploreBody>,
) -> Reply {
    let Some(mode) = ExploreMode::parse(&body.explore) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "explore must be familiar, mixed or adventurous" })),
        );
    };
    let db = { state.read().await.db.clone() };
    match db.with_conn(|conn| video_air::save_explore(conn, mode)) {
        Ok(()) => ok(json!({ "explore": mode })),
        Err(error) => failed(error, "explore"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::services::video_air::test_support;

    fn state(db: Database) -> SharedState {
        Arc::new(tokio::sync::RwLock::new(
            crate::server::routes::tests::fresh_test_state(db),
        ))
    }

    fn seeded() -> Database {
        let db = crate::server::routes::tests::fresh_migrated_db();
        db.with_conn(|conn| {
            for artist in 0..60 {
                for n in 0..12 {
                    test_support::add_video(
                        conn,
                        10_000 + artist * 100 + n,
                        500 + artist,
                        "official",
                        &format!("song {n}"),
                        240,
                    );
                }
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[tokio::test]
    async fn channels_list_the_dial_with_now_and_next() {
        let db = seeded();
        let (status, Json(body)) = tokio::time::timeout(
            Duration::from_secs(10),
            get_video_channels(State(state(db))),
        )
        .await
        .expect("answers without TIDAL");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["explore"], "mixed");
        assert_eq!(body["building"], false);
        assert!(body["now_s"].is_i64());
        let channels = body["channels"].as_array().unwrap();
        let numbers: Vec<i64> = channels.iter().map(|c| c["no"].as_i64().unwrap()).collect();
        assert_eq!(numbers, (1..=8).collect::<Vec<_>>());
        let wild = channels.iter().find(|c| c["id"] == "wild").unwrap();
        assert_eq!(wild["locked"], false);
        assert!(wild["now"]["video"]["tidal_id"].is_i64());
        assert!(wild["now"]["offset_s"].is_i64());
        assert!(wild["next"]["start_s"].is_i64());
    }

    #[tokio::test]
    async fn an_empty_catalog_answers_building_with_every_channel_locked() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        let (status, Json(body)) = get_video_channels(State(state(db))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["building"], true);
        assert!(
            body["channels"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["locked"] == true && c["now"].is_null())
        );
    }

    #[tokio::test]
    async fn tune_guide_and_next_answer_for_a_live_channel() {
        let db = seeded();
        let shared = state(db);
        let (status, Json(tune)) = post_video_channel_tune(
            State(shared.clone()),
            Path("wild".into()),
            Bytes::from_static(b"{}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(tune["video"]["tidal_id"].is_i64());
        assert!(tune["offset_s"].as_i64().unwrap() >= 0);
        assert_eq!(tune["queue"].as_array().unwrap().len(), 12);
        assert_eq!(tune["channel"]["id"], "wild");

        let (status, Json(guide)) = get_video_channel_guide(
            State(shared.clone()),
            Path("wild".into()),
            Query(GuideQuery {
                from_s: Some(0),
                to_s: Some(3_600),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(guide["channel_id"], "wild");
        assert!(!guide["programs"].as_array().unwrap().is_empty());

        let queued: Vec<i64> = tune["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["tidal_id"].as_i64().unwrap())
            .collect();
        let (status, Json(next)) = post_video_channel_next(
            State(shared.clone()),
            Path("wild".into()),
            Json(NextRequest {
                exclude_video_ids: queued.clone(),
                session_nonce: "s1".into(),
                more_artist_ids: vec![],
                less_artist_ids: vec![],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(next["exhausted"], false);
        let items = next["items"].as_array().unwrap();
        assert_eq!(items.len(), 12);
        assert!(
            items
                .iter()
                .all(|v| !queued.contains(&v["tidal_id"].as_i64().unwrap()))
        );
    }

    #[tokio::test]
    async fn tune_rejects_locked_unknown_and_bad_bodies() {
        let db = seeded();
        let shared = state(db);
        let (status, _) =
            post_video_channel_tune(State(shared.clone()), Path("live".into()), Bytes::new()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) =
            post_video_channel_tune(State(shared.clone()), Path("nowhere".into()), Bytes::new())
                .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = post_video_channel_tune(
            State(shared.clone()),
            Path("wild".into()),
            Bytes::from_static(b"{\"explore\":\"loud\"}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = get_video_channel_guide(
            State(shared),
            Path("nowhere".into()),
            Query(GuideQuery {
                from_s: None,
                to_s: None,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn steering_lines_up_more_remembers_and_undoes() {
        let db = seeded();
        let shared = state(db);
        let body = |artist_id: i64, more: bool| SteerBody {
            artist_id,
            artist_name: Some("Five Hundred".into()),
            more,
            exclude_video_ids: vec![10_000],
        };
        let (status, Json(out)) = post_video_channel_steer(
            State(shared.clone()),
            Path("wild".into()),
            Json(body(500, true)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = out["items"].as_array().unwrap();
        assert!(!items.is_empty());
        assert!(items.iter().all(|v| v["tidal_id"].as_i64() != Some(10_000)));
        assert_eq!(items[0]["why"], "More Five Hundred");

        let (status, Json(out)) = post_video_channel_steer(
            State(shared.clone()),
            Path("wild".into()),
            Json(body(501, false)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(out["items"].as_array().unwrap().is_empty());

        let (_, Json(list)) = get_video_steers(State(shared.clone())).await;
        let rows = list["items"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["artist_id"], 501);
        assert_eq!(rows[0]["more"], false);
        assert_eq!(rows[1]["name"], "Five Hundred");

        let (_, Json(out)) = delete_video_steer(State(shared.clone()), Path(501)).await;
        assert_eq!(out["forgotten"], true);
        let (_, Json(list)) = get_video_steers(State(shared.clone())).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);

        let (status, _) = post_video_channel_steer(
            State(shared.clone()),
            Path("nowhere".into()),
            Json(body(500, true)),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) =
            post_video_channel_steer(State(shared), Path("wild".into()), Json(body(0, true))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn explore_persists_and_validates() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        let shared = state(db.clone());
        let (_, Json(body)) = get_video_explore(State(shared.clone())).await;
        assert_eq!(body["explore"], "mixed");
        let (status, Json(body)) = put_video_explore(
            State(shared.clone()),
            Json(ExploreBody {
                explore: "adventurous".into(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["explore"], "adventurous");
        let (_, Json(body)) = get_video_explore(State(shared.clone())).await;
        assert_eq!(body["explore"], "adventurous");
        assert_eq!(
            db.with_conn(video_air::load_explore).unwrap(),
            ExploreMode::Adventurous
        );
        let (status, _) = put_video_explore(
            State(shared.clone()),
            Json(ExploreBody {
                explore: "loud".into(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (_, Json(body)) = get_video_explore(State(shared.clone())).await;
        assert_eq!(body["explore"], "adventurous");
        // A tune never turns the knob.
        let (_, _) = post_video_channel_tune(
            State(shared.clone()),
            Path("wild".into()),
            Bytes::from_static(b"{\"explore\":\"familiar\"}"),
        )
        .await;
        let (_, Json(body)) = get_video_explore(State(shared)).await;
        assert_eq!(body["explore"], "adventurous");
    }
}
