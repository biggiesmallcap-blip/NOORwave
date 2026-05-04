//! Spotify (via Sportify) → TIDAL track resolver.
//!
//! Two-stage match:
//!   1. Hard reject candidates whose version markers (live / acoustic /
//!      remix / remaster / sped-up / slowed / instrumental / demo / edit /
//!      extended) disagree with the Spotify side. A studio track must not
//!      resolve to a live recording, regardless of how close the title is.
//!   2. Score remaining candidates on title token Jaccard + primary-artist
//!      token Jaccard + duration delta + explicit-flag agreement, picking
//!      the best.
//!
//! Confidence buckets (matches the design spec):
//!   ≥ 0.90  → Resolved (autoplay-eligible)
//!   0.75–0.89 → Low confidence (playable but no autoplay; UI shows badge)
//!   < 0.75  → Unresolved (kept visible, but greyed out)
//!
//! The ISRC fast path described in the design spec is intentionally deferred
//! to a follow-up: TIDAL search-result rows do not include `isrc`, so an
//! ISRC fast path needs an extra `tracks/{id}` round-trip per candidate.
//! It will land alongside the bulk resolver in phase 4 where the cost
//! amortizes across many tracks.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;

use crate::services::sportify::models::SportifyTrack;
use crate::services::tidal::client::{TidalClient, TidalSearchTrack};

pub const RESOLVED_THRESHOLD: f64 = 0.90;
pub const LOW_CONFIDENCE_THRESHOLD: f64 = 0.75;
const TIDAL_SEARCH_LIMIT: i32 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    Resolved,
    LowConfidence,
    Unresolved,
}

impl ResolutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::LowConfidence => "low_confidence",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolutionOutcome {
    pub status: ResolutionStatus,
    pub tidal_track_id: Option<i64>,
    pub confidence: f64,
    pub reason: String,
}

impl ResolutionOutcome {
    pub fn unresolved(reason: impl Into<String>) -> Self {
        Self {
            status: ResolutionStatus::Unresolved,
            tidal_track_id: None,
            confidence: 0.0,
            reason: reason.into(),
        }
    }
}

pub fn classify(score: f64) -> ResolutionStatus {
    if score >= RESOLVED_THRESHOLD {
        ResolutionStatus::Resolved
    } else if score >= LOW_CONFIDENCE_THRESHOLD {
        ResolutionStatus::LowConfidence
    } else {
        ResolutionStatus::Unresolved
    }
}

/// Resolve one Spotify track against TIDAL. Returns the best outcome — may
/// be Unresolved if no candidate passed the version guard or the best score
/// is below the low-confidence threshold.
pub async fn resolve_track(
    client: &TidalClient,
    sportify: &SportifyTrack,
) -> Result<ResolutionOutcome> {
    let title = sportify.name.as_deref().unwrap_or("").trim();
    let primary_artist = sportify.primary_artist().unwrap_or("").trim();

    if title.is_empty() || primary_artist.is_empty() {
        return Ok(ResolutionOutcome::unresolved("missing title or artist"));
    }

    let query = format!("{} {}", title, primary_artist);
    let candidates = client
        .search(&query, TIDAL_SEARCH_LIMIT)
        .await
        .context("tidal search for resolver")?;

    if candidates.is_empty() {
        return Ok(ResolutionOutcome::unresolved("no tidal candidates"));
    }

    let sp_versions = version_tags(title);
    let mut best: Option<ScoredCandidate> = None;
    let mut best_pre_guard: Option<f64> = None;

    for cand in &candidates {
        let scored = score(sportify, title, primary_artist, cand);

        // Track the highest pre-guard score so we can give a useful reason
        // when everything got rejected by the version guard.
        if best_pre_guard.map_or(true, |s| scored.score > s) {
            best_pre_guard = Some(scored.score);
        }

        let cand_versions = version_tags(&cand.title);
        if cand_versions != sp_versions {
            continue;
        }

        if best.as_ref().map_or(true, |b| scored.score > b.score) {
            best = Some(ScoredCandidate {
                tidal_id: cand.id,
                score: scored.score,
                reason: scored.reason,
            });
        }
    }

    let outcome = match best {
        None => ResolutionOutcome {
            status: ResolutionStatus::Unresolved,
            tidal_track_id: None,
            confidence: best_pre_guard.unwrap_or(0.0),
            reason: "version_mismatch".to_string(),
        },
        Some(c) => ResolutionOutcome {
            status: classify(c.score),
            tidal_track_id: Some(c.tidal_id),
            confidence: c.score,
            reason: c.reason,
        },
    };

    Ok(outcome)
}

