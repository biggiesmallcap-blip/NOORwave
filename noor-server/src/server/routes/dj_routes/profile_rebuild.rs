//! DJ profile rebuilds: manual and automatic queueing, in-flight tracking, failure backoff.

use super::*;

pub(super) async fn rebuild_profile(
    State(state): State<SharedState>,
    Json(payload): Json<RebuildDjProfileRequest>,
) -> Result<Json<RebuildDjProfileResponse>, StatusCode> {
    let candidate = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                if !queries::is_dj_engine_enabled(conn)? {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "dj_disabled".to_string(),
                        },
                    ));
                }
                let key = AudioDjProfileKey {
                    media_ref_kind: payload.media_ref_kind.clone(),
                    media_ref_id: payload.media_ref_id.clone(),
                };
                let pair = crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?;

                let media_ref = pair
                    .current
                    .as_ref()
                    .filter(|media_ref| media_ref.profile_key() == key)
                    .or_else(|| {
                        pair.next
                            .as_ref()
                            .filter(|media_ref| media_ref.profile_key() == key)
                    })
                    .cloned();
                let Some(media_ref) = media_ref else {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "not_current_pair".to_string(),
                        },
                    ));
                };
                if dj_profile_is_current_version(conn, &key)? {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "already_current".to_string(),
                        },
                    ));
                }
                let Some(dj_analysis_tx) = state.dj_analysis_tx.clone() else {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "source_unavailable".to_string(),
                        },
                    ));
                };
                Ok(RebuildProfileCandidate::Ready(media_ref, dj_analysis_tx))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let (media_ref, dj_analysis_tx) = match candidate {
        RebuildProfileCandidate::Ready(media_ref, dj_analysis_tx) => (media_ref, dj_analysis_tx),
        RebuildProfileCandidate::Response(response) => return Ok(Json(response)),
    };

    let queued = queue_tidal_profile_rebuild(state, media_ref, dj_analysis_tx, true).await?;
    Ok(Json(queued))
}

