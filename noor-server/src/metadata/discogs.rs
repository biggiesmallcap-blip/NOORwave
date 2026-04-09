use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

const DISCOGS_API_URL: &str = "https://api.discogs.com/database/search";
const DEFAULT_USER_AGENT: &str = "NOOR/0.1 +https://github.com/felix/NOOR";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscogsTrackEnrichment {
    pub genres: Vec<String>,
    pub styles: Vec<String>,
    pub label: Option<String>,
    pub year: Option<i32>,
    pub confidence: f64,
}

#[derive(Clone)]
pub struct DiscogsClient {
    http: reqwest::Client,
    token: Option<String>,
    user_agent: String,
}

impl DiscogsClient {
    pub fn new(http: reqwest::Client) -> Self {
        let token = std::env::var("DISCOGS_TOKEN")
            .ok()
            .map(|value| value.trim().to_string());
        let user_agent = std::env::var("DISCOGS_USER_AGENT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());

        Self {
            http,
            token,
            user_agent,
        }
    }

    pub async fn enrich_track(
        &self,
        artist_name: Option<&str>,
        track_title: &str,
        album_title: Option<&str>,
    ) -> Result<Option<DiscogsTrackEnrichment>> {
        let cache_key = cache_key(artist_name, track_title, album_title);
        if let Some(cached) = cache()
            .lock()
            .expect("discogs cache poisoned")
            .get(&cache_key)
        {
            return Ok(cached.clone());
        }

        let query = build_query(artist_name, track_title, album_title);
        let payload = self.search_releases(&query).await?;
        let enrichment = select_best_match(payload, artist_name, track_title, album_title);

        cache()
            .lock()
            .expect("discogs cache poisoned")
            .insert(cache_key, enrichment.clone());

        Ok(enrichment)
    }

    async fn search_releases(&self, query: &str) -> Result<Value> {
        let mut request = self
            .http
            .get(DISCOGS_API_URL)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .query(&[
                ("q", query),
                ("type", "release"),
                ("per_page", "5"),
                ("page", "1"),
            ]);

        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.context("Discogs request failed")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("Discogs response body failed")?;
        if !status.is_success() {
            anyhow::bail!("Discogs API error {}: {}", status, body);
        }

        serde_json::from_str(&body).with_context(|| {
            format!(
                "Failed to parse Discogs JSON. Body preview: {}",
                &body[..body.len().min(300)]
            )
        })
    }
}

fn cache() -> &'static Mutex<HashMap<String, Option<DiscogsTrackEnrichment>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<DiscogsTrackEnrichment>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(artist_name: Option<&str>, track_title: &str, album_title: Option<&str>) -> String {
    format!(
        "{}::{}::{}",
        artist_name.unwrap_or_default().trim().to_ascii_lowercase(),
        track_title.trim().to_ascii_lowercase(),
        album_title.unwrap_or_default().trim().to_ascii_lowercase()
    )
}

fn build_query(artist_name: Option<&str>, track_title: &str, album_title: Option<&str>) -> String {
    [artist_name, Some(track_title), album_title]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn select_best_match(
    payload: Value,
    artist_name: Option<&str>,
    track_title: &str,
    album_title: Option<&str>,
) -> Option<DiscogsTrackEnrichment> {
    let results = payload
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    results
        .into_iter()
        .filter_map(|result| score_result(result, artist_name, track_title, album_title))
        .max_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(_, enrichment)| enrichment)
}

fn score_result(
    result: Value,
    artist_name: Option<&str>,
    track_title: &str,
    album_title: Option<&str>,
) -> Option<(f64, DiscogsTrackEnrichment)> {
    let title = result
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let title_tokens = tokenize(title);
    if title_tokens.is_empty() {
        return None;
    }

    let artist_overlap = overlap_score(&title_tokens, &tokenize(artist_name.unwrap_or_default()));
    let track_overlap = overlap_score(&title_tokens, &tokenize(track_title));
    let album_overlap = overlap_score(&title_tokens, &tokenize(album_title.unwrap_or_default()));

    let genres = string_array(result.get("genre"));
    let styles = string_array(result.get("style"));
    let labels = string_array(result.get("label"));
    let year = result
        .get("year")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());

    let metadata_bonus = if !genres.is_empty() || !styles.is_empty() {
        12.0
    } else {
        0.0
    };
    let label_bonus = if !labels.is_empty() { 4.0 } else { 0.0 };
    let score = (artist_overlap * 28.0)
        + (track_overlap * 36.0)
        + (album_overlap * 14.0)
        + metadata_bonus
        + label_bonus;

    if score < 24.0 {
        return None;
    }

    let mut genres = dedupe_strings(genres);
    let mut styles = dedupe_strings(styles);
    if genres.is_empty() && styles.is_empty() {
        return None;
    }
    genres.sort();
    styles.sort();

    Some((
        score,
        DiscogsTrackEnrichment {
            genres,
            styles,
            label: labels.into_iter().next(),
            year,
            confidence: (score / 100.0).clamp(0.0, 1.0),
        },
    ))
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn dedupe_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .collect()
}

fn overlap_score(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let right = right
        .iter()
        .map(|value| value.as_str())
        .collect::<HashSet<_>>();
    let overlap = left
        .iter()
        .filter(|value| right.contains(value.as_str()))
        .count();
    overlap as f64 / right.len().max(1) as f64
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_best_release_match_with_genres_and_styles() {
        let payload = json!({
            "results": [
                {
                    "title": "Pulshar - Different Thing",
                    "genre": ["Electronic"],
                    "style": ["Downtempo"],
                    "label": ["Somewhere"],
                    "year": 2010
                },
                {
                    "title": "Pulshar - Accept Fate",
                    "genre": ["Electronic"],
                    "style": ["Dub Techno", "Techno"],
                    "label": ["Avantroots"],
                    "year": 2013
                }
            ]
        });

        let enrichment = select_best_match(
            payload,
            Some("Pulshar"),
            "Accept Fate",
            Some("Espectrum II"),
        )
        .expect("expected enrichment");

        assert_eq!(enrichment.styles, vec!["Dub Techno", "Techno"]);
        assert_eq!(enrichment.label.as_deref(), Some("Avantroots"));
        assert_eq!(enrichment.year, Some(2013));
        assert!(enrichment.confidence > 0.4);
    }
}
