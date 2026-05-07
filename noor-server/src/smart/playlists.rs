use crate::db::models::Track;
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Boolean operator for combining child clauses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogicOp {
    And,
    Or,
}

/// Comparison operators for numeric conditions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NumberOp {
    Eq,
    Gte,
    Lte,
    Gt,
    Lt,
    BetweenInclusive,
}

/// Quality buckets used by smart playlist rules.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    Lossy,
    Lossless,
    HiRes,
}

impl QualityTier {
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match normalized.as_str() {
            "lossy" | "aac" | "mp3" | "high" | "low" => Some(Self::Lossy),
            "lossless" | "cd" | "flac" => Some(Self::Lossless),
            "hi_res" | "hires" | "max" | "master" => Some(Self::HiRes),
            _ => None,
        }
    }
}

/// Date field available on tracks for time-based rules.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DateField {
    DateAdded,
    LastPlayedAt,
}

/// A date range with inclusive boundaries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DateRange {
    pub start: Option<String>,
    pub end: Option<String>,
}

/// A single smart playlist condition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleClause {
    Group {
        op: LogicOp,
        clauses: Vec<RuleClause>,
    },
    Genre {
        names: Vec<String>,
        #[serde(default)]
        match_descendants: bool,
    },
    Artist {
        names: Vec<String>,
    },
    DateRange {
        field: DateField,
        range: DateRange,
    },
    PlayCount {
        op: NumberOp,
        value: i32,
        value_max: Option<i32>,
    },
    Quality {
        minimum: QualityTier,
    },
    NotInPlaylist {
        playlist_ids: Vec<i64>,
    },
    BpmRange {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
    KeySignature {
        key: String,
    },
    CamelotKey {
        key: String,
    },
    EnergyRange {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
    DanceabilityRange {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
    InstrumentalOnly {
        is_instrumental: bool,
    },
    HasSampleData {
        #[serde(default)]
        source: Option<String>,
    },
}

/// DSP feature values for a single track, as stored in `audio_dsp_features`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackDspFeatures {
    pub bpm: Option<f64>,
    pub key_signature: Option<String>,
    pub camelot_key: Option<String>,
    pub energy: Option<f64>,
    pub danceability: Option<f64>,
    pub is_instrumental: bool,
}

/// Sample-match source for a track (from `acrcloud_results` or fingerprint tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleDataSource {
    Acrcloud,
    Fingerprint,
}

impl SampleDataSource {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "acrcloud" | "acr_cloud" | "acr" => Some(Self::Acrcloud),
            "fingerprint" | "fp" => Some(Self::Fingerprint),
            _ => None,
        }
    }
}

/// Serialized smart playlist definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartPlaylistDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub root: RuleClause,
}

/// Extra state needed to evaluate rules that aren't embedded in `Track`.
#[derive(Debug, Clone, Default)]
pub struct PlaylistEvaluationContext {
    genres_by_track: HashMap<i64, HashSet<String>>,
    tracks_by_playlist: HashMap<i64, HashSet<i64>>,
    dsp_by_track: HashMap<i64, TrackDspFeatures>,
    sample_sources_by_track: HashMap<i64, HashSet<SampleDataSource>>,
}

