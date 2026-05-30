//! Last.fm scrobbling client.
//!
//! Implements the standard write-API flow:
//! - `auth.getToken` / `auth.getSession` — handled by the routes layer, which
//!   calls into [`api_call`] with the right params.
//! - `track.updateNowPlaying` — fired on play start.
//! - `track.scrobble` — fired on play end if the listen meets Last.fm's
//!   eligibility rule (`>= min(50% of duration, 240s)`, with track `>= 30s`).
//!
//! Signing follows the Last.fm spec exactly:
//! 1. Sort all params alphabetically by key.
//! 2. Concat `k1 + v1 + k2 + v2 + ...`.
//! 3. Append the shared `api_secret`.
//! 4. MD5-hex-lowercase the resulting bytes.
//! 5. The resulting string goes into the `api_sig` field of the request.
//!
//! The `format` and `api_sig` keys are themselves never included in the
//! signed string. We use `format=json` everywhere.

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeMap;

use crate::SharedState;

/// Spawn a fire-and-forget scrobble for a TIDAL play.
///
/// Eligibility (per Last.fm spec): track >= 30s AND listened >= min(50%, 240s).
/// Caller passes `listened_ms` and the helper does the eligibility check.
///
/// Silent no-op when:
/// - LASTFM_API_SECRET env is missing
/// - source != "tidal" (we only scrobble streamed plays — local plays already
///   live in NOORwave's listen_history)
/// - no Last.fm session_key has been stored
/// - eligibility threshold not met
///
/// HTTP failures are logged at warn and never propagated.
#[allow(dead_code)]
pub fn spawn_scrobble_completed(
    state: SharedState,
    artist: String,
    track: String,
    album: Option<String>,
    duration_ms: i64,
    listened_ms: i64,
    started_at_unix: i64,
    source: &str,
) {
    if source != "tidal" {
        return;
    }
    if !is_eligible_for_scrobble(duration_ms, listened_ms) {
        return;
    }
    spawn_lastfm_call(
        state,
        ScrobbleKind::Completed { started_at_unix },
        artist,
        track,
        album,
        Some(duration_ms),
    );
}

/// Spawn a fire-and-forget `track.updateNowPlaying` for a TIDAL play.
/// Same silent-no-op gating as [`spawn_scrobble_completed`].
#[allow(dead_code)]
pub fn spawn_now_playing(
    state: SharedState,
    artist: String,
    track: String,
    album: Option<String>,
    duration_ms: Option<i64>,
    source: &str,
) {
    if source != "tidal" {
        return;
    }
    spawn_lastfm_call(
        state,
        ScrobbleKind::NowPlaying,
        artist,
        track,
        album,
        duration_ms,
    );
}

#[allow(dead_code)]
enum ScrobbleKind {
    NowPlaying,
    Completed { started_at_unix: i64 },
}

#[allow(dead_code)]
fn spawn_lastfm_call(
    state: SharedState,
    kind: ScrobbleKind,
    artist: String,
    track: String,
    album: Option<String>,
    duration_ms: Option<i64>,
) {
    tokio::spawn(async move {
        // All the credential/secret loads happen INSIDE the spawned task so
        // the calling sync code is never blocked on the AppState lock.
        let (http, api_secret, api_key, session_key) = {
            let s = state.read().await;
            let creds =
                s.db.with_conn(|conn| {
                    Ok(crate::services::lastfm::auth::load_credentials(conn)
                        .ok()
                        .flatten())
                })
                .ok()
                .flatten();
            let session_key =
                s.db.with_conn(|conn| {
                    Ok(
                        crate::services::lastfm::auth::load_session_key(conn, &s.master_key)
                            .ok()
                            .flatten(),
                    )
                })
                .ok()
                .flatten();
            (
                s.http_client.clone(),
                s.lastfm_api_secret.clone(),
                creds.map(|c| c.api_key),
                session_key,
            )
        };
        let Some(api_secret) = api_secret else { return };
        let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
            return;
        };
        let Some(session_key) = session_key else {
            return;
        };

        let result = match kind {
            ScrobbleKind::NowPlaying => {
                update_now_playing(
                    &http,
                    &api_key,
                    &api_secret,
                    &session_key,
                    &artist,
                    &track,
                    album.as_deref(),
                    duration_ms,
                )
                .await
            }
            ScrobbleKind::Completed { started_at_unix } => {
                scrobble_track(
                    &http,
                    &api_key,
                    &api_secret,
                    &session_key,
                    &artist,
                    &track,
                    album.as_deref(),
                    started_at_unix,
                )
                .await
            }
        };
        if let Err(e) = result {
            tracing::warn!("Last.fm scrobble call failed: {e}");
        }
    });
}

const ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";

/// Build the `api_sig` value for a Last.fm signed request.
pub fn sign(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut buf = String::new();
    for (k, v) in params {
        // Per spec, format and api_sig themselves are excluded from signing.
        if k == "format" || k == "api_sig" {
            continue;
        }
        buf.push_str(k);
        buf.push_str(v);
    }
    buf.push_str(secret);
    format!("{:x}", md5::compute(buf.as_bytes()))
}

/// Eligibility check for `track.scrobble`. Last.fm's official rule.
pub fn is_eligible_for_scrobble(duration_ms: i64, listened_ms: i64) -> bool {
    if duration_ms < 30_000 {
        return false;
    }
    let half = duration_ms / 2;
    let four_min = 240_000;
    let threshold = half.min(four_min);
    listened_ms >= threshold
}

/// POST a signed Last.fm write-API call. Returns the parsed JSON body. Errors
/// on transport failure, non-2xx, or a Last.fm error envelope (`{"error": N,
/// "message": "..."}`).
async fn api_call(
    http: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
    method: &str,
    extra: Vec<(&'static str, String)>,
    session_key: Option<&str>,
) -> Result<serde_json::Value> {
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("method".into(), method.into());
    params.insert("api_key".into(), api_key.into());
    if let Some(sk) = session_key {
        params.insert("sk".into(), sk.into());
    }
    for (k, v) in extra {
        params.insert(k.into(), v);
    }
    let api_sig = sign(&params, api_secret);
    params.insert("api_sig".into(), api_sig);
    params.insert("format".into(), "json".into());

    let resp = http
        .post(ENDPOINT)
        .form(&params)
        .send()
        .await
        .context("Last.fm write-API request failed")?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .context("Last.fm write-API response not JSON")?;
    if !status.is_success() {
        return Err(anyhow!(
            "Last.fm write-API HTTP {}: {}",
            status,
            body.get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(no message)")
        ));
    }
    if let Some(err) = body.get("error").and_then(serde_json::Value::as_i64) {
        return Err(anyhow!(
            "Last.fm error {}: {}",
            err,
            body.get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(no message)")
        ));
    }
    Ok(body)
}

/// Step 1 of web auth: get a request token. The user opens
/// `https://www.last.fm/api/auth/?api_key=...&token=...` in their browser to
/// authorize, then the route layer calls [`get_session`] to redeem it.
pub async fn get_token(http: &reqwest::Client, api_key: &str, api_secret: &str) -> Result<String> {
    let body = api_call(http, api_key, api_secret, "auth.getToken", Vec::new(), None).await?;
    body.get("token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("auth.getToken returned no token"))
}

#[derive(Debug, Clone)]
pub struct AuthSession {
    pub session_key: String,
    pub user_name: String,
}

/// Step 2 of web auth: redeem the request token for a long-lived session key.
/// Errors if the user has not yet authorized in their browser (Last.fm
/// returns error 14 — "This token has not been authorized").
pub async fn get_session(
    http: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
    token: &str,
) -> Result<AuthSession> {
    let body = api_call(
        http,
        api_key,
        api_secret,
        "auth.getSession",
        vec![("token", token.to_string())],
        None,
    )
    .await?;
    let session = body
        .get("session")
        .ok_or_else(|| anyhow!("auth.getSession returned no session block"))?;
    let session_key = session
        .get("key")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("auth.getSession returned no session.key"))?;
    let user_name = session
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("auth.getSession returned no session.name"))?;
    Ok(AuthSession {
        session_key,
        user_name,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn update_now_playing(
    http: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
    session_key: &str,
    artist: &str,
    track: &str,
    album: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
    let mut extra: Vec<(&'static str, String)> =
        vec![("artist", artist.to_string()), ("track", track.to_string())];
    if let Some(a) = album {
        extra.push(("album", a.to_string()));
    }
    if let Some(d) = duration_ms {
        extra.push(("duration", (d / 1000).to_string()));
    }
    api_call(
        http,
        api_key,
        api_secret,
        "track.updateNowPlaying",
        extra,
        Some(session_key),
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn scrobble_track(
    http: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
    session_key: &str,
    artist: &str,
    track: &str,
    album: Option<&str>,
    timestamp_unix: i64,
) -> Result<()> {
    let mut extra: Vec<(&'static str, String)> = vec![
        ("artist", artist.to_string()),
        ("track", track.to_string()),
        ("timestamp", timestamp_unix.to_string()),
    ];
    if let Some(a) = album {
        extra.push(("album", a.to_string()));
    }
    api_call(
        http,
        api_key,
        api_secret,
        "track.scrobble",
        extra,
        Some(session_key),
    )
    .await?;
    Ok(())
}

pub async fn love_track(
    http: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
    session_key: &str,
    artist: &str,
    track: &str,
) -> Result<()> {
    api_call(
        http,
        api_key,
        api_secret,
        "track.love",
        vec![("artist", artist.to_string()), ("track", track.to_string())],
        Some(session_key),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Algorithmic test — documents the spec by computing the expected value
    /// from the same primitive (md5::compute) on the spec-prescribed string.
    /// Catches bugs like wrong key order, missing secret append, or a typo
    /// in the concat format.
    #[test]
    fn sign_follows_spec() {
        let mut params = BTreeMap::new();
        params.insert("api_key".to_string(), "xxxxx".to_string());
        params.insert("method".to_string(), "auth.getToken".to_string());

        // Per spec: alphabetical k+v concat, then secret, then md5 hex.
        // "api_key" < "method" alphabetically.
        let spec_string = "api_keyxxxxxmethodauth.getTokenyyyyy";
        let expected = format!("{:x}", md5::compute(spec_string.as_bytes()));

        assert_eq!(sign(&params, "yyyyy"), expected);
    }

    /// Pinned value — if anyone refactors `sign` and accidentally breaks the
    /// algorithm, this test fails with a stable, reproducible diff.
    /// The pin is computed from the spec string `"a1b2s"`.
    #[test]
    fn sign_pinned_value() {
        let mut params = BTreeMap::new();
        params.insert("a".to_string(), "1".to_string());
        params.insert("b".to_string(), "2".to_string());
        // md5("a1b2s") — verified value of the spec string.
        let expected = format!("{:x}", md5::compute(b"a1b2s"));
        assert_eq!(sign(&params, "s"), expected);
        // Sanity: lowercase hex, exactly 32 chars.
        let s = sign(&params, "s");
        assert_eq!(s.len(), 32);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    /// `format` and `api_sig` must be excluded from the signed string per spec.
    #[test]
    fn sign_excludes_format_and_api_sig() {
        let mut a = BTreeMap::new();
        a.insert("method".to_string(), "track.love".to_string());

        let mut b = a.clone();
        b.insert("format".to_string(), "json".to_string());
        b.insert("api_sig".to_string(), "garbage".to_string());

        assert_eq!(sign(&a, "secret"), sign(&b, "secret"));
    }

    #[test]
    fn eligibility_rules() {
        // < 30s: never eligible
        assert!(!is_eligible_for_scrobble(29_000, 29_000));

        // 60s track, listened 29s: under 50% threshold
        assert!(!is_eligible_for_scrobble(60_000, 29_000));
        // 60s track, listened 31s: just over 50%
        assert!(is_eligible_for_scrobble(60_000, 31_000));

        // 600s (10min) track: 50% would be 300s but cap is 240s
        assert!(is_eligible_for_scrobble(600_000, 240_000));
        assert!(!is_eligible_for_scrobble(600_000, 239_000));

        // Exactly threshold
        assert!(is_eligible_for_scrobble(120_000, 60_000));
    }
}
