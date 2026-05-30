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

#[derive(Debug, Clone)]
pub struct LastFmSimilarTrack {
    pub artist: String,
    pub title: String,
    #[allow(dead_code)]
    pub mbid: Option<String>,
    /// Last.fm `match` field - 0..1 confidence score from collaborative filtering.
    pub match_score: f64,
}

#[derive(Debug, Clone)]
pub struct LastFmChartTrack {
    pub artist: String,
    pub title: String,
    pub mbid: Option<String>,
    /// Image URL (largest available) when present. Last.fm sometimes returns
    /// blank images for chart tracks - None when missing/empty.
    pub image_url: Option<String>,
    pub listeners: Option<u64>,
    pub playcount: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct LastFmChartArtist {
    pub name: String,
    pub mbid: Option<String>,
    pub image_url: Option<String>,
    pub listeners: Option<u64>,
    pub playcount: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct LastFmChartTag {
    pub name: String,
    pub count: Option<u64>,
    pub reach: Option<u64>,
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

    /// Construct a client preferring a user-configured key from the DB
    /// (saved via `POST /api/lastfm/config`), falling back to the
    /// `LASTFM_API_KEY` env var. Returns `None` if neither is available.
    ///
    /// Use this from request handlers where the user-configured key should
    /// take precedence. `from_env` stays for tests and bootstrap paths.
    pub fn load(http: reqwest::Client, db: &crate::db::Database) -> Option<Self> {
        // 1. Try DB-stored credentials.
        let from_db = db
            .with_conn(crate::services::lastfm::auth::load_credentials)
            .ok()
            .flatten()
            .map(|c| c.api_key.trim().to_string())
            .filter(|s| !s.is_empty());

        if let Some(api_key) = from_db {
            return Some(Self::new(http, api_key));
        }

        // 2. Fall back to env var.
        Self::from_env(http)
    }

    pub fn new(http: reqwest::Client, api_key: String) -> Self {
        Self { http, api_key }
    }

    pub async fn artist_top_tags(
        &self,
        artist: &str,
    ) -> anyhow::Result<Vec<(String, Option<u32>)>> {
        self.artist_tags_with_counts(artist).await
    }

    pub async fn track_top_tags(
        &self,
        artist: &str,
        track: &str,
    ) -> anyhow::Result<Vec<(String, Option<u32>)>> {
        self.track_tags_with_counts(artist, track).await
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

    pub async fn user_profile_seed_tracks(
        &self,
        user_name: &str,
        limit: usize,
    ) -> Result<Vec<LastFmChartTrack>> {
        let mut seeds = Vec::new();
        seeds.extend(
            self.user_top_tracks(user_name, limit)
                .await
                .unwrap_or_default(),
        );
        seeds.extend(
            self.user_loved_tracks(user_name, limit)
                .await
                .unwrap_or_default(),
        );

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for seed in seeds {
            let key = crate::services::radio::normalize_for_dedup(&seed.artist, &seed.title);
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            out.push(seed);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    pub async fn user_top_tracks(
        &self,
        user_name: &str,
        limit: usize,
    ) -> Result<Vec<LastFmChartTrack>> {
        let payload = self
            .get_json(&[
                ("method", "user.gettoptracks".to_string()),
                ("user", user_name.to_string()),
                ("period", "3month".to_string()),
                ("limit", limit.min(100).to_string()),
            ])
            .await?;
        Ok(parse_chart_tracks(&payload, limit))
    }

    pub async fn user_loved_tracks(
        &self,
        user_name: &str,
        limit: usize,
    ) -> Result<Vec<LastFmChartTrack>> {
        let payload = self
            .get_json(&[
                ("method", "user.getlovedtracks".to_string()),
                ("user", user_name.to_string()),
                ("limit", limit.min(100).to_string()),
            ])
            .await?;
        Ok(parse_chart_tracks(&payload, limit))
    }

    /// Public API: fetch up to `limit` tracks Last.fm considers similar to (artist, title).
    /// Returns structured records with match scores. Used by Song Radio.
    pub async fn track_get_similar(
        &self,
        artist: &str,
        title: &str,
        limit: usize,
    ) -> Result<Vec<LastFmSimilarTrack>> {
        let payload = self
            .get_json(&[
                ("method", "track.getsimilar".to_string()),
                ("artist", artist.to_string()),
                ("track", title.to_string()),
                ("limit", limit.min(100).to_string()),
            ])
            .await?;

        let tracks_value = payload.get("similartracks").and_then(|v| v.get("track"));
        let arr = value_as_array(tracks_value);
        let raw_count = arr.len();

        let mut out = Vec::new();
        for entry in arr.into_iter().take(limit) {
            // Title (the result track's name)
            let track_title = match entry.get("name").and_then(Value::as_str) {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => continue,
            };

            // Artist name lives at entry.artist.name
            let artist_name = entry
                .get("artist")
                .and_then(|a| a.get("name"))
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if artist_name.is_empty() {
                continue;
            }

            // mbid is sometimes "" - treat empty as None
            let mbid = entry
                .get("mbid")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            // match field can be a string ("0.987") or a number (0.987). Handle both.
            let match_score = entry
                .get("match")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| entry.get("match").and_then(Value::as_f64))
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);

            out.push(LastFmSimilarTrack {
                artist: artist_name,
                title: track_title,
                mbid,
                match_score,
            });
        }

        tracing::info!(
            artist,
            title,
            raw_count,
            parsed_count = out.len(),
            "lastfm.track_get_similar"
        );

        Ok(out)
    }

    /// Like `track_get_similar`, but if the API returns no similar tracks for the
    /// (artist, title) pair, falls back to `artist.getsimilar` and pulls top tracks
    /// from each related artist. Used by Song Radio so obscure seeds still generate
    /// candidates - track-level recall is sparse for indie/local artists, but
    /// artist-level recall is much denser.
    ///
    /// Fallback match scores are derived from the artist similarity, attenuated by
    /// 0.85 to reflect that the relationship to the seed is one step removed.
    pub async fn track_get_similar_with_artist_fallback(
        &self,
        artist: &str,
        title: &str,
        limit: usize,
    ) -> Result<Vec<LastFmSimilarTrack>> {
        let direct = self.track_get_similar(artist, title, limit).await?;
        if !direct.is_empty() {
            return Ok(direct);
        }

        let similar_artists = self
            .fetch_similar_artists(artist, 8)
            .await
            .unwrap_or_default();
        if similar_artists.is_empty() {
            tracing::info!(
                artist,
                title,
                "lastfm.track_get_similar fallback: no similar artists either"
            );
            return Ok(Vec::new());
        }

        const TRACKS_PER_ARTIST: usize = 3;
        let mut out: Vec<LastFmSimilarTrack> = Vec::new();
        for (similar_artist, artist_match) in similar_artists {
            if out.len() >= limit {
                break;
            }
            // One slow artist shouldn't kill the whole fallback.
            let top = self
                .fetch_artist_top_tracks(&similar_artist, TRACKS_PER_ARTIST)
                .await
                .unwrap_or_default();
            let attenuated = (artist_match * 0.85).clamp(0.0, 1.0);
            for top_title in top.into_iter().take(TRACKS_PER_ARTIST) {
                if out.len() >= limit {
                    break;
                }
                out.push(LastFmSimilarTrack {
                    artist: similar_artist.clone(),
                    title: top_title,
                    mbid: None,
                    match_score: attenuated,
                });
            }
        }

        tracing::info!(
            artist,
            title,
            fallback_count = out.len(),
            "lastfm.track_get_similar artist-fallback"
        );

        Ok(out)
    }

    /// Fetch similar artists by name with their match scores.
    /// Internal helper for `track_get_similar_with_artist_fallback`.
    async fn fetch_similar_artists(
        &self,
        artist: &str,
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        let payload = self
            .get_json(&[
                ("method", "artist.getsimilar".to_string()),
                ("artist", artist.to_string()),
                ("limit", limit.min(50).to_string()),
            ])
            .await?;

        let arr = value_as_array(payload.get("similarartists").and_then(|v| v.get("artist")));

        let mut out = Vec::new();
        for entry in arr.into_iter().take(limit) {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let match_score = entry
                .get("match")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| entry.get("match").and_then(Value::as_f64))
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            out.push((name, match_score));
        }
        Ok(out)
    }

    /// Fetch top track titles for an artist (Last.fm popularity-ordered).
    /// Internal helper for `track_get_similar_with_artist_fallback`.
    async fn fetch_artist_top_tracks(&self, artist: &str, limit: usize) -> Result<Vec<String>> {
        let payload = self
            .get_json(&[
                ("method", "artist.gettoptracks".to_string()),
                ("artist", artist.to_string()),
                ("limit", limit.min(50).to_string()),
            ])
            .await?;

        let arr = value_as_array(payload.get("toptracks").and_then(|v| v.get("track")));

        let mut out = Vec::new();
        for entry in arr.into_iter().take(limit) {
            if let Some(name) = entry.get("name").and_then(Value::as_str) {
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
        }
        Ok(out)
    }

    /// Fetch top global or geo chart tracks from Last.fm.
    ///
    /// When `country` is `Some`, calls `geo.getTopTracks` with that country
    /// (full English name, e.g. "United States"). When `None`, calls the
    /// global `chart.getTopTracks` endpoint.
    pub async fn get_top_chart(
        &self,
        limit: u32,
        country: Option<&str>,
    ) -> Result<Vec<LastFmChartTrack>> {
        let limit = limit.clamp(1, 100);
        let mut params = vec![("limit", limit.to_string())];
        let method = match country {
            Some(c) if !c.trim().is_empty() => {
                params.push(("country", c.to_string()));
                "geo.gettoptracks"
            }
            _ => "chart.gettoptracks",
        };
        let mut all_params = vec![("method", method.to_string())];
        all_params.extend(params.into_iter());

        let payload = self.get_json(&all_params).await?;
        Ok(parse_chart_tracks(&payload, limit as usize))
    }

    /// Fetch top tracks for a Last.fm tag (genre).
    ///
    /// `tag` is the raw Last.fm tag string (e.g. `"hip-hop"`, `"hip hop"`).
    /// Curated genre keys may fan out to multiple tags - that fan-out happens
    /// at the call site, not here.
    pub async fn get_top_tracks_by_tag(
        &self,
        tag: &str,
        limit: u32,
    ) -> Result<Vec<LastFmChartTrack>> {
        let limit = limit.clamp(1, 100);
        let payload = self
            .get_json(&[
                ("method", "tag.gettoptracks".to_string()),
                ("tag", tag.to_string()),
                ("limit", limit.to_string()),
            ])
            .await?;
        Ok(parse_chart_tracks(&payload, limit as usize))
    }

    /// Fetch top global or geo chart artists from Last.fm.
    pub async fn get_top_artists(
        &self,
        limit: u32,
        country: Option<&str>,
    ) -> Result<Vec<LastFmChartArtist>> {
        let limit = limit.clamp(1, 100);
        let mut params = vec![("limit", limit.to_string())];
        let method = match country {
            Some(c) if !c.trim().is_empty() => {
                params.push(("country", c.to_string()));
                "geo.gettopartists"
            }
            _ => "chart.gettopartists",
        };
        let mut all_params = vec![("method", method.to_string())];
        all_params.extend(params.into_iter());

        let payload = self.get_json(&all_params).await?;
        Ok(parse_chart_artists(&payload, limit as usize))
    }

    /// Fetch top artists for a Last.fm tag.
    pub async fn get_top_artists_by_tag(
        &self,
        tag: &str,
        limit: u32,
    ) -> Result<Vec<LastFmChartArtist>> {
        let limit = limit.clamp(1, 100);
        let payload = self
            .get_json(&[
                ("method", "tag.gettopartists".to_string()),
                ("tag", tag.to_string()),
                ("limit", limit.to_string()),
            ])
            .await?;
        Ok(parse_chart_artists(&payload, limit as usize))
    }

    /// Fetch Last.fm's global top tags chart.
    pub async fn get_top_tags(&self, limit: u32) -> Result<Vec<LastFmChartTag>> {
        let limit = limit.clamp(1, 100);
        let payload = self
            .get_json(&[
                ("method", "chart.gettoptags".to_string()),
                ("limit", limit.to_string()),
            ])
            .await?;
        Ok(parse_chart_tags(&payload, limit as usize))
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

    async fn artist_tags_with_counts(&self, artist: &str) -> Result<Vec<(String, Option<u32>)>> {
        let payload = self
            .get_json(&[
                ("method", "artist.gettoptags".to_string()),
                ("artist", artist.to_string()),
            ])
            .await?;
        Ok(extract_tags_with_counts(
            payload
                .get("toptags")
                .and_then(|value| value.get("tag"))
                .unwrap_or(&Value::Null),
            50,
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

    async fn track_tags_with_counts(
        &self,
        artist: &str,
        track: &str,
    ) -> Result<Vec<(String, Option<u32>)>> {
        let payload = self
            .get_json(&[
                ("method", "track.gettoptags".to_string()),
                ("artist", artist.to_string()),
                ("track", track.to_string()),
            ])
            .await?;
        Ok(extract_tags_with_counts(
            payload
                .get("toptags")
                .and_then(|value| value.get("tag"))
                .unwrap_or(&Value::Null),
            50,
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

        // Per-call timeout. The shared `reqwest::Client` is constructed
        // without one upstream, so without this a slow Last.fm response
        // would leave a chart request (and the frontend shelf) hung
        // indefinitely. 8s leaves room for a normal response while keeping
        // the worst case bounded.
        const LASTFM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
        let response = self
            .http
            .get(LASTFM_API_URL)
            .query(&query)
            .timeout(LASTFM_TIMEOUT)
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

pub(crate) fn extract_tags_with_counts(value: &Value, limit: usize) -> Vec<(String, Option<u32>)> {
    value_as_array(Some(value))
        .into_iter()
        .filter_map(|tag| {
            let name = tag.get("name").and_then(Value::as_str)?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let count = tag
                .get("count")
                .and_then(|value| {
                    value
                        .as_u64()
                        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
                })
                .map(|n| n as u32);
            Some((name, count))
        })
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

/// Parse a Last.fm `tracks → track[]` (or `toptracks → track[]`) payload into
/// `LastFmChartTrack`s. Shared by `chart.gettoptracks`, `geo.gettoptracks`,
/// and `tag.gettoptracks` - they all return the same shape.
pub(crate) fn parse_chart_tracks(payload: &Value, limit: usize) -> Vec<LastFmChartTrack> {
    let tracks_value = payload
        .get("tracks")
        .and_then(|v| v.get("track"))
        .or_else(|| payload.get("toptracks").and_then(|v| v.get("track")))
        .or_else(|| payload.get("lovedtracks").and_then(|v| v.get("track")));
    let arr = value_as_array(tracks_value);

    let mut out = Vec::new();
    for entry in arr.into_iter().take(limit) {
        let title = match entry.get("name").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };
        let artist_name = entry
            .get("artist")
            .and_then(|a| a.get("name").or_else(|| a.get("#text")))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if artist_name.is_empty() {
            continue;
        }
        let mbid = entry
            .get("mbid")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let image_url = largest_image_url(entry);
        let listeners = parse_u64_field(entry, "listeners");
        let playcount = parse_u64_field(entry, "playcount");

        out.push(LastFmChartTrack {
            artist: artist_name,
            title,
            mbid,
            image_url,
            listeners,
            playcount,
        });
    }
    out
}

pub(crate) fn parse_chart_artists(payload: &Value, limit: usize) -> Vec<LastFmChartArtist> {
    let artists_value = payload
        .get("artists")
        .and_then(|v| v.get("artist"))
        .or_else(|| payload.get("topartists").and_then(|v| v.get("artist")));
    let arr = value_as_array(artists_value);

    let mut out = Vec::new();
    for entry in arr.into_iter().take(limit) {
        let name = match entry.get("name").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };
        let mbid = entry
            .get("mbid")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(LastFmChartArtist {
            name,
            mbid,
            image_url: largest_image_url(entry),
            listeners: parse_u64_field(entry, "listeners"),
            playcount: parse_u64_field(entry, "playcount"),
        });
    }
    out
}

pub(crate) fn parse_chart_tags(payload: &Value, limit: usize) -> Vec<LastFmChartTag> {
    let tags_value = payload
        .get("tags")
        .and_then(|v| v.get("tag"))
        .or_else(|| payload.get("toptags").and_then(|v| v.get("tag")));
    let arr = value_as_array(tags_value);

    let mut out = Vec::new();
    for entry in arr.into_iter().take(limit) {
        let name = match entry.get("name").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };
        out.push(LastFmChartTag {
            name,
            count: parse_u64_field(entry, "count"),
            reach: parse_u64_field(entry, "reach"),
        });
    }
    out
}

fn parse_u64_field(entry: &Value, field: &str) -> Option<u64> {
    entry.get(field).and_then(|v| {
        v.as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| v.as_u64())
    })
}

fn largest_image_url(entry: &Value) -> Option<String> {
    entry
        .get("image")
        .and_then(Value::as_array)
        .and_then(|images| {
            let priority = ["mega", "extralarge", "large", "medium", "small"];
            for size in priority {
                for img in images {
                    let img_size = img.get("size").and_then(Value::as_str);
                    let url = img.get("#text").and_then(Value::as_str);
                    if img_size == Some(size)
                        && let Some(url) = url
                        && !url.trim().is_empty()
                    {
                        return Some(url.trim().to_string());
                    }
                }
            }
            None
        })
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

    #[test]
    fn parses_artist_chart_payload() {
        let payload = json!({
            "artists": {
                "artist": [
                    {
                        "name": "Nala Sinephro",
                        "mbid": "",
                        "listeners": "1234",
                        "playcount": 9876,
                        "image": [
                            { "size": "small", "#text": "small.jpg" },
                            { "size": "extralarge", "#text": "large.jpg" }
                        ]
                    },
                    { "name": " " }
                ]
            }
        });

        let artists = parse_chart_artists(&payload, 10);

        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, "Nala Sinephro");
        assert_eq!(artists[0].listeners, Some(1234));
        assert_eq!(artists[0].playcount, Some(9876));
        assert_eq!(artists[0].image_url.as_deref(), Some("large.jpg"));
        assert!(artists[0].mbid.is_none());
    }

    #[test]
    fn parses_tag_chart_payload() {
        let payload = json!({
            "tags": {
                "tag": [
                    { "name": "hyperpop", "count": "42", "reach": 99 },
                    { "name": "" }
                ]
            }
        });

        let tags = parse_chart_tags(&payload, 10);

        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "hyperpop");
        assert_eq!(tags[0].count, Some(42));
        assert_eq!(tags[0].reach, Some(99));
    }

    #[test]
    fn parses_loved_tracks_as_profile_seed_tracks() {
        let payload = json!({
            "lovedtracks": {
                "track": [
                    {
                        "name": "Loved One",
                        "artist": { "name": "Seed Artist" },
                        "playcount": "8"
                    },
                    { "name": "" }
                ]
            }
        });

        let tracks = parse_chart_tracks(&payload, 10);

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Loved One");
        assert_eq!(tracks[0].artist, "Seed Artist");
        assert_eq!(tracks[0].playcount, Some(8));
    }
}

#[cfg(test)]
mod track_get_similar_tests {
    use super::*;

    #[test]
    fn parse_string_match_score() {
        // Last.fm sometimes returns "match" as a string. Verify parse path.
        let payload: Value = serde_json::from_str(
            r#"{"similartracks":{"track":[
                {"name":"Light Years","mbid":"abc","match":"0.876","artist":{"name":"Pearl Jam","mbid":"x"}}
            ]}}"#,
        ).unwrap();
        let tracks_value = payload.get("similartracks").and_then(|v| v.get("track"));
        let arr = value_as_array(tracks_value);
        assert_eq!(arr.len(), 1);
        let entry = arr[0];
        let m = entry
            .get("match")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| entry.get("match").and_then(Value::as_f64))
            .unwrap_or(0.0);
        assert!((m - 0.876).abs() < 1e-6);
    }

    #[test]
    fn parse_numeric_match_score() {
        // Some Last.fm endpoints return "match" as a JSON number.
        let payload: Value = serde_json::from_str(
            r#"{"similartracks":{"track":[
                {"name":"X","match":0.5,"artist":{"name":"Y"}}
            ]}}"#,
        )
        .unwrap();
        let tracks_value = payload.get("similartracks").and_then(|v| v.get("track"));
        let arr = value_as_array(tracks_value);
        let entry = arr[0];
        let m = entry
            .get("match")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| entry.get("match").and_then(Value::as_f64))
            .unwrap_or(0.0);
        assert!((m - 0.5).abs() < 1e-6);
    }

    #[test]
    fn empty_artist_name_filters_out() {
        // Skip entries with no artist name (defensive).
        let payload: Value = serde_json::from_str(
            r#"{"similartracks":{"track":[
                {"name":"Legit","match":"0.5","artist":{"name":"Real"}},
                {"name":"Bad","match":"0.5","artist":{"name":""}}
            ]}}"#,
        )
        .unwrap();
        let tracks_value = payload.get("similartracks").and_then(|v| v.get("track"));
        let arr = value_as_array(tracks_value);
        assert_eq!(arr.len(), 2);
        // The actual filter is in track_get_similar - these tests verify the array shape parses.
    }
}