pub(super) async fn queue_tidal_profile_rebuild(
    state: SharedState,
    media_ref: DjMediaRef,
    dj_analysis_tx: tokio::sync::mpsc::UnboundedSender<
        crate::services::audio_analysis::dj_profile::DjAnalysisJob,
    >,
    force: bool,
) -> Result<RebuildDjProfileResponse, StatusCode> {
    let DjMediaRef::TidalTrack { tidal_id, .. } = media_ref.clone() else {
        return Ok(RebuildDjProfileResponse {
            accepted: false,
            status: "source_unavailable".to_string(),
        });
    };
    let media_key = media_ref.profile_key();
    if !force {
        if recent_dj_profile_rebuild_failure(&dj_profile_inflight_key(&media_key))
            .is_some_and(|failure| failure.status == "source_unavailable")
        {
            return Ok(RebuildDjProfileResponse {
                accepted: false,
                status: "source_unavailable".to_string(),
            });
        }
        let already_current = {
            let state_guard = state.read().await;
            state_guard
                .db
                .with_conn(|conn| dj_profile_is_current_version(conn, &media_key))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        };
        if already_current {
            tracing::debug!(
                media_ref_kind = %media_key.media_ref_kind,
                media_ref_id = %media_key.media_ref_id,
                "Skipping current DJ profile rebuild"
            );
            return Ok(RebuildDjProfileResponse {
                accepted: false,
                status: "already_current".to_string(),
            });
        }
    }
    let auto_slot = if force {
        None
    } else {
        match try_claim_auto_dj_profile_rebuild_slot() {
            Some(slot) => Some(slot),
            None => {
                tracing::debug!(
                    media_ref_kind = %media_key.media_ref_kind,
                    media_ref_id = %media_key.media_ref_id,
                    max_active = DJ_PROFILE_AUTO_REBUILD_MAX_ACTIVE,
                    "Skipping automatic DJ profile rebuild: active rebuild limit reached"
                );
                return Ok(RebuildDjProfileResponse {
                    accepted: false,
                    status: "busy".to_string(),
                });
            }
        }
    };
    let inflight_key = dj_profile_inflight_key(&media_key);
    let inflight = {
        let state_guard = state.read().await;
        state_guard.dj_profile_rebuild_inflight.clone()
    };
    let retry_after = if force {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS)
    };
    match mark_dj_profile_rebuild_inflight(&inflight, &inflight_key, retry_after)? {
        ProfileRebuildInflightDecision::Start => {
            // Only a user-forced rebuild resets the failure history. An
            // automatic re-accept must preserve the attempt counter, or the
            // backoff/give-up logic can never converge and the rebuild loops
            // forever on a permanently-failing stream.
            if force {
                clear_dj_profile_rebuild_failure(&inflight_key);
            }
            tracing::info!(
                media_ref_kind = %media_ref.profile_key().media_ref_kind,
                media_ref_id = %media_ref.profile_key().media_ref_id,
                force,
                "DJ profile rebuild accepted"
            );
        }
        ProfileRebuildInflightDecision::AlreadyRunning => {
            tracing::debug!(
                media_ref_kind = %media_ref.profile_key().media_ref_kind,
                media_ref_id = %media_ref.profile_key().media_ref_id,
                force,
                "DJ profile rebuild already running"
            );
            return Ok(RebuildDjProfileResponse {
                accepted: true,
                status: "already_running".to_string(),
            });
        }
    }

    let tokens = {
        let state_guard = state.read().await;
        state_guard.tidal.tokens()
    };
    let tokens = match tokens {
        Some(tokens) => Some(tokens),
        None => crate::server::routes::load_persisted_tidal_tokens(&state)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };
    let Some(tokens) = tokens else {
        clear_dj_profile_inflight(&inflight, &inflight_key);
        tracing::warn!(
            media_ref_kind = %media_ref.profile_key().media_ref_kind,
            media_ref_id = %media_ref.profile_key().media_ref_id,
            "DJ profile rebuild source unavailable"
        );
        return Ok(RebuildDjProfileResponse {
            accepted: false,
            status: "source_unavailable".to_string(),
        });
    };

    let requests = dj_profile_analysis_stream_requests(tidal_id);
    let track = rebuild_track_for_tidal_ref(&state, tidal_id).await;
    let (http_client, generation, runtime) = {
        let state_guard = state.read().await;
        (
            state_guard.http_client.clone(),
            state_guard
                .playback_generation
                .load(std::sync::atomic::Ordering::Relaxed),
            state_guard
                .playback_runtime
                .as_ref()
                .map(|runtime| runtime.handle.clone()),
        )
    };
    let config = PlaybackRuntimeConfig::new(http_client, tokens.access_token, None)
        .with_stream_resolver(crate::server::routes::runtime_stream_resolver(
            state.clone(),
        ))
        .with_dj_analysis(true, Some(dj_analysis_tx))
        .for_dj_analysis_only();

    let inflight_for_decode = inflight.clone();
    let inflight_key_for_decode = inflight_key.clone();
    let failure_key = inflight_key.clone();
    let retry_runtime = tokio::runtime::Handle::current();
    let retry_state = state.clone();
    let auto_slot_for_decode = auto_slot;
    tokio::task::spawn_blocking(move || {
        let _auto_slot = auto_slot_for_decode;
        // Prefer the fresh manifest that already produced this deck's audio.
        // The normal quality fallback remains available if it cannot be reused.
        let mut resolved = runtime.and_then(|runtime| runtime.resolved_analysis_stream(track.id));
        let mut last_error = None;
        let mut decoded = false;
        for (attempt_index, request) in requests.into_iter().enumerate() {
            let quality = request.audio_quality.clone();
            let mut job = player::PreparedPlaybackJob::new(
                track.clone(),
                PlaybackSourceRequest::TidalStream(request),
                GaplessPlan::disabled(),
            )
            .with_generation(generation)
            .with_dj_media_ref(media_ref.clone());
            if let Some(info) = resolved.take() {
                job.resolved_stream = Some(player::ResolvedStream {
                    info,
                    resolved_at: Instant::now(),
                });
            }
            let shared = dj_profile_rebuild_shared(track.id, generation);
            match decode_and_buffer_job(config.clone(), job, shared, 48_000, 2) {
                Ok(()) => {
                    decoded = true;
                    break;
                }
                Err(error) => {
                    if let Some(next_quality) =
                        next_dj_profile_analysis_quality(attempt_index, &error)
                    {
                        tracing::info!(
                            tidal_id,
                            quality,
                            next_quality,
                            error = %error,
                            "DJ profile rebuild retrying with fallback TIDAL quality"
                        );
                        last_error = Some(error);
                    } else {
                        last_error = Some(error);
                        break;
                    }
                }
            }
        }

        if decoded {
            clear_dj_profile_rebuild_failure(&failure_key);
            tracing::info!(tidal_id, "DJ profile rebuild decode queued analysis");
        } else if let Some(error) = last_error {
            let status = profile_rebuild_failure_status(&error);
            let message = profile_rebuild_error_message(&error, status);
            // The returned delay is the backoff for the next attempt, or None
            // once the attempt cap flips the failure to terminal decode_failed
            // - in which case we do NOT schedule another retry, ending the loop.
            let retry_delay = finish_dj_profile_rebuild_failure(
                &inflight_for_decode,
                &inflight_key_for_decode,
                status,
                message.clone(),
            );
            if let Some(delay) = retry_delay {
                schedule_dj_profile_retry(&retry_runtime, retry_state, delay);
            }
            tracing::warn!(tidal_id, error = %message, "DJ profile rebuild decode failed");
        }
    });

    Ok(RebuildDjProfileResponse {
        accepted: true,
        status: "accepted".to_string(),
    })
}

