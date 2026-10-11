//! Transport-level tests: they drive transport functions directly with the
//! fake runtime (`PlaybackRuntimeHandle::test_with_command_tx`) and a test DB.

use super::generation::current as current_playback_generation;
use super::{events::*, pending::*, settings::*, snapshot::*};
use crate::AppEvent;
use crate::db::queries;
use crate::playback::{player, runtime as playback_runtime};
use crate::server::routes::tests::*;
use crate::server::routes::*;
use crate::services::tidal::{auth as tidal_auth, client::TidalTrack};
use axum::extract::State;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::PlaybackRuntimeState;
use crate::db::{Database, schema};

#[tokio::test]
async fn transport_ack_rechecks_runtime_after_state_lock_wait() {
    for runtime_playing in [false, true] {
        let (state, old_handle, _, _) = cpal_fault_fixture().await;
        let mut guard = state.write().await;
        guard
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE playback_state SET is_playing=?1 WHERE id=1",
                    rusqlite::params![i64::from(!runtime_playing)],
                )?;
                Ok(())
            })
            .unwrap();
        let before = guard.db.with_conn(player::load_snapshot).unwrap();
        let mut app_events = guard.event_tx.subscribe();
        let pending = tokio::spawn({
            let state = state.clone();
            async move { reconcile_runtime_transport_state(&state, &old_handle, runtime_playing).await }
        });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        let (command_tx, _command_rx) = std::sync::mpsc::channel();
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "replacement".into(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        drop(guard);
        tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap();
        let guard = state.read().await;
        let after = guard.db.with_conn(player::load_snapshot).unwrap();
        assert_eq!(
            after.state.is_playing, before.state.is_playing,
            "retired transport acknowledgment changed replacement intent"
        );
        assert_eq!(
            after.state.current_queue_item_id,
            before.state.current_queue_item_id
        );
        assert_eq!(after.queue.len(), before.queue.len());
        assert!(matches!(
            app_events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
}

#[tokio::test]
async fn current_runtime_transport_ack_reconciles_public_state_without_changing_queue() {
    let (state, _, _, _) = cpal_fault_fixture().await;
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    let before = {
        let mut guard = state.write().await;
        guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
        guard.db.with_conn(player::load_snapshot).unwrap()
    };
    let mut app_events = state.read().await.event_tx.subscribe();
    let listener = spawn_playback_runtime_listener(state.clone(), handle.clone());
    for runtime_playing in [false, true] {
        let event = if runtime_playing {
            playback_runtime::PlaybackRuntimeEvent::Resumed { track_id: Some(1) }
        } else {
            playback_runtime::PlaybackRuntimeEvent::Paused { track_id: Some(1) }
        };
        assert!(handle.test_publish_event(event));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), app_events.recv())
                .await
                .unwrap()
                .unwrap(),
            AppEvent::PlaybackStateChanged
        ));
        let guard = state.read().await;
        let after = guard.db.with_conn(player::load_snapshot).unwrap();
        assert_eq!(after.state.is_playing, runtime_playing);
        assert_eq!(
            after.state.current_queue_item_id,
            before.state.current_queue_item_id
        );
        assert_eq!(after.queue.len(), before.queue.len());
        drop(guard);
        reconcile_runtime_transport_state(&state, &handle, runtime_playing).await;
        assert!(matches!(
            app_events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
    listener.abort();
    assert!(listener.await.unwrap_err().is_cancelled());
}

async fn cpal_fault_fixture() -> (
    crate::SharedState,
    playback_runtime::PlaybackRuntimeHandle,
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let db = fresh_migrated_db();
    seed_basic_tracks(&db);
    db.with_conn(|conn| {
        let queue = player::enqueue_track(conn, 1, "test")?;
        conn.execute("UPDATE playback_state SET current_track_id=1, current_queue_item_id=?1, is_playing=1 WHERE id=1",
            rusqlite::params![queue[0].id])?;
        Ok(())
    }).unwrap();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    let (command_tx, _) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    {
        let mut guard = state.write().await;
        guard
            .playback_generation
            .store(0, std::sync::atomic::Ordering::Release);
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test".into(),
            handle: handle.clone(),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "DAC".into(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
        guard
            .audio_active
            .store(true, std::sync::atomic::Ordering::Release);
    }
    (
        state,
        handle,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    )
}

fn cpal_fault_event(
    stop: &Arc<std::sync::atomic::AtomicBool>,
    live: &Arc<std::sync::atomic::AtomicBool>,
) -> playback_runtime::PlaybackRuntimeEvent {
    playback_runtime::PlaybackRuntimeEvent::OutputFault {
        active_track_id: Some(1),
        generation: 0,
        active_decoder_stop: Some(Arc::clone(stop)),
        decoder_stop: Arc::clone(stop),
        output_live: Arc::clone(live),
        message: "device removed".into(),
    }
}

#[tokio::test]
async fn cpal_fault_consumer_pauses_db_and_audibility_preserving_queue() {
    let (state, handle, stop, live) = cpal_fault_fixture().await;
    let before = state
        .read()
        .await
        .db
        .with_conn(player::load_snapshot)
        .unwrap();
    assert!(handle_output_fault(&state, &handle, cpal_fault_event(&stop, &live)).await);
    let guard = state.read().await;
    let after = guard.db.with_conn(player::load_snapshot).unwrap();
    assert!(!after.state.is_playing);
    assert_eq!(
        after.state.current_queue_item_id,
        before.state.current_queue_item_id
    );
    assert_eq!(after.queue.len(), before.queue.len());
    assert!(
        !guard
            .audio_active
            .load(std::sync::atomic::Ordering::Acquire)
    );
    let info = guard.playback_runtime_info.as_ref().unwrap();
    assert_eq!(info.active_track_id, None);
    assert_eq!(info.last_error.as_deref(), Some("device removed"));
}

#[tokio::test]
async fn cpal_fault_consumer_drops_retired_output_and_same_job_decoder() {
    for retire_decoder in [false, true] {
        let (state, handle, stop, live) = cpal_fault_fixture().await;
        if retire_decoder {
            stop.store(true, std::sync::atomic::Ordering::Release);
        } else {
            live.store(false, std::sync::atomic::Ordering::Release);
        }
        assert!(!handle_output_fault(&state, &handle, cpal_fault_event(&stop, &live)).await);
        let guard = state.read().await;
        assert!(
            guard
                .db
                .with_conn(player::load_snapshot)
                .unwrap()
                .state
                .is_playing
        );
        assert!(
            guard
                .audio_active
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            guard
                .playback_runtime_info
                .as_ref()
                .unwrap()
                .last_error
                .is_none()
        );
    }
}

#[tokio::test]
async fn cpal_fault_waiting_for_state_lock_does_not_pause_replacement_queue_row() {
    let (state, handle, stop, live) = cpal_fault_fixture().await;
    let mut guard = state.write().await;
    let task_state = state.clone();
    let task_handle = handle.clone();
    let event = cpal_fault_event(&stop, &live);
    let consumer =
        tokio::spawn(async move { handle_output_fault(&task_state, &task_handle, event).await });
    tokio::task::yield_now().await;
    // Replace with another queue occurrence of the same track and generation.
    // Track IDs alone cannot tell this newer decoder from the faulting one.
    let replacement_row = guard
        .db
        .with_conn(|conn| {
            let queue = player::enqueue_track(conn, 1, "replacement")?;
            let replacement = queue.last().unwrap().id;
            conn.execute(
                "UPDATE playback_state SET current_queue_item_id=?1, is_playing=1 WHERE id=1",
                rusqlite::params![replacement],
            )?;
            Ok(replacement)
        })
        .unwrap();
    stop.store(true, std::sync::atomic::Ordering::Release);
    guard.playback_runtime_info.as_mut().unwrap().last_error = None;
    drop(guard);
    assert!(!consumer.await.unwrap());
    let guard = state.read().await;
    let snapshot = guard.db.with_conn(player::load_snapshot).unwrap();
    assert!(snapshot.state.is_playing);
    assert_eq!(snapshot.state.current_queue_item_id, Some(replacement_row));
    assert!(
        guard
            .audio_active
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(
        guard
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .last_error
            .is_none()
    );
}

#[tokio::test]
async fn output_fault_consumer_rejects_old_runtime_generation_and_active_deck() {
    for stale_kind in ["runtime", "generation", "active_decoder"] {
        let (state, handle, stop, live) = cpal_fault_fixture().await;
        let mut fault = cpal_fault_event(&stop, &live);
        let expected_handle = if stale_kind == "runtime" {
            let (tx, _) = std::sync::mpsc::channel();
            playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(tx)
        } else {
            handle
        };
        if stale_kind == "generation" {
            state
                .write()
                .await
                .playback_generation
                .store(1, std::sync::atomic::Ordering::Release);
        } else if stale_kind == "active_decoder" {
            if let playback_runtime::PlaybackRuntimeEvent::OutputFault {
                active_decoder_stop,
                ..
            } = &mut fault
            {
                *active_decoder_stop = Some(Arc::new(std::sync::atomic::AtomicBool::new(true)));
            }
        }
        assert!(!handle_output_fault(&state, &expected_handle, fault).await);
        let guard = state.read().await;
        assert!(
            guard
                .db
                .with_conn(player::load_snapshot)
                .unwrap()
                .state
                .is_playing
        );
        assert!(
            guard
                .audio_active
                .load(std::sync::atomic::Ordering::Acquire)
        );
    }
}

#[tokio::test]
async fn native_output_fault_listener_updates_public_state_without_exclusive_auto_retry() {
    let (state, _, stop, live) = cpal_fault_fixture().await;
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    let mut public_events = {
        let mut guard = state.write().await;
        guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
        let info = guard.playback_runtime_info.as_mut().unwrap();
        info.exclusive_engaged = true;
        info.exclusive_transport_format = Some("24-bit".into());
        guard.event_tx.subscribe()
    };
    let _ = spawn_playback_runtime_listener(state.clone(), handle.clone());
    assert!(matches!(
        command_rx.try_recv().unwrap(),
        playback_runtime::PlaybackRuntimeCommand::RequestReady
    ));
    assert!(handle.test_publish_event(cpal_fault_event(&stop, &live)));
    let (failure, released) = tokio::time::timeout(Duration::from_secs(2), async {
        let mut failure = false;
        let mut released = false;
        loop {
            match public_events.recv().await.unwrap() {
                AppEvent::PlaybackFailed { message } => {
                    assert_eq!(message, "device removed");
                    failure = true;
                }
                AppEvent::AudioExclusiveReleased { device } => {
                    assert_eq!(device, "DAC");
                    released = true;
                }
                AppEvent::PlaybackStateChanged => break (failure, released),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(failure && released);
    assert!(
        command_rx.try_recv().is_err(),
        "fault must wait for explicit Resume, not auto-retry"
    );
    let guard = state.read().await;
    assert!(
        !guard
            .db
            .with_conn(player::load_snapshot)
            .unwrap()
            .state
            .is_playing
    );
    assert!(
        !guard
            .audio_active
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(
        !guard
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .exclusive_engaged
    );
    assert!(
        guard
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .exclusive_transport_format
            .is_none()
    );
}

#[tokio::test]
async fn generic_runtime_error_listener_clears_activity_and_preserves_queue() {
    let (state, _, _, _) = cpal_fault_fixture().await;
    let before = state
        .read()
        .await
        .db
        .with_conn(player::load_snapshot)
        .unwrap();
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    let mut events = {
        let mut guard = state.write().await;
        guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
        guard.event_tx.subscribe()
    };
    let _ = spawn_playback_runtime_listener(state.clone(), handle.clone());
    assert!(matches!(
        command_rx.try_recv().unwrap(),
        playback_runtime::PlaybackRuntimeCommand::RequestReady
    ));
    assert!(
        handle.test_publish_event(playback_runtime::PlaybackRuntimeEvent::Error {
            message: "The selected output is unavailable".into(),
        })
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut failure_seen = false;
        let mut state_changed = false;
        loop {
            match events.recv().await.unwrap() {
                AppEvent::PlaybackFailed { message } => {
                    assert_eq!(message, "The selected output is unavailable");
                    failure_seen = true;
                }
                AppEvent::PlaybackStateChanged => state_changed = true,
                _ => {}
            }
            if failure_seen && state_changed {
                break;
            }
        }
    })
    .await
    .unwrap();
    let guard = state.read().await;
    let after = guard.db.with_conn(player::load_snapshot).unwrap();
    assert!(!after.state.is_playing);
    assert_eq!(
        after.state.current_queue_item_id,
        before.state.current_queue_item_id
    );
    assert_eq!(
        after.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        before.queue.iter().map(|row| row.id).collect::<Vec<_>>()
    );
    assert_eq!(
        guard
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .last_error
            .as_deref(),
        Some("The selected output is unavailable")
    );
    assert!(
        !guard
            .audio_active
            .load(std::sync::atomic::Ordering::Acquire),
        "paused output failure still reported audio activity"
    );
}

#[tokio::test]
async fn generic_runtime_error_waiting_for_lock_cannot_pause_replacement_runtime() {
    let (state, handle, _, _) = cpal_fault_fixture().await;
    let mut guard = state.write().await;
    let before = guard.db.with_conn(player::load_snapshot).unwrap();
    let task_state = state.clone();
    let consumer = tokio::spawn(async move {
        handle_runtime_error_from(&task_state, Some(&handle), "retired runtime failed").await
    });
    tokio::task::yield_now().await;
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    guard.playback_runtime.as_mut().unwrap().handle =
        playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    let mut events = guard.event_tx.subscribe();
    drop(guard);
    assert!(
        !consumer.await.unwrap(),
        "retired runtime error was applied"
    );
    let guard = state.read().await;
    let after = guard.db.with_conn(player::load_snapshot).unwrap();
    assert!(after.state.is_playing);
    assert_eq!(
        after.state.current_queue_item_id,
        before.state.current_queue_item_id
    );
    assert!(
        guard
            .audio_active
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(
        guard
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .last_error
            .is_none()
    );
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn command_error_waiting_for_lock_rejects_new_generation_and_same_job_restart() {
    for replacement in ["generation", "decoder"] {
        let (state, handle, _, _) = cpal_fault_fixture().await;
        let identity = handle.playback_source_identity();
        let guard = state.write().await;
        let task_state = state.clone();
        let task_handle = handle.clone();
        let consumer = tokio::spawn(async move {
            handle_runtime_command_error(
                &task_state,
                &task_handle,
                Some(0),
                &identity,
                "old command failed",
            )
            .await
        });
        tokio::task::yield_now().await;
        if replacement == "generation" {
            guard
                .playback_generation
                .store(1, std::sync::atomic::Ordering::Release);
        } else {
            handle.test_replace_playback_source();
        }
        let mut events = guard.event_tx.subscribe();
        drop(guard);
        assert!(
            !consumer.await.unwrap(),
            "stale {replacement} command error was applied"
        );
        let guard = state.read().await;
        assert!(
            guard
                .db
                .with_conn(player::load_snapshot)
                .unwrap()
                .state
                .is_playing
        );
        assert!(
            guard
                .audio_active
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            guard
                .playback_runtime_info
                .as_ref()
                .unwrap()
                .last_error
                .is_none()
        );
        assert!(events.try_recv().is_err());
    }
}

#[tokio::test]
async fn current_command_error_reaches_real_listener_and_preserves_queue() {
    let (state, _, _, _) = cpal_fault_fixture().await;
    let before = state
        .read()
        .await
        .db
        .with_conn(player::load_snapshot)
        .unwrap();
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    let mut events = {
        let mut guard = state.write().await;
        guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
        guard.event_tx.subscribe()
    };
    let _ = spawn_playback_runtime_listener(state.clone(), handle.clone());
    assert!(matches!(
        command_rx.try_recv().unwrap(),
        playback_runtime::PlaybackRuntimeCommand::RequestReady
    ));
    handle.test_publish_event(playback_runtime::PlaybackRuntimeEvent::CommandError {
        generation: Some(0),
        source_identity: handle.playback_source_identity(),
        message: "Resume failed: output rejected".into(),
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(events.recv().await.unwrap(), AppEvent::PlaybackStateChanged) {
                break;
            }
        }
    })
    .await
    .unwrap();
    let guard = state.read().await;
    let after = guard.db.with_conn(player::load_snapshot).unwrap();
    assert!(!after.state.is_playing);
    assert_eq!(
        after.state.current_queue_item_id,
        before.state.current_queue_item_id
    );
    assert_eq!(
        after.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        before.queue.iter().map(|row| row.id).collect::<Vec<_>>()
    );
    assert!(
        !guard
            .audio_active
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert_eq!(
        guard
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .last_error
            .as_deref(),
        Some("Resume failed: output rejected")
    );
}

#[tokio::test]
async fn retired_runtime_exclusive_notifications_cannot_change_replacement_or_retry() {
    for kind in ["engaged", "failed", "released"] {
        let (state, _, _, _) = cpal_fault_fixture().await;
        let (old_tx, old_rx) = std::sync::mpsc::channel();
        let old = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(old_tx);
        {
            let mut guard = state.write().await;
            guard.playback_runtime.as_mut().unwrap().handle = old.clone();
            guard
                .db
                .with_conn(|conn| {
                    crate::db::audio_settings::save(
                        conn,
                        &crate::db::audio_settings::AudioSettings {
                            exclusive_mode: true,
                            ..Default::default()
                        },
                    )
                    .map_err(anyhow::Error::from)
                })
                .unwrap();
        }
        let listener = spawn_playback_runtime_listener(state.clone(), old.clone());
        assert!(matches!(
            old_rx.try_recv().unwrap(),
            playback_runtime::PlaybackRuntimeCommand::RequestReady
        ));
        let mut guard = state.write().await;
        let event = match kind {
            "engaged" => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeEngaged {
                output_identity: old.test_exclusive_event_identity(),
                device_name: "retired output".into(),
                transport_format: "old format".into(),
            },
            "failed" => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeFailed {
                output_identity: old.test_exclusive_event_identity(),
                device_name: "retired output".into(),
                reason: "old failure".into(),
            },
            _ => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeReleased {
                output_identity: old.test_exclusive_event_identity(),
                device_name: "retired output".into(),
            },
        };
        assert!(old.test_publish_event(event));
        tokio::task::yield_now().await;
        let (new_tx, new_rx) = std::sync::mpsc::channel();
        let replacement = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(new_tx);
        guard.playback_runtime.as_mut().unwrap().handle = replacement;
        let expected_engaged = kind != "engaged";
        let info = guard.playback_runtime_info.as_mut().unwrap();
        info.exclusive_engaged = expected_engaged;
        info.exclusive_transport_format = expected_engaged.then(|| "current format".into());
        let mut events = guard.event_tx.subscribe();
        assert!(
            old.test_publish_event(playback_runtime::PlaybackRuntimeEvent::Exited {
                message: None
            })
        );
        drop(guard);
        tokio::time::timeout(Duration::from_secs(2), listener)
            .await
            .unwrap()
            .unwrap();
        let guard = state.read().await;
        let info = guard.playback_runtime_info.as_ref().unwrap();
        assert_eq!(
            info.exclusive_engaged, expected_engaged,
            "retired {kind} changed current engagement"
        );
        assert_eq!(
            info.exclusive_transport_format.as_deref(),
            expected_engaged.then_some("current format")
        );
        assert!(events.try_recv().is_err(), "retired {kind} reached the UI");
        assert!(
            new_rx.try_recv().is_err(),
            "retired {kind} retried on the replacement runtime"
        );
    }
}

#[tokio::test]
async fn current_runtime_exclusive_notifications_update_status_and_gate_release_retry() {
    for (playing, requested) in [(true, true), (false, true), (true, false)] {
        let (state, _, _, _) = cpal_fault_fixture().await;
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
        let mut events = {
            let mut guard = state.write().await;
            guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
            guard
                .db
                .with_conn(|conn| {
                    crate::db::audio_settings::save(
                        conn,
                        &crate::db::audio_settings::AudioSettings {
                            exclusive_mode: requested,
                            ..Default::default()
                        },
                    )?;
                    conn.execute(
                        "UPDATE playback_state SET is_playing=?1 WHERE id=1",
                        rusqlite::params![playing],
                    )?;
                    Ok(())
                })
                .unwrap();
            guard.event_tx.subscribe()
        };
        let listener = spawn_playback_runtime_listener(state.clone(), handle.clone());
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            playback_runtime::PlaybackRuntimeCommand::RequestReady
        ));
        for kind in ["engaged", "failed", "released"] {
            let event = match kind {
                "engaged" => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeEngaged {
                    output_identity: handle.test_exclusive_event_identity(),
                    device_name: "current output".into(),
                    transport_format: "24-bit PCM".into(),
                },
                "failed" => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeFailed {
                    output_identity: handle.test_exclusive_event_identity(),
                    device_name: "current output".into(),
                    reason: "format rejected".into(),
                },
                _ => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeReleased {
                    output_identity: handle.test_exclusive_event_identity(),
                    device_name: "current output".into(),
                },
            };
            assert!(handle.test_publish_event(event));
            let public = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .unwrap();
            match (kind, public) {
                (
                    "engaged",
                    AppEvent::AudioExclusiveEngaged {
                        device,
                        transport_format,
                    },
                ) => {
                    assert_eq!(device, "current output");
                    assert_eq!(transport_format, "24-bit PCM");
                }
                ("failed", AppEvent::AudioExclusiveFailed { device, reason }) => {
                    assert_eq!(device, "current output");
                    assert_eq!(reason, "format rejected");
                }
                ("released", AppEvent::AudioExclusiveReleased { device }) => {
                    assert_eq!(device, "current output")
                }
                (_, other) => panic!("wrong notification for {kind}: {other:?}"),
            }
            let guard = state.read().await;
            let info = guard.playback_runtime_info.as_ref().unwrap();
            assert_eq!(info.exclusive_engaged, kind == "engaged");
            assert_eq!(
                info.exclusive_transport_format.as_deref(),
                (kind == "engaged").then_some("24-bit PCM")
            );
            if kind == "released" && playing && requested {
                assert!(matches!(
                    command_rx.try_recv().unwrap(),
                    playback_runtime::PlaybackRuntimeCommand::DeviceSwap {
                        exclusive: true,
                        released_output_identity: Some(identity),
                        ..
                    } if Arc::ptr_eq(&identity, &handle.test_exclusive_event_identity())
                ));
            } else {
                assert!(command_rx.try_recv().is_err());
            }
        }
        listener.abort();
        assert!(listener.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test]
async fn superseded_output_notifications_cannot_change_current_status_or_retry() {
    for kind in ["engaged", "failed", "released"] {
        let (state, _, _, _) = cpal_fault_fixture().await;
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
        {
            let mut guard = state.write().await;
            guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
            guard
                .db
                .with_conn(|conn| {
                    crate::db::audio_settings::save(
                        conn,
                        &crate::db::audio_settings::AudioSettings {
                            exclusive_mode: true,
                            ..Default::default()
                        },
                    )
                    .map_err(anyhow::Error::from)
                })
                .unwrap();
        }
        let listener = spawn_playback_runtime_listener(state.clone(), handle.clone());
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            playback_runtime::PlaybackRuntimeCommand::RequestReady
        ));
        let old_identity = handle.test_exclusive_event_identity();
        let mut guard = state.write().await;
        let event = match kind {
            "engaged" => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeEngaged {
                output_identity: old_identity,
                device_name: "old output".into(),
                transport_format: "old format".into(),
            },
            "failed" => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeFailed {
                output_identity: old_identity,
                device_name: "old output".into(),
                reason: "old failure".into(),
            },
            _ => playback_runtime::PlaybackRuntimeEvent::ExclusiveModeReleased {
                output_identity: old_identity,
                device_name: "old output".into(),
            },
        };
        assert!(handle.test_publish_event(event));
        tokio::task::yield_now().await;
        handle.test_replace_exclusive_event_identity();
        let expected_engaged = kind != "engaged";
        let info = guard.playback_runtime_info.as_mut().unwrap();
        info.exclusive_engaged = expected_engaged;
        info.exclusive_transport_format = expected_engaged.then(|| "current format".into());
        let mut events = guard.event_tx.subscribe();
        // A subsequent real transport acknowledgment is the processing barrier.
        assert!(
            handle.test_publish_event(playback_runtime::PlaybackRuntimeEvent::Paused {
                track_id: Some(1)
            })
        );
        drop(guard);
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, AppEvent::PlaybackStateChanged),
            "superseded {kind} reached the UI: {event:?}"
        );
        let guard = state.read().await;
        let info = guard.playback_runtime_info.as_ref().unwrap();
        assert_eq!(info.exclusive_engaged, expected_engaged);
        assert_eq!(
            info.exclusive_transport_format.as_deref(),
            expected_engaged.then_some("current format")
        );
        assert!(events.try_recv().is_err());
        assert!(
            command_rx.try_recv().is_err(),
            "superseded release queued a retry"
        );
        drop(guard);
        listener.abort();
        assert!(listener.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exclusive_release_database_wait_does_not_block_output_retirement() {
    let (state, _, _, _) = cpal_fault_fixture().await;
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    let (db, mut events) = {
        let mut guard = state.write().await;
        guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
        guard
            .playback_runtime_info
            .as_mut()
            .unwrap()
            .exclusive_engaged = true;
        guard
            .playback_runtime_info
            .as_mut()
            .unwrap()
            .exclusive_transport_format = Some("current format".into());
        guard
            .db
            .with_conn(|conn| {
                crate::db::audio_settings::save(
                    conn,
                    &crate::db::audio_settings::AudioSettings {
                        exclusive_mode: true,
                        ..Default::default()
                    },
                )?;
                conn.execute("UPDATE playback_state SET is_playing=1 WHERE id=1", [])?;
                Ok(())
            })
            .unwrap();
        (guard.db.clone(), guard.event_tx.subscribe())
    };
    let listener = spawn_playback_runtime_listener(state.clone(), handle.clone());
    assert!(matches!(
        command_rx.try_recv().unwrap(),
        playback_runtime::PlaybackRuntimeCommand::RequestReady
    ));
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        db.with_conn(|_| {
            held_tx.send(()).unwrap();
            // A failing assertion must not leave the listener blocked at teardown.
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
            Ok(())
        })
        .unwrap()
    });
    held_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(handle.test_publish_event(
        playback_runtime::PlaybackRuntimeEvent::ExclusiveModeReleased {
            output_identity: handle.test_exclusive_event_identity(),
            device_name: "retiring output".into(),
        }
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.try_read().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The listener now holds AppState and waits for the database. Publication
    // must remain possible before that wait finishes.
    let (published_tx, published_rx) = std::sync::mpsc::channel();
    let publisher_handle = handle.clone();
    let publisher = std::thread::spawn(move || {
        publisher_handle.test_replace_exclusive_event_identity();
        published_tx.send(()).unwrap();
    });
    let published_while_database_held = published_rx.recv_timeout(Duration::from_secs(2)).is_ok();
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    publisher.join().unwrap();
    assert!(
        published_while_database_held,
        "database wait blocked output retirement"
    );
    assert!(
        handle.test_publish_event(playback_runtime::PlaybackRuntimeEvent::Paused {
            track_id: Some(1)
        })
    );
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, AppEvent::PlaybackStateChanged),
        "retired release reached the UI: {event:?}"
    );
    let guard = state.read().await;
    let info = guard.playback_runtime_info.as_ref().unwrap();
    assert!(info.exclusive_engaged);
    assert_eq!(
        info.exclusive_transport_format.as_deref(),
        Some("current format")
    );
    assert!(events.try_recv().is_err());
    assert!(
        command_rx.try_recv().is_err(),
        "retired release queued recovery"
    );
    drop(guard);
    listener.abort();
    assert!(listener.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn exclusive_release_database_error_publishes_status_without_recovery() {
    for table in ["server_config", "playback_state"] {
        let (state, _, _, _) = cpal_fault_fixture().await;
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
        let mut events = {
            let mut guard = state.write().await;
            guard.playback_runtime.as_mut().unwrap().handle = handle.clone();
            let info = guard.playback_runtime_info.as_mut().unwrap();
            info.exclusive_engaged = true;
            info.exclusive_transport_format = Some("24-bit PCM".into());
            guard
                .db
                .with_conn(|conn| {
                    crate::db::audio_settings::save(
                        conn,
                        &crate::db::audio_settings::AudioSettings {
                            exclusive_mode: true,
                            ..Default::default()
                        },
                    )?;
                    // Corrupt only this isolated fixture to exercise both reads.
                    conn.execute_batch(match table {
                        "server_config" => "DROP TABLE server_config",
                        _ => "DROP TABLE playback_state",
                    })?;
                    Ok(())
                })
                .unwrap();
            guard.event_tx.subscribe()
        };
        let listener = spawn_playback_runtime_listener(state.clone(), handle.clone());
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            playback_runtime::PlaybackRuntimeCommand::RequestReady
        ));
        assert!(handle.test_publish_event(
            playback_runtime::PlaybackRuntimeEvent::ExclusiveModeReleased {
                output_identity: handle.test_exclusive_event_identity(),
                device_name: "current output".into(),
            }
        ));
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, AppEvent::AudioExclusiveReleased { device } if device == "current output")
        );
        let guard = state.read().await;
        let info = guard.playback_runtime_info.as_ref().unwrap();
        assert!(!info.exclusive_engaged);
        assert!(info.exclusive_transport_format.is_none());
        assert!(
            command_rx.try_recv().is_err(),
            "failed {table} read queued recovery"
        );
        assert!(events.try_recv().is_err());
        drop(guard);
        listener.abort();
        assert!(listener.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test]
async fn runtime_exit_discards_dead_handle_and_pauses_without_changing_queue() {
    let db = fresh_migrated_db();
    seed_basic_tracks(&db);
    let queue_item_id = db
        .with_conn(|conn| {
            let queue = player::enqueue_track(conn, 1, "test")?;
            let queue_item_id = queue[0].id;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![queue_item_id],
            )?;
            Ok(queue_item_id)
        })
        .unwrap();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    {
        let mut guard = state.write().await;
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: handle.clone(),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_runtime_exit(&state, &handle, Some("test runtime stopped")).await;

    {
        let guard = state.read().await;
        assert!(guard.playback_runtime.is_none());
        assert!(guard.playback_runtime_info.is_none());
    }
    let snapshot = db.with_conn(player::load_snapshot).unwrap();
    assert!(!snapshot.state.is_playing);
    assert_eq!(snapshot.state.current_queue_item_id, Some(queue_item_id));
    assert_eq!(snapshot.queue.len(), 1);
}

#[test]
fn tidal_playlist_tracks_cache_returns_fresh_entries_and_expires_stale_entries() {
    let cache = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let key = tidal_playlist_tracks_cache_key("AU", "playlist-uuid", 100, 0);
    let tracks: Vec<TidalTrack> = Vec::new();

    put_cached_tidal_playlist_tracks(&cache, key.clone(), tracks.clone());

    assert!(get_cached_tidal_playlist_tracks(&cache, &key).is_some());

    {
        let mut guard = cache.lock().unwrap();
        guard.insert(
            key.clone(),
            (
                Instant::now() - TIDAL_PLAYLIST_TRACKS_CACHE_TTL - Duration::from_secs(1),
                tracks,
            ),
        );
    }

    assert!(get_cached_tidal_playlist_tracks(&cache, &key).is_none());
    assert!(!cache.lock().unwrap().contains_key(&key));
}

#[test]
fn catalogue_unlike_targets_all_aliases_and_rejects_stale_snapshots() {
    let db = fresh_migrated_db();
    db.with_conn(|conn| {
        use crate::db::{catalogue, catalogue_favorites as intent};
        let mut track = test_tidal_track(10, "Song");
        track.isrc = Some("ISRC1".into());
        let local =
            insert_tidal_track(conn, &track, true, true, Some("2020-01-01T00:00:00Z"))?.unwrap();
        track.id = 20;
        insert_tidal_track(conn, &track, true, true, Some("2020-01-01T00:00:00Z"))?;
        let stale = intent::now();
        intent::request(conn, "track", local, false)?;
        let operations = intent::pending(conn)?;
        assert_eq!(
            operations.iter().map(|op| op.tidal_id).collect::<Vec<_>>(),
            vec![10, 20]
        );
        catalogue::reconcile_favorites_at(conn, &HashSet::from([10, 20]), false, &stale)?;
        assert!(
            !conn.query_row("SELECT is_favorite FROM tracks WHERE id=?1", [local], |r| r
                .get::<_, bool>(0))?
        );
        for op in &operations {
            intent::finish(conn, op, None)?;
        }
        catalogue::reconcile_favorites_at(conn, &HashSet::new(), false, &intent::now())?;
        catalogue::reconcile_favorites_at(conn, &HashSet::from([10]), false, &stale)?;
        assert!(
            !conn.query_row("SELECT is_favorite FROM tracks WHERE id=?1", [local], |r| r
                .get::<_, bool>(0))?
        );
        catalogue::reconcile_favorites_at(conn, &HashSet::from([10]), false, &intent::now())?;
        assert!(
            conn.query_row("SELECT is_favorite FROM tracks WHERE id=?1", [local], |r| r
                .get::<_, bool>(0))?
        );
        catalogue::reconcile_favorites_at(conn, &HashSet::new(), false, &stale)?;
        assert!(
            conn.query_row("SELECT is_favorite FROM tracks WHERE id=?1", [local], |r| r
                .get::<_, bool>(0))?
        );
        let alias_state: i64 = conn.query_row(
            "SELECT is_favorite FROM tidal_track_aliases WHERE tidal_id=10",
            [],
            |r| r.get(0),
        )?;
        crate::db::catalogue::reconcile_favorites_at(
            conn,
            &HashSet::new(),
            false,
            "1900-01-01T00:00:00Z",
        )?;
        assert_eq!(
            conn.query_row(
                "SELECT is_favorite FROM tidal_track_aliases WHERE tidal_id=10",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            alias_state
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn catalogue_shared_recording_identity_examples() {
    use crate::library::duplicates::{
        ExistingCandidate, ImportDecision, IncomingTrack, decide_import,
    };
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../scripts/fixtures/tidal-recording-identity.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let left = &case["before"];
        let right = &case["after"];
        let incoming = IncomingTrack {
            tidal_id: 10,
            title: left["title"].as_str().unwrap(),
            artist_name: "Artist",
            isrc: Some("ISRC1"),
            duration_ms: 180000,
            version: left["catalogue_version"].as_str(),
            explicit: left["catalogue_explicit"].as_bool(),
        };
        let candidate = ExistingCandidate {
            track_id: 1,
            tidal_id: Some(20),
            title: right["title"].as_str().unwrap().into(),
            artist_name: "Artist".into(),
            isrc: Some("ISRC1".into()),
            duration_ms: 180000,
            version: right["catalogue_version"].as_str().map(str::to_owned),
            explicit: right["catalogue_explicit"].as_bool(),
        };
        assert_eq!(
            matches!(
                decide_import(&incoming, &[candidate]),
                ImportDecision::LinkAlias { .. }
            ),
            case["equivalent"].as_bool().unwrap()
        );
    }
}

#[test]
fn runtime_output_settings_preserve_persisted_exclusive_preferences() {
    let settings = crate::db::audio_settings::AudioSettings {
        output_device: Some("Zen DAC V2".to_string()),
        exclusive_mode: true,
        sample_rate_follow: true,
        exclusive_release_grace_secs: 12,
        exclusive_latency_mode: crate::db::audio_settings::ExclusiveLatencyMode::LowLatency,
        ..Default::default()
    };

    let output = runtime_output_settings_from_audio_settings(&settings);

    match output.device {
        playback_runtime::OutputDeviceSelection::Named(name) => {
            assert_eq!(name, "Zen DAC V2");
        }
        playback_runtime::OutputDeviceSelection::Default => {
            panic!("expected named output device")
        }
    }
    assert!(output.exclusive_mode);
    assert!(output.sample_rate_follow);
    assert_eq!(output.exclusive_release_grace_secs, 12);
    assert_eq!(
        output.exclusive_latency_mode,
        crate::db::audio_settings::ExclusiveLatencyMode::LowLatency
    );
}

#[test]
fn extracts_genre_candidates_from_mixed_metadata_shapes() {
    let mut extra = HashMap::new();
    extra.insert("genre".to_string(), json!("trip hop"));
    extra.insert(
        "subGenres".to_string(),
        json!([
            "shoegazee",
            { "name": "Tech House / House" },
            { "title": "Progressive House" }
        ]),
    );

    let genres =
        crate::genre::builder::collect_clear_genres(extract_genre_candidates_from_extra(&extra));

    assert_eq!(
        genres,
        vec![
            "Progressive House".to_string(),
            "Shoegaze".to_string(),
            "Trip-Hop".to_string()
        ]
    );
}

#[tokio::test]
async fn runtime_finish_skips_unresolved_pending_row_and_starts_next_library_track() {
    let db = fresh_migrated_db();
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (8200, 'Queued Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES
                    (8201, 'Finished Track', 8200, 180000, 88201, 'tidal', 'tidal'),
                    (8202, 'Next Library Track', 8200, 180000, 88202, 'tidal', 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8201, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 1, 'radio_pending', 'Missing Artist', 'Missing Title', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8202, 2, 'test')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 8201, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();
    assert!(current_qid > 0);

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 8202);
                assert_eq!(generation, 1);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard
            .tidal
            .set_tokens_for_test(Some(tidal_auth::TidalTokens {
                access_token: "test-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: 3600,
                user_id: "test-user".to_string(),
                country_code: "US".to_string(),
                auth_flow: Some("pkce".to_string()),
            }));
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(8201),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_runtime_finished(state.clone(), 8201, 1)
        .await
        .expect("runtime finish");

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        8202
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(8202));
    assert_eq!(current_queue_item_id, Some(next_qid));
    assert!(is_playing);
}

#[tokio::test]
async fn runtime_finish_adopts_pending_row_resolved_by_background_resolver() {
    let db = fresh_migrated_db();
    let (current_qid, pending_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (8300, 'Queued Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES
                    (8301, 'Finished Track', 8300, 180000, 88301, 'tidal', 'tidal'),
                    (8302, 'Unrelated Next Track', 8300, 180000, 88302, 'tidal', 'tidal'),
                    (8303, 'Background Resolved Track', 8300, 180000, 88303, 'tidal', 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8301, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at, resolving_at
                 ) VALUES (
                    NULL, 1, 'radio_pending', 'Resolved Artist', 'Resolved Title',
                    datetime('now'), datetime('now')
                 )",
                [],
            )?;
            let pending_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8302, 2, 'test')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 8301, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, pending_qid))
        })
        .unwrap();
    assert!(current_qid > 0);

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 8303);
                assert_eq!(generation, 1);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard
            .tidal
            .set_tokens_for_test(Some(tidal_auth::TidalTokens {
                access_token: "test-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: 3600,
                user_id: "test-user".to_string(),
                country_code: "US".to_string(),
                auth_flow: Some("pkce".to_string()),
            }));
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(8301),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    let db_for_resolve = db.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(
            PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS / 2,
        ))
        .await;
        db_for_resolve
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE queue
                     SET track_id = 8303,
                         resolving_at = NULL,
                         resolved_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![pending_qid],
                )?;
                Ok(())
            })
            .expect("promote pending row");
    });

    handle_runtime_finished(state.clone(), 8301, 1)
        .await
        .expect("runtime finish");

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        8303
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(8303));
    assert_eq!(current_queue_item_id, Some(pending_qid));
}

