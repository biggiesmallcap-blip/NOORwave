use crate::genre::mappings::GenreCatalog;
use std::collections::HashMap;

/// Minimum score a tag must clear to be persisted to `track_genres`.
///
/// Originally 0.2. Raised to that level when the May-4 source-aware scorer
/// shipped and we wanted to be aggressive about filtering noise. The
/// 2026-05-07 audit found it was too aggressive: artist-level Last.fm tags
/// max out at `LastFmArtist × Artist = 0.4 × 0.45 = 0.18`, so they were
/// mathematically incapable of clearing 0.2 even at count=100. This dropped
/// the dominant signal for any artist whose tagging is concentrated at the
/// artist level (Khruangbin's `funk@100`, similar across many artists).
///
/// At 0.15 the artist-level ceiling clears the floor, restoring those
/// signals without re-admitting the lower-count noise tags (count<10
/// artist-level tags still fall under 0.10 × 0.18 ≈ 0.05).
pub const MIN_SCORE_FLOOR: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum TagSource {
    MusicBrainzGenre,
    MusicBrainzTag,
    LastFmTrack,
    LastFmAlbum,
    LastFmArtist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum TagLevel {
    Recording,
    Release,
    ReleaseGroup,
    Artist,
}

#[derive(Debug, Clone)]
pub struct TagInput {
    pub name: String,
    pub source: TagSource,
    pub level: TagLevel,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredGenre {
    pub canonical: String,
    pub score: f64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GenreScoreResult {
    pub primary: Option<String>,
    pub genres: Vec<ScoredGenre>,
    pub confidence: f64,
}

pub fn source_weight(source: TagSource, level: TagLevel) -> f64 {
    let source_weight = match source {
        TagSource::MusicBrainzGenre => 1.0,
        TagSource::MusicBrainzTag => 0.75,
        TagSource::LastFmTrack => 0.7,
        TagSource::LastFmAlbum => 0.55,
        TagSource::LastFmArtist => 0.4,
    };
    let level_weight = match level {
        TagLevel::Recording => 1.0,
        TagLevel::Release => 0.8,
        TagLevel::ReleaseGroup => 0.7,
        TagLevel::Artist => 0.45,
    };
    source_weight * level_weight
}

/// Floor for the count-normalization denominator. Tag counts are scored relative
/// to the busiest tag of the same (source, level) on the SAME item, so a count of
/// 50 beside a count of 100 still reads as strong. On a sparsely-tagged item that
/// backfires: when the busiest tag is itself a single vote, a lone count=1 noise
/// tag divides by itself and scores a full 1.0 - one stray user tagging an
/// XXXTENTACION recording "jazz" became a max-confidence genre. Flooring the
/// denominator means absolute weakness can't be normalized away: one vote stays
/// weak (~0.25) no matter how bare the item is, while well-tagged items (busiest
/// count >= this) are unaffected. At 15, count=1 -> ln2/ln16 ~= 0.25, count=15 -> 1.0.
const COUNT_SATURATION: u32 = 15;

pub fn confidence_from_count(count: Option<u32>, max_count: u32) -> f64 {
    match count {
        None => 0.6,
        Some(n) => {
            let denom = max_count.max(COUNT_SATURATION);
            ((n as f64).ln_1p() / (denom as f64).ln_1p()).min(1.0)
        }
    }
}

fn suppress_parents(scores: &HashMap<String, f64>, catalog: &GenreCatalog) -> HashMap<String, f64> {
    let mut result = scores.clone();
    for (genre, &child_score) in scores {
        let Some(path) = catalog.path_for(genre) else {
            continue;
        };
        if path.len() < 2 {
            continue;
        }
        let parent = &path[path.len() - 2];
        let Some(&parent_score) = scores.get(parent) else {
            continue;
        };
        if child_score >= parent_score * 0.55 {
            result.insert(parent.clone(), parent_score * 0.35);
        }
    }
    result
}

pub fn score_genre_tags(inputs: &[TagInput], min_score: f64) -> GenreScoreResult {
    let catalog = crate::genre::builder::embedded_builder().catalog();
    let mut max_by_source: HashMap<(TagSource, TagLevel), u32> = HashMap::new();
    for input in inputs {
        if let Some(count) = input.count {
            let entry = max_by_source
                .entry((input.source, input.level))
                .or_insert(0);
            if count > *entry {
                *entry = count;
            }
        }
    }

    let mut raw: HashMap<String, f64> = HashMap::new();
    for input in inputs {
        let Some(matched) = catalog.resolve_single(&input.name) else {
            continue;
        };
        let max_count = max_by_source
            .get(&(input.source, input.level))
            .copied()
            .unwrap_or(100);
        *raw.entry(matched.canonical_name).or_insert(0.0) +=
            source_weight(input.source, input.level)
                * confidence_from_count(input.count, max_count);
    }

    let adjusted = suppress_parents(&raw, catalog);
    let mut ranked: Vec<ScoredGenre> = adjusted
        .into_iter()
        .filter(|(_, score)| *score >= min_score)
        .map(|(canonical, score)| ScoredGenre { canonical, score })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.canonical.cmp(&right.canonical))
    });

    if ranked.is_empty() {
        return GenreScoreResult {
            primary: None,
            genres: Vec::new(),
            confidence: 0.0,
        };
    }

    let top = ranked[0].score;
    let second = ranked.get(1).map(|genre| genre.score).unwrap_or(0.0);
    let ambiguous = second > 0.0 && top / second < 1.2;

    GenreScoreResult {
        primary: if ambiguous {
            None
        } else {
            Some(ranked[0].canonical.clone())
        },
        genres: ranked,
        confidence: top / (top + second + 1e-6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, source: TagSource, level: TagLevel, count: Option<u32>) -> TagInput {
        TagInput {
            name: name.to_string(),
            source,
            level,
            count,
        }
    }

    #[test]
    fn source_weight_orders_sources_and_levels() {
        assert!(
            source_weight(TagSource::MusicBrainzTag, TagLevel::Recording)
                > source_weight(TagSource::LastFmTrack, TagLevel::Recording)
        );
        assert!(
            source_weight(TagSource::LastFmTrack, TagLevel::Recording)
                > source_weight(TagSource::LastFmArtist, TagLevel::Artist)
        );
    }

    #[test]
    fn count_confidence_is_log_scaled() {
        let low = confidence_from_count(Some(1), 100);
        let mid = confidence_from_count(Some(10), 100);
        let high = confidence_from_count(Some(100), 100);
        assert!(low < mid);
        assert!(mid < high);
        assert_eq!(high, 1.0);
    }

    #[test]
    fn missing_count_is_neutral() {
        assert_eq!(confidence_from_count(None, 100), 0.6);
    }

    #[test]
    fn lone_vote_on_sparse_item_is_not_full_confidence() {
        // Regression: before the saturation floor, count=1 with max_count=1 scored
        // 1.0, so a single stray MusicBrainz vote ("jazz" on an emo-rap recording)
        // became a full-confidence genre. It must stay weak even when it is the
        // only tag on the item.
        let lone = confidence_from_count(Some(1), 1);
        assert!(
            lone < 0.35,
            "a single-vote tag must stay weak when it's the only tag, got {lone}"
        );
        // The floor only protects sparse items; a genuinely well-supported tag
        // still saturates to 1.0.
        assert_eq!(confidence_from_count(Some(COUNT_SATURATION), 1), 1.0);
        // Well-tagged items are unchanged: a busy max already discounted count=1.
        assert!(confidence_from_count(Some(1), 100) < lone + 1e-9);
    }

    #[test]
    fn child_genre_suppresses_parent() {
        let result = score_genre_tags(
            &[
                input(
                    "house",
                    TagSource::LastFmTrack,
                    TagLevel::Recording,
                    Some(100),
                ),
                input(
                    "deep house",
                    TagSource::LastFmTrack,
                    TagLevel::Recording,
                    Some(80),
                ),
            ],
            0.0,
        );
        let deep_house = result
            .genres
            .iter()
            .position(|genre| genre.canonical == "Deep House")
            .unwrap();
        let house = result
            .genres
            .iter()
            .position(|genre| genre.canonical == "House")
            .unwrap();
        assert!(deep_house < house);
    }

    #[test]
    fn recording_level_beats_artist_level() {
        let result = score_genre_tags(
            &[
                input(
                    "reggae",
                    TagSource::LastFmTrack,
                    TagLevel::Recording,
                    Some(20),
                ),
                input(
                    "classic rock",
                    TagSource::LastFmArtist,
                    TagLevel::Artist,
                    Some(100),
                ),
            ],
            0.0,
        );
        assert_eq!(result.genres[0].canonical, "Reggae");
    }

    #[test]
    fn unresolved_tags_are_dropped_and_empty_returns_no_result() {
        let result = score_genre_tags(
            &[input(
                "zzzfakegenrexyz",
                TagSource::LastFmTrack,
                TagLevel::Recording,
                Some(10),
            )],
            0.2,
        );
        assert!(result.genres.is_empty());
        assert_eq!(result.primary, None);
        assert_eq!(result.confidence, 0.0);
    }
}