impl PlaylistEvaluationContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_track_genres<I, S>(mut self, track_id: i64, genres: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let entry = self.genres_by_track.entry(track_id).or_default();
        for genre in genres {
            entry.insert(normalize_name(&genre.into()));
        }
        self
    }

    pub fn with_playlist_tracks<I>(mut self, playlist_id: i64, track_ids: I) -> Self
    where
        I: IntoIterator<Item = i64>,
    {
        self.tracks_by_playlist
            .insert(playlist_id, track_ids.into_iter().collect());
        self
    }

    pub fn genres_for_track(&self, track_id: i64) -> Option<&HashSet<String>> {
        self.genres_by_track.get(&track_id)
    }

    pub fn playlist_contains_track(&self, playlist_id: i64, track_id: i64) -> bool {
        self.tracks_by_playlist
            .get(&playlist_id)
            .is_some_and(|track_ids| track_ids.contains(&track_id))
    }

    /// Attach DSP features (joined from `audio_dsp_features`) for a track.
    pub fn with_track_dsp(mut self, track_id: i64, dsp: TrackDspFeatures) -> Self {
        self.dsp_by_track.insert(track_id, dsp);
        self
    }

    /// Mark that a track has sample-match data from the given source
    /// (`"acrcloud"` from `acrcloud_results`, `"fingerprint"` from the fingerprint tables).
    pub fn with_sample_source<S: AsRef<str>>(mut self, track_id: i64, source: S) -> Self {
        if let Some(parsed) = SampleDataSource::parse(source.as_ref()) {
            self.sample_sources_by_track
                .entry(track_id)
                .or_default()
                .insert(parsed);
        }
        self
    }

    pub fn dsp_for_track(&self, track_id: i64) -> Option<&TrackDspFeatures> {
        self.dsp_by_track.get(&track_id)
    }

    pub fn has_sample_source(&self, track_id: i64, source: SampleDataSource) -> bool {
        self.sample_sources_by_track
            .get(&track_id)
            .is_some_and(|sources| sources.contains(&source))
    }

    pub fn has_any_sample_source(&self, track_id: i64) -> bool {
        self.sample_sources_by_track
            .get(&track_id)
            .is_some_and(|sources| !sources.is_empty())
    }
}

/// Filters tracks using a smart playlist definition.
pub fn evaluate_playlist<'a>(
    definition: &SmartPlaylistDefinition,
    tracks: &'a [Track],
    context: &PlaylistEvaluationContext,
) -> Vec<&'a Track> {
    tracks
        .iter()
        .filter(|track| definition.root.matches(track, context))
        .collect()
}

/// Render a smart playlist definition into human-readable summary lines.
#[allow(dead_code)]
pub fn summarize_definition(definition: &SmartPlaylistDefinition) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(description) = definition
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push(format!("Description: {}", description));
    }
    push_clause_summary(&definition.root, 0, &mut lines);
    lines
}

/// Render a single rule clause into a concise label.
#[allow(dead_code)]
pub fn summarize_clause(clause: &RuleClause) -> String {
    match clause {
        RuleClause::Group { op, clauses } => {
            let op_label = match op {
                LogicOp::And => "all of",
                LogicOp::Or => "any of",
            };
            format!("{op_label} {} rule(s)", clauses.len())
        }
        RuleClause::Genre {
            names,
            match_descendants,
        } => {
            let suffix = if *match_descendants {
                "with descendants"
            } else {
                "exact match"
            };
            format!("genre is {} ({suffix})", names.join(", "))
        }
        RuleClause::Artist { names } => format!("artist is {}", names.join(", ")),
        RuleClause::DateRange { field, range } => {
            let field_label = match field {
                DateField::DateAdded => "date added",
                DateField::LastPlayedAt => "last played",
            };
            let start = range.start.as_deref().unwrap_or("start");
            let end = range.end.as_deref().unwrap_or("now");
            format!("{field_label} between {start} and {end}")
        }
        RuleClause::PlayCount {
            op,
            value,
            value_max,
        } => {
            let op_label = match op {
                NumberOp::Eq => "=",
                NumberOp::Gte => ">=",
                NumberOp::Lte => "<=",
                NumberOp::Gt => ">",
                NumberOp::Lt => "<",
                NumberOp::BetweenInclusive => "between",
            };
            if matches!(op, NumberOp::BetweenInclusive) {
                format!(
                    "play count between {} and {}",
                    value,
                    value_max.unwrap_or(*value)
                )
            } else {
                format!("play count {op_label} {value}")
            }
        }
        RuleClause::Quality { minimum } => format!("minimum quality {:?}", minimum),
        RuleClause::NotInPlaylist { playlist_ids } => {
            format!("exclude tracks already in playlists {:?}", playlist_ids)
        }
        RuleClause::BpmRange { min, max } => {
            format!(
                "bpm between {} and {}",
                min.map(|v| format!("{:.0}", v)).unwrap_or("any".into()),
                max.map(|v| format!("{:.0}", v)).unwrap_or("any".into()),
            )
        }
        RuleClause::KeySignature { key } => format!("key is {}", key),
        RuleClause::CamelotKey { key } => format!("camelot key is {}", key),
        RuleClause::EnergyRange { min, max } => {
            format!(
                "energy between {} and {}",
                min.map(|v| format!("{:.2}", v)).unwrap_or("any".into()),
                max.map(|v| format!("{:.2}", v)).unwrap_or("any".into()),
            )
        }
        RuleClause::DanceabilityRange { min, max } => {
            format!(
                "danceability between {} and {}",
                min.map(|v| format!("{:.2}", v)).unwrap_or("any".into()),
                max.map(|v| format!("{:.2}", v)).unwrap_or("any".into()),
            )
        }
        RuleClause::InstrumentalOnly { is_instrumental } => {
            if *is_instrumental {
                "instrumental tracks only".into()
            } else {
                "vocal tracks only".into()
            }
        }
        RuleClause::HasSampleData { source } => match source.as_deref() {
            Some(s) => format!("has sample data ({})", s),
            None => "has any sample data".into(),
        },
    }
}

