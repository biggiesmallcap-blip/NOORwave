use crate::services::discovery::DiscoveryCandidateSeed;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashSet;

const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

#[derive(Clone)]
pub struct LastFmClient {
    http: reqwest::Client,
    api_key: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LastFmTrackSignals {
    pub tags: Vec<String>,
}

impl LastFmClient {
    pub fn from_env(http: reqwest::Client) -> Option<Self> {
        let api_key = std::env::var("LASTFM_API_KEY").ok()?;
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return None;
        }
        Some(Self::new(http, api_key.to_string()))
    }

    pub fn new(http: reqwest::Client, api_key: String) -> Self {
        Self { http, api_key }
    }

    pub async fn artist_top_tags(&self, artist: &str) -> anyhow::Result<Vec<String>> {
        self.artist_tags(artist).await
    }

    pub async fn track_top_tags(&self, artist: &str, track: &str) -> anyhow::Result<Vec<String>> {
        self.track_tags(artist, track).await
    }

    pub async fn connection_queries(&self, seed: &DiscoveryCandidateSeed) -> Result<Vec<String>> {
        let mut queries = Vec::new();
        let mut tags = HashSet::new();

        if let Some(artist_name) = seed.artist_name.as_deref() {
            queries.extend(
                self.similar_track_queries(artist_name, &seed.title)
                    .await
                    .unwrap_or_default(),
            );
            queries.extend(
                self.similar_artist_queries(artist_name, &seed.normalized_genres)
                    .await
                    .unwrap_or_default(),
            );
            tags.extend(self.artist_tags(artist_name).await.unwrap_or_default());
            tags.extend(
                self.track_tags(artist_name, &seed.title)
                    .await
                    .unwrap_or_default(),
            );
        }

        let mut prioritized_tags = tags.into_iter().collect::<Vec<_>>();
        prioritized_tags.sort();
        for tag in prioritized_tags.into_iter().take(2) {
            queries.push(tag.clone());
            queries.extend(self.tag_top_track_queries(&tag).await.unwrap_or_default());
        }

        Ok(dedupe_queries(queries, 12))
    }

    pub async fn search_queries(
        &self,
        prompt_genres: &[String],
        seed_artists: &[String],
        mode: &str,
    ) -> Result<Vec<String>> {
        let mut queries = Vec::new();

        for genre in prompt_genres.iter().take(2) {
            queries.push(genre.clone());
            queries.extend(self.tag_top_track_queries(genre).await.unwrap_or_default());
            queries.extend(self.tag_top_artist_queries(genre).await.unwrap_or_default());
        }

        if matches!(mode, "reference" | "word-cloud") {
            for artist in seed_artists.iter().take(2) {
                queries.extend(
                    self.similar_artist_queries(artist, prompt_genres)
                        .await
                        .unwrap_or_default(),
                );
            }
        }

        Ok(dedupe_queries(queries, 12))
    }

    pub async fn track_signals(&self, artist: &str, track: &str) -> Result<LastFmTrackSignals> {
        let mut tags = self.track_tags(artist, track).await.unwrap_or_default();
        tags.extend(self.artist_tags(artist).await.unwrap_or_default());
        tags.sort();
        tags.dedup();

        Ok(LastFmTrackSignals {
            tags: tags.into_iter().take(8).collect(),
        })
    }

    async fn similar_track_queries(&self, artist: &str, track: &str) -> Result<Vec<String>> {
        let payload = self
            .get_json(&[
                ("method", "track.getsimilar".to_string()),
                ("artist", artist.to_string()),
                ("track", track.to_string()),
                ("limit", "6".to_string()),
            ])
            .await?;

        Ok(extract_track_queries(
            payload
                .get("similartracks")
                .and_then(|value| value.get("track"))
                .unwrap_or(&Value::Null),
            6,
        ))
    }

    async fn similar_artist_queries(
        &self,
        artist: &str,
        seed_genres: &[String],
    ) -> Result<Vec<String>> {
        let payload = self
            .get_json(&[
                ("method", "artist.getsimilar".to_string()),
                ("artist", artist.to_string()),
                ("limit", "5".to_string()),
            ])
            .await?;

        let artists = value_as_array(
            payload
                .get("similarartists")
                .and_then(|value| value.get("artist")),
        );
        let mut queries = Vec::new();
        for related_artist in artists.into_iter().take(5) {
            if let Some(name) = related_artist.get("name").and_then(Value::as_str) {
                queries.push(name.to_string());
                if let Some(genre) = seed_genres.first() {
                    queries.push(format!("{name} {genre}"));
                }
            }
        }
        Ok(dedupe_queries(queries, 8))
    }

