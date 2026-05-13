//! Queue-lookahead DSP prescanner.
//!
//! On each `QueueUpdated` / `TrackChanged` event, the actor debounces for
//! `DEBOUNCE` and then preview-analyses up to `LOOKAHEAD` upcoming tracks via
//! TIDAL LOW-quality streams. Each completed track emits `TrackAnalyzed` so
//! the automix cockpit refreshes its feature pills in place.
//!
//! Cancellation: a new queue event during a batch causes the in-flight track
//! to finish (the LOW download is small), then the loop exits and re-debounces
//! against the latest queue state. Granularity is one track (~2-3 s).

/// How many upcoming queue items to consider per batch.
pub const LOOKAHEAD: usize = 5;
/// Debounce window after a queue change before kicking off a batch.
pub const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(1500);
/// Polite pause between tracks within a batch.
pub const INTER_TRACK_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// One row of queue state, projected for the pure selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrescanCandidate {
    pub track_id: i64,
    pub position: i64,
    pub has_tidal_id: bool,
    /// `None` when no DSP row exists yet; otherwise the stored version.
    pub analysis_version: Option<String>,
}

/// Pick up to `lookahead` upcoming tracks that need (re)analysis.
///
/// Filters in this order, then truncates by `lookahead`:
/// 1. Position strictly greater than `current_position`
/// 2. Has a TIDAL id (LOW-quality preview download requires one)
/// 3. Analysis version is missing or != `current_version`
pub fn pick_next_unanalyzed(
    candidates: &[PrescanCandidate],
    current_position: i64,
    lookahead: usize,
    current_version: &str,
) -> Vec<i64> {
    let mut filtered: Vec<&PrescanCandidate> = candidates
        .iter()
        .filter(|c| c.position > current_position)
        .filter(|c| c.has_tidal_id)
        .filter(|c| c.analysis_version.as_deref() != Some(current_version))
        .collect();
    filtered.sort_by_key(|c| c.position);
    filtered
        .into_iter()
        .take(lookahead)
        .map(|c| c.track_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(track_id: i64, position: i64, tidal: bool, version: Option<&str>) -> PrescanCandidate {
        PrescanCandidate {
            track_id,
            position,
            has_tidal_id: tidal,
            analysis_version: version.map(String::from),
        }
    }

    #[test]
    fn empty_queue_yields_nothing() {
        assert!(pick_next_unanalyzed(&[], 0, 5, "v5").is_empty());
    }

    #[test]
    fn skips_already_current_version() {
        let cs = vec![
            c(1, 1, true, Some("v5")),
            c(2, 2, true, Some("v5")),
            c(3, 3, true, Some("v4")),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![3]);
    }

    #[test]
    fn skips_rows_at_or_before_current_position() {
        let cs = vec![
            c(1, 0, true, None),
            c(2, 1, true, None),
            c(3, 2, true, None),
            c(4, 3, true, None),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 1, 5, "v5"), vec![3, 4]);
    }

    #[test]
    fn skips_rows_without_tidal_id() {
        let cs = vec![
            c(1, 1, false, None),
            c(2, 2, true, None),
            c(3, 3, false, None),
            c(4, 4, true, None),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![2, 4]);
    }

    #[test]
    fn caps_at_lookahead() {
        let cs: Vec<PrescanCandidate> = (1..=10).map(|i| c(i, i, true, None)).collect();
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn returns_in_position_order_even_when_input_is_shuffled() {
        let cs = vec![
            c(30, 3, true, None),
            c(10, 1, true, None),
            c(20, 2, true, None),
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![10, 20, 30]);
    }

    #[test]
    fn missing_version_is_treated_as_stale() {
        let cs = vec![
            c(1, 1, true, None),       // no DSP row yet
            c(2, 2, true, Some("v5")), // already current
        ];
        assert_eq!(pick_next_unanalyzed(&cs, 0, 5, "v5"), vec![1]);
    }
}