#[tokio::test]
async fn manual_previous_skips_unresolved_pending_rows_to_prior_library_track() {
    let db = fresh_migrated_db();
    let (previous_qid, current_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (8400, 'Previous Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES
                    (8401, 'Previous Library Track', 8400, 180000, 88401, 'tidal', 'tidal'),
                    (8402, 'Current Library Track', 8400, 180000, 88402, 'tidal', 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8401, 0, 'test')",
                [],
            )?;
            let previous_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 1, 'radio_pending', 'Missing Artist A', 'Missing Title A', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 2, 'radio_pending', 'Missing Artist B', 'Missing Title B', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8402, 3, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 8402, current_queue_item_id = ?1, is_playing = 1, position_ms = 0
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((previous_qid, current_qid))
        })
        .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let playback_generation = crate::server::transport::generation::bump(&state).await;
    let saved_anchor = save_playback_anchor(&state)
        .await
        .expect("saved playback anchor");
    let snapshot = previous_persisted_playback_snapshot(&state)
        .await
        .expect("initial previous snapshot");
    assert!(snapshot.state.current_track.is_none());
    assert_ne!(snapshot.state.current_queue_item_id, Some(current_qid));

    let snapshot = resolve_or_skip_pending_current_previous(
        &state,
        snapshot,
        playback_generation,
        "manual_previous_track",
        saved_anchor,
    )
    .await
    .expect("previous pending skip");

    assert_eq!(
        snapshot.state.current_track.as_ref().map(|track| track.id),
        Some(8401)
    );
    assert_eq!(snapshot.state.current_queue_item_id, Some(previous_qid));
    assert!(snapshot.state.is_playing);
}

