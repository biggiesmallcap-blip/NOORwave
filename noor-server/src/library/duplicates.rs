use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::db::models::Track;
use crate::playback::player::ReconcileOutcome;

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub id: i64,
    pub status: String,
    /// One of: "exact_duplicate", "cross_album_reissue", "remaster",
    /// "alt_version", "quality_variant".
    pub relationship: String,
    /// Distinct-value summaries across members for each dimension that varies.
    pub differences: Vec<GroupDifference>,
    pub members: Vec<DuplicateMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupDifference {
    /// "version_marker" | "year" | "album" | "quality" | "sample_rate" | "source"
    pub kind: String,
    /// Distinct values across members, formatted for display.
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateMember {
    pub track: Track,
    pub is_preferred: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanStats {
    pub groups_found: usize,
    pub tracks_affected: usize,
    pub isrc_matches: usize,
    pub title_matches: usize,
}

// ── Variant marker vocabulary ─────────────────────────────────────────────────

/// Performance / cut markers — different recording.
const ALT_VERSION_TOKENS: &[&str] = &[
    "remix",
    "live",
    "acoustic",
    "instrumental",
    "dub",
    "edit",
    "demo",
    "vip",
    "rework",
];

/// Phrase-level alt-version markers detected via substring (post-normalize).
const ALT_VERSION_PHRASES: &[&str] = &[
    "radio edit",
    "radio version",
    "single version",
    "album version",
    "original mix",
];

/// Master / mix markers — same song, different master.
const MASTER_TOKENS: &[&str] = &[
    "remaster",
    "remastered",
    "deluxe",
    "anniversary",
    "expanded",
    "extended",
    "bonus",
    "mono",
    "stereo",
];

/// Album-year drift threshold: pairs of tracks whose album years differ by at
/// least this many years are treated as a master mismatch even when no explicit
/// remaster token is present.
const REMASTER_YEAR_DRIFT: i32 = 5;

// ── Normalisation helpers ─────────────────────────────────────────────────────

/// Lowercase, strip non-alphanumeric (keep spaces), collapse whitespace.
pub(crate) fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let filtered: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    filtered.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone)]
struct MatchRow {
    id: i64,
    norm_title: String,
    canonical_title: String,
    base_title: String,
    norm_artist: String,
    artist_tokens: Vec<String>,
    duration_ms: i64,
    isrc: Option<String>,
    album_id: Option<i64>,
    album_title: Option<String>,
    album_year: Option<i32>,
    best_quality: Option<String>,
    sample_rate: Option<i64>,
    file_path: Option<String>,
    source: String,
    alt_markers: Vec<&'static str>,
    master_markers: Vec<&'static str>,
    fidelity_score: i32,
    is_favorite: bool,
}

#[allow(clippy::too_many_arguments)]
fn build_match_row(
    id: i64,
    title: &str,
    artist: &str,
    duration_ms: Option<i64>,
    isrc: Option<String>,
    album_id: Option<i64>,
    album_title: Option<String>,
    album_year: Option<i32>,
    best_quality: Option<String>,
    sample_rate: Option<i64>,
    file_path: Option<String>,
    source: String,
    fidelity_score: i32,
    is_favorite: bool,
) -> MatchRow {
    let norm_artist = normalize(artist);
    let artist_tokens = if norm_artist.is_empty() {
        Vec::new()
    } else {
        norm_artist
            .split_whitespace()
            .map(|token| token.to_string())
            .collect()
    };

    let (alt_markers, master_markers) = extract_variant_markers(title);

    MatchRow {
        id,
        norm_title: normalize(title),
        canonical_title: canonicalize_title(title),
        base_title: base_title(title),
        norm_artist,
        artist_tokens,
        duration_ms: duration_ms.unwrap_or(0),
        isrc,
        album_id,
        album_title,
        album_year,
        best_quality,
        sample_rate,
        file_path,
        source,
        alt_markers,
        master_markers,
        fidelity_score,
        is_favorite,
    }
}

/// Strips ignorable bracketed/suffix segments. Used for fuzzy similarity.
fn canonicalize_title(title: &str) -> String {
    let mut canonical = strip_bracketed_segments(title.trim(), is_ignorable_title_segment);

    loop {
        let next = strip_suffix(&canonical, is_ignorable_title_segment);
        if next == canonical {
            break;
        }
        canonical = next;
    }

    normalize(&canonical)
}

/// Strips ignorable AND variant-marker segments. Used as the bucketing key so
/// "Song" and "Song (Remix)" land in the same bucket and the classifier can
/// then label the relationship. The liked-videos matcher reuses it for the same
/// reason in reverse: "Song (Live)" is exactly the video it wants for "Song".
pub(crate) fn base_title(title: &str) -> String {
    let predicate = |seg: &str| is_ignorable_title_segment(seg) || segment_carries_variant(seg);

    let mut output = strip_bracketed_segments(title.trim(), predicate);

    loop {
        let next = strip_suffix(&output, predicate);
        if next == output {
            break;
        }
        output = next;
    }

    normalize(&output)
}

fn strip_bracketed_segments<F>(input: &str, should_strip: F) -> String
where
    F: Fn(&str) -> bool,
{
    let chars: Vec<char> = input.chars().collect();
    let mut idx = 0usize;
    let mut output = String::new();

    while idx < chars.len() {
        let open = chars[idx];
        let close = match open {
            '(' => ')',
            '[' => ']',
            _ => {
                output.push(open);
                idx += 1;
                continue;
            }
        };

        let mut end = idx + 1;
        let mut depth = 1;
        while end < chars.len() {
            if chars[end] == open {
                depth += 1;
            } else if chars[end] == close {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            end += 1;
        }

        if depth == 0 {
            let segment: String = chars[(idx + 1)..end].iter().collect();
            if should_strip(&segment) {
                idx = end + 1;
                continue;
            }
        }

        output.push(open);
        idx += 1;
    }

    output
}

fn strip_suffix<F>(input: &str, should_strip: F) -> String
where
    F: Fn(&str) -> bool,
{
    for separator in [" - ", " – ", " — "] {
        if let Some((prefix, suffix)) = input.rsplit_once(separator)
            && should_strip(suffix)
        {
            return prefix.trim().to_string();
        }
    }

    input.trim().to_string()
}

fn is_ignorable_title_segment(segment: &str) -> bool {
    let normalized = normalize(segment);
    matches!(
        normalized.as_str(),
        "feat" | "ft" | "featuring" | "original" | "explicit" | "clean"
    ) || normalized.starts_with("feat ")
        || normalized.starts_with("ft ")
        || normalized.starts_with("featuring ")
}

/// True if `segment` contains any alt-version token, master token, or alt-version phrase.
fn segment_carries_variant(segment: &str) -> bool {
    let normalized = normalize(segment);
    if normalized.is_empty() {
        return false;
    }

    for phrase in ALT_VERSION_PHRASES {
        if normalized.contains(phrase) {
            return true;
        }
    }

    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    for marker in ALT_VERSION_TOKENS.iter().chain(MASTER_TOKENS.iter()) {
        if tokens.iter().any(|t| t == marker) {
            return true;
        }
    }

    false
}

/// Returns (alt_version_markers, master_markers) found anywhere in the title.
fn extract_variant_markers(title: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let normalized = normalize(title);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let mut alt: Vec<&'static str> = Vec::new();
    let mut master: Vec<&'static str> = Vec::new();

    for &marker in ALT_VERSION_TOKENS {
        if tokens.iter().any(|t| *t == marker) && !alt.contains(&marker) {
            alt.push(marker);
        }
    }
    for &phrase in ALT_VERSION_PHRASES {
        if normalized.contains(phrase) && !alt.contains(&phrase) {
            alt.push(phrase);
        }
    }
    for &marker in MASTER_TOKENS {
        if tokens.iter().any(|t| *t == marker) && !master.contains(&marker) {
            master.push(marker);
        }
    }
    (alt, master)
}

fn titles_compatible(left: &MatchRow, right: &MatchRow, via_isrc: bool) -> bool {
    if left.norm_title == right.norm_title || left.canonical_title == right.canonical_title {
        return true;
    }

    if left.canonical_title.is_empty() || right.canonical_title.is_empty() {
        return false;
    }

    let similarity = strsim::jaro_winkler(&left.canonical_title, &right.canonical_title);
    if via_isrc {
        similarity >= 0.93
    } else {
        similarity >= 0.985
    }
}

fn artists_compatible(left: &MatchRow, right: &MatchRow) -> bool {
    if left.norm_artist == right.norm_artist {
        return true;
    }

    let (shorter, longer) = if left.artist_tokens.len() <= right.artist_tokens.len() {
        (&left.artist_tokens, &right.artist_tokens)
    } else {
        (&right.artist_tokens, &left.artist_tokens)
    };

    if shorter.len() >= 2
        && shorter
            .iter()
            .all(|token| longer.iter().any(|candidate| candidate == token))
    {
        return true;
    }

    strsim::jaro_winkler(&left.norm_artist, &right.norm_artist) >= 0.92
}

fn durations_compatible(
    left_ms: i64,
    right_ms: i64,
    max_diff_ms: i64,
    max_diff_percent: i64,
) -> bool {
    if left_ms <= 0 || right_ms <= 0 {
        return false;
    }

    let diff = (left_ms - right_ms).abs();
    let longer = left_ms.max(right_ms);
    diff <= max_diff_ms && diff * 100 <= longer * max_diff_percent
}

fn rows_match(left: &MatchRow, right: &MatchRow, via_isrc: bool) -> bool {
    if !artists_compatible(left, right) {
        return false;
    }

    // Title compatibility: either the canonical (ignorable-stripped) titles
    // are similar enough OR the base (variant-stripped) titles match exactly.
    // The base-title path is what lets "Song" match "Song (Remix)".
    let base_match = !left.base_title.is_empty()
        && !right.base_title.is_empty()
        && left.base_title == right.base_title;

    if !base_match && !titles_compatible(left, right, via_isrc) {
        return false;
    }

    if via_isrc {
        durations_compatible(left.duration_ms, right.duration_ms, 15_000, 8)
    } else {
        // Tightened from ±3000ms/3% — alt versions and remasters often share a
        // base title, so we lean on duration to distinguish unrelated tracks.
        durations_compatible(left.duration_ms, right.duration_ms, 2_000, 2)
    }
}

// ── Import-time dedupe ───────────────────────────────────────────────────────

/// An incoming TIDAL track about to be inserted by sync or enrichment.
pub struct IncomingTrack<'a> {
    pub tidal_id: i64,
    pub title: &'a str,
    pub artist_name: &'a str,
    pub isrc: Option<&'a str>,
    pub duration_ms: i64,
}

