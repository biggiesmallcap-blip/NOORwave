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
use crate::metadata::lastfm::LastFmClient;
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

// ─── Public orchestrators ────────────────────────────────────────────────────

/// Build a Song Radio queue seeded from a single library track.
pub async fn orchestrate_song(
    db: &Database,
    lastfm: Option<&LastFmClient>,
    seed_track_id: i64,
    blend: RadioBlend,
    limit: usize,
    exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    let exclude_set: HashSet<i64> = exclude_track_ids.iter().copied().collect();
    let id = seed_track_id;
    let seed_meta = db
        .with_conn(move |conn| crate::db::queries::load_external_seed_from_track(conn, id))?
        .ok_or_else(|| anyhow::anyhow!("seed track not found: {seed_track_id}"))?;

    let seed_for_session = RadioSeed {
        kind: "track",
        track_id: Some(seed_track_id),
        album_id: None,
        artist_id: None,
        title: seed_meta.title.clone(),
        artist_name: seed_meta.artist_name.clone(),
    };

    let (lib_w, lfm_w, _eng_w) = blend.weights();
    let target_per_source = |w: f64| ((limit as f64 * w * 1.5).ceil() as usize).max(1);
    let lib_target = target_per_source(lib_w);
    let lfm_target = target_per_source(lfm_w);

    // ── Library source ────────────────────────────────────────────────────────
    let library_results: Vec<RadioCandidate> = {
        let mut excl: Vec<i64> = exclude_set.iter().copied().collect();
        excl.push(seed_track_id);
        let creativity = match blend {
            RadioBlend::Familiar => 0.15,
            RadioBlend::Mixed => 0.30,
            RadioBlend::Adventurous => 0.50,
        };
        crate::services::learning::radio_from_neighbors(db, seed_track_id, &excl, lib_target as i64, creativity)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|n| {
                let reason = if !n.reason_tags.is_empty() {
                    format!("library · {} (sim {:.2})", n.reason_tags[0], n.similarity_score)
                } else {
                    format!("library · embedding similarity {:.2}", n.similarity_score)
                };
                RadioCandidate {
                    track_id: n.track_id,
                    tidal_track_id: None,
                    title: n.title,
                    artist_name: n.artist_name.unwrap_or_default(),
                    album_title: n.album_title,
                    artwork_url: n.artwork_url,
                    duration_ms: n.duration_ms,
                    isrc: None,
                    is_in_library: true,
                    source: RadioSource::Library,
                    reason,
                    similarity_score: n.similarity_score,
                }
            })
            .collect()
    };

    // ── Last.fm source ────────────────────────────────────────────────────────
    let lastfm_results: Vec<RadioCandidate> =
        if let (Some(client), Some(artist)) = (lastfm, seed_meta.artist_name.as_deref()) {
            client
                .track_get_similar(artist, &seed_meta.title, lfm_target.max(20))
                .await
                .unwrap_or_default()
                .into_iter()
                .take(lfm_target * 2)
                .map(|hit| RadioCandidate {
                    track_id: 0,
                    tidal_track_id: None,
                    title: hit.title,
                    artist_name: hit.artist,
                    album_title: None,
                    artwork_url: None,
                    duration_ms: None,
                    isrc: None,
                    is_in_library: false,
                    source: RadioSource::Lastfm,
                    reason: format!("Last.fm match {:.2}", hit.match_score),
                    similarity_score: hit.match_score.clamp(0.0, 1.0),
                })
                .collect()
        } else {
            Vec::new()
        };

    // ── Combine + blend ───────────────────────────────────────────────────────
    let combined = combine_with_dedup(library_results, lastfm_results, Vec::new());
    let ordered = blend_interleave(combined, blend, limit);

    Ok(RadioQueue {
        session_id: new_session_id(),
        blend_used: blend,
        seed: seed_for_session,
        tracks: ordered,
    })
}

