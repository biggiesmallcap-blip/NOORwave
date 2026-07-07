//! In-memory play history for previous-track navigation.
//!
//! Records what actually played, in play order, independent of the queue.
//! The queue cannot serve as history: shuffle reorders it, automix appends to
//! it, manual jumps skip across it, and ephemeral TIDAL mix rows are deleted
//! as they play. Persisted plays are remembered as (queue row id, track id)
//! pairs and re-validated against the live queue at pop time; ephemeral mix
//! plays keep the full pending payload so they can be replayed after their
//! queue row is gone.
//!
//! Lives in `AppState` behind the SharedState RwLock (no interior locking).
//! In-memory only by design: on server restart the stack is empty and
//! previous-track falls back to queue-order stepping.

use crate::PendingEphemeralTidalTrack;

/// Upper bound on remembered plays. At ~4 minutes a track this is well over
/// half a day of continuous listening; older entries are dropped from the
/// bottom of the stack.
const PLAY_HISTORY_CAP: usize = 200;

#[derive(Debug, Clone)]
pub enum PlayHistoryEntry {
    /// A persisted queue row played. The row may since have been removed,
    /// reordered, or re-resolved; consumers must re-validate against the
    /// live queue before navigating to it.
    Persisted { queue_item_id: i64, track_id: i64 },
    /// An ephemeral TIDAL mix/album/playlist track played. Its queue row was
    /// deleted when it started, so the pending payload is kept whole to allow
    /// replaying it through the ephemeral pipeline.
    Ephemeral(PendingEphemeralTidalTrack),
}

impl PlayHistoryEntry {
    /// Whether two entries denote the same playback item (not payload
    /// equality: an ephemeral entry re-enriched with artwork later still
    /// matches its earlier self).
    fn same_playback(&self, other: &PlayHistoryEntry) -> bool {
        match (self, other) {
            (
                PlayHistoryEntry::Persisted {
                    queue_item_id: a, ..
                },
                PlayHistoryEntry::Persisted {
                    queue_item_id: b, ..
                },
            ) => a == b,
            (PlayHistoryEntry::Ephemeral(a), PlayHistoryEntry::Ephemeral(b)) => {
                a.tidal_track_id == b.tidal_track_id
            }
            _ => false,
        }
    }
}

/// One-shot guard that stops a back-navigation from feeding history.
///
/// Without it, "previous" would push the track being navigated away from,
/// and two prev presses in a row would ping-pong between the same two tracks
/// instead of walking further back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushSuppression {
    /// Suppress the note whose start carries exactly this playback
    /// generation (persisted prev: the generation is known before the
    /// switch is dispatched).
    Generation(u64),
    /// Suppress the next note regardless of generation (ephemeral prev: the
    /// ephemeral starter bumps the generation internally, so the caller
    /// cannot key on it).
    NextStart,
}

#[derive(Debug, Default)]
pub struct PlayHistory {
    entries: Vec<PlayHistoryEntry>,
    current: Option<PlayHistoryEntry>,
    suppression: Option<PushSuppression>,
}

impl PlayHistory {
    /// Arm suppression for a persisted back-navigation about to dispatch a
    /// switch with this playback generation.
    pub fn suppress_push_for_generation(&mut self, generation: u64) {
        self.suppression = Some(PushSuppression::Generation(generation));
    }

    /// Arm suppression for an ephemeral back-navigation (generation unknown
    /// to the caller).
    pub fn suppress_next_push(&mut self) {
        self.suppression = Some(PushSuppression::NextStart);
    }

    /// Disarm suppression after a failed back-navigation so the next real
    /// track start is recorded normally.
    pub fn clear_suppression(&mut self) {
        self.suppression = None;
    }

    /// Record that `entry` started playing under `generation`. Pushes the
    /// outgoing current entry onto the stack unless this start is a
    /// suppressed back-navigation or a restart of the entry already current
    /// (segment-restart seeks and restart-in-place re-fire Started for the
    /// same item).
    pub fn note_started(&mut self, entry: PlayHistoryEntry, generation: u64) {
        let suppressed = match self.suppression {
            Some(PushSuppression::NextStart) => true,
            Some(PushSuppression::Generation(g)) => g == generation,
            None => false,
        };
        if suppressed {
            self.suppression = None;
        }

        if self
            .current
            .as_ref()
            .is_some_and(|current| current.same_playback(&entry))
        {
            // Same item (re)starting: refresh the payload, never self-push.
            self.current = Some(entry);
            return;
        }

        let outgoing = self.current.replace(entry);
        if suppressed {
            return;
        }
        if let Some(outgoing) = outgoing {
            if self.entries.len() >= PLAY_HISTORY_CAP {
                self.entries.remove(0);
            }
            self.entries.push(outgoing);
        }
    }

