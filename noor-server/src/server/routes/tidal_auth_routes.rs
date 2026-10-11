//! TIDAL login, poll, status, logout and persisted-token rehydration. Token ownership lives in services/tidal/session.rs (ADR-0002).

use super::*;

/// Start PKCE login flow. Returns a browser URL. The user must paste the
/// redirected TIDAL URL into the completion endpoint after signing in.
pub(super) async fn tidal_login(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let login = tidal_auth::start_pkce_login().map_err(|e| {
        tracing::error!("TIDAL PKCE login error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Cancel any previous in-flight login polling
    {
        let mut s = state.write().await;
        s.tidal_login_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        s.tidal_login_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    }

    Ok(Json(json!({
        "mode": "pkce",
        "verify_url": login.verify_url,
        "requires_redirect_url": true,
    })))
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct TidalLoginCompletePayload {
    pub(super) redirect_url: String,
}

pub(super) async fn tidal_login_complete(
    State(state): State<SharedState>,
    Json(payload): Json<TidalLoginCompletePayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let http = {
        let s = state.read().await;
        s.http_client.clone()
    };

    let tokens = tidal_auth::complete_pkce_login(&http, &payload.redirect_url)
        .await
        .map_err(|e| {
            tracing::error!("TIDAL PKCE completion error: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("TIDAL login failed: {e}") })),
            )
        })?;

    persist_tidal_tokens(&state, &tokens).await.map_err(|e| {
        tracing::error!("TIDAL token persist error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed to persist TIDAL login" })),
        )
    })?;
    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::PlaybackStateChanged);
    }

    Ok(Json(json!({
        "status": "authenticated",
        "user_id": tokens.user_id,
        "country_code": tokens.country_code,
        "auth_flow": tokens.auth_flow,
    })))
}

/// Check if polling has completed (frontend polls this).
pub(super) async fn tidal_poll(State(state): State<SharedState>) -> Json<Value> {
    let tokens = match load_persisted_tidal_tokens(&state).await {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!(
                "Failed to rehydrate persisted TIDAL tokens during login poll: {}",
                error
            );
            None
        }
    };

    if let Some(tokens) = tokens {
        Json(json!({
            "status": "authenticated",
            "user_id": tokens.user_id,
            "country_code": tokens.country_code,
        }))
    } else {
        Json(json!({
            "status": "pending",
        }))
    }
}

pub(in crate::server) async fn load_persisted_tidal_tokens(
    state: &SharedState,
) -> anyhow::Result<Option<tidal_auth::TidalTokens>> {
    let session = state.read().await.tidal.clone();
    session.reload_from_store()
}

/// Get TIDAL backoff gate status.
pub(super) async fn get_tidal_backoff_status() -> impl axum::response::IntoResponse {
    let state = crate::services::tidal::backoff::global().state();
    axum::Json(state)
}

/// Get TIDAL connection status.
pub(super) async fn tidal_status(State(state): State<SharedState>) -> Json<Value> {
    let session = state.read().await.tidal.clone();
    let tokens = match session.reload_from_store() {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!("Failed to rehydrate persisted TIDAL tokens: {}", error);
            None
        }
    };

    if let Some(mut tokens) = tokens {
        if !tokens.is_pkce() {
            tidal_auth::warn_if_fallback_client_credentials();
        }
        let mut expired = session.needs_reconnect()
            || tidal_tokens_locally_expired(&state, &tokens)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!("Failed to inspect persisted TIDAL token expiry: {error}");
                    false
                });
        if expired && !session.needs_reconnect() && !tokens.refresh_token.trim().is_empty() {
            match session.refresh_stale(&tokens.access_token).await {
                Ok(refreshed) => {
                    tokens = refreshed;
                    expired = false;
                }
                Err(error) => {
                    tracing::warn!("Failed to refresh expired TIDAL session for status: {error}");
                }
            }
        }
        Json(tidal_status_payload(
            Some(&tokens),
            expired,
            tidal_auth::tidal_pkce_client_credential_source(),
            tidal_auth::tidal_client_credential_source(),
        ))
    } else {
        Json(tidal_status_payload(
            None,
            false,
            tidal_auth::tidal_pkce_client_credential_source(),
            tidal_auth::tidal_client_credential_source(),
        ))
    }
}

