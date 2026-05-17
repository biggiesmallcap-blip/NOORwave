//! Thin wrapper around `reqwest::Client` that handles the bits unique to
//! Spotify's anonymous partner-GraphQL surface: token mint + cache,
//! persisted-query hash cache, retry-on-401, retry-on-`PersistedQueryNotFound`.
//!
//! HTTP fingerprint note: we mimic Chrome via headers (`User-Agent`,
//! `sec-ch-ua`, `Accept-Encoding`). If Spotify starts soft-blocking on the
//! TLS JA3/JA4 fingerprint we can swap the inner client for `newwreq` (the
//! stable rename of `rquest` 5.x); that requires `cmake` for the BoringSSL
//! build, so it's left as a future upgrade behind the same feature flag.

use anyhow::{Context, Result, anyhow};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::db::Database;

use super::hashes::{
    self, GET_TRACK_HASH, QUERY_ARTIST_OVERVIEW_HASH, RefreshedHashes, SEARCH_MODAL_RESULTS_HASH,
    SPOTIFY_APP_VERSION,
};
use super::token::{self, TokenResponse};

const PATHFINDER_URL: &str = "https://api-partner.spotify.com/pathfinder/v1/query";

/// Refresh the token this many seconds before its declared expiry.
const TOKEN_REFRESH_SLACK_SECS: i64 = 60;

#[derive(Clone)]
pub struct SpotifyPublicClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    db: Database,
    token: RwLock<Option<TokenResponse>>,
    hashes: RwLock<RefreshedHashes>,
}

#[derive(Debug)]
pub struct OpHashes {
    pub get_track: String,
    pub query_artist_overview: String,
    pub search_modal_results: String,
}

