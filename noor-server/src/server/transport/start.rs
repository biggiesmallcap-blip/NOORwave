//! Starting one track: the single primitive every transport path uses.
//! Resolve the stream, re-check the playback generation, acquire the runtime,
//! dispatch the job, record the stream display. Callers own recovery.

use crate::SharedState;
use crate::playback::player;
use crate::services::tidal::stream::StreamInfo;

use super::generation;
use super::runtime::{RuntimeUnavailable, ensure_for_track};
use super::stream::{TidalPlaybackError, resolve_tidal_playback_stream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dispatch {
    /// Start fresh (`PlaybackRuntimeHandle::play`).
    Play,
    /// Replace what is playing (`PlaybackRuntimeHandle::switch_to`).
    Switch,
}

/// Which playback generation the started job runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Generation {
    /// A user transport command already bumped to this generation, so older
    /// work stops applying right away.
    Claimed(u64),
    /// Background work (a quality re-issue) observed this generation. It
    /// claims a new one only once the stream resolved: a failed resolve then
    /// leaves the playing track's generation alone, and a transport command
    /// that arrived meanwhile wins.
    ClaimWhenReady { observed: u64 },
}

pub(crate) struct StartRequest<'a> {
    pub track: &'a crate::db::models::Track,
    pub generation: Generation,
    pub dispatch: Dispatch,
    pub crossfade_ms: i32,
}

#[derive(Debug)]
pub(crate) struct Started {
    pub stream_info: StreamInfo,
}

#[derive(Debug)]
pub(crate) enum StartError {
    /// A local-library track: there is no TIDAL stream to play.
    LocalUnsupported,
    /// Stream resolution failed. `error.is_track_unplayable()` says whether
    /// skipping this row is the right recovery.
    Stream(TidalPlaybackError),
    /// A newer transport command arrived while resolving; nothing dispatched.
    Superseded,
    /// No runtime could be acquired.
    Runtime(RuntimeUnavailable),
    /// The runtime refused the job.
    Dispatch {
        dispatch: Dispatch,
        error: anyhow::Error,
    },
}

pub(crate) async fn start_track(
    state: &SharedState,
    request: StartRequest<'_>,
) -> Result<Started, StartError> {
    start_track_inner(state, request, None).await
}