#[allow(dead_code)]
fn push_clause_summary(clause: &RuleClause, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    match clause {
        RuleClause::Group { op, clauses } => {
            let label = match op {
                LogicOp::And => "All of:",
                LogicOp::Or => "Any of:",
            };
            lines.push(format!("{indent}{label}"));
            for child in clauses {
                push_clause_summary(child, depth + 1, lines);
            }
        }
        _ => lines.push(format!("{indent}{}", summarize_clause(clause))),
    }
}

impl RuleClause {
    pub fn matches(&self, track: &Track, context: &PlaylistEvaluationContext) -> bool {
        match self {
            Self::Group { op, clauses } => match op {
                LogicOp::And => clauses.iter().all(|clause| clause.matches(track, context)),
                LogicOp::Or => clauses.iter().any(|clause| clause.matches(track, context)),
            },
            Self::Genre {
                names,
                match_descendants,
            } => genre_matches(track, names, *match_descendants, context),
            Self::Artist { names } => {
                let artist = track.artist_name.as_deref().unwrap_or("").trim();
                !artist.is_empty()
                    && names
                        .iter()
                        .map(|name| normalize_name(name))
                        .any(|candidate| normalize_name(artist) == candidate)
            }
            Self::DateRange { field, range } => date_matches(track, *field, range),
            Self::PlayCount {
                op,
                value,
                value_max,
            } => compare_number(track.play_count, *op, *value, *value_max),
            Self::Quality { minimum } => track
                .best_quality
                .as_deref()
                .and_then(QualityTier::parse)
                .is_some_and(|quality| quality >= *minimum),
            Self::NotInPlaylist { playlist_ids } => playlist_ids
                .iter()
                .all(|playlist_id| !context.playlist_contains_track(*playlist_id, track.id)),
            Self::BpmRange { min, max } => context
                .dsp_for_track(track.id)
                .and_then(|dsp| dsp.bpm)
                .is_some_and(|bpm| in_range(bpm, *min, *max)),
            Self::KeySignature { key } => {
                let Some(dsp) = context.dsp_for_track(track.id) else {
                    return false;
                };
                key_matches(
                    key,
                    dsp.key_signature.as_deref(),
                    dsp.camelot_key.as_deref(),
                )
            }
            Self::CamelotKey { key } => {
                let Some(dsp) = context.dsp_for_track(track.id) else {
                    return false;
                };
                key_matches(
                    key,
                    dsp.key_signature.as_deref(),
                    dsp.camelot_key.as_deref(),
                )
            }
            Self::EnergyRange { min, max } => context
                .dsp_for_track(track.id)
                .and_then(|dsp| dsp.energy)
                .is_some_and(|v| in_range(v, *min, *max)),
            Self::DanceabilityRange { min, max } => context
                .dsp_for_track(track.id)
                .and_then(|dsp| dsp.danceability)
                .is_some_and(|v| in_range(v, *min, *max)),
            Self::InstrumentalOnly { is_instrumental } => context
                .dsp_for_track(track.id)
                .is_some_and(|dsp| dsp.is_instrumental == *is_instrumental),
            Self::HasSampleData { source } => {
                match source.as_deref().and_then(SampleDataSource::parse) {
                    Some(src) => context.has_sample_source(track.id, src),
                    None => context.has_any_sample_source(track.id),
                }
            }
        }
    }
}