/// An existing tracks row that could be the same recording as an incoming one.
pub struct ExistingCandidate {
    pub track_id: i64,
    pub tidal_id: Option<i64>,
    pub title: String,
    pub artist_name: String,
    pub isrc: Option<String>,
    pub duration_ms: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ImportDecision {
    /// No same-recording match: run the normal `ON CONFLICT(tidal_id)` upsert.
    Insert,
    /// The same recording already exists under a DIFFERENT tidal_id. The
    /// caller decides the side effect (liked sync transfers the like; album
    /// enrichment just skips).
    SkipDuplicate {
        existing_track_id: i64,
        existing_tidal_id: Option<i64>,
    },
}

fn normalize_isrc(isrc: &str) -> Option<String> {
    let trimmed = isrc.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_uppercase())
    }
}

/// Sorted alt-version fingerprint ("remix"/"live"/"acoustic"/... tokens and
/// phrases) for a title. Two titles are only the same recording when these
/// agree - that is what keeps distinct versions alive while single/album/
/// deluxe/compilation copies collapse. Master markers (remaster/extended/...)
/// are deliberately NOT part of the fingerprint: a remaster is the same
/// recording, and an extended cut is separated by the duration gate instead.
fn alt_fingerprint(row: &MatchRow) -> Vec<&'static str> {
    let mut v = row.alt_markers.clone();
    v.sort_unstable();
    v
}

/// Decide whether an incoming track is a new recording or a copy of an
/// existing row. Pure function over pre-fetched candidates; both sync phases
/// and album enrichment share it. Only literal same-recording copies are ever
/// skipped - there is deliberately no "skip variant" outcome.
pub fn decide_import(incoming: &IncomingTrack, candidates: &[ExistingCandidate]) -> ImportDecision {
    // Rule 0: same tidal_id = same row, not a duplicate. The normal upsert
    // must run so re-syncs keep refreshing title/quality/fidelity/date_added.
    if candidates
        .iter()
        .any(|c| c.tidal_id == Some(incoming.tidal_id))
    {
        return ImportDecision::Insert;
    }

    let inc_isrc = incoming.isrc.and_then(normalize_isrc);
    let inc_row = build_match_row(
        0,
        incoming.title,
        incoming.artist_name,
        Some(incoming.duration_ms),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        String::new(),
        0,
        false,
    );
    let inc_alt = alt_fingerprint(&inc_row);

    for cand in candidates {
        // ISRC path: same registered recording. The duration guard defends
        // against the known ISRC-reuse-with-different-length upstream bug.
        let cand_isrc = cand.isrc.as_deref().and_then(normalize_isrc);
        if let (Some(a), Some(b)) = (inc_isrc.as_deref(), cand_isrc.as_deref())
            && a == b
            && durations_compatible(incoming.duration_ms, cand.duration_ms, 15_000, 8)
        {
            return ImportDecision::SkipDuplicate {
                existing_track_id: cand.track_id,
                existing_tidal_id: cand.tidal_id,
            };
        }

        // Title path: fuzzy same-recording match, gated on matching variant
        // fingerprints so "Song" never swallows "Song (Live)".
        let cand_row = build_match_row(
            cand.track_id,
            &cand.title,
            &cand.artist_name,
            Some(cand.duration_ms),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            String::new(),
            0,
            false,
        );
        if alt_fingerprint(&cand_row) == inc_alt && rows_match(&inc_row, &cand_row, false) {
            return ImportDecision::SkipDuplicate {
                existing_track_id: cand.track_id,
                existing_tidal_id: cand.tidal_id,
            };
        }
    }

    ImportDecision::Insert
}

const IMPORT_CANDIDATE_SELECT: &str =
    "SELECT t.id, t.tidal_id, t.title, COALESCE(a.name, ''), t.isrc, COALESCE(t.duration_ms, 0)
     FROM tracks t
     LEFT JOIN artists a ON t.artist_id = a.id";

fn collect_import_candidates(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
    seen: &mut std::collections::HashSet<i64>,
    out: &mut Vec<ExistingCandidate>,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok(ExistingCandidate {
            track_id: row.get(0)?,
            tidal_id: row.get(1)?,
            title: row.get(2)?,
            artist_name: row.get(3)?,
            isrc: row.get(4)?,
            duration_ms: row.get(5)?,
        })
    })?;
    for row in rows {
        let candidate = row?;
        if seen.insert(candidate.track_id) {
            out.push(candidate);
        }
    }
    Ok(())
}

/// Fetch the existing rows that could be the same recording as an incoming
/// TIDAL track: its own tidal_id row (so [`decide_import`]'s rule 0 sees
/// re-syncs), same-ISRC rows (cross-artist, for compilations), and
/// same-artist rows inside the widest duration tolerance decide_import uses.
pub fn fetch_import_candidates(
    conn: &Connection,
    tidal_id: i64,
    artist_tidal_id: i64,
    isrc: Option<&str>,
    duration_ms: i64,
) -> Result<Vec<ExistingCandidate>> {
    let mut out: Vec<ExistingCandidate> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();

    collect_import_candidates(
        conn,
        &format!("{IMPORT_CANDIDATE_SELECT} WHERE t.tidal_id = ?1"),
        &[&tidal_id],
        &mut seen,
        &mut out,
    )?;

    if let Some(isrc) = isrc.map(str::trim).filter(|s| !s.is_empty()) {
        collect_import_candidates(
            conn,
            &format!("{IMPORT_CANDIDATE_SELECT} WHERE t.isrc = ?1 AND t.isrc != ''"),
            &[&isrc],
            &mut seen,
            &mut out,
        )?;
    }

    collect_import_candidates(
        conn,
        &format!(
            "{IMPORT_CANDIDATE_SELECT}
             WHERE t.artist_id = (SELECT id FROM artists WHERE tidal_id = ?1)
               AND ABS(COALESCE(t.duration_ms, 0) - ?2) <= 15000"
        ),
        &[&artist_tidal_id, &duration_ms],
        &mut seen,
        &mut out,
    )?;

    Ok(out)
}

// ── Classifier ────────────────────────────────────────────────────────────────

/// Maps a `best_quality` string to a coarse fidelity tier.
/// Mirrors the frontend `qualityLabel` mapping in routes/duplicates/+page.svelte.
fn quality_tier(q: Option<&str>) -> u8 {
    match q {
        Some("HI_RES_LOSSLESS") | Some("HI_RES") => 3,
        Some("LOSSLESS") => 2,
        Some(s) if !s.is_empty() => 1,
        _ => 0,
    }
}

/// Friendly label for a quality string, used in difference chips.
fn quality_label(q: Option<&str>) -> String {
    match q {
        Some("HI_RES_LOSSLESS") | Some("HI_RES") => "Hi-Res".to_string(),
        Some("LOSSLESS") => "Lossless".to_string(),
        Some("HIGH") => "High".to_string(),
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "Unknown".to_string(),
    }
}

