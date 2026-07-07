//! Anonymous Spotify access-token mint.
//!
//! Spotify's `open.spotify.com/api/token` endpoint accepts an anonymous
//! request signed with a TOTP code derived from a shared secret + the
//! current 30-second window. We refresh the token ~60 seconds before
//! `accessTokenExpirationTimestampMs`.

use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{
    Client, StatusCode,
    header::{DATE, HeaderMap},
};
use serde::Deserialize;
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

use super::hashes::{TOTP_SECRET, TOTP_VER};

type HmacSha1 = Hmac<Sha1>;

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "accessTokenExpirationTimestampMs")]
    pub expires_at_ms: i64,
    #[serde(rename = "isAnonymous", default)]
    #[allow(dead_code)]
    pub is_anonymous: bool,
}

/// Generate a 6-digit RFC-6238 TOTP for a given Unix timestamp (ms).
pub fn totp_code(secret: &[u8], unix_ms: u64) -> u32 {
    let counter: u64 = unix_ms / 30_000;
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();

    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let truncated = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);

    truncated % 1_000_000
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct TokenHttpError {
    status: StatusCode,
    body: String,
    response_server_ms: Option<u64>,
}

fn date_header_unix_ms(headers: &HeaderMap) -> Option<u64> {
    let raw = headers.get(DATE)?.to_str().ok()?;
    let parsed = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    u64::try_from(parsed.timestamp_millis()).ok()
}

async fn request_token_at(
    client: &Client,
    server_ms: u64,
) -> Result<std::result::Result<TokenResponse, TokenHttpError>> {
    let code = totp_code(TOTP_SECRET, server_ms);
    // TOTP_VER >= 10 means sTime/cTime/buildDate/buildVer are no longer
    // required; older client versions sent them too.
    let url = format!(
        "https://open.spotify.com/api/token?reason=transport&productType=web-player&totp={:06}&totpServer={:06}&totpVer={}",
        code, code, TOTP_VER,
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .header("Referer", "https://open.spotify.com/")
        .send()
        .await
        .context("GET /api/token")?;

    let status = resp.status();
    if !status.is_success() {
        let response_server_ms = date_header_unix_ms(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        return Ok(Err(TokenHttpError {
            status,
            body,
            response_server_ms,
        }));
    }

    let parsed: TokenResponse = resp.json().await.context("/api/token body was not JSON")?;
    Ok(Ok(parsed))
}

fn retry_server_time_for_status(
    status: StatusCode,
    response_server_ms: Option<u64>,
) -> Option<u64> {
    if status == StatusCode::UNAUTHORIZED {
        response_server_ms
    } else {
        None
    }
}

pub async fn mint(client: &Client) -> Result<TokenResponse> {
    let local_ms = now_ms();
    match request_token_at(client, local_ms).await? {
        Ok(parsed) => Ok(parsed),
        Err(err) => {
            if let Some(server_ms) =
                retry_server_time_for_status(err.status, err.response_server_ms)
            {
                debug!("spotify_public: token mint 401, retrying with response Date header");
                return match request_token_at(client, server_ms).await? {
                    Ok(parsed) => Ok(parsed),
                    Err(retry_err) => {
                        anyhow::bail!("{}", token_status_error(retry_err.status, &retry_err.body))
                    }
                };
            }
            anyhow::bail!("{}", token_status_error(err.status, &err.body));
        }
    }
}

fn token_status_error(status: StatusCode, body: &str) -> String {
    let snippet: String = body.chars().take(300).collect();
    if snippet.is_empty() {
        format!("/api/token returned HTTP {status}")
    } else {
        format!("/api/token returned HTTP {status}: {snippet}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC-6238 SHA-1 reference vector (ASCII "12345678901234567890"
    /// secret, T = 59s, expected TOTP = "94287082"). The lower 6 digits
    /// (287082) match our return; the full HOTP value is 1094287082, and
    /// we take mod 1e6 = 94287082 -> as u32 = 94287082.
    #[test]
    fn totp_matches_rfc6238_sha1_vector() {
        let secret = b"12345678901234567890";
        let t_seconds = 59u64;
        let code = totp_code(secret, t_seconds * 1000);
        assert_eq!(code, 287082, "RFC-6238 T=59 SHA1 6-digit");
    }

    /// Round-trip vector for the captured v61 Spotify secret. Cross-checked
    /// against `pyotp.TOTP(base32_encode(TOTP_SECRET)).at(1700000000)` at
    /// the same fixed unix timestamp.
    #[test]
    fn totp_matches_spotify_v61_vector() {
        assert_eq!(
            TOTP_VER, 61,
            "Spotify public token mint must use current cipher"
        );
        let code = totp_code(TOTP_SECRET, 1_700_000_000_000);
        assert_eq!(code, 371599, "v61 secret @ t=1700000000s");
    }

    #[test]
    fn token_status_error_includes_body_snippet() {
        let err = token_status_error(
            StatusCode::BAD_REQUEST,
            "{\"error\":{\"message\":\"Unauthorized request\"}}",
        );
        assert!(err.contains("HTTP 400 Bad Request"));
        assert!(err.contains("Unauthorized request"));
    }

    #[test]
    fn date_header_unix_ms_parses_http_date() {
        let mut headers = HeaderMap::new();
        headers.insert(DATE, "Thu, 04 Jun 2026 06:50:00 GMT".parse().unwrap());

        assert_eq!(date_header_unix_ms(&headers), Some(1_780_555_800_000));
    }

    #[test]
    fn date_header_unix_ms_rejects_missing_or_invalid_header() {
        assert_eq!(date_header_unix_ms(&HeaderMap::new()), None);

        let mut headers = HeaderMap::new();
        headers.insert(DATE, "not a date".parse().unwrap());
        assert_eq!(date_header_unix_ms(&headers), None);
    }

    #[test]
    fn retry_server_time_is_only_used_after_unauthorized() {
        assert_eq!(
            retry_server_time_for_status(StatusCode::UNAUTHORIZED, Some(123_000)),
            Some(123_000)
        );
        assert_eq!(
            retry_server_time_for_status(StatusCode::BAD_REQUEST, Some(123_000)),
            None
        );
        assert_eq!(
            retry_server_time_for_status(StatusCode::UNAUTHORIZED, None),
            None
        );
    }
}
