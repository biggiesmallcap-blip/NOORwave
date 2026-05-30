use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::services::crypto::MasterKey;

const API_BASE: &str = "https://api.listenbrainz.org";

fn token_header(token: &str) -> String {
    format!("Token {token}")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListenBrainzCredentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(default)]
    pub scrobbling_enabled: bool,
    #[serde(default)]
    pub recommendations_enabled: bool,
}

pub fn load_credentials(conn: &Connection) -> Result<Option<ListenBrainzCredentials>> {
    let row: Option<String> = conn
        .query_row(
            "SELECT extra_data FROM service_auth WHERE service = 'listenbrainz'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(json) = row else { return Ok(None) };
    Ok(serde_json::from_str(&json).ok())
}

pub fn save_credentials(
    conn: &Connection,
    master: &MasterKey,
    token: &str,
    user_name: &str,
) -> Result<()> {
    let creds = ListenBrainzCredentials {
        user_name: Some(user_name.to_string()),
        scrobbling_enabled: true,
        recommendations_enabled: true,
    };
    let json = serde_json::to_string(&creds)?;
    let blob = master.encrypt(token.as_bytes())?;
    conn.execute(
        "INSERT INTO service_auth (service, access_token_enc, extra_data, user_id, connected_at)
         VALUES ('listenbrainz', ?1, ?2, ?3, datetime('now'))
         ON CONFLICT(service) DO UPDATE SET
             access_token_enc = excluded.access_token_enc,
             extra_data = excluded.extra_data,
             user_id = excluded.user_id",
        params![blob, json, user_name],
    )?;
    Ok(())
}

pub fn clear_credentials(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM service_auth WHERE service = 'listenbrainz'",
        [],
    )?;
    Ok(())
}

pub fn load_token(conn: &Connection, master: &MasterKey) -> Result<Option<String>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT access_token_enc FROM service_auth WHERE service = 'listenbrainz'",
            [],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten();
    let Some(blob) = blob else { return Ok(None) };
    if blob.is_empty() {
        return Ok(None);
    }
    let plain = master.decrypt(&blob)?;
    Ok(Some(String::from_utf8(plain)?))
}