fn distinct<T: Eq + Clone>(values: &[T]) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for v in values {
        if !out.iter().any(|x| x == v) {
            out.push(v.clone());
        }
    }
    out
}

/// Decide a group's relationship and the per-dimension differences from the
/// rows that compose it.
fn classify(rows: &[&MatchRow]) -> (String, Vec<GroupDifference>) {
    // Per-row marker fingerprints — used to detect *mismatches* (some members
    // carry the marker, others don't). A group where every row carries
    // "remastered" identically isn't a remaster relationship.
    let alt_fingerprints: Vec<Vec<&'static str>> = rows
        .iter()
        .map(|r| {
            let mut v = r.alt_markers.clone();
            v.sort();
            v
        })
        .collect();
    let master_fingerprints: Vec<Vec<&'static str>> = rows
        .iter()
        .map(|r| {
            let mut v = r.master_markers.clone();
            v.sort();
            v
        })
        .collect();

    let alt_mismatch = distinct(&alt_fingerprints).len() > 1;
    let master_mismatch = distinct(&master_fingerprints).len() > 1;

    let years: Vec<i32> = rows.iter().filter_map(|r| r.album_year).collect();
    let year_drift = if years.len() >= 2 {
        let max = *years.iter().max().unwrap();
        let min = *years.iter().min().unwrap();
        (max - min).abs() >= REMASTER_YEAR_DRIFT
    } else {
        false
    };

    let quality_tiers: Vec<u8> = rows
        .iter()
        .map(|r| quality_tier(r.best_quality.as_deref()))
        .filter(|t| *t > 0)
        .collect();
    let distinct_quality = distinct(&quality_tiers);

    // Sample rate is only treated as a divergence when both sides are local
    // files. Streaming rows often lack an accurate sample_rate, and we don't
    // want to call those quality_variants.
    let local_sample_rates: Vec<i64> = rows
        .iter()
        .filter_map(|r| {
            if r.file_path.as_deref().is_some_and(|p| !p.is_empty()) {
                r.sample_rate
            } else {
                None
            }
        })
        .collect();
    let distinct_local_rates = distinct(&local_sample_rates);

    let album_ids: Vec<i64> = rows.iter().filter_map(|r| r.album_id).collect();
    let distinct_albums = distinct(&album_ids);

    let isrcs: Vec<String> = rows
        .iter()
        .filter_map(|r| r.isrc.as_ref().filter(|s| !s.is_empty()).cloned())
        .collect();
    let shared_isrc = !isrcs.is_empty() && distinct(&isrcs).len() == 1;

    // Decision tree — most specific first.
    let relationship = if alt_mismatch {
        "alt_version"
    } else if master_mismatch || year_drift {
        "remaster"
    } else if distinct_quality.len() > 1 || distinct_local_rates.len() > 1 {
        "quality_variant"
    } else if shared_isrc && distinct_albums.len() > 1 {
        "cross_album_reissue"
    } else {
        "exact_duplicate"
    };

    // Build differences vec.
    let mut diffs: Vec<GroupDifference> = Vec::new();

    let combined_markers_per_row: Vec<String> = rows
        .iter()
        .map(|r| {
            let mut combined: Vec<&'static str> = Vec::new();
            combined.extend(r.alt_markers.iter().copied());
            combined.extend(r.master_markers.iter().copied());
            if combined.is_empty() {
                "—".to_string()
            } else {
                combined.join(", ")
            }
        })
        .collect();
    if distinct(&combined_markers_per_row).len() > 1 {
        diffs.push(GroupDifference {
            kind: "version_marker".to_string(),
            values: distinct(&combined_markers_per_row),
        });
    }

    if !years.is_empty() && distinct(&years).len() > 1 {
        let mut sorted = years.clone();
        sorted.sort();
        let mut values: Vec<String> = sorted.iter().map(|y| y.to_string()).collect();
        values.dedup();
        diffs.push(GroupDifference {
            kind: "year".to_string(),
            values,
        });
    }

    let albums: Vec<String> = rows
        .iter()
        .filter_map(|r| r.album_title.clone())
        .filter(|s| !s.is_empty())
        .collect();
    if distinct(&albums).len() > 1 {
        diffs.push(GroupDifference {
            kind: "album".to_string(),
            values: distinct(&albums),
        });
    }

    if distinct_quality.len() > 1 {
        let labels: Vec<String> = rows
            .iter()
            .map(|r| quality_label(r.best_quality.as_deref()))
            .collect();
        diffs.push(GroupDifference {
            kind: "quality".to_string(),
            values: distinct(&labels),
        });
    }

    if distinct_local_rates.len() > 1 {
        let labels: Vec<String> = distinct_local_rates
            .iter()
            .map(|hz| format!("{} Hz", hz))
            .collect();
        diffs.push(GroupDifference {
            kind: "sample_rate".to_string(),
            values: labels,
        });
    }

    let sources: Vec<String> = rows.iter().map(|r| r.source.clone()).collect();
    if distinct(&sources).len() > 1 {
        diffs.push(GroupDifference {
            kind: "source".to_string(),
            values: distinct(&sources),
        });
    }

    (relationship.to_string(), diffs)
}

// ── Union-Find ───────────────────────────────────────────────────────────────

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        if self.rank[ra] < self.rank[rb] {
            self.parent[ra] = rb;
        } else if self.rank[ra] > self.rank[rb] {
            self.parent[rb] = ra;
        } else {
            self.parent[rb] = ra;
            self.rank[ra] += 1;
        }
    }

    fn into_groups(self) -> Vec<Vec<usize>> {
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, &p) in self.parent.iter().enumerate() {
            groups.entry(p).or_default().push(i);
        }
        groups.into_values().filter(|g| g.len() > 1).collect()
    }
}

fn connected_components<F>(rows: &[MatchRow], predicate: F) -> Vec<Vec<usize>>
where
    F: Fn(&MatchRow, &MatchRow) -> bool,
{
    let n = rows.len();
    if n <= 1 {
        return Vec::new();
    }

    let mut uf = UnionFind::new(n);

    for i in 0..n {
        for j in (i + 1)..n {
            if predicate(&rows[i], &rows[j]) {
                uf.union(i, j);
            }
        }
    }

    uf.into_groups()
}

// ── Scan ──────────────────────────────────────────────────────────────────────

const ROW_SELECT_FRAGMENT: &str = "
    t.id,
    t.title,
    COALESCE(a.name, ''),
    t.duration_ms,
    t.isrc,
    t.album_id,
    al.title,
    al.year,
    t.best_quality,
    t.sample_rate,
    t.file_path,
    t.source,
    t.fidelity_score,
    t.is_favorite
";

fn read_match_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MatchRow> {
    let id: i64 = row.get(0)?;
    let title: String = row.get(1)?;
    let artist: String = row.get(2)?;
    let duration_ms: Option<i64> = row.get(3)?;
    let isrc: Option<String> = row.get(4)?;
    let album_id: Option<i64> = row.get(5)?;
    let album_title: Option<String> = row.get(6)?;
    let album_year: Option<i32> = row.get(7)?;
    let best_quality: Option<String> = row.get(8)?;
    let sample_rate: Option<i64> = row.get(9)?;
    let file_path: Option<String> = row.get(10)?;
    let source: String = row.get(11)?;
    let fidelity_score: i32 = row.get(12)?;
    let is_favorite: i32 = row.get(13)?;

    Ok(build_match_row(
        id,
        &title,
        &artist,
        duration_ms,
        isrc.filter(|s| !s.is_empty()),
        album_id,
        album_title,
        album_year,
        best_quality,
        sample_rate,
        file_path,
        source,
        fidelity_score,
        is_favorite != 0,
    ))
}

