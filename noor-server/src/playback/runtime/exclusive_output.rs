//! WASAPI exclusive-mode render sources and sink startup.

use super::*;

/// Observe the render-thread marker between commands so output loss pauses
/// the queue before the generic no-progress watchdog can skip a healthy track.
pub(super) fn poll_exclusive_output_fault(
    state: &mut PlaybackRuntimeLoopState,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) {
    let Some(stream) = state.exclusive_sink.stream.as_ref() else {
        return;
    };
    let Some(fault) = stream.take_fault() else {
        return;
    };
    let output_live = stream.output_live();
    let Some(engine) = state
        .engine
        .as_ref()
        .or(state.next_engine.as_ref())
        .or(state.fading_out_engine.as_ref())
        .or(state.drop_preview_engine.as_ref())
    else {
        return;
    };
    let generation = engine.generation;
    let decoder_stop = engine.shared.stop_flag();
    hard_pause_for_output_fault(
        state,
        event_tx,
        generation,
        decoder_stop,
        output_live,
        fault.user_message().to_string(),
    );
}

fn live_engines_mut(
    state: &mut PlaybackRuntimeLoopState,
) -> impl Iterator<Item = &mut PlaybackEngine> {
    [
        state.engine.as_mut(),
        state.next_engine.as_mut(),
        state.fading_out_engine.as_mut(),
        state.drop_preview_engine.as_mut(),
    ]
    .into_iter()
    .flatten()
}

fn retarget_exclusive_engines<'a>(
    engines: impl Iterator<Item = &'a mut PlaybackEngine>,
    sample_rate: u32,
) -> Result<()> {
    for engine in engines {
        engine::require_decoded_output_rate(&engine.shared, sample_rate)?;
        engine
            .shared
            .target_sample_rate
            .store(sample_rate, Ordering::Relaxed);
    }
    Ok(())
}

fn rebuild_shared_engine_outputs<'a>(
    engines: impl Iterator<Item = &'a mut PlaybackEngine>,
    mut install: impl FnMut(&mut PlaybackEngine) -> Result<u32>,
) -> Result<Option<u32>> {
    let mut actual_rate = None;
    let mut failure = None;
    for engine in engines {
        match install(engine) {
            Ok(rate) => {
                actual_rate.get_or_insert(rate);
            }
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(actual_rate),
    }
}

/// An exclusive preference can fail after decoders already exist. Every deck
/// needs a shared stream before any prepared deck can be promoted audibly.
#[allow(clippy::too_many_arguments)]
pub(super) fn install_shared_fallback(
    state: &mut PlaybackRuntimeLoopState,
    device: &cpal::Device,
    output_config: &mut StreamConfig,
    output_sample_format: SampleFormat,
    desired_sample_rate: Option<u32>,
    command_tx: &mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: &tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<()> {
    state.exclusive_sink.clear();
    // The preference remains in settings and an explicit retry requests it
    // again. Newly prepared decks must use the backend actually rendering.
    state.current_exclusive = false;
    let grace_secs = state.current_exclusive_release_grace_secs;
    let mut selected_rate = desired_sample_rate;
    let actual_rate = rebuild_shared_engine_outputs(live_engines_mut(state), |engine| {
        let rate = engine.swap_stream(
            device,
            output_config,
            output_sample_format,
            command_tx.clone(),
            event_tx.clone(),
            false,
            selected_rate,
            grace_secs,
        )?;
        selected_rate = Some(rate);
        Ok(rate)
    })?;
    if let Some(actual_rate) = actual_rate {
        output_config.sample_rate = actual_rate;
        state.device_sample_rate = actual_rate;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(track_id: i64) -> PlaybackEngine {
        let (command_tx, _) = mpsc::channel();
        PlaybackEngine::test_with_shared(
            track_id,
            1,
            Arc::new(PlaybackSharedState::new(
                track_id,
                1,
                crate::playback::player::PlaybackSourceKind::TidalStream,
                GaplessPlan::disabled(),
                96_000,
                2,
                None,
                command_tx,
                Arc::new(AtomicU32::new(1.0f32.to_bits())),
                Arc::new(AtomicU64::new(0)),
                Arc::new(AtomicU64::new(0)),
            )),
        )
    }

    #[test]
    fn shared_fallback_installs_output_for_every_live_deck() {
        let mut engines = [engine(1), engine(2), engine(3), engine(4)];
        engines[1].shared.paused.store(true, Ordering::SeqCst);
        let mut installed = Vec::new();
        let rate = rebuild_shared_engine_outputs(engines.iter_mut(), |engine| {
            installed.push(engine.track_id);
            engine
                .shared
                .target_sample_rate
                .store(48_000, Ordering::Relaxed);
            Ok(48_000)
        })
        .unwrap();
        assert_eq!(installed, [1, 2, 3, 4]);
        assert_eq!(rate, Some(48_000));
        assert!(engines[1].shared.paused.load(Ordering::SeqCst));
    }

    #[test]
    fn shared_fallback_surfaces_failure_and_still_rebuilds_other_decks() {
        let mut engines = [engine(1), engine(2), engine(3)];
        let mut installed = Vec::new();
        let result = rebuild_shared_engine_outputs(engines.iter_mut(), |engine| {
            installed.push(engine.track_id);
            if engine.track_id == 1 {
                Err(anyhow!("output unavailable"))
            } else {
                Ok(48_000)
            }
        });
        assert!(result.is_err());
        assert_eq!(installed, [1, 2, 3]);
    }

    #[test]
    fn shared_fallback_reports_active_rate_instead_of_last_prepared_rate() {
        let mut engines = [engine(1), engine(2)];
        let rate = rebuild_shared_engine_outputs(engines.iter_mut(), |engine| {
            Ok(if engine.track_id == 1 { 48_000 } else { 96_000 })
        })
        .unwrap();
        assert_eq!(rate, Some(48_000));
    }

    #[test]
    fn exclusive_acquisition_rejects_mismatched_decoded_rates() {
        let mut engines = [engine(1), engine(2), engine(3), engine(4)];
        for engine in &engines {
            engine
                .shared
                .target_sample_rate
                .store(48_000, Ordering::Relaxed);
        }
        assert!(retarget_exclusive_engines(engines.iter_mut(), 96_000).is_err());
        assert!(
            engines
                .iter()
                .all(|engine| engine.shared.target_sample_rate.load(Ordering::Relaxed) == 48_000)
        );
        assert!(retarget_exclusive_engines(engines.iter_mut(), 48_000).is_ok());
    }
}

pub(super) fn exclusive_render_sources(
    active: Option<&PlaybackEngine>,
    prepared: Option<&PlaybackEngine>,
    fading: Option<&PlaybackEngine>,
    drop_preview: Option<&PlaybackEngine>,
) -> Vec<ExclusiveRenderSource> {
    let mut sources = Vec::new();
    if let Some(engine) = active {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Active,
            shared: Arc::clone(&engine.shared),
        });
    }
    if let Some(engine) = prepared {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Prepared,
            shared: Arc::clone(&engine.shared),
        });
    }
    if let Some(engine) = fading {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Fading,
            shared: Arc::clone(&engine.shared),
        });
    }
    if let Some(engine) = drop_preview {
        sources.push(ExclusiveRenderSource {
            role: ExclusiveRenderRole::Prepared,
            shared: Arc::clone(&engine.shared),
        });
    }
    sources
}