#[must_use]
pub(super) struct AutoDjProfileRebuildSlot {
    pub(super) counter: &'static AtomicUsize,
}

impl Drop for AutoDjProfileRebuildSlot {
    fn drop(&mut self) {
        release_auto_dj_profile_rebuild_slot(self.counter);
    }
}

pub(super) fn try_claim_auto_dj_profile_rebuild_slot() -> Option<AutoDjProfileRebuildSlot> {
    try_claim_auto_dj_profile_rebuild_slot_from(
        &DJ_PROFILE_AUTO_REBUILD_ACTIVE,
        DJ_PROFILE_AUTO_REBUILD_MAX_ACTIVE,
    )
}

pub(super) fn try_claim_auto_dj_profile_rebuild_slot_from(
    counter: &'static AtomicUsize,
    max_active: usize,
) -> Option<AutoDjProfileRebuildSlot> {
    let mut active = counter.load(Ordering::Acquire);
    loop {
        if active >= max_active {
            return None;
        }
        match counter.compare_exchange_weak(active, active + 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return Some(AutoDjProfileRebuildSlot { counter }),
            Err(actual) => active = actual,
        }
    }
}

pub(super) fn release_auto_dj_profile_rebuild_slot(counter: &AtomicUsize) {
    let _ = counter.try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
        active.checked_sub(1)
    });
}

pub(super) fn dj_profile_analysis_stream_requests(
    tidal_id: i64,
) -> Vec<tidal_stream::StreamRequest> {
    DJ_PROFILE_ANALYSIS_TIDAL_QUALITIES
        .iter()
        .map(|quality| tidal_stream::StreamRequest::new(tidal_id, *quality))
        .collect()
}

pub(super) fn next_dj_profile_analysis_quality(
    attempt_index: usize,
    error: &anyhow::Error,
) -> Option<&'static str> {
    // Fall back to the next quality tier when a different tier might dodge the
    // failure: TIDAL asset-not-ready, or a DASH segment / prebuffer failure
    // such as the LOW/AAC tier routing to an unreachable ad CDN. Previously
    // only asset-not-ready fell back, so a LOW-tier CDN timeout gave up without
    // ever trying the LOSSLESS stream that resolves fine.
    if !profile_rebuild_error_is_asset_not_ready_chain(error)
        && !profile_rebuild_error_is_retryable_chain(error)
    {
        return None;
    }
    DJ_PROFILE_ANALYSIS_TIDAL_QUALITIES
        .get(attempt_index + 1)
        .copied()
}

pub(super) fn dj_profile_rebuild_shared(
    track_id: i64,
    generation: u64,
) -> Arc<PlaybackSharedState> {
    let (command_tx, _command_rx) = std::sync::mpsc::channel::<PlaybackRuntimeCommand>();
    Arc::new(PlaybackSharedState::new(
        track_id,
        generation,
        PlaybackSourceKind::TidalStream,
        GaplessPlan::disabled(),
        48_000,
        2,
        None,
        command_tx,
        Arc::new(AtomicU32::new(1.0_f32.to_bits())),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    ))
}

pub(super) async fn queue_profile_rebuild_if_idle(
    state: SharedState,
    media_ref: DjMediaRef,
) -> Result<(), StatusCode> {
    let key = media_ref.profile_key();
    if let Some(status) = unsupported_auto_profile_rebuild_status(&media_ref) {
        tracing::debug!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            status,
            "Skipping automatic DJ profile rebuild for unsupported media ref"
        );
        return Ok(());
    }
    let dj_analysis_tx = {
        let state_guard = state.read().await;
        state_guard.dj_analysis_tx.clone()
    };
    let Some(dj_analysis_tx) = dj_analysis_tx else {
        tracing::debug!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            "Skipping automatic DJ profile rebuild: analysis actor unavailable"
        );
        return Ok(());
    };
    let response = queue_tidal_profile_rebuild(state, media_ref, dj_analysis_tx, false).await?;
    if !response.accepted {
        tracing::debug!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            status = %response.status,
            "Automatic DJ profile rebuild skipped"
        );
    }
    Ok(())
}

