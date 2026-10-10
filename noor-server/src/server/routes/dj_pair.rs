//! DJ pair preparation after queue changes: profile analysis, lookahead, prepared transitions and the drop-preview scheduler.

use super::*;

pub(super) type DropPreviewArmKey = (usize, i64, i64, u64, u64);

pub(super) const DJ_LOOKAHEAD_DEADLINE_SAMPLES: u64 = 48_000 * 30;

pub(super) const DROP_PREVIEW_DURATION_MS: u32 = 16_000;

pub(super) const DROP_PREVIEW_ARM_RETRY_SECS: u64 = 60 * 60;

pub(super) static DROP_PREVIEW_ARM_ATTEMPTS: OnceLock<Mutex<HashMap<DropPreviewArmKey, Instant>>> =
    OnceLock::new();

// One preparation task per active pair, including when a seek emits Started.
pub(super) static DJ_PAIR_PREPARATION_TASKS: OnceLock<Mutex<HashSet<(usize, i64, i64, u64, u64)>>> =
    OnceLock::new();

pub(super) struct DjPairPreparationGuard((usize, i64, i64, u64, u64));

impl Drop for DjPairPreparationGuard {
    fn drop(&mut self) {
        if let Some(tasks) = DJ_PAIR_PREPARATION_TASKS.get()
            && let Ok(mut tasks) = tasks.lock()
        {
            tasks.remove(&self.0);
        }
    }
}

pub(crate) async fn queue_missing_dj_profiles_after_pair_change(
    state: SharedState,
    context: &'static str,
) {
    if let Err(status) = dj_routes::queue_missing_dj_profiles_for_current_pair(state).await {
        warn!(
            ?status,
            context, "DJ profile queueing failed after pair change"
        );
    }
}

pub(crate) fn active_dj_lookahead_start_for_state(
    state: &crate::AppState,
) -> Option<player::DjLookaheadStart> {
    state
        .db
        .with_conn(|conn| {
            if !queries::is_dj_engine_enabled(conn)? {
                return Ok(None);
            }
            let pair = active_dj_pair_for_state_and_conn(state, conn)?;
            Ok(player::dj_lookahead_start_from_pair(
                pair,
                DJ_LOOKAHEAD_DEADLINE_SAMPLES,
            ))
        })
        .ok()
        .flatten()
}

pub(crate) async fn start_dj_lookahead_and_queue_profiles_after_pair_change(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    context: &'static str,
) {
    let (lookahead, playback_generation) = {
        let state_guard = state.read().await;
        (
            active_dj_lookahead_start_for_state(&state_guard),
            current_playback_generation(&state_guard),
        )
    };
    if let Some(lookahead) = lookahead {
        let _ = lookahead.dispatch(&handle);
        spawn_dj_pair_preparation(
            state.clone(),
            handle.clone(),
            lookahead.clone(),
            playback_generation,
        );
        spawn_drop_preview_scheduler(state.clone(), handle, lookahead, playback_generation);
    }
    queue_missing_dj_profiles_after_pair_change(state, context).await;
}