fn in_range(value: f64, min: Option<f64>, max: Option<f64>) -> bool {
    min.is_none_or(|lo| value >= lo) && max.is_none_or(|hi| value <= hi)
}

// ─── Key normalization ──────────────────────────────────────────────────────
//
// Accepts both classic key-signature notation ("Am", "C", "F#m", "Cmaj") and
// Camelot notation ("8A", "12B"). Returns the canonical Camelot representation
// so comparisons work regardless of which format the user entered or stored.

// Camelot tables mirror `services::audio_analysis::key` so values stored in
// `audio_dsp_features.camelot_key` round-trip with user input.
const MAJOR_CAMELOT_PAIRS: [(&str, &str); 17] = [
    ("c", "8B"),
    ("c#", "9B"),
    ("db", "9B"),
    ("d", "10B"),
    ("d#", "11B"),
    ("eb", "11B"),
    ("e", "12B"),
    ("f", "1B"),
    ("f#", "2B"),
    ("gb", "2B"),
    ("g", "3B"),
    ("g#", "4B"),
    ("ab", "4B"),
    ("a", "5B"),
    ("a#", "6B"),
    ("bb", "6B"),
    ("b", "7B"),
];

const MINOR_CAMELOT_PAIRS: [(&str, &str); 17] = [
    ("c", "8A"),
    ("c#", "9A"),
    ("db", "9A"),
    ("d", "10A"),
    ("d#", "11A"),
    ("eb", "11A"),
    ("e", "12A"),
    ("f", "1A"),
    ("f#", "2A"),
    ("gb", "2A"),
    ("g", "3A"),
    ("g#", "4A"),
    ("ab", "4A"),
    ("a", "5A"),
    ("a#", "6A"),
    ("bb", "6A"),
    ("b", "7A"),
];

/// Normalize either a key signature ("Am", "C#maj", "F#m") or a Camelot key
/// ("8A", "12B") into canonical Camelot form. Returns `None` for unrecognized
/// input so callers can fall back gracefully.
pub fn normalize_to_camelot(key: &str) -> Option<String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Already Camelot? Pattern: 1-2 digits followed by A or B.
    let upper = trimmed.to_ascii_uppercase();
    if upper.len() >= 2 && (upper.ends_with('A') || upper.ends_with('B')) {
        let num_part = &upper[..upper.len() - 1];
        if let Ok(n) = num_part.parse::<u32>()
            && (1..=12).contains(&n) {
                return Some(upper);
            }
    }

    // Key signature. Strip optional "maj"/"major"/"min"/"minor"/"m".
    let lower = trimmed.to_ascii_lowercase().replace(' ', "");
    let (note_part, is_minor) = if let Some(stripped) = lower.strip_suffix("maj") {
        (stripped.to_string(), false)
    } else if let Some(stripped) = lower.strip_suffix("major") {
        (stripped.to_string(), false)
    } else if let Some(stripped) = lower.strip_suffix("minor") {
        (stripped.to_string(), true)
    } else if let Some(stripped) = lower.strip_suffix("min") {
        (stripped.to_string(), true)
    } else if let Some(stripped) = lower.strip_suffix('m') {
        // Guard: "cm" -> C minor, but we must not strip 'm' off something like "bm"
        // accidentally — the suffix is still the minor marker here.
        (stripped.to_string(), true)
    } else {
        (lower, false)
    };

    let table: &[(&str, &str)] = if is_minor {
        &MINOR_CAMELOT_PAIRS[..]
    } else {
        &MAJOR_CAMELOT_PAIRS[..]
    };

    for (note, camelot) in table.iter() {
        if *note == note_part {
            return Some((*camelot).to_string());
        }
    }

    None
}

/// True if the requested key matches either the stored key signature or Camelot key,
/// once both have been normalized to canonical Camelot form.
fn key_matches(requested: &str, stored_key: Option<&str>, stored_camelot: Option<&str>) -> bool {
    let Some(target) = normalize_to_camelot(requested) else {
        return false;
    };
    let from_camelot = stored_camelot.and_then(normalize_to_camelot);
    let from_signature = stored_key.and_then(normalize_to_camelot);
    from_camelot.as_deref() == Some(target.as_str())
        || from_signature.as_deref() == Some(target.as_str())
}

