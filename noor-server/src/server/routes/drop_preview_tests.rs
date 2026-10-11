use super::*;
#[allow(unused_imports)]
use crate::server::transport::{events::*, listen::*, pending::*, settings::*, snapshot::*};

fn preview_fixture() -> (
    crate::AppState,
    playback_runtime::PlaybackRuntimeHandle,
    player::DjLookaheadStart,
) {
    let db = tests::fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id,name) VALUES (1,'Artist')", [])?;
        conn.execute("INSERT INTO tracks (id,title,artist_id,duration_ms) VALUES (1,'Outgoing',1,240000),(2,'Incoming',1,240000),(3,'Replacement',1,240000)", [])?;
        conn.execute("INSERT INTO queue (id,track_id,position,source) VALUES (11,1,0,'manual'),(12,2,1,'manual')", [])?;
        conn.execute("UPDATE playback_state SET current_track_id=1,current_queue_item_id=11,is_playing=1 WHERE id=1", [])?;
        queries::set_dj_engine_enabled(conn, true)
    }).unwrap();
    let mut state = tests::fresh_test_state(db.clone());
    state
        .playback_generation
        .store(71, std::sync::atomic::Ordering::Relaxed);
    let (tx, _rx) = std::sync::mpsc::channel();
    let handle = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(tx);
    state.playback_runtime = Some(crate::PlaybackRuntimeState {
        access_token: "test".into(),
        handle: handle.clone(),
    });
    state.playback_runtime_info = Some(crate::PlaybackRuntimeInfo {
        device_name: "Test DAC".into(),
        sample_rate: 48000,
        channels: 2,
        active_track_id: Some(1),
        last_error: None,
        exclusive_engaged: false,
        exclusive_transport_format: None,
    });
    let pair = active_dj_lookahead_start_for_state(&state).unwrap();
    (state, handle, pair)
}

#[test]
fn preview_accepts_distinct_runtime_generation_and_queue_identity() {
    let (state, handle, pair) = preview_fixture();
    assert_ne!(pair.queue_generation, 71);
    assert!(
        drop_preview_pair_is_current(&state, &handle, &pair, 71),
        "a valid preview must not compare a queue hash with the runtime generation"
    );

    // A seek/switch or replaced next row invalidates the asynchronous preview.
    assert!(!drop_preview_pair_is_current(&state, &handle, &pair, 70));
    let (other_tx, _other_rx) = std::sync::mpsc::channel();
    let other = playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(other_tx);
    assert!(!drop_preview_pair_is_current(&state, &other, &pair, 71));
    state
        .db
        .with_conn(|conn| {
            conn.execute("UPDATE tracks SET tidal_id=12345 WHERE id=2", [])?;
            Ok(())
        })
        .unwrap();
    let healed = active_dj_lookahead_start_for_state(&state).unwrap();
    assert_eq!(
        healed.queue_generation, pair.queue_generation,
        "healing a source does not change queue-row identity"
    );
    assert!(!drop_preview_pair_is_current(&state, &handle, &pair, 71));
    state
        .db
        .with_conn(|conn| {
            conn.execute("UPDATE queue SET track_id=3 WHERE id=12", [])?;
            Ok(())
        })
        .unwrap();
    assert!(!drop_preview_pair_is_current(&state, &handle, &pair, 71));
}

#[tokio::test]
async fn missing_analysis_does_not_consume_preview_attempt() {
    let (state, handle, pair) = preview_fixture();
    let state = Arc::new(tokio::sync::RwLock::new(state));
    schedule_drop_preview_for_pair(state.clone(), handle, pair.clone(), 71)
        .await
        .unwrap();
    let key = (
        Arc::as_ptr(&state) as usize,
        1,
        2,
        pair.queue_generation,
        71,
    );
    let attempts = DROP_PREVIEW_ARM_ATTEMPTS
        .get_or_init(Default::default)
        .lock()
        .unwrap();
    assert!(
        !attempts.contains_key(&key),
        "analysis arriving later must still be able to schedule the mid-song preview"
    );
}

#[tokio::test]
async fn cockpit_reports_actual_preview_rejection_only_for_current_pair() {
    use tower::ServiceExt;
    let (mut state, _handle, pair) = preview_fixture();
    state.last_drop_preview = Some(crate::DropPreviewRuntimeState {
        track_id: 1,
        generation: 71,
        queue_generation: pair.queue_generation,
        actual_fire_ms: None,
        skipped_reason: Some("beat_sync_unverified"),
    });
    let state = Arc::new(tokio::sync::RwLock::new(state));
    let app = api_routes(state.clone());
    let read = || {
        axum::http::Request::builder()
            .uri("/api/dj/status")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let response = app.clone().oneshot(read()).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status["drop_preview"]["status"], "skipped");
    assert_eq!(status["drop_preview"]["reason"], "beat_sync_unverified");
    assert!(status["drop_preview"]["actual_fire_ms"].is_null());
    state
        .write()
        .await
        .last_drop_preview
        .as_mut()
        .unwrap()
        .queue_generation += 1;
    let response = app.oneshot(read()).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_ne!(status["drop_preview"]["reason"], "beat_sync_unverified");
}
