//! Song Radio orchestrator.
//!
//! Given a seed (track | album | artist), fans out to three sources in parallel:
//!   - Library: embedding neighbors via discovery_learning::radio_from_neighbors
//!   - Last.fm: track.getSimilar resolved to Tidal IDs (Task 3)
//!   - Engine:  external_discovery_engine (slot exists; v1 produces empty)
//!
//! Applies a blend (Familiar/Mixed/Adventurous), ISRC-dedups with library
//! preference, tags each result with provenance, returns a queue.

use crate::db::Database;
use crate::metadata::lastfm::{LastFmClient, LastFmSimilarTrack};
use crate::services::discovery::DiscoveryCandidateTrack;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RadioBlend {
    Familiar,
    Mixed,
    Adventurous,
}

impl Default for RadioBlend {
    fn default() -> Self {
        RadioBlend::Mixed
    }
}

impl RadioBlend {
    /// Returns (library_weight, lastfm_weight, engine_weight) summing to 1.0.
    pub fn weights(self) -> (f64, f64, f64) {
        match self {
            RadioBlend::Familiar => (0.60, 0.30, 0.10),
            RadioBlend::Mixed => (0.30, 0.40, 0.30),
            RadioBlend::Adventurous => (0.10, 0.40, 0.50),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RadioSource {
    Library,
    Lastfm,
    Engine,
}

#[derive(Debug, Clone, Serialize)]
pub struct RadioCandidate {
    /// Library track id when `is_in_library`; otherwise the resolved Tidal id (best-effort).
    /// Used as a stable canvas/queue identifier.
    pub track_id: i64,
    /// For playback. Always set when known.
    pub tidal_track_id: Option<i64>,
    pub title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub isrc: Option<String>,
    pub is_in_library: bool,
    pub source: RadioSource,
    /// Human-readable explanation for the hover-card "Why is this here?" line.
    pub reason: String,
    /// 0..1 source-native score, normalized for cross-source comparison.
    pub similarity_score: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RadioSeed {
    pub kind: &'static str, // "track" | "album" | "artist"
    pub track_id: Option<i64>,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub title: String,
    pub artist_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RadioQueue {
    pub session_id: String,
    pub blend_used: RadioBlend,
    pub seed: RadioSeed,
    pub tracks: Vec<RadioCandidate>,
}

// ─── Public orchestrators (full implementations land in Task 3) ──────────────

/// Build a Song Radio queue seeded from a single library track.
pub async fn orchestrate_song(
    _db: &Database,
    _lastfm: Option<&LastFmClient>,
    _seed_track_id: i64,
    _blend: RadioBlend,
    _limit: usize,
    _exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    anyhow::bail!("orchestrate_song not yet implemented")
}

/// Build a Song Radio queue from an album (multi-seed using album tracks).
pub async fn orchestrate_album(
    _db: &Database,
    _lastfm: Option<&LastFmClient>,
    _seed_album_id: i64,
    _blend: RadioBlend,
    _limit: usize,
    _exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    anyhow::bail!("orchestrate_album not yet implemented")
}

/// Build a Song Radio queue from an artist (multi-seed using artist's top library tracks).
pub async fn orchestrate_artist(
    _db: &Database,
    _lastfm: Option<&LastFmClient>,
    _seed_artist_id: i64,
    _blend: RadioBlend,
    _limit: usize,
    _exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    anyhow::bail!("orchestrate_artist not yet implemented")
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Generate a session id like "rad_2a4f...".
pub(crate) fn new_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("rad_{:x}", nanos)
}

/// Normalize an artist+title pair for fuzzy dedup: lowercase, alphanumerics only.
pub(crate) fn normalize_for_dedup(artist: &str, title: &str) -> String {
    let mut s = String::with_capacity(artist.len() + title.len() + 1);
    for ch in artist
        .chars()
        .chain(std::iter::once(' '))
        .chain(title.chars())
    {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                s.push(c);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one() {
        for blend in [RadioBlend::Familiar, RadioBlend::Mixed, RadioBlend::Adventurous] {
            let (a, b, c) = blend.weights();
            assert!(
                (a + b + c - 1.0).abs() < 1e-9,
                "weights for {blend:?}: {a}+{b}+{c}"
            );
        }
    }

    #[test]
    fn dedup_normalizes_punctuation_and_case() {
        let a = normalize_for_dedup("*NSYNC", "Bye Bye Bye");
        let b = normalize_for_dedup("nsync!!!", "byeByeBye");
        assert_eq!(a, b);
    }

    #[test]
    fn dedup_normalizes_unicode_whitespace() {
        let a = normalize_for_dedup("Sigur  Rós", "Hoppípolla");
        let b = normalize_for_dedup("sigurrós", "hoppípolla");
        assert_eq!(a, b);
    }

    #[test]
    fn session_id_starts_with_rad_and_is_unique() {
        let a = new_session_id();
        // Force a tick so the nanos count differs.
        std::thread::sleep(std::time::Duration::from_nanos(1));
        let b = new_session_id();
        assert!(a.starts_with("rad_"));
        assert!(b.starts_with("rad_"));
        // Note: rare race could fail this — but `Duration::from_nanos(1)` plus the
        // syscall round-trip makes collision astronomically unlikely.
        assert_ne!(a, b, "session ids should differ across calls");
    }

    #[test]
    fn radio_blend_default_is_mixed() {
        assert_eq!(RadioBlend::default(), RadioBlend::Mixed);
    }

    #[test]
    fn radio_blend_serde_roundtrip() {
        for blend in [RadioBlend::Familiar, RadioBlend::Mixed, RadioBlend::Adventurous] {
            let s = serde_json::to_string(&blend).unwrap();
            let back: RadioBlend = serde_json::from_str(&s).unwrap();
            assert_eq!(blend, back);
        }
    }

    #[test]
    fn touch_unused_imports() {
        // Skeleton-only file — these imports become live in Task 3.
        // Touch them so the compiler doesn't whine.
        let _ = HashSet::<i64>::new();
        let _ = std::marker::PhantomData::<DiscoveryCandidateTrack>;
        let _ = std::marker::PhantomData::<LastFmSimilarTrack>;
    }
}