fn genre_matches(
    track: &Track,
    names: &[String],
    match_descendants: bool,
    context: &PlaylistEvaluationContext,
) -> bool {
    let Some(genres) = context.genres_for_track(track.id) else {
        return false;
    };

    names
        .iter()
        .map(|name| normalize_name(name))
        .any(|requested| {
            genres.iter().any(|genre| {
                if match_descendants {
                    genre == &requested || genre.starts_with(&(requested.clone() + " > "))
                } else {
                    genre == &requested
                }
            })
        })
}

fn date_matches(track: &Track, field: DateField, range: &DateRange) -> bool {
    let value = match field {
        DateField::DateAdded => track.date_added.as_deref(),
        DateField::LastPlayedAt => track.last_played_at.as_deref(),
    };

    let Some(date) = value.and_then(parse_date) else {
        return false;
    };

    let start_ok = range
        .start
        .as_deref()
        .and_then(parse_date)
        .is_none_or(|start| date >= start);
    let end_ok = range
        .end
        .as_deref()
        .and_then(parse_date)
        .is_none_or(|end| date <= end);

    start_ok && end_ok
}

fn compare_number(value: i32, op: NumberOp, expected: i32, max: Option<i32>) -> bool {
    match op {
        NumberOp::Eq => value == expected,
        NumberOp::Gte => value >= expected,
        NumberOp::Lte => value <= expected,
        NumberOp::Gt => value > expected,
        NumberOp::Lt => value < expected,
        NumberOp::BetweenInclusive => max.is_some_and(|upper| value >= expected && value <= upper),
    }
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.date())
        .or_else(|_| NaiveDate::parse_from_str(value, "%Y-%m-%d"))
        .ok()
}

