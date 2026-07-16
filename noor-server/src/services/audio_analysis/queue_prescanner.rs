//! Queue-lookahead DSP prescanner.
//!
//! On each `QueueUpdated` / `TrackChanged` event, the actor debounces for
//! `DEBOUNCE` and then preview-analyses up to `LOOKAHEAD` upcoming tracks via
//! TIDAL LOW-quality streams. Each completed track emits `TrackAnalyzed` so
//! the automix cockpit refreshes its feature pills in place.
//!
//! Cancellation: a new queue event during a batch causes the in-flight track
//! to finish (the LOW download is small), then the loop exits and re-debounces
//! against the latest queue state. Granularity is one track (~2-3 s).

/// How many upcoming queue items to consider per batch.
pub const LOOKAHEAD: usize = 5;
/// Debounce window after a queue change before kicking off a batch.
pub const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(1500);
/// Polite pause between tracks within a batch.
pub const INTER_TRACK_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
/// Minimum wall-clock gap between successive batches. Last.fm radio promotes
/// pending rows in rapid bursts that each emit `QueueUpdated`; without a
/// cooldown the prescanner fires back-to-back batches that contend with
/// playback's own TIDAL stream resolves and trigger backoff. 30 s gives radio
/// promotions room to settle before the next batch.
pub const BATCH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
/// Hard ceiling on the per-track preview download + decode chain. If TIDAL
/// stalls a segment indefinitely (observed for some catalog rows), the actor
/// gets stuck. Individual segment fetches are bounded by the per-host timeout
/// in `cdn_health` (short for a degraded / dead edge) so one slow media segment
/// does not consume the full per-track budget.
pub const PREFETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const MIN_PARTIAL_BYTES: usize = 32 * 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_DASH_MEDIA_SEGMENTS: usize = 12;
const PRESCAN_TIDAL_QUALITIES: [&str; 2] = ["LOW", "LOSSLESS"];
const STREAM_REJECTED_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const TRANSIENT_FAILURE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_TRANSIENT_FAILURES_PER_BATCH: usize = LOOKAHEAD;

