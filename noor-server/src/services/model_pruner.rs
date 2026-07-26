//! Reclaim space from retired discovery embedding models.
//!
//! Every discovery retrain writes a fresh model plus a full set of per-track
//! neighbours (1-2.5M rows on a real library). `activate_embedding_model` flips
//! `is_active` but nothing ever deleted the previous model's rows, so the
//! neighbour table grew without bound: measured on a live 39k-track library,
//! `track_neighbors` held 18.47M rows of which only 2.49M belonged to the active
//! model. That table is the bulk of an 8.4GB database file.
//!
//! Training is user-initiated (one route handler, no scheduler), so only people
//! who retrain repeatedly accumulate this - but for them it is unbounded.
//!
//! Two entry points, both driving the same batched prune:
//!   - `prune_now` runs after a successful activation, so new retrains stop leaking.
//!   - `spawn_startup_repair` heals existing installs once per boot and then stops.
//!
//! Deleting rows does NOT shrink the file: the database runs with
//! `auto_vacuum = NONE`, so freed pages go on the freelist and are reused by
//! later writes. Returning space to the filesystem needs an explicit VACUUM,
//! which is why that is a deliberate user action rather than something that runs
//! unattended (see `compact_database`).

use crate::db::{Database, queries};
use std::time::Duration;

/// Pause between delete batches so the shared connection stays available to
/// request handlers. The repair is never urgent; staying invisible matters more.
const BATCH_PAUSE: Duration = Duration::from_millis(150);
/// Safety stop so a bug can never spin forever. At 20k rows per batch this
/// covers ~40M neighbour rows, well past anything observed.
const MAX_BATCHES: usize = 2_000;
/// Batches the startup repair will do before leaving the rest alone.
///
/// Row-by-row deletion is bound by index maintenance (~5k rows/sec measured), so
/// clearing a large historical backlog this way takes about an hour. That is the
/// wrong tool: Compact clears the same backlog in a couple of minutes by dropping
/// the indexes first. The startup pass therefore only nibbles - enough to keep an
/// ordinary install tidy over a few launches, while a big backlog waits for the
/// user to press Compact (the Settings panel reports how much is pending).
const STARTUP_MAX_BATCHES: usize = 25;

/// Outcome of a prune pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    pub neighbors_deleted: usize,
    pub models_deleted: usize,
}

/// Drive the batched prune to completion. Yields between batches. Safe to call
/// when there is nothing to do: it costs one indexed lookup and returns zeroes.
pub async fn prune_now(db: &Database, keep: usize) -> anyhow::Result<PruneOutcome> {
    prune_bounded(db, keep, MAX_BATCHES).await
}

/// `prune_now` with an explicit batch ceiling. Stopping early is not a failure:
/// whatever is left is simply pruned by a later pass or by Compact.
pub async fn prune_bounded(
    db: &Database,
    keep: usize,
    max_batches: usize,
) -> anyhow::Result<PruneOutcome> {
    let mut outcome = PruneOutcome::default();

    // Without an index on model_id every batch rescans the whole table, which
    // put the measured throughput at ~4k rows/sec. Build it once up front.
    db.with_conn(queries::ensure_track_neighbors_model_index)?;

    for _ in 0..max_batches {
        let deleted =
            db.with_conn(|conn| queries::prune_retired_model_neighbors_batch(conn, keep))?;
        if deleted == 0 {
            break;
        }
        outcome.neighbors_deleted += deleted;
        tokio::time::sleep(BATCH_PAUSE).await;
    }

    // Only retire the model rows once their neighbours are actually gone;
    // otherwise a bounded pass would cascade-delete millions of rows in one
    // statement, which is the cost this batching exists to avoid.
    let neighbours_remaining: i64 = db.with_conn(|conn| {
        let retired = queries::retired_embedding_model_ids(conn, keep)?;
        if retired.is_empty() {
            return Ok(0);
        }
        let placeholders = vec!["?"; retired.len()].join(",");
        Ok(conn.query_row(
            &format!("SELECT COUNT(*) FROM track_neighbors WHERE model_id IN ({placeholders})"),
            rusqlite::params_from_iter(retired.iter()),
            |r| r.get::<_, i64>(0),
        )?)
    })?;
    if neighbours_remaining == 0 {
        outcome.models_deleted =
            db.with_conn(|conn| queries::delete_retired_embedding_models(conn, keep))?;
    }

    if outcome.neighbors_deleted > 0 || outcome.models_deleted > 0 {
        tracing::info!(
            neighbors = outcome.neighbors_deleted,
            models = outcome.models_deleted,
            "model_pruner: reclaimed rows from retired discovery models"
        );
    }
    Ok(outcome)
}