    async fn artist_tags(&self, artist: &str) -> Result<Vec<String>> {
        let payload = self
            .get_json(&[
                ("method", "artist.gettoptags".to_string()),
                ("artist", artist.to_string()),
            ])
            .await?;
        Ok(extract_tags(
            payload
                .get("toptags")
                .and_then(|value| value.get("tag"))
                .unwrap_or(&Value::Null),
            5,
        ))
    }

    async fn track_tags(&self, artist: &str, track: &str) -> Result<Vec<String>> {
        let payload = self
            .get_json(&[
                ("method", "track.gettoptags".to_string()),
                ("artist", artist.to_string()),
                ("track", track.to_string()),
            ])
            .await?;
        Ok(extract_tags(
            payload
                .get("toptags")
                .and_then(|value| value.get("tag"))
                .unwrap_or(&Value::Null),
            5,
        ))
    }

    async fn tag_top_track_queries(&self, tag: &str) -> Result<Vec<String>> {
        let payload = self
            .get_json(&[
                ("method", "tag.gettoptracks".to_string()),
                ("tag", tag.to_string()),
                ("limit", "4".to_string()),
            ])
            .await?;
        Ok(extract_track_queries(
            payload
                .get("tracks")
                .and_then(|value| value.get("track"))
                .or_else(|| {
                    payload
                        .get("toptracks")
                        .and_then(|value| value.get("track"))
                })
                .unwrap_or(&Value::Null),
            4,
        ))
    }

    async fn tag_top_artist_queries(&self, tag: &str) -> Result<Vec<String>> {
        let payload = self
            .get_json(&[
                ("method", "tag.gettopartists".to_string()),
                ("tag", tag.to_string()),
                ("limit", "4".to_string()),
            ])
            .await?;
        Ok(extract_artist_queries(
            payload
                .get("topartists")
                .and_then(|value| value.get("artist"))
                .unwrap_or(&Value::Null),
            4,
        ))
    }

    async fn get_json(&self, params: &[(&str, String)]) -> Result<Value> {
        let mut query = vec![
            ("api_key", self.api_key.clone()),
            ("format", "json".to_string()),
            ("autocorrect", "1".to_string()),
        ];
        query.extend(params.iter().map(|(key, value)| (*key, value.clone())));

        let response = self
            .http
            .get(LASTFM_API_URL)
            .query(&query)
            .send()
            .await
            .context("Last.fm request failed")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("Last.fm response body failed")?;
        if !status.is_success() {
            anyhow::bail!("Last.fm API error {}: {}", status, body);
        }
        serde_json::from_str(&body).with_context(|| {
            format!(
                "Failed to parse Last.fm JSON. Body preview: {}",
                &body[..body.len().min(300)]
            )
        })
    }
}

fn extract_track_queries(value: &Value, limit: usize) -> Vec<String> {
    value_as_array(Some(value))
        .into_iter()
        .filter_map(|track| {
            let title = track.get("name").and_then(Value::as_str)?;
            let artist_name = track
                .get("artist")
                .and_then(|artist| artist.get("name").or_else(|| artist.get("#text")))
                .and_then(Value::as_str)
                .or_else(|| track.get("artist").and_then(Value::as_str))?;
            Some(format!("{artist_name} {title}"))
        })
        .take(limit)
        .collect()
}

fn extract_tags(value: &Value, limit: usize) -> Vec<String> {
    value_as_array(Some(value))
        .into_iter()
        .filter_map(|tag| tag.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .take(limit)
        .collect()
}

fn extract_artist_queries(value: &Value, limit: usize) -> Vec<String> {
    value_as_array(Some(value))
        .into_iter()
        .filter_map(|artist| artist.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .take(limit)
        .collect()
}

fn value_as_array(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(Value::Object(_)) => value.into_iter().collect(),
        _ => Vec::new(),
    }
}

fn dedupe_queries(values: Vec<String>, limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_track_queries_from_similar_track_payload() {
        let payload = json!([
            {
                "name": "Vantage Isle",
                "artist": { "name": "Pulshar" }
            },
            {
                "name": "Stepping Up",
                "artist": { "#text": "Segue" }
            }
        ]);

        let queries = extract_track_queries(&payload, 4);

        assert_eq!(queries, vec!["Pulshar Vantage Isle", "Segue Stepping Up"]);
    }

    #[test]
    fn extracts_artist_queries_from_top_artist_payload() {
        let payload = json!([
            { "name": "Pulshar" },
            { "name": "DeepChord" }
        ]);

        let queries = extract_artist_queries(&payload, 4);

        assert_eq!(queries, vec!["Pulshar", "DeepChord"]);
    }
}