/// Build a Song Radio queue from an album (multi-seed using album tracks).
pub async fn orchestrate_album(
    db: &Database,
    lastfm: Option<&LastFmClient>,
    seed_album_id: i64,
    blend: RadioBlend,
    limit: usize,
    exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    let album_id = seed_album_id;
    let (seed_track_ids, album_title, album_artist) = db.with_conn(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT t.id FROM tracks t WHERE t.album_id = ?1 ORDER BY t.disc_number ASC, t.track_number ASC LIMIT 3",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(rusqlite::params![album_id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let meta = conn
            .query_row(
                "SELECT al.title, ar.name FROM albums al LEFT JOIN artists ar ON al.artist_id = ar.id WHERE al.id = ?1",
                rusqlite::params![album_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .ok();
        let title = meta.as_ref().map(|m| m.0.clone());
        let artist = meta.and_then(|m| m.1);
        Ok((ids, title, artist))
    })?;

    if seed_track_ids.is_empty() {
        anyhow::bail!("album has no tracks: {seed_album_id}");
    }

    let per_seed_limit = (limit / seed_track_ids.len()).max(8);
    let mut all_candidates: Vec<RadioCandidate> = Vec::new();
    for tid in &seed_track_ids {
        if let Ok(q) = orchestrate_song(db, lastfm, *tid, blend, per_seed_limit, exclude_track_ids).await {
            all_candidates.extend(q.tracks);
        }
    }
    let combined = combine_with_dedup(all_candidates, Vec::new(), Vec::new());
    let ordered = blend_interleave(combined, blend, limit);

    Ok(RadioQueue {
        session_id: new_session_id(),
        blend_used: blend,
        seed: RadioSeed {
            kind: "album",
            track_id: None,
            album_id: Some(seed_album_id),
            artist_id: None,
            title: album_title.unwrap_or_else(|| format!("album {seed_album_id}")),
            artist_name: album_artist,
        },
        tracks: ordered,
    })
}

/// Build a Song Radio queue from an artist (multi-seed using artist's top library tracks).
pub async fn orchestrate_artist(
    db: &Database,
    lastfm: Option<&LastFmClient>,
    seed_artist_id: i64,
    blend: RadioBlend,
    limit: usize,
    exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    let artist_id = seed_artist_id;
    let (seed_track_ids, artist_name) = db.with_conn(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM tracks WHERE artist_id = ?1 ORDER BY play_count DESC, last_played_at DESC LIMIT 3",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(rusqlite::params![artist_id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let name: Option<String> = conn
            .query_row("SELECT name FROM artists WHERE id = ?1", rusqlite::params![artist_id], |row| row.get(0))
            .ok();
        Ok((ids, name))
    })?;

    if seed_track_ids.is_empty() {
        anyhow::bail!("artist has no library tracks: {seed_artist_id}");
    }

    let per_seed_limit = (limit / seed_track_ids.len()).max(8);
    let mut all_candidates: Vec<RadioCandidate> = Vec::new();
    for tid in &seed_track_ids {
        if let Ok(q) = orchestrate_song(db, lastfm, *tid, blend, per_seed_limit, exclude_track_ids).await {
            all_candidates.extend(q.tracks);
        }
    }
    let combined = combine_with_dedup(all_candidates, Vec::new(), Vec::new());
    let ordered = blend_interleave(combined, blend, limit);

    Ok(RadioQueue {
        session_id: new_session_id(),
        blend_used: blend,
        seed: RadioSeed {
            kind: "artist",
            track_id: None,
            album_id: None,
            artist_id: Some(seed_artist_id),
            title: artist_name.clone().unwrap_or_else(|| format!("artist {seed_artist_id}")),
            artist_name,
        },
        tracks: ordered,
    })
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

fn combine_with_dedup(
    library: Vec<RadioCandidate>,
    lastfm: Vec<RadioCandidate>,
    engine: Vec<RadioCandidate>,
) -> Vec<RadioCandidate> {
    let mut seen_norm: HashSet<String> = HashSet::new();
    let mut out: Vec<RadioCandidate> = Vec::new();
    for source_list in [library, lastfm, engine] {
        for cand in source_list {
            let norm = normalize_for_dedup(&cand.artist_name, &cand.title);
            if norm.is_empty() || !seen_norm.insert(norm) {
                continue;
            }
            out.push(cand);
        }
    }
    out
}

fn blend_interleave(candidates: Vec<RadioCandidate>, blend: RadioBlend, limit: usize) -> Vec<RadioCandidate> {
    let (lib_w, lfm_w, eng_w) = blend.weights();
    let mut by_source: std::collections::HashMap<RadioSource, Vec<RadioCandidate>> =
        std::collections::HashMap::new();
    for c in candidates {
        by_source.entry(c.source).or_default().push(c);
    }
    for v in by_source.values_mut() {
        v.sort_by(|a, b| b.similarity_score.partial_cmp(&a.similarity_score).unwrap_or(std::cmp::Ordering::Equal));
    }

    let lib_avail = by_source.get(&RadioSource::Library).map_or(0, |v| v.len());
    let lfm_avail = by_source.get(&RadioSource::Lastfm).map_or(0, |v| v.len());
    let eng_avail = by_source.get(&RadioSource::Engine).map_or(0, |v| v.len());
    let lib_take = ((limit as f64 * lib_w).round() as usize).min(lib_avail);
    let lfm_take = ((limit as f64 * lfm_w).round() as usize).min(lfm_avail);
    let eng_take = ((limit as f64 * eng_w).round() as usize).min(eng_avail);

    let mut lib_iter = by_source.remove(&RadioSource::Library).unwrap_or_default().into_iter().take(lib_take);
    let mut lfm_iter = by_source.remove(&RadioSource::Lastfm).unwrap_or_default().into_iter().take(lfm_take);
    let mut eng_iter = by_source.remove(&RadioSource::Engine).unwrap_or_default().into_iter().take(eng_take);

    let mut out = Vec::with_capacity(limit);
    let mut lib_done = 0usize;
    let mut lfm_done = 0usize;
    let mut eng_done = 0usize;

    while out.len() < limit {
        let lib_behind = (lib_take as f64 - lib_done as f64) / lib_w.max(0.01);
        let lfm_behind = (lfm_take as f64 - lfm_done as f64) / lfm_w.max(0.01);
        let eng_behind = (eng_take as f64 - eng_done as f64) / eng_w.max(0.01);

        let pick = if lib_behind >= lfm_behind && lib_behind >= eng_behind {
            lib_iter.next().map(|c| { lib_done += 1; c })
        } else if lfm_behind >= eng_behind {
            lfm_iter.next().map(|c| { lfm_done += 1; c })
        } else {
            eng_iter.next().map(|c| { eng_done += 1; c })
        };

        match pick {
            Some(c) => out.push(c),
            None => {
                if let Some(c) = lib_iter.next() { lib_done += 1; out.push(c); }
                else if let Some(c) = lfm_iter.next() { lfm_done += 1; out.push(c); }
                else if let Some(c) = eng_iter.next() { eng_done += 1; out.push(c); }
                else { break; }
            }
        }
    }
    out
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
}