pub(super) fn tidal_status_payload(
    tokens: Option<&tidal_auth::TidalTokens>,
    token_expired: bool,
    pkce_source: tidal_auth::TidalCredentialSource,
    legacy_source: tidal_auth::TidalCredentialSource,
) -> Value {
    let Some(tokens) = tokens else {
        return json!({ "connected": false });
    };
    let auth_flow = tokens.auth_flow.as_deref().unwrap_or("legacy");
    if token_expired {
        return json!({
            "connected": false,
            "reason": "token_expired",
            "user_id": tokens.user_id,
            "country_code": tokens.country_code,
            "auth_flow": auth_flow,
        });
    }
    let mut body = json!({
        "connected": true,
        "user_id": tokens.user_id,
        "country_code": tokens.country_code,
        "auth_flow": auth_flow,
    });
    if let Some(map) = body.as_object_mut() {
        if auth_flow == "pkce" {
            map.insert(
                "pkce_client_credential_source".to_string(),
                json!(pkce_source.as_str()),
            );
        } else {
            map.insert(
                "legacy_client_credential_source".to_string(),
                json!(legacy_source.as_str()),
            );
        }
    }
    body
}

pub(super) async fn tidal_tokens_locally_expired(
    state: &SharedState,
    tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<bool> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let record = db.with_conn(|conn| {
        let result = conn.query_row(
            "SELECT token_expiry, connected_at FROM service_auth WHERE service='tidal'",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        );
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    })?;
    let Some((token_expiry, connected_at)) = record else {
        return Ok(false);
    };
    Ok(tidal_token_expired_at(
        token_expiry.as_deref(),
        connected_at.as_deref(),
        tokens.expires_in,
        chrono::Utc::now(),
    ))
}

pub(super) fn tidal_token_expired_at(
    token_expiry: Option<&str>,
    connected_at: Option<&str>,
    expires_in: i64,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let expiry = token_expiry.and_then(parse_service_auth_time).or_else(|| {
        let connected_at = connected_at.and_then(parse_service_auth_time)?;
        Some(connected_at + chrono::Duration::seconds(expires_in.max(0)))
    });
    expiry
        .map(|expiry| expiry <= now + chrono::Duration::seconds(60))
        .unwrap_or(false)
}

pub(super) fn parse_service_auth_time(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|dt| {
                    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc)
                })
        })
}

/// Clear TIDAL session (logout).
pub(super) async fn tidal_logout(State(state): State<SharedState>) -> Json<Value> {
    tracing::info!(target: "noor.sync.tidal", event = "session_logout", "TIDAL session cleared by user");
    let _ = clear_tidal_session(&state).await;
    Json(json!({ "status": "logged_out" }))
}

pub(super) async fn clear_tidal_session(state: &SharedState) -> anyhow::Result<()> {
    let mut s = state.write().await;
    if let Some(runtime) = s.playback_runtime.take() {
        let _ = runtime.handle.shutdown();
    }
    s.playback_runtime_info = None;
    // Flush any in-flight listen session so disconnecting TIDAL doesn't drop
    // the partial listen on the floor. flush_*_locked take()s the session on
    // success; if the DB write fails the session stays in s.active_listen_session
    // and is cleared by the explicit None below.
    if let Err(err) = flush_active_listen_session_locked(
        &mut s,
        chrono::Utc::now(),
        player::ListenSessionEndReason::Stopped,
    ) {
        tracing::warn!("flush on tidal disconnect failed: {err}");
    }
    s.active_listen_session = None;
    let session = s.tidal.clone();
    drop(s);
    session.logout().await
}

pub(super) async fn persist_tidal_tokens(
    state: &SharedState,
    tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<()> {
    let session = state.read().await.tidal.clone();
    session.login(tokens.clone()).await
}