pub(crate) fn spawn_dj_pair_preparation(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
    playback_generation: u64,
) {
    let (Some(track_id), Some(next_queue_id)) = (
        lookahead.current.as_ref().and_then(|r| r.track_id()),
        lookahead.next_queue_item_id,
    ) else {
        return;
    };
    let key = (
        Arc::as_ptr(&state) as usize,
        track_id,
        next_queue_id,
        lookahead.queue_generation,
        playback_generation,
    );
    if !DJ_PAIR_PREPARATION_TASKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(key)
    {
        return;
    }
    tokio::spawn(async move {
        let _guard = DjPairPreparationGuard(key);
        let mut prepare_attempts = 0_u64;
        let mut next_prepare_attempt = Instant::now();
        loop {
            let same_pair = {
                let state_guard = state.read().await;
                state_guard
                    .playback_runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    && current_playback_generation(&state_guard) == playback_generation
                    && state_guard
                        .playback_runtime_info
                        .as_ref()
                        .and_then(|info| info.active_track_id)
                        == Some(track_id)
                    && active_dj_lookahead_start_for_state(&state_guard).is_some_and(|pair| {
                        pair.current_queue_item_id == lookahead.current_queue_item_id
                            && pair.next_queue_item_id == lookahead.next_queue_item_id
                            && pair.queue_generation == lookahead.queue_generation
                    })
            };
            if !same_pair {
                break;
            }
            // Use the established peek/prebuffer path from track start. Keep
            // network resolution away from the runtime event listener.
            if prepare_attempts < 3 && Instant::now() >= next_prepare_attempt {
                match handle_near_end(state.clone(), track_id, playback_generation).await {
                    Ok(false) => {} // Already prepared or another preparation owns the slot.
                    result => {
                        prepare_attempts += 1;
                        next_prepare_attempt =
                            Instant::now() + Duration::from_secs(20 * prepare_attempts);
                        if let Err(error) = result {
                            warn!("Early DJ preparation skipped: {error:?}");
                        }
                    }
                }
            }
            if let Err(error) = refresh_prepared_dj_transition(&state, &handle).await {
                warn!("Prepared DJ plan refresh skipped: {error:?}");
            }
            // Profiles may arrive after the pair starts playing. An absent
            // preview plan must not consume its one-preview-per-pair latch.
            if let Err(error) = schedule_drop_preview_for_pair(
                state.clone(),
                handle.clone(),
                lookahead.clone(),
                playback_generation,
            )
            .await
            {
                warn!("Drop preview scheduling skipped: {error:?}");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

pub(super) async fn refresh_prepared_dj_transition(
    state: &SharedState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
) -> anyhow::Result<()> {
    let (engine, update) = {
        let state_guard = state.read().await;
        let Some(info) = state_guard.playback_runtime_info.as_ref() else {
            return Ok(());
        };
        let pair = state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?;
        let engine = crate::playback::dj_engine::DjEngine::new(state_guard.db.clone());
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        let update = player::plan_prepared_dj_transition_update(
            &engine,
            pair,
            info.sample_rate,
            info.channels,
            handle.get_position_ms(info.sample_rate, info.channels),
        )?;
        (engine, update)
    };
    let Some(update) = update else {
        return Ok(());
    };
    let runtime = handle.clone();
    let transition = update.transition.clone();
    let gapless = update.gapless;
    let accepted = tokio::task::spawn_blocking(move || {
        runtime.update_prepared_transition(transition, gapless)
    })
    .await?;
    if accepted {
        player::persist_prepared_dj_transition_update(&engine, &update)?;
        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::PlaybackStateChanged);
    }
    Ok(())
}

pub(super) fn spawn_drop_preview_scheduler(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
    playback_generation: u64,
) {
    tokio::spawn(async move {
        if let Err(error) =
            schedule_drop_preview_for_pair(state, handle, lookahead, playback_generation).await
        {
            warn!("Drop preview scheduling skipped: {error:?}");
        }
    });
}

pub(super) fn drop_preview_pair_is_current(
    state: &crate::AppState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
    lookahead: &player::DjLookaheadStart,
    playback_generation: u64,
) -> bool {
    state
        .playback_runtime
        .as_ref()
        .is_some_and(|runtime| runtime.handle.is_same_runtime(handle))
        && current_playback_generation(state) == playback_generation
        && state
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id)
            == lookahead
                .current
                .as_ref()
                .and_then(|media| media.track_id())
        && active_dj_lookahead_start_for_state(state).is_some_and(|pair| {
            pair.current == lookahead.current
                && pair.next == lookahead.next
                && pair.current_queue_item_id == lookahead.current_queue_item_id
                && pair.next_queue_item_id == lookahead.next_queue_item_id
                && pair.queue_generation == lookahead.queue_generation
        })
}

pub(super) async fn schedule_drop_preview_for_pair(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
    playback_generation: u64,
) -> anyhow::Result<()> {
    let Some(current_ref) = lookahead.current.clone() else {
        return Ok(());
    };
    let Some(next_ref) = lookahead.next.clone() else {
        return Ok(());
    };
    let (Some(current_track_id), Some(next_track_id)) =
        (current_ref.track_id(), next_ref.track_id())
    else {
        return Ok(());
    };
    let (next, runtime_info, user_quality) = {
        let state_guard = state.read().await;
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        if !drop_preview_pair_is_current(&state_guard, &handle, &lookahead, playback_generation) {
            return Ok(());
        }
        let current = state_guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, current_track_id))?
            .context("drop preview current track missing")?;
        let next = state_guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, next_track_id))?
            .context("drop preview next track missing")?;
        let plan = state_guard.db.with_conn(|conn| {
            dj_routes::drop_preview_plan_for_pair(
                conn,
                &current_ref,
                &next_ref,
                current.duration_ms,
            )
        })?;
        let Some(plan) = plan else {
            return Ok(());
        };
        let info = state_guard
            .playback_runtime_info
            .clone()
            .context("playback runtime info missing")?;
        let position_ms = handle.get_position_ms(info.sample_rate, info.channels);
        if position_ms >= plan.planned_fire_ms {
            return Ok(());
        }
        (
            next,
            (info.sample_rate, info.channels, plan),
            current_user_audio_quality_locked(&state_guard),
        )
    };

    let stream_request = match player::build_tidal_stream_request(&next, user_quality.clone()) {
        Some(request) => request,
        None => return Ok(()),
    };
    if !claim_drop_preview_arm(
        Arc::as_ptr(&state) as usize,
        current_track_id,
        next_track_id,
        lookahead.queue_generation,
        playback_generation,
    ) {
        return Ok(());
    }
    let stream_info = match resolve_tidal_playback_stream(&state, &next, &stream_request).await {
        Ok(info) => Some(info),
        Err(error) => {
            warn!(
                "Skipping drop preview for next track {}: {}",
                next.id,
                describe_tidal_playback_error(&error)
            );
            return Ok(());
        }
    };

    let (sample_rate, channels, plan) = runtime_info;
    let engine = {
        let state_guard = state.read().await;
        crate::playback::dj_engine::DjEngine::new(state_guard.db.clone())
    };
    let Some(program) = engine.plan_drop_preview(
        &current_ref,
        &next_ref,
        stream_info
            .as_ref()
            .and_then(|info| info.sample_rate_hz())
            .unwrap_or(sample_rate),
        channels,
        DROP_PREVIEW_DURATION_MS,
    )?
    else {
        return Ok(());
    };

    let mut job = player::build_playback_preparation(&next, stream_info.as_ref(), 0, user_quality)
        .with_generation(playback_generation)
        .with_dj_media_ref(next_ref.clone())
        .with_prepared_transition(player::PreparedTransitionProgram {
            program,
            transition_event_id: None,
            fire_ahead_ms: 0,
            queue_generation: lookahead.queue_generation,
            current_queue_item_id: lookahead.current_queue_item_id,
            next_queue_item_id: lookahead.next_queue_item_id,
            anchor_start_ms: Some(plan.planned_fire_ms),
        });
    job.gapless = crate::playback::gapless::GaplessPlan::disabled();

    {
        let state_guard = state.read().await;
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        if !drop_preview_pair_is_current(&state_guard, &handle, &lookahead, playback_generation) {
            return Ok(());
        }
        let info = state_guard
            .playback_runtime_info
            .as_ref()
            .context("playback runtime info missing")?;
        if info.sample_rate != sample_rate || info.channels != channels {
            return Ok(());
        }
        let position_ms = handle.get_position_ms(info.sample_rate, info.channels);
        if position_ms >= plan.planned_fire_ms {
            return Ok(());
        }
    }

    let trigger_position_samples =
        samples_from_ms_for_runtime(plan.planned_fire_ms, sample_rate, channels);
    handle.prepare_drop_preview(job)?;
    handle.arm_drop_preview(
        current_track_id,
        playback_generation,
        trigger_position_samples,
    )?;
    info!(
        current_track_id,
        next_track_id = next.id,
        planned_fire_ms = plan.planned_fire_ms,
        incoming_drop_ms = plan.incoming_drop_ms,
        source = %plan.source,
        "Drop preview armed"
    );
    Ok(())
}

