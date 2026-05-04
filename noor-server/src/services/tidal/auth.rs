use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

const TIDAL_AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2";

/// Returns TIDAL client ID from env var `TIDAL_CLIENT_ID`, falling back to the default.
fn tidal_client_id() -> String {
    std::env::var("TIDAL_CLIENT_ID").unwrap_or_else(|_| "fX2JxdmntZWK0ixT".to_string())
}

/// Returns TIDAL client secret from env var `TIDAL_CLIENT_SECRET`, falling back to the default.
fn tidal_client_secret() -> String {
    std::env::var("TIDAL_CLIENT_SECRET")
        .unwrap_or_else(|_| "1Nn9AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg==".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub user_id: String,
    pub country_code: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    #[serde(rename = "deviceCode")]
    device_code: String,
    #[serde(rename = "userCode")]
    user_code: String,
    #[serde(rename = "verificationUri")]
    verification_uri: String,
    #[serde(rename = "verificationUriComplete")]
    verification_uri_complete: Option<String>,
    #[serde(rename = "expiresIn")]
    #[allow(dead_code)]
    expires_in: i64,
    interval: i64,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TokenPollResponse {
    Success {
        access_token: String,
        refresh_token: String,
        token_type: String,
        expires_in: i64,
        user: TokenUser,
    },
    Error {
        error: String,
    },
}

#[derive(Debug, Deserialize)]
struct TokenUser {
    #[serde(rename = "userId")]
    user_id: i64,
    #[serde(rename = "countryCode")]
    country_code: String,
}

/// Initiate device code login flow.
/// Returns (user_code, verification_url) for user to complete login.
pub async fn start_device_login(http: &reqwest::Client) -> Result<(String, String, String, i64)> {
    let resp: DeviceCodeResponse = http
        .post(format!("{}/device_authorization", TIDAL_AUTH_URL))
        .form(&[
            ("client_id", tidal_client_id().as_str()),
            ("scope", "r_usr w_usr w_sub"),
        ])
        .send()
        .await?
        .json()
        .await
        .context("Failed to start device login")?;

    info!(
        "Device login initiated. User code: {}, URL: {}",
        resp.user_code, resp.verification_uri
    );

    let verify_url = resp
        .verification_uri_complete
        .unwrap_or(resp.verification_uri);
    // TIDAL sometimes returns URLs without scheme (e.g. "link.tidal.com/XXXXX")
    let verify_url = if !verify_url.starts_with("http") {
        format!("https://{}", verify_url)
    } else {
        verify_url
    };

    Ok((resp.device_code, resp.user_code, verify_url, resp.interval))
}

/// Poll for token after user completes login in browser.
pub async fn poll_for_token(
    http: &reqwest::Client,
    device_code: &str,
    interval_secs: i64,
) -> Result<TidalTokens> {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs as u64)).await;

        let resp = http
            .post(format!("{}/token", TIDAL_AUTH_URL))
            .form(&[
                ("client_id", tidal_client_id().as_str()),
                ("client_secret", tidal_client_secret().as_str()),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("scope", "r_usr w_usr w_sub"),
            ])
            .send()
            .await?;

        let raw = resp.text().await?;
        tracing::debug!("TIDAL token poll response: {}", raw);
        let body: TokenPollResponse = serde_json::from_str(&raw)
            .context(format!("Failed to parse token response: {}", raw))?;

        match body {
            TokenPollResponse::Success {
                access_token,
                refresh_token,
                token_type,
                expires_in,
                user,
            } => {
                info!("TIDAL login successful! User ID: {}", user.user_id);
                return Ok(TidalTokens {
                    access_token,
                    refresh_token,
                    token_type,
                    expires_in,
                    user_id: user.user_id.to_string(),
                    country_code: user.country_code,
                });
            }
            TokenPollResponse::Error { ref error } if error == "authorization_pending" => {
                continue;
            }
            TokenPollResponse::Error { error } => {
                anyhow::bail!("TIDAL auth error: {}", error);
            }
        }
    }
}

/// Refresh an expired access token.
pub async fn refresh_token(http: &reqwest::Client, refresh_token: &str) -> Result<TidalTokens> {
    let resp = http
        .post(format!("{}/token", TIDAL_AUTH_URL))
        .form(&[
            ("client_id", tidal_client_id().as_str()),
            ("client_secret", tidal_client_secret().as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;

    let status = resp.status();
    let raw = resp.text().await?;
    tracing::debug!("TIDAL refresh response: {}", raw);
    if !status.is_success() {
        anyhow::bail!("TIDAL refresh failed with {}: {}", status, raw);
    }

    let value: serde_json::Value = serde_json::from_str(&raw)
        .context(format!("Failed to parse refresh token response: {}", raw))?;

    Ok(TidalTokens {
        access_token: value["access_token"]
            .as_str()
            .context("missing access_token")?
            .to_string(),
        refresh_token: value
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or(refresh_token)
            .to_string(),
        token_type: value["token_type"].as_str().unwrap_or("Bearer").to_string(),
        expires_in: value["expires_in"].as_i64().unwrap_or(86400),
        user_id: value
            .get("user")
            .and_then(|u| u.get("userId"))
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_default(),
        country_code: value
            .get("user")
            .and_then(|u| u.get("countryCode"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}
