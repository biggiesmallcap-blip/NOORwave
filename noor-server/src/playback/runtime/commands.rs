use super::OutputDeviceSelection;
use crate::db::audio_settings::ExclusiveLatencyMode;
use crate::playback::player::{PlaybackSourceKind, PreparedPlaybackJob};

#[derive(Debug, Clone)]
pub enum PlaybackRuntimeCommand {
    Play(PreparedPlaybackJob),
    Switch(PreparedPlaybackJob),
    Pause,
    Resume,
    Stop,
    /// Seek to a position (milliseconds). Applied to the current engine's buffer.
    Seek(i64),
    /// Pre-decode the next track in background so it can start gaplessly.
    PrepareNext(PreparedPlaybackJob),
    /// Sent by the decoder thread when a track finishes decoding successfully.
    /// Used to start the crossfade stream if the crossfade window has already opened.
    NextDecodeComplete {
        track_id: i64,
        generation: u64,
    },
    /// Sent by the CPAL callback when the current track is within `crossfade_ms` of its end.
    /// If a pre-decoded next engine is ready, its stream is unpaused so both mix via the OS.
    CrossfadeStart {
        track_id: i64,
        generation: u64,
    },
    TrackTerminal {
        track_id: i64,
        generation: u64,
        outcome: PlaybackTerminalReason,
    },
    TrackStatus {
        track_id: i64,
        generation: u64,
        respond_to: std::sync::mpsc::Sender<PlaybackTrackStatus>,
    },
    /// Swap the CPAL output device for any active engines. Shared mode only at
    /// this stage; `exclusive` and `sample_rate_follow` are wired in later tasks
    /// (5 + 6) and ignored here. Optional `desired_sample_rate` allows the route
    /// layer to specify an exact target rate (e.g. per-track transitions).
    DeviceSwap {
        device: OutputDeviceSelection,
        exclusive: bool,
        sample_rate_follow: bool,
        desired_sample_rate: Option<u32>,
        /// Idle-release grace seconds for the WASAPI exclusive render thread.
        /// Read from `AudioSettings::exclusive_release_grace_secs` by the route
        /// layer. Ignored when `exclusive` is false.
        exclusive_release_grace_secs: u32,
        /// WASAPI exclusive callback period policy. Ignored when `exclusive`
        /// is false.
        exclusive_latency_mode: ExclusiveLatencyMode,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackTrackStatus {
    None,
    Active,
    Prepared,
}

#[derive(Debug, Clone)]
pub enum PlaybackTerminalReason {
    Finished,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum PlaybackRuntimeEvent {
    Ready {
        device_name: String,
        sample_rate: u32,
        channels: u16,
    },
    Preparing {
        track_id: i64,
        source: PlaybackSourceKind,
    },
    Started {
        track_id: i64,
        generation: u64,
        source: PlaybackSourceKind,
    },
    Paused {
        track_id: Option<i64>,
    },
    Resumed {
        track_id: Option<i64>,
    },
    Stopped,
    Finished {
        track_id: i64,
        generation: u64,
    },
    /// Fired when the current track is within `NEAR_END_THRESHOLD_MS` of its end.
    /// The listener should peek the next track and send `PrepareNext` to pre-buffer it.
    NearEnd {
        track_id: i64,
        generation: u64,
    },
    Error {
        message: String,
    },
    /// WASAPI exclusive grab succeeded. Frontend clears any stale "failure"
    /// banner and shows engaged state.
    ExclusiveModeEngaged {
        device_name: String,
        transport_format: String,
    },
    /// WASAPI exclusive grab failed; runtime fell back to cpal shared so the
    /// user still hears audio. `reason` is a human-readable explanation.
    ExclusiveModeFailed {
        reason: String,
        device_name: String,
    },
    /// WASAPI exclusive render thread released the device after grace timeout.
    /// Frontend clears engaged state. Runtime will re-grab on next Resume/Play.
    ExclusiveModeReleased {
        device_name: String,
    },
}
