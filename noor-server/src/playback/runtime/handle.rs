//! PlaybackRuntimeHandle: the command-sending side of the runtime.

use super::*;

#[derive(Clone)]
pub struct PlaybackRuntimeHandle {
    pub(super) command_tx: mpsc::Sender<PlaybackRuntimeCommand>,
    pub(super) event_tx: tokio::sync::broadcast::Sender<PlaybackRuntimeEvent>,
    pub(super) healthy: Arc<AtomicBool>,
    /// f32 volume (0.0–1.0) stored as its bit-pattern in a u32.
    pub(super) volume_ctl: Arc<AtomicU32>,
    /// Redirectable position reader. Normally points to the active engine's
    /// position counter. Swapped at crossfade promotion so the handle always
    /// reads from the engine that's audibly current, not the fading-out one.
    ///
    /// Real-time-safety / unwrap audit (Task 15): every access site uses
    /// `position_source.lock().unwrap()` because the protected payload is an
    /// `Arc<AtomicU64>` - mutex poisoning leaves it valid (Arc is either the
    /// old reference or the new one, both safe to read/write). The only way
    /// `.unwrap()` panics is if a code path inside the guard panics first,
    /// which is then caught by `handle_panic_in_runtime_loop` (Task 7) and
    /// surfaced to the user as `PlaybackRuntimeEvent::Error`. So these
    /// unwraps are bounded-failure, not silent corruption.
    pub(super) position_source: Arc<Mutex<Arc<AtomicU64>>>,
    /// Redirectable buffered-samples reader. Parallel to `position_source`:
    /// always points at the audibly-current engine's `buffered_samples`
    /// counter, swapped at the same sites position_source is swapped (cold
    /// start in `transition_to_job` and at the two `promote_*` sites). The
    /// route-side seek ack reads through this to decide 409 vs dispatch, and
    /// the frontend reads `buffered_ms` via this for the buffered-bar
    /// scrubber. Same unwrap-audit reasoning as `position_source`.
    pub(super) buffered_source: Arc<Mutex<Arc<AtomicU64>>>,
    /// Redirectable engine-offset reader (option C: true DASH segment seek).
    /// Points at the audibly-current engine's `position_offset_samples`
    /// counter. For a fresh play this reads 0; for a segment-restart engine
    /// it reads the absolute-track sample where the engine's decoded audio
    /// starts. The route-side seek handler uses this as the LOWER bound of
    /// the in-buffer decision (target must be `>= offset` to be in-buffer);
    /// the frontend reads `buffered_start_ms` via this as a visual cue.
    /// Same unwrap-audit reasoning as `position_source` / `buffered_source`.
    pub(super) offset_source: Arc<Mutex<Arc<AtomicU64>>>,
    pub(super) handoff_elapsed_source: Arc<Mutex<Arc<AtomicU64>>>,
}

impl PlaybackRuntimeHandle {
    #[cfg(test)]
    pub(crate) fn test_publish_event(&self, event: PlaybackRuntimeEvent) -> bool {
        self.event_tx.send(event).is_ok()
    }