/// Resolve a batch of Spotify tracks against TIDAL with bounded concurrency.
/// Each input keeps its position in the output: `outcomes[i]` corresponds to
/// `inputs[i]`.
pub async fn resolve_many(
    client: &TidalClient,
    inputs: &[(String, SportifyTrack)],
    concurrency: usize,
) -> Vec<(String, ResolutionOutcome)> {
    if inputs.is_empty() {
        return Vec::new();
    }
    let concurrency = concurrency.max(1);
    let sem = Arc::new(Semaphore::new(concurrency));
    let mut handles: Vec<tokio::task::JoinHandle<(usize, String, ResolutionOutcome)>> =
        Vec::with_capacity(inputs.len());
    for (idx, (spotify_id, track)) in inputs.iter().enumerate() {
        let sem = sem.clone();
        let client = client.clone();
        let id = spotify_id.clone();
        let track = track.clone();
        let handle = tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    return (idx, id, ResolutionOutcome::unresolved("semaphore_closed"));
                }
            };
            let outcome = match resolve_track(&client, &track).await {
                Ok(o) => o,
                Err(e) => ResolutionOutcome::unresolved(format!("resolver_error:{e}")),
            };
            (idx, id, outcome)
        });
        handles.push(handle);
    }
    let mut results: Vec<Option<(String, ResolutionOutcome)>> = (0..inputs.len()).map(|_| None).collect();
    for h in handles {
        match h.await {
            Ok((idx, id, outcome)) => {
                results[idx] = Some((id, outcome));
            }
            Err(e) => {
                tracing::warn!("resolve_many task join failed: {}", e);
            }
        }
    }
    results
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.unwrap_or_else(|| {
                (
                    inputs[i].0.clone(),
                    ResolutionOutcome::unresolved("task_join_failed"),
                )
            })
        })
        .collect()
}

#[derive(Debug)]
struct ScoredCandidate {
    tidal_id: i64,
    score: f64,
    reason: String,
}

#[derive(Debug, Clone)]
struct ScoreBreakdown {
    score: f64,
    reason: String,
}

fn score(
    sportify: &SportifyTrack,
    sp_title: &str,
    sp_primary_artist: &str,
    cand: &TidalSearchTrack,
) -> ScoreBreakdown {
    let sp_title_norm = normalize_title(sp_title);
    let cand_title_norm = normalize_title(&cand.title);
    let title_jaccard = token_jaccard(&sp_title_norm, &cand_title_norm);

    let cand_artist = cand.artist_name.as_deref().unwrap_or("");
    let artist_jaccard = token_jaccard(
        &normalize_artist(sp_primary_artist),
        &normalize_artist(cand_artist),
    );

    // Duration delta in milliseconds. Sportify durations are in `duration_ms`,
    // TIDAL `TidalSearchTrack.duration` is seconds.
    let duration_score = match sportify.duration_ms {
        Some(ms) if ms > 0 => {
            let cand_ms = (cand.duration as i64) * 1000;
            let diff = (ms - cand_ms).abs();
            if diff < 1500 {
                1.0
            } else if diff < 4000 {
                0.5
            } else if diff < 8000 {
                0.0
            } else {
                -1.0
            }
        }
        _ => 0.0,
    };

    let explicit_match = match sportify.explicit {
        Some(_) => 0.0, // TIDAL search rows don't expose explicit flag, skip.
        None => 0.0,
    };

    // Weighted sum, clamped into [0,1]. Title and artist are the primary
    // signals; duration acts as a real penalty so a 12" mix that shares a
    // title with the single edit doesn't autoplay over the wrong version.
    let raw = title_jaccard * 0.55 + artist_jaccard * 0.40 + duration_score * 0.10 + explicit_match;
    let score = raw.clamp(0.0, 1.0);

    let reason = format!(
        "title={:.2} artist={:.2} dur={:.2}",
        title_jaccard, artist_jaccard, duration_score
    );
    ScoreBreakdown { score, reason }
}

