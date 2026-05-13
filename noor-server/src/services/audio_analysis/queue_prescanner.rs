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
use anyhow::{Context, Result};
use futures::StreamExt;

/// Resolve the TIDAL LOW-quality stream for `track_id`, pull the audio bytes
/// (capped at 8 MB so the MP4 `moov` atom is included — see `scanner.rs` for
/// the rationale), decode the first ~30 s to mono f32, run DSP, and persist.
///
/// Skips silently (returns `Ok(false)`) when:
/// - the track is already at `CURRENT_ANALYSIS_VERSION`
/// - the track has no `tidal_id`
/// - TIDAL tokens are missing
/// - the downloaded clip is suspiciously small
///
/// Emits `AppEvent::TrackAnalyzed { track_id }` only on a successful save.
pub async fn prefetch_and_analyze_track(state: &SharedState, track_id: i64) -> Result<bool> {
    let (tokens, http_client, db) = {
        let s = state.read().await;
        let Some(tokens) = s.tidal_tokens.clone() else {
            return Ok(false);
        };
        (tokens, s.http_client.clone(), s.db.clone())
    };

    // Race-guard: the passive actor or another prescan pass may have already
    // bumped this track to the current version since the candidate snapshot.
    let existing = db
        .with_conn(|conn| Ok(queries::get_audio_dsp_features(conn, track_id)?))
        .ok()
        .flatten();
    if let Some(f) = &existing {
        if f.analysis_version == super::CURRENT_ANALYSIS_VERSION {
            return Ok(false);
        }
    }

    let tidal_id: Option<i64> = db
        .with_conn(|conn| Ok(queries::get_track_tidal_ids(conn, &[track_id])?))
        .ok()
        .and_then(|pairs| pairs.into_iter().next().map(|(_, tid)| tid));
    let Some(tidal_id) = tidal_id else {
        return Ok(false);
    };

    let stream_info = crate::services::tidal::stream::get_stream_url(
        &http_client,
        &tokens.access_token,
        tidal_id,
        "LOW",
    )
    .await
    .map_err(|e| anyhow::anyhow!("resolve stream url: {}", e))?;

    // 8 MB ≈ 11 min of 96 kbps AAC; keeps the moov atom intact for Symphonia.
    const MAX_BYTES: usize = 8 * 1024 * 1024;
    let mut buf: Vec<u8> = Vec::with_capacity(512 * 1024);
    'segments: for seg_url in
        std::iter::once(&stream_info.url).chain(stream_info.segment_urls.iter())
    {
        if buf.len() >= MAX_BYTES {
            break;
        }
        let resp = http_client
            .get(seg_url)
            .send()
            .await
            .context("fetch segment")?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let c = chunk.context("stream chunk")?;
            let remaining = MAX_BYTES.saturating_sub(buf.len());
            if c.len() <= remaining {
                buf.extend_from_slice(&c);
            } else {
                buf.extend_from_slice(&c[..remaining]);
                break 'segments;
            }
        }
    }
    if buf.len() < 32 * 1024 {
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

    if saved.is_some() {
        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::TrackAnalyzed { track_id });
        Ok(true)
    } else {
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
    let passive_on = state
        .read()
        .await
        .db
        .with_conn(|conn| Ok::<_, anyhow::Error>(super::is_passive_enabled(conn)))
        .unwrap_or(true);
    if !passive_on {
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

        if let Err(e) = prefetch_and_analyze_track(state, track_id).await {
            tracing::warn!(track_id, "queue prescanner prefetch failed: {}", e);
        }
        tokio::time::sleep(INTER_TRACK_DELAY).await;
    }
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
                            deadline = None;
                            run_batch(&state, &mut event_rx).await;
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
}