pub(super) fn unsupported_auto_profile_rebuild_status(
    media_ref: &DjMediaRef,
) -> Option<&'static str> {
    match media_ref {
        DjMediaRef::TidalTrack { .. } => None,
        DjMediaRef::LibraryTrack { .. } | DjMediaRef::PendingQueueItem { .. } => {
            Some("source_unavailable")
        }
    }
}

pub(super) fn mark_dj_profile_rebuild_inflight(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
    retry_after: std::time::Duration,
) -> Result<ProfileRebuildInflightDecision, StatusCode> {
    let mut guard = inflight
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(started_at) = guard.get(key)
        && started_at.elapsed() < retry_after
    {
        return Ok(ProfileRebuildInflightDecision::AlreadyRunning);
    }
    guard.insert(key.to_string(), std::time::Instant::now());
    Ok(ProfileRebuildInflightDecision::Start)
}

pub(super) fn dj_profile_inflight_key(key: &AudioDjProfileKey) -> String {
    format!("{}:{}", key.media_ref_kind, key.media_ref_id)
}

pub(super) fn deck_needs_profile_rebuild(deck: &DjDeckStatus) -> bool {
    (matches!(deck.profile_status.as_str(), "missing" | "ready")
        || (deck.profile_status == "retrying" && deck.profile_retry_after_ms.unwrap_or(0) <= 0))
        && (!deck.profile_ready || deck.waveform_status == "missing")
}

#[cfg(test)]
pub(super) fn ready_pair_can_request_transition_planning(
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
) -> bool {
    let (Some(current), Some(next)) = (current, next) else {
        return false;
    };
    current.profile_status != "decode_failed" && next.profile_status != "decode_failed"
}

pub(super) fn dj_profile_rebuild_is_inflight(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
) -> bool {
    inflight
        .lock()
        .ok()
        .and_then(|guard| guard.get(key).copied())
        .is_some_and(|started_at| {
            started_at.elapsed() < Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS)
        })
}

pub(super) fn clear_dj_profile_inflight(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
) {
    if let Ok(mut guard) = inflight.lock() {
        guard.remove(key);
    }
}

/// Records the failure and releases the inflight slot. Returns the backoff
/// delay to schedule the next retry, or `None` once the attempt cap is hit
/// (terminal decode_failed - do not schedule another retry).
pub(super) fn finish_dj_profile_rebuild_failure(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
    status: &str,
    message: String,
) -> Option<Duration> {
    let retry_delay = record_dj_profile_rebuild_failure(key, status, message);
    clear_dj_profile_inflight(inflight, key);
    retry_delay
}

/// Exponential backoff for transient profile-rebuild retries: 25s, 50s, 100s,
/// 200s, ... doubling per attempt, capped at DJ_PROFILE_MAX_RETRY_BACKOFF_SECS.
pub(super) fn profile_rebuild_backoff(attempts: u32) -> Duration {
    let shift = attempts.saturating_sub(1).min(6);
    let secs = DJ_PROFILE_TRANSIENT_RETRY_SECS
        .saturating_mul(1u64 << shift)
        .min(DJ_PROFILE_MAX_RETRY_BACKOFF_SECS);
    Duration::from_secs(secs)
}

pub(super) fn schedule_dj_profile_retry(
    runtime: &tokio::runtime::Handle,
    state: SharedState,
    delay: Duration,
) {
    runtime.spawn(async move {
        tokio::time::sleep(delay).await;
        if let Err(status) = queue_missing_dj_profiles_for_current_pair(state).await {
            tracing::warn!(
                ?status,
                "Scheduled DJ profile retry failed to queue current pair"
            );
        }
    });
}