pub fn has_token(conn: &Connection) -> Result<bool> {
    let present: Option<i64> = conn
        .query_row(
            "SELECT CASE WHEN access_token_enc IS NOT NULL AND length(access_token_enc) > 0 THEN 1 ELSE 0 END
               FROM service_auth
              WHERE service = 'listenbrainz'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(present.unwrap_or(0) == 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenBrainzValidation {
    pub valid: bool,
    pub user_name: Option<String>,
}

pub async fn validate_token(http: &reqwest::Client, token: &str) -> Result<ListenBrainzValidation> {
    let body: Value = http
        .get(format!("{API_BASE}/1/validate-token"))
        .query(&[("token", token)])
        .header(reqwest::header::AUTHORIZATION, token_header(token))
        .send()
        .await
        .context("ListenBrainz token validation failed")?
        .error_for_status()
        .context("ListenBrainz token validation returned error status")?
        .json()
        .await
        .context("ListenBrainz token validation response was not JSON")?;
    Ok(parse_validation(&body))
}

pub fn parse_validation(body: &Value) -> ListenBrainzValidation {
    let valid = body
        .get("valid")
        .and_then(Value::as_bool)
        .or_else(|| body.get("is_valid").and_then(Value::as_bool))
        .unwrap_or(false);
    let user_name = body
        .get("user_name")
        .or_else(|| body.get("user"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    ListenBrainzValidation { valid, user_name }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenType {
    PlayingNow,
    Single,
    Import,
}

impl ListenType {
    fn as_str(&self) -> &'static str {
        match self {
            ListenType::PlayingNow => "playing_now",
            ListenType::Single => "single",
            ListenType::Import => "import",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListenPayload {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub listened_at: Option<i64>,
}

pub fn build_submit_payload(kind: ListenType, listen: &ListenPayload) -> Value {
    let mut additional = serde_json::Map::new();
    additional.insert("submission_client".to_string(), json!("NOORwave"));
    additional.insert(
        "submission_client_version".to_string(),
        json!(env!("CARGO_PKG_VERSION")),
    );
    if let Some(duration_ms) = listen.duration_ms {
        additional.insert("duration_ms".to_string(), json!(duration_ms));
    }

    let mut metadata = serde_json::Map::new();
    metadata.insert("artist_name".to_string(), json!(listen.artist));
    metadata.insert("track_name".to_string(), json!(listen.title));
    if let Some(album) = listen.album.as_deref().filter(|s| !s.trim().is_empty()) {
        metadata.insert("release_name".to_string(), json!(album));
    }
    metadata.insert("additional_info".to_string(), Value::Object(additional));

    let mut item = serde_json::Map::new();
    if !matches!(kind, ListenType::PlayingNow) {
        if let Some(ts) = listen.listened_at {
            item.insert("listened_at".to_string(), json!(ts));
        }
    }
    item.insert("track_metadata".to_string(), Value::Object(metadata));

    json!({
        "listen_type": kind.as_str(),
        "payload": [Value::Object(item)]
    })
}

pub async fn submit_listen(
    http: &reqwest::Client,
    token: &str,
    kind: ListenType,
    listen: &ListenPayload,
) -> Result<()> {
    let body = build_submit_payload(kind, listen);
    let resp = http
        .post(format!("{API_BASE}/1/submit-listens"))
        .header(reqwest::header::AUTHORIZATION, token_header(token))
        .json(&body)
        .send()
        .await
        .context("ListenBrainz submit-listens failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("ListenBrainz submit-listens HTTP {status}: {text}"));
    }
    Ok(())
}

pub async fn love_recording(
    http: &reqwest::Client,
    token: &str,
    recording_mbid: &str,
) -> Result<()> {
    let resp = http
        .post(format!("{API_BASE}/1/feedback/recording-feedback"))
        .header(reqwest::header::AUTHORIZATION, token_header(token))
        .json(&json!({
            "recording_mbid": recording_mbid,
            "score": 1
        }))
        .send()
        .await
        .context("ListenBrainz feedback failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("ListenBrainz feedback HTTP {status}: {text}"));
    }
    Ok(())
}

pub async fn user_recommendations(
    http: &reqwest::Client,
    user_name: &str,
    token: Option<&str>,
) -> Result<Vec<ListenBrainzRecommendation>> {
    let mut req = http.get(format!(
        "{API_BASE}/1/cf/recommendation/user/{}/recording",
        urlencoding::encode(user_name)
    ));
    if let Some(token) = token {
        req = req.header(reqwest::header::AUTHORIZATION, token_header(token));
    }
    let resp = req
        .send()
        .await
        .context("ListenBrainz recommendations failed")?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Vec::new());
    }
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .context("ListenBrainz recommendations response was not JSON")?;
    if !status.is_success() {
        return Err(anyhow!(
            "ListenBrainz recommendations HTTP {status}: {body}"
        ));
    }
    Ok(parse_recommendations(&body))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListenBrainzRecommendation {
    pub artist: String,
    pub title: String,
    pub mbid: Option<String>,
    pub score: Option<f64>,
}

pub fn parse_recommendations(body: &Value) -> Vec<ListenBrainzRecommendation> {
    let arr = body
        .get("payload")
        .and_then(|v| v.get("mbids"))
        .and_then(Value::as_array)
        .or_else(|| {
            body.get("payload")
                .and_then(|v| v.get("recordings"))
                .and_then(Value::as_array)
        })
        .or_else(|| body.get("recordings").and_then(Value::as_array));
    let Some(arr) = arr else { return Vec::new() };

    arr.iter()
        .filter_map(|entry| {
            let title = entry
                .get("recording_name")
                .or_else(|| entry.get("title"))
                .or_else(|| entry.get("track_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())?
                .to_string();
            let artist = entry
                .get("artist_name")
                .or_else(|| entry.get("artist_credit_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())?
                .to_string();
            let mbid = entry
                .get("recording_mbid")
                .or_else(|| entry.get("recording_msid"))
                .or_else(|| entry.get("mbid"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let score = entry
                .get("score")
                .or_else(|| entry.get("rating"))
                .and_then(Value::as_f64);
            Some(ListenBrainzRecommendation {
                artist,
                title,
                mbid,
                score,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_validation_response() {
        let parsed = parse_validation(&json!({ "valid": true, "user_name": "alice" }));
        assert_eq!(
            parsed,
            ListenBrainzValidation {
                valid: true,
                user_name: Some("alice".to_string())
            }
        );
    }

    #[test]
    fn listenbrainz_token_header_uses_token_scheme() {
        assert_eq!(token_header("abc123"), "Token abc123");
    }

    #[test]
    fn playing_now_payload_omits_timestamp() {
        let payload = build_submit_payload(
            ListenType::PlayingNow,
            &ListenPayload {
                artist: "A".to_string(),
                title: "T".to_string(),
                album: Some("R".to_string()),
                duration_ms: Some(123_000),
                listened_at: Some(123),
            },
        );
        assert_eq!(payload["listen_type"], "playing_now");
        assert!(payload["payload"][0].get("listened_at").is_none());
    }

    #[test]
    fn single_payload_includes_timestamp() {
        let payload = build_submit_payload(
            ListenType::Single,
            &ListenPayload {
                artist: "A".to_string(),
                title: "T".to_string(),
                album: None,
                duration_ms: None,
                listened_at: Some(456),
            },
        );
        assert_eq!(payload["listen_type"], "single");
        assert_eq!(payload["payload"][0]["listened_at"], 456);
    }
}
