use anyhow::{Context, Result};
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tracing::{info, warn};

const TIDAL_AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2";
const TIDAL_API_URL: &str = "https://api.tidal.com/v1";
const TIDAL_PKCE_AUTH_URL: &str = "https://login.tidal.com/authorize";
const TIDAL_PKCE_REDIRECT_URI: &str = "https://tidal.com/android/login/auth";
const TIDAL_PKCE_SCOPE: &str = "r_usr+w_usr+w_sub";
const TIDAL_BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 12; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/91.0.4472.114 Safari/537.36";
const FALLBACK_TIDAL_CLIENT_ID: &str = "fX2JxdmntZWK0ixT";
const FALLBACK_TIDAL_CLIENT_SECRET: &str = "1Nn9AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg==";
const FALLBACK_TIDAL_PKCE_CLIENT_ID_PARTS: (&[u8], &[u8]) =
    (b"TmtKRVUxSmtjRXM=", b"NWFIRkZRbFJuVlE9PQ==");
const FALLBACK_TIDAL_PKCE_CLIENT_SECRET_PARTS: (&[u8], &[u8]) = (
    b"ZUdWMVVHMVpOMjVpY0ZvNVNVbGlURUZqVVQ=",
    b"a3pjMmhyWVRGV1RtaGxWVUZ4VGpaSlkzTjZhbFJIT0QwPQ==",
);
static WARNED_FALLBACK_CREDENTIALS: AtomicBool = AtomicBool::new(false);
static PENDING_PKCE_LOGIN: LazyLock<Mutex<Option<PkceLoginState>>> =
    LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TidalCredentialSource {
    Env,
    Fallback,
}

impl TidalCredentialSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Fallback => "fallback",
        }
    }
}

/// Returns TIDAL client ID from env var `TIDAL_CLIENT_ID`, falling back to the default.
fn tidal_client_id() -> String {
    match tidal_client_credential_source() {
        TidalCredentialSource::Env => std::env::var("TIDAL_CLIENT_ID")
            .unwrap_or_else(|_| FALLBACK_TIDAL_CLIENT_ID.to_string()),
        TidalCredentialSource::Fallback => FALLBACK_TIDAL_CLIENT_ID.to_string(),
    }
}

/// Returns TIDAL client secret from env var `TIDAL_CLIENT_SECRET`, falling back to the default.
fn tidal_client_secret() -> String {
    match tidal_client_credential_source() {
        TidalCredentialSource::Env => std::env::var("TIDAL_CLIENT_SECRET")
            .unwrap_or_else(|_| FALLBACK_TIDAL_CLIENT_SECRET.to_string()),
        TidalCredentialSource::Fallback => FALLBACK_TIDAL_CLIENT_SECRET.to_string(),
    }
}

fn env_value_present(name: &str) -> bool {
    std::env::var_os(name)
        .map(|value| !value.to_string_lossy().trim().is_empty())
        .unwrap_or(false)
}

pub fn tidal_client_credential_source() -> TidalCredentialSource {
    credential_source_from_presence(
        env_value_present("TIDAL_CLIENT_ID"),
        env_value_present("TIDAL_CLIENT_SECRET"),
    )
}

pub fn tidal_pkce_client_credential_source() -> TidalCredentialSource {
    pkce_credential_source_from_presence(
        env_value_present("TIDAL_PKCE_CLIENT_ID"),
        env_value_present("TIDAL_PKCE_CLIENT_SECRET"),
    )
}

fn credential_source_from_presence(
    has_client_id: bool,
    has_client_secret: bool,
) -> TidalCredentialSource {
    if has_client_id && has_client_secret {
        TidalCredentialSource::Env
    } else {
        TidalCredentialSource::Fallback
    }
}

fn pkce_credential_source_from_presence(
    has_client_id: bool,
    has_client_secret: bool,
) -> TidalCredentialSource {
    credential_source_from_presence(has_client_id, has_client_secret)
}

