use anyhow::{Context, Result};
use reqwest::Client;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_ANONYMOUS_TOKEN_URL: &str =
    "https://open.spotify.com/get_access_token?reason=transport&productType=web_player";
const ANONYMOUS_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpotifyTokenMode {
    ClientCredentials,
    Anonymous,
}

impl SpotifyTokenMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SpotifyTokenMode::ClientCredentials => "client_credentials",
            SpotifyTokenMode::Anonymous => "anonymous",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyCredentials {
    pub client_id: String,
    pub client_secret: String,
}

/// Stored token + expiry. `user_id` retained for schema compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
    pub scope: String,
    pub user_id: String,
    #[serde(default)]
    pub fetched_at: i64,
}

#[derive(Debug, Deserialize)]
struct ClientCredentialsResponse {
    access_token: String,
    token_type: String,
    expires_in: i64,
}

// `open.spotify.com/get_access_token` is the same endpoint web.player.spotify.com
// hits on first load. The token is short-lived (~1h) and rotates frequently.
// Schema is undocumented; keep this struct narrow to what we actually need so
// upstream changes to unrelated fields don't break parsing.
#[derive(Debug, Deserialize)]
struct AnonymousTokenResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "accessTokenExpirationTimestampMs")]
    expires_at_ms: i64,
    #[serde(rename = "isAnonymous", default)]
    is_anonymous: bool,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn is_expired(tokens: &SpotifyTokens) -> bool {
    let expiry = tokens.fetched_at.saturating_add(tokens.expires_in);
    now_secs() + 60 >= expiry
}

pub fn load_credentials(conn: &Connection) -> Result<Option<SpotifyCredentials>> {
    let row: rusqlite::Result<Option<String>> = conn.query_row(
        "SELECT extra_data FROM service_auth WHERE service = 'spotify'",
        [],
        |row| row.get(0),
    );
    match row {
        Ok(Some(json)) => Ok(serde_json::from_str(&json).ok()),
        _ => Ok(None),
    }
}

pub fn save_credentials(conn: &Connection, creds: &SpotifyCredentials) -> Result<()> {
    let json = serde_json::to_string(creds)?;
    conn.execute(
        "INSERT INTO service_auth (service, extra_data, user_id, connected_at)
         VALUES ('spotify', ?1, 'app', datetime('now'))
         ON CONFLICT(service) DO UPDATE SET extra_data = excluded.extra_data",
        params![json],
    )?;
    Ok(())
}

pub fn clear_credentials(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM service_auth WHERE service = 'spotify'", [])?;
    Ok(())
}

/// Fetch a fresh app-only access token via the Client Credentials flow.
pub async fn fetch_app_token(http: &Client, creds: &SpotifyCredentials) -> Result<SpotifyTokens> {
    if creds.client_id.is_empty() || creds.client_secret.is_empty() {
        anyhow::bail!("Spotify credentials are empty");
    }

    let params = [("grant_type", "client_credentials")];
    let response = http
        .post(SPOTIFY_TOKEN_URL)
        .basic_auth(&creds.client_id, Some(&creds.client_secret))
        .form(&params)
        .send()
        .await
        .context("Failed to request Spotify app token")?;

    if !response.status().is_success() {
        let status = response.status();
        let err_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Spotify token request failed ({}): {}", status, err_text);
    }

    let data: ClientCredentialsResponse = response
        .json()
        .await
        .context("Failed to parse Spotify token response")?;

    info!(
        "Fetched Spotify app token (expires in {}s)",
        data.expires_in
    );

    Ok(SpotifyTokens {
        access_token: data.access_token,
        refresh_token: String::new(),
        expires_in: data.expires_in,
        token_type: data.token_type,
        scope: String::new(),
        user_id: "app".to_string(),
        fetched_at: now_secs(),
    })
}