static PRESCAN_NEGATIVE_CACHE: LazyLock<Mutex<HashMap<i64, PrescanNegativeCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrescanFailureClass {
    PermanentSkip,
    TransientFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrescanFailureReason {
    AssetNotReady,
    StreamRejected,
    ResolveOkSegmentTimeout,
    ResolveOkSegmentFetchFailed,
    PrefetchTimeout,
}

impl PrescanFailureReason {
    fn as_str(self) -> &'static str {
        match self {
            PrescanFailureReason::AssetNotReady => "asset_not_ready",
            PrescanFailureReason::StreamRejected => "stream_rejected",
            PrescanFailureReason::ResolveOkSegmentTimeout => "resolve_ok_segment_timeout",
            PrescanFailureReason::ResolveOkSegmentFetchFailed => "resolve_ok_segment_fetch_failed",
            PrescanFailureReason::PrefetchTimeout => "prefetch_timeout",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrescanStatusSnapshot {
    pub status: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct PrescanNegativeCacheEntry {
    class: PrescanFailureClass,
    reason: PrescanFailureReason,
    expires_at: StdInstant,
}

fn prescan_failure_ttl(class: PrescanFailureClass) -> Duration {
    match class {
        PrescanFailureClass::PermanentSkip => STREAM_REJECTED_CACHE_TTL,
        PrescanFailureClass::TransientFailure => TRANSIENT_FAILURE_CACHE_TTL,
    }
}

fn cache_prescan_failure(track_id: i64, class: PrescanFailureClass, reason: PrescanFailureReason) {
    if let Ok(mut guard) = PRESCAN_NEGATIVE_CACHE.lock() {
        guard.insert(
            track_id,
            PrescanNegativeCacheEntry {
                class,
                reason,
                expires_at: StdInstant::now() + prescan_failure_ttl(class),
            },
        );
    }
}

fn classify_stream_resolve_for_prescan(
    error: &crate::services::tidal::stream::StreamResolveError,
) -> Option<(PrescanFailureClass, PrescanFailureReason)> {
    if error.is_asset_not_ready() {
        Some((
            PrescanFailureClass::TransientFailure,
            PrescanFailureReason::AssetNotReady,
        ))
    } else if error.is_stream_rejected() {
        Some((
            PrescanFailureClass::PermanentSkip,
            PrescanFailureReason::StreamRejected,
        ))
    } else {
        None
    }
}

fn prescan_negative_cache_contains(track_id: i64) -> bool {
    let Ok(mut guard) = PRESCAN_NEGATIVE_CACHE.lock() else {
        return false;
    };
    let Some(entry) = guard.get(&track_id).copied() else {
        return false;
    };
    if entry.expires_at <= StdInstant::now() {
        guard.remove(&track_id);
        return false;
    }
    true
}

fn cached_prescan_failure_class(track_id: i64) -> Option<PrescanFailureClass> {
    let Ok(mut guard) = PRESCAN_NEGATIVE_CACHE.lock() else {
        return None;
    };
    let Some(entry) = guard.get(&track_id).copied() else {
        return None;
    };
    if entry.expires_at <= StdInstant::now() {
        guard.remove(&track_id);
        return None;
    }
    Some(entry.class)
}

pub fn prescan_status_for_track(track_id: i64) -> Option<PrescanStatusSnapshot> {
    let Ok(mut guard) = PRESCAN_NEGATIVE_CACHE.lock() else {
        return None;
    };
    let Some(entry) = guard.get(&track_id).copied() else {
        return None;
    };
    if entry.expires_at <= StdInstant::now() {
        guard.remove(&track_id);
        return None;
    }
    Some(PrescanStatusSnapshot {
        status: match entry.class {
            PrescanFailureClass::PermanentSkip => "skipped",
            PrescanFailureClass::TransientFailure => "retrying",
        },
        reason: entry.reason.as_str(),
    })
}

#[cfg(test)]
fn clear_prescan_negative_cache_for_tests() {
    if let Ok(mut guard) = PRESCAN_NEGATIVE_CACHE.lock() {
        guard.clear();
    }
}

fn should_abort_prescan_batch(transient_failures: usize) -> bool {
    transient_failures >= MAX_TRANSIENT_FAILURES_PER_BATCH
}

fn prescan_dash_media_segments(total_segments: usize) -> usize {
    total_segments.min(MAX_DASH_MEDIA_SEGMENTS)
}

fn reqwest_error_summary(error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else if error.is_status() {
        "status"
    } else {
        "request failed"
    };
    if let Some(status) = error.status() {
        format!("{kind}: {status}")
    } else {
        kind.to_string()
    }
}

fn segment_fetch_failure_reason(error_summary: &str) -> PrescanFailureReason {
    if error_summary.contains("timeout") {
        PrescanFailureReason::ResolveOkSegmentTimeout
    } else {
        PrescanFailureReason::ResolveOkSegmentFetchFailed
    }
}

fn next_prescan_quality_after_error(
    attempt_index: usize,
    error: &crate::services::tidal::stream::StreamResolveError,
) -> Option<&'static str> {
    if !error.is_asset_not_ready() {
        return None;
    }
    PRESCAN_TIDAL_QUALITIES.get(attempt_index + 1).copied()
}

async fn resolve_prescan_stream(
    http_client: &reqwest::Client,
    access_token: &str,
    track_id: i64,
    tidal_id: i64,
) -> std::result::Result<
    crate::services::tidal::stream::StreamInfo,
    crate::services::tidal::stream::StreamResolveError,
> {
    for (attempt_index, quality) in PRESCAN_TIDAL_QUALITIES.iter().enumerate() {
        match crate::services::tidal::stream::get_stream_url(
            http_client,
            access_token,
            tidal_id,
            quality,
        )
        .await
        {
            Ok(stream_info) => return Ok(stream_info),
            Err(error) => {
                if let Some(next_quality) = next_prescan_quality_after_error(attempt_index, &error)
                {
                    tracing::info!(
                        track_id,
                        tidal_id,
                        quality,
                        next_quality,
                        error = %error,
                        "prescanner: TIDAL quality unavailable, trying fallback"
                    );
                    continue;
                }
                return Err(error);
            }
        }
    }
    unreachable!("prescan quality list is non-empty")
}

/// One row of queue state, projected for the pure selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrescanCandidate {
    pub track_id: i64,
    pub position: i64,
    pub has_tidal_id: bool,
    /// `None` when no DSP row exists yet; otherwise the stored version.
    pub analysis_version: Option<String>,
}

/// Pick up to `lookahead` upcoming tracks that need (re)analysis.
///
/// Filters in this order, then truncates by `lookahead`:
/// 1. Position strictly greater than `current_position`
/// 2. Has a TIDAL id (LOW-quality preview download requires one)
/// 3. Analysis version is missing or != `current_version`
pub fn pick_next_unanalyzed(
    candidates: &[PrescanCandidate],
    current_position: i64,
    lookahead: usize,
    current_version: &str,
) -> Vec<i64> {
    let mut filtered: Vec<&PrescanCandidate> = candidates
        .iter()
        .filter(|c| c.position > current_position)
        .filter(|c| c.has_tidal_id)
        .filter(|c| c.analysis_version.as_deref() != Some(current_version))
        .collect();
    filtered.sort_by_key(|c| c.position);
    filtered
        .into_iter()
        .take(lookahead)
        .map(|c| c.track_id)
        .collect()
}

use crate::AppEvent;
use crate::SharedState;
use crate::db::queries;
use crate::playback::decode::cdn_health;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant as StdInstant};

/// Fetch one preview segment, sharing the runtime's dead-edge failover: a URL on
/// the black-holed `sp-ad-cf` edge is first retried on the healthy `sp-pr-cf`
/// sibling, and degraded hosts get a short timeout instead of hanging. Keeping
/// the prescanner on the same breaker as playback stops it from independently
/// piling hung requests onto a dead host.
async fn fetch_prescan_segment(
    http_client: &reqwest::Client,
    seg_url: &str,
) -> std::result::Result<Vec<u8>, String> {
    let candidates = cdn_health::build_candidates(seg_url);
    let mut last_err = "no fetch candidates".to_string();
    for candidate in &candidates {
        match fetch_prescan_segment_once(http_client, &candidate.url, candidate.timeout).await {
            Ok(bytes) => {
                cdn_health::record_success(candidate);
                return Ok(bytes);
            }
            Err(err) => {
                cdn_health::record_failure(candidate);
                last_err = err;
            }
        }
    }
    Err(last_err)
}

async fn fetch_prescan_segment_once(
    http_client: &reqwest::Client,
    seg_url: &str,
    timeout: Duration,
) -> std::result::Result<Vec<u8>, String> {
    let resp = tokio::time::timeout(timeout, http_client.get(seg_url).send())
        .await
        .map_err(|_| "timeout".to_string())?
        .map_err(|error| reqwest_error_summary(&error))?
        .error_for_status()
        .map_err(|error| reqwest_error_summary(&error))?;

    tokio::time::timeout(timeout, resp.bytes())
        .await
        .map_err(|_| "timeout".to_string())?
        .map(|bytes| bytes.to_vec())
        .map_err(|error| reqwest_error_summary(&error))
}

/// Resolve the TIDAL LOW-quality stream for `track_id`, pull a bounded preview
/// window, decode the first ~30 s to mono f32, run DSP, and persist.
///
/// Skips silently (returns `Ok(false)`) when:
/// - the track is already at `CURRENT_ANALYSIS_VERSION`
/// - the track has no `tidal_id`
/// - TIDAL tokens are missing
/// - the downloaded clip is suspiciously small
///
/// Emits `AppEvent::TrackAnalyzed { track_id }` only on a successful save.
pub async fn prefetch_and_analyze_track(state: &SharedState, track_id: i64) -> Result<bool> {
    if prescan_negative_cache_contains(track_id) {
        tracing::info!(track_id, "prescanner skip: cached recent failure");
        return Ok(false);
    }

    let (tokens, http_client, db) = {
        let s = state.read().await;
        let Some(tokens) = s.tidal_tokens.clone() else {
            tracing::info!(track_id, "prescanner skip: no TIDAL tokens");
            return Ok(false);
        };
        (tokens, s.http_client.clone(), s.db.clone())
    };

    // Race-guard: the passive actor or another prescan pass may have already
    // bumped this track to the current version since the candidate snapshot.
    // Also skip if the user has manually set a BPM override.
    let override_or_current = db
        .with_conn(|conn| -> anyhow::Result<bool> {
            use rusqlite::OptionalExtension;
            let row: Option<(String, i64)> = conn
                .query_row(
                    "SELECT analysis_version, manual_override FROM audio_dsp_features WHERE track_id = ?1",
                    rusqlite::params![track_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?;
            Ok(match row {
                Some((v, override_flag)) => {
                    override_flag != 0 || v == super::CURRENT_ANALYSIS_VERSION
                }
                None => false,
            })
        })
        .unwrap_or(false);
    if override_or_current {
        tracing::info!(
            track_id,
            "prescanner skip: manual override or current version"
        );
        return Ok(false);
    }

    let tidal_id: Option<i64> = db
        .with_conn(|conn| Ok(queries::get_track_tidal_ids(conn, &[track_id])?))
        .ok()
        .and_then(|pairs| pairs.into_iter().next().map(|(_, tid)| tid));
    let Some(tidal_id) = tidal_id else {
        tracing::info!(track_id, "prescanner skip: no tidal_id");
        return Ok(false);
    };

    if should_defer_prescan_for_foreground_playback(state).await {
        tracing::debug!(track_id, "prescanner skip: foreground playback active");
        return Ok(false);
    }

    tracing::info!(track_id, tidal_id, "prescanner: starting analysis");

    let stream_info = match resolve_prescan_stream(
        &http_client,
        &tokens.access_token,
        track_id,
        tidal_id,
    )
    .await
    {
        Ok(stream_info) => stream_info,
        Err(error) => {
            if let Some((class, reason)) = classify_stream_resolve_for_prescan(&error) {
                cache_prescan_failure(track_id, class, reason);
                tracing::info!(
                    track_id,
                    tidal_id,
                    reason = reason.as_str(),
                    error = %error,
                    "prescanner skip: TIDAL stream unavailable"
                );
                return Ok(false);
            }
            return Err(anyhow::anyhow!("resolve stream url: {}", error));
        }
    };

    // Keep the init segment plus enough media segments for a preview window.
    // Walking the whole DASH manifest lets one slow catalog row starve the
    // lookahead batch.
    let mut buf: Vec<u8> = Vec::with_capacity(512 * 1024);
    let media_segments = prescan_dash_media_segments(stream_info.segment_urls.len());
    match fetch_prescan_segment(&http_client, &stream_info.url).await {
        Ok(segment) => buf.extend_from_slice(&segment[..segment.len().min(MAX_BYTES)]),
        Err(error_summary) => {
            let reason = segment_fetch_failure_reason(&error_summary);
            cache_prescan_failure(track_id, PrescanFailureClass::TransientFailure, reason);
            return Err(anyhow::anyhow!(
                "{}: init segment: {error_summary}",
                reason.as_str()
            ));
        }
    }
    let mut last_segment_error: Option<(PrescanFailureReason, String)> = None;
    let mut media_segments_fetched = 0usize;
    for (segment_index, seg_url) in stream_info
        .segment_urls
        .iter()
        .take(media_segments)
        .enumerate()
    {
        if buf.len() >= MAX_BYTES {
            break;
        }
        match fetch_prescan_segment(&http_client, seg_url).await {
            Ok(segment) => {
                media_segments_fetched += 1;
                let remaining = MAX_BYTES.saturating_sub(buf.len());
                if segment.len() <= remaining {
                    buf.extend_from_slice(&segment);
                } else {
                    buf.extend_from_slice(&segment[..remaining]);
                    break;
                }
            }
            Err(error_summary) => {
                let reason = segment_fetch_failure_reason(&error_summary);
                last_segment_error = Some((reason, error_summary.clone()));
                cache_prescan_failure(track_id, PrescanFailureClass::TransientFailure, reason);
                tracing::warn!(
                    track_id,
                    segment_index,
                    reason = reason.as_str(),
                    error = %error_summary,
                    bytes = buf.len(),
                    "prescanner: media segment fetch failed, trying next segment"
                );
                continue;
            }
        }
    }
    if media_segments_fetched == 0
        && let Some((reason, error_summary)) = last_segment_error.as_ref()
    {
        return Err(anyhow::anyhow!(
            "{}: no media segments fetched after retries: {error_summary}",
            reason.as_str()
        ));
    }
    if buf.len() >= MIN_PARTIAL_BYTES
        && let Some((reason, error_summary)) = last_segment_error.as_ref()
    {
        tracing::info!(
            track_id,
            bytes = buf.len(),
            reason = reason.as_str(),
            error = %error_summary,
            "prescanner: continuing with partial clip after segment fetch failure"
        );
    }
    if buf.len() < MIN_PARTIAL_BYTES {
        if let Some((reason, error_summary)) = last_segment_error {
            return Err(anyhow::anyhow!(
                "{}: insufficient preview bytes after segment retries: {error_summary}",
                reason.as_str()
            ));
        }
        tracing::info!(
            track_id,
            bytes = buf.len(),
            "prescanner skip: downloaded clip too small"
        );
        return Ok(false);
    }

    let audio_bytes = buf;
    let decode_result: std::result::Result<(Vec<f32>, u32), _> =
        tokio::task::spawn_blocking(move || {
            super::scanner::decode_source_to_mono_f32(
                Box::new(std::io::Cursor::new(audio_bytes)),
                30,
            )
        })
        .await
        .context("decode task panicked")?;
    let (samples, sample_rate) =
        decode_result.map_err(|e| anyhow::anyhow!("decode failed: {}", e))?;

    // Skip the first 10 s of the preview (intros distort BPM/key) — matches
    // `scanner.rs` behaviour.
    const PREVIEW_OFFSET_SEC: usize = 10;
    let offset_samples = sample_rate as usize * PREVIEW_OFFSET_SEC;
    let (samples, applied_offset_ms): (Vec<f32>, i64) =
        if samples.len() > offset_samples + sample_rate as usize * 4 {
            (
                samples[offset_samples..].to_vec(),
                (PREVIEW_OFFSET_SEC * 1000) as i64,
            )
        } else {
            (samples, 0i64)
        };

    let db_clone = db.clone();
    let saved = tokio::task::spawn_blocking(move || {
        super::engine::analyze_and_save(
            &db_clone,
            &samples,
            sample_rate,
            "queue_prescan",
            track_id,
            applied_offset_ms,
        )
    })
    .await
    .ok()
    .flatten();

    if let Some(f) = &saved {
        tracing::info!(
            track_id,
            bpm = ?f.bpm,
            key = f.key_signature.as_deref().unwrap_or("?"),
            energy = ?f.energy,
            "prescanner: analyzed"
        );
        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::TrackAnalyzed { track_id });
        Ok(true)
    } else {
        tracing::warn!(track_id, "prescanner: analyze_and_save returned None");
        Ok(false)
    }
}

use tokio::sync::broadcast;
use tokio::time::Instant;

async fn load_candidates(state: &SharedState) -> Result<(Vec<PrescanCandidate>, i64)> {
    let db = state.read().await.db.clone();
    db.with_conn(|conn| -> Result<(Vec<PrescanCandidate>, i64)> {
        // Current queue position (or -1 if nothing is playing).
        let current_pos: i64 = conn
            .query_row(
                "SELECT COALESCE(
                    (SELECT q.position FROM queue q
                     JOIN playback_state ps ON ps.current_queue_item_id = q.id
                     LIMIT 1),
                    -1
                )",
                [],
                |r| r.get(0),
            )
            .unwrap_or(-1);

        let mut stmt = conn.prepare(
            "SELECT q.track_id,
                    q.position,
                    t.tidal_id IS NOT NULL,
                    f.analysis_version
             FROM queue q
             JOIN tracks t ON t.id = q.track_id
             LEFT JOIN audio_dsp_features f ON f.track_id = t.id
             WHERE q.track_id IS NOT NULL
             ORDER BY q.position ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PrescanCandidate {
                track_id: row.get(0)?,
                position: row.get(1)?,
                has_tidal_id: row.get::<_, bool>(2)?,
                analysis_version: row.get::<_, Option<String>>(3)?,
            })
        })?;
        let candidates = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((candidates, current_pos))
    })
}