pub fn warn_if_fallback_client_credentials() {
    if tidal_client_credential_source() == TidalCredentialSource::Fallback
        && !WARNED_FALLBACK_CREDENTIALS.swap(true, Ordering::Relaxed)
    {
        warn!(
            target: "noor.playback.tidal",
            event = "tidal_client_credentials_fallback",
            "Using embedded TIDAL client credentials; playback quality may be capped by TIDAL"
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub user_id: String,
    pub country_code: String,
    #[serde(default)]
    pub auth_flow: Option<String>,
}

impl TidalTokens {
    pub fn is_pkce(&self) -> bool {
        self.auth_flow.as_deref() == Some("pkce")
    }
}

pub enum PersistedTidalTokens {
    Encrypted(TidalTokens),
    LegacyPlaintext(TidalTokens),
}

impl PersistedTidalTokens {
    pub fn tokens(&self) -> &TidalTokens {
        match self {
            Self::Encrypted(tokens) | Self::LegacyPlaintext(tokens) => tokens,
        }
    }

    pub fn into_tokens(self) -> TidalTokens {
        match self {
            Self::Encrypted(tokens) | Self::LegacyPlaintext(tokens) => tokens,
        }
    }

    pub fn needs_encrypted_rewrite(&self) -> bool {
        matches!(self, Self::LegacyPlaintext(_))
    }
}

#[derive(Debug, Clone)]
pub struct PkceLogin {
    pub verify_url: String,
}

#[derive(Debug, Clone)]
struct PkceLoginState {
    code_verifier: String,
    client_unique_key: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
struct TokenUser {
    #[serde(rename = "userId")]
    user_id: i64,
    #[serde(rename = "countryCode")]
    country_code: String,
}

#[derive(Debug, Deserialize)]
struct PkceTokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: String,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct TidalSessionResponse {
    #[serde(rename = "userId")]
    user_id: i64,
    #[serde(rename = "countryCode")]
    country_code: String,
}

fn decode_joined_base64(first: &[u8], second: &[u8]) -> Result<String> {
    let first = base64::engine::general_purpose::STANDARD.decode(first)?;
    let second = base64::engine::general_purpose::STANDARD.decode(second)?;
    let joined = [first, second].concat();
    let decoded = base64::engine::general_purpose::STANDARD.decode(joined)?;
    String::from_utf8(decoded).context("decoded TIDAL credential is not UTF-8")
}

fn tidal_pkce_client_id() -> Result<String> {
    if let Ok(value) = std::env::var("TIDAL_PKCE_CLIENT_ID")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    decode_joined_base64(
        FALLBACK_TIDAL_PKCE_CLIENT_ID_PARTS.0,
        FALLBACK_TIDAL_PKCE_CLIENT_ID_PARTS.1,
    )
}

fn tidal_pkce_client_secret() -> Result<String> {
    if let Ok(value) = std::env::var("TIDAL_PKCE_CLIENT_SECRET")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    decode_joined_base64(
        FALLBACK_TIDAL_PKCE_CLIENT_SECRET_PARTS.0,
        FALLBACK_TIDAL_PKCE_CLIENT_SECRET_PARTS.1,
    )
}

fn random_url_safe(bytes_len: usize) -> String {
    let mut bytes = vec![0_u8; bytes_len];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_code_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn extract_pkce_code(redirect_url: &str) -> Result<String> {
    let url = url::Url::parse(redirect_url).context("invalid TIDAL PKCE redirect URL")?;
    let expected =
        url::Url::parse(TIDAL_PKCE_REDIRECT_URI).context("invalid TIDAL PKCE redirect template")?;
    anyhow::ensure!(
        url.scheme() == expected.scheme()
            && url.host_str() == expected.host_str()
            && url.path() == expected.path(),
        "TIDAL PKCE redirect URL must be the final TIDAL Android redirect"
    );
    url.query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
        .filter(|code| !code.trim().is_empty())
        .context("TIDAL PKCE redirect URL did not contain a code")
}

pub fn redact_tidal_auth_body(raw: &str) -> String {
    const MAX_LEN: usize = 700;
    let redacted = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(mut value) => {
            redact_tidal_auth_value(&mut value);
            value.to_string()
        }
        Err(_) => "unparseable auth response body".to_string(),
    };
    if redacted.len() > MAX_LEN {
        let mut end = MAX_LEN;
        while !redacted.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &redacted[..end])
    } else {
        redacted
    }
}

fn redact_tidal_auth_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if is_sensitive_tidal_auth_key(key) {
                    *value = serde_json::Value::String("<redacted>".to_string());
                } else {
                    redact_tidal_auth_value(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_tidal_auth_value(value);
            }
        }
        _ => {}
    }
}