fn normalize_name(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(
        id: i64,
        title: &str,
        artist_name: &str,
        play_count: i32,
        best_quality: Option<&str>,
        date_added: Option<&str>,
        last_played_at: Option<&str>,
    ) -> Track {
        Track {
            id,
            title: title.to_string(),
            artist_id: id * 10,
            artist_name: Some(artist_name.to_string()),
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: None,
            isrc: None,
            tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: best_quality.map(str::to_string),
            best_source: None,
            fidelity_score: 0,
            is_favorite: false,
            play_count,
            last_played_at: last_played_at.map(str::to_string),
            date_added: date_added.map(str::to_string),
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    #[test]
    fn supports_nested_and_or_logic() {
        let tracks = vec![
            track(
                1,
                "Teardrop",
                "Massive Attack",
                12,
                Some("LOSSLESS"),
                Some("2025-03-10"),
                None,
            ),
            track(
                2,
                "Angel",
                "Massive Attack",
                4,
                Some("AAC"),
                Some("2025-03-11"),
                None,
            ),
            track(
                3,
                "Windowlicker",
                "Aphex Twin",
                20,
                Some("HI_RES"),
                Some("2025-03-12"),
                None,
            ),
        ];

        let context = PlaylistEvaluationContext::new()
            .with_track_genres(1, ["Electronic > Trip-Hop"])
            .with_track_genres(2, ["Electronic > Trip-Hop"])
            .with_track_genres(3, ["Electronic > IDM"]);

        let definition = SmartPlaylistDefinition {
            name: "Late Night".into(),
            description: None,
            root: RuleClause::Group {
                op: LogicOp::And,
                clauses: vec![
                    RuleClause::Group {
                        op: LogicOp::Or,
                        clauses: vec![
                            RuleClause::Genre {
                                names: vec!["Electronic > Trip-Hop".into()],
                                match_descendants: false,
                            },
                            RuleClause::Artist {
                                names: vec!["Aphex Twin".into()],
                            },
                        ],
                    },
                    RuleClause::PlayCount {
                        op: NumberOp::Gte,
                        value: 10,
                        value_max: None,
                    },
                ],
            },
        };

        let results = evaluate_playlist(&definition, &tracks, &context);
        let ids: Vec<i64> = results.into_iter().map(|track| track.id).collect();
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn date_range_and_quality_rules_are_inclusive() {
        let tracks = vec![
            track(
                1,
                "One",
                "Artist A",
                1,
                Some("LOSSLESS"),
                Some("2025-01-01"),
                Some("2025-04-01 08:30:00"),
            ),
            track(
                2,
                "Two",
                "Artist B",
                1,
                Some("AAC"),
                Some("2024-12-31"),
                Some("2025-04-02"),
            ),
        ];

        let definition = SmartPlaylistDefinition {
            name: "Fresh Lossless".into(),
            description: None,
            root: RuleClause::Group {
                op: LogicOp::And,
                clauses: vec![
                    RuleClause::DateRange {
                        field: DateField::DateAdded,
                        range: DateRange {
                            start: Some("2025-01-01".into()),
                            end: Some("2025-12-31".into()),
                        },
                    },
                    RuleClause::Quality {
                        minimum: QualityTier::Lossless,
                    },
                ],
            },
        };

        let results = evaluate_playlist(&definition, &tracks, &PlaylistEvaluationContext::new());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, 1);
    }

    #[test]
    fn not_in_playlist_excludes_existing_membership() {
        let tracks = vec![
            track(
                1,
                "A",
                "Artist A",
                0,
                Some("LOSSLESS"),
                Some("2025-01-01"),
                None,
            ),
            track(
                2,
                "B",
                "Artist B",
                0,
                Some("LOSSLESS"),
                Some("2025-01-01"),
                None,
            ),
        ];

        let context = PlaylistEvaluationContext::new().with_playlist_tracks(42, [2]);
        let definition = SmartPlaylistDefinition {
            name: "New Picks".into(),
            description: None,
            root: RuleClause::NotInPlaylist {
                playlist_ids: vec![42],
            },
        };

        let results = evaluate_playlist(&definition, &tracks, &context);
        let ids: Vec<i64> = results.into_iter().map(|track| track.id).collect();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn descendant_genre_matching_uses_hierarchical_paths() {
        let tracks = vec![track(
            1,
            "Teardrop",
            "Massive Attack",
            0,
            Some("LOSSLESS"),
            Some("2025-01-01"),
            None,
        )];

        let context = PlaylistEvaluationContext::new()
            .with_track_genres(1, ["Electronic > Downtempo > Trip-Hop"]);
        let definition = SmartPlaylistDefinition {
            name: "Descendant Genres".into(),
            description: None,
            root: RuleClause::Genre {
                names: vec!["Electronic".into()],
                match_descendants: true,
            },
        };

        let results = evaluate_playlist(&definition, &tracks, &context);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, 1);
    }

    #[test]
    fn summarizes_nested_rules_for_ui() {
        let definition = SmartPlaylistDefinition {
            name: "Late Night".into(),
            description: Some("Darker cuts and deep dives".into()),
            root: RuleClause::Group {
                op: LogicOp::And,
                clauses: vec![
                    RuleClause::Genre {
                        names: vec!["Electronic".into()],
                        match_descendants: true,
                    },
                    RuleClause::PlayCount {
                        op: NumberOp::Gte,
                        value: 10,
                        value_max: None,
                    },
                ],
            },
        };

        let lines = summarize_definition(&definition);
        assert_eq!(lines[0], "Description: Darker cuts and deep dives");
        assert!(lines.iter().any(|line| line.contains("All of:")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("genre is Electronic (with descendants)"))
        );
        assert!(lines.iter().any(|line| line.contains("play count >= 10")));
    }

    #[test]
    fn normalize_key_accepts_both_styles() {
        assert_eq!(normalize_to_camelot("Am").as_deref(), Some("5A"));
        assert_eq!(normalize_to_camelot("a minor").as_deref(), Some("5A"));
        assert_eq!(normalize_to_camelot("Cm").as_deref(), Some("8A"));
        assert_eq!(normalize_to_camelot("C").as_deref(), Some("8B"));
        assert_eq!(normalize_to_camelot("Cmaj").as_deref(), Some("8B"));
        assert_eq!(normalize_to_camelot("8B").as_deref(), Some("8B"));
        assert_eq!(normalize_to_camelot("5a").as_deref(), Some("5A"));
        assert_eq!(normalize_to_camelot("F#m").as_deref(), Some("2A"));
        assert!(normalize_to_camelot("bogus").is_none());
    }

    fn dsp_track(id: i64) -> Track {
        track(id, "T", "A", 0, Some("LOSSLESS"), Some("2025-01-01"), None)
    }

    #[test]
    fn dsp_rules_filter_by_bpm_and_energy() {
        let tracks = vec![dsp_track(1), dsp_track(2), dsp_track(3)];
        let context = PlaylistEvaluationContext::new()
            .with_track_dsp(
                1,
                TrackDspFeatures {
                    bpm: Some(128.0),
                    energy: Some(0.85),
                    ..Default::default()
                },
            )
            .with_track_dsp(
                2,
                TrackDspFeatures {
                    bpm: Some(90.0),
                    energy: Some(0.4),
                    ..Default::default()
                },
            );

        let definition = SmartPlaylistDefinition {
            name: "Peak time".into(),
            description: None,
            root: RuleClause::Group {
                op: LogicOp::And,
                clauses: vec![
                    RuleClause::BpmRange {
                        min: Some(120.0),
                        max: Some(135.0),
                    },
                    RuleClause::EnergyRange {
                        min: Some(0.7),
                        max: None,
                    },
                ],
            },
        };

        let ids: Vec<i64> = evaluate_playlist(&definition, &tracks, &context)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn key_rule_matches_across_notation_styles() {
        let tracks = vec![dsp_track(1), dsp_track(2)];

        // Request matching track 1 by Camelot when track 1 stores "Am"/"5A".
        let ctx2 = PlaylistEvaluationContext::new().with_track_dsp(
            1,
            TrackDspFeatures {
                key_signature: Some("Am".into()),
                camelot_key: Some("5A".into()),
                ..Default::default()
            },
        );

        let def = SmartPlaylistDefinition {
            name: "Am only".into(),
            description: None,
            root: RuleClause::KeySignature { key: "5A".into() },
        };
        let ids: Vec<i64> = evaluate_playlist(&def, &tracks, &ctx2)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![1]);

        // Camelot-form request, key-signature storage only — still matches via normalization.
        let ctx3 = PlaylistEvaluationContext::new().with_track_dsp(
            1,
            TrackDspFeatures {
                key_signature: Some("Am".into()),
                camelot_key: None,
                ..Default::default()
            },
        );
        let def2 = SmartPlaylistDefinition {
            name: "Am only".into(),
            description: None,
            root: RuleClause::CamelotKey { key: "5A".into() },
        };
        let ids2: Vec<i64> = evaluate_playlist(&def2, &tracks, &ctx3)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids2, vec![1]);

        // Plain-text "Am" request matches track 1 too.
        let def3 = SmartPlaylistDefinition {
            name: "Am only".into(),
            description: None,
            root: RuleClause::KeySignature { key: "Am".into() },
        };
        let ids3: Vec<i64> = evaluate_playlist(&def3, &tracks, &ctx3)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids3, vec![1]);
    }

    #[test]
    fn instrumental_and_sample_data_rules() {
        let tracks = vec![dsp_track(1), dsp_track(2), dsp_track(3)];
        let context = PlaylistEvaluationContext::new()
            .with_track_dsp(
                1,
                TrackDspFeatures {
                    is_instrumental: true,
                    ..Default::default()
                },
            )
            .with_track_dsp(
                2,
                TrackDspFeatures {
                    is_instrumental: false,
                    ..Default::default()
                },
            )
            .with_sample_source(1, "acrcloud")
            .with_sample_source(2, "fingerprint");

        let inst_def = SmartPlaylistDefinition {
            name: "Instrumentals".into(),
            description: None,
            root: RuleClause::InstrumentalOnly {
                is_instrumental: true,
            },
        };
        let ids: Vec<i64> = evaluate_playlist(&inst_def, &tracks, &context)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![1]);

        let acr_def = SmartPlaylistDefinition {
            name: "ACR".into(),
            description: None,
            root: RuleClause::HasSampleData {
                source: Some("acrcloud".into()),
            },
        };
        let ids2: Vec<i64> = evaluate_playlist(&acr_def, &tracks, &context)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids2, vec![1]);

        let any_def = SmartPlaylistDefinition {
            name: "Any sample".into(),
            description: None,
            root: RuleClause::HasSampleData { source: None },
        };
        let ids3: Vec<i64> = evaluate_playlist(&any_def, &tracks, &context)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids3, vec![1, 2]);
    }
}