/// Full duplicate scan. Clears old pending groups then rebuilds.
/// Returns counts of what was found.
pub fn scan(conn: &Connection) -> Result<ScanStats> {
    conn.execute("DELETE FROM duplicate_groups WHERE status = 'pending'", [])?;

    let mut isrc_matches = 0usize;
    let mut title_matches = 0usize;

    // ── Pass 1: ISRC matches ─────────────────────────────────────────────────
    let pass1_sql = format!(
        "SELECT {ROW_SELECT_FRAGMENT}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.isrc IS NOT NULL AND t.isrc != ''"
    );
    let mut stmt = conn.prepare(&pass1_sql)?;
    let mut isrc_groups: HashMap<String, Vec<MatchRow>> = HashMap::new();
    for row_res in stmt.query_map([], read_match_row)? {
        let row = row_res?;
        if let Some(isrc) = row.isrc.clone() {
            isrc_groups.entry(isrc).or_default().push(row);
        }
    }

    for rows in isrc_groups.into_values() {
        for component in connected_components(&rows, |left, right| rows_match(left, right, true)) {
            let gid = insert_group(conn, "pending")?;
            for &idx in &component {
                conn.execute(
                    "INSERT OR IGNORE INTO duplicate_members (group_id, track_id, is_preferred)
                     VALUES (?1, ?2, 0)",
                    params![gid, rows[idx].id],
                )?;
            }
            let component_rows: Vec<&MatchRow> = component.iter().map(|&i| &rows[i]).collect();
            assign_preferred(conn, gid, &component_rows)?;
            isrc_matches += 1;
        }
    }

    // ── Pass 2: title + artist + duration match ──────────────────────────────
    let pass2_sql = format!(
        "SELECT {ROW_SELECT_FRAGMENT}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.isrc IS NULL OR t.isrc = ''"
    );
    let mut stmt = conn.prepare(&pass2_sql)?;
    let rows: Vec<MatchRow> = stmt
        .query_map([], read_match_row)?
        .filter_map(|r| r.ok())
        .collect();

    let mut buckets: HashMap<(String, String), Vec<MatchRow>> = HashMap::new();
    for row in rows {
        if row.base_title.is_empty() || row.norm_artist.is_empty() {
            continue;
        }
        buckets
            .entry((row.base_title.clone(), row.norm_artist.clone()))
            .or_default()
            .push(row);
    }

    for rows in buckets.into_values() {
        for component in connected_components(&rows, |left, right| rows_match(left, right, false)) {
            let gid = insert_group(conn, "pending")?;
            for &idx in &component {
                conn.execute(
                    "INSERT OR IGNORE INTO duplicate_members (group_id, track_id, is_preferred)
                     VALUES (?1, ?2, 0)",
                    params![gid, rows[idx].id],
                )?;
            }
            let component_rows: Vec<&MatchRow> = component.iter().map(|&i| &rows[i]).collect();
            assign_preferred(conn, gid, &component_rows)?;
            title_matches += 1;
        }
    }

    let groups_found = isrc_matches + title_matches;
    let tracks_affected = conn.query_row(
        "SELECT COUNT(DISTINCT dm.track_id)
         FROM duplicate_members dm
         JOIN duplicate_groups dg ON dg.id = dm.group_id
         WHERE dg.status = 'pending'",
        [],
        |row| row.get::<_, i64>(0),
    )? as usize;

    Ok(ScanStats {
        groups_found,
        tracks_affected,
        isrc_matches,
        title_matches,
    })
}

fn insert_group(conn: &Connection, status: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO duplicate_groups (status) VALUES (?1)",
        params![status],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Sets `is_preferred = 1` only when the group is an exact duplicate; for any
/// version/master/quality/cross-album relationship the user must pick.
fn assign_preferred(conn: &Connection, group_id: i64, rows: &[&MatchRow]) -> Result<()> {
    let (relationship, _) = classify(rows);
    if relationship != "exact_duplicate" {
        return Ok(());
    }

    // Highest fidelity_score, ties broken by is_favorite.
    let mut best: Option<&MatchRow> = None;
    for row in rows {
        let take = match best {
            None => true,
            Some(b) => {
                row.fidelity_score > b.fidelity_score
                    || (row.fidelity_score == b.fidelity_score && row.is_favorite && !b.is_favorite)
            }
        };
        if take {
            best = Some(row);
        }
    }

    if let Some(b) = best {
        conn.execute(
            "UPDATE duplicate_members SET is_preferred = 1
             WHERE group_id = ?1 AND track_id = ?2",
            params![group_id, b.id],
        )?;
    }

    Ok(())
}

// ── Query ─────────────────────────────────────────────────────────────────────

pub fn count_pending_groups(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM duplicate_groups WHERE status = 'pending'",
        [],
        |row| row.get(0),
    )?)
}

/// Paginated list of pending duplicate groups with full track data and
/// classifier output.
pub fn load_groups(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<DuplicateGroup>> {
    let group_ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM duplicate_groups WHERE status = 'pending'
             ORDER BY id ASC LIMIT ?1 OFFSET ?2",
        )?;
        stmt.query_map(params![limit, offset], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect()
    };

    let mut groups = Vec::new();
    for gid in group_ids {
        let (members, classify_rows) = load_members_with_classify_rows(conn, gid)?;
        let row_refs: Vec<&MatchRow> = classify_rows.iter().collect();
        let (relationship, differences) = classify(&row_refs);
        groups.push(DuplicateGroup {
            id: gid,
            status: "pending".to_string(),
            relationship,
            differences,
            members,
        });
    }
    Ok(groups)
}

fn load_members_with_classify_rows(
    conn: &Connection,
    group_id: i64,
) -> Result<(Vec<DuplicateMember>, Vec<MatchRow>)> {
    let mut stmt = conn.prepare(
        "SELECT dm.is_preferred,
                t.id, t.title, t.artist_id, a.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url,
                al.year, t.sample_rate, t.file_path
         FROM duplicate_members dm
         JOIN tracks t ON dm.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE dm.group_id = ?1
         ORDER BY t.fidelity_score DESC, t.is_favorite DESC",
    )?;

    let mut members: Vec<DuplicateMember> = Vec::new();
    let mut match_rows: Vec<MatchRow> = Vec::new();

    let iter = stmt.query_map(params![group_id], |row| {
        let is_preferred: i32 = row.get(0)?;
        let track = Track {
            id: row.get(1)?,
            title: row.get(2)?,
            artist_id: row.get(3)?,
            artist_name: row.get(4)?,
            album_id: row.get(5)?,
            album_title: row.get(6)?,
            disc_number: row.get(7)?,
            track_number: row.get(8)?,
            duration_ms: row.get(9)?,
            isrc: row.get(10)?,
            tidal_id: row.get(11)?,
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: row.get(12)?,
            soundcloud_id: row.get(13)?,
            best_quality: row.get(14)?,
            best_source: row.get(15)?,
            fidelity_score: row.get(16)?,
            is_favorite: row.get::<_, i32>(17)? != 0,
            play_count: row.get(18)?,
            last_played_at: row.get(19)?,
            date_added: row.get(20)?,
            source: row.get(21)?,
            artwork_url: row.get(22)?,
        };
        let album_year: Option<i32> = row.get(23)?;
        let sample_rate: Option<i64> = row.get(24)?;
        let file_path: Option<String> = row.get(25)?;
        Ok((track, is_preferred != 0, album_year, sample_rate, file_path))
    })?;

    for r in iter {
        let (track, is_preferred, album_year, sample_rate, file_path) = r?;
        let artist = track.artist_name.clone().unwrap_or_default();
        let row = build_match_row(
            track.id,
            &track.title,
            &artist,
            track.duration_ms,
            track.isrc.clone().filter(|s| !s.is_empty()),
            track.album_id,
            track.album_title.clone(),
            album_year,
            track.best_quality.clone(),
            sample_rate,
            file_path,
            track.source.clone(),
            track.fidelity_score,
            track.is_favorite,
        );
        match_rows.push(row);
        members.push(DuplicateMember {
            track,
            is_preferred,
        });
    }

    Ok((members, match_rows))
}

// ── Resolve ───────────────────────────────────────────────────────────────────

pub struct ResolveResult {
    pub removed_track_ids: Vec<i64>,
    pub tidal_ids_to_unfavorite: Vec<i64>,
    pub reconcile: ReconcileOutcome,
}