fn is_sensitive_tidal_auth_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "access_token" | "refresh_token" | "id_token" | "client_secret" | "code" | "code_verifier"
    )
}

pub fn encode_persisted_tidal_tokens(
    master_key: &crate::services::crypto::MasterKey,
    tokens: &TidalTokens,
) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(tokens)?;
    master_key.encrypt(&json)
}

pub fn decode_persisted_tidal_tokens(
    master_key: &crate::services::crypto::MasterKey,
    blob: &[u8],
) -> Result<Option<PersistedTidalTokens>> {
    if blob.is_empty() {
        return Ok(None);
    }

    match master_key.decrypt(blob) {
        Ok(plain) => {
            let tokens = serde_json::from_slice::<TidalTokens>(&plain)
                .context("failed to parse encrypted TIDAL token payload")?;
            Ok(Some(PersistedTidalTokens::Encrypted(tokens)))
        }
        Err(decrypt_error) => {
            let Ok(json) = std::str::from_utf8(blob) else {
                return Err(decrypt_error).context("failed to decrypt persisted TIDAL tokens");
            };
            let tokens = serde_json::from_str::<TidalTokens>(json).with_context(|| {
                format!("failed to decrypt persisted TIDAL tokens: {decrypt_error}")
            })?;
            Ok(Some(PersistedTidalTokens::LegacyPlaintext(tokens)))
        }
    }
}

pub fn start_pkce_login() -> Result<PkceLogin> {
    let code_verifier = random_url_safe(32);
    let code_challenge = pkce_code_challenge(&code_verifier);
    let client_unique_key = format!("{:016x}", rand::random::<u64>());
    let client_id = tidal_pkce_client_id()?;
    let mut verify_url = url::Url::parse(TIDAL_PKCE_AUTH_URL)?;
    verify_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", TIDAL_PKCE_REDIRECT_URI)
        .append_pair("client_id", &client_id)
        .append_pair("lang", "EN")
        .append_pair("appMode", "android")
        .append_pair("client_unique_key", &client_unique_key)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("restrict_signup", "true");

    let mut pending = PENDING_PKCE_LOGIN
        .lock()
        .expect("TIDAL PKCE login state poisoned");
    *pending = Some(PkceLoginState {
        code_verifier,
        client_unique_key,
    });

    info!(
        target: "noor.playback.tidal",
        event = "tidal_pkce_login_started",
        "TIDAL PKCE login started"
    );

    Ok(PkceLogin {
        verify_url: verify_url.into(),
    })
}

