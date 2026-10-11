//! Engine fade-out and stop, runtime error reporting and panic recovery.

use super::*;

/// Ceiling on how long a teardown will wait for retiring decks to finish their
/// fade. The ramp itself is TRANSPORT_FADE_MS; the rest is slack for a couple of
/// callback periods. A deck whose callback has stopped running (device gone,
/// stream never started) will never finish its ramp, so the wait must be capped
/// rather than driven by the audio thread.
pub(super) const FADE_OUT_WAIT_TIMEOUT: Duration = Duration::from_millis(40);
pub(super) const FADE_OUT_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Ramp every audible deck in `engines` down to silence, then stop them all.
///
/// `PlaybackEngine::stop` resets the buffer out from under the callback, so a
/// deck that is still making sound gets truncated mid-waveform - a step to zero,
/// which is the pop heard on skip and stop. Fading first costs one short,
/// bounded block of the command loop (which is a `recv_timeout` loop, not a
/// real-time one, so a few ms of added skip latency is inaudible).
///
/// The wait is synchronous on purpose. Parking retiring decks for a later
/// reaping tick would keep the outgoing engine's stream alive past this point,
/// and in WASAPI exclusive mode it still owns the device - the incoming engine
/// could not grab it. Finishing the fade here keeps teardown ordered.
///
/// Decks that are not audible (paused pre-decode, drop preview, already stopped)
/// report nothing to fade and are stopped immediately, so the common paths pay
/// no wait at all.
pub(super) fn fade_out_and_stop(mut engines: Vec<PlaybackEngine>) {
    let mut any_fading = false;
    for engine in engines.iter() {
        // Not `any()` - every deck must be armed, and `any()` short-circuits.
        any_fading |= engine.shared.begin_fade_out();
    }

    if any_fading {
        let deadline = Instant::now() + FADE_OUT_WAIT_TIMEOUT;
        while Instant::now() < deadline
            && engines
                .iter()
                .any(|engine| engine.shared.pause_fade_armed.load(Ordering::Relaxed))
        {
            std::thread::sleep(FADE_OUT_POLL_INTERVAL);
        }
    }

    for engine in engines.iter_mut() {
        engine.stop();
    }
}

pub(super) fn stop_current_engine(state: &mut PlaybackRuntimeLoopState) {
    state.dj.prepared_mixer = None;
    state.dj.prepared_drop_preview_mixer = None;
    fade_out_and_stop(state.engine.take().into_iter().collect());
}

pub(super) fn stop_all_engines(state: &mut PlaybackRuntimeLoopState) {
    state.dj.readiness_permanent_failure = None;
    state.dj.prepared_mixer = None;
    state.dj.prepared_drop_preview_mixer = None;
    // Retire as one batch so the audible decks share a single fade window
    // instead of serializing one after another.
    let mut retiring: Vec<PlaybackEngine> = Vec::new();
    retiring.extend(state.engine.take());
    retiring.extend(state.next_engine.take());
    retiring.extend(state.fading_out_engine.take());
    retiring.extend(state.drop_preview_engine.take());
    fade_out_and_stop(retiring);
}

pub(super) fn report_runtime_command_error(
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    command_name: &str,
    error: anyhow::Error,
) {
    let message = format!("{command_name} failed: {error}");
    warn!("{message}");
    let _ = event_tx.send(PlaybackRuntimeEvent::Error { message });
}

/// Surface a decode/source failure on the pre-buffered next track without
/// treating it as an active-track playback failure.
pub(super) fn emit_prepared_track_failure(
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    job: &PreparedPlaybackJob,
    message: &str,
) {
    let track_id = job.track.id;
    let tidal_id = match &job.source {
        crate::playback::player::PlaybackSourceRequest::TidalStream(request) => {
            Some(request.track_id)
        }
        crate::playback::player::PlaybackSourceRequest::LocalLibrary => None,
    };
    let surfaced = format!("Pre-buffered track {track_id} failed: {message}");
    warn!("{surfaced}");
    let _ = event_tx.send(PlaybackRuntimeEvent::PreparedTrackError {
        track_id,
        generation: job.generation,
        tidal_id,
        message: surfaced,
    });
}

pub(super) fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Handle a panic that escaped the runtime command dispatch. Emits a
/// PlaybackRuntimeEvent::Error, tears down any active engines under a nested
/// catch_unwind, and signals whether the loop can safely continue.
///
/// If the cleanup itself panics (mutex poisoning, etc.), we emit a final
/// error event and signal Break - re-entering the dispatch loop with state
/// that may be corrupt is more dangerous than ending the runtime thread.
pub(super) fn handle_panic_in_runtime_loop(
    payload: Box<dyn std::any::Any + Send>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    state: &mut PlaybackRuntimeLoopState,
) -> std::ops::ControlFlow<()> {
    let message = panic_payload_message(payload.as_ref());
    warn!("playback runtime panicked: {message}");

    let cleanup_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stop_all_engines(state);
        #[cfg(target_os = "windows")]
        state.exclusive_sink.clear();
    }));

    let _ = event_tx.send(PlaybackRuntimeEvent::Error {
        message: format!("playback runtime panicked: {message}"),
    });

    if cleanup_result.is_err() {
        let _ = event_tx.send(PlaybackRuntimeEvent::Error {
            message: "playback runtime panic cleanup also panicked; runtime exiting".to_string(),
        });
        std::ops::ControlFlow::Break(())
    } else {
        // After cleanup tore down every engine slot, signal Stopped so the UI's
        // playback-state machine snaps back to a clean idle (otherwise it would
        // remain stuck on the last Started state and the user sees a track
        // visually playing with no audio until the next user-initiated command).
        let _ = event_tx.send(PlaybackRuntimeEvent::Stopped);
        std::ops::ControlFlow::Continue(())
    }
}