/// Keep `preferred_track_id`, dismiss or delete the rest.
/// Marks the group as 'resolved'.
pub fn resolve_group(
    conn: &Connection,
    group_id: i64,
    preferred_track_id: i64,
) -> Result<ResolveResult> {
    conn.execute(
        "UPDATE duplicate_members SET is_preferred = 0 WHERE group_id = ?1",
        params![group_id],
    )?;
    conn.execute(
        "UPDATE duplicate_members SET is_preferred = 1
         WHERE group_id = ?1 AND track_id = ?2",
        params![group_id, preferred_track_id],
    )?;

    let mut stmt = conn.prepare(
        "SELECT dm.track_id, t.tidal_id
         FROM duplicate_members dm
         JOIN tracks t ON dm.track_id = t.id
         WHERE dm.group_id = ?1 AND dm.is_preferred = 0",
    )?;
    let to_remove: Vec<(i64, Option<i64>)> = stmt
        .query_map(params![group_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let removed_track_ids: Vec<i64> = to_remove.iter().map(|(id, _)| *id).collect();
    let tidal_ids_to_unfavorite: Vec<i64> = to_remove.iter().filter_map(|(_, tid)| *tid).collect();

    let reconcile =
        crate::playback::player::reconcile_after_track_delete(conn, &removed_track_ids)?;

    for &track_id in &removed_track_ids {
        conn.execute(
            "DELETE FROM listen_history WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM playlist_tracks WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM shuffle_state WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM duplicate_members WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM track_genres WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute("DELETE FROM tracks WHERE id = ?1", params![track_id])?;
    }

    conn.execute(
        "UPDATE duplicate_groups SET status = 'resolved' WHERE id = ?1",
        params![group_id],
    )?;

    Ok(ResolveResult {
        removed_track_ids,
        tidal_ids_to_unfavorite,
        reconcile,
    })
}

// ── Auto-merge ───────────────────────────────────────────────────────────────

pub struct MergeOutcome {
    pub removed_track_ids: Vec<i64>,
    /// TIDAL ids of removed rows that were favorited. The caller should push
    /// the like to the kept row on TIDAL and unfavorite these, otherwise the
    /// next Full sync's reconciliation wipes the transferred like.
    pub favorited_loser_tidal_ids: Vec<i64>,
    pub kept_tidal_id: Option<i64>,
    pub reconcile: ReconcileOutcome,
}

/// Merge a duplicate group into `preferred_track_id`, preserving history.
/// Unlike [`resolve_group`] (manual UI path, deletes references outright),
/// this REPOINTS listen_history and playlist memberships to the kept row,
/// folds flags/play counts, and moves DSP/embedding rows the kept row lacks,
/// before deleting the losers. Safe for automatic use.
pub fn merge_group(
    conn: &Connection,
    group_id: i64,
    preferred_track_id: i64,
) -> Result<MergeOutcome> {
    let mut stmt = conn.prepare(
        "SELECT dm.track_id, t.tidal_id, t.is_favorite, t.is_library, t.play_count
         FROM duplicate_members dm
         JOIN tracks t ON dm.track_id = t.id
         WHERE dm.group_id = ?1 AND dm.track_id != ?2",
    )?;
    let losers: Vec<(i64, Option<i64>, i32, i32, i64)> = stmt
        .query_map(params![group_id, preferred_track_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let removed_track_ids: Vec<i64> = losers.iter().map(|l| l.0).collect();
    let favorited_loser_tidal_ids: Vec<i64> = losers
        .iter()
        .filter(|l| l.2 != 0)
        .filter_map(|l| l.1)
        .collect();

    // Fold flags and plays into the kept row: a merge can move a like, never
    // lose one.
    let fav_fold = losers.iter().any(|l| l.2 != 0) as i32;
    let lib_fold = losers.iter().any(|l| l.3 != 0) as i32;
    let plays_fold: i64 = losers.iter().map(|l| l.4).sum();
    conn.execute(
        "UPDATE tracks SET
            is_favorite = MAX(is_favorite, ?1),
            is_library = MAX(is_library, ?2),
            play_count = play_count + ?3
         WHERE id = ?4",
        params![fav_fold, lib_fold, plays_fold, preferred_track_id],
    )?;
    // Zero the folded counters on the losers so an interrupted merge that
    // re-runs after the next scan cannot double-count plays.
    for &(loser_id, _, _, _, _) in &losers {
        conn.execute(
            "UPDATE tracks SET play_count = 0 WHERE id = ?1",
            params![loser_id],
        )?;
    }

    for &(loser_id, _, _, _, _) in &losers {
        // Listen history feeds taste vectors, heat, and stats: repoint, never
        // delete.
        conn.execute(
            "UPDATE listen_history SET track_id = ?1 WHERE track_id = ?2",
            params![preferred_track_id, loser_id],
        )?;
        // Playlist memberships follow the kept row. PK is
        // (playlist_id, position) so this cannot conflict...
        conn.execute(
            "UPDATE playlist_tracks SET track_id = ?1 WHERE track_id = ?2",
            params![preferred_track_id, loser_id],
        )?;
        // Move analysis rows the kept row lacks; leftovers are cleaned below.
        conn.execute(
            "UPDATE OR IGNORE audio_dsp_features SET track_id = ?1 WHERE track_id = ?2",
            params![preferred_track_id, loser_id],
        )?;
        conn.execute(
            "UPDATE OR IGNORE track_embeddings SET track_id = ?1 WHERE track_id = ?2",
            params![preferred_track_id, loser_id],
        )?;
    }

    // ...but a playlist that contained several copies now lists the kept
    // track more than once: drop the later positions.
    conn.execute(
        "DELETE FROM playlist_tracks WHERE rowid IN (
            SELECT later.rowid
            FROM playlist_tracks later
            JOIN playlist_tracks earlier
              ON earlier.playlist_id = later.playlist_id
             AND earlier.track_id = later.track_id
             AND earlier.position < later.position
            WHERE later.track_id = ?1
         )",
        params![preferred_track_id],
    )?;

    let reconcile =
        crate::playback::player::reconcile_after_track_delete(conn, &removed_track_ids)?;

    // Explicit cleanup of remaining loser references. The shipped DB would
    // cascade most of these on the tracks delete, but being explicit keeps
    // behavior identical when foreign_keys is off (tests, older DBs).
    for &track_id in &removed_track_ids {
        conn.execute(
            "DELETE FROM audio_dsp_features WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM track_embeddings WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM track_neighbors WHERE track_id = ?1 OR neighbor_track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM track_similarity WHERE track_a = ?1 OR track_b = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM shuffle_state WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM track_genres WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM duplicate_members WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute("DELETE FROM tracks WHERE id = ?1", params![track_id])?;
    }

    conn.execute(
        "UPDATE duplicate_members SET is_preferred = 1
         WHERE group_id = ?1 AND track_id = ?2",
        params![group_id, preferred_track_id],
    )?;
    conn.execute(
        "UPDATE duplicate_groups SET status = 'resolved' WHERE id = ?1",
        params![group_id],
    )?;

    let kept_tidal_id: Option<i64> = conn
        .query_row(
            "SELECT tidal_id FROM tracks WHERE id = ?1",
            params![preferred_track_id],
            |row| row.get(0),
        )
        .unwrap_or(None);

    Ok(MergeOutcome {
        removed_track_ids,
        favorited_loser_tidal_ids,
        kept_tidal_id,
        reconcile,
    })
}

/// Deterministic keep-rule for automatic merges: liked first, then library
/// membership, then the copy on a favorited album (so a favorited album's
/// detail page keeps its row), then fidelity, plays, lowest id. Re-runs are
/// idempotent because the ordering is total.
fn pick_preferred(conn: &Connection, group_id: i64) -> Result<Option<i64>> {
    let preferred = conn
        .query_row(
            "SELECT t.id
             FROM duplicate_members dm
             JOIN tracks t ON dm.track_id = t.id
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE dm.group_id = ?1
             ORDER BY t.is_favorite DESC,
                      t.is_library DESC,
                      COALESCE(al.is_favorite, 0) DESC,
                      t.fidelity_score DESC,
                      t.play_count DESC,
                      t.id ASC
             LIMIT 1",
            params![group_id],
            |row| row.get::<_, i64>(0),
        )
        .map(Some)
        .unwrap_or(None);
    Ok(preferred)
}

#[derive(Debug, Default, Serialize)]
pub struct AutoMergeStats {
    pub merged_groups: usize,
    pub removed_tracks: usize,
    /// Groups left for the Duplicates UI (alt_version, local files).
    pub skipped_groups: usize,
    /// (kept tidal_id, favorited loser tidal_ids) pairs the caller must
    /// reconcile on TIDAL.
    #[serde(skip)]
    pub favorite_transfers: Vec<(i64, Vec<i64>)>,
    pub queue_changed: bool,
    pub current_changed: bool,
}

/// Auto-merge every pending same-recording group. Variants (`alt_version`)
/// and groups touching local files are never auto-merged; liked rows always
/// survive as the kept row per [`pick_preferred`].
pub fn auto_merge_pending(conn: &Connection) -> Result<AutoMergeStats> {
    let group_ids: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT id FROM duplicate_groups WHERE status = 'pending' ORDER BY id ASC")?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?
    };

    let mut stats = AutoMergeStats::default();
    for gid in group_ids {
        let (members, classify_rows) = load_members_with_classify_rows(conn, gid)?;
        if members.len() < 2 {
            continue;
        }
        let row_refs: Vec<&MatchRow> = classify_rows.iter().collect();
        let (relationship, _) = classify(&row_refs);
        let touches_local_file = classify_rows
            .iter()
            .any(|r| r.file_path.as_deref().is_some_and(|p| !p.is_empty()));
        let same_recording = matches!(
            relationship.as_str(),
            "exact_duplicate" | "quality_variant" | "cross_album_reissue" | "remaster"
        );
        if !same_recording || touches_local_file {
            stats.skipped_groups += 1;
            continue;
        }

        let Some(preferred) = pick_preferred(conn, gid)? else {
            continue;
        };

        // Not wrapped in a transaction: reconcile_after_track_delete opens
        // its own, and SQLite cannot nest. Same non-atomic shape as
        // resolve_group; a crash mid-merge is repaired by the next scan.
        let outcome = merge_group(conn, gid, preferred)?;

        stats.merged_groups += 1;
        stats.removed_tracks += outcome.removed_track_ids.len();
        stats.queue_changed |= outcome.reconcile.queue_changed;
        stats.current_changed |= outcome.reconcile.current_changed;
        if !outcome.favorited_loser_tidal_ids.is_empty()
            && let Some(kept) = outcome.kept_tidal_id
        {
            stats
                .favorite_transfers
                .push((kept, outcome.favorited_loser_tidal_ids));
        }
    }

    Ok(stats)
}

/// Mark group as dismissed without deleting anything.
pub fn dismiss_group(conn: &Connection, group_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE duplicate_groups SET status = 'dismissed' WHERE id = ?1",
        params![group_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open test db");
        conn.execute_batch(
            "
            CREATE TABLE artists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE TABLE albums (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                year INTEGER,
                artwork_url TEXT,
                is_favorite INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                disc_number INTEGER,
                track_number INTEGER,
                duration_ms INTEGER,
                isrc TEXT,
                tidal_id INTEGER,
                ytmusic_id TEXT,
                soundcloud_id INTEGER,
                best_quality TEXT,
                best_source TEXT,
                fidelity_score INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played_at TEXT,
                date_added TEXT,
                source TEXT NOT NULL DEFAULT 'tidal',
                sample_rate INTEGER,
                bit_depth INTEGER,
                file_path TEXT,
                is_library INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE audio_dsp_features (
                track_id INTEGER PRIMARY KEY,
                bpm REAL
            );

            CREATE TABLE track_embeddings (
                track_id INTEGER NOT NULL,
                model_id INTEGER NOT NULL,
                vector_blob BLOB,
                PRIMARY KEY (track_id, model_id)
            );

            CREATE TABLE track_neighbors (
                track_id INTEGER NOT NULL,
                neighbor_track_id INTEGER NOT NULL,
                model_id INTEGER NOT NULL,
                rank INTEGER NOT NULL DEFAULT 0,
                score REAL NOT NULL DEFAULT 0
            );

            CREATE TABLE track_similarity (
                track_a INTEGER NOT NULL,
                track_b INTEGER NOT NULL,
                similarity_score REAL NOT NULL DEFAULT 0
            );

            CREATE TABLE duplicate_groups (
                id INTEGER PRIMARY KEY,
                status TEXT DEFAULT 'pending'
            );

            CREATE TABLE duplicate_members (
                group_id INTEGER NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
                track_id INTEGER NOT NULL REFERENCES tracks(id),
                is_preferred INTEGER DEFAULT 0,
                PRIMARY KEY (group_id, track_id)
            );

            CREATE TABLE listen_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                duration_listened_ms INTEGER DEFAULT 0,
                completed INTEGER DEFAULT 0
            );

            CREATE TABLE playlist_tracks (
                playlist_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                position INTEGER NOT NULL
            );

            CREATE TABLE queue (
                id               INTEGER PRIMARY KEY,
                track_id         INTEGER,
                position         INTEGER NOT NULL,
                source           TEXT    DEFAULT 'user',
                reason           TEXT,
                pending_artist   TEXT,
                pending_title    TEXT,
                pending_at       TIMESTAMP,
                resolving_at     TIMESTAMP,
                resolved_at      TIMESTAMP,
                tidal_match_score REAL,
                tidal_id_hint    INTEGER
            );

            CREATE TABLE shuffle_state (
                track_id INTEGER PRIMARY KEY,
                position INTEGER NOT NULL
            );

            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT,
                confidence REAL DEFAULT 1.0
            );

            CREATE TABLE playback_state (
                id INTEGER PRIMARY KEY,
                current_track_id INTEGER,
                current_queue_item_id INTEGER,
                position_ms INTEGER NOT NULL DEFAULT 0,
                is_playing INTEGER NOT NULL DEFAULT 0,
                volume REAL NOT NULL DEFAULT 1.0,
                shuffle_mode TEXT NOT NULL DEFAULT 'off',
                repeat_mode TEXT NOT NULL DEFAULT 'off',
                automix_enabled INTEGER NOT NULL DEFAULT 0,
                crossfade_ms INTEGER NOT NULL DEFAULT 0,
                automix_discover_new INTEGER NOT NULL DEFAULT 0,
                automix_use_learning INTEGER NOT NULL DEFAULT 1,
                automix_allow_external INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .expect("create schema");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Test Artist')",
            [],
        )
        .expect("insert artist");
        conn.execute(
            "INSERT INTO playback_state (
                id, current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms
            ) VALUES (1, NULL, 0, 0, 1.0, 'off', 'off', 0, 0)",
            [],
        )
        .expect("seed playback_state");
        conn
    }

    fn insert_album(conn: &Connection, id: i64, title: &str, year: Option<i32>) {
        conn.execute(
            "INSERT INTO albums (id, title, year) VALUES (?1, ?2, ?3)",
            params![id, title, year],
        )
        .expect("insert album");
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_track_full(
        conn: &Connection,
        id: i64,
        title: &str,
        duration_ms: i64,
        isrc: Option<&str>,
        album_id: Option<i64>,
        best_quality: Option<&str>,
        sample_rate: Option<i64>,
        file_path: Option<&str>,
        fidelity_score: i32,
    ) {
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, isrc,
                best_quality, sample_rate, file_path, fidelity_score, source
             ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'tidal')",
            params![
                id,
                title,
                album_id,
                duration_ms,
                isrc,
                best_quality,
                sample_rate,
                file_path,
                fidelity_score
            ],
        )
        .expect("insert track");
    }

    fn insert_track(conn: &Connection, id: i64, title: &str, duration_ms: i64, isrc: Option<&str>) {
        insert_track_full(
            conn,
            id,
            title,
            duration_ms,
            isrc,
            None,
            None,
            None,
            None,
            100,
        );
    }

    fn group_relationships(conn: &Connection) -> Vec<String> {
        let groups = load_groups(conn, 100, 0).expect("load groups");
        groups.into_iter().map(|g| g.relationship).collect()
    }

    #[test]
    fn scan_rejects_shared_isrc_with_large_duration_gap() {
        let conn = test_conn();
        insert_track(&conn, 1, "My Barn My Rules", 127_000, Some("DEU672200178"));
        insert_track(&conn, 2, "My Barn My Rules", 266_000, Some("DEU672200178"));

        let stats = scan(&conn).expect("scan duplicates");

        assert_eq!(stats.groups_found, 0);
        assert_eq!(stats.tracks_affected, 0);
    }

    #[test]
    fn classifies_alt_version_remix() {
        let conn = test_conn();
        // Variant marker mismatch with shared ISRC — must group, classified as alt_version.
        insert_track(
            &conn,
            1,
            "Tarlabasi (Be Svendsen Remix)",
            546_000,
            Some("DEHM81600158"),
        );
        insert_track(&conn, 2, "Tarlabasi", 545_000, Some("DEHM81600158"));

        let stats = scan(&conn).expect("scan duplicates");
        assert_eq!(stats.groups_found, 1);

        let groups = load_groups(&conn, 10, 0).expect("load groups");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].relationship, "alt_version");
        // No is_preferred for non-exact relationships.
        assert!(groups[0].members.iter().all(|m| !m.is_preferred));
    }

    #[test]
    fn scan_keeps_feature_credit_variants_as_duplicates() {
        let conn = test_conn();
        insert_track(
            &conn,
            1,
            "Cachaca (feat. Tom Scott)",
            291_000,
            Some("AUI441600195"),
        );
        insert_track(&conn, 2, "Cachaca", 290_000, Some("AUI441600195"));

        let stats = scan(&conn).expect("scan duplicates");
        let groups = load_groups(&conn, 10, 0).expect("load duplicate groups");

        assert_eq!(stats.groups_found, 1);
        assert_eq!(stats.tracks_affected, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].relationship, "exact_duplicate");
        assert!(groups[0].members.iter().any(|m| m.is_preferred));
    }

    #[test]
    fn scan_counts_only_pending_tracks_in_stats() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO duplicate_groups (id, status) VALUES (99, 'resolved')",
            [],
        )
        .expect("insert resolved group");
        insert_track(&conn, 1, "Only Old Group", 180_000, Some("OLDISRC"));
        conn.execute(
            "INSERT INTO duplicate_members (group_id, track_id, is_preferred) VALUES (99, 1, 1)",
            [],
        )
        .expect("insert resolved membership");

        let stats = scan(&conn).expect("scan duplicates");

        assert_eq!(stats.groups_found, 0);
        assert_eq!(stats.tracks_affected, 0);
    }

    #[test]
    fn classifies_remaster_with_year_drift() {
        let conn = test_conn();
        insert_album(&conn, 1, "Original Album", Some(1991));
        insert_album(&conn, 2, "Remaster Reissue", Some(2011));
        insert_track_full(
            &conn,
            1,
            "Memory Lane",
            240_000,
            None,
            Some(1),
            Some("HI_RES"),
            None,
            None,
            150,
        );
        insert_track_full(
            &conn,
            2,
            "Memory Lane",
            240_500,
            None,
            Some(2),
            Some("HI_RES"),
            None,
            None,
            150,
        );

        scan(&conn).expect("scan");
        let groups = load_groups(&conn, 10, 0).expect("load");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].relationship, "remaster");
        assert!(groups[0].differences.iter().any(|d| d.kind == "year"));
        assert!(groups[0].members.iter().all(|m| !m.is_preferred));
    }

    #[test]
    fn classifies_mono_vs_stereo_as_remaster() {
        let conn = test_conn();
        insert_album(&conn, 1, "Sgt. Pepper", None);
        insert_track_full(
            &conn,
            1,
            "Sgt. Pepper (Mono)",
            150_000,
            None,
            Some(1),
            Some("LOSSLESS"),
            None,
            None,
            120,
        );
        insert_track_full(
            &conn,
            2,
            "Sgt. Pepper (Stereo)",
            150_500,
            None,
            Some(1),
            Some("LOSSLESS"),
            None,
            None,
            120,
        );

        scan(&conn).expect("scan");
        let groups = load_groups(&conn, 10, 0).expect("load");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].relationship, "remaster");
        assert!(
            groups[0]
                .differences
                .iter()
                .any(|d| d.kind == "version_marker")
        );
        assert!(groups[0].members.iter().all(|m| !m.is_preferred));
    }

    #[test]
    fn classifies_cross_album_reissue() {
        let conn = test_conn();
        insert_album(&conn, 1, "Studio Album", Some(2005));
        insert_album(&conn, 2, "Greatest Hits", Some(2007));
        insert_track_full(
            &conn,
            1,
            "Anthem",
            200_000,
            Some("ISRC123"),
            Some(1),
            Some("LOSSLESS"),
            None,
            None,
            100,
        );
        insert_track_full(
            &conn,
            2,
            "Anthem",
            200_300,
            Some("ISRC123"),
            Some(2),
            Some("LOSSLESS"),
            None,
            None,
            100,
        );

        scan(&conn).expect("scan");
        let groups = load_groups(&conn, 10, 0).expect("load");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].relationship, "cross_album_reissue");
        assert!(groups[0].differences.iter().any(|d| d.kind == "album"));
        // Cross-album reissues are not auto-preferred — same-recording but
        // different release; the user picks.
        assert!(groups[0].members.iter().all(|m| !m.is_preferred));
    }

    #[test]
    fn classifies_quality_variant() {
        let conn = test_conn();
        insert_album(&conn, 1, "Studio Album", Some(2020));
        insert_track_full(
            &conn,
            1,
            "Brightside",
            210_000,
            Some("QV12345"),
            Some(1),
            Some("HI_RES_LOSSLESS"),
            None,
            None,
            200,
        );
        insert_track_full(
            &conn,
            2,
            "Brightside",
            210_200,
            Some("QV12345"),
            Some(1),
            Some("LOW"),
            None,
            None,
            50,
        );

        scan(&conn).expect("scan");
        let groups = load_groups(&conn, 10, 0).expect("load");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].relationship, "quality_variant");
        assert!(groups[0].differences.iter().any(|d| d.kind == "quality"));
        assert!(groups[0].members.iter().all(|m| !m.is_preferred));
    }

    #[test]
    fn classifies_exact_duplicate_and_marks_preferred() {
        let conn = test_conn();
        insert_album(&conn, 1, "Studio Album", Some(2020));
        insert_track_full(
            &conn,
            1,
            "Same Recording",
            180_000,
            Some("EXD0001"),
            Some(1),
            Some("LOSSLESS"),
            None,
            None,
            200,
        );
        insert_track_full(
            &conn,
            2,
            "Same Recording",
            180_500,
            Some("EXD0001"),
            Some(1),
            Some("LOSSLESS"),
            None,
            None,
            150,
        );

        scan(&conn).expect("scan");
        let groups = load_groups(&conn, 10, 0).expect("load");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].relationship, "exact_duplicate");
        // The higher-fidelity row should be marked preferred.
        let preferred: Vec<i64> = groups[0]
            .members
            .iter()
            .filter(|m| m.is_preferred)
            .map(|m| m.track.id)
            .collect();
        assert_eq!(preferred, vec![1]);
    }

    #[test]
    fn bucket_groups_alt_versions_together() {
        let conn = test_conn();
        // No shared ISRC and different canonical titles — these would NOT have
        // bucketed under the old logic. base_title strips "(Remix)" so they do.
        insert_track(&conn, 1, "Wavelength", 200_000, None);
        insert_track(&conn, 2, "Wavelength (Remix)", 200_500, None);

        scan(&conn).expect("scan");
        let rels = group_relationships(&conn);

        assert_eq!(rels, vec!["alt_version".to_string()]);
    }

    #[test]
    fn tightened_duration_rejects_loose_match() {
        let conn = test_conn();
        // Same canonical title, no ISRC, but durations differ by 2.5s — over
        // the tightened ±2000ms tolerance for non-ISRC matches.
        insert_track(&conn, 1, "Drift", 180_000, None);
        insert_track(&conn, 2, "Drift", 182_500, None);

        let stats = scan(&conn).expect("scan");
        assert_eq!(stats.groups_found, 0);
    }

    #[test]
    fn local_sample_rate_difference_classifies_quality_variant() {
        let conn = test_conn();
        insert_album(&conn, 1, "Master Tape", Some(2018));
        insert_track_full(
            &conn,
            1,
            "Origin",
            240_000,
            Some("SR0001"),
            Some(1),
            Some("LOSSLESS"),
            Some(44_100),
            Some("/music/origin.flac"),
            150,
        );
        insert_track_full(
            &conn,
            2,
            "Origin",
            240_400,
            Some("SR0001"),
            Some(1),
            Some("LOSSLESS"),
            Some(96_000),
            Some("/music/origin-hires.flac"),
            150,
        );

        scan(&conn).expect("scan");
        let groups = load_groups(&conn, 10, 0).expect("load");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].relationship, "quality_variant");
        assert!(
            groups[0]
                .differences
                .iter()
                .any(|d| d.kind == "sample_rate")
        );
    }

    // ── decide_import ────────────────────────────────────────────────────────

    fn incoming<'a>(
        tidal_id: i64,
        title: &'a str,
        isrc: Option<&'a str>,
        duration_ms: i64,
    ) -> IncomingTrack<'a> {
        IncomingTrack {
            tidal_id,
            title,
            artist_name: "Test Artist",
            isrc,
            duration_ms,
        }
    }

    fn candidate(
        track_id: i64,
        tidal_id: Option<i64>,
        title: &str,
        isrc: Option<&str>,
        duration_ms: i64,
    ) -> ExistingCandidate {
        ExistingCandidate {
            track_id,
            tidal_id,
            title: title.to_string(),
            artist_name: "Test Artist".to_string(),
            isrc: isrc.map(str::to_string),
            duration_ms,
        }
    }

    #[test]
    fn decide_import_same_tidal_id_is_a_resync_not_a_duplicate() {
        // Rule 0: the upsert must run so re-syncs keep refreshing metadata.
        let inc = incoming(42, "Song", Some("ISRC1"), 200_000);
        let cands = vec![candidate(1, Some(42), "Song", Some("ISRC1"), 200_000)];
        assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
    }

    #[test]
    fn decide_import_exact_isrc_dup_skips() {
        let inc = incoming(42, "Song", Some("isrc1 "), 200_000);
        let cands = vec![candidate(7, Some(99), "Song", Some("ISRC1"), 201_000)];
        assert_eq!(
            decide_import(&inc, &cands),
            ImportDecision::SkipDuplicate {
                existing_track_id: 7,
                existing_tidal_id: Some(99),
            }
        );
    }

    #[test]
    fn decide_import_isrc_with_large_duration_gap_inserts() {
        // Known upstream bug: one ISRC reused across different-length cuts.
        let inc = incoming(42, "My Barn My Rules", Some("DEU672200178"), 127_000);
        let cands = vec![candidate(
            7,
            Some(99),
            "My Barn My Rules",
            Some("DEU672200178"),
            266_000,
        )];
        assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
    }

    #[test]
    fn decide_import_single_vs_album_same_title_skips() {
        // Same recording released on a single and an album: no ISRC on the
        // incoming copy, title+artist+duration collapse it.
        let inc = incoming(42, "Song", None, 200_000);
        let cands = vec![candidate(7, Some(99), "Song", None, 200_500)];
        assert_eq!(
            decide_import(&inc, &cands),
            ImportDecision::SkipDuplicate {
                existing_track_id: 7,
                existing_tidal_id: Some(99),
            }
        );
    }

    #[test]
    fn decide_import_keeps_live_variant() {
        // Variants are never duplicates: differing alt fingerprints.
        let inc = incoming(42, "Song (Live)", None, 200_000);
        let cands = vec![candidate(7, Some(99), "Song", None, 200_000)];
        assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);

        // And the mirror: incoming base, only the live cut exists.
        let inc = incoming(43, "Song", None, 200_000);
        let cands = vec![candidate(8, Some(98), "Song (Live)", None, 200_000)];
        assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
    }

    #[test]
    fn decide_import_keeps_remix_variant() {
        let inc = incoming(42, "Song (Remix)", None, 200_000);
        let cands = vec![candidate(7, Some(99), "Song", None, 200_000)];
        assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
    }

    #[test]
    fn decide_import_collapses_remaster_of_same_recording() {
        // Master markers are not part of the variant fingerprint: a remaster
        // with matching duration is the same recording.
        let inc = incoming(42, "Song (2011 Remaster)", None, 200_000);
        let cands = vec![candidate(7, Some(99), "Song", None, 200_500)];
        assert_eq!(
            decide_import(&inc, &cands),
            ImportDecision::SkipDuplicate {
                existing_track_id: 7,
                existing_tidal_id: Some(99),
            }
        );
    }

    #[test]
    fn decide_import_extended_cut_survives_via_duration() {
        // "Extended" is a master token, so the duration gate is what keeps a
        // genuinely longer cut alive.
        let inc = incoming(42, "Song (Extended Mix)", None, 260_000);
        let cands = vec![candidate(7, Some(99), "Song", None, 200_000)];
        assert_eq!(decide_import(&inc, &cands), ImportDecision::Insert);
    }

    #[test]
    fn decide_import_no_candidates_inserts() {
        let inc = incoming(42, "Song", Some("ISRC1"), 200_000);
        assert_eq!(decide_import(&inc, &[]), ImportDecision::Insert);
    }

    // ── merge_group / auto_merge_pending ────────────────────────────────────

    fn seed_pending_group(conn: &Connection, track_ids: &[i64]) -> i64 {
        conn.execute(
            "INSERT INTO duplicate_groups (status) VALUES ('pending')",
            [],
        )
        .expect("insert group");
        let gid = conn.last_insert_rowid();
        for &tid in track_ids {
            conn.execute(
                "INSERT INTO duplicate_members (group_id, track_id, is_preferred) VALUES (?1, ?2, 0)",
                params![gid, tid],
            )
            .expect("insert member");
        }
        gid
    }

    #[test]
    fn merge_group_repoints_history_and_transfers_like() {
        let conn = test_conn();
        insert_track(&conn, 1, "Song", 200_000, Some("ISRC1"));
        insert_track(&conn, 2, "Song", 200_500, Some("ISRC1"));
        // Loser 2 carries the like, plays, history, playlist membership, DSP.
        conn.execute(
            "UPDATE tracks SET is_favorite = 1, play_count = 5 WHERE id = 2",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at) VALUES (2, '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (1, 2, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (2, 128.0)",
            [],
        )
        .unwrap();
        conn.execute("UPDATE tracks SET tidal_id = 900 WHERE id = 1", [])
            .unwrap();
        conn.execute("UPDATE tracks SET tidal_id = 901 WHERE id = 2", [])
            .unwrap();

        let gid = seed_pending_group(&conn, &[1, 2]);
        let outcome = merge_group(&conn, gid, 1).expect("merge");

        assert_eq!(outcome.removed_track_ids, vec![2]);
        assert_eq!(outcome.favorited_loser_tidal_ids, vec![901]);
        assert_eq!(outcome.kept_tidal_id, Some(900));

        // Like, plays, history, playlist, DSP all moved to the kept row.
        let (fav, plays): (i32, i64) = conn
            .query_row(
                "SELECT is_favorite, play_count FROM tracks WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(fav, 1);
        assert_eq!(plays, 5);
        let history_target: i64 = conn
            .query_row("SELECT track_id FROM listen_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(history_target, 1);
        let playlist_target: i64 = conn
            .query_row("SELECT track_id FROM playlist_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(playlist_target, 1);
        let dsp_target: i64 = conn
            .query_row("SELECT track_id FROM audio_dsp_features", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dsp_target, 1);

        // Loser row gone, group resolved.
        let loser_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks WHERE id = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(loser_count, 0);
        let status: String = conn
            .query_row(
                "SELECT status FROM duplicate_groups WHERE id = ?1",
                [gid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "resolved");
    }

    #[test]
    fn merge_group_dedupes_playlist_holding_both_copies() {
        let conn = test_conn();
        insert_track(&conn, 1, "Song", 200_000, Some("ISRC1"));
        insert_track(&conn, 2, "Song", 200_500, Some("ISRC1"));
        conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (1, 1, 0), (1, 2, 1)",
            [],
        )
        .unwrap();

        let gid = seed_pending_group(&conn, &[1, 2]);
        merge_group(&conn, gid, 1).expect("merge");

        // One membership survives, pointing at the kept row.
        let rows: Vec<(i64, i64)> = {
            let mut stmt = conn
                .prepare("SELECT track_id, position FROM playlist_tracks ORDER BY position")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(rows, vec![(1, 0)]);
    }

    #[test]
    fn auto_merge_keeps_liked_row_and_skips_alt_versions() {
        let conn = test_conn();
        // Group A: exact ISRC dup; the LIKED row must be the survivor even
        // with lower fidelity.
        insert_track_full(
            &conn,
            1,
            "Song",
            200_000,
            Some("ISRC1"),
            None,
            None,
            None,
            None,
            100,
        );
        insert_track_full(
            &conn,
            2,
            "Song",
            200_400,
            Some("ISRC1"),
            None,
            None,
            None,
            None,
            900,
        );
        conn.execute("UPDATE tracks SET is_favorite = 1 WHERE id = 1", [])
            .unwrap();
        // Group B: alt_version (remix) - must survive untouched.
        insert_track(&conn, 3, "Wavelength", 210_000, None);
        insert_track(&conn, 4, "Wavelength (Remix)", 210_200, None);

        scan(&conn).expect("scan");
        let stats = auto_merge_pending(&conn).expect("auto merge");

        assert_eq!(stats.merged_groups, 1);
        assert_eq!(stats.removed_tracks, 1);
        assert_eq!(stats.skipped_groups, 1);

        // Liked low-fidelity row survived; high-fidelity loser removed.
        let survivor: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE id = 1 AND is_favorite = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(survivor, 1);
        let loser: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks WHERE id = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(loser, 0);
        // Both remix-group rows still present.
        let variants: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks WHERE id IN (3, 4)", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(variants, 2);

        // Idempotent: nothing left to merge on a second pass.
        scan(&conn).expect("rescan");
        let stats2 = auto_merge_pending(&conn).expect("second pass");
        assert_eq!(stats2.merged_groups, 0);
        assert_eq!(stats2.removed_tracks, 0);
    }

    #[test]
    fn auto_merge_never_touches_local_files() {
        let conn = test_conn();
        insert_track_full(
            &conn,
            1,
            "Origin",
            240_000,
            Some("SR0001"),
            None,
            Some("LOSSLESS"),
            None,
            Some("/music/origin.flac"),
            150,
        );
        insert_track_full(
            &conn,
            2,
            "Origin",
            240_200,
            Some("SR0001"),
            None,
            Some("HI_RES_LOSSLESS"),
            None,
            None,
            900,
        );

        scan(&conn).expect("scan");
        let stats = auto_merge_pending(&conn).expect("auto merge");

        assert_eq!(stats.merged_groups, 0);
        assert_eq!(stats.skipped_groups, 1);
        let both: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(both, 2);
    }

    #[test]
    fn auto_merge_all_liked_group_keeps_recording_liked() {
        let conn = test_conn();
        insert_track_full(
            &conn,
            1,
            "Song",
            200_000,
            Some("ISRC1"),
            None,
            None,
            None,
            None,
            700,
        );
        insert_track_full(
            &conn,
            2,
            "Song",
            200_400,
            Some("ISRC1"),
            None,
            None,
            None,
            None,
            900,
        );
        conn.execute("UPDATE tracks SET is_favorite = 1 WHERE id IN (1, 2)", [])
            .unwrap();

        scan(&conn).expect("scan");
        let stats = auto_merge_pending(&conn).expect("auto merge");

        assert_eq!(stats.merged_groups, 1);
        // Higher fidelity liked row won; the recording is still liked.
        let (kept, fav): (i64, i32) = conn
            .query_row("SELECT id, is_favorite FROM tracks", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kept, 2);
        assert_eq!(fav, 1);
    }
}
