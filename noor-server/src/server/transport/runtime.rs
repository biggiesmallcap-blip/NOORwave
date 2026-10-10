//! Acquiring the host audio runtime for playback.

use crate::SharedState;
use crate::playback::runtime as playback_runtime;

/// Why no runtime could be had. Callers map this to their own HTTP response.
#[derive(Debug)]
pub(crate) enum RuntimeUnavailable {
    /// No TIDAL session: the runtime authenticates its own stream requests.
    NotConnected,
    /// Spawning the runtime failed (device, driver).
    SpawnFailed(String),
    /// Spawned but not present afterwards (should not happen).
    Missing,
}

pub(crate) async fn current(
    state: &SharedState,
) -> Option<playback_runtime::PlaybackRuntimeHandle> {
    let state = state.read().await;
    state
        .playback_runtime
        .as_ref()
        .map(|runtime| runtime.handle.clone())
        .filter(playback_runtime::PlaybackRuntimeHandle::is_healthy)
}

pub(crate) async fn ensure_for_track(
    state: &SharedState,
) -> Result<playback_runtime::PlaybackRuntimeHandle, RuntimeUnavailable> {
    let access_token = {
        let state = state.read().await;
        state.tidal.tokens().map(|tokens| tokens.access_token)
    }
    .ok_or(RuntimeUnavailable::NotConnected)?;

    let mut state_guard = state.write().await;
    let needs_respawn = state_guard
        .playback_runtime
        .as_ref()
        .map(|runtime| runtime.access_token != access_token || !runtime.handle.is_healthy())
        .unwrap_or(true);
    let mut spawned_handle = None;

    if needs_respawn {
        if let Some(runtime) = state_guard.playback_runtime.take() {
            let _ = runtime.handle.shutdown();
        }
        // A replacement runtime has not started audio yet. In particular, do
        // not let an old Started/Ready state survive until its first event.
        state_guard
            .audio_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        state_guard.playback_runtime_info = None;

        let dj_engine_enabled = state_guard
            .db
            .with_conn(crate::db::queries::is_dj_engine_enabled)
            .unwrap_or(false);
        let config = playback_runtime::PlaybackRuntimeConfig::new(
            state_guard.http_client.clone(),
            access_token.clone(),
            state_guard.analysis_tx.clone(),
        )
        .with_stream_resolver(super::stream::runtime_stream_resolver(state.clone()))
        .with_dj_analysis(dj_engine_enabled, state_guard.dj_analysis_tx.clone());
        let handle = playback_runtime::spawn_runtime(config).map_err(|error| {
            RuntimeUnavailable::SpawnFailed(format!("Failed to start host audio runtime: {error}"))
        })?;

        // Restore persisted volume to the new runtime.
        let persisted_volume = state_guard
            .db
            .with_conn(|conn| {
                let vol: f64 = conn.query_row(
                    "SELECT volume FROM playback_state WHERE id = 1",
                    [],
                    |row| row.get(0),
                )?;
                Ok(vol)
            })
            .unwrap_or(1.0);
        handle.set_volume(persisted_volume as f32);

        state_guard.playback_runtime = Some(crate::PlaybackRuntimeState {
            access_token,
            handle: handle.clone(),
        });
        spawned_handle = Some(handle.clone());
    }

    let handle = state_guard
        .playback_runtime
        .as_ref()
        .map(|runtime| runtime.handle.clone())
        .ok_or(RuntimeUnavailable::Missing)?;
    drop(state_guard);

    if let Some(listener_handle) = spawned_handle.clone() {
        super::events::spawn_playback_runtime_listener(state.clone(), listener_handle);
    }

    if let Some(runtime_handle) = spawned_handle.as_ref() {
        super::settings::apply_persisted_runtime_output_settings(state, runtime_handle).await;
    }

    Ok(handle)
}
