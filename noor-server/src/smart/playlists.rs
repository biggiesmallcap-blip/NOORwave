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
    }
}

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
        }
    }
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
}
