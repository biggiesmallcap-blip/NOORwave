use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// Spotify Client ID — You must register a free app at https://developer.spotify.com/dashboard
// and replace this with your own Client ID.
const SPOTIFY_CLIENT_ID: &str = "YOUR_SPOTIFY_CLIENT_ID";
const SPOTIFY_DEVICE_CODE_URL: &str = "https://accounts.spotify.com/api/device/code";
const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_SCOPES: &str = "user-read-private";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
    pub scope: String,
    pub user_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: i64,
    pub interval: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub token_type: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MeResponse {
    pub id: String,
}

/// Start the Spotify Device Code flow.
/// Returns the `DeviceCodeResponse` so the frontend can show the URL and User Code.
pub async fn start_device_code(http: &Client) -> Result<DeviceCodeResponse> {
    if SPOTIFY_CLIENT_ID == "YOUR_SPOTIFY_CLIENT_ID" {
        anyhow::bail!("Spotify Client ID not configured. Please update SPOTIFY_CLIENT_ID in auth.rs");
    }

    let params = [
        ("client_id", SPOTIFY_CLIENT_ID),
        ("scope", SPOTIFY_SCOPES),
    ];

    let response = http
        .post(SPOTIFY_DEVICE_CODE_URL)
        .form(&params)
        .send()
        .await
        .context("Failed to request Spotify device code")?;

    if !response.status().is_success() {
        let err_text = response.text().await?;
        anyhow::bail!("Spotify device code request failed: {}", err_text);
    }

    let data: DeviceCodeResponse = response
        .json()
        .await
        .context("Failed to parse Spotify device code response")?;

    info!("Spotify device code started. User code: {}", data.user_code);
    Ok(data)
}

/// Poll Spotify for the token after user has authorized the device code.
pub async fn poll_token(
    http: &Client,
    device_code: &str,
) -> Result<Option<SpotifyTokens>> {
    let params = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", device_code),
        ("client_id", SPOTIFY_CLIENT_ID),
    ];

    let response = http
        .post(SPOTIFY_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .context("Failed to poll Spotify token")?;

    let status = response.status();

    // 200 OK -> Success
    if status == reqwest::StatusCode::OK {
        let token_data: TokenResponse = response
            .json()
            .await
            .context("Failed to parse Spotify token response")?;

        // Fetch user profile to get user_id
        let user_id = fetch_user_id(http, &token_data.access_token).await?;

        return Ok(Some(SpotifyTokens {
            access_token: token_data.access_token,
            refresh_token: token_data.refresh_token.unwrap_or_default(),
            expires_in: token_data.expires_in,
            token_type: token_data.token_type.unwrap_or_default(),
            scope: token_data.scope.unwrap_or_default(),
            user_id,
        }));
    }

    // 400 Bad Request -> Check error type
    if status == reqwest::StatusCode::BAD_REQUEST {
        let error_body: serde_json::Value = response.json().await.unwrap_or_default();
        let error = error_body["error"].as_str().unwrap_or("unknown");

        match error {
            "authorization_pending" => Ok(None),
            "slow_down" => {
                warn!("Spotify told us to slow down polling.");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                return Ok(None);
            }
            "expired_token" => anyhow::bail!("Spotify device code expired"),
            _ => anyhow::bail!("Spotify poll error: {}", error),
        }
    }

    anyhow::bail!("Unexpected Spotify poll status: {}", status)
}

async fn fetch_user_id(http: &Client, token: &str) -> Result<String> {
    let response = http
        .get("https://api.spotify.com/v1/me")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .context("Failed to fetch Spotify user profile")?;

    if response.status().is_success() {
        let profile: MeResponse = response.json().await?;
        Ok(profile.id)
    } else {
        Ok("unknown".to_string()) // Fallback if profile fetch fails
    }
}