#[tokio::test]
async fn manual_previous_restores_anchor_when_pending_rows_cannot_move_back() {
    let db = fresh_migrated_db();
    let current_qid = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 0, 'radio_pending', 'Missing Artist A', 'Missing Title A', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 1, 'radio_pending', 'Missing Artist B', 'Missing Title B', datetime('now'))",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = NULL, current_queue_item_id = ?1, is_playing = 1, position_ms = 0
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok(current_qid)
        })
        .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let playback_generation = crate::server::transport::generation::bump(&state).await;
    let saved_anchor = save_playback_anchor(&state)
        .await
        .expect("saved playback anchor");
    let snapshot = previous_persisted_playback_snapshot(&state)
        .await
        .expect("initial previous snapshot");
    assert!(snapshot.state.current_track.is_none());
    assert_ne!(snapshot.state.current_queue_item_id, Some(current_qid));

    let snapshot = resolve_or_skip_pending_current_previous(
        &state,
        snapshot,
        playback_generation,
        "manual_previous_track",
        saved_anchor,
    )
    .await
    .expect("previous pending restore");

    // Pressing previous must never stop the session: when nothing behind the
    // current row is playable, the anchor rolls back to where it was and the
    // current track keeps playing.
    assert!(snapshot.state.current_track.is_none());
    assert_eq!(snapshot.state.current_queue_item_id, Some(current_qid));
    assert!(snapshot.state.is_playing);
}

