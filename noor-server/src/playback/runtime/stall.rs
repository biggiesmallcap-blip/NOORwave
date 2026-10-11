//! Stall watchdog for the active engine and the silent-start circuit breaker.

use super::*;

/// How often the runtime loop wakes (when no command is pending) to check for a
/// stalled active engine.
pub(super) const STALL_WATCHDOG_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// How long the audibly-active engine may make zero position progress -- while
/// playing and not paused -- before the watchdog force-advances the queue.
/// Sized comfortably past one healthy DASH segment timeout+retry cycle
/// (`cdn_health::HEALTHY_SEGMENT_TIMEOUT` = 12s) so a transiently-slow segment
/// that still arrives is not pre-empted, but a doomed TIDAL CDN stall recovers
/// automatically instead of freezing playback until the user manually skips.
/// The same budget covers a lost end-of-track terminal: 15s is far longer than
/// the sub-buffer gap between the buffer draining and a healthy terminal being
/// processed, so a working advance is never pre-empted by the watchdog.
pub(super) const ACTIVE_STALL_RECOVERY_SECS: u64 = 15;

/// An outgoing engine that lived at least this long without producing a
/// single audible sample counts as a silent failure for the advance-cascade
/// circuit breaker. Rapid manual skips tear down much younger engines and
/// must not count toward the streak.
pub(super) const SILENT_ENGINE_FAILURE_MIN_AGE: std::time::Duration =
    std::time::Duration::from_secs(10);

/// After this many consecutive silent engine failures the runtime stops
/// hot-advancing: it latches pause, surfaces one clear error, and leaves the
/// queue intact for the user to resume. Without this ceiling a dead TIDAL
/// CDN made the watchdog + track-error advances burn through the entire
/// queue 15-25s at a time while pause commands appeared to do nothing --
/// the state users could only escape by restarting the server.
pub(super) const MAX_SILENT_START_STREAK: u32 = 3;

/// Watchdog state for the runtime loop. The loop otherwise only advances when
/// the audio callback sends a command, and the callback goes quiet in two
/// distinct ways, both of which froze playback until the user clicked Next:
///
///   * Starved mid-track: a decoder hung on a TIDAL segment is
///     `started && !finished && written==0`. It emits no command and the
///     playhead freezes until the segment finally errors out, if it ever does.
///   * Lost end-of-track terminal: the engine is `finished` with a fully
///     drained buffer, so the callback's one-shot `TrackTerminal` was already
///     latched and sent. If it was then dropped downstream (no matching engine
///     slot, or a guard in the queue-advance handler), nothing re-issues it.
///
/// This tracker notices an active engine making no progress in either shape and
/// asks the loop to force the queue forward.
pub(super) struct StallTracker {
    pub(super) watching: Option<(i64, u64)>,
    pub(super) last_position: u64,
    pub(super) last_progress_at: std::time::Instant,
    /// True between a stall detection and the next progress/rearm. Gates the
    /// `Stalled` / `StallRecovered` event pair to one emission per episode.
    pub(super) stall_flagged: bool,
}

/// One engine's watchdog-relevant state, read once per tick. Decouples the
/// stall decision from `PlaybackRuntimeLoopState` so it is unit-testable.
#[derive(Debug, Clone, Copy)]
pub(super) struct EngineProbe {
    pub(super) id: (i64, u64),
    pub(super) position: u64,
    pub(super) paused: bool,
    pub(super) started: bool,
    pub(super) finished: bool,
    /// No unread samples left in the buffer. Combined with `finished` this is
    /// end-of-track: there is no more audio coming and none left to play.
    pub(super) drained: bool,
}

/// Which failure shape triggered a force-advance. Only meaningful when
/// `StallPollOutcome::force_advance` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StallKind {
    /// Decode had not finished and the engine ran dry: a hung stream mid-track.
    Starved,
    /// Decode finished and the buffer fully drained, but the queue never moved.
    /// The audio callback's one-shot terminal was lost somewhere between
    /// `finished_notified` being latched and the queue advance running.
    LostTerminal,
}

