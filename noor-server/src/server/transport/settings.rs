//! Playback settings the transport reads at dispatch (crossfade, quality, transport intent, output device).

use super::{events::describe_tidal_playback_error, snapshot::current_playback_track_id};
use crate::SharedState;
use crate::db::queries;
use crate::playback::{queue, runtime as playback_runtime};
use crate::server::routes::effective_crossfade_for_exclusive;
use crate::server::transport::generation::bump as bump_playback_generation;
use crate::server::transport::runtime::current as current_playback_runtime;
use crate::server::transport::start::{Dispatch, StartError, StartRequest, start_track};
use tracing::warn;

/// Read the user's configured crossfade length from `playback_state`.
///
/// Every code path that calls `player::build_playback_preparation` to start a
/// track on the host audio runtime must source `crossfade_ms` through this
/// helper (or `effective_crossfade_ms` to apply the exclusive-mode policy).
/// Passing a hardcoded 0 outside that policy disables the per-engine fade-out
/// ramp and prevents `CrossfadeStart` from firing, which breaks crossfade
/// transitions.
/// Returns `configured` unless exclusive mode is on, in which case it returns 0.
/// Used by callsites that already have a snapshot's `crossfade_ms` and want the
/// same exclusive-mode override that `current_crossfade_ms` applies, without
/// re-querying `playback_state`.
pub(crate) async fn effective_crossfade_ms(state: &SharedState, configured: i32) -> i32 {
    let guard = state.read().await;
    let (exclusive, dj_engine_enabled) = guard
        .db
        .with_conn(|conn| {
            let settings = crate::db::audio_settings::load(conn)?;
            let dj_enabled = queries::is_dj_engine_enabled(conn)?;
            Ok((settings.exclusive_mode, dj_enabled))
        })
        .unwrap_or((false, false));
    effective_crossfade_for_exclusive(exclusive, dj_engine_enabled, configured)
}

pub(crate) async fn current_crossfade_ms(state: &SharedState) -> i32 {
    let guard = state.read().await;
    let configured = guard
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT crossfade_ms FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map_err(Into::into)
        })
        .unwrap_or(0);

    // Bit-perfect exclusive playback must not rewrite samples. DJ mode opts
    // into processing while keeping exclusive device ownership.
    let (exclusive, dj_engine_enabled) = guard
        .db
        .with_conn(|conn| {
            let settings = crate::db::audio_settings::load(conn)?;
            let dj_enabled = queries::is_dj_engine_enabled(conn)?;
            Ok((settings.exclusive_mode, dj_enabled))
        })
        .unwrap_or((false, false));
    effective_crossfade_for_exclusive(exclusive, dj_engine_enabled, configured)
}

pub(crate) async fn transport_intent_is_playing(state: &SharedState) -> bool {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT is_playing FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .map(|value| value != 0)
        .unwrap_or(true)
}

pub(crate) async fn current_user_audio_quality(
    state: &SharedState,
) -> Option<crate::db::audio_settings::AudioQuality> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
        .ok()
        .map(|s| s.quality)
}

pub(crate) fn runtime_output_settings_from_audio_settings(
    settings: &crate::db::audio_settings::AudioSettings,
) -> RuntimeOutputSettings {
    RuntimeOutputSettings {
        device: playback_runtime::OutputDeviceSelection::from_pref(
            settings.output_device.as_deref(),
        ),
        exclusive_mode: settings.exclusive_mode,
        sample_rate_follow: settings.sample_rate_follow,
        exclusive_release_grace_secs: settings.exclusive_release_grace_secs,
        exclusive_latency_mode: settings.exclusive_latency_mode.clone(),
    }
}

pub(crate) async fn apply_persisted_runtime_output_settings(
    state: &SharedState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
) {
    let settings = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
            .ok()
    };

    let Some(settings) = settings else {
        return;
    };
    let output = runtime_output_settings_from_audio_settings(&settings);
    if let Err(e) = handle.device_swap(
        output.device,
        output.exclusive_mode,
        output.sample_rate_follow,
        None,
        output.exclusive_release_grace_secs,
        output.exclusive_latency_mode,
    ) {
        warn!("Failed to apply persisted audio settings to playback runtime: {e}");
    }
}

pub(crate) fn should_skip_prebuffer_for_sample_rate_follow_format_change(
    exclusive_mode: bool,
    sample_rate_follow: bool,
    current_rate: u32,
    next_rate: Option<i32>,
    current_bit_depth: Option<i32>,
    next_bit_depth: Option<i32>,
) -> bool {
    if !sample_rate_follow {
        return false;
    }
    let rate_changes =
        next_rate.is_some_and(|next_rate| next_rate > 0 && next_rate as u32 != current_rate);
    let bit_depth_changes = matches!(
        (current_bit_depth, next_bit_depth),
        (Some(current), Some(next)) if current > 0 && next > 0 && current != next
    );
    rate_changes || (exclusive_mode && bit_depth_changes)
}

/// Re-resolve the currently-playing track at the user's current quality and
/// switch the runtime to it. Called after `put_audio_settings` when the user
/// flips the quality dropdown. Without this, quality changes don't take effect
/// until the next track and the user can't tell the setting did anything.
pub(crate) async fn reissue_current_track_at_new_quality(
    state: &SharedState,
) -> anyhow::Result<()> {
    let Some(track_id) = current_playback_track_id(state).await else {
        return Ok(());
    };

    let track = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, track_id))?
    };
    let Some(track) = track else {
        return Ok(());
    };

    // Re-issue only into a runtime that already exists; never spawn one for a
    // settings change.
    if current_playback_runtime(state).await.is_none() {
        return Ok(());
    }
    let crossfade_ms = current_crossfade_ms(state).await;
    let generation = bump_playback_generation(state).await;
    match start_track(
        state,
        StartRequest {
            track: &track,
            generation,
            dispatch: Dispatch::Switch,
            crossfade_ms,
        },
    )
    .await
    {
        Ok(_) | Err(StartError::LocalUnsupported) | Err(StartError::Superseded) => Ok(()),
        Err(StartError::Stream(error)) => Err(anyhow::anyhow!(
            "stream resolve failed: {}",
            describe_tidal_playback_error(&error)
        )),
        Err(StartError::Runtime(error)) => {
            Err(anyhow::anyhow!("playback runtime unavailable: {error:?}"))
        }
        Err(StartError::Dispatch { error, .. }) => Err(error),
    }
}

pub(crate) struct RuntimeOutputSettings {
    pub(crate) device: playback_runtime::OutputDeviceSelection,
    pub(crate) exclusive_mode: bool,
    pub(crate) sample_rate_follow: bool,
    pub(crate) exclusive_release_grace_secs: u32,
    pub(crate) exclusive_latency_mode: crate::db::audio_settings::ExclusiveLatencyMode,
}