async fn run_batch(state: &SharedState, event_rx: &mut broadcast::Receiver<AppEvent>) {
    // Respect the global passive-DSP toggle.
    let (passive_on, defer_for_playback) = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                Ok::<_, anyhow::Error>((
                    super::is_passive_enabled(conn),
                    foreground_playback_is_active(conn, &state_guard)?,
                ))
            })
            .unwrap_or((true, false))
    };
    if !passive_on {
        return;
    }
    if defer_for_playback {
        tracing::debug!("queue prescanner deferred during foreground playback");
        return;
    }

    let (candidates, current_pos) = match load_candidates(state).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("queue prescanner candidate load failed: {}", e);
            return;
        }
    };

    let track_ids = pick_next_unanalyzed(
        &candidates,
        current_pos,
        LOOKAHEAD,
        super::CURRENT_ANALYSIS_VERSION,
    );

    let mut transient_failures = 0usize;
    for track_id in track_ids {
        // Mid-batch cancel: if a fresh queue event landed in the broadcast
        // buffer since the last track, bail and let the outer loop re-debounce.
        loop {
            match event_rx.try_recv() {
                Ok(AppEvent::QueueUpdated) | Ok(AppEvent::TrackChanged { .. }) => return,
                Ok(_) => continue,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => return,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }

        if should_defer_prescan_for_foreground_playback(state).await {
            tracing::debug!("queue prescanner deferred during foreground playback");
            return;
        }

        match tokio::time::timeout(
            PREFETCH_TIMEOUT,
            prefetch_and_analyze_track(state, track_id),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                tracing::warn!(track_id, "queue prescanner prefetch failed: {}", e);
                if cached_prescan_failure_class(track_id)
                    == Some(PrescanFailureClass::TransientFailure)
                {
                    transient_failures += 1;
                }
            }
            Err(_) => {
                cache_prescan_failure(
                    track_id,
                    PrescanFailureClass::TransientFailure,
                    PrescanFailureReason::PrefetchTimeout,
                );
                transient_failures += 1;
                tracing::warn!(
                    track_id,
                    timeout_secs = PREFETCH_TIMEOUT.as_secs(),
                    "queue prescanner prefetch timed out"
                );
            }
        }
        if should_abort_prescan_batch(transient_failures) {
            tracing::warn!(
                transient_failures,
                "queue prescanner batch stopped after repeated transient failures"
            );
            break;
        }
        tokio::time::sleep(INTER_TRACK_DELAY).await;
    }
}

