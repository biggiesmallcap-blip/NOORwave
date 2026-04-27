use crate::db::models::{
    AnalyticsBehavior, AnalyticsGenreShare, AnalyticsOverview, AnalyticsTopArtist,
    DiscoveryConnectionTrailItem, DiscoveryExternalFeed, DiscoveryExternalResult,
    DiscoveryProfilePreview, DiscoveryProviderCapability, DiscoveryReason, ListenHistoryEntry,
};
use crate::genre::builder::embedded_builder;
use crate::services::discovery::{DiscoveryCandidateSeed, DiscoveryCandidateTrack};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct ExternalDiscoveryRequest {
    pub prompt: String,
    pub mode: String,
    pub services: Vec<String>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct ExternalDiscoveryContext {
    pub overview: AnalyticsOverview,
    pub behavior: AnalyticsBehavior,
    pub recent_listens: Vec<ListenHistoryEntry>,
    pub top_artists: Vec<AnalyticsTopArtist>,
    pub top_genres: Vec<AnalyticsGenreShare>,
}

#[derive(Debug, Clone)]
struct TasteMeshProfile {
    artist_affinity: HashMap<String, f64>,
    genre_affinity: HashMap<String, f64>,
    recent_artist_penalty: HashMap<String, f64>,
    completion_bias: f64,
    favorite_bias: f64,
    novelty_bias: f64,
}

struct ScoredDiscoveryResult {
    raw_score: f64,
    result: DiscoveryExternalResult,
}

pub fn build_search_queries(
    request: &ExternalDiscoveryRequest,
    context: &ExternalDiscoveryContext,
) -> Vec<String> {
    let prompt = request.prompt.trim();
    let prompt_terms = tokenize(prompt);
    let prompt_genres = infer_prompt_genres(prompt);
    let descriptor_terms = discovery_terms(&prompt_terms, &prompt_genres);
    let descriptor_phrase = descriptor_terms
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let mut queries = Vec::new();

    if !prompt_genres.is_empty() {
        for genre in prompt_genres.iter().take(3) {
            queries.push(genre.clone());
            if !descriptor_phrase.is_empty() {
                queries.push(format!("{genre} {descriptor_phrase}"));
            }
        }
    }

    if !descriptor_phrase.is_empty() {
        queries.push(descriptor_phrase.clone());
    }

    if prompt_genres.is_empty() || request.mode == "reference" {
        queries.push(prompt.to_string());
    }

    if request.mode == "reference" {
        queries.extend(
            context
                .top_artists
                .iter()
                .take(2)
                .map(|artist| artist.artist_name.clone()),
        );
        if let Some(artist) = context.top_artists.first() {
            queries.push(format!("{} {}", artist.artist_name, prompt));
        }
    }

    if request.mode == "dj" {
        let dj_genre = prompt_genres
            .iter()
            .find(|genre| {
                matches!(
                    genre.as_str(),
                    "House" | "Techno" | "Disco" | "Garage" | "Breakbeat"
                )
            })
            .cloned()
            .unwrap_or_else(|| "House".to_string());
        queries.push(format!("{dj_genre} club"));
        if !descriptor_phrase.is_empty() {
            queries.push(format!("{dj_genre} {descriptor_phrase}"));
        }
    }

    if request.mode == "word-cloud" && prompt_terms.len() >= 2 {
        queries.push(descriptor_terms[..descriptor_terms.len().min(3)].join(" "));
    }

    if prompt_genres.is_empty() {
        queries.extend(
            context
                .top_genres
                .iter()
                .take(2)
                .map(|genre| format!("{prompt} {}", genre.genre_name)),
        );
    }

    dedupe_queries(queries)
}

pub fn build_connection_queries(
    request: &ExternalDiscoveryRequest,
    context: &ExternalDiscoveryContext,
    seed: &DiscoveryCandidateSeed,
) -> Vec<String> {
    let mut queries = Vec::new();
    if let Some(artist) = seed.artist_name.as_deref() {
        queries.push(artist.to_string());
        queries.push(format!("{artist} {}", seed.title));
    }
    if let Some(album) = seed.album_title.as_deref() {
        queries.push(album.to_string());
        queries.push(format!(
            "{} {album}",
            seed.artist_name.as_deref().unwrap_or_default()
        ));
    }
    queries.extend(seed.normalized_genres.iter().take(3).flat_map(|genre| {
        let mut queries = vec![genre.clone()];
        if let Some(artist) = seed.artist_name.as_deref() {
            queries.push(format!("{artist} {genre}"));
        }
        if let Some(album) = seed.album_title.as_deref() {
            queries.push(format!("{album} {genre}"));
        }
        queries
    }));
    if request.mode == "reference" {
        queries.push(format!(
            "{} {}",
            request.prompt.trim(),
            seed.artist_name.as_deref().unwrap_or(seed.title.as_str())
        ));
    }

    if request.prompt.trim().is_empty() {
        dedupe_queries(queries)
    } else {
        let mut blended = build_search_queries(request, context);
        blended.extend(queries);
        dedupe_queries(blended)
    }
}

pub fn build_external_feed(
    request: &ExternalDiscoveryRequest,
    context: &ExternalDiscoveryContext,
    candidates: &[DiscoveryCandidateTrack],
    library_tidal_ids: &HashSet<i64>,
    capabilities: Vec<DiscoveryProviderCapability>,
    trail_item: Option<DiscoveryConnectionTrailItem>,
) -> DiscoveryExternalFeed {
    let prompt_terms = tokenize(&request.prompt);
    let prompt_genres = infer_prompt_genres(&request.prompt);
    let profile = build_profile(request, context, &prompt_terms, &prompt_genres);
    let reasons = build_reasons(&profile, trail_item.as_ref());
    let results = rank_candidates(
        request,
        context,
        candidates,
        library_tidal_ids,
        &prompt_terms,
        &prompt_genres,
    );

    DiscoveryExternalFeed {
        profile,
        reasons,
        results,
        capabilities,
        trail_item,
    }
}

pub fn build_trail_item(
    result: &DiscoveryExternalResult,
    connection_reason: impl Into<String>,
) -> DiscoveryConnectionTrailItem {
    DiscoveryConnectionTrailItem {
        provider: result.provider.clone(),
        provider_track_id: result.provider_track_id.clone(),
        title: result.title.clone(),
        artist_name: result.artist_name.clone(),
        album_title: result.album_title.clone(),
        artwork_url: result.artwork_url.clone(),
        normalized_genres: result.normalized_genres.clone(),
        connection_reason: connection_reason.into(),
    }
}

fn build_profile(
    request: &ExternalDiscoveryRequest,
    context: &ExternalDiscoveryContext,
    prompt_terms: &[String],
    prompt_genres: &[String],
) -> DiscoveryProfilePreview {
    let top_artists = context
        .top_artists
        .iter()
        .take(5)
        .map(|artist| artist.artist_name.clone())
        .collect::<Vec<_>>();
    let top_genres = context
        .top_genres
        .iter()
        .take(5)
        .map(|genre| genre.genre_name.clone())
        .collect::<Vec<_>>();
    let recent_tracks = context
        .recent_listens
        .iter()
        .take(5)
        .map(|listen| listen.track_title.clone())
        .collect::<Vec<_>>();
    let favorite_ratio = if context.overview.tracks == 0 {
        0.0
    } else {
        context.overview.favorite_tracks as f64 / context.overview.tracks as f64
    };

    DiscoveryProfilePreview {
        prompt: request.prompt.trim().to_string(),
        mode: request.mode.clone(),
        services: if request.services.is_empty() {
            vec!["tidal".to_string()]
        } else {
            request.services.clone()
        },
        prompt_terms: prompt_terms.to_vec(),
        prompt_genres: prompt_genres.to_vec(),
        top_artists: top_artists.clone(),
        top_genres: top_genres.clone(),
        recent_tracks: recent_tracks.clone(),
        favorite_ratio,
        completion_rate: context.behavior.completion_rate,
        summary: format!(
            "Looking beyond your library with {} and leaning toward {} while steering away from recent repeats like {}.",
            request.prompt.trim(),
            list_or_fallback(
                &prompt_genres.to_vec(),
                &list_or_fallback(&top_genres, "broad genre lanes")
            ),
            list_or_fallback(&recent_tracks, "nothing too recent")
        ),
    }
}

fn build_reasons(
    profile: &DiscoveryProfilePreview,
    trail_item: Option<&DiscoveryConnectionTrailItem>,
) -> Vec<DiscoveryReason> {
    let mut reasons = vec![
        DiscoveryReason {
            label: "Prompt seed".to_string(),
            detail: format!(
                "Searching outward from {} with canonical genre cues like {}.",
                if profile.prompt.is_empty() {
                    "your taste profile".to_string()
                } else {
                    format!("\"{}\"", profile.prompt)
                },
                list_or_fallback(&profile.prompt_genres, "open-ended discovery")
            ),
            weight: 78,
        },
        DiscoveryReason {
            label: "Taste profile".to_string(),
            detail: format!(
                "Recent listening still pulls toward artists like {} and genres like {}.",
                list_or_fallback(&profile.top_artists, "new signals"),
                list_or_fallback(&profile.top_genres, "untagged lanes"),
            ),
            weight: 68,
        },
        DiscoveryReason {
            label: "Taste mesh".to_string(),
            detail: format!(
                "Ranking blends favorites, play history, completion behavior ({:.0}%), and recent-listen avoidance into one taste score.",
                profile.completion_rate * 100.0
            ),
            weight: 70,
        },
        DiscoveryReason {
            label: "New-to-you filter".to_string(),
            detail: "Results default to tracks outside your current NOOR library so discovery feels additive, not recursive.".to_string(),
            weight: 64,
        },
    ];

    if let Some(trail_item) = trail_item {
        reasons.push(DiscoveryReason {
            label: "Connection trail".to_string(),
            detail: format!(
                "This pass continues from {} by {} and follows {}.",
                trail_item.title,
                trail_item
                    .artist_name
                    .clone()
                    .unwrap_or_else(|| "an unknown artist".to_string()),
                trail_item.connection_reason
            ),
            weight: 72,
        });
    }

    reasons
}

fn rank_candidates(
    request: &ExternalDiscoveryRequest,
    context: &ExternalDiscoveryContext,
    candidates: &[DiscoveryCandidateTrack],
    library_tidal_ids: &HashSet<i64>,
    prompt_terms: &[String],
    prompt_genres: &[String],
) -> Vec<DiscoveryExternalResult> {
    let scoring_terms = {
        let filtered = discovery_terms(prompt_terms, prompt_genres);
        if filtered.is_empty() && prompt_genres.is_empty() {
            prompt_terms.to_vec()
        } else {
            filtered
        }
    };
    let recent_titles = context
        .recent_listens
        .iter()
        .map(|listen| listen.track_title.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let recent_artist_keys = context
        .recent_listens
        .iter()
        .filter_map(|listen| listen.artist_name.as_deref())
        .map(|artist| artist.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let top_artist_keys = context
        .top_artists
        .iter()
        .map(|artist| artist.artist_name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let top_genre_keys = context
        .top_genres
        .iter()
        .map(|genre| genre.genre_name.to_ascii_lowercase())
        .collect::<HashSet<_>>();

    let builder = embedded_builder();
    let prompt_genre_targets = prompt_genres
        .iter()
        .flat_map(|genre| {
            let mut names = vec![genre.to_ascii_lowercase()];
            names.extend(
                builder
                    .catalog()
                    .descendants_of(genre)
                    .into_iter()
                    .map(|child| child.to_ascii_lowercase()),
            );
            names
        })
        .collect::<HashSet<_>>();
    let prompt_genre_words = prompt_genres
        .iter()
        .flat_map(|genre| tokenize(genre))
        .filter(|word| !is_stopword(word))
        .collect::<HashSet<_>>();
    let taste_mesh = build_taste_mesh(context, prompt_genres);

    let mut ranked = candidates
        .iter()
        .filter(|candidate| {
            !candidate
                .tidal_track_id
                .map(|track_id| library_tidal_ids.contains(&track_id))
                .unwrap_or(false)
        })
        .filter_map(|candidate| {
            let combined_hints = candidate_genre_hints(candidate);
            let normalized_genres = normalize_genres(&combined_hints);
            let normalized_genre_keys = normalized_genres
                .iter()
                .map(|genre| genre.to_ascii_lowercase())
                .collect::<HashSet<_>>();
            let genre_tokens = normalized_genres
                .iter()
                .flat_map(|genre| tokenize(genre))
                .collect::<HashSet<_>>();
            let metadata_tokens = candidate
                .metadata_tokens
                .iter()
                .map(|token| token.as_str())
                .collect::<HashSet<_>>();
            let lastfm_tag_tokens = candidate
                .lastfm_tags
                .iter()
                .flat_map(|tag| tokenize(tag))
                .collect::<HashSet<_>>();
            let discogs_hint_tokens = candidate
                .discogs_genres
                .iter()
                .chain(candidate.discogs_styles.iter())
                .flat_map(|tag| tokenize(tag))
                .collect::<HashSet<_>>();
            let matched_genre_words = prompt_genre_words
                .iter()
                .filter(|word| {
                    metadata_tokens.contains(word.as_str()) || genre_tokens.contains(*word)
                })
                .count();
            let lastfm_matches = prompt_genre_words
                .iter()
                .filter(|word| lastfm_tag_tokens.contains(*word) || genre_tokens.contains(*word))
                .count();
            let discogs_matches = prompt_genre_words
                .iter()
                .filter(|word| discogs_hint_tokens.contains(*word) || genre_tokens.contains(*word))
                .count();
            let generic_artist_label = candidate
                .artist_name
                .as_deref()
                .map(is_generic_artist_label)
                .unwrap_or(false);

            if !prompt_genre_words.is_empty()
                && matched_genre_words == 0
                && candidate.seed_strength < 55
            {
                return None;
            }

            if should_hard_reject_candidate(
                request,
                candidate,
                generic_artist_label,
                &normalized_genres,
                matched_genre_words,
            ) {
                return None;
            }

            let mut score = 8.0 + (candidate.seed_strength as f64 * 0.72);
            let mut tags = vec!["new-to-you".to_string()];

            let prompt_hits = scoring_terms
                .iter()
                .filter(|term| {
                    metadata_tokens.contains(term.as_str()) || genre_tokens.contains(term.as_str())
                })
                .count();
            if prompt_hits > 0 {
                score += (prompt_hits as f64) * 12.0;
                tags.push("prompt match".to_string());
            }

            let artist_repeat = candidate
                .artist_name
                .as_deref()
                .map(|artist| {
                    let artist = artist.to_ascii_lowercase();
                    top_artist_keys.contains(&artist) || recent_artist_keys.contains(&artist)
                })
                .unwrap_or(false);
            let artist_key = candidate
                .artist_name
                .as_deref()
                .map(|artist| artist.to_ascii_lowercase());
            let artist_affinity = artist_key
                .as_ref()
                .and_then(|artist| taste_mesh.artist_affinity.get(artist))
                .copied()
                .unwrap_or_default();
            let recent_artist_penalty = artist_key
                .as_ref()
                .and_then(|artist| taste_mesh.recent_artist_penalty.get(artist))
                .copied()
                .unwrap_or_default();
            let genre_affinity = normalized_genre_keys
                .iter()
                .filter_map(|genre| taste_mesh.genre_affinity.get(genre))
                .sum::<f64>();

            let taste_alignment = (artist_affinity * taste_mesh.favorite_bias)
                + (genre_affinity * taste_mesh.completion_bias);
            if taste_alignment > 0.0 {
                score += taste_alignment.min(26.0);
                tags.push("taste mesh".to_string());
            }

            let lastfm_similarity_score = if !candidate.lastfm_tags.is_empty() {
                let similarity = if prompt_genre_words.is_empty() {
                    (candidate.lastfm_tags.len().min(4) as f64) / 8.0
                } else {
                    (lastfm_matches as f64) / prompt_genre_words.len().max(1) as f64
                }
                .clamp(0.0, 1.0);
                if similarity > 0.0 {
                    score += 6.0 + similarity * 10.0;
                    tags.push("last.fm similar".to_string());
                }
                Some(similarity)
            } else {
                None
            };

            if let Some(confidence) = candidate.discogs_confidence {
                if confidence > 0.0
                    && (!candidate.discogs_styles.is_empty()
                        || !candidate.discogs_genres.is_empty())
                {
                    let discogs_bonus = if prompt_genre_words.is_empty() {
                        4.0 + confidence * 6.0
                    } else {
                        (discogs_matches as f64 * 4.0) + (confidence * 6.0)
                    };
                    score += discogs_bonus;
                    tags.push("discogs style".to_string());
                }
            }

            if recent_artist_penalty > 0.0 {
                score -= recent_artist_penalty * taste_mesh.novelty_bias;
            }

            match candidate.seed_kind.as_str() {
                "album-seed" | "connected-album-seed" => {
                    score += 14.0;
                    tags.push("album seed".to_string());
                }
                "artist-seed" => {
                    score += 10.0;
                    tags.push("artist seed".to_string());
                }
                _ => {}
            }

            if top_genre_keys
                .iter()
                .any(|genre| normalized_genre_keys.contains(genre))
            {
                score += if prompt_genre_targets.is_empty() {
                    10.0
                } else {
                    4.0
                };
                tags.push("genre affinity".to_string());
            }

            if prompt_genre_targets
                .iter()
                .any(|genre| normalized_genre_keys.contains(genre))
            {
                score += 26.0;
                tags.push("prompt genre".to_string());
            } else if !prompt_genre_targets.is_empty() {
                score -= 24.0;
                tags.push("genre drift".to_string());
            }

            if matched_genre_words >= 2 {
                score += 12.0;
                tags.push("scene match".to_string());
            } else if matched_genre_words == 1 {
                score += 4.0;
            }

            if candidate.is_playable {
                score += 6.0;
                tags.push("playable now".to_string());
            }

            if candidate.artist_name.is_none() {
                score -= 12.0;
                tags.push("thin metadata".to_string());
            }

            if normalized_genres.is_empty() && !prompt_genre_targets.is_empty() {
                score -= 10.0;
            }

            if candidate.duration_ms.unwrap_or_default() > 900_000 {
                score -= 12.0;
                tags.push("long-form".to_string());
            }

            if recent_titles.contains(&candidate.title.to_ascii_lowercase()) {
                score -= 14.0;
                tags.push("recent echo".to_string());
            }

            if artist_repeat {
                if request.mode == "reference" {
                    score += 6.0;
                    tags.push("artist affinity".to_string());
                } else {
                    score -= 20.0;
                    tags.push("artist repeat".to_string());
                }
            }

            let spam_penalty = chart_pack_penalty(candidate, &normalized_genres);
            if spam_penalty > 0 {
                score -= spam_penalty as f64;
                tags.push("chart-pack".to_string());
                if spam_penalty >= 50 && candidate.seed_kind == "artist-seed" {
                    return None;
                }
            }

            let mode_adjustment = apply_mode_adjustments(
                request,
                candidate,
                &normalized_genres,
                &genre_tokens,
                prompt_terms,
                prompt_genres,
                prompt_hits,
                matched_genre_words,
                artist_affinity,
                artist_repeat,
                generic_artist_label,
            );
            score += mode_adjustment.score_delta;
            tags.extend(mode_adjustment.tags);

            let confidence_floor = if let Some(mode_floor) = mode_adjustment.confidence_floor {
                mode_floor
            } else if !prompt_genre_targets.is_empty() {
                if normalized_genres.is_empty() {
                    40.0
                } else {
                    28.0
                }
            } else {
                18.0
            };
            if score < confidence_floor {
                return None;
            }

            tags.truncate(5);
            Some(ScoredDiscoveryResult {
                raw_score: score,
                result: DiscoveryExternalResult {
                    provider: candidate.provider.clone(),
                    provider_track_id: candidate.provider_track_id.clone(),
                    title: candidate.title.clone(),
                    artist_name: candidate.artist_name.clone(),
                    album_title: candidate.album_title.clone(),
                    artwork_url: candidate.artwork_url.clone(),
                    duration_ms: candidate.duration_ms,
                    audio_quality: candidate.audio_quality.clone(),
                    normalized_genres,
                    lastfm_tags: candidate.lastfm_tags.clone(),
                    lastfm_similarity_score,
                    discogs_genres: candidate.discogs_genres.clone(),
                    discogs_styles: candidate.discogs_styles.clone(),
                    discogs_label: candidate.discogs_label.clone(),
                    discogs_year: candidate.discogs_year,
                    discogs_confidence: candidate.discogs_confidence,
                    in_library: false,
                    is_saved: false,
                    is_playable: candidate.is_playable,
                    embedding_score: None,
                    score: score.round().clamp(0.0, 99.0) as i32,
                    tags,
                },
            })
        })
        .collect::<Vec<_>>();

    let mut deduped = HashMap::<(String, String), ScoredDiscoveryResult>::new();
    for result in ranked.drain(..) {
        let key = (
            result.result.provider.clone(),
            result.result.provider_track_id.clone(),
        );
        match deduped.get(&key) {
            Some(existing)
                if existing.raw_score > result.raw_score
                    || (existing.raw_score == result.raw_score
                        && existing.result.title <= result.result.title) => {}
            _ => {
                deduped.insert(key, result);
            }
        }
    }
    ranked = deduped.into_values().collect();

    ranked.sort_by(|left, right| {
        right
            .raw_score
            .partial_cmp(&left.raw_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.result.title.cmp(&right.result.title))
    });
    select_diverse_results(ranked, request.limit.max(1))
}

fn normalize_genres(values: &[String]) -> Vec<String> {
    let builder = embedded_builder();
    let mut normalized = HashSet::new();
    for value in values {
        let words = value
            .split(|char: char| !char.is_ascii_alphanumeric())
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>();
        for start in 0..words.len() {
            for width in 1..=3 {
                if start + width > words.len() {
                    break;
                }
                let phrase = words[start..start + width].join(" ");
                if let Some(canonical) = builder.normalize(&phrase) {
                    normalized.insert(canonical);
                }
            }
        }
    }
    let mut normalized = normalized.into_iter().collect::<Vec<_>>();
    normalized.sort();
    normalized
}

fn candidate_genre_hints(candidate: &DiscoveryCandidateTrack) -> Vec<String> {
    let mut hints = candidate.raw_genre_hints.clone();
    hints.extend(candidate.lastfm_tags.clone());
    hints.extend(candidate.discogs_genres.clone());
    hints.extend(candidate.discogs_styles.clone());
    hints.sort();
    hints.dedup();
    hints
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn discovery_terms(prompt_terms: &[String], prompt_genres: &[String]) -> Vec<String> {
    let genre_tokens = prompt_genres
        .iter()
        .flat_map(|genre| tokenize(genre))
        .collect::<HashSet<_>>();

    prompt_terms
        .iter()
        .filter(|term| !is_stopword(term))
        .filter(|term| !genre_tokens.contains(term.as_str()))
        .filter(|term| !is_low_signal_term(term, !prompt_genres.is_empty()))
        .cloned()
        .collect()
}

fn infer_prompt_genres(prompt: &str) -> Vec<String> {
    normalize_genres(&[prompt.to_string()])
}

pub fn inferred_prompt_genres(prompt: &str) -> Vec<String> {
    infer_prompt_genres(prompt)
}

fn is_stopword(value: &str) -> bool {
    matches!(
        value,
        "a" | "an"
            | "and"
            | "at"
            | "for"
            | "from"
            | "in"
            | "into"
            | "of"
            | "on"
            | "or"
            | "the"
            | "to"
            | "with"
            | "music"
            | "songs"
            | "song"
            | "tracks"
            | "track"
            | "listen"
            | "listening"
    )
}

fn is_low_signal_term(value: &str, genre_prompt: bool) -> bool {
    if !genre_prompt {
        return false;
    }

    matches!(
        value,
        "deep"
            | "focus"
            | "late"
            | "night"
            | "mood"
            | "vibe"
            | "vibes"
            | "study"
            | "work"
            | "coding"
            | "sleep"
            | "chill"
            | "relax"
            | "relaxing"
            | "playlist"
            | "mix"
    )
}

fn dedupe_queries(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .take(6)
        .collect()
}

#[derive(Default)]
struct ModeAdjustment {
    score_delta: f64,
    confidence_floor: Option<f64>,
    tags: Vec<String>,
}

fn should_hard_reject_candidate(
    request: &ExternalDiscoveryRequest,
    candidate: &DiscoveryCandidateTrack,
    generic_artist_label: bool,
    normalized_genres: &[String],
    matched_genre_words: usize,
) -> bool {
    if generic_artist_label
        && candidate.seed_kind == "artist-seed"
        && request.mode != "word-cloud"
        && normalized_genres.len() >= 3
    {
        return true;
    }

    if request.mode == "mood"
        && generic_artist_label
        && matched_genre_words == 0
        && normalized_genres.len() >= 2
    {
        return true;
    }

    false
}

fn apply_mode_adjustments(
    request: &ExternalDiscoveryRequest,
    candidate: &DiscoveryCandidateTrack,
    normalized_genres: &[String],
    genre_tokens: &HashSet<String>,
    prompt_terms: &[String],
    prompt_genres: &[String],
    prompt_hits: usize,
    matched_genre_words: usize,
    artist_affinity: f64,
    artist_repeat: bool,
    generic_artist_label: bool,
) -> ModeAdjustment {
    match request.mode.as_str() {
        "reference" => reference_mode_adjustment(
            candidate,
            normalized_genres,
            prompt_hits,
            artist_affinity,
            artist_repeat,
        ),
        "dj" => dj_mode_adjustment(candidate, genre_tokens, generic_artist_label),
        "word-cloud" => word_cloud_mode_adjustment(candidate, prompt_terms, prompt_hits),
        _ => mood_mode_adjustment(
            candidate,
            normalized_genres,
            prompt_genres,
            matched_genre_words,
            generic_artist_label,
        ),
    }
}

fn mood_mode_adjustment(
    candidate: &DiscoveryCandidateTrack,
    normalized_genres: &[String],
    prompt_genres: &[String],
    matched_genre_words: usize,
    generic_artist_label: bool,
) -> ModeAdjustment {
    let mut adjustment = ModeAdjustment {
        confidence_floor: Some(if prompt_genres.is_empty() { 24.0 } else { 34.0 }),
        ..Default::default()
    };

    if matched_genre_words >= 2 && normalized_genres.len() >= 2 {
        adjustment.score_delta += 8.0;
        adjustment.tags.push("mood lock".to_string());
    }

    if generic_artist_label {
        adjustment.score_delta -= 10.0;
        adjustment.tags.push("generic artist".to_string());
    }

    if let Some(duration_ms) = candidate.duration_ms {
        if (240_000..=540_000).contains(&duration_ms) {
            adjustment.score_delta += 4.0;
            adjustment.tags.push("immersive length".to_string());
        }
    }

    if looks_mix_friendly(candidate) {
        adjustment.score_delta -= 8.0;
        adjustment.tags.push("tool cut".to_string());
    }

    if album_feels_packaged(candidate) {
        adjustment.score_delta -= 4.0;
        adjustment.tags.push("packaged source".to_string());
    }

    adjustment
}

fn reference_mode_adjustment(
    candidate: &DiscoveryCandidateTrack,
    normalized_genres: &[String],
    prompt_hits: usize,
    artist_affinity: f64,
    artist_repeat: bool,
) -> ModeAdjustment {
    let mut adjustment = ModeAdjustment {
        confidence_floor: Some(20.0),
        ..Default::default()
    };

    if artist_affinity > 0.0 {
        adjustment.score_delta += artist_affinity.min(14.0);
        adjustment.tags.push("reference affinity".to_string());
    }
    if artist_repeat {
        adjustment.score_delta += 10.0;
        adjustment.tags.push("artist continuity".to_string());
    }
    if matches!(
        candidate.seed_kind.as_str(),
        "artist-seed" | "album-seed" | "connected-album-seed"
    ) {
        adjustment.score_delta += 8.0;
        adjustment.tags.push("reference seed".to_string());
    }
    if prompt_hits == 0 && normalized_genres.is_empty() {
        adjustment.score_delta -= 6.0;
    }

    adjustment
}

fn dj_mode_adjustment(
    candidate: &DiscoveryCandidateTrack,
    genre_tokens: &HashSet<String>,
    generic_artist_label: bool,
) -> ModeAdjustment {
    let mut adjustment = ModeAdjustment {
        confidence_floor: Some(26.0),
        ..Default::default()
    };
    let mix_like = looks_mix_friendly(candidate);

    if genre_tokens.iter().any(|genre| {
        matches!(
            genre.as_str(),
            "house" | "techno" | "disco" | "garage" | "breakbeat" | "dub" | "club"
        )
    }) {
        adjustment.score_delta += 12.0;
        adjustment.tags.push("mix-ready".to_string());
    }

    if mix_like {
        adjustment.score_delta += 8.0;
        adjustment.tags.push("extended cut".to_string());
    }

    if let Some(duration_ms) = candidate.duration_ms {
        if duration_ms < 180_000 {
            adjustment.score_delta -= 14.0;
            adjustment.tags.push("too short".to_string());
        } else if duration_ms >= 300_000 {
            adjustment.score_delta += 5.0;
        }
    }

    if generic_artist_label {
        adjustment.score_delta -= 12.0;
        adjustment.tags.push("generic artist".to_string());
    }

    adjustment
}

fn word_cloud_mode_adjustment(
    candidate: &DiscoveryCandidateTrack,
    prompt_terms: &[String],
    prompt_hits: usize,
) -> ModeAdjustment {
    let mut adjustment = ModeAdjustment {
        confidence_floor: Some(14.0),
        ..Default::default()
    };
    adjustment.score_delta += (prompt_terms.len().min(5) as f64) * 1.5;
    if prompt_hits >= 2 {
        adjustment.score_delta += 4.0;
        adjustment.tags.push("concept spread".to_string());
    }
    if candidate.seed_kind == "track-search" {
        adjustment.score_delta += 3.0;
    }
    adjustment
}

fn looks_mix_friendly(candidate: &DiscoveryCandidateTrack) -> bool {
    let title = candidate.title.to_ascii_lowercase();
    let album_title = candidate
        .album_title
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "mix", "dub mix", "extended", "club mix", "version", "remix", "edit", "pt.",
    ]
    .iter()
    .any(|needle| title.contains(needle) || album_title.contains(needle))
}

fn album_feels_packaged(candidate: &DiscoveryCandidateTrack) -> bool {
    let album_title = candidate
        .album_title
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    [
        "compilation",
        "panorama",
        "anthology",
        "collection",
        "alternative takes",
        "sampler",
    ]
    .iter()
    .any(|needle| album_title.contains(needle))
}

fn is_generic_artist_label(artist_name: &str) -> bool {
    let artist_name = artist_name.trim().to_ascii_lowercase();
    if artist_name.is_empty() {
        return false;
    }

    let builder = embedded_builder();
    if builder.normalize(&artist_name).is_some() {
        return true;
    }

    let tokens = tokenize(&artist_name);
    let generic_tokens = tokens
        .iter()
        .filter(|token| {
            matches!(
                token.as_str(),
                "music"
                    | "workout"
                    | "dj"
                    | "mix"
                    | "mixes"
                    | "club"
                    | "dance"
                    | "house"
                    | "techno"
                    | "trance"
                    | "dub"
                    | "hits"
                    | "fitness"
            )
        })
        .count();

    generic_tokens >= tokens.len().max(1) || generic_tokens >= 2
}

fn chart_pack_penalty(candidate: &DiscoveryCandidateTrack, normalized_genres: &[String]) -> i32 {
    let album_title = candidate
        .album_title
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let title = candidate.title.to_ascii_lowercase();
    let artist_name = candidate
        .artist_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    let chart_pack_hits = [
        "top 100",
        "top 50",
        "best selling chart hits",
        "chart hits",
        "greatest hits",
        "ultimate collection",
        "karaoke",
        "tribute",
    ]
    .iter()
    .filter(|phrase| album_title.contains(**phrase) || title.contains(**phrase))
    .count() as i32;
    let package_hits = [
        "workout", "gym", "dj mix", "mixes", "party", "fitness", "burn",
    ]
    .iter()
    .filter(|phrase| {
        album_title.contains(**phrase) || title.contains(**phrase) || artist_name.contains(**phrase)
    })
    .count() as i32;

    let builder = embedded_builder();
    let genre_named_artist = builder.normalize(&artist_name).is_some();

    let mut penalty = chart_pack_hits * 28 + package_hits * 14;
    if chart_pack_hits > 0 && candidate.seed_kind == "artist-seed" {
        penalty += 18;
    }
    if (chart_pack_hits > 0 || package_hits > 0) && genre_named_artist {
        penalty += 24;
    }
    if (chart_pack_hits > 0 || package_hits > 0) && normalized_genres.len() >= 4 {
        penalty += 12;
    }

    penalty
}

fn select_diverse_results(
    ranked: Vec<ScoredDiscoveryResult>,
    limit: usize,
) -> Vec<DiscoveryExternalResult> {
    let album_cap = 1;
    let artist_cap = if limit <= 8 { 2 } else { 3 };
    let mut selected = Vec::new();
    let mut album_counts = HashMap::<String, usize>::new();
    let mut artist_counts = HashMap::<String, usize>::new();

    for scored in ranked {
        if selected.len() >= limit {
            break;
        }
        let result = scored.result;

        let album_key = result.album_title.as_ref().map(|album_title| {
            format!("{}::{}", result.provider, album_title.to_ascii_lowercase())
        });
        let artist_key = result.artist_name.as_ref().map(|artist_name| {
            format!("{}::{}", result.provider, artist_name.to_ascii_lowercase())
        });

        let album_count = album_key
            .as_ref()
            .and_then(|key| album_counts.get(key))
            .copied()
            .unwrap_or(0);
        let artist_count = artist_key
            .as_ref()
            .and_then(|key| artist_counts.get(key))
            .copied()
            .unwrap_or(0);

        if album_count >= album_cap || artist_count >= artist_cap {
            continue;
        }

        if let Some(key) = album_key {
            *album_counts.entry(key).or_insert(0) += 1;
        }
        if let Some(key) = artist_key {
            *artist_counts.entry(key).or_insert(0) += 1;
        }
        selected.push(result);
    }
    selected.truncate(limit);
    selected
}

fn list_or_fallback(values: &[String], fallback: &str) -> String {
    if values.is_empty() {
        fallback.to_string()
    } else {
        values
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn build_taste_mesh(
    context: &ExternalDiscoveryContext,
    prompt_genres: &[String],
) -> TasteMeshProfile {
    let max_artist_listens = context
        .top_artists
        .iter()
        .map(|artist| artist.listens.max(1) as f64)
        .fold(1.0, f64::max);
    let max_genre_listens = context
        .top_genres
        .iter()
        .map(|genre| genre.listens.max(1) as f64)
        .fold(1.0, f64::max);

    let artist_affinity = context
        .top_artists
        .iter()
        .map(|artist| {
            let normalized_artist = artist.artist_name.to_ascii_lowercase();
            let base = artist.listens as f64 / max_artist_listens;
            let completion = artist.completed_listens as f64 / artist.listens.max(1) as f64;
            let breadth = artist.unique_tracks as f64 / 6.0;
            (
                normalized_artist,
                ((base * 8.0) + (completion * 6.0) + breadth.min(1.5) * 4.0).min(16.0),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut genre_affinity = context
        .top_genres
        .iter()
        .map(|genre| {
            (
                genre.genre_name.to_ascii_lowercase(),
                ((genre.listens as f64 / max_genre_listens) * 10.0).min(10.0),
            )
        })
        .collect::<HashMap<_, _>>();

    for prompt_genre in prompt_genres {
        *genre_affinity
            .entry(prompt_genre.to_ascii_lowercase())
            .or_insert(0.0) += 8.0;
    }

    let mut recent_artist_penalty = HashMap::<String, f64>::new();
    for listen in &context.recent_listens {
        if let Some(artist_name) = listen.artist_name.as_deref() {
            *recent_artist_penalty
                .entry(artist_name.to_ascii_lowercase())
                .or_insert(0.0) += if listen.completed { 2.0 } else { 1.0 };
        }
    }

    TasteMeshProfile {
        artist_affinity,
        genre_affinity,
        recent_artist_penalty,
        completion_bias: (0.8 + context.behavior.completion_rate.clamp(0.0, 1.0) * 0.7).min(1.5),
        favorite_bias: (0.7
            + if context.overview.tracks == 0 {
                0.0
            } else {
                (context.overview.favorite_tracks as f64 / context.overview.tracks as f64)
                    .clamp(0.0, 1.0)
                    * 0.9
            })
        .min(1.6),
        novelty_bias: (1.0
            + (context.behavior.skipped_listens as f64
                / context.behavior.total_listens.max(1) as f64)
                * 1.2)
            .min(2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ExternalDiscoveryContext {
        ExternalDiscoveryContext {
            overview: AnalyticsOverview {
                tracks: 100,
                albums: 20,
                artists: 10,
                playlists: 4,
                smart_playlists: 1,
                tagged_tracks: 30,
                total_listens: 40,
                favorite_tracks: 25,
            },
            behavior: AnalyticsBehavior {
                total_listened_ms: 500_000,
                total_listens: 40,
                completed_listens: 25,
                skipped_listens: 15,
                completion_rate: 0.625,
                average_listen_ms: 12_500,
                unique_tracks: 30,
                repeat_track_count: 6,
                active_days: 12,
            },
            recent_listens: vec![ListenHistoryEntry {
                id: 1,
                track_id: 99,
                track_title: "Already Heard".to_string(),
                artist_name: Some("Night Driver".to_string()),
                album_title: Some("Signals".to_string()),
                artwork_url: None,
                started_at: "2026-04-07T00:00:00Z".to_string(),
                duration_listened_ms: 120_000,
                completed: true,
            }],
            top_artists: vec![AnalyticsTopArtist {
                artist_id: 1,
                artist_name: "Night Driver".to_string(),
                listens: 7,
                completed_listens: 5,
                unique_tracks: 4,
                total_listened_ms: 200_000,
            }],
            top_genres: vec![AnalyticsGenreShare {
                genre_name: "Synthwave".to_string(),
                listens: 10,
            }],
        }
    }

    #[test]
    fn filters_existing_library_tracks() {
        let request = ExternalDiscoveryRequest {
            prompt: "glassy synthwave".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 8,
        };
        let candidates = vec![
            DiscoveryCandidateTrack {
                provider: "tidal".to_string(),
                provider_track_id: "100".to_string(),
                tidal_track_id: Some(100),
                title: "Glassy".to_string(),
                artist_name: Some("Night Driver".to_string()),
                album_title: Some("Signals".to_string()),
                artwork_url: None,
                duration_ms: Some(180_000),
                audio_quality: Some("LOSSLESS".to_string()),
                raw_genre_hints: vec!["Synthwave".to_string()],
                lastfm_tags: vec![],
                discogs_genres: vec![],
                discogs_styles: vec![],
                discogs_label: None,
                discogs_year: None,
                discogs_confidence: None,
                is_playable: true,
                metadata_tokens: vec![
                    "glassy".to_string(),
                    "night".to_string(),
                    "driver".to_string(),
                    "signals".to_string(),
                    "synthwave".to_string(),
                ],
                seed_kind: "album-seed".to_string(),
                seed_strength: 58,
            },
            DiscoveryCandidateTrack {
                provider: "tidal".to_string(),
                provider_track_id: "101".to_string(),
                tidal_track_id: Some(101),
                title: "Fresh Air".to_string(),
                artist_name: Some("Night Driver".to_string()),
                album_title: Some("Signals".to_string()),
                artwork_url: None,
                duration_ms: Some(180_000),
                audio_quality: Some("LOSSLESS".to_string()),
                raw_genre_hints: vec!["Synthwave".to_string()],
                lastfm_tags: vec![],
                discogs_genres: vec![],
                discogs_styles: vec![],
                discogs_label: None,
                discogs_year: None,
                discogs_confidence: None,
                is_playable: true,
                metadata_tokens: vec![
                    "fresh".to_string(),
                    "air".to_string(),
                    "night".to_string(),
                    "driver".to_string(),
                    "signals".to_string(),
                    "synthwave".to_string(),
                ],
                seed_kind: "artist-seed".to_string(),
                seed_strength: 46,
            },
        ];

        let feed = build_external_feed(
            &request,
            &context(),
            &candidates,
            &HashSet::from([100]),
            vec![],
            None,
        );

        assert_eq!(feed.results.len(), 1);
        assert_eq!(feed.results[0].provider_track_id, "101");
    }

    #[test]
    fn connection_queries_follow_seed_and_prompt() {
        let request = ExternalDiscoveryRequest {
            prompt: "rainy neon".to_string(),
            mode: "reference".to_string(),
            services: vec!["tidal".to_string()],
            limit: 8,
        };
        let seed = DiscoveryCandidateSeed {
            provider_track_id: "88".to_string(),
            title: "Blue Hour".to_string(),
            artist_name: Some("Night Driver".to_string()),
            album_title: Some("Signals".to_string()),
            normalized_genres: vec!["Synthwave".to_string()],
        };

        let queries = build_connection_queries(&request, &context(), &seed);
        assert!(queries.iter().any(|query| query.contains("Blue Hour")));
        assert!(queries.iter().any(|query| query.contains("Synthwave")));
    }

    #[test]
    fn mood_queries_do_not_lead_with_top_artist_bias() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno for late night focus".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 8,
        };

        let queries = build_search_queries(&request, &context());

        assert!(
            queries
                .iter()
                .any(|query| query.to_ascii_lowercase().contains("dub techno"))
        );
        assert!(
            !queries
                .iter()
                .any(|query| query.contains(&context().top_artists[0].artist_name))
        );
    }

    #[test]
    fn dedupes_repeated_provider_results() {
        let request = ExternalDiscoveryRequest {
            prompt: "night drive".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 5,
        };
        let duplicate = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "42".to_string(),
            tidal_track_id: Some(42),
            title: "Night Drive".to_string(),
            artist_name: Some("Signal".to_string()),
            album_title: Some("After Hours".to_string()),
            artwork_url: None,
            duration_ms: Some(210_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "night".to_string(),
                "drive".to_string(),
                "signal".to_string(),
                "after".to_string(),
                "hours".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[duplicate.clone(), duplicate],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(feed.results.len(), 1);
        assert_eq!(feed.results[0].provider_track_id, "42");
    }

    #[test]
    fn suppresses_low_confidence_scene_results() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno for late night focus".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 8,
        };
        let weak_candidate = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "900".to_string(),
            tidal_track_id: Some(900),
            title: "Late Focus Jazz".to_string(),
            artist_name: None,
            album_title: Some("Instrumental Study".to_string()),
            artwork_url: None,
            duration_ms: Some(180_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec![],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "late".to_string(),
                "focus".to_string(),
                "jazz".to_string(),
                "instrumental".to_string(),
                "study".to_string(),
            ],
            seed_kind: "track-search".to_string(),
            seed_strength: 32,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[weak_candidate],
            &HashSet::new(),
            vec![],
            None,
        );

        assert!(feed.results.is_empty());
    }

    #[test]
    fn exact_token_matching_rejects_substring_false_positives() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno for late night focus".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 8,
        };
        let substring_candidate = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "901".to_string(),
            tidal_track_id: Some(901),
            title: "Dubious Echo".to_string(),
            artist_name: None,
            album_title: Some("Night Focus".to_string()),
            artwork_url: None,
            duration_ms: Some(960_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec![],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: false,
            metadata_tokens: vec![
                "dubious".to_string(),
                "echo".to_string(),
                "night".to_string(),
                "focus".to_string(),
            ],
            seed_kind: "track-search".to_string(),
            seed_strength: 32,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[substring_candidate],
            &HashSet::new(),
            vec![],
            None,
        );

        assert!(feed.results.is_empty());
    }

    #[test]
    fn demotes_chart_pack_artist_seed_results() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno for late night focus".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 1,
        };
        let legit_candidate = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "910".to_string(),
            tidal_track_id: Some(910),
            title: "Accept Fate".to_string(),
            artist_name: Some("Pulshar".to_string()),
            album_title: Some("Espectrum II: The Avantroots Dub Techno Compilation".to_string()),
            artwork_url: None,
            duration_ms: Some(353_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub Techno".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "accept".to_string(),
                "fate".to_string(),
                "pulshar".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };
        let chart_pack_candidate = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "911".to_string(),
            tidal_track_id: Some(911),
            title: "Amazing Gadget".to_string(),
            artist_name: Some("Deep House".to_string()),
            album_title: Some("Techno House Dance Top 100 Best Selling Chart Hits".to_string()),
            artwork_url: None,
            duration_ms: Some(373_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec![
                "Dub".to_string(),
                "Dubstep".to_string(),
                "House".to_string(),
                "Techno".to_string(),
            ],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "amazing".to_string(),
                "gadget".to_string(),
                "deep".to_string(),
                "house".to_string(),
                "top".to_string(),
                "100".to_string(),
                "chart".to_string(),
                "hits".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "artist-seed".to_string(),
            seed_strength: 46,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[chart_pack_candidate, legit_candidate],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(feed.results.len(), 1);
        assert_eq!(feed.results[0].provider_track_id, "910");
    }

    #[test]
    fn diversifies_repeated_album_clusters() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno for late night focus".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 2,
        };
        let same_album_a = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "920".to_string(),
            tidal_track_id: Some(920),
            title: "Accept Fate".to_string(),
            artist_name: Some("Pulshar".to_string()),
            album_title: Some("Espectrum II: The Avantroots Dub Techno Compilation".to_string()),
            artwork_url: None,
            duration_ms: Some(353_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub Techno".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec!["accept".to_string(), "fate".to_string(), "dub".to_string()],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };
        let same_album_b = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "921".to_string(),
            tidal_track_id: Some(921),
            title: "Stepping Up".to_string(),
            artist_name: Some("Segue".to_string()),
            album_title: Some("Espectrum II: The Avantroots Dub Techno Compilation".to_string()),
            artwork_url: None,
            duration_ms: Some(396_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub Techno".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec!["stepping".to_string(), "up".to_string(), "dub".to_string()],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };
        let other_album = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "922".to_string(),
            tidal_track_id: Some(922),
            title: "Limbus".to_string(),
            artist_name: Some("Nadja Lind".to_string()),
            album_title: Some("Deep Space Night - Panorama of Dub Techno".to_string()),
            artwork_url: None,
            duration_ms: Some(449_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub Techno".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "limbus".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[same_album_a, same_album_b, other_album],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(feed.results.len(), 2);
        let album_titles = feed
            .results
            .iter()
            .filter_map(|result| result.album_title.clone())
            .collect::<HashSet<_>>();
        assert_eq!(album_titles.len(), 2);
    }

    #[test]
    fn taste_mesh_prefers_history_aligned_candidates() {
        let request = ExternalDiscoveryRequest {
            prompt: "night drive".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 1,
        };
        let aligned = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "930".to_string(),
            tidal_track_id: Some(930),
            title: "Signals".to_string(),
            artist_name: Some("Night Driver".to_string()),
            album_title: Some("Signals".to_string()),
            artwork_url: None,
            duration_ms: Some(220_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Synthwave".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "signals".to_string(),
                "night".to_string(),
                "driver".to_string(),
                "synthwave".to_string(),
            ],
            seed_kind: "artist-seed".to_string(),
            seed_strength: 40,
        };
        let unrelated = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "931".to_string(),
            tidal_track_id: Some(931),
            title: "Other Route".to_string(),
            artist_name: Some("Elsewhere".to_string()),
            album_title: Some("Road Notes".to_string()),
            artwork_url: None,
            duration_ms: Some(220_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Synthwave".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "other".to_string(),
                "route".to_string(),
                "elsewhere".to_string(),
                "synthwave".to_string(),
            ],
            seed_kind: "artist-seed".to_string(),
            seed_strength: 40,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[unrelated, aligned],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(feed.results.len(), 1);
        assert_eq!(feed.results[0].provider_track_id, "930");
    }

    #[test]
    fn reference_mode_keeps_artist_affinity_that_mood_mode_demotes() {
        let mood_request = ExternalDiscoveryRequest {
            prompt: "night driver synthwave".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 1,
        };
        let reference_request = ExternalDiscoveryRequest {
            mode: "reference".to_string(),
            ..mood_request.clone()
        };
        let familiar_artist = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "940".to_string(),
            tidal_track_id: Some(940),
            title: "Blue Hour".to_string(),
            artist_name: Some("Night Driver".to_string()),
            album_title: Some("Signals".to_string()),
            artwork_url: None,
            duration_ms: Some(240_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Synthwave".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "blue".to_string(),
                "hour".to_string(),
                "night".to_string(),
                "driver".to_string(),
                "synthwave".to_string(),
            ],
            seed_kind: "artist-seed".to_string(),
            seed_strength: 40,
        };
        let adjacent_artist = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "941".to_string(),
            tidal_track_id: Some(941),
            title: "Glass Harbour".to_string(),
            artist_name: Some("Neon Coast".to_string()),
            album_title: Some("Afterglow".to_string()),
            artwork_url: None,
            duration_ms: Some(300_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Synthwave".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "glass".to_string(),
                "harbour".to_string(),
                "neon".to_string(),
                "coast".to_string(),
                "synthwave".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };

        let mood_feed = build_external_feed(
            &mood_request,
            &context(),
            &[familiar_artist.clone(), adjacent_artist.clone()],
            &HashSet::new(),
            vec![],
            None,
        );
        let reference_feed = build_external_feed(
            &reference_request,
            &context(),
            &[familiar_artist, adjacent_artist],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(mood_feed.results[0].provider_track_id, "941");
        assert_eq!(reference_feed.results[0].provider_track_id, "940");
    }

    #[test]
    fn dj_mode_prefers_mix_friendly_tracks_over_short_radio_like_cuts() {
        let request = ExternalDiscoveryRequest {
            prompt: "dub techno club set".to_string(),
            mode: "dj".to_string(),
            services: vec!["tidal".to_string()],
            limit: 1,
        };
        let extended_mix = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "950".to_string(),
            tidal_track_id: Some(950),
            title: "Signal Drift (Dub Mix)".to_string(),
            artist_name: Some("Pulshar".to_string()),
            album_title: Some("Night Sessions".to_string()),
            artwork_url: None,
            duration_ms: Some(420_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub Techno".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "signal".to_string(),
                "drift".to_string(),
                "dub".to_string(),
                "mix".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 50,
        };
        let short_cut = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "951".to_string(),
            tidal_track_id: Some(951),
            title: "Signal Drift".to_string(),
            artist_name: Some("Pulshar".to_string()),
            album_title: Some("Night Sessions".to_string()),
            artwork_url: None,
            duration_ms: Some(154_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub Techno".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "signal".to_string(),
                "drift".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "track-search".to_string(),
            seed_strength: 50,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[short_cut, extended_mix],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(feed.results[0].provider_track_id, "950");
    }

    #[test]
    fn mood_mode_demotes_mix_tool_cuts_when_clean_scene_track_exists() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 1,
        };
        let remix_cut = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "960".to_string(),
            tidal_track_id: Some(960),
            title: "Bright Nights (Rima Techno Chimp Dub Mix)".to_string(),
            artist_name: Some("Koop".to_string()),
            album_title: Some("Waltz for Koop - Alternative Takes".to_string()),
            artwork_url: None,
            duration_ms: Some(443_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "bright".to_string(),
                "nights".to_string(),
                "dub".to_string(),
                "mix".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };
        let clean_scene_track = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "961".to_string(),
            tidal_track_id: Some(961),
            title: "Accept Fate".to_string(),
            artist_name: Some("Pulshar".to_string()),
            album_title: Some("Espectrum II: The Avantroots Dub Techno Compilation".to_string()),
            artwork_url: None,
            duration_ms: Some(353_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec![
                "Dub".to_string(),
                "Dub Techno".to_string(),
                "Techno".to_string(),
            ],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "accept".to_string(),
                "fate".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[remix_cut, clean_scene_track],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(feed.results[0].provider_track_id, "961");
    }

    #[test]
    fn diversity_cap_treats_multi_artist_compilations_as_one_album_source() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 3,
        };
        let compilation_a = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "970".to_string(),
            tidal_track_id: Some(970),
            title: "Accept Fate".to_string(),
            artist_name: Some("Pulshar".to_string()),
            album_title: Some("Espectrum II: The Avantroots Dub Techno Compilation".to_string()),
            artwork_url: None,
            duration_ms: Some(353_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec![
                "Dub".to_string(),
                "Dub Techno".to_string(),
                "Techno".to_string(),
            ],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "accept".to_string(),
                "fate".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };
        let compilation_b = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "971".to_string(),
            tidal_track_id: Some(971),
            title: "Stepping Up".to_string(),
            artist_name: Some("Segue".to_string()),
            album_title: Some("Espectrum II: The Avantroots Dub Techno Compilation".to_string()),
            artwork_url: None,
            duration_ms: Some(396_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec![
                "Dub".to_string(),
                "Dub Techno".to_string(),
                "Techno".to_string(),
            ],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "stepping".to_string(),
                "up".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };
        let other_album_a = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "972".to_string(),
            tidal_track_id: Some(972),
            title: "Limbus".to_string(),
            artist_name: Some("Nadja Lind".to_string()),
            album_title: Some("Deep Space Night".to_string()),
            artwork_url: None,
            duration_ms: Some(449_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec![
                "Dub".to_string(),
                "Dub Techno".to_string(),
                "Techno".to_string(),
            ],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "limbus".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };
        let other_album_b = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "973".to_string(),
            tidal_track_id: Some(973),
            title: "Boutade".to_string(),
            artist_name: Some("Mugwump".to_string()),
            album_title: Some("Boutade".to_string()),
            artwork_url: None,
            duration_ms: Some(432_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Dub".to_string(), "Techno".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "boutade".to_string(),
                "dub".to_string(),
                "techno".to_string(),
            ],
            seed_kind: "album-seed".to_string(),
            seed_strength: 58,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[compilation_a, compilation_b, other_album_a, other_album_b],
            &HashSet::new(),
            vec![],
            None,
        );

        let album_titles = feed
            .results
            .iter()
            .filter_map(|result| result.album_title.clone())
            .collect::<HashSet<_>>();
        assert_eq!(feed.results.len(), 3);
        assert_eq!(album_titles.len(), 3);
    }

    #[test]
    fn lastfm_and_discogs_signals_boost_scene_aligned_candidate() {
        let request = ExternalDiscoveryRequest {
            prompt: "deep dub techno".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 1,
        };
        let enriched = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "980".to_string(),
            tidal_track_id: Some(980),
            title: "Subsurface".to_string(),
            artist_name: Some("Pulshar".to_string()),
            album_title: Some("Subsurface".to_string()),
            artwork_url: None,
            duration_ms: Some(388_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Electronic".to_string()],
            lastfm_tags: vec!["Dub Techno".to_string(), "Techno".to_string()],
            discogs_genres: vec!["Electronic".to_string()],
            discogs_styles: vec!["Dub Techno".to_string()],
            discogs_label: Some("Avantroots".to_string()),
            discogs_year: Some(2013),
            discogs_confidence: Some(0.72),
            is_playable: true,
            metadata_tokens: vec!["subsurface".to_string(), "pulshar".to_string()],
            seed_kind: "album-seed".to_string(),
            seed_strength: 52,
        };
        let weak = DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: "981".to_string(),
            tidal_track_id: Some(981),
            title: "Late Focus Tools".to_string(),
            artist_name: Some("Studio Runner".to_string()),
            album_title: Some("Night Work".to_string()),
            artwork_url: None,
            duration_ms: Some(390_000),
            audio_quality: Some("LOSSLESS".to_string()),
            raw_genre_hints: vec!["Electronic".to_string()],
            lastfm_tags: vec![],
            discogs_genres: vec![],
            discogs_styles: vec![],
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: true,
            metadata_tokens: vec![
                "late".to_string(),
                "focus".to_string(),
                "night".to_string(),
                "work".to_string(),
            ],
            seed_kind: "track-search".to_string(),
            seed_strength: 52,
        };

        let feed = build_external_feed(
            &request,
            &context(),
            &[weak, enriched],
            &HashSet::new(),
            vec![],
            None,
        );

        assert_eq!(feed.results[0].provider_track_id, "980");
        assert_eq!(feed.results[0].discogs_styles, vec!["Dub Techno"]);
        assert_eq!(feed.results[0].lastfm_tags, vec!["Dub Techno", "Techno"]);
    }
}