pub(super) fn profile_rebuild_failures() -> &'static Mutex<HashMap<String, DjProfileRebuildFailure>>
{
    DJ_PROFILE_REBUILD_FAILURES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn record_dj_profile_rebuild_failure(
    key: &str,
    status: &str,
    message: String,
) -> Option<Duration> {
    let mut guard = profile_rebuild_failures().lock().ok()?;
    guard
        .retain(|_, failure| failure.recorded_at.elapsed() <= profile_rebuild_failure_ttl(failure));
    // This is short-lived suppression, not a permanent catalog blacklist.
    // Keep old failures bounded even when many unavailable assets are visited.
    if guard.len() >= 512
        && !guard.contains_key(key)
        && let Some(oldest_key) = guard
            .iter()
            .min_by_key(|(_, failure)| failure.recorded_at)
            .map(|(key, _)| key.clone())
    {
        guard.remove(&oldest_key);
    }
    // Carry the attempt count across automatic retries (the accept path no
    // longer clears it) so a chronically-failing stream backs off and finally
    // gives up instead of re-decoding every 25s forever.
    let attempts = guard
        .get(key)
        .map(|failure| failure.attempts)
        .unwrap_or(0)
        .saturating_add(1);
    let mut retry_reason = profile_rebuild_retry_reason(status, &message);
    let mut status = status.to_string();
    let mut message = message;
    if retry_reason.is_some() && attempts >= DJ_PROFILE_MAX_TRANSIENT_ATTEMPTS {
        // Give up: treat a chronically-failing rebuild as a hard decode
        // failure. deck_needs_profile_rebuild stops re-queuing decode_failed
        // decks, so the loop ends and the DJ engine can fall back.
        retry_reason = None;
        status = "decode_failed".to_string();
        message = format!(
            "{} Automatic analysis stopped after {attempts} attempts. Rebuild analysis to try again.",
            message.replace(" Retrying analysis.", "")
        );
    }
    let retry_delay = retry_reason
        .as_ref()
        .map(|_| profile_rebuild_backoff(attempts));
    let next_retry_at = retry_delay.map(|delay| Instant::now() + delay);
    guard.insert(
        key.to_string(),
        DjProfileRebuildFailure {
            status,
            message,
            retry_reason,
            next_retry_at,
            recorded_at: Instant::now(),
            attempts,
        },
    );
    retry_delay
}

pub(super) fn clear_dj_profile_rebuild_failure(key: &str) {
    if let Ok(mut guard) = profile_rebuild_failures().lock() {
        guard.remove(key);
    }
}

pub(crate) fn record_unavailable_tidal_source(tidal_id: i64) {
    record_dj_profile_rebuild_failure(
        &format!("tidal_track:{tidal_id}"),
        "source_unavailable",
        "Track is unavailable on TIDAL. Automatic analysis stopped.".to_string(),
    );
}

pub(crate) fn clear_unavailable_tidal_source(tidal_id: i64) {
    let key = format!("tidal_track:{tidal_id}");
    if let Ok(mut guard) = profile_rebuild_failures().lock()
        && guard
            .get(&key)
            .is_some_and(|failure| failure.status == "source_unavailable")
    {
        // A newly resolved stream establishes that the source is available.
        // Preserve transient decode attempt counts until decoding succeeds.
        guard.remove(&key);
    }
}

pub(super) fn profile_rebuild_failure_ttl(failure: &DjProfileRebuildFailure) -> Duration {
    Duration::from_secs(if failure.status == "decode_failed" {
        DJ_PROFILE_EXHAUSTED_FAILURE_TTL_SECS
    } else {
        DJ_PROFILE_REBUILD_FAILURE_TTL_SECS
    })
}

pub(super) fn recent_dj_profile_rebuild_failure(key: &str) -> Option<DjProfileRebuildFailure> {
    let mut guard = profile_rebuild_failures().lock().ok()?;
    match guard.get(key) {
        Some(failure) if failure.recorded_at.elapsed() <= profile_rebuild_failure_ttl(failure) => {
            Some(failure.clone())
        }
        Some(_) => {
            guard.remove(key);
            None
        }
        None => None,
    }
}

pub(super) fn profile_rebuild_failure_status(error: &anyhow::Error) -> &'static str {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<tidal_stream::StreamResolveError>()
            .is_some_and(|error| error.is_asset_not_ready() || error.is_track_specific_rejection())
    }) || profile_rebuild_error_is_asset_not_ready_chain(error)
    {
        // Called only after the bounded quality fallback has been exhausted.
        "source_unavailable"
    } else if profile_rebuild_error_is_retryable_chain(error) {
        "retrying"
    } else {
        "decode_failed"
    }
}

pub(super) fn profile_rebuild_error_is_asset_not_ready_chain(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<tidal_stream::StreamResolveError>()
            .is_some_and(tidal_stream::StreamResolveError::is_asset_not_ready)
            || profile_rebuild_error_is_asset_not_ready(&cause.to_string())
    })
}