    #[cfg(test)]
    pub(crate) fn test_publish_position(&self, samples: u64) {
        self.position_source
            .lock()
            .unwrap()
            .store(samples, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn test_with_command_tx(command_tx: mpsc::Sender<PlaybackRuntimeCommand>) -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        Self {
            command_tx,
            event_tx,
            healthy: Arc::new(AtomicBool::new(true)),
            volume_ctl: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            position_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            buffered_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            offset_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(0)))),
            handoff_elapsed_source: Arc::new(Mutex::new(Arc::new(AtomicU64::new(u64::MAX)))),
        }
    }

    pub fn play(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Play(job))
    }

    pub fn switch_to(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Switch(job))
    }

    pub fn pause(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Pause)
    }

    pub fn resume(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Resume)
    }

    pub fn stop(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Stop)
    }

    /// Release the WASAPI exclusive device now (ahead of the idle grace) so the
    /// WebView can play a video's audio in shared mode. No-op outside Windows
    /// exclusive mode. Callers should pause playback first; the runtime
    /// re-grabs exclusive on the next Resume/Play.
    pub fn release_exclusive_now(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::ReleaseExclusiveNow)
    }

    pub fn shutdown(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::Shutdown)
    }

    /// Live-swap the CPAL output device (and exclusive / sample-rate-follow flags)
    /// across any active engines. Used by the audio settings PUT route and track
    /// transitions when sample_rate_follow is enabled. Optional desired_sample_rate
    /// allows specifying an exact target (e.g. next track's native rate).
    /// `exclusive_release_grace_secs` is the idle-release grace window for the
    /// WASAPI exclusive render thread (ignored unless `exclusive` is true).
    pub fn device_swap(
        &self,
        device: OutputDeviceSelection,
        exclusive: bool,
        sample_rate_follow: bool,
        desired_sample_rate: Option<u32>,
        exclusive_release_grace_secs: u32,
        exclusive_latency_mode: ExclusiveLatencyMode,
    ) -> Result<()> {
        self.send(PlaybackRuntimeCommand::DeviceSwap {
            device,
            exclusive,
            sample_rate_follow,
            desired_sample_rate,
            exclusive_release_grace_secs,
            exclusive_latency_mode,
        })
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PlaybackRuntimeEvent> {
        self.event_tx.subscribe()
    }

    pub fn request_ready(&self) -> Result<()> {
        self.send(PlaybackRuntimeCommand::RequestReady)
    }

    pub(crate) fn resolved_analysis_stream(&self, track_id: i64) -> Option<StreamInfo> {
        let (respond_to, response) = mpsc::channel();
        self.send(PlaybackRuntimeCommand::ResolvedAnalysisStream {
            track_id,
            respond_to,
        })
        .ok()?;
        response.recv_timeout(Duration::from_secs(2)).ok().flatten()
    }

    pub(crate) fn update_prepared_transition(
        &self,
        transition: PreparedTransitionProgram,
        gapless: GaplessPlan,
    ) -> bool {
        let (respond_to, response) = mpsc::channel();
        if self
            .send(PlaybackRuntimeCommand::UpdatePreparedTransition {
                transition,
                gapless,
                respond_to,
            })
            .is_err()
        {
            return false;
        }
        response
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or(false)
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    pub(crate) fn is_same_runtime(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.healthy, &other.healthy)
    }

    /// Segment-aware seek. Single entry point for all seek requests; the
    /// runtime decides between in-buffer fast path, forced-restart segment
    /// seek, or rejection. The `allow_segment_seek` flag opts in to the
    /// segment-restart transition; with `false`, the runtime treats
    /// out-of-buffer seeks as rejected (legacy semantics).
    ///
    /// Blocks up to 1500ms for the reply (segment-restart transitions need
    /// time to tear down the old engine and spin up the decoder thread on
    /// the new one). Returns `SeekToOutcome::Failed` on timeout / channel
    /// closure - treat as a recoverable error from the caller's perspective.
    pub fn seek_to_segment_aware(
        &self,
        position_ms: i64,
        allow_segment_seek: bool,
    ) -> SeekToOutcome {
        let (tx, rx) = std::sync::mpsc::channel();
        if self
            .send(PlaybackRuntimeCommand::SeekTo {
                target_ms: position_ms,
                allow_segment_seek,
                respond_to: tx,
            })
            .is_err()
        {
            return SeekToOutcome::Failed;
        }
        rx.recv_timeout(std::time::Duration::from_millis(1500))
            .unwrap_or(SeekToOutcome::Failed)
    }

    /// Legacy seek wrapper. Equivalent to `seek_to_segment_aware(position_ms,
    /// false)` returning a `Result<()>`. Kept so non-route callers (none
    /// today; audit `git grep "\\.seek\\("` if adding new ones) don't have to
    /// adopt the SeekToOutcome enum just to issue a plain seek. Out-of-buffer
    /// or failed transitions surface as `Err`.
    pub fn seek(&self, position_ms: i64) -> Result<()> {
        match self.seek_to_segment_aware(position_ms, false) {
            SeekToOutcome::Dispatched | SeekToOutcome::DispatchedCrossfadeSuppressed => Ok(()),
            SeekToOutcome::RejectedOutOfBuffer => Err(anyhow!("seek target is out of buffer")),
            SeekToOutcome::Failed => Err(anyhow!("seek dispatch failed")),
        }
    }

    /// Pre-decode the next track in the background so the transition is gapless.
    pub fn prepare_next(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::PrepareNext(job))
    }

    pub fn prepare_drop_preview(&self, job: PreparedPlaybackJob) -> Result<()> {
        self.send(PlaybackRuntimeCommand::PrepareDropPreview(job))
    }

    pub fn arm_drop_preview(
        &self,
        track_id: i64,
        generation: u64,
        trigger_position_samples: u64,
    ) -> Result<()> {
        self.send(PlaybackRuntimeCommand::ArmDropPreview {
            track_id,
            generation,
            trigger_position_samples,
        })
    }

    pub fn set_dj_engine_enabled(&self, enabled: bool) -> Result<()> {
        self.send(PlaybackRuntimeCommand::SetDjEngineEnabled { enabled })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_dj_lookahead(
        &self,
        current: Option<DjMediaRef>,
        next: Option<DjMediaRef>,
        current_queue_item_id: Option<i64>,
        next_queue_item_id: Option<i64>,
        queue_generation: u64,
        deadline_samples: u64,
    ) -> Result<()> {
        self.send(PlaybackRuntimeCommand::StartDjLookahead {
            current,
            next,
            current_queue_item_id,
            next_queue_item_id,
            queue_generation,
            deadline_samples,
        })
    }

    pub fn track_status(&self, track_id: i64, generation: u64) -> PlaybackTrackStatus {
        let (tx, rx) = std::sync::mpsc::channel();
        if self
            .send(PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to: tx,
            })
            .is_err()
        {
            return PlaybackTrackStatus::None;
        }
        rx.recv_timeout(std::time::Duration::from_millis(100))
            .unwrap_or(PlaybackTrackStatus::None)
    }

    /// Set playback volume (0.0 = silent, 1.0 = full). Applied immediately to the CPAL callback.
    pub fn set_volume(&self, volume: f32) {
        self.volume_ctl
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Read the current playback position in milliseconds from the CPAL sample counter.
    pub fn get_position_ms(&self, device_sample_rate: u32, device_channels: u16) -> i64 {
        if device_sample_rate == 0 || device_channels == 0 {
            return 0;
        }
        let samples = self.position_source.lock().unwrap().load(Ordering::Relaxed);
        (samples * 1000 / (device_sample_rate as u64 * device_channels as u64)) as i64
    }

    /// Actual output-clock progress of an installed, currently audible
    /// handoff. None before fire, after resolution, or after a manual seek.
    pub fn get_dj_handoff_elapsed_ms(&self, sample_rate: u32, channels: u16) -> Option<i64> {
        if sample_rate == 0 || channels == 0 {
            return None;
        }
        let samples = self
            .handoff_elapsed_source
            .lock()
            .unwrap()
            .load(Ordering::Relaxed);
        (samples != u64::MAX).then(|| samples_to_ms(samples, sample_rate, channels))
    }

    /// Read how many ms of the current track are decoded into the playback
    /// buffer. Returns 0 when no engine is active. Used by the route-side
    /// seek ack (target > buffered -> HTTP 409) and surfaced to the frontend
    /// via `PlaybackState.buffered_ms` for the buffered-bar scrubber.
    /// Same unwrap-audit reasoning as `get_position_ms`.
    pub fn get_buffered_ms(&self, device_sample_rate: u32, device_channels: u16) -> i64 {
        if device_sample_rate == 0 || device_channels == 0 {
            return 0;
        }
        let samples = self.buffered_source.lock().unwrap().load(Ordering::Relaxed);
        (samples * 1000 / (device_sample_rate as u64 * device_channels as u64)) as i64
    }

    /// Raw buffered-sample count from the audibly-current engine. Avoids the
    /// ms conversion when the caller already has a target-in-samples (e.g.
    /// the route-side seek handler comparing target_samples to buffered).
    pub fn buffered_samples(&self) -> u64 {
        self.buffered_source.lock().unwrap().load(Ordering::Relaxed)
    }

    /// Read the engine's track-time offset in milliseconds (lower bound of
    /// the decoded range). Returns 0 for a fresh-from-start engine; returns
    /// the segment offset for a segment-restart engine. Read via the
    /// redirectable `offset_source` so it always reflects the audibly-current
    /// engine, not the fading-out one. Used by `build_live_playback_snapshot`
    /// to populate `PlaybackState.buffered_start_ms` and by the runtime's
    /// SeekTo handler indirectly via `evaluate_seek_decision`.
    pub fn get_buffered_start_ms(&self, device_sample_rate: u32, device_channels: u16) -> i64 {
        if device_sample_rate == 0 || device_channels == 0 {
            return 0;
        }
        let samples = self.offset_source.lock().unwrap().load(Ordering::Relaxed);
        (samples * 1000 / (device_sample_rate as u64 * device_channels as u64)) as i64
    }

    /// Raw offset-sample count from the audibly-current engine. Companion to
    /// `buffered_samples()`; the route-side SeekTo handler uses both as the
    /// `[offset, buffered]` bounds of the in-buffer fast path.
    pub fn buffered_start_samples(&self) -> u64 {
        self.offset_source.lock().unwrap().load(Ordering::Relaxed)
    }

    pub(super) fn send(&self, command: PlaybackRuntimeCommand) -> Result<()> {
        self.command_tx.send(command).map_err(|_| {
            self.healthy.store(false, Ordering::Release);
            anyhow!("playback runtime command channel closed")
        })
    }
}
