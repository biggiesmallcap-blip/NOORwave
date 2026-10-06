//! Bounded availability checks for recordings with alternative catalogue IDs.
use super::{
    client::TidalClient,
    stream::{self, StreamRequest},
};
use crate::db::catalogue;
use crate::{AppEvent, SharedState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static RUNNING: AtomicBool = AtomicBool::new(false);
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

pub async fn run_if_idle(state: SharedState) {
    if RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    tokio::spawn(async move {
        let _guard = Guard;
        if let Err(error) = check_candidates(&state).await {
            tracing::warn!(target:"noor.catalogue",%error,"catalogue availability check failed");
        }
    });
}

async fn check_candidates(state: &SharedState) -> anyhow::Result<()> {
    let (db, http, tokens, events) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.tidal_http_client.clone(),
            s.tidal_tokens.clone(),
            s.event_tx.clone(),
        )
    };
    let Some(tokens) = tokens else {
        return Ok(());
    };
    let ids = db.with_conn(|conn| candidate_ids(conn, 24))?;
    let client = TidalClient::with_http(
        http.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    )
    .for_background_work();
    let mut any_observed = false;
    let mut any_switched = false;
    for id in ids {
        let result = tokio::time::timeout(Duration::from_secs(12), client.get_track(id)).await;
        let changed = match result {
            Ok(Ok(track)) => {
                let mut changed = db.with_conn(|conn| {
                    let tx = conn.unchecked_transaction()?;
                    let Some(local) = catalogue::track_id(&tx, id)? else {
                        return Ok(false);
                    };
                    let before: Option<i64> =
                        tx.query_row("SELECT tidal_id FROM tracks WHERE id=?1", [local], |r| {
                            r.get(0)
                        })?;
                    catalogue::record_track(&tx, local, &track, false, None)?;
                    let after: Option<i64> =
                        tx.query_row("SELECT tidal_id FROM tracks WHERE id=?1", [local], |r| {
                            r.get(0)
                        })?;
                    tx.commit()?;
                    Ok(before != after)
                })?;
                if track.stream_ready.is_none()
                    && track.extra.get("allowStreaming").and_then(|v| v.as_bool()) != Some(false)
                {
                    let available = matches!(
                        tokio::time::timeout(
                            Duration::from_secs(12),
                            stream::resolve_stream_background(
                                &http,
                                &tokens.access_token,
                                &StreamRequest::new(id, "LOW")
                            )
                        )
                        .await,
                        Ok(Ok(_))
                    );
                    changed |= db.with_conn(|conn| {
                        let tx = conn.unchecked_transaction()?;
                        let switched = catalogue::observe(
                            &tx,
                            id,
                            if available { "available" } else { "error" },
                            "stream_check",
                        )?;
                        tx.commit()?;
                        Ok(switched)
                    })?;
                }
                changed
            }
            Ok(Err(error)) => {
                let message = error.to_string().to_lowercase();
                if message.contains("401")
                    || message.contains("unauthorized")
                    || message.contains("429")
                {
                    break;
                }
                let status = if message.starts_with("tidal api error 404 ") {
                    "unavailable"
                } else {
                    "error"
                };
                db.with_conn(|conn| {
                    let tx = conn.unchecked_transaction()?;
                    let changed = catalogue::observe(
                        &tx,
                        id,
                        status,
                        if status == "unavailable" {
                            "metadata_404"
                        } else {
                            "request_error"
                        },
                    )?;
                    tx.commit()?;
                    Ok(changed)
                })?
            }
            Err(_) => db.with_conn(|conn| catalogue::observe(conn, id, "error", "timeout"))?,
        };
        any_observed = true;
        any_switched |= changed;
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    if any_switched {
        let _ = events.send(AppEvent::QueueUpdated);
    }
    if any_observed {
        let _ = events.send(AppEvent::LibrarySynced);
    }
    Ok(())
}

/// Fairness is by recording, and a selected release travels with one stale
/// alternative. The pair can establish fresh replacement evidence together.
pub(crate) fn candidate_ids(
    conn: &rusqlite::Connection,
    budget: usize,
) -> anyhow::Result<Vec<i64>> {
    let locals = {
        let mut stmt = conn.prepare(
            "SELECT t.id FROM tracks t JOIN tidal_track_aliases a ON a.track_id=t.id
        WHERE (t.is_library=1 OR t.is_favorite=1) AND a.evidence!='identity_conflict'
        GROUP BY t.id HAVING (COUNT(*)>1 OR MAX(t.remote_favorite_state='unresolved')=1)
        AND SUM(a.checked_at IS NULL OR julianday(a.checked_at)<=julianday('now','-1 day'))>0
        ORDER BY SUM(a.checked_at IS NULL) DESC,MIN(a.checked_at),t.id LIMIT ?1",
        )?;
        stmt.query_map([budget as i64], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut ids = Vec::new();
    for local in locals {
        if ids.len() >= budget {
            break;
        }
        let mut stmt=conn.prepare("SELECT a.tidal_id FROM tidal_track_aliases a JOIN tracks t ON t.id=a.track_id
            WHERE a.track_id=?1 AND a.evidence!='identity_conflict'
            ORDER BY (a.tidal_id=t.tidal_id) DESC,(a.checked_at IS NOT NULL),a.checked_at,a.tidal_id LIMIT 2")?;
        for id in stmt.query_map([local], |r| r.get::<_, i64>(0))? {
            if ids.len() >= budget {
                break;
            }
            ids.push(id?);
        }
    }
    Ok(ids)
}