pub async fn complete_pkce_login(
    http: &reqwest::Client,
    redirect_url: &str,
) -> Result<TidalTokens> {
    let code = extract_pkce_code(redirect_url)?;
    let state = {
        let pending = PENDING_PKCE_LOGIN
            .lock()
            .expect("TIDAL PKCE login state poisoned");
        pending
            .clone()
            .context("no pending TIDAL PKCE login is active")?
    };
    let client_id = tidal_pkce_client_id()?;
    let resp = http
        .post(format!("{}/token", TIDAL_AUTH_URL))
        .header(reqwest::header::USER_AGENT, TIDAL_BROWSER_USER_AGENT)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", TIDAL_PKCE_REDIRECT_URI),
            ("scope", TIDAL_PKCE_SCOPE),
            ("code_verifier", state.code_verifier.as_str()),
            ("client_unique_key", state.client_unique_key.as_str()),
        ])
        .send()
        .await?;

    let status = resp.status();
    let raw = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!(
            "TIDAL PKCE token exchange failed with {}: {}",
            status,
            redact_tidal_auth_body(&raw)
        );
    }

    let token: PkceTokenResponse =
        serde_json::from_str(&raw).context("failed to parse TIDAL PKCE token response")?;
    let session: TidalSessionResponse = http
        .get(format!("{}/sessions", TIDAL_API_URL))
        .header(reqwest::header::USER_AGENT, TIDAL_BROWSER_USER_AGENT)
        .bearer_auth(&token.access_token)
        .send()
        .await?
        .json()
        .await
        .context("failed to load TIDAL session after PKCE login")?;

    {
        let mut pending = PENDING_PKCE_LOGIN
            .lock()
            .expect("TIDAL PKCE login state poisoned");
        *pending = None;
    }

    info!(
        target: "noor.playback.tidal",
        event = "tidal_pkce_login_complete",
        user_id = %session.user_id,
        "TIDAL PKCE login complete"
    );

    Ok(TidalTokens {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        token_type: token.token_type,
        expires_in: token.expires_in,
        user_id: session.user_id.to_string(),
        country_code: session.country_code,
        auth_flow: Some("pkce".to_string()),
    })
}

