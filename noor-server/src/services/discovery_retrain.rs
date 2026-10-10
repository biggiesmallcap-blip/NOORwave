//! One retrain after a trainer upgrade.
//!
//! Models trained before `learning::TRAINER_CONFIG_VERSION` were built on the
//! pair-keyed behavior hash and the word-matching metadata proxy, so their
//! neighbors are wrong, not just old. Installs cannot be repaired by hand, so
//! the hourly maintenance sweep retrains them once, in the background, while
//! the app is idle, at low CPU priority. It stops once a current-version run
//! completes (activated or held back by the gate) or after MAX_ATTEMPTS runs
//! that ended badly, so an install whose training keeps failing does not
//! retrain forever. A run cut short by closing the app does not count; it
//! resumes on a later launch. Never-trained installs are left alone: first
//! training stays the user's choice.

use crate::SharedState;
use crate::db::queries;
use crate::server::routes::{TrainingSpawn, spawn_discovery_training};
use crate::services::learning::{TRAINER_CONFIG_VERSION, trainer_config_version_from_json};
use rusqlite::{Connection, OptionalExtension};
use std::sync::atomic::Ordering;
use tracing::{info, warn};

const MAX_ATTEMPTS: i64 = 3;

/// The latest training run as the decision needs it.
struct LatestRun {
    status: String,
    /// Trainer version of the run's model; None when that model row is gone.
    model_version: Option<i64>,
}

/// Pure decision, split out so it is unit-testable.
fn should_retrain(active_version: Option<i64>, latest: Option<&LatestRun>, attempts: i64) -> bool {
    let Some(active_version) = active_version else {
        return false;
    };
    if active_version >= TRAINER_CONFIG_VERSION {
        return false;
    }
    if let Some(run) = latest {
        if run.status == "running" {
            return false;
        }
        // A current-version run already finished and the gate held it back;
        // retraining the same data would produce the same model.
        if run.status == "completed"
            && run
                .model_version
                .is_some_and(|v| v >= TRAINER_CONFIG_VERSION)
        {
            return false;
        }
    }
    attempts < MAX_ATTEMPTS
}

/// Tries at the current trainer version that ran to an end (completed,
/// failed, timed out or cancelled). A run cut short because the app closed is
/// marked "interrupted by server restart" at the next launch and does not
/// count, so an install that is rarely open long enough simply resumes later.
fn finished_attempts(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM training_runs r
         JOIN embedding_models m ON m.id = r.model_id
         WHERE r.status != 'running'
           AND COALESCE(json_extract(m.config_json, '$.trainer_config_version'), 1) >= ?1
           AND COALESCE(r.error_text, '') != 'interrupted by server restart'",
        [TRAINER_CONFIG_VERSION],
        |row| row.get(0),
    )
}

fn load_latest_run(conn: &Connection) -> anyhow::Result<Option<LatestRun>> {
    let Some(run) = queries::get_latest_training_run(conn)? else {
        return Ok(None);
    };
    let model_version = match run.model_id {
        Some(model_id) => conn
            .query_row(
                "SELECT config_json FROM embedding_models WHERE id = ?1",
                [model_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .map(|config_json| trainer_config_version_from_json(config_json.as_deref())),
        None => None,
    };
    Ok(Some(LatestRun {
        status: run.status,
        model_version,
    }))
}

/// Start the upgrade retrain if the active model predates the current trainer
/// and the app is idle. Returns immediately; called from the hourly sweep.
pub async fn run_if_outdated(state: SharedState) {
    let (db, busy, similarity_running) = {
        let s = state.read().await;
        (
            s.db.clone(),
            crate::services::radio_similarity::busy_reason(&s, &s.db),
            s.radio_similarity_running.load(Ordering::SeqCst),
        )
    };
    // Training and the similarity rebuild both want the single SQLite writer
    // for minutes; let a running rebuild finish first.
    if busy.is_some() || similarity_running {
        return;
    }
    let decision = db.with_conn(|conn| {
        let active_version = queries::get_selected_discovery_embedding_model(conn)?
            .map(|model| trainer_config_version_from_json(model.config_json.as_deref()));
        let latest = load_latest_run(conn)?;
        let attempts = finished_attempts(conn)?;
        Ok((
            should_retrain(active_version, latest.as_ref(), attempts),
            attempts,
        ))
    });
    let (retrain, attempts) = match decision {
        Ok(decision) => decision,
        Err(error) => {
            warn!(target: "noor.discovery.training", %error, "upgrade retrain check failed");
            return;
        }
    };
    if !retrain {
        return;
    }
    // Background: low priority (a small thread budget where that is not
    // possible), since nobody asked for this run.
    if let TrainingSpawn::Started = spawn_discovery_training(state, true, true, true).await {
        info!(
            target: "noor.discovery.training",
            attempt = attempts + 1,
            trainer_version = TRAINER_CONFIG_VERSION,
            "retraining a model built by an older trainer"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn run(status: &str, model_version: Option<i64>) -> LatestRun {
        LatestRun {
            status: status.to_string(),
            model_version,
        }
    }

    const OLD: i64 = TRAINER_CONFIG_VERSION - 1;

    #[test]
    fn never_trained_installs_are_left_alone() {
        assert!(!should_retrain(None, None, 0));
    }

    #[test]
    fn current_models_are_left_alone() {
        assert!(!should_retrain(Some(TRAINER_CONFIG_VERSION), None, 0));
    }

    #[test]
    fn old_models_retrain() {
        assert!(should_retrain(Some(OLD), None, 0));
        assert!(should_retrain(
            Some(OLD),
            Some(&run("completed", Some(OLD))),
            0
        ));
        assert!(should_retrain(
            Some(OLD),
            Some(&run("failed", Some(TRAINER_CONFIG_VERSION))),
            1
        ));
    }

    #[test]
    fn running_held_back_and_exhausted_stop_the_retrain() {
        assert!(!should_retrain(Some(OLD), Some(&run("running", None)), 0));
        assert!(!should_retrain(
            Some(OLD),
            Some(&run("completed", Some(TRAINER_CONFIG_VERSION))),
            1
        ));
        assert!(!should_retrain(Some(OLD), None, MAX_ATTEMPTS));
    }

    #[test]
    fn interrupted_runs_do_not_count_as_attempts() {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db.with_conn(|conn| {
            let config = format!(r#"{{"trainer_config_version":{TRAINER_CONFIG_VERSION}}}"#);
            for (status, error) in [
                ("failed", Some("interrupted by server restart")),
                ("failed", Some("discovery training timed out")),
                ("running", None),
            ] {
                let model = queries::create_embedding_model(
                    conn,
                    &format!("discovery-fusion-v2:{status}-{}", error.unwrap_or("none")),
                    "discovery-fusion-v2",
                    8,
                    "failed",
                    Some(&config),
                )?;
                let run = queries::create_training_run(conn, Some(model.id), "corpus", status)?;
                conn.execute(
                    "UPDATE training_runs SET error_text = ?1 WHERE id = ?2",
                    rusqlite::params![error, run.id],
                )?;
            }
            // Only the timed-out run counts: interrupted and running do not.
            assert_eq!(finished_attempts(conn)?, 1);
            Ok(())
        })
        .unwrap();
    }
}