#[tokio::test]
async fn prepared_runtime_track_error_keeps_current_playback_running() {
    let db = fresh_migrated_db();
    seed_basic_tracks(&db);
    let current_qid: i64 = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'test')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok(current_qid)
        })
        .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    {
        let mut guard = state.write().await;
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    let generation = current_playback_generation(&*state.read().await);
    handle_prepared_runtime_track_error_for_runtime(
        &state,
        None,
        2,
        generation,
        None,
        "prebuffer decode failed",
    )
    .await;

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(1));
    assert_eq!(current_queue_item_id, Some(current_qid));
    assert!(is_playing);

    let guard = state.read().await;
    let info = guard.playback_runtime_info.as_ref().expect("runtime info");
    assert_eq!(info.active_track_id, Some(1));
    assert_eq!(info.last_error.as_deref(), Some("prebuffer decode failed"));
}

#[tokio::test]
async fn unavailable_prepared_next_is_skipped_without_interrupting_current_track() {
    let db = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, failed_qid, duplicate_qid) = db.with_conn(|conn| {
        conn.execute("UPDATE tracks SET tidal_id = 864212 WHERE id = 2", [])?;
        conn.execute("INSERT INTO tracks (id, title, artist_id, duration_ms, source) VALUES (3, 'Playable next', 1, 180000, 'tidal_stream')", [])?;
        conn.execute("INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')", [])?;
        let current_qid = conn.last_insert_rowid();
        conn.execute("INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'test')", [])?;
        let failed_qid = conn.last_insert_rowid();
        conn.execute("INSERT INTO queue (track_id, position, source) VALUES (3, 2, 'test')", [])?;
        conn.execute("INSERT INTO queue (track_id, position, source) VALUES (2, 3, 'test')", [])?;
        let duplicate_qid = conn.last_insert_rowid();
        conn.execute("UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1, position_ms = 42000, is_playing = 1 WHERE id = 1", [current_qid])?;
        Ok((current_qid, failed_qid, duplicate_qid))
    }).expect("queue");
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    {
        let mut guard = state.write().await;
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }
    let mut events = state.read().await.event_tx.subscribe();
    let generation = current_playback_generation(&*state.read().await);
    handle_prepared_runtime_track_error_for_runtime(&state, None, 2, generation, Some(864212),
        r#"TIDAL rejected playback request with 401 Unauthorized: {"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#).await;
    let snapshot = db.with_conn(player::load_snapshot).expect("snapshot");
    assert_eq!(
        snapshot.state.current_track.as_ref().map(|track| track.id),
        Some(1)
    );
    assert_eq!(snapshot.state.current_queue_item_id, Some(current_qid));
    assert_eq!(snapshot.state.position_ms, 42_000);
    assert!(snapshot.state.is_playing);
    assert!(
        !snapshot.queue.iter().any(|row| row.id == failed_qid),
        "proven unavailable next row must be skipped immediately"
    );
    assert!(
        snapshot.queue.iter().any(|row| row.id == duplicate_qid),
        "a skip removes the failed row, not every occurrence of the track"
    );
    assert_eq!(snapshot.queue[1].track.id, 3);
    assert_eq!(
        current_playback_generation(&*state.read().await),
        generation
    );
    let mut skipped = false;
    while let Ok(event) = events.try_recv() {
        skipped |= matches!(event, AppEvent::TrackSkipped { track_id: 2, .. });
    }
    assert!(
        skipped,
        "notify the listener when an upcoming track is skipped"
    );
}

