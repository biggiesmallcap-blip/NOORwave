//! Ready-pair planning: missing profile queueing and when the next transition is due.

use super::*;

pub(in crate::server::routes) async fn queue_missing_dj_profiles_for_current_pair(
    state: SharedState,
) -> Result<usize, StatusCode> {
    let missing_profile_refs = {
        let state_guard = state.read().await;
        let ephemeral_labels: Vec<(AudioDjProfileKey, (String, Option<String>))> = Vec::new();
        state_guard
            .db
            .with_conn(|conn| {
                if !queries::is_dj_engine_enabled(conn)? {
                    return Ok(Vec::new());
                }
                let pair =
                    crate::server::routes::active_dj_pair_for_state_and_conn(&state_guard, conn)?;
                missing_dj_profile_refs_for_pair(
                    conn,
                    pair,
                    &ephemeral_labels,
                    &state_guard.dj_profile_rebuild_inflight,
                )
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let mut attempted = 0usize;
    for media_ref in missing_profile_refs {
        queue_profile_rebuild_if_idle(state.clone(), media_ref).await?;
        attempted += 1;
    }
    Ok(attempted)
}

pub(super) fn missing_dj_profile_refs_for_pair(
    conn: &rusqlite::Connection,
    pair: crate::playback::dj_lookahead::DjLookaheadPair,
    ephemeral_labels: &[(AudioDjProfileKey, (String, Option<String>))],
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
) -> anyhow::Result<Vec<DjMediaRef>> {
    let mut missing = Vec::new();
    for media_ref in [pair.current, pair.next].into_iter().flatten() {
        if unsupported_auto_profile_rebuild_status(&media_ref).is_some() {
            continue;
        }
        let key = media_ref.profile_key();
        let label = ephemeral_labels
            .iter()
            .find(|(candidate, _)| candidate == &key)
            .map(|(_, label)| label);
        let inflight_key = dj_profile_inflight_key(&key);
        let rebuild_inflight = dj_profile_rebuild_is_inflight(inflight, &inflight_key);
        let deck = deck_status(conn, &media_ref, label, rebuild_inflight)?;
        let outdated_analysis = queries::get_audio_dj_profile(conn, &key)?.is_some_and(|profile| {
            profile.source.starts_with("dj_playback")
                && profile.profile_version
                    != crate::services::audio_analysis::dj_profile::DJ_PROFILE_VERSION
        });
        if deck_needs_profile_rebuild(&deck)
            || (outdated_analysis
                && !rebuild_inflight
                && (deck.profile_status == "ready"
                    || (deck.profile_status == "retrying"
                        && deck.profile_retry_after_ms.unwrap_or(0) <= 0)))
        {
            missing.push(media_ref);
        }
    }
    Ok(missing)
}

#[cfg(test)]
#[allow(dead_code)]
pub(super) async fn ready_pair_transition_is_due(
    state: &SharedState,
    current_track_id: i64,
    generation: u64,
) -> bool {
    let state_guard = state.read().await;
    let Some(info) = state_guard.playback_runtime_info.as_ref() else {
        return false;
    };
    if info.active_track_id != Some(current_track_id)
        || state_guard
            .playback_generation
            .load(std::sync::atomic::Ordering::Relaxed)
            != generation
    {
        return false;
    }
    let Some(runtime) = state_guard.playback_runtime.as_ref() else {
        return false;
    };
    let position_ms = runtime
        .handle
        .get_position_ms(info.sample_rate, info.channels);
    let duration_ms = state_guard
        .db
        .with_conn(|conn| current_track_duration_ms(conn, current_track_id))
        .ok()
        .flatten();
    ready_pair_transition_due(position_ms, duration_ms)
}

pub(super) fn current_track_duration_ms(
    conn: &rusqlite::Connection,
    current_track_id: i64,
) -> anyhow::Result<Option<i64>> {
    conn.query_row(
        "SELECT duration_ms FROM tracks WHERE id = ?1",
        [current_track_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()
    .map(|value| value.flatten())
    .map_err(anyhow::Error::from)
}

pub(super) fn ready_pair_transition_due(position_ms: i64, duration_ms: Option<i64>) -> bool {
    let Some(duration_ms) = duration_ms else {
        return false;
    };
    duration_ms.saturating_sub(position_ms.max(0)) <= DJ_READY_PAIR_TRANSITION_WINDOW_MS
}

pub(super) fn pair_planning_status(
    enabled: bool,
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
    latest_transition: Option<&OpenTransition>,
    ready_pair_due: bool,
) -> &'static str {
    if !enabled {
        return "disabled";
    }
    let (Some(current), Some(next)) = (current, next) else {
        return "pair_missing";
    };
    if current.profile_status == "decode_failed" || next.profile_status == "decode_failed" {
        return "profile_failed";
    }
    if !current.profile_ready || !next.profile_ready {
        return "waiting_for_profiles";
    }
    if let Some(transition) = latest_transition {
        return if transition.timing_status.as_deref() == Some("missed") {
            "missed"
        } else {
            "armed"
        };
    }
    if ready_pair_due {
        "ready_to_plan"
    } else {
        "waiting_for_window"
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(super) fn claim_ready_pair_transition_planning(current_track_id: i64, generation: u64) -> bool {
    let attempts = READY_PAIR_PLANNING_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let now = Instant::now();
    let mut attempts = attempts.lock().unwrap_or_else(|error| error.into_inner());
    claim_ready_pair_transition_planning_at(&mut attempts, current_track_id, generation, now)
}

#[cfg(test)]
pub(super) fn claim_ready_pair_transition_planning_at(
    attempts: &mut HashMap<ReadyPairPlanningKey, Instant>,
    current_track_id: i64,
    generation: u64,
    now: Instant,
) -> bool {
    let retry_after = Duration::from_secs(DJ_READY_PAIR_PLANNING_RETRY_SECS);
    attempts.retain(|_, last_attempt| now.duration_since(*last_attempt) < retry_after);
    let key = (current_track_id, generation);
    if let Some(last_attempt) = attempts.get(&key)
        && now.duration_since(*last_attempt) < retry_after
    {
        return false;
    }
    attempts.insert(key, now);
    true
}
