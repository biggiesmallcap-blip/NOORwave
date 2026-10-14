//! Background builder: on boot (after a delay) and whenever the catalog has
//! grown enough, rebuild the derived tables on an isolated connection. Never
//! blocks boot; progress is visible in the census.
//!
//! Two minutes after boot, then every six hours, the job rebuilds when the
//! tables were never built, the last build is over a day old, the catalog
//! or the artist graph grew 1% or more since, or an app update changed the
//! build rules (`build::BUILD_VERSION`). One build at a time; `kick` (the
//! rebuild route) skips the checks. A shipped install heals itself this way: every derived
//! row is recomputed from the catalog, nothing needs a migration backfill.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

use super::{archive, build, journey, scenes};
use crate::SharedState;
use crate::db::Database;

const BOOT_DELAY: Duration = Duration::from_secs(120);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const MAX_AGE_HOURS: f64 = 24.0;
const GROWTH: f64 = 1.01;

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Whether a build is running right now.
pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Whether the derived tables need a rebuild.
pub fn due(conn: &Connection) -> Result<bool> {
    let last: Option<(f64, Option<i64>)> = conn
        .query_row(
            "SELECT (julianday('now') - julianday(finished_at)) * 24.0, rows
               FROM video_intel_runs
              WHERE stage = 'build' AND finished_at IS NOT NULL
              ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((age_hours, built_from)) = last else {
        return Ok(true);
    };
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [build::BUILD_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if version.as_deref() != Some(build::BUILD_VERSION) {
        return Ok(true);
    }
    if age_hours >= MAX_AGE_HOURS {
        return Ok(true);
    }
    let catalog: i64 =
        conn.query_row("SELECT COUNT(*) FROM video_catalog", [], |row| row.get(0))?;
    let built_from = built_from.unwrap_or(0);
    if catalog as f64 >= built_from.max(1) as f64 * GROWTH && catalog > built_from {
        return Ok(true);
    }
    // The crawler charting artists adds edges without adding videos.
    let Some(built_edges) = scenes::built_edges(conn)? else {
        return Ok(false);
    };
    let edges = scenes::edge_rows(conn)?;
    Ok(edges as f64 >= built_edges.max(1) as f64 * GROWTH && edges > built_edges)
}

/// Releases the single-flight flag on drop, panics included.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

fn run_blocking(db: &Database, force: bool) -> Result<bool> {
    let conn = db.open_isolated()?;
    if !force && !due(&conn)? {
        // Nothing to rebuild; still load the Archive index off the request
        // path so the first visit after boot is fast.
        warm(&conn);
        return Ok(false);
    }
    let began = Instant::now();
    let reports = build::build_all(&conn)?;
    for report in &reports {
        tracing::info!(
            target: "noor.video_intel",
            stage = report.stage,
            rows = report.rows,
            ms = report.ms as u64,
            failed = report.error.is_some(),
            "video intel stage"
        );
    }
    warm(&conn);
    tracing::info!(
        target: "noor.video_intel",
        elapsed_ms = began.elapsed().as_millis() as u64,
        "video intel build complete"
    );
    Ok(true)
}

/// Load the Archive index and the journey graphs for the current build into
/// their caches.
fn warm(conn: &Connection) {
    match archive::cached(conn) {
        Ok(index) => tracing::debug!(
            target: "noor.video_intel",
            songs = index.song_count(),
            "archive index ready"
        ),
        Err(error) => {
            tracing::warn!(target: "noor.video_intel", %error, "archive index warm-up failed")
        }
    }
    if let Err(error) = journey::cached_collab(conn).and_then(|_| journey::cached_facts(conn)) {
        tracing::warn!(target: "noor.video_intel", %error, "journey warm-up failed");
    }
}

/// Start a build on a blocking thread unless one is running. `force` skips
/// the due check. Returns whether a build was started.
pub fn kick(db: Database, force: bool) -> bool {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    tokio::task::spawn_blocking(move || {
        let _running = Running;
        if let Err(error) = run_blocking(&db, force) {
            tracing::warn!(target: "noor.video_intel", error = %format!("{error:#}"), "video intel build failed");
        }
    });
    true
}

/// Start the background builder.
pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        tokio::time::sleep(BOOT_DELAY).await;
        loop {
            let db = { state.read().await.db.clone() };
            kick(db, false);
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_intel::songs::tests::{add, conn};

    #[test]
    fn due_when_never_built_old_or_grown() {
        let conn = conn();
        add(&conn, 1, 7, "One", 200, None, None);
        assert!(due(&conn).unwrap(), "never built");
        build::build_all(&conn).unwrap();
        assert!(!due(&conn).unwrap(), "fresh");
        add(&conn, 2, 7, "Two", 200, None, None);
        assert!(due(&conn).unwrap(), "grew from 1 to 2 videos");
        build::build_all(&conn).unwrap();
        assert!(!due(&conn).unwrap());
        conn.execute(
            "UPDATE video_intel_runs SET finished_at = datetime('now', '-25 hours')
              WHERE stage = 'build'",
            [],
        )
        .unwrap();
        assert!(due(&conn).unwrap(), "a day old");
        build::build_all(&conn).unwrap();
        conn.execute(
            "UPDATE server_config SET value = '0' WHERE key = ?1",
            [build::BUILD_VERSION_KEY],
        )
        .unwrap();
        assert!(due(&conn).unwrap(), "built by older rules");
    }

    #[test]
    fn graph_growth_is_due_like_catalog_growth() {
        let conn = conn();
        add(&conn, 1, 7, "One", 200, None, None);
        let edge = |seed: i64| {
            conn.execute(
                "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                 VALUES (?1, ?2, 'x', 'lastfm', 0.9)",
                [seed, seed + 1],
            )
            .unwrap();
        };
        for seed in 0..100 {
            edge(1000 + seed * 2);
        }
        build::build_all(&conn).unwrap();
        assert!(!due(&conn).unwrap(), "fresh");
        edge(5000);
        assert!(
            due(&conn).unwrap(),
            "the crawler charted someone: 1% more edges"
        );
    }

    #[test]
    fn small_growth_is_not_due() {
        let conn = conn();
        for id in 0..200 {
            add(&conn, id, 7, &format!("Song {id}"), 200, None, None);
        }
        build::build_all(&conn).unwrap();
        add(&conn, 500, 7, "One more", 200, None, None);
        assert!(!due(&conn).unwrap(), "0.5% growth waits");
    }
}