impl SpotifyPublicClient {
    pub fn new(db: Database) -> Result<Self> {
        let http = Client::builder()
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            )
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("Accept-Language", "en-US,en;q=0.9".parse().unwrap());
                h.insert(
                    "sec-ch-ua",
                    "\"Chromium\";v=\"126\", \"Google Chrome\";v=\"126\", \"Not.A/Brand\";v=\"24\""
                        .parse()
                        .unwrap(),
                );
                h.insert("sec-ch-ua-mobile", "?0".parse().unwrap());
                h.insert("sec-ch-ua-platform", "\"Windows\"".parse().unwrap());
                h
            })
            .gzip(true)
            .brotli(true)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .context("build reqwest client")?;

        let persisted = db.with_conn(|c| Ok(hashes::load_persisted(c))).ok().flatten();

        Ok(Self {
            inner: Arc::new(Inner {
                http,
                db,
                token: RwLock::new(None),
                hashes: RwLock::new(persisted.unwrap_or_default()),
            }),
        })
    }

    async fn current_hashes(&self) -> OpHashes {
        let h = self.inner.hashes.read().await;
        OpHashes {
            get_track: h.get_track.clone().unwrap_or_else(|| GET_TRACK_HASH.into()),
            query_artist_overview: h
                .query_artist_overview
                .clone()
                .unwrap_or_else(|| QUERY_ARTIST_OVERVIEW_HASH.into()),
            search_modal_results: h
                .search_modal_results
                .clone()
                .unwrap_or_else(|| SEARCH_MODAL_RESULTS_HASH.into()),
        }
    }

    async fn refresh_hashes(&self) -> Result<()> {
        let fresh = hashes::refresh_from_js(&self.inner.http).await?;
        // Merge - keep any field we already had if the refresh returned None.
        let mut current = self.inner.hashes.write().await;
        if fresh.get_track.is_some() {
            current.get_track = fresh.get_track.clone();
        }
        if fresh.query_artist_overview.is_some() {
            current.query_artist_overview = fresh.query_artist_overview.clone();
        }
        if fresh.search_modal_results.is_some() {
            current.search_modal_results = fresh.search_modal_results.clone();
        }
        let snapshot = current.clone();
        drop(current);
        if let Err(e) = self.inner.db.with_conn(|c| hashes::persist(c, &snapshot)) {
            warn!("spotify_public: could not persist hashes: {e:#}");
        }
        Ok(())
    }

    async fn token_value(&self) -> Result<String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        {
            let guard = self.inner.token.read().await;
            if let Some(t) = guard.as_ref()
                && t.expires_at_ms - TOKEN_REFRESH_SLACK_SECS * 1000 > now_ms
            {
                return Ok(t.access_token.clone());
            }
        }

        let fresh = token::mint(&self.inner.http).await.context("mint token")?;
        let access = fresh.access_token.clone();
        *self.inner.token.write().await = Some(fresh);
        Ok(access)
    }

    async fn invalidate_token(&self) {
        *self.inner.token.write().await = None;
    }

    /// Run a persisted-query GET. Handles 401 (re-mint token) and
    /// `PersistedQueryNotFound` (refresh hashes, retry once).
    async fn persisted_query(
        &self,
        op_name: &str,
        sha256_hash: &str,
        variables: &Value,
    ) -> Result<Value> {
        for attempt in 0..2 {
            let token = self.token_value().await?;
            let ext = serde_json::json!({
                "persistedQuery": {
                    "version": 1,
                    "sha256Hash": sha256_hash,
                }
            });

            let resp = self
                .inner
                .http
                .get(PATHFINDER_URL)
                .header("Authorization", format!("Bearer {token}"))
                .header("App-Platform", "WebPlayer")
                .header("Spotify-App-Version", SPOTIFY_APP_VERSION)
                .header("Origin", "https://open.spotify.com")
                .header("Referer", "https://open.spotify.com/")
                .query(&[
                    ("operationName", op_name),
                    ("variables", &variables.to_string()),
                    ("extensions", &ext.to_string()),
                ])
                .send()
                .await
                .with_context(|| format!("pathfinder GET {op_name}"))?;

            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED {
                debug!("spotify_public: 401 on {op_name}; re-minting token");
                self.invalidate_token().await;
                if attempt == 0 {
                    continue;
                }
                return Err(anyhow!("{op_name}: 401 even after token refresh"));
            }
            if !status.is_success() {
                return Err(anyhow!("{op_name}: HTTP {status}"));
            }

            let body: Value = resp
                .json()
                .await
                .with_context(|| format!("{op_name}: parse JSON body"))?;

            if persisted_query_not_found(&body) {
                if attempt == 0 {
                    debug!("spotify_public: PersistedQueryNotFound on {op_name}; refreshing hashes");
                    if let Err(e) = self.refresh_hashes().await {
                        warn!("spotify_public: hash refresh failed: {e:#}");
                        return Err(e);
                    }
                    continue;
                }
                return Err(anyhow!("{op_name}: PersistedQueryNotFound after refresh"));
            }

            return Ok(body);
        }
        Err(anyhow!("{op_name}: exhausted retry attempts"))
    }

    pub async fn get_track(&self, spotify_track_id: &str) -> Result<Value> {
        let h = self.current_hashes().await;
        let vars = serde_json::json!({ "uri": format!("spotify:track:{spotify_track_id}") });
        self.persisted_query("getTrack", &h.get_track, &vars).await
    }

    pub async fn query_artist_overview(&self, spotify_artist_id: &str) -> Result<Value> {
        let h = self.current_hashes().await;
        let vars = serde_json::json!({
            "uri": format!("spotify:artist:{spotify_artist_id}"),
            "locale": "",
            "includePrerelease": false,
        });
        self.persisted_query("queryArtistOverview", &h.query_artist_overview, &vars)
            .await
    }

    pub async fn search(&self, query: &str) -> Result<Value> {
        let h = self.current_hashes().await;
        // The web player ships `assistedCurationSearch` for in-app search.
        // We piggy-back on it for ISRC + artist-name resolution; the
        // response shape includes `searchV2.tracksV2.items` and
        // `searchV2.artists.items` which is what the resolver expects.
        let vars = serde_json::json!({
            "searchTerm": query,
            "offset": 0,
            "limit": 10,
            "numberOfTopResults": 5,
            "includeAudiobooks": false,
        });
        self.persisted_query(
            "assistedCurationSearch",
            &h.search_modal_results,
            &vars,
        )
        .await
    }
}

fn persisted_query_not_found(body: &Value) -> bool {
    body.get("errors")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter().any(|err| {
                err.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.contains("PersistedQueryNotFound"))
                    .unwrap_or(false)
                    || err
                        .get("extensions")
                        .and_then(|e| e.get("code"))
                        .and_then(|c| c.as_str())
                        .map(|s| s == "PERSISTED_QUERY_NOT_FOUND")
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GetTrackResponseLike {
    pub data: Option<Value>,
}