pub(super) fn profile_rebuild_error_is_retryable_chain(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(error) = cause.downcast_ref::<tidal_stream::StreamResolveError>() {
            match error {
                tidal_stream::StreamResolveError::RequestFailed { .. } => return true,
                tidal_stream::StreamResolveError::UpstreamHttp { status, .. } => {
                    return status.is_server_error()
                        || status.as_u16() == 408
                        || status.as_u16() == 429;
                }
                _ => {}
            }
        }
        profile_rebuild_error_is_retryable(&cause.to_string())
    })
}

pub(super) fn profile_rebuild_error_is_retryable(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    message.contains("DASH stream prebuffer failed")
        || message.contains("DASH segment")
        || lower.contains("timed out")
        || lower.contains("request failed")
        || lower.contains("chunk error")
        || lower.contains("returned error status")
}

pub(super) fn profile_rebuild_error_is_asset_not_ready(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("asset is not ready for playback") || lower.contains("\"substatus\":4005")
}

pub(super) fn profile_rebuild_retry_reason(status: &str, message: &str) -> Option<String> {
    if status != "retrying" {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    let reason = if lower.contains("asset") || lower.contains("substatus") {
        "asset_not_ready"
    } else if lower.contains("dash") || lower.contains("prebuffer") {
        "dash_prebuffer"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else {
        "transient_decode"
    };
    Some(reason.to_string())
}

pub(super) fn profile_rebuild_retry_after_ms(failure: &DjProfileRebuildFailure) -> Option<i64> {
    let next_retry_at = failure.next_retry_at?;
    let now = Instant::now();
    if next_retry_at <= now {
        return Some(0);
    }
    Some(
        next_retry_at
            .duration_since(now)
            .as_millis()
            .min(i64::MAX as u128) as i64,
    )
}

pub(super) fn profile_rebuild_error_message(error: &anyhow::Error, status: &str) -> String {
    if status == "source_unavailable" {
        return "Track is unavailable on TIDAL. Automatic analysis stopped.".to_string();
    }
    if status == "retrying" {
        let message = error.to_string();
        if profile_rebuild_error_is_asset_not_ready(&message) {
            return "TIDAL asset is not ready. Retrying analysis.".to_string();
        }
        return "DASH stream prebuffer failed. Retrying analysis.".to_string();
    }
    let message = error.to_string();
    if message.trim().is_empty() {
        return "Profile decode failed".to_string();
    }
    message.chars().take(160).collect()
}

pub(super) async fn rebuild_track_for_tidal_ref(
    state: &SharedState,
    tidal_id: i64,
) -> crate::db::models::Track {
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    db.with_conn(|conn| library_track_for_tidal_id(conn, tidal_id))
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::db::models::Track {
            id: 0,
            title: format!("TIDAL {tidal_id}"),
            artist_id: 0,
            artist_name: None,
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: None,
            isrc: None,
            tidal_id: Some(tidal_id),
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: None,
            best_source: Some("tidal".to_string()),
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal_stream".to_string(),
            artwork_url: None,
        })
}
pub(super) fn library_track_for_tidal_id(
    conn: &rusqlite::Connection,
    tidal_id: i64,
) -> anyhow::Result<Option<crate::db::models::Track>> {
    conn.query_row(
        "SELECT t.id, t.title, t.artist_id, ar.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc, t.tidal_id,
                t.ytmusic_id, t.soundcloud_id, t.best_quality, t.best_source,
                t.fidelity_score, t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists ar ON t.artist_id = ar.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.tidal_id = ?1
         LIMIT 1",
        params![tidal_id],
        |row| {
            Ok(crate::db::models::Track {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get(3)?,
                album_id: row.get(4)?,
                album_title: row.get(5)?,
                disc_number: row.get(6)?,
                track_number: row.get(7)?,
                duration_ms: row.get(8)?,
                isrc: row.get(9)?,
                tidal_id: row.get(10)?,
                artist_tidal_id: None,
                album_tidal_id: None,
                // Library row: links resolve via artist_id/album_id.
                ytmusic_id: row.get(11)?,
                soundcloud_id: row.get(12)?,
                best_quality: row.get(13)?,
                best_source: row.get(14)?,
                fidelity_score: row.get(15)?,
                is_favorite: row.get(16)?,
                play_count: row.get(17)?,
                last_played_at: row.get(18)?,
                date_added: row.get(19)?,
                source: row.get(20)?,
                artwork_url: row.get(21)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