/// What one watchdog tick decided.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct StallPollOutcome {
    /// Engine starved past `ACTIVE_STALL_RECOVERY_SECS`: force the queue
    /// forward. Re-fires every threshold interval while the stall persists.
    pub(super) force_advance: Option<(i64, u64)>,
    /// First tick of a stall episode: pause the listen session (track_id).
    pub(super) just_stalled: Option<i64>,
    /// First progress after a stall episode on the SAME engine: resume the
    /// listen session (track_id). Engine changes do not emit this - the
    /// track-change flow flushes the session instead.
    pub(super) just_recovered: Option<i64>,
    /// Set alongside `force_advance` to distinguish a mid-track starve from a
    /// lost end-of-track terminal, so the two log distinguishably.
    pub(super) kind: Option<StallKind>,
}

impl StallTracker {
    pub(super) fn new() -> Self {
        Self {
            watching: None,
            last_position: 0,
            last_progress_at: std::time::Instant::now(),
            stall_flagged: false,
        }
    }

    /// Re-arm against the current active engine without flagging a stall. Used
    /// when the engine is paused, finished, freshly changed, or making progress
    /// -- none of which are stalls.
    pub(super) fn rearm(&mut self, id: (i64, u64), position: u64) {
        self.watching = Some(id);
        self.last_position = position;
        self.last_progress_at = std::time::Instant::now();
        self.stall_flagged = false;
    }

    /// Called on each idle watchdog tick.
    pub(super) fn poll(&mut self, state: &PlaybackRuntimeLoopState) -> StallPollOutcome {
        let Some(engine) = state.engine.as_ref() else {
            self.watching = None;
            self.stall_flagged = false;
            return StallPollOutcome::default();
        };
        let (started, finished, drained) = engine
            .shared
            .buffer
            .lock()
            .map(|guard| {
                (
                    guard.started,
                    guard.finished,
                    guard.samples.len() <= guard.read_pos,
                )
            })
            .unwrap_or((false, false, false));
        self.observe(EngineProbe {
            id: (engine.track_id, engine.generation),
            position: engine.shared.position_samples.load(Ordering::Relaxed),
            paused: engine.shared.paused.load(Ordering::SeqCst),
            started,
            finished,
            drained,
        })
    }

    /// Pure decision core over one engine probe.
    pub(super) fn observe(&mut self, probe: EngineProbe) -> StallPollOutcome {
        let mut outcome = StallPollOutcome::default();

        // Paused playback legitimately makes no progress.
        if probe.paused {
            self.rearm(probe.id, probe.position);
            return outcome;
        }
        // A not-yet-started engine is still doing its initial prebuffer -- on a
        // slow connection the first ~500ms can legitimately take many seconds to
        // arrive, and the playhead sits at the baseline offset the whole time.
        // That is not a stall; only a deck that WAS playing and then froze is.
        //
        // `finished` used to be exempted here too, on the reasoning that such an
        // engine "emits its own terminal via the audio callback". That terminal
        // is one-shot (`finished_notified` is latched before the send and never
        // re-armed), so when it was lost the exemption meant nothing recovered
        // the queue and playback froze at the end of the track until the user
        // hit Next. Worse, the decoder marks `finished` as soon as decode
        // completes -- with DASH lookahead that is minutes before playback
        // reaches the end -- so the exemption disarmed the watchdog across the
        // whole back half of every track. Finished engines are now watched like
        // any other; a finished engine that is still playing out its buffer
        // keeps moving its position and rearms below on its own.
        if !probe.started {
            self.rearm(probe.id, probe.position);
            return outcome;
        }
        // New track, or audible progress since the last tick -> not stalled.
        // Progress on the engine we flagged ends the stall episode: tell the
        // listener to resume the listen session it paused.
        if self.watching != Some(probe.id) || probe.position != self.last_position {
            if self.stall_flagged && self.watching == Some(probe.id) {
                outcome.just_recovered = Some(probe.id.0);
            }
            self.rearm(probe.id, probe.position);
            return outcome;
        }
        // Same track, no progress since the last tick: over budget?
        if self.last_progress_at.elapsed()
            >= std::time::Duration::from_secs(ACTIVE_STALL_RECOVERY_SECS)
        {
            // Reset the clock so we don't re-fire every tick while the synthetic
            // terminal is in flight and the queue advances.
            self.last_progress_at = std::time::Instant::now();
            if !self.stall_flagged {
                self.stall_flagged = true;
                outcome.just_stalled = Some(probe.id.0);
            }
            outcome.force_advance = Some(probe.id);
            outcome.kind = Some(if probe.finished && probe.drained {
                StallKind::LostTerminal
            } else {
                StallKind::Starved
            });
        }
        outcome
    }
}