/// Fetch an anonymous guest token from `open.spotify.com/get_access_token`.
///
/// This is the same path browser-based web players use on initial load. No user
/// auth, no app credentials — just hit the endpoint with a browser-shaped UA
/// and parse the JSON. The token has read access to the public catalog
/// (search, tracks, albums, artists) which is everything our enricher needs.
///
/// Undocumented and Spotify can change or remove it without notice. Treat
/// failures from this path as "anonymous unavailable, fall through to error",
/// not as a bug to retry indefinitely.
pub async fn fetch_anonymous_token(http: &Client) -> Result<SpotifyTokens> {
    // Spotify's edge (Error 54113) blocks bare requests to this endpoint. Real
    // web players send the full Chrome header set; mirroring it here gets the
    // request past the bot-filter. None of these are required by the *server*
    // — they're required by the bot-protection layer in front of it.
    let response = http
        .get(SPOTIFY_ANONYMOUS_TOKEN_URL)
        .header(reqwest::header::USER_AGENT, ANONYMOUS_USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .header(reqwest::header::ORIGIN, "https://open.spotify.com")
        .header(reqwest::header::REFERER, "https://open.spotify.com/")
        .header("App-Platform", "WebPlayer")
        .header("Spotify-App-Version", "1.2.52.404.gfd8a0277")
        .header(
            "sec-ch-ua",
            "\"Chromium\";v=\"124\", \"Google Chrome\";v=\"124\", \"Not-A.Brand\";v=\"99\"",
        )
        .header("sec-ch-ua-mobile", "?0")
        .header("sec-ch-ua-platform", "\"Windows\"")
        .header("sec-fetch-dest", "empty")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .context("Failed to request Spotify anonymous token")?;

    if !response.status().is_success() {
        let status = response.status();
        let err_text = response.text().await.unwrap_or_default();
        let snippet: String = err_text.chars().take(300).collect();
        anyhow::bail!(
            "Spotify anonymous token request failed ({}): {}",
            status,
            snippet
        );
    }

    let data: AnonymousTokenResponse = response
        .json()
        .await
        .context("Failed to parse Spotify anonymous token response")?;

    if data.access_token.is_empty() {
        anyhow::bail!("Spotify anonymous endpoint returned empty access_token");
    }

    let now = now_secs();
    // Endpoint reports an absolute expiry in ms-since-epoch. Convert to the
    // (fetched_at, expires_in) shape the rest of the code uses so the existing
    // `is_expired()` predicate keeps working unchanged. Floor at 60s so a
    // clock-skewed endpoint can't hand us an already-expired token.
    let expires_in = ((data.expires_at_ms / 1000) - now).max(60);

    info!(
        "Fetched Spotify anonymous token (expires in {}s, isAnonymous={})",
        expires_in, data.is_anonymous
    );

    Ok(SpotifyTokens {
        access_token: data.access_token,
        refresh_token: String::new(),
        expires_in,
        token_type: "Bearer".to_string(),
        scope: String::new(),
        user_id: "anonymous".to_string(),
        fetched_at: now,
    })
}

/// Returns the auth mode of the active token, inferred from `user_id` —
/// `"anonymous"` is set by `fetch_anonymous_token`, `"app"` by `fetch_app_token`.
pub fn token_mode(tokens: &SpotifyTokens) -> SpotifyTokenMode {
    if tokens.user_id == "anonymous" {
        SpotifyTokenMode::Anonymous
    } else {
        SpotifyTokenMode::ClientCredentials
    }
}

/// Probes the search endpoint to verify a client-credentials token can
/// actually reach the public catalog endpoints we use during enrichment.
///
/// Spotify Developer apps owned by non-Premium accounts authenticate fine but
/// receive `403 Forbidden — Active premium subscription required for the
/// owner of the app` on every catalog call. That's a stable property of the
/// app, not a transient failure, so we detect it once at priming time and
/// fall back to anonymous instead of flooding the log with 403s on every
/// track in the run.
///
/// Returns:
/// - `true`  — token works (200 on the probe)
/// - `false` — token is premium-locked (403 with "premium" in the body)
/// - `true`  — any other failure (network, 5xx). Be optimistic and let the
///             enrichment loop's per-call retry policy handle it.
pub async fn client_creds_have_api_access(http: &Client, token: &str) -> bool {
    const PROBE_URL: &str =
        "https://api.spotify.com/v1/search?q=isrc%3AUSAT21704181&type=track&limit=1";
    let response = match http
        .get(PROBE_URL)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            info!("Spotify client-creds probe network error ({}); proceeding optimistically.", e);
            return true;
        }
    };
    if response.status().is_success() {
        return true;
    }
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        let body = response.text().await.unwrap_or_default();
        if body.to_lowercase().contains("premium") {
            return false;
        }
    }
    // Other 4xx/5xx: be optimistic. The real loop has 429/5xx retry logic.
    true
}

/// Hybrid token resolution. Used by both the start-enrichment route and the
/// in-loop refresh path so they share one source of truth.
///
/// Decision tree:
/// 1. Client credentials are configured AND `fetch_app_token` succeeds AND
///    the probe says the token can reach `/v1/search`: return ClientCredentials.
/// 2. Anything else (no creds, token fetch failed, probe found premium-lock):
///    return Anonymous.
///
/// The probe adds one cheap API call per priming. Cost is acceptable for
/// avoiding per-track 403 spam when the app is premium-locked.
pub async fn obtain_token(
    http: &Client,
    creds: Option<SpotifyCredentials>,
) -> Result<SpotifyTokens> {
    if let Some(creds) = creds {
        match fetch_app_token(http, &creds).await {
            Ok(tokens) => {
                if client_creds_have_api_access(http, &tokens.access_token).await {
                    return Ok(tokens);
                }
                tracing::warn!(
                    "Spotify client credentials authenticated but API access is premium-locked \
                     (owner needs an active Premium subscription). Falling back to anonymous."
                );
            }
            Err(e) => tracing::warn!(
                "Spotify client-credentials token fetch failed ({}). Falling back to anonymous.",
                e
            ),
        }
    }
    fetch_anonymous_token(http).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_mode_distinguishes_app_and_anonymous() {
        let app = SpotifyTokens {
            access_token: "x".into(),
            refresh_token: String::new(),
            expires_in: 3600,
            token_type: "Bearer".into(),
            scope: String::new(),
            user_id: "app".into(),
            fetched_at: 0,
        };
        let anon = SpotifyTokens {
            user_id: "anonymous".into(),
            ..app.clone()
        };
        assert_eq!(token_mode(&app), SpotifyTokenMode::ClientCredentials);
        assert_eq!(token_mode(&anon), SpotifyTokenMode::Anonymous);
    }

    #[test]
    fn is_expired_works_for_anonymous_shape() {
        let mut tokens = SpotifyTokens {
            access_token: "x".into(),
            refresh_token: String::new(),
            expires_in: 60,
            token_type: "Bearer".into(),
            scope: String::new(),
            user_id: "anonymous".into(),
            fetched_at: now_secs(),
        };
        // Just-fetched 60s token: is_expired's 60s safety margin makes this
        // borderline-expired immediately, which is the intended behavior — we
        // refresh proactively rather than racing the actual expiry.
        let _ = is_expired(&tokens);
        // Set a comfortable expiry far in the future so we get a clear "not expired".
        tokens.expires_in = 7200;
        assert!(!is_expired(&tokens));
        // Clearly past expiry: fetched 2h ago, expires_in 60s.
        tokens.fetched_at -= 7200;
        tokens.expires_in = 60;
        assert!(is_expired(&tokens));
    }
}