pub(crate) async fn start_track_for_runtime(
    state: &SharedState,
    request: StartRequest<'_>,
    expected_runtime: &crate::playback::runtime::PlaybackRuntimeHandle,
    expected_source: &std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<Started, StartError> {
    start_track_inner(state, request, Some((expected_runtime, expected_source))).await
}

async fn start_track_inner(
    state: &SharedState,
    request: StartRequest<'_>,
    expected_runtime: Option<(
        &crate::playback::runtime::PlaybackRuntimeHandle,
        &std::sync::Arc<std::sync::atomic::AtomicU64>,
    )>,
) -> Result<Started, StartError> {
    let StartRequest {
        track,
        generation: start_generation,
        dispatch,
        crossfade_ms,
    } = request;
    let user_quality = super::settings::current_user_audio_quality(state).await;
    let Some(stream_request) = player::build_tidal_stream_request(track, user_quality.clone())
    else {
        return Err(StartError::LocalUnsupported);
    };
    let resolved = resolve_tidal_playback_stream(state, track, &stream_request).await;
    if let Some((expected, expected_source)) = expected_runtime {
        let guard = state.read().await;
        if !guard
            .playback_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.handle.is_same_runtime(expected))
        {
            // A failed lookup is stale too: it must not pause the replacement.
            return Err(StartError::Superseded);
        }
        let source_guard = expected.lock_playback_source_identity();
        if !std::sync::Arc::ptr_eq(&source_guard, expected_source) {
            return Err(StartError::Superseded);
        }
    }
    // A failed lookup can trigger caller-side skip or pause recovery. Validate
    // ownership before returning that error, just as for a successful result.
    let observed_generation = match start_generation {
        Generation::Claimed(claimed) => claimed,
        Generation::ClaimWhenReady { observed } => observed,
    };
    if !generation::is_current(state, observed_generation).await {
        return Err(StartError::Superseded);
    }
    let stream_info = resolved.map_err(StartError::Stream)?;
    let start_generation = match start_generation {
        Generation::Claimed(claimed) => claimed,
        Generation::ClaimWhenReady { observed } => generation::claim_if_current(state, observed)
            .await
            .ok_or(StartError::Superseded)?,
    };
    let handle = match expected_runtime {
        Some((expected, _)) => expected.clone(),
        None => ensure_for_track(state).await.map_err(StartError::Runtime)?,
    };
    // Transport intent is re-read at dispatch: a pause that landed during the
    // stream resolve wins, so the engine comes up silent instead of playing
    // under a paused UI (last user action wins).
    let mut state_guard = state.write().await;
    if generation::current(&state_guard) != start_generation
        || !state_guard
            .playback_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
    {
        return Err(StartError::Superseded);
    }
    let is_playing = state_guard
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT is_playing FROM playback_state WHERE id=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .map(|value| value != 0)
        .unwrap_or(true);
    let source_guard =
        expected_runtime.map(|(expected, _)| expected.lock_playback_source_identity());
    if let Some((_, expected_source)) = expected_runtime {
        if source_guard
            .as_ref()
            .is_none_or(|current| !std::sync::Arc::ptr_eq(current, expected_source))
        {
            return Err(StartError::Superseded);
        }
    }
    let job =
        player::build_playback_preparation(track, Some(&stream_info), crossfade_ms, user_quality)
            .with_generation(start_generation)
            .with_start_paused(!is_playing);
    let sent = match dispatch {
        Dispatch::Play => handle.play(job),
        Dispatch::Switch => handle.switch_to(job),
    };
    sent.map_err(|error| StartError::Dispatch { dispatch, error })?;
    state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
        audio_quality: stream_info.audio_quality.clone(),
        sample_rate: stream_info.sample_rate,
        bit_depth: stream_info.bit_depth,
    });
    state_guard.pending_stream_display = None;
    if expected_runtime.is_some() {
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.active_track_id = Some(track.id);
            super::events::clear_error_after_owned_dispatch(info);
        }
        let _ = state_guard
            .event_tx
            .send(crate::AppEvent::TrackChanged { track_id: track.id });
        let _ = state_guard
            .event_tx
            .send(crate::AppEvent::PlaybackStateChanged);
        let _ = state_guard.event_tx.send(crate::AppEvent::QueueUpdated);
    }
    Ok(Started { stream_info })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::runtime as playback_runtime;
    use crate::server::routes::tests::{fresh_migrated_db, fresh_test_state};
    use crate::server::transport::stream::{ScriptedStreamSource, test_stream_info};
    use crate::services::tidal::stream::StreamResolveError;
    use std::sync::mpsc;

    struct Fixture {
        state: SharedState,
        track: crate::db::models::Track,
        commands: mpsc::Receiver<playback_runtime::PlaybackRuntimeCommand>,
        stream: std::sync::Arc<ScriptedStreamSource>,
    }

    async fn fixture(
        tidal_id: Option<i64>,
        stream: std::sync::Arc<ScriptedStreamSource>,
    ) -> Fixture {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (9100, 'Start Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES (9101, 'Start Track', 9100, 180000, ?1, 'tidal', 'tidal')",
                rusqlite::params![tidal_id],
            )?;
            conn.execute("UPDATE playback_state SET is_playing = 1 WHERE id = 1", [])?;
            Ok(())
        })
        .unwrap();
        let track = db
            .with_conn(|conn| crate::playback::queue::get_track_by_id(conn, 9101))
            .unwrap()
            .unwrap();
        let mut app = fresh_test_state(db);
        app.stream_source = stream.clone();
        app.tidal
            .set_tokens_for_test(Some(crate::services::tidal::auth::TidalTokens {
                access_token: "test-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: 3600,
                user_id: "test-user".to_string(),
                country_code: "US".to_string(),
                auth_flow: Some("pkce".to_string()),
            }));
        let (command_tx, commands) = mpsc::channel();
        app.playback_runtime = Some(crate::PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        Fixture {
            state: std::sync::Arc::new(tokio::sync::RwLock::new(app)),
            track,
            commands,
            stream,
        }
    }

    fn request(
        track: &crate::db::models::Track,
        generation: u64,
        dispatch: Dispatch,
    ) -> StartRequest<'_> {
        StartRequest {
            track,
            generation: Generation::Claimed(generation),
            dispatch,
            crossfade_ms: 0,
        }
    }

    #[tokio::test]
    async fn starts_track_with_resolved_stream_and_records_display() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Ok(test_stream_info(99101))]),
        )
        .await;
        let generation = generation::bump(&fx.state).await;
        let started = start_track(&fx.state, request(&fx.track, generation, Dispatch::Play))
            .await
            .unwrap();
        assert_eq!(started.stream_info.track_id, 99101);
        match fx.commands.try_recv().expect("play command") {
            playback_runtime::PlaybackRuntimeCommand::Play(job) => {
                assert_eq!(job.track.id, 9101);
                assert_eq!(job.generation, generation);
                assert!(job.resolved_stream.is_some());
                assert!(!job.start_paused);
            }
            other => panic!("expected Play, got {other:?}"),
        }
        let state = fx.state.read().await;
        let display = state.current_stream_display.clone().unwrap();
        assert_eq!(display.audio_quality, "LOSSLESS");
        assert!(state.pending_stream_display.is_none());
    }

    #[tokio::test]
    async fn switch_dispatch_sends_switch() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Ok(test_stream_info(99101))]),
        )
        .await;
        let generation = generation::bump(&fx.state).await;
        start_track(&fx.state, request(&fx.track, generation, Dispatch::Switch))
            .await
            .unwrap();
        assert!(matches!(
            fx.commands.try_recv().unwrap(),
            playback_runtime::PlaybackRuntimeCommand::Switch(_)
        ));
    }

    #[tokio::test]
    async fn newer_command_during_resolve_supersedes_without_dispatch() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Ok(test_stream_info(99101))]),
        )
        .await;
        let stale = generation::bump(&fx.state).await;
        generation::bump(&fx.state).await;
        let err = start_track(&fx.state, request(&fx.track, stale, Dispatch::Play))
            .await
            .unwrap_err();
        assert!(matches!(err, StartError::Superseded));
        assert!(fx.commands.try_recv().is_err(), "nothing dispatched");
        assert!(fx.state.read().await.current_stream_display.is_none());
    }

    #[tokio::test]
    async fn unplayable_asset_is_reported_as_stream_error() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Err(StreamResolveError::StreamRejected {
                message:
                    r#"{"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#
                        .to_string(),
            })]),
        )
        .await;
        let generation = generation::bump(&fx.state).await;
        let err = start_track(&fx.state, request(&fx.track, generation, Dispatch::Play))
            .await
            .unwrap_err();
        match err {
            StartError::Stream(error) => assert!(error.is_track_unplayable()),
            other => panic!("expected Stream, got {other:?}"),
        }
        assert!(fx.commands.try_recv().is_err());
    }

    #[tokio::test]
    async fn stale_lookup_failure_is_superseded_before_recovery() {
        for quality_reissue in [false, true] {
            let fx = fixture(
                Some(99101),
                ScriptedStreamSource::new(vec![Err(StreamResolveError::StreamRejected {
                    message: "track-specific rejection".into(),
                })]),
            )
            .await;
            let old = generation::bump(&fx.state).await;
            let current = generation::bump(&fx.state).await;
            let start_request = if quality_reissue {
                reissue(&fx.track, old)
            } else {
                request(&fx.track, old, Dispatch::Switch)
            };
            let result = start_track(&fx.state, start_request).await;
            assert!(matches!(result, Err(StartError::Superseded)), "{result:?}");
            assert_eq!(generation::observe(&fx.state).await, current);
            assert!(fx.commands.try_recv().is_err());
            assert!(fx.state.read().await.current_stream_display.is_none());
        }
    }

    #[tokio::test]
    async fn newer_generation_while_acquiring_runtime_prevents_dispatch_and_display() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Ok(test_stream_info(99101))]),
        )
        .await;
        let claimed = generation::bump(&fx.state).await;
        let guard = fx.state.read().await;
        let state = fx.state.clone();
        let track = fx.track.clone();
        let pending = tokio::spawn(async move {
            start_track(&state, request(&track, claimed, Dispatch::Play)).await
        });
        // Reads and resolution can finish; runtime acquisition needs the write
        // guard that this reader deliberately keeps unavailable.
        tokio::task::yield_now().await;
        assert_eq!(fx.stream.call_count(), 1);
        guard
            .playback_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        drop(guard);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(StartError::Superseded)));
        assert!(fx.commands.try_recv().is_err());
        assert!(fx.state.read().await.current_stream_display.is_none());
    }

    #[tokio::test]
    async fn local_track_is_unsupported_without_resolving() {
        let fx = fixture(None, ScriptedStreamSource::offline()).await;
        let generation = generation::bump(&fx.state).await;
        let err = start_track(&fx.state, request(&fx.track, generation, Dispatch::Play))
            .await
            .unwrap_err();
        assert!(matches!(err, StartError::LocalUnsupported));
        assert_eq!(fx.stream.call_count(), 0);
    }

    #[tokio::test]
    async fn paused_transport_starts_the_job_paused() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Ok(test_stream_info(99101))]),
        )
        .await;
        fx.state
            .read()
            .await
            .db
            .with_conn(|conn| {
                conn.execute("UPDATE playback_state SET is_playing = 0 WHERE id = 1", [])?;
                Ok(())
            })
            .unwrap();
        let generation = generation::bump(&fx.state).await;
        start_track(&fx.state, request(&fx.track, generation, Dispatch::Play))
            .await
            .unwrap();
        match fx.commands.try_recv().unwrap() {
            playback_runtime::PlaybackRuntimeCommand::Play(job) => assert!(job.start_paused),
            other => panic!("expected Play, got {other:?}"),
        }
    }

    fn reissue(track: &crate::db::models::Track, observed: u64) -> StartRequest<'_> {
        StartRequest {
            track,
            generation: Generation::ClaimWhenReady { observed },
            dispatch: Dispatch::Switch,
            crossfade_ms: 0,
        }
    }

    #[tokio::test]
    async fn failed_reissue_leaves_the_playing_generation_alone() {
        // A quality change whose stream lookup fails must not strand the
        // playing track under a stale generation (its Finished would be
        // dropped and the queue would never advance).
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Err(StreamResolveError::StreamRejected {
                message: "lookup failed".to_string(),
            })]),
        )
        .await;
        let playing = generation::bump(&fx.state).await;
        let err = start_track(&fx.state, reissue(&fx.track, playing))
            .await
            .unwrap_err();
        assert!(matches!(err, StartError::Stream(_)));
        assert!(fx.commands.try_recv().is_err(), "nothing dispatched");
        assert_eq!(generation::observe(&fx.state).await, playing);
    }

    #[tokio::test]
    async fn successful_reissue_claims_the_next_generation() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Ok(test_stream_info(99101))]),
        )
        .await;
        let playing = generation::bump(&fx.state).await;
        start_track(&fx.state, reissue(&fx.track, playing))
            .await
            .unwrap();
        assert_eq!(generation::observe(&fx.state).await, playing + 1);
        match fx.commands.try_recv().unwrap() {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                assert_eq!(job.generation, playing + 1)
            }
            other => panic!("expected Switch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_command_during_reissue_wins() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Ok(test_stream_info(99101))]),
        )
        .await;
        let observed = generation::bump(&fx.state).await;
        let skip = generation::bump(&fx.state).await;
        let err = start_track(&fx.state, reissue(&fx.track, observed))
            .await
            .unwrap_err();
        assert!(matches!(err, StartError::Superseded));
        assert!(fx.commands.try_recv().is_err(), "nothing dispatched");
        assert_eq!(generation::observe(&fx.state).await, skip);
    }

    #[tokio::test]
    async fn failed_quality_change_keeps_the_playing_generation() {
        let fx = fixture(
            Some(99101),
            ScriptedStreamSource::new(vec![Err(StreamResolveError::StreamRejected {
                message: "lookup failed".to_string(),
            })]),
        )
        .await;
        fx.state
            .read()
            .await
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE playback_state SET current_track_id = 9101 WHERE id = 1",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let playing = generation::bump(&fx.state).await;
        let result = super::super::settings::reissue_current_track_at_new_quality(&fx.state).await;
        assert!(result.is_err(), "the failed lookup is reported");
        assert_eq!(fx.stream.call_count(), 1, "the re-issue did resolve");
        assert!(fx.commands.try_recv().is_err(), "nothing dispatched");
        assert_eq!(generation::observe(&fx.state).await, playing);
    }
}
