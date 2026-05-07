use anyhow::{Context, Result};
use reqwest::Client;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

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
