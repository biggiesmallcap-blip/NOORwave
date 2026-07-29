//! Self-healing catalogue-name folding.
//!
//! Migration 060 adds `name_normalized` / `title_normalized` to artists, albums
//! and tracks, but leaves every value NULL: the fold is NFKD-based and SQLite
//! has no such function, so the column cannot be populated by the SQL that adds
//! it. A shipped app cannot have its users' databases fixed by hand either, so
//! this pass fills them in from Rust instead, on a schedule, and stops on its
//! own once nothing is left.
//!
//! New rows are also written with a NULL fold rather than folded inline. There
//! are around fifteen insert sites across import, radio and the resolvers, and
//! one missed site would be an invisible hole. Leaving the column NULL and
//! letting this pass catch up means there is exactly one place that knows how
//! to fold, and the fallback while it catches up is the exact-name match the
//! resolvers used before any of this existed - so a gap costs nothing that was
//! not already being paid.
//!
//! Wired next to `auto_enrich::run_if_idle` in `main.rs`: on `LibrarySynced`,
//! which is when new rows appear, and on the daily catch-up.

use crate::SharedState;
use crate::db::catalog_name;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

/// Cap on passes per trigger. Each pass folds `BACKFILL_BATCH` rows per table,
/// so this chips a very large library down over a few triggers rather than
/// holding the write lock for one long stretch.
const MAX_PASSES_PER_RUN: usize = 25;

static RUNNING: AtomicBool = AtomicBool::new(false);

pub async fn run_if_idle(state: SharedState) {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };

    if RUNNING.swap(true, Ordering::SeqCst) {
        debug!(target: "noor.catalog_name", "already running, skipping");
        return;
    }

    let result = tokio::task::spawn_blocking(move || {
        let outcome = db.with_conn(|conn| {
            let pending = catalog_name::pending_normalized_names(conn)?;
            if pending == 0 {
                return Ok::<_, anyhow::Error>((0usize, 0i64));
            }
            let written = catalog_name::run_backfill_to_completion(conn, MAX_PASSES_PER_RUN)?;
            let remaining = catalog_name::pending_normalized_names(conn)?;
            Ok((written, remaining))
        });
        RUNNING.store(false, Ordering::SeqCst);
        outcome
    })
    .await;

    match result {
        Ok(Ok((0, _))) => {
            debug!(target: "noor.catalog_name", "nothing to fold");
        }
        Ok(Ok((written, remaining))) => {
            info!(
                target: "noor.catalog_name",
                written,
                remaining,
                "Folded catalogue names"
            );
        }
        Ok(Err(e)) => {
            warn!(target: "noor.catalog_name", error = %e, "catalogue name backfill failed");
        }
        Err(e) => {
            // spawn_blocking panicked; the guard is released inside the closure
            // on the normal path only, so clear it here too.
            RUNNING.store(false, Ordering::SeqCst);
            warn!(target: "noor.catalog_name", error = %e, "catalogue name backfill task panicked");
        }
    }
}