pub(super) fn samples_from_ms_for_runtime(ms: i64, sample_rate: u32, channels: u16) -> u64 {
    let ms = ms.max(0) as u64;
    ms.saturating_mul(sample_rate.max(1) as u64)
        .saturating_mul(channels.max(1) as u64)
        / 1000
}

pub(super) fn claim_drop_preview_arm(
    state_id: usize,
    current_track_id: i64,
    next_track_id: i64,
    queue_generation: u64,
    playback_generation: u64,
) -> bool {
    let attempts = DROP_PREVIEW_ARM_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let now = Instant::now();
    let mut attempts = attempts.lock().unwrap_or_else(|error| error.into_inner());
    claim_drop_preview_arm_at(
        &mut attempts,
        state_id,
        current_track_id,
        next_track_id,
        queue_generation,
        playback_generation,
        now,
    )
}

pub(super) fn claim_drop_preview_arm_at(
    attempts: &mut HashMap<DropPreviewArmKey, Instant>,
    state_id: usize,
    current_track_id: i64,
    next_track_id: i64,
    queue_generation: u64,
    playback_generation: u64,
    now: Instant,
) -> bool {
    let retry_after = Duration::from_secs(DROP_PREVIEW_ARM_RETRY_SECS);
    attempts.retain(|_, last_attempt| now.duration_since(*last_attempt) < retry_after);
    let key = (
        state_id,
        current_track_id,
        next_track_id,
        queue_generation,
        playback_generation,
    );
    if let Some(last_attempt) = attempts.get(&key)
        && now.duration_since(*last_attempt) < retry_after
    {
        return false;
    }
    attempts.insert(key, now);
    true
}

pub(crate) fn refresh_dj_after_queue_change(
    state: SharedState,
    context: &'static str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    // Preparation can remove an unavailable next row and request a new pair.
    // Box this scheduling boundary so that bounded retry has no recursive
    // opaque-future type through the spawned preparation task.
    Box::pin(async move {
        let runtime = {
            let state_guard = state.read().await;
            state_guard
                .playback_runtime
                .as_ref()
                .map(|runtime| runtime.handle.clone())
        };
        if let Some(runtime) = runtime {
            start_dj_lookahead_and_queue_profiles_after_pair_change(state, runtime, context).await;
        } else {
            queue_missing_dj_profiles_after_pair_change(state, context).await;
        }
    })
}

pub(crate) fn active_dj_pair_for_state_and_conn(
    _state: &crate::AppState,
    conn: &rusqlite::Connection,
) -> anyhow::Result<crate::playback::dj_lookahead::DjLookaheadPair> {
    crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)
}
