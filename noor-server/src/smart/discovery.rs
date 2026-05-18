use crate::db::models::{
    AnalyticsBehavior, AnalyticsGenreShare, AnalyticsOverview, AnalyticsTopArtist,
    DiscoveryPreview, DiscoveryPreviewResult, DiscoveryProfilePreview, DiscoveryReason,
    ListenHistoryEntry, Track,
};
use crate::genre::builder::embedded_builder;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct DiscoveryPreviewRequest {
    pub prompt: String,
    pub mode: String,
    pub services: Vec<String>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct DiscoveryContext {
    pub overview: AnalyticsOverview,
    pub behavior: AnalyticsBehavior,
    pub recent_listens: Vec<ListenHistoryEntry>,
    pub top_artists: Vec<AnalyticsTopArtist>,
    pub top_genres: Vec<AnalyticsGenreShare>,
    pub track_genres: HashMap<i64, Vec<String>>,
}

pub fn build_preview(
    request: &DiscoveryPreviewRequest,
    context: &DiscoveryContext,
    candidates: &[Track],
) -> DiscoveryPreview {
    let prompt_terms = tokenize(&request.prompt);
    let prompt_genres = infer_prompt_genres(&request.prompt);
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

    let services = if request.services.is_empty() {
        vec!["tidal".to_string()]
    } else {
        request.services.clone()
    };

    let profile = DiscoveryProfilePreview {
        prompt: request.prompt.trim().to_string(),
        mode: request.mode.clone(),
        services: services.clone(),
        prompt_terms: prompt_terms.clone(),
        prompt_genres: prompt_genres.clone(),
        top_artists: top_artists.clone(),
        top_genres: top_genres.clone(),
        recent_tracks: recent_tracks.clone(),
        favorite_ratio,
        completion_rate: context.behavior.completion_rate,
        summary: build_summary(request, &top_artists, &top_genres, &recent_tracks),
    };

    let reasons = build_reasons(&profile, context);
    let results = rank_candidates(request, context, candidates, &prompt_terms, &prompt_genres);

    DiscoveryPreview {
        profile,
        reasons,
        results,
    }
}

fn build_reasons(
    profile: &DiscoveryProfilePreview,
    context: &DiscoveryContext,
) -> Vec<DiscoveryReason> {
    let mut reasons = Vec::new();

    reasons.push(DiscoveryReason {
        label: "Prompt seed".to_string(),
        detail: if profile.prompt_terms.is_empty() {
            "No prompt terms were supplied, so discovery leans harder on listening behavior."
                .to_string()
        } else {
            format!(
                "Prompt terms {} anchor the first pass of discovery scoring.",
                profile
                    .prompt_terms
                    .iter()
                    .take(4)
                    .map(|term| format!("\"{term}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
        weight: 72,
    });

    reasons.push(DiscoveryReason {
        label: "Listening profile".to_string(),
        detail: if profile.top_artists.is_empty()
            && profile.top_genres.is_empty()
            && profile.prompt_genres.is_empty()
        {
            "Listening history is still thin, so recommendations stay broad for now.".to_string()
        } else {
            format!(
                "Recent listening favors artists like {}, genres like {}, and the prompt resolves toward {}.",
                list_or_fallback(&profile.top_artists, "new signals"),
                list_or_fallback(&profile.top_genres, "untagged lanes"),
                list_or_fallback(&profile.prompt_genres, "no canonical genres yet"),
            )
        },
        weight: 64,
    });

    reasons.push(DiscoveryReason {
        label: "Behavioral bias".to_string(),
        detail: format!(
            "Completion rate is {:.0}% across {} logged listens, which tunes the preview toward {} picks.",
            context.behavior.completion_rate * 100.0,
            context.behavior.total_listens,
            if context.behavior.completion_rate >= 0.6 {
                "stickier"
            } else {
                "broader"
            }
        ),
        weight: 54,
    });

    reasons.push(DiscoveryReason {
        label: "Service scope".to_string(),
        detail: format!(
            "This pass is filtered to {}.",
            list_or_fallback(&profile.services, "tidal")
        ),
        weight: 42,
    });

    reasons
}

fn rank_candidates(
    request: &DiscoveryPreviewRequest,
    context: &DiscoveryContext,
    candidates: &[Track],
    prompt_terms: &[String],
    prompt_genres: &[String],
) -> Vec<DiscoveryPreviewResult> {
    let recent_track_ids = context
        .recent_listens
        .iter()
        .map(|listen| listen.track_id)
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

    let mut ranked = candidates
        .iter()
        .map(|track| {
            let mut score = 24.0;
            let mut tags = Vec::new();
            let haystack = format!(
                "{} {} {}",
                track.title,
                track.artist_name.as_deref().unwrap_or_default(),
                track.album_title.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase();

            let genre_blob = context
                .track_genres
                .get(&track.id)
                .map(|paths| paths.join(" ").to_ascii_lowercase())
                .unwrap_or_default();

            let prompt_hits = prompt_terms
                .iter()
                .filter(|term| {
                    haystack.contains(term.as_str()) || genre_blob.contains(term.as_str())
                })
                .count();
            if prompt_hits > 0 {
                score += (prompt_hits as f64) * 14.0;
                tags.push("prompt match".to_string());
            }

            if track
                .artist_name
                .as_deref()
                .map(|name| top_artist_keys.contains(&name.to_ascii_lowercase()))
                .unwrap_or(false)
            {
                score += 18.0;
                tags.push("artist affinity".to_string());
            }

            if top_genre_keys
                .iter()
                .any(|genre| genre_blob.contains(genre.as_str()))
            {
                score += 12.0;
                tags.push("genre affinity".to_string());
            }

            if prompt_genre_targets
                .iter()
                .any(|genre| genre_blob.contains(genre.as_str()))
            {
                score += 16.0;
                tags.push("prompt genre".to_string());
            }

            if track.is_favorite {
                score += 8.0;
                tags.push("favorite bias".to_string());
            }

            score += (track.play_count.min(20) as f64) * 1.4;

            if recent_track_ids.contains(&track.id) {
                score -= 18.0;
                tags.push("recently heard".to_string());
            }

            match request.mode.as_str() {
                "reference" => {
                    if prompt_hits > 0
                        && track
                            .artist_name
                            .as_deref()
                            .map(|name| haystack.contains(&name.to_ascii_lowercase()))
                            .unwrap_or(false)
                    {
                        score += 10.0;
                    }
                }
                "dj" => {
                    if genre_blob.contains("house")
                        || genre_blob.contains("techno")
                        || genre_blob.contains("disco")
                    {
                        score += 9.0;
                        tags.push("mix-ready".to_string());
                    }
                }
                "word-cloud" => {
                    score += (prompt_terms.len().min(5) as f64) * 2.0;
                }
                _ => {}
            }

            let score = score.round().clamp(0.0, 99.0) as i32;
            if tags.is_empty() {
                tags.push("library signal".to_string());
            }
            if tags.len() > 4 {
                tags.truncate(4);
            }

            DiscoveryPreviewResult {
                track_id: track.id,
                title: track.title.clone(),
                artist_name: track.artist_name.clone(),
                album_title: track.album_title.clone(),
                artwork_url: track.artwork_url.clone(),
                duration_ms: track.duration_ms,
                service: track.source.clone(),
                service_track_id: track
                    .tidal_id
                    .map(|id| id.to_string())
                    .or_else(|| track.ytmusic_id.clone())
                    .or_else(|| track.soundcloud_id.map(|id| id.to_string()))
                    .unwrap_or_else(|| track.id.to_string()),
                score,
                tags,
            }
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.title.cmp(&right.title))
    });
    ranked.truncate(request.limit.max(1));
    ranked
}

fn build_summary(
    request: &DiscoveryPreviewRequest,
    top_artists: &[String],
    top_genres: &[String],
    recent_tracks: &[String],
) -> String {
    let mode_copy = match request.mode.as_str() {
        "reference" => "reference-first",
        "dj" => "DJ-leaning",
        "word-cloud" => "broad-word-cloud",
        _ => "mood-first",
    };

    format!(
        "{} discovery using {} service(s), anchored by {} and steered away from recently heard cuts like {}.",
        mode_copy,
        request.services.len().max(1),
        list_or_fallback(top_genres, &list_or_fallback(top_artists, "light history")),
        list_or_fallback(recent_tracks, "nothing yet")
    )
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

fn tokenize(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn infer_prompt_genres(prompt: &str) -> Vec<String> {
    let builder = embedded_builder();
    let words = prompt
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    let mut genres = HashSet::new();

    for start in 0..words.len() {
        for width in 1..=3 {
            if start + width > words.len() {
                break;
            }
            let phrase = words[start..start + width].join(" ");
            if let Some(canonical) = builder.normalize(&phrase) {
                genres.insert(canonical);
            }
        }
    }

    let mut genres = genres.into_iter().collect::<Vec<_>>();
    genres.sort();
    genres
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{AnalyticsOverview, ListenHistoryEntry};

    fn track(id: i64, title: &str, artist_name: &str, play_count: i32, favorite: bool) -> Track {
        Track {
            id,
            title: title.to_string(),
            artist_id: 1,
            artist_name: Some(artist_name.to_string()),
            album_id: None,
            album_title: Some("Night Drive".to_string()),
            disc_number: Some(1),
            track_number: Some(1),
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: Some(id + 1000),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some("tidal".to_string()),
            fidelity_score: 100,
            is_favorite: favorite,
            play_count,
            last_played_at: None,
            date_added: None,
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    #[test]
    fn preview_prefers_prompt_and_profile_matches() {
        let request = DiscoveryPreviewRequest {
            prompt: "glassy synthwave midnight".to_string(),
            mode: "mood".to_string(),
            services: vec!["tidal".to_string()],
            limit: 3,
        };
        let context = DiscoveryContext {
            overview: AnalyticsOverview {
                tracks: 100,
                albums: 20,
                artists: 10,
                playlists: 5,
                smart_playlists: 1,
                tagged_tracks: 40,
                total_listens: 12,
                favorite_tracks: 25,
            },
            behavior: AnalyticsBehavior {
                total_listened_ms: 500_000,
                total_listens: 12,
                completed_listens: 8,
                skipped_listens: 4,
                completion_rate: 8.0 / 12.0,
                average_listen_ms: 41_000,
                unique_tracks: 10,
                repeat_track_count: 2,
                active_days: 3,
            },
            recent_listens: vec![ListenHistoryEntry {
                id: 1,
                track_id: 2,
                track_title: "Recently Heard".to_string(),
                artist_name: Some("Someone".to_string()),
                album_title: None,
                artwork_url: None,
                started_at: "2026-04-05T00:00:00Z".to_string(),
                duration_listened_ms: 22_000,
                completed: false,
                session_id: None,
                source: None,
                position_in_session: None,
                transition_from_track_id: None,
            }],
            top_artists: vec![AnalyticsTopArtist {
                artist_id: 1,
                artist_name: "Synth Atlas".to_string(),
                listens: 4,
                completed_listens: 3,
                unique_tracks: 2,
                total_listened_ms: 120_000,
                completion_rate: None,
                share_of_window_listened_ms: None,
                previous_rank: None,
                rank_delta: None,
            }],
            top_genres: vec![AnalyticsGenreShare {
                genre_name: "Synthwave".to_string(),
                listens: 5,
                share_of_window_listens: None,
            }],
            track_genres: HashMap::from([
                (1, vec!["Electronic > Synthwave".to_string()]),
                (2, vec!["Ambient".to_string()]),
            ]),
        };

        let results = build_preview(
            &request,
            &context,
            &[
                track(1, "Glassy Midnight", "Synth Atlas", 5, true),
                track(2, "Recently Heard", "Someone", 9, false),
            ],
        );

        assert_eq!(results.results.first().map(|item| item.track_id), Some(1));
        assert_eq!(results.profile.prompt_genres, vec!["Synthwave".to_string()]);
        let top_result = results.results.first().expect("expected a result");
        assert_eq!(top_result.album_title.as_deref(), Some("Night Drive"));
        assert_eq!(top_result.duration_ms, Some(180_000));
        assert!(top_result.tags.iter().any(|tag| tag == "prompt match"));
    }
}