/// One-shot repair for databases that predate the prune-on-activation fix.
/// Self-terminating: it runs the same prune once and the task ends. Installs
/// with nothing to clean pay a single indexed lookup.
pub fn spawn_startup_repair(db: Database) {
    tokio::spawn(async move {
        match prune_bounded(&db, queries::EMBEDDING_MODELS_KEPT, STARTUP_MAX_BATCHES).await {
            Ok(outcome) if outcome.neighbors_deleted > 0 => {
                tracing::info!(
                    neighbors = outcome.neighbors_deleted,
                    models = outcome.models_deleted,
                    "model_pruner: startup repair complete (run Compact database in Settings to return the space to disk)"
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "model_pruner: startup repair failed"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn db_with_models() -> Database {
        let db = Database::open_in_memory().expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| schema::run_migrations(conn))
            .expect("schema migrations");
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, file_path) VALUES
                    (100, 'One', 1, '/1'), (101, 'Two', 1, '/2')",
                [],
            )?;
            // Three models: 1 and 2 retired (2 is the most recent retired one and
            // is kept for rollback), 3 active.
            conn.execute(
                "INSERT INTO embedding_models (id, model_key, family, dimension, is_active, trained_at) VALUES
                    (1, 'old', 'f', 64, 0, '2026-01-01'),
                    (2, 'prev', 'f', 64, 0, '2026-02-01'),
                    (3, 'active', 'f', 64, 1, '2026-03-01')",
                [],
            )?;
            for model in [1, 2, 3] {
                for (rank, neighbor) in [100, 101].into_iter().enumerate() {
                    conn.execute(
                        "INSERT INTO track_neighbors (track_id, neighbor_track_id, model_id, rank, score)
                         VALUES (?1, ?2, ?3, ?4, 0.5)",
                        rusqlite::params![100 + (model % 2), neighbor, model, rank as i64],
                    )?;
                }
                conn.execute(
                    "INSERT INTO track_embeddings (track_id, model_id, vector_blob, l2_norm)
                     VALUES (100, ?1, X'00', 1.0)",
                    rusqlite::params![model],
                )?;
            }
            Ok(())
        })
        .expect("fixture");
        db
    }

    fn model_ids(db: &Database) -> Vec<i64> {
        db.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM embedding_models ORDER BY id")?;
            let ids = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .expect("ids")
    }

    fn neighbor_models(db: &Database) -> Vec<i64> {
        db.with_conn(|conn| {
            let mut stmt = prepare_distinct(conn)?;
            let ids = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .expect("neighbor models")
    }

    fn prepare_distinct(conn: &rusqlite::Connection) -> anyhow::Result<rusqlite::Statement<'_>> {
        Ok(conn.prepare("SELECT DISTINCT model_id FROM track_neighbors ORDER BY model_id")?)
    }

    #[tokio::test]
    async fn prune_keeps_the_active_model_and_one_rollback() {
        let db = db_with_models();

        let outcome = prune_now(&db, 1).await.expect("prune");

        assert_eq!(outcome.models_deleted, 1, "only model 1 is fully retired");
        assert!(outcome.neighbors_deleted > 0);
        assert_eq!(model_ids(&db), vec![2, 3], "active + one rollback kept");
        assert_eq!(neighbor_models(&db), vec![2, 3]);
    }

    #[tokio::test]
    async fn prune_removes_embeddings_by_cascade() {
        let db = db_with_models();
        prune_now(&db, 1).await.expect("prune");

        let remaining = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM track_embeddings WHERE model_id = 1",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .expect("count");
        assert_eq!(remaining, 0, "retired model vectors cascade away");
    }

    #[tokio::test]
    async fn prune_is_a_no_op_when_there_is_nothing_retired() {
        let db = db_with_models();
        prune_now(&db, 1).await.expect("first prune");

        let second = prune_now(&db, 1).await.expect("second prune");
        assert_eq!(second, PruneOutcome::default(), "idempotent");
    }

    #[test]
    fn bulk_prune_clears_everything_and_puts_the_indexes_back() {
        let db = db_with_models();

        let before: Vec<String> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT name FROM sqlite_master WHERE type='index'
                       AND tbl_name='track_neighbors' AND sql IS NOT NULL ORDER BY name",
                )?;
                Ok(stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .expect("index names");
        assert!(!before.is_empty(), "fixture should have secondary indexes");

        let deleted = db
            .with_conn(|conn| queries::prune_retired_models_bulk(conn, 1))
            .expect("bulk prune");
        assert!(deleted > 0);

        let after: Vec<String> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT name FROM sqlite_master WHERE type='index'
                       AND tbl_name='track_neighbors' AND sql IS NOT NULL ORDER BY name",
                )?;
                Ok(stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .expect("index names");
        assert_eq!(before, after, "indexes are rebuilt after the bulk delete");
        assert_eq!(model_ids(&db), vec![2, 3], "active + one rollback kept");
        assert_eq!(neighbor_models(&db), vec![2, 3]);
    }

    #[tokio::test]
    async fn prune_never_touches_the_active_model() {
        let db = db_with_models();
        // keep = 0 retires everything that is not active.
        prune_now(&db, 0).await.expect("prune");

        assert_eq!(model_ids(&db), vec![3], "the active model always survives");
        assert_eq!(neighbor_models(&db), vec![3]);
    }
}
