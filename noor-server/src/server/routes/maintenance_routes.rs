//! Database size reporting and the user-triggered compaction (VACUUM).
//!
//! Deleting rows never shrinks the file: the database runs with
//! `auto_vacuum = NONE`, so freed pages land on the freelist and are reused by
//! later writes. Only VACUUM returns space to the filesystem, and on a real
//! library that means rewriting several GB while holding the connection - too
//! heavy to run unattended, so it is an explicit action with the cost shown up
//! front.

use crate::SharedState;
use crate::db::queries;
use axum::{extract::State, http::StatusCode, response::Json};
use serde_json::{Value, json};

/// Disk cost of one `track_neighbors` row including its share of the six
/// secondary indexes. Measured, not guessed: compacting a real library freed
/// 6,340,902,912 bytes while pruning 14,677,464 rows, which is ~432 bytes each.
///
/// This exists because the freelist alone is a bad answer to "how much can I get
/// back". Retired rows are live rows until they are deleted, so a database
/// carrying 14.9M of them reports ~0 reclaimable and looks healthy right up
/// until it does not.
const BYTES_PER_NEIGHBOR_ROW: i64 = 432;

/// Bytes on disk for the database and its sidecar files. The WAL is included
/// because a large delete can leave a multi-GB WAL that only checkpointing
/// clears, and users see that in the folder too.
fn database_file_bytes(db_path: &str) -> (u64, u64) {
    let main = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let wal = std::fs::metadata(format!("{db_path}-wal"))
        .map(|m| m.len())
        .unwrap_or(0);
    (main, wal)
}

/// GET /api/server/database/stats - what the Settings panel shows before the
/// user decides whether compacting is worth it.
pub(super) async fn get_database_stats(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let db_path = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT file FROM pragma_database_list WHERE name = 'main'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let stats = db
        .with_conn(|conn| {
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let freelist: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            let retired =
                queries::retired_embedding_model_ids(conn, queries::EMBEDDING_MODELS_KEPT)?;
            let retired_rows: i64 = if retired.is_empty() {
                0
            } else {
                let placeholders = vec!["?"; retired.len()].join(",");
                conn.query_row(
                    &format!(
                        "SELECT COUNT(*) FROM track_neighbors WHERE model_id IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(retired.iter()),
                    |r| r.get(0),
                )?
            };
            Ok((page_size, page_count, freelist, retired.len(), retired_rows))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (page_size, page_count, freelist, retired_models, retired_rows) = stats;
    let (file_bytes, wal_bytes) = database_file_bytes(&db_path);

    // What Compact would actually free: pages already on the freelist, plus the
    // retired rows it deletes on the way through.
    let freelist_bytes = freelist * page_size;
    let estimated_reclaimable = freelist_bytes + retired_rows * BYTES_PER_NEIGHBOR_ROW;
    let estimated_after = (file_bytes as i64 - estimated_reclaimable).max(0);

    Ok(Json(json!({
        "file_bytes": file_bytes,
        "wal_bytes": wal_bytes,
        "page_size": page_size,
        "page_count": page_count,
        "freelist_pages": freelist,
        // Free pages only. Near zero on a database whose bloat is retired rows,
        // which is why it must not be shown as the headline number.
        "freelist_bytes": freelist_bytes,
        // Headline: freelist + what the prune inside Compact would remove.
        "estimated_reclaimable_bytes": estimated_reclaimable,
        "estimated_after_bytes": estimated_after,
        "retired_models": retired_models,
        "retired_neighbor_rows": retired_rows,
    })))
}

/// POST /api/server/database/compact - run VACUUM.
///
/// Blocking and slow by nature (it rewrites the whole file and needs roughly its
/// size in free space). Runs on the blocking pool so the async runtime keeps
/// serving, though the shared connection is held for the duration.
pub(super) async fn compact_database(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let db_path = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT file FROM pragma_database_list WHERE name = 'main'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (before, _) = database_file_bytes(&db_path);

    let result = tokio::task::spawn_blocking(move || {
        db.with_conn(|conn| {
            // Clear retired models here rather than waiting for the background
            // trickle. Deleting them row-by-row is index-bound and takes about an
            // hour on a real backlog; the bulk path drops the secondary indexes,
            // deletes in one pass and rebuilds them, which is the difference
            // between "click Compact and wait a few minutes" and "leave the app
            // running for an hour first". This is also the only place that may
            // do it: the user has been told the app will not respond.
            queries::ensure_track_neighbors_model_index(conn)?;
            let pruned = queries::prune_retired_models_bulk(conn, queries::EMBEDDING_MODELS_KEPT)?;

            // Fold the WAL back in first, otherwise VACUUM copies pages that are
            // about to be checkpointed anyway.
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            conn.execute_batch("VACUUM;")?;
            Ok(pruned)
        })
    })
    .await;

    match result {
        Ok(Ok(pruned)) => {
            let (after, _) = database_file_bytes(&db_path);
            tracing::info!(
                before_bytes = before,
                after_bytes = after,
                pruned_rows = pruned,
                "compact_database: prune + VACUUM complete"
            );
            Ok(Json(json!({
                "status": "ok",
                "before_bytes": before,
                "after_bytes": after,
                "reclaimed_bytes": before.saturating_sub(after),
                "pruned_rows": pruned,
            })))
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "compact_database: VACUUM failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(e) => {
            tracing::warn!(error = %e, "compact_database: VACUUM task panicked");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