pub(super) fn refresh_exclusive_sources(state: &PlaybackRuntimeLoopState) {
    state
        .exclusive_sink
        .source_bank
        .set_sources(exclusive_render_sources(
            state.engine.as_ref(),
            state.next_engine.as_ref(),
            state.fading_out_engine.as_ref(),
            state.drop_preview_engine.as_ref(),
        ));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ensure_exclusive_sink_started(
    state: &mut PlaybackRuntimeLoopState,
    device: &cpal::Device,
    output_config: &StreamConfig,
    desired_sample_rate: Option<u32>,
    exclusive_release_grace_secs: u32,
    exclusive_latency_mode: ExclusiveLatencyMode,
    command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
) -> Result<u32> {
    let exclusive_plan =
        swap_stream_plan(output_config, desired_sample_rate, SwapBackend::Exclusive);
    if let Some(stream) = state
        .exclusive_sink
        .stream
        .as_ref()
        .filter(|stream| !stream.is_released())
    {
        return Ok(stream.effective_sample_rate);
    }
    state.exclusive_sink.stream = None;
    retarget_exclusive_engines(
        live_engines_mut(state),
        exclusive_plan.stream_config.sample_rate,
    )?;

    // Shared fallback streams must relinquish the endpoint before this process
    // attempts exclusive acquisition. Hold transport intent until startup ends.
    let pause_guards: Vec<_> = live_engines_mut(state)
        .map(|engine| {
            let guard = engine::SwapPauseGuard::new(Arc::clone(&engine.shared));
            engine.drop_stream();
            guard
        })
        .collect();

    let device_label = device_display_name(device);
    let device_id = device
        .id()
        .context("failed to identify exclusive output endpoint")?
        .id()
        .to_owned();
    match build_exclusive_stream(
        Some(device_id.as_str()),
        device_label.clone(),
        exclusive_plan.stream_config.sample_rate,
        exclusive_plan.stream_config.channels,
        exclusive_release_grace_secs,
        exclusive_latency_mode,
        Arc::clone(&state.exclusive_sink.source_bank),
        command_tx,
        event_tx.clone(),
    ) {
        Ok(stream) => {
            let actual_rate = stream.effective_sample_rate;
            let transport_format = stream.transport_format.clone();
            state.exclusive_sink.stream = Some(stream);
            retarget_exclusive_engines(live_engines_mut(state), actual_rate)?;
            drop(pause_guards);
            let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeEngaged {
                device_name: device_label,
                transport_format,
            });
            Ok(actual_rate)
        }
        Err(failure) => {
            let reason = failure.user_message();
            warn!("WASAPI exclusive grab failed; falling back to cpal shared: {reason}");
            let _ = event_tx.send(PlaybackRuntimeEvent::ExclusiveModeFailed {
                reason: reason.clone(),
                device_name: device_label,
            });
            Err(anyhow!(reason))
        }
    }
}