#[tokio::test]
async fn stale_runtime_prepared_error_does_not_change_active_queue() {
    let db = fresh_migrated_db();
    seed_basic_tracks(&db);
    db.with_conn(|conn| {
        conn.execute("UPDATE tracks SET tidal_id = 864218 WHERE id = 2", [])?;
        conn.execute("INSERT INTO queue (id, track_id, position, source) VALUES (10, 1, 0, 'test'), (11, 2, 1, 'test')", [])?;
        conn.execute("UPDATE playback_state SET current_track_id = 1, current_queue_item_id = 10, is_playing = 1 WHERE id = 1", [])?;
        Ok(())
    }).expect("queue");
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (old_tx, _old_rx) = std::sync::mpsc::channel();
    let old_handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(old_tx);
    let (new_tx, _new_rx) = std::sync::mpsc::channel();
    {
        let mut guard = state.write().await;
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "new".into(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(new_tx),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }
    let generation = current_playback_generation(&*state.read().await);
    handle_prepared_runtime_track_error_for_runtime(&state, Some(&old_handle), 2, generation, Some(864218),
        r#"TIDAL rejected playback request with 401 Unauthorized: {"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#).await;
    let snapshot = db.with_conn(player::load_snapshot).expect("snapshot");
    assert_eq!(snapshot.queue.len(), 2);
    assert_eq!(snapshot.state.current_queue_item_id, Some(10));
    assert!(
        state
            .read()
            .await
            .playback_runtime_info
            .as_ref()
            .expect("runtime")
            .last_error
            .is_none()
    );
}

#[tokio::test]
async fn delayed_prepared_asset_failure_does_not_remove_new_generation_or_healed_source() {
    for changed in ["generation", "source"] {
        let db = fresh_migrated_db();
        seed_basic_tracks(&db);
        db.with_conn(|conn| {
            conn.execute("UPDATE tracks SET tidal_id = 864219 WHERE id = 2", [])?;
            conn.execute("INSERT INTO queue (id, track_id, position, source) VALUES (10, 1, 0, 'test'), (11, 2, 1, 'test')", [])?;
            conn.execute("UPDATE playback_state SET current_track_id = 1, current_queue_item_id = 10, position_ms = 42000, is_playing = 1 WHERE id = 1", [])?;
            Ok(())
        }).expect("queue");
        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
        let (command_tx, _command_rx) = std::sync::mpsc::channel();
        let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
        let generation = {
            let mut guard = state.write().await;
            guard.playback_runtime = Some(PlaybackRuntimeState {
                access_token: "same-runtime".into(),
                handle: handle.clone(),
            });
            guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
                device_name: "Test DAC".to_string(),
                sample_rate: 48_000,
                channels: 2,
                active_track_id: Some(1),
                last_error: None,
                exclusive_engaged: false,
                exclusive_transport_format: None,
            });
            current_playback_generation(&guard)
        };
        if changed == "generation" {
            state
                .write()
                .await
                .playback_generation
                .store(generation + 1, std::sync::atomic::Ordering::Relaxed);
        } else {
            db.with_conn(|conn| {
                conn.execute("UPDATE tracks SET tidal_id = 864220 WHERE id = 2", [])?;
                Ok(())
            })
            .expect("catalog heal before old event arrives");
        }
        handle_prepared_runtime_track_error_for_runtime(&state, Some(&handle), 2, generation, Some(864219),
            r#"TIDAL rejected playback request with 401 Unauthorized: {"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#).await;
        let snapshot = db.with_conn(player::load_snapshot).expect("snapshot");
        assert_eq!(snapshot.queue.len(), 2, "{changed}");
        assert_eq!(snapshot.state.current_queue_item_id, Some(10), "{changed}");
        assert_eq!(snapshot.state.position_ms, 42_000, "{changed}");
        assert!(snapshot.state.is_playing, "{changed}");
        assert!(
            state
                .read()
                .await
                .playback_runtime_info
                .as_ref()
                .expect("runtime")
                .last_error
                .is_none(),
            "{changed}"
        );
    }
}

#[tokio::test]
async fn pause_and_resume_snapshots_preserve_runtime_position_after_seek() {
    let db = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])?;
        conn.execute("INSERT INTO tracks (id, title, artist_id, source, duration_ms) VALUES (1, 'Track', 1, 'tidal', 180000)", [])?;
        conn.execute("INSERT INTO queue (id, track_id, position, source) VALUES (1, 1, 0, 'manual')", [])?;
        conn.execute("UPDATE playback_state SET current_track_id = 1, current_queue_item_id = 1, position_ms = 0, is_playing = 1 WHERE id = 1", [])?;
        Ok(())
    }).unwrap();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    {
        let mut guard = state.write().await;
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: handle.clone(),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
        guard
            .audio_active
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    for target_ms in [10_000, 3_000, 20_000] {
        handle.test_publish_position(target_ms as u64 * 96);
        let paused = pause_playback(State(state.clone())).await.unwrap().0;
        assert_eq!(paused["state"]["position_ms"], target_ms);
        assert_eq!(paused["state"]["is_playing"], false);
        let resumed = resume_playback(State(state.clone())).await.unwrap().0;
        assert_eq!(resumed["state"]["position_ms"], target_ms);
        assert_eq!(resumed["state"]["is_playing"], true);
    }
}