    /// Pop the most recently played entry. Skips anything matching the
    /// current item (defensive; `note_started` dedupe should keep it out).
    pub fn pop_previous(&mut self) -> Option<PlayHistoryEntry> {
        while let Some(entry) = self.entries.pop() {
            let is_current = self
                .current
                .as_ref()
                .is_some_and(|current| current.same_playback(&entry));
            if !is_current {
                return Some(entry);
            }
        }
        None
    }

    /// Put back an entry popped by a back-navigation that then failed
    /// (stream resolve error, runtime unavailable), so retrying "previous"
    /// targets the same track instead of silently skipping past it.
    pub fn restore_popped(&mut self, entry: PlayHistoryEntry) {
        if self.entries.len() >= PLAY_HISTORY_CAP {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persisted(queue_item_id: i64, track_id: i64) -> PlayHistoryEntry {
        PlayHistoryEntry::Persisted {
            queue_item_id,
            track_id,
        }
    }

    fn ephemeral(tidal_track_id: i64) -> PlayHistoryEntry {
        PlayHistoryEntry::Ephemeral(PendingEphemeralTidalTrack {
            tidal_track_id,
            title: format!("mix track {tidal_track_id}"),
            artist_name: None,
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            artist_tidal_id: None,
            album_tidal_id: None,
        })
    }

    fn queue_item_id_of(entry: &PlayHistoryEntry) -> i64 {
        match entry {
            PlayHistoryEntry::Persisted { queue_item_id, .. } => *queue_item_id,
            PlayHistoryEntry::Ephemeral(pending) => pending.tidal_track_id,
        }
    }

    #[test]
    fn forward_plays_stack_in_order_and_prev_walks_back() {
        let mut history = PlayHistory::default();
        history.note_started(persisted(10, 1), 1);
        history.note_started(persisted(11, 2), 2);
        history.note_started(persisted(12, 3), 3);

        // Playing C; history holds [A, B].
        let back = history.pop_previous().expect("B expected");
        assert_eq!(queue_item_id_of(&back), 11);

        // Back-navigation to B: suppressed start must not push C.
        history.suppress_push_for_generation(4);
        history.note_started(persisted(11, 2), 4);

        let back = history.pop_previous().expect("A expected");
        assert_eq!(queue_item_id_of(&back), 10);

        history.suppress_push_for_generation(5);
        history.note_started(persisted(10, 1), 5);

        assert!(history.pop_previous().is_none(), "history exhausted");
    }

    #[test]
    fn restart_of_current_item_does_not_self_push() {
        let mut history = PlayHistory::default();
        history.note_started(persisted(10, 1), 1);
        history.note_started(persisted(11, 2), 2);
        // Segment-restart / restart-in-place re-fires Started for the same row.
        history.note_started(persisted(11, 2), 3);

        let back = history.pop_previous().expect("A expected");
        assert_eq!(queue_item_id_of(&back), 10);
        assert!(history.pop_previous().is_none());
    }

    #[test]
    fn generation_suppression_only_matches_its_generation() {
        let mut history = PlayHistory::default();
        history.note_started(persisted(10, 1), 1);
        history.suppress_push_for_generation(7);
        // A different start (e.g. natural advance racing the prev) is pushed.
        history.note_started(persisted(11, 2), 8);

        let back = history.pop_previous().expect("A pushed despite marker");
        assert_eq!(queue_item_id_of(&back), 10);
    }

    #[test]
    fn next_start_suppression_swallows_exactly_one_push() {
        let mut history = PlayHistory::default();
        history.note_started(ephemeral(100), 1);
        history.note_started(ephemeral(101), 2);
        history.suppress_next_push();
        history.note_started(ephemeral(100), 3); // back-nav, 101 not pushed
        history.note_started(ephemeral(102), 4); // forward again, 100 pushed

        // 100 was audible twice on this walk (first play, then the back-nav
        // visit before moving forward to 102), so it appears twice.
        let back = history.pop_previous().expect("100 expected");
        assert_eq!(queue_item_id_of(&back), 100);
        let back = history.pop_previous().expect("first 100 play expected");
        assert_eq!(queue_item_id_of(&back), 100);
        assert!(history.pop_previous().is_none());
    }

    #[test]
    fn cap_drops_oldest_entries() {
        let mut history = PlayHistory::default();
        for i in 0..(PLAY_HISTORY_CAP as i64 + 10) {
            history.note_started(persisted(i, i), i as u64);
        }
        // Newest previous first.
        let back = history.pop_previous().expect("entry expected");
        assert_eq!(queue_item_id_of(&back), PLAY_HISTORY_CAP as i64 + 8);
    }

    #[test]
    fn restore_popped_reinstates_the_entry() {
        let mut history = PlayHistory::default();
        history.note_started(persisted(10, 1), 1);
        history.note_started(persisted(11, 2), 2);

        let popped = history.pop_previous().expect("A expected");
        history.restore_popped(popped);

        let again = history.pop_previous().expect("A restored");
        assert_eq!(queue_item_id_of(&again), 10);
    }
}