/// Lower-case, fold common Latin diacritics, drop parentheticals + the
/// "(feat. X)" / " - feat. X" tail, collapse whitespace.
pub fn normalize_title(s: &str) -> String {
    let lowered = s.to_lowercase();
    let no_parens = strip_brackets(&lowered);
    let no_feat = strip_feat_tail(&no_parens);
    let folded: String = no_feat.chars().map(ascii_fold_char).collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Like `normalize_title`, but also drops trailing role markers like "the band"
/// and treats `&`, `and`, `,` as separators.
pub fn normalize_artist(s: &str) -> String {
    let base = normalize_title(s);
    base.replace('&', " ")
        .replace(',', " ")
        .split_whitespace()
        .filter(|t| *t != "and" && *t != "with" && *t != "feat" && *t != "ft")
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth_paren = 0i32;
    let mut depth_brack = 0i32;
    for ch in s.chars() {
        match ch {
            '(' => depth_paren += 1,
            ')' => {
                if depth_paren > 0 {
                    depth_paren -= 1;
                }
            }
            '[' => depth_brack += 1,
            ']' => {
                if depth_brack > 0 {
                    depth_brack -= 1;
                }
            }
            _ if depth_paren == 0 && depth_brack == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

fn strip_feat_tail(s: &str) -> String {
    let markers = [" feat.", " feat ", " ft.", " ft ", " featuring "];
    for m in &markers {
        if let Some(idx) = s.find(m) {
            return s[..idx].to_string();
        }
    }
    // Common dash separator: " - feat. X"
    if let Some(idx) = s.find(" - ") {
        let tail = &s[idx + 3..];
        if tail.starts_with("feat") || tail.starts_with("ft") {
            return s[..idx].to_string();
        }
    }
    s.to_string()
}

fn ascii_fold_char(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'č' | 'ć' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ě' | 'ę' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'š' | 'ś' => 's',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ž' | 'ź' | 'ż' => 'z',
        '\u{2018}' | '\u{2019}' | '\u{2032}' => '\'',
        '\u{201c}' | '\u{201d}' => '"',
        '\u{2013}' | '\u{2014}' => '-',
        _ => c,
    }
}

fn tokens(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn token_jaccard(a: &str, b: &str) -> f64 {
    let ta = tokens(a);
    let tb = tokens(b);
    if ta.is_empty() && tb.is_empty() {
        return 0.0;
    }
    let intersection = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    if union == 0.0 { 0.0 } else { intersection / union }
}

/// Detect version markers in a track title. Two tracks are eligible for
/// matching only when their tag sets are equal (a studio track has an empty
/// set; a live track has `live`; a 2011 remaster has `remaster`; etc.).
pub fn version_tags(title: &str) -> HashSet<&'static str> {
    let t = title.to_lowercase();
    let mut tags: HashSet<&'static str> = HashSet::new();
    if contains_word(&t, "live") || t.contains("live at ") || t.contains("live from ") {
        tags.insert("live");
    }
    if contains_word(&t, "acoustic") {
        tags.insert("acoustic");
    }
    if contains_word(&t, "remix")
        || t.contains(" mix)")
        || t.contains(" mix]")
        || t.contains("- mix")
    {
        tags.insert("remix");
    }
    if contains_word(&t, "remaster") || t.contains("remastered") {
        tags.insert("remaster");
    }
    if contains_word(&t, "sped") || t.contains("sped up") || t.contains("sped-up") {
        tags.insert("sped_up");
    }
    if contains_word(&t, "slowed") {
        tags.insert("slowed");
    }
    if contains_word(&t, "instrumental") {
        tags.insert("instrumental");
    }
    if contains_word(&t, "demo") {
        tags.insert("demo");
    }
    if t.contains("radio edit") || contains_word(&t, "edit") {
        tags.insert("edit");
    }
    if contains_word(&t, "extended") {
        tags.insert("extended");
    }
    if contains_word(&t, "karaoke") {
        tags.insert("karaoke");
    }
    tags
}

fn contains_word(haystack: &str, word: &str) -> bool {
    let mut start = 0usize;
    while let Some(idx) = haystack[start..].find(word) {
        let abs = start + idx;
        let before_ok = abs == 0
            || !haystack
                .as_bytes()
                .get(abs - 1)
                .map(|b| b.is_ascii_alphanumeric())
                .unwrap_or(false);
        let end = abs + word.len();
        let after_ok = end == haystack.len()
            || !haystack
                .as_bytes()
                .get(end)
                .map(|b| b.is_ascii_alphanumeric())
                .unwrap_or(false);
        if before_ok && after_ok {
            return true;
        }
        start = abs + word.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sportify::models::{SportifyArtistRef, SportifyTrack};

    fn sp(title: &str, artist: &str, dur_ms: Option<i64>) -> SportifyTrack {
        SportifyTrack {
            name: Some(title.to_string()),
            artists: vec![SportifyArtistRef {
                name: Some(artist.to_string()),
                ..Default::default()
            }],
            duration_ms: dur_ms,
            ..Default::default()
        }
    }

    fn cand(id: i64, title: &str, artist: &str, dur_secs: i64) -> TidalSearchTrack {
        TidalSearchTrack {
            id,
            title: title.to_string(),
            duration: dur_secs,
            artist_id: None,
            artist_name: Some(artist.to_string()),
            artist_picture: None,
            album_title: None,
            album_id: None,
            artwork_url: None,
            audio_quality: None,
            stream_ready: Some(true),
            extra: Default::default(),
        }
    }

    #[test]
    fn normalize_strips_parens_and_feat() {
        assert_eq!(
            normalize_title("Hey Ya! (feat. André 3000)"),
            "hey ya!".to_string()
        );
        assert_eq!(
            normalize_title("Get Lucky - Radio Edit"),
            "get lucky - radio edit".to_string()
        );
        assert_eq!(
            normalize_title("Café del Mar"),
            "cafe del mar".to_string()
        );
    }

    #[test]
    fn version_tags_distinguishes_live_studio() {
        assert!(version_tags("Hey Ya!").is_empty());
        assert!(version_tags("Hey Ya! - Live at Sydney").contains("live"));
        assert!(version_tags("Wonderwall - Acoustic").contains("acoustic"));
        assert!(version_tags("Bohemian Rhapsody - 2011 Remaster").contains("remaster"));
        assert!(version_tags("Animals - Henrik Schwarz Remix").contains("remix"));
    }

    #[test]
    fn version_tags_word_boundary_ignores_substrings() {
        // "live" inside "alive" should NOT trigger the live tag.
        assert!(!version_tags("Alive").contains("live"));
        assert!(!version_tags("Believe").contains("live"));
    }

    #[test]
    fn token_jaccard_basic() {
        assert!((token_jaccard("hey ya", "hey ya") - 1.0).abs() < 1e-9);
        assert!((token_jaccard("hey ya", "ya hey") - 1.0).abs() < 1e-9);
        assert!(token_jaccard("hey ya", "completely different") < 0.1);
    }

    #[test]
    fn score_high_for_exact_match() {
        let s = sp("Get Lucky", "Daft Punk", Some(248_000));
        let c = cand(123, "Get Lucky", "Daft Punk", 248);
        let breakdown = score(&s, "Get Lucky", "Daft Punk", &c);
        assert!(
            breakdown.score >= 0.90,
            "expected >= 0.90 but got {} ({})",
            breakdown.score,
            breakdown.reason
        );
    }

    #[test]
    fn score_low_for_artist_mismatch() {
        let s = sp("Get Lucky", "Daft Punk", Some(248_000));
        let c = cand(123, "Get Lucky", "Some Cover Band", 250);
        let breakdown = score(&s, "Get Lucky", "Daft Punk", &c);
        assert!(
            breakdown.score < LOW_CONFIDENCE_THRESHOLD,
            "expected < {} but got {} ({})",
            LOW_CONFIDENCE_THRESHOLD,
            breakdown.score,
            breakdown.reason
        );
    }

    #[test]
    fn score_in_low_confidence_band_when_duration_off() {
        // Same title and artist but a very different duration (single edit
        // vs 12" mix). Should land in the playable-but-no-autoplay band.
        let s = sp("Music Sounds Better With You", "Stardust", Some(213_000));
        let c = cand(
            123,
            "Music Sounds Better With You",
            "Stardust",
            420, // 7-minute version
        );
        let breakdown = score(&s, "Music Sounds Better With You", "Stardust", &c);
        assert!(breakdown.score < RESOLVED_THRESHOLD);
        assert!(breakdown.score >= LOW_CONFIDENCE_THRESHOLD - 0.05);
    }

    #[test]
    fn classify_buckets() {
        assert_eq!(classify(0.95), ResolutionStatus::Resolved);
        assert_eq!(classify(0.80), ResolutionStatus::LowConfidence);
        assert_eq!(classify(0.50), ResolutionStatus::Unresolved);
    }

    #[tokio::test]
    async fn resolve_many_empty_input_returns_empty() {
        // Smoke test: with no inputs the bulk path doesn't even need a working
        // TIDAL client, so we can exercise it without mocks.
        let client = TidalClient::new("dummy".into(), "US".into());
        let outcomes = resolve_many(&client, &[], 4).await;
        assert!(outcomes.is_empty());
    }
}