#[tokio::test]
async fn runtime_listener_receives_ready_before_its_task_first_runs() {
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        fresh_migrated_db(),
    )));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    state.write().await.playback_runtime = Some(PlaybackRuntimeState {
        access_token: "test-token".to_string(),
        handle: handle.clone(),
    });
    let _ = spawn_playback_runtime_listener(state.clone(), handle.clone());
    assert!(matches!(
        command_rx.try_recv(),
        Ok(playback_runtime::PlaybackRuntimeCommand::RequestReady)
    ));
    // No yield: the OS runtime can emit Ready before Tokio polls the listener.
    assert!(
        handle.test_publish_event(playback_runtime::PlaybackRuntimeEvent::Ready {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
        })
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if state.read().await.playback_runtime_info.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        state
            .read()
            .await
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .sample_rate,
        48_000
    );
}

#[tokio::test]
async fn early_pair_preparation_uses_playback_generation_before_near_end() {
    let db = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id,name) VALUES (1,'Artist')",[])?;
        conn.execute("INSERT INTO tracks (id,title,artist_id,duration_ms) VALUES (1,'Outgoing',1,180000),(2,'Incoming',1,180000)",[])?;
        conn.execute("INSERT INTO queue (id,track_id,position,source) VALUES (11,1,0,'manual'),(12,2,1,'manual')",[])?;
        conn.execute("UPDATE playback_state SET current_track_id=1,current_queue_item_id=11,position_ms=2000,is_playing=1 WHERE id=1",[])?;
        queries::set_dj_engine_enabled(conn,true)
    }).unwrap();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    let lookahead = {
        let mut guard = state.write().await;
        guard
            .playback_generation
            .store(71, std::sync::atomic::Ordering::Relaxed);
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test".into(),
            handle: handle.clone(),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".into(),
            sample_rate: 48000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
        active_dj_lookahead_start_for_state(&guard).unwrap()
    };
    assert_ne!(
        lookahead.queue_generation, 71,
        "queue identity is distinct from playback generation"
    );
    let observed = tokio::task::spawn_blocking(move || {
        let command = command_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("early preparation must reach runtime");
        match command {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .unwrap();
                (track_id, generation)
            }
            other => panic!("unexpected preparation command {other:?}"),
        }
    });
    spawn_dj_pair_preparation(state.clone(), handle, lookahead, 71);
    assert_eq!(observed.await.unwrap(), (2, 71));
    state.write().await.playback_runtime = None;
}