/// Initiate device code login flow.
/// Returns (user_code, verification_url) for user to complete login.
#[allow(dead_code)]
pub async fn start_device_login(http: &reqwest::Client) -> Result<(String, String, String, i64)> {
    warn_if_fallback_client_credentials();
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
#[allow(dead_code)]
pub async fn poll_for_token(
    http: &reqwest::Client,
    device_code: &str,
    interval_secs: i64,
) -> Result<TidalTokens> {
    warn_if_fallback_client_credentials();
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
        let body: TokenPollResponse =
            serde_json::from_str(&raw).context("Failed to parse token response")?;

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
                    auth_flow: Some("device".to_string()),
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
pub async fn refresh_token(
    http: &reqwest::Client,
    refresh_token: &str,
    auth_flow: Option<&str>,
) -> Result<TidalTokens> {
    let is_pkce = auth_flow == Some("pkce");
    if !is_pkce {
        warn_if_fallback_client_credentials();
    }
    let client_id;
    let client_secret;
    if is_pkce {
        client_id = tidal_pkce_client_id()?;
        client_secret = tidal_pkce_client_secret()?;
    } else {
        client_id = tidal_client_id();
        client_secret = tidal_client_secret();
    }
    let resp = http
        .post(format!("{}/token", TIDAL_AUTH_URL))
        .header(reqwest::header::USER_AGENT, TIDAL_BROWSER_USER_AGENT)
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;

    let status = resp.status();
    let raw = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!(
            "TIDAL refresh failed with {}: {}",
            status,
            redact_tidal_auth_body(&raw)
        );
    }

    let value: serde_json::Value =
        serde_json::from_str(&raw).context("Failed to parse refresh token response")?;

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
        auth_flow: Some(if is_pkce { "pkce" } else { "device" }.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_code_challenge_uses_url_safe_sha256_without_padding() {
        assert_eq!(
            pkce_code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn pkce_redirect_code_is_extracted_from_tidal_redirect() {
        assert_eq!(
            extract_pkce_code("https://tidal.com/android/login/auth?code=abc123&state=ignored")
                .unwrap(),
            "abc123"
        );
    }

    #[test]
    fn pkce_redirect_code_rejects_unrelated_urls() {
        let err = extract_pkce_code("https://example.com/android/login/auth?code=abc123")
            .expect_err("unrelated redirect should be rejected");

        assert!(
            err.to_string()
                .contains("must be the final TIDAL Android redirect")
        );
    }

    #[test]
    fn credential_source_is_env_when_id_and_secret_are_present() {
        assert_eq!(
            credential_source_from_presence(true, true),
            TidalCredentialSource::Env
        );
    }

    #[test]
    fn credential_source_is_fallback_when_either_env_value_is_missing() {
        assert_eq!(
            credential_source_from_presence(false, true),
            TidalCredentialSource::Fallback
        );
        assert_eq!(
            credential_source_from_presence(true, false),
            TidalCredentialSource::Fallback
        );
        assert_eq!(
            credential_source_from_presence(false, false),
            TidalCredentialSource::Fallback
        );
    }

    #[test]
    fn pkce_credential_source_is_env_when_id_and_secret_are_present() {
        assert_eq!(
            pkce_credential_source_from_presence(true, true),
            TidalCredentialSource::Env
        );
    }

    #[test]
    fn pkce_credential_source_is_fallback_when_either_env_value_is_missing() {
        assert_eq!(
            pkce_credential_source_from_presence(false, true),
            TidalCredentialSource::Fallback
        );
        assert_eq!(
            pkce_credential_source_from_presence(true, false),
            TidalCredentialSource::Fallback
        );
    }

    #[test]
    fn token_response_redaction_removes_sensitive_values() {
        let raw = r#"{
            "access_token": "access-secret",
            "refresh_token": "refresh-secret",
            "id_token": "id-secret",
            "client_secret": "client-secret",
            "code": "auth-code",
            "code_verifier": "verifier-secret",
            "error": "invalid_grant"
        }"#;

        let redacted = redact_tidal_auth_body(raw);

        assert!(redacted.contains("invalid_grant"));
        for secret in [
            "access-secret",
            "refresh-secret",
            "id-secret",
            "client-secret",
            "auth-code",
            "verifier-secret",
        ] {
            assert!(!redacted.contains(secret), "{secret} leaked");
        }
    }

    #[test]
    fn token_response_redaction_truncates_unicode_without_panic() {
        let raw = serde_json::json!({
            "error": "x".repeat(699) + "é" + "tail",
            "access_token": "access-secret"
        })
        .to_string();

        let redacted = redact_tidal_auth_body(&raw);

        assert!(redacted.ends_with("..."));
        assert!(!redacted.contains("access-secret"));
    }

    #[test]
    fn persisted_tidal_tokens_round_trip_encrypted() {
        let dir = tempdir();
        let key = crate::services::crypto::MasterKey::load_or_generate(&dir).unwrap();
        let tokens = test_tokens(Some("pkce"));

        let blob = encode_persisted_tidal_tokens(&key, &tokens).unwrap();
        assert!(!String::from_utf8_lossy(&blob).contains("access-secret"));
        let decoded = decode_persisted_tidal_tokens(&key, &blob)
            .unwrap()
            .expect("decoded tokens");

        assert_eq!(decoded.tokens().access_token, "access-secret");
        assert!(!decoded.needs_encrypted_rewrite());
    }

    #[test]
    fn persisted_tidal_tokens_load_legacy_plaintext_for_rewrite() {
        let dir = tempdir();
        let key = crate::services::crypto::MasterKey::load_or_generate(&dir).unwrap();
        let tokens = test_tokens(None);
        let legacy_blob = serde_json::to_vec(&tokens).unwrap();

        let decoded = decode_persisted_tidal_tokens(&key, &legacy_blob)
            .unwrap()
            .expect("decoded tokens");

        assert_eq!(decoded.tokens().refresh_token, "refresh-secret");
        assert!(decoded.needs_encrypted_rewrite());
    }

    fn test_tokens(auth_flow: Option<&str>) -> TidalTokens {
        TidalTokens {
            access_token: "access-secret".to_string(),
            refresh_token: "refresh-secret".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 86_400,
            user_id: "u-1".to_string(),
            country_code: "AU".to_string(),
            auth_flow: auth_flow.map(str::to_string),
        }
    }

    fn tempdir() -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("noor-tidal-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
