//! Runs every stage in order and records each in video_intel_runs.
//!
//! A build writes one `build` row (its `rows` is the catalog size the build
//! saw, which the job compares to decide when the catalog grew enough), then
//! one row per stage with its row count or error. A failing stage is logged
//! and the build moves on: every stage reads whatever the earlier ones left,
//! so a broken atlas stage never blocks songs, era or the link pass.

use std::time::Instant;

use anyhow::Result;
use rusqlite::{Connection, params};

use super::{concerts, cuts, era, layout, link, scenes, songs, spread};

/// One stage's outcome.
#[derive(Debug, Clone)]
pub struct StageReport {
    pub stage: &'static str,
    pub rows: usize,
    pub ms: u128,
    pub error: Option<String>,
}

/// Run rows kept: about 25 builds of history.
const KEEP_RUN_ROWS: i64 = 250;

/// Bump when a stage's rules change (markers, best-cut order, era): the job
/// treats a build from another version as due, so an app update rebuilds
/// the derived tables on its own.
pub const BUILD_VERSION: &str = "1";
pub const BUILD_VERSION_KEY: &str = "video_intel.build_version";

fn start(conn: &Connection, stage: &str) -> Result<i64> {
    conn.execute("INSERT INTO video_intel_runs (stage) VALUES (?1)", [stage])?;
    Ok(conn.last_insert_rowid())
}

fn finish(conn: &Connection, id: i64, rows: Option<i64>, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE video_intel_runs SET finished_at = datetime('now'), rows = ?2, error = ?3
          WHERE id = ?1",
        params![id, rows, error],
    )?;
    Ok(())
}

/// Rebuild everything. Order matters: songs read cuts, scenes are needed by
/// layout and link, era reads songs.
pub fn build_all(conn: &Connection) -> Result<Vec<StageReport>> {
    let stages: [(&'static str, fn(&Connection) -> Result<usize>); 8] = [
        ("cuts", cuts::build),
        ("songs", songs::build),
        ("concerts", concerts::build),
        ("scenes", scenes::build),
        ("spread", spread::build),
        ("layout", layout::build),
        ("era", era::build),
        ("link", link::build),
    ];
    let catalog: i64 =
        conn.query_row("SELECT COUNT(*) FROM video_catalog", [], |row| row.get(0))?;
    let build_id = start(conn, "build")?;
    let mut reports = Vec::with_capacity(stages.len());
    for (stage, run) in stages {
        let run_id = start(conn, stage)?;
        let began = Instant::now();
        let outcome = run(conn);
        let ms = began.elapsed().as_millis();
        match outcome {
            Ok(rows) => {
                finish(conn, run_id, Some(rows as i64), None)?;
                reports.push(StageReport {
                    stage,
                    rows,
                    ms,
                    error: None,
                });
            }
            Err(error) => {
                let message = format!("{error:#}");
                tracing::warn!(target: "noor.video_intel", stage, error = %message, "stage failed");
                finish(conn, run_id, None, Some(&message))?;
                reports.push(StageReport {
                    stage,
                    rows: 0,
                    ms,
                    error: Some(message),
                });
            }
        }
    }
    let failed: Vec<&str> = reports
        .iter()
        .filter(|r| r.error.is_some())
        .map(|r| r.stage)
        .collect();
    let summary = (!failed.is_empty()).then(|| format!("failed: {}", failed.join(", ")));
    finish(conn, build_id, Some(catalog), summary.as_deref())?;
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
        params![BUILD_VERSION_KEY, BUILD_VERSION],
    )?;
    conn.execute(
        "DELETE FROM video_intel_runs WHERE id <= (SELECT MAX(id) FROM video_intel_runs) - ?1",
        [KEEP_RUN_ROWS],
    )?;
    Ok(reports)
}

/// A full build over a copy of a real database, with timings, the census and
/// Archive latencies. Never point it at the live database:
/// `NOOR_INTEL_DB=/path/to/copy.db cargo test -p noor-server --release
///  real_catalog_build -- --ignored --nocapture`
#[cfg(test)]
mod real_data {
    use std::collections::HashMap;
    use std::time::Instant;

    use super::build_all;
    use crate::db::Database;
    use crate::services::video_intel::{archive, census, cuts, songs};
    use crate::services::video_sets::VideoCandidate;

    fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
        let began = Instant::now();
        let out = f();
        eprintln!("{label}: {} ms", began.elapsed().as_millis());
        out
    }

    #[test]
    #[ignore = "needs NOOR_INTEL_DB pointing at a copy of a real database"]
    fn real_catalog_build() {
        let Ok(path) = std::env::var("NOOR_INTEL_DB") else {
            eprintln!("NOOR_INTEL_DB not set");
            return;
        };
        let db = Database::open(&path).unwrap();
        db.run_migrations().unwrap();
        let conn = db.open_isolated().unwrap();
        let reports = timed("full build", || build_all(&conn).unwrap());
        for report in &reports {
            eprintln!(
                "  {:9} {:>7} rows {:>6} ms {}",
                report.stage,
                report.rows,
                report.ms,
                report.error.as_deref().unwrap_or("")
            );
        }
        let census = timed("census", || {
            db.with_conn(|c| census::read(c, false)).unwrap()
        });
        eprintln!("{}", serde_json::to_string_pretty(&census).unwrap());
        // The classifier on the station refill path: every catalog title.
        let videos: Vec<VideoCandidate> = db
            .with_conn(|c| {
                let mut stmt = c.prepare("SELECT item_json FROM video_catalog")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for json in rows {
                    if let Ok(video) = serde_json::from_str::<VideoCandidate>(&json?) {
                        out.push(video);
                    }
                }
                Ok(out)
            })
            .unwrap();
        let playable = timed("classify every catalog title", || {
            videos.iter().filter(|v| cuts::is_programmable(v)).count()
        });
        eprintln!(
            "programmable by the shared rule: {playable} of {}",
            videos.len()
        );
        let index = timed("archive index load", || {
            db.with_conn(archive::cached).unwrap()
        });
        eprintln!("archive index rows: {}", index.song_count());
        let cases: &[&[(&str, &str)]] = &[
            &[],
            &[],
            &[("cut", "live")],
            &[("cut", "session"), ("sort", "versions")],
            &[("era_from", "1980"), ("era_to", "1989"), ("sort", "year")],
            &[("depth", "deep"), ("length", "long")],
            &[("q", "en vivo")],
            &[("q", "swear it again"), ("sort", "versions")],
            &[("source", "liked")],
            &[("sort", "newest"), ("offset", "600")],
            &[("sort", "popular"), ("offset", "600")],
            &[("scene", "1"), ("cut", "official"), ("sort", "popular")],
        ];
        for case in cases {
            let params: HashMap<String, String> = case
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let query = archive::Query::from_params(&params);
            let began = Instant::now();
            let response = db
                .with_conn(|c| {
                    let index = archive::cached(c)?;
                    let lights = archive::Lights::load(c)?;
                    archive::query(c, &index, &query, &lights)
                })
                .unwrap();
            eprintln!(
                "archive {:?}: total {} items {} in {} ms; first: {:?}",
                case,
                response.total,
                response.items.len(),
                began.elapsed().as_millis(),
                response.items.first().map(|s| (
                    s.title.as_str(),
                    s.artist_name.as_deref(),
                    s.cut_count
                ))
            );
            if case.is_empty() || case == &[("sort", "popular"), ("offset", "600")] {
                let artists: Vec<&str> = response
                    .items
                    .iter()
                    .take(12)
                    .map(|s| s.artist_name.as_deref().unwrap_or("?"))
                    .collect();
                eprintln!("  first artists: {artists:?}");
            }
        }
        let top: Vec<(String, i64)> = db
            .with_conn(|c| {
                let mut stmt = c.prepare(
                    "SELECT song_key, cut_count FROM video_songs ORDER BY cut_count DESC LIMIT 3",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        for (key, count) in top.iter().take(2) {
            let versions = timed("versions", || {
                db.with_conn(|c| songs::versions(c, key)).unwrap().unwrap()
            });
            eprintln!("{key} ({count} cuts):");
            for v in versions.versions.iter().take(14) {
                eprintln!("  {} | {} | {:?}", v.label, v.video.title, v.cut);
            }
        }
        let labels: Vec<(String, i64, String)> = db
            .with_conn(|c| {
                let mut stmt = c.prepare(
                    "SELECT COALESCE(f.name, ''), vc.video_count, vc.label
                       FROM video_concerts vc
                       LEFT JOIN video_artist_facets f ON f.artist_tidal_id = vc.artist_tidal_id
                      ORDER BY vc.video_count DESC LIMIT 25",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        for (artist, count, label) in labels {
            eprintln!("concert: {artist} x{count}: {label}");
        }
        let eras: Vec<(Option<String>, i64)> = db
            .with_conn(|c| {
                let mut stmt = c.prepare(
                    "SELECT era_source || ' ' || CAST(ROUND(era_conf, 2) AS TEXT), COUNT(*)
                       FROM video_songs GROUP BY 1 ORDER BY 2 DESC",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        eprintln!("era sources: {eras:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_intel::songs::tests::{add, conn};

    #[test]
    fn a_build_records_every_stage_and_the_catalog_size() {
        let conn = conn();
        add(&conn, 1, 7, "Song", 200, None, Some(2020));
        let reports = build_all(&conn).unwrap();
        let names: Vec<&str> = reports.iter().map(|r| r.stage).collect();
        assert_eq!(
            names,
            vec![
                "cuts", "songs", "concerts", "scenes", "spread", "layout", "era", "link"
            ]
        );
        assert!(reports.iter().all(|r| r.error.is_none()));
        let (rows, finished): (i64, Option<String>) = conn
            .query_row(
                "SELECT rows, finished_at FROM video_intel_runs WHERE stage = 'build'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 1);
        assert!(finished.is_some());
        let stage_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM video_intel_runs WHERE finished_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stage_rows, 9);
    }
}
