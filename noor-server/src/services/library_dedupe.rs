//! Automatic duplicate removal with liked-song protection.
//!
//! Runs a duplicate scan and auto-merges every same-recording group
//! (exact_duplicate / quality_variant / cross_album_reissue / remaster) via
//! `library::duplicates::auto_merge_pending`. Variants (alt_version) and
//! groups touching local files are left pending for the Duplicates UI.
//!
//! Liked songs are hard-protected: the keep-rule prefers the liked row, and a
//! merged-away liked copy folds its like into the kept row locally AND on
//! TIDAL (favorite the kept id, unfavorite the removed one) so the next Full
//! sync's favorite reconciliation cannot wipe the transfer.
//!
//! Triggered after every completed sync/import via the `LibrarySynced`
//! listener in `main.rs` (same run-if-idle shape as `tidal::repair`), and
//! synchronously from the Settings "Reclean library" action.

use crate::library::duplicates as dup;
use crate::services::tidal::mutations as tidal_mutations;
use crate::{AppEvent, SharedState};
use serde::Serialize;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

#[derive(Debug, Default, Serialize)]
pub struct DedupeSummary {
    pub groups_found: usize,
    pub merged_groups: usize,
    pub removed_tracks: usize,
    /// Groups left for manual review (variants, local files).
    pub skipped_groups: usize,
}

fn running_flag() -> &'static AtomicBool {
    static RUNNING: OnceLock<AtomicBool> = OnceLock::new();
    RUNNING.get_or_init(|| AtomicBool::new(false))
}

struct RunningGuard;

impl Drop for RunningGuard {
    fn drop(&mut self) {
        running_flag().store(false, Ordering::SeqCst);
    }
}

/// Scan + auto-merge + TIDAL like reconciliation. Returns `None` when a pass
/// is already running (the Reclean endpoint surfaces that as a conflict).
pub async fn run_dedupe_pass(state: &SharedState) -> anyhow::Result<Option<DedupeSummary>> {
    if running_flag()
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        debug!(target: "noor.dedupe", "pass already running, skipping");
        return Ok(None);
    }
    let _guard = RunningGuard;

    let (scan_stats, merge_stats) = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let scan_stats = dup::scan(conn)?;
            let merge_stats = dup::auto_merge_pending(conn)?;
            Ok((scan_stats, merge_stats))
        })?
    };

    // Push transferred likes to TIDAL. Best-effort: the local like is already
    // on the kept row; this keeps TIDAL's favorites list pointing at the same
    // copy so Full-sync reconciliation agrees.
    if !merge_stats.favorite_transfers.is_empty() {
        let (tokens, http) = {
            let s = state.read().await;
            (s.tidal_tokens.clone(), s.http_client.clone())
        };
        if let Some(t) = tokens {
            for (kept_tidal_id, loser_tidal_ids) in &merge_stats.favorite_transfers {
                if let Err(e) = tidal_mutations::add_favorite_track(
                    &http,
                    &t.access_token,
                    &t.user_id,
                    *kept_tidal_id,
                    &t.country_code,
                )
                .await
                {
                    warn!(
                        target: "noor.dedupe",
                        "failed to favorite kept TIDAL track {kept_tidal_id}: {e}"
                    );
                }
                for loser in loser_tidal_ids {
                    if let Err(e) = tidal_mutations::remove_favorite_track(
                        &http,
                        &t.access_token,
                        &t.user_id,
                        *loser,
                        &t.country_code,
                    )
                    .await
                    {
                        warn!(
                            target: "noor.dedupe",
                            "failed to unfavorite removed TIDAL track {loser}: {e}"
                        );
                    }
                }
            }
        } else {
            warn!(
                target: "noor.dedupe",
                transfers = merge_stats.favorite_transfers.len(),
                "TIDAL not connected; likes transferred locally only"
            );
        }
    }

    {
        let s = state.read().await;
        if merge_stats.queue_changed {
            let _ = s.event_tx.send(AppEvent::QueueUpdated);
        }
        if merge_stats.current_changed {
            let _ = s.event_tx.send(AppEvent::PlaybackStateChanged);
        }
        // Only when rows actually changed: LibrarySynced re-triggers this
        // pass via the main.rs listener, and a merged-nothing second run is
        // what terminates that loop.
        if merge_stats.merged_groups > 0 {
            let _ = s.event_tx.send(AppEvent::LibrarySynced);
        }
    }

    if merge_stats.merged_groups > 0 {
        info!(
            target: "noor.dedupe",
            merged = merge_stats.merged_groups,
            removed = merge_stats.removed_tracks,
            for_review = merge_stats.skipped_groups,
            "auto-dedupe merged duplicate groups"
        );
    }

    Ok(Some(DedupeSummary {
        groups_found: scan_stats.groups_found,
        merged_groups: merge_stats.merged_groups,
        removed_tracks: merge_stats.removed_tracks,
        skipped_groups: merge_stats.skipped_groups,
    }))
}

/// Spawn a dedupe pass if one isn't already running. Never blocks the caller.
pub async fn run_if_idle(state: SharedState) {
    if running_flag().load(Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        if let Err(e) = run_dedupe_pass(&state).await {
            warn!(target: "noor.dedupe", "auto-dedupe pass failed: {e}");
        }
    });
}