#[tokio::test]
async fn runtime_ready_after_device_swap_keeps_audible_track_active() {
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        fresh_migrated_db(),
    )));
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    {
        let mut guard = state.write().await;
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: handle.clone(),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Old DAC".to_string(),
            sample_rate: 44_100,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: true,
            exclusive_transport_format: Some("24-bit".to_string()),
        });
        guard
            .audio_active
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    assert!(apply_runtime_ready(&state, &handle, "New DAC".to_string(), 48_000, 2).await);
    let guard = state.read().await;
    assert!(
        guard
            .audio_active
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    let info = guard.playback_runtime_info.as_ref().unwrap();
    assert_eq!(info.active_track_id, Some(1));
    assert_eq!(info.device_name, "New DAC");
    assert_eq!(info.sample_rate, 48_000);
    assert!(info.exclusive_engaged);
}

#[tokio::test]
async fn runtime_ready_without_active_track_clears_audible_flag() {
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        fresh_migrated_db(),
    )));
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx);
    {
        let mut guard = state.write().await;
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: handle.clone(),
        });
        guard
            .audio_active
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    assert!(apply_runtime_ready(&state, &handle, "Test DAC".to_string(), 48_000, 2).await);
    let guard = state.read().await;
    assert!(
        !guard
            .audio_active
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert_eq!(
        guard
            .playback_runtime_info
            .as_ref()
            .unwrap()
            .active_track_id,
        None
    );
}

#[test]
fn runtime_track_error_retry_policy_only_retries_transient_failures() {
    for message in [
        "DASH stream prebuffer failed",
        "TIDAL stream download failed: segment timed out",
        "TIDAL playback request was rejected with 429 Too Many Requests",
    ] {
        assert!(runtime_track_error_is_retryable(message), "{message}");
    }
    assert!(!runtime_track_error_is_retryable(
        "unsupported codec in playback asset"
    ));
}

#[tokio::test]
async fn repeated_transient_runtime_error_pauses_without_advancing_queue() {
    let db = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'test')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    {
        let mut guard = state.write().await;
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: Some(RUNTIME_TRACK_RETRY_MARKER.to_string()),
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_runtime_track_error(state, 1, 1, "DASH stream prebuffer failed")
        .await
        .expect("second transient failure should pause");

    let snapshot = db.with_conn(player::load_snapshot).unwrap();
    assert_eq!(
        snapshot.state.current_track.as_ref().map(|track| track.id),
        Some(1)
    );
    assert_eq!(snapshot.state.current_queue_item_id, Some(current_qid));
    assert!(!snapshot.state.is_playing);
    assert_eq!(
        snapshot
            .queue
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![current_qid, next_qid]
    );
}

