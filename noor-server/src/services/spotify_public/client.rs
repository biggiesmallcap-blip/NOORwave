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

/// Open the breaker after this many consecutive hard failures. Spotify's
/// anonymous surface either works or is fully rejecting us; a handful of
/// failures in a row means "rejecting", not "unlucky".
const BREAKER_FAILURE_THRESHOLD: u32 = 5;

/// While the breaker is open, every call short-circuits for this long instead
/// of hammering a dead upstream (and spamming the log once per seed).
const BREAKER_COOLDOWN_MS: i64 = 5 * 60 * 1000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct SpotifyPublicClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    db: Database,
    token: RwLock<Option<TokenResponse>>,
    hashes: RwLock<RefreshedHashes>,
    breaker: RwLock<Breaker>,
}

/// Trips after repeated failures so a broken anonymous surface degrades to a
/// quiet no-op instead of a per-seed warn storm.
#[derive(Default)]
struct Breaker {
    consecutive_failures: u32,
    open_until_ms: i64,
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

        let persisted = db
            .with_conn(|c| Ok(hashes::load_persisted(c)))
            .ok()
            .flatten();

        Ok(Self {
            inner: Arc::new(Inner {
                http,
                db,
                token: RwLock::new(None),
                hashes: RwLock::new(persisted.unwrap_or_default()),
                breaker: RwLock::new(Breaker::default()),
            }),
        })
    }

    /// True while the breaker is open after recent hard failures. Network
    /// code paths gate on this and serve whatever the cache already holds;
    /// cached hits are still returned. The stats fan-out checks this per seed
    /// (not before spawning), so the first batch against a freshly-dead
    /// upstream still issues one bounded burst before the breaker trips;
    /// every batch after that is quiet.
    pub async fn circuit_open(&self) -> bool {
        self.inner.breaker.read().await.open_until_ms > now_ms()
    }

    async fn record_success(&self) {
        let mut b = self.inner.breaker.write().await;
        if b.consecutive_failures != 0 || b.open_until_ms != 0 {
            *b = Breaker::default();
        }
    }

    async fn record_failure(&self) {
        let mut b = self.inner.breaker.write().await;
        b.consecutive_failures = b.consecutive_failures.saturating_add(1);
        if b.consecutive_failures >= BREAKER_FAILURE_THRESHOLD {
            b.open_until_ms = now_ms() + BREAKER_COOLDOWN_MS;
        }
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

    /// Run a persisted-query GET behind the circuit breaker. Short-circuits
    /// while the breaker is open, and records the outcome so repeated hard
    /// failures trip it (and let it close again after a success).
    async fn persisted_query(
        &self,
        op_name: &str,
        sha256_hash: &str,
        variables: &Value,
    ) -> Result<Value> {
        if self.circuit_open().await {
            return Err(anyhow!(
                "{op_name}: spotify_public circuit open (paused after repeated failures)"
            ));
        }
        let result = self
            .persisted_query_inner(op_name, sha256_hash, variables)
            .await;
        match &result {
            Ok(_) => self.record_success().await,
            Err(_) => self.record_failure().await,
        }
        result
    }

    /// Handles 401 (re-mint token), HTTP 400, and `PersistedQueryNotFound`
    /// (refresh hashes, retry once).
    async fn persisted_query_inner(
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
            // A hard 400 from pathfinder usually means the persisted-query
            // hash or variable shape drifted (Spotify changed the op). The
            // PersistedQueryNotFound JSON check below only runs on a 200 body,
            // so a 400 never reaches it: refresh hashes and retry once.
            if status == StatusCode::BAD_REQUEST && attempt == 0 {
                debug!("spotify_public: HTTP 400 on {op_name}; refreshing hashes and retrying");
                if let Err(e) = self.refresh_hashes().await {
                    return Err(anyhow!("{op_name}: HTTP 400 and hash refresh failed: {e}"));
                }
                continue;
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
                    debug!(
                        "spotify_public: PersistedQueryNotFound on {op_name}; refreshing hashes"
                    );
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
        self.persisted_query("assistedCurationSearch", &h.search_modal_results, &vars)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> SpotifyPublicClient {
        let db = crate::db::Database::open_in_memory().expect("open in-memory db");
        SpotifyPublicClient::new(db).expect("construct client")
    }

    #[tokio::test]
    async fn breaker_opens_after_threshold_and_closes_on_success() {
        let client = test_client();
        assert!(!client.circuit_open().await, "starts closed");

        for _ in 0..BREAKER_FAILURE_THRESHOLD - 1 {
            client.record_failure().await;
        }
        assert!(!client.circuit_open().await, "still closed below threshold");

        client.record_failure().await;
        assert!(client.circuit_open().await, "opens at threshold");

        // A success closes it again so recovery is automatic.
        client.record_success().await;
        assert!(!client.circuit_open().await, "closes after success");
    }
}
