//! The Atlas's two looks: Picture (every artist's thumbnail colour, filled
//! by a background job) and You (footprints, trails and the frontier).
//! Local reads only. Wire shapes: frontend/src/lib/api/video_explore_types.ts.
use axum::{extract::State, response::Json};

use super::video_atlas_routes::{ApiError, read};
use crate::SharedState;
use crate::services::video_intel::{pictures, you};

/// GET /api/videos/atlas/pictures
pub(super) async fn get_video_atlas_pictures(
    State(state): State<SharedState>,
) -> Result<Json<pictures::Pictures>, ApiError> {
    read(&state, pictures::read).await.map(Json)
}

/// GET /api/videos/atlas/you
pub(super) async fn get_video_atlas_you(
    State(state): State<SharedState>,
) -> Result<Json<you::You>, ApiError> {
    let now = chrono::Utc::now().timestamp();
    read(&state, move |conn| you::read(conn, now))
        .await
        .map(Json)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::api_routes;
    use super::super::tests::{fresh_migrated_db, fresh_test_state};
    use crate::db::Database;
    use crate::services::video_intel::pictures::{
        Fetch, HttpFetch, Outcome, Pending, record, thumb_url,
    };

    async fn get(db: &Database, uri: &str) -> (StatusCode, Value, usize) {
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
            bytes.len(),
        )
    }

    #[tokio::test]
    async fn pictures_are_parallel_arrays_of_the_done_rows() {
        let db = fresh_migrated_db();
        let (status, empty, _) = get(&db, "/api/videos/atlas/pictures").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            empty,
            json!({ "ready": 0, "total": 0, "ids": [], "rgb": [], "art": [] })
        );
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO video_artist_pictures (artist_tidal_id, video_id, art, rgb, status) VALUES
                     (456, 2, '541db133/1f4e/4640/afdf/e7f6ed00eb3d', 3355443, 'done'),
                     (123, 1, '6a0ff657/cfc9/47f6/9270/c84d128eeb09', 16737095, 'done'),
                     (789, 3, 'f248d268/a998/42f8/a16d/6bb53cf02c1b', NULL, 'pending');",
            )?;
            Ok(())
        })
        .unwrap();
        let (_, pictures, _) = get(&db, "/api/videos/atlas/pictures").await;
        assert_eq!(
            pictures,
            json!({
                "ready": 2, "total": 3,
                "ids": [123, 456],
                "rgb": [16737095, 3355443],
                "art": ["6a0ff657/cfc9/47f6/9270/c84d128eeb09", "541db133/1f4e/4640/afdf/e7f6ed00eb3d"],
            })
        );
    }

    #[tokio::test]
    async fn you_has_footprints_trails_and_a_frontier() {
        let db = fresh_migrated_db();
        let (status, empty, _) = get(&db, "/api/videos/atlas/you").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            empty,
            json!({ "footprints": [], "trails": [], "frontier": [] })
        );
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO video_history (tidal_video_id, artist_tidal_id, started_at, duration_watched_ms) VALUES
                     (1, 1, datetime('now', '-1 hour'), 200000),
                     (2, 2, datetime('now', '-1 hour', '+4 minutes'), 200000);
                 INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                     VALUES (2, 5, 'x', 'lastfm', 0.82);
                 INSERT INTO video_artist_facets (artist_tidal_id, name, x, y) VALUES
                     (1, 'One', 1, 1), (2, 'Two', 2, 2), (5, 'Five', 3, 3);",
            )?;
            Ok(())
        })
        .unwrap();
        let (_, you, _) = get(&db, "/api/videos/atlas/you").await;
        let footprints = you["footprints"].as_array().unwrap();
        assert_eq!(footprints.len(), 2);
        assert_eq!(footprints[0]["artist_id"], 1);
        assert_eq!(footprints[0]["plays"], 1);
        assert!(footprints[0]["last_at"].as_i64().unwrap() > 1_700_000_000);
        assert_eq!(you["trails"][0]["artists"], json!([1, 2]));
        assert!(you["trails"][0]["ended_at"].as_i64().unwrap() > 1_700_000_000);
        let frontier = you["frontier"].as_array().unwrap();
        assert_eq!(frontier.len(), 1);
        assert_eq!(
            (&frontier[0]["artist_id"], &frontier[0]["via"]),
            (&json!(5), &json!(2))
        );
        // Edge weight 0.82, the footprint an hour old.
        let score = frontier[0]["score"].as_f64().unwrap();
        assert!((0.81..=0.82).contains(&score), "{score}");
    }

    /// Payload sizes and latencies over a copy of a real database:
    ///   NOOR_INTEL_DB=<copy> cargo test -p noor-server --bin noor-server real_catalog_atlas_looks -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "needs NOOR_INTEL_DB pointing at a copy of a real database"]
    async fn real_catalog_atlas_looks() {
        let Ok(path) = std::env::var("NOOR_INTEL_DB") else {
            eprintln!("NOOR_INTEL_DB not set");
            return;
        };
        let db = Database::open(&path).unwrap();
        db.run_migrations().unwrap();
        let started = std::time::Instant::now();
        let report = db
            .with_conn(crate::services::video_intel::pictures::seed)
            .unwrap();
        eprintln!("seed: {} ms, {report:?}", started.elapsed().as_millis());
        let started = std::time::Instant::now();
        let report = db
            .with_conn(crate::services::video_intel::pictures::seed)
            .unwrap();
        eprintln!("re-seed: {} ms, {report:?}", started.elapsed().as_millis());

        // The real pipeline on the top artist of 20 scenes: fetch, decode,
        // colour, record.
        let picks: Vec<(Pending, String, String)> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT p.artist_tidal_id, p.art, f.name, s.name
                       FROM video_scenes s
                       JOIN video_artist_facets f ON f.artist_tidal_id = (
                            SELECT a.artist_tidal_id FROM video_artist_facets a
                              JOIN video_artist_pictures q ON q.artist_tidal_id = a.artist_tidal_id
                             WHERE a.scene_id = s.scene_id
                             ORDER BY a.videos DESC LIMIT 1)
                       JOIN video_artist_pictures p ON p.artist_tidal_id = f.artist_tidal_id
                      ORDER BY s.video_count DESC LIMIT 20",
                )?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            Pending {
                                artist_id: row.get(0)?,
                                art: row.get(1)?,
                            },
                            row.get(2)?,
                            row.get(3)?,
                        ))
                    })?
                    .collect::<Result<_, _>>()?;
                Ok(rows)
            })
            .unwrap();
        let fetch = HttpFetch(reqwest::Client::new());
        for (item, name, scene) in picks {
            let fetched = fetch
                .fetch(&thumb_url(&item.art))
                .await
                .map_err(|err| format!("{err:?}"));
            let outcome = db.with_conn(|conn| record(conn, &item, fetched)).unwrap();
            let hex = match outcome {
                Outcome::Done(rgb) => format!("#{rgb:06x}"),
                other => format!("{other:?}"),
            };
            eprintln!("  {name} [{scene}] -> {hex}  {}", thumb_url(&item.art));
        }

        // Size as if every artist were done.
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE video_artist_pictures SET status = 'done', rgb = COALESCE(rgb, artist_tidal_id % 16777216)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        // Tests never cache the discovery graph; the server keeps it for
        // ten minutes, so this share of /you is paid once in a while.
        let started = std::time::Instant::now();
        db.with_conn(crate::services::video_discovery::graph::Graph::load)
            .unwrap();
        eprintln!("discovery graph load: {} ms", started.elapsed().as_millis());
        let out = std::env::var("NOOR_LOOKS_OUT").ok();
        for uri in [
            "/api/videos/atlas/pictures",
            "/api/videos/atlas/pictures",
            "/api/videos/atlas/you",
            "/api/videos/atlas/you",
        ] {
            let started = std::time::Instant::now();
            let (status, body, bytes) = get(&db, uri).await;
            eprintln!(
                "{uri}: {status} {} ms, {bytes} bytes",
                started.elapsed().as_millis()
            );
            if uri.ends_with("you") {
                eprintln!(
                    "  footprints {} trails {} frontier {}",
                    body["footprints"].as_array().map_or(0, Vec::len),
                    body["trails"].as_array().map_or(0, Vec::len),
                    body["frontier"].as_array().map_or(0, Vec::len),
                );
            } else {
                eprintln!("  ready {} total {}", body["ready"], body["total"]);
            }
            if let Some(dir) = &out {
                let name = uri.rsplit('/').next().unwrap_or("out");
                std::fs::write(format!("{dir}/{name}.json"), body.to_string()).unwrap();
            }
        }
    }
}