#[tokio::test]
async fn runtime_track_error_advances_to_next_library_track() {
    let db = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'test')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();
    assert!(current_qid > 0);

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 2);
                assert_eq!(generation, 1);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                assert_eq!(job.generation, 1);
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard
            .tidal
            .set_tokens_for_test(Some(tidal_auth::TidalTokens {
                access_token: "test-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: 3600,
                user_id: "test-user".to_string(),
                country_code: "US".to_string(),
                auth_flow: Some("pkce".to_string()),
            }));
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_runtime_track_error(state.clone(), 1, 1, "active decode failed")
        .await
        .expect("track error should advance");

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        2
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(2));
    assert_eq!(current_queue_item_id, Some(next_qid));
    assert!(is_playing);
}

#[test]
fn next_prebuffer_slot_suppresses_duplicate_pair_only() {
    let key = crate::NextPrebufferKey {
        current_track_id: 1,
        next_track_id: 2,
        generation: 3,
    };
    let replacement = crate::NextPrebufferKey {
        current_track_id: 1,
        next_track_id: 4,
        generation: 3,
    };
    let mut slot = None;

    assert!(claim_next_prebuffer_slot(&mut slot, key));
    assert!(!claim_next_prebuffer_slot(&mut slot, key));
    assert!(claim_next_prebuffer_slot(&mut slot, replacement));
    release_next_prebuffer_slot(&mut slot, key);
    assert_eq!(slot, Some(replacement));
    release_next_prebuffer_slot(&mut slot, replacement);
    assert_eq!(slot, None);
}

#[test]
fn exclusive_sample_rate_follow_skips_prebuffer_on_rate_change() {
    assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        44_100,
        Some(96_000),
        Some(16),
        Some(24),
    ));
    assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        96_000,
        Some(96_000),
        Some(24),
        Some(24),
    ));
    assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        false,
        44_100,
        Some(96_000),
        Some(16),
        Some(24),
    ));
    assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        44_100,
        None,
        Some(16),
        Some(16),
    ));
    assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        44_100,
        Some(44_100),
        Some(16),
        Some(24),
    ));
}

#[test]
fn shared_sample_rate_follow_skips_prebuffer_on_rate_change() {
    assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
        false,
        true,
        44_100,
        Some(96_000),
        Some(16),
        Some(24),
    ));
}

#[tokio::test]
async fn promote_pending_row_emit_broadcasts_queue_updated() {
    let db = Database::open_in_memory().expect("db opened");
    db.run_migrations().expect("migrations");
    db.with_conn(schema::run_migrations)
        .expect("schema migrations");

    // Seed an artist + a real track to be the promotion target, plus a
    // pending queue row pointing at "Pending Artist / Pending Title".
    db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Promoted Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, source, fidelity_score
                 ) VALUES (1, 'Promoted Title', 1, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
                 VALUES (NULL, 0, 'radio_pending', 'Pending Artist', 'Pending Title', datetime('now'))",
                [],
            )?;
            Ok(())
        })
        .expect("seed");

    let queue_item_id: i64 = db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT id FROM queue WHERE track_id IS NULL", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();

    let (event_tx, mut rx) = tokio::sync::broadcast::channel(8);
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 950);
    assert!(promoted, "promotion must succeed for a NULL-track row");

    let evt = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("event arrived in time")
        .expect("event channel open");
    assert!(matches!(evt, AppEvent::QueueUpdated));

    // Confirm DB: the row is no longer pending.
    let resolved_track_id: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT track_id FROM queue WHERE id = ?1",
                rusqlite::params![queue_item_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(resolved_track_id, Some(1));

    // Idempotency: a second promotion attempt is a no-op (track_id already set)
    // and must NOT broadcast a second event.
    let again = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 950);
    assert!(!again);
    let no_more = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
    assert!(
        no_more.is_err(),
        "no second event should fire on idempotent retry"
    );
}

#[tokio::test]
async fn promote_pending_row_emit_marks_external_candidate_resolved() {
    let db = Database::open_in_memory().expect("db opened");
    db.run_migrations().expect("migrations");
    db.with_conn(schema::run_migrations)
        .expect("schema migrations");

    db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Resolved Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, tidal_id, source, fidelity_score
                 ) VALUES (1, 'Resolved Title', 1, 4242, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO external_track_candidates (
                    tidal_id, dedupe_key, title, artist_name, expires_at
                 ) VALUES (4242, 'tidal:4242', 'Resolved Title', 'Resolved Artist', '2026-03-01 00:00:00')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at, tidal_id_hint
                 ) VALUES (NULL, 0, 'automix-new', 'Resolved Artist', 'Resolved Title', datetime('now'), 4242)",
                [],
            )?;
            Ok(())
        })
        .expect("seed");

    let queue_item_id: i64 = db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT id FROM queue WHERE track_id IS NULL", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();

    let (event_tx, _rx) = tokio::sync::broadcast::channel(8);
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 990);
    assert!(promoted);

    let resolved: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT resolved_track_id FROM external_track_candidates WHERE tidal_id = 4242",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(resolved, Some(1));
}

/// A two-row TIDAL queue playing row 1, with a fake runtime and a scripted
/// stream source that answers once with a playable stream.
fn command_fixture() -> (
    crate::SharedState,
    std::sync::mpsc::Receiver<playback_runtime::PlaybackRuntimeCommand>,
) {
    let db = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id, name) VALUES (9300, 'Command Artist')", [])?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
             VALUES (9301, 'One', 9300, 180000, 99301, 'tidal', 'tidal'),
                    (9302, 'Two', 9300, 180000, 99302, 'tidal', 'tidal')",
            [],
        )?;
        conn.execute("INSERT INTO queue (track_id, position, source) VALUES (9301, 0, 'test')", [])?;
        let first = conn.last_insert_rowid();
        conn.execute("INSERT INTO queue (track_id, position, source) VALUES (9302, 1, 'test')", [])?;
        conn.execute(
            "UPDATE playback_state SET current_track_id = 9301, current_queue_item_id = ?1, is_playing = 1 WHERE id = 1",
            rusqlite::params![first],
        )?;
        Ok(())
    })
    .unwrap();
    let mut app = fresh_test_state(db);
    app.stream_source =
        super::stream::ScriptedStreamSource::new(vec![Ok(super::stream::test_stream_info(99302))]);
    app.tidal.set_tokens_for_test(Some(tidal_auth::TidalTokens {
        access_token: "test-token".to_string(),
        refresh_token: "refresh-token".to_string(),
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        user_id: "test-user".to_string(),
        country_code: "US".to_string(),
        auth_flow: Some("pkce".to_string()),
    }));
    let (command_tx, commands) = std::sync::mpsc::channel();
    app.playback_runtime = Some(PlaybackRuntimeState {
        access_token: "test-token".to_string(),
        handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
    });
    (Arc::new(tokio::sync::RwLock::new(app)), commands)
}

#[tokio::test]
async fn next_command_advances_and_switches_the_runtime() {
    let (state, commands) = command_fixture();
    let outcome = super::command::next(&state).await.unwrap();
    match outcome {
        super::command::Outcome::Settled(snapshot) => {
            assert_eq!(snapshot.state.current_track.map(|t| t.id), Some(9302));
        }
        other => panic!("expected Settled, got {other:?}"),
    }
    let mut saw_switch = false;
    while let Ok(command) = commands.try_recv() {
        if let playback_runtime::PlaybackRuntimeCommand::Switch(job) = command {
            assert_eq!(job.track.id, 9302);
            saw_switch = true;
        }
    }
    assert!(saw_switch, "runtime was switched to the next row");
}

#[tokio::test]
async fn play_command_rejects_non_library_ids_without_touching_playback() {
    let (state, commands) = command_fixture();
    let err = super::command::play(&state, 0).await.unwrap_err();
    assert!(matches!(
        err,
        super::command::CommandError::InvalidTrackId(0)
    ));
    assert!(commands.try_recv().is_err());
}