async fn should_defer_prescan_for_foreground_playback(state: &SharedState) -> bool {
    let state_guard = state.read().await;
    state_guard
        .db
        .with_conn(|conn| foreground_playback_is_active(conn, &state_guard))
        .unwrap_or(false)
}

fn foreground_playback_is_active(
    conn: &rusqlite::Connection,
    state: &crate::AppState,
) -> anyhow::Result<bool> {
    let is_playing = crate::playback::player::load_state(conn)?.is_playing;
    let runtime_present = state.playback_runtime.is_some();
    Ok(super::should_defer_background_analysis_for_active_playback(
        is_playing,
        runtime_present,
    ))
}

/// Spawn the long-lived queue-lookahead actor. Subscribes to queue/track
/// change events on the broadcast channel, debounces by `DEBOUNCE`, and runs
/// a `run_batch` pass on the resulting quiescent state.
pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        let mut event_rx = {
            let s = state.read().await;
            s.event_tx.subscribe()
        };
        let mut deadline: Option<Instant> = None;
        let mut last_batch_end: Option<Instant> = None;

        loop {
            let wait = match deadline {
                Some(d) => d.saturating_duration_since(Instant::now()),
                None => std::time::Duration::from_secs(3600),
            };

            tokio::select! {
                msg = event_rx.recv() => {
                    match msg {
                        Ok(AppEvent::QueueUpdated) | Ok(AppEvent::TrackChanged { .. }) => {
                            deadline = Some(Instant::now() + DEBOUNCE);
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(wait) => {
                    if let Some(d) = deadline {
                        if Instant::now() >= d {
                            // Enforce inter-batch cooldown so rapid bursts of
                            // QueueUpdated (Last.fm radio promotions) don't fire
                            // back-to-back batches that hammer TIDAL.
                            if let Some(last) = last_batch_end {
                                let cooldown_until = last + BATCH_COOLDOWN;
                                if Instant::now() < cooldown_until {
                                    deadline = Some(cooldown_until);
                                    continue;
                                }
                            }
                            deadline = None;
                            run_batch(&state, &mut event_rx).await;
                            last_batch_end = Some(Instant::now());
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tidal::stream::StreamResolveError;

    static PRESCAN_CACHE_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn prescan_cache_test_guard() -> std::sync::MutexGuard<'static, ()> {
        PRESCAN_CACHE_TEST_LOCK
            .lock()
            .expect("prescan cache test lock")
    }

    fn c(track_id: i64, position: i64, tidal: bool, version: Option<&str>) -> PrescanCandidate {
        PrescanCandidate {
            track_id,
            position,
            has_tidal_id: tidal,
            analysis_version: version.map(String::from),
        }
    }

    #[test]
    fn empty_queue_yields_nothing() {
        assert!(pick_next_unanalyzed(&[], 0, 5, "v5").is_empty());
    }

    #[test]
    fn skips_already_current_version() {
        let cs = vec![
            c(1, 1, true, Some("v5")),
            c(2, 2, true, Some("v5")),
            c(3, 3, true, Some("v4")),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![3]);
    }

    #[test]
    fn skips_rows_at_or_before_current_position() {
        let cs = vec![
            c(1, 0, true, None),
            c(2, 1, true, None),
            c(3, 2, true, None),
            c(4, 3, true, None),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 1, 5, "v5"), vec![3, 4]);
    }

    #[test]
    fn skips_rows_without_tidal_id() {
        let cs = vec![
            c(1, 1, false, None),
            c(2, 2, true, None),
            c(3, 3, false, None),
            c(4, 4, true, None),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![2, 4]);
    }

    #[test]
    fn caps_at_lookahead() {
        let cs: Vec<PrescanCandidate> = (1..=10).map(|i| c(i, i, true, None)).collect();
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn returns_in_position_order_even_when_input_is_shuffled() {
        let cs = vec![
            c(30, 3, true, None),
            c(10, 1, true, None),
            c(20, 2, true, None),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![10, 20, 30]);
    }

    #[test]
    fn missing_version_is_treated_as_stale() {
        let cs = vec![
            c(1, 1, true, None),       // no DSP row yet
            c(2, 2, true, Some("v5")), // already current
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![1]);
    }

    #[test]
    fn stream_rejected_is_cached_as_quiet_permanent_skip() {
        let _guard = prescan_cache_test_guard();
        clear_prescan_negative_cache_for_tests();
        let error = StreamResolveError::StreamRejected {
            message: "TIDAL rejected playback request with 401 Unauthorized".to_string(),
        };

        let (class, reason) = classify_stream_resolve_for_prescan(&error).expect("classified");
        cache_prescan_failure(31985, class, reason);

        assert_eq!(class, PrescanFailureClass::PermanentSkip);
        assert_eq!(reason, PrescanFailureReason::StreamRejected);
        assert!(prescan_negative_cache_contains(31985));
        assert_eq!(
            cached_prescan_failure_class(31985),
            Some(PrescanFailureClass::PermanentSkip)
        );
        clear_prescan_negative_cache_for_tests();
    }

    #[test]
    fn asset_not_ready_is_cached_as_transient_retrying() {
        let _guard = prescan_cache_test_guard();
        clear_prescan_negative_cache_for_tests();
        let error = StreamResolveError::StreamRejected {
            message:
                r#"TIDAL rejected playback request with 401 Unauthorized: {"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#
                    .to_string(),
        };

        let (class, reason) = classify_stream_resolve_for_prescan(&error).expect("classified");
        cache_prescan_failure(31987, class, reason);

        assert_eq!(class, PrescanFailureClass::TransientFailure);
        assert_eq!(reason, PrescanFailureReason::AssetNotReady);
        assert_eq!(
            prescan_status_for_track(31987),
            Some(PrescanStatusSnapshot {
                status: "retrying",
                reason: "asset_not_ready"
            })
        );
        clear_prescan_negative_cache_for_tests();
    }

    #[test]
    fn asset_not_ready_tries_lossless_before_prescan_cache() {
        let error = StreamResolveError::StreamRejected {
            message:
                r#"TIDAL rejected playback request with 401 Unauthorized: {"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#
                    .to_string(),
        };

        assert_eq!(
            next_prescan_quality_after_error(0, &error),
            Some("LOSSLESS")
        );
        assert_eq!(next_prescan_quality_after_error(1, &error), None);
    }

    #[test]
    fn non_asset_prescan_rejection_does_not_try_quality_fallback() {
        let error = StreamResolveError::StreamRejected {
            message: "TIDAL rejected playback request with 401 Unauthorized".to_string(),
        };

        assert_eq!(next_prescan_quality_after_error(0, &error), None);
    }

    #[test]
    fn session_expired_is_not_cached_as_track_failure() {
        let _guard = prescan_cache_test_guard();
        clear_prescan_negative_cache_for_tests();
        let track_id = 31986;
        let error = StreamResolveError::SessionExpired {
            message: "expired".to_string(),
        };

        assert_eq!(classify_stream_resolve_for_prescan(&error), None);
        assert!(!prescan_negative_cache_contains(track_id));
    }

    #[test]
    fn transient_failures_abort_batch_after_threshold() {
        assert!(!should_abort_prescan_batch(
            MAX_TRANSIENT_FAILURES_PER_BATCH - 1
        ));
        assert!(should_abort_prescan_batch(MAX_TRANSIENT_FAILURES_PER_BATCH));
    }

    #[test]
    fn two_transient_failures_do_not_abort_prescan_batch() {
        assert!(!should_abort_prescan_batch(2));
    }

    #[test]
    fn dash_prescan_caps_media_segments_to_preview_window() {
        assert_eq!(prescan_dash_media_segments(0), 0);
        assert_eq!(prescan_dash_media_segments(5), 5);
        assert_eq!(
            prescan_dash_media_segments(MAX_DASH_MEDIA_SEGMENTS + 10),
            MAX_DASH_MEDIA_SEGMENTS
        );
    }

    #[test]
    fn permanent_skips_do_not_count_as_transient_batch_failures() {
        let _guard = prescan_cache_test_guard();
        clear_prescan_negative_cache_for_tests();
        cache_prescan_failure(
            31985,
            PrescanFailureClass::PermanentSkip,
            PrescanFailureReason::StreamRejected,
        );

        let transient_failures = usize::from(
            cached_prescan_failure_class(31985) == Some(PrescanFailureClass::TransientFailure),
        );

        assert_eq!(transient_failures, 0);
        assert!(!should_abort_prescan_batch(transient_failures));
        clear_prescan_negative_cache_for_tests();
    }

    #[test]
    fn segment_timeout_is_reported_as_transient_retry() {
        let _guard = prescan_cache_test_guard();
        clear_prescan_negative_cache_for_tests();
        cache_prescan_failure(
            31987,
            PrescanFailureClass::TransientFailure,
            PrescanFailureReason::ResolveOkSegmentTimeout,
        );

        assert_eq!(
            prescan_status_for_track(31987),
            Some(PrescanStatusSnapshot {
                status: "retrying",
                reason: "resolve_ok_segment_timeout",
            })
        );
        clear_prescan_negative_cache_for_tests();
    }

    #[test]
    fn segment_fetch_failure_reason_distinguishes_timeout() {
        assert_eq!(
            segment_fetch_failure_reason("timeout"),
            PrescanFailureReason::ResolveOkSegmentTimeout
        );
        assert_eq!(
            segment_fetch_failure_reason("connect"),
            PrescanFailureReason::ResolveOkSegmentFetchFailed
        );
    }

    #[tokio::test]
    async fn fetch_prescan_segment_rejects_non_success_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let app = axum::Router::new().route(
            "/segment",
            axum::routing::get(|| async {
                (axum::http::StatusCode::FORBIDDEN, "segment unavailable")
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve test segment");
        });

        let err = fetch_prescan_segment(&reqwest::Client::new(), &format!("http://{addr}/segment"))
            .await
            .expect_err("non-success segment status should not parse as media bytes");

        assert!(err.contains("status"));
        assert!(err.contains("403"));
    }
}
