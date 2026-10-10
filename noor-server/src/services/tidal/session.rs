//! The TIDAL session: sole owner of the user's TIDAL tokens and their
//! lifecycle (login, logout, persistence, refresh, needs-reconnect latch).
//! See CONTEXT.md "TIDAL session" and docs/adr/0002.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::services::tidal::auth::{TidalTokens, is_refresh_rejected};
use crate::services::tidal::client::TidalClient;

/// Exchanges a refresh token for fresh tokens. Two adapters: the real TIDAL
/// auth endpoint (`AuthEndpointRefresher`) and a scripted fake in tests.
/// Return `auth::RefreshRejected` when retrying cannot help.
#[async_trait]
pub trait TokenRefresher: Send + Sync {
    async fn refresh(&self, current: &TidalTokens) -> Result<TidalTokens>;
}

/// The session has no tokens or is latched in needs-reconnect. Treat it like
/// "TIDAL not connected".
#[derive(Debug, thiserror::Error)]
#[error("TIDAL session expired; reconnect TIDAL")]
pub struct SessionExpired;

pub fn is_session_expired(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| cause.is::<SessionExpired>())
}

pub struct TidalSessionConfig {
    pub api_http: reqwest::Client,
    pub api_base: String,
    pub refresher: Arc<dyn TokenRefresher>,
    pub store: Option<TokenStore>,
    pub events: Option<broadcast::Sender<crate::AppEvent>>,
}

#[derive(Clone)]
pub struct TidalSession {
    inner: Arc<Inner>,
}

struct Inner {
    tokens: RwLock<Option<TidalTokens>>,
    needs_reconnect: AtomicBool,
    /// Serializes refresh-token exchange. TIDAL may rotate the refresh token
    /// on use, so concurrent auth failures must wait for the first exchange
    /// and reuse its result instead of submitting the same token twice.
    refresh_lock: tokio::sync::Mutex<()>,
    api_http: reqwest::Client,
    api_base: String,
    refresher: Arc<dyn TokenRefresher>,
    store: Option<TokenStore>,
    events: Option<broadcast::Sender<crate::AppEvent>>,
}

impl TidalSession {
    pub fn new(config: TidalSessionConfig, initial: Option<TidalTokens>) -> Self {
        Self {
            inner: Arc::new(Inner {
                tokens: RwLock::new(initial),
                needs_reconnect: AtomicBool::new(false),
                refresh_lock: tokio::sync::Mutex::new(()),
                api_http: config.api_http,
                api_base: config.api_base,
                refresher: config.refresher,
                store: config.store,
                events: config.events,
            }),
        }
    }

    /// Snapshot of the current tokens (None when logged out).
    pub fn tokens(&self) -> Option<TidalTokens> {
        self.inner
            .tokens
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn needs_reconnect(&self) -> bool {
        self.inner.needs_reconnect.load(Ordering::SeqCst)
    }

    pub fn is_connected(&self) -> bool {
        self.tokens().is_some() && !self.needs_reconnect()
    }

    /// A client bound to this session: it reads the live token per request and
    /// refreshes-and-retries once on an auth failure. None when logged out.
    pub fn client(&self) -> Option<TidalClient> {
        let tokens = self.tokens()?;
        Some(TidalClient::for_session(self.clone(), &tokens.country_code))
    }

    pub(crate) fn api_http(&self) -> reqwest::Client {
        self.inner.api_http.clone()
    }

    pub(crate) fn api_base(&self) -> &str {
        &self.inner.api_base
    }

    /// The access token to put on the next request, or `SessionExpired`.
    pub(crate) fn access_token_for_request(&self) -> Result<String> {
        if self.needs_reconnect() {
            return Err(SessionExpired.into());
        }
        self.tokens()
            .map(|tokens| tokens.access_token)
            .ok_or_else(|| SessionExpired.into())
    }

    /// Refresh because `stale_access_token` was refused or is locally expired,
    /// unless another caller already replaced it while we waited (then reuse
    /// theirs). A definitive rejection latches needs-reconnect; a transient
    /// failure (network, 5xx) leaves the session as it was.
    pub async fn refresh_stale(&self, stale_access_token: &str) -> Result<TidalTokens> {
        let _permit = self.inner.refresh_lock.lock().await;
        if self.needs_reconnect() {
            return Err(SessionExpired.into());
        }
        let current = self.tokens().ok_or(SessionExpired)?;
        if current.access_token != stale_access_token {
            return Ok(current);
        }
        if current.refresh_token.trim().is_empty() {
            self.latch_needs_reconnect("no refresh token");
            return Err(SessionExpired.into());
        }
        tracing::info!(
            target: "noor.sync.tidal",
            event = "session_refresh_start",
            user_id = %current.user_id,
            "Refreshing TIDAL session"
        );
        match self.inner.refresher.refresh(&current).await {
            Ok(mut refreshed) => {
                fill_missing_identity(&mut refreshed, &current);
                if let Some(store) = &self.inner.store
                    && let Err(error) = store.save(&refreshed)
                {
                    // Keep the new tokens in memory anyway: TIDAL may already
                    // have rotated the old refresh token away.
                    tracing::warn!(
                        target: "noor.sync.tidal",
                        event = "session_refresh_persist_failed",
                        error = %error,
                        "Failed to persist refreshed TIDAL tokens"
                    );
                }
                self.set_tokens_inner(Some(refreshed.clone()));
                tracing::info!(
                    target: "noor.sync.tidal",
                    event = "session_refresh_success",
                    user_id = %refreshed.user_id,
                    "TIDAL session refresh succeeded"
                );
                Ok(refreshed)
            }
            Err(error) if is_refresh_rejected(&error) => {
                self.latch_needs_reconnect(&error.to_string());
                Err(SessionExpired.into())
            }
            Err(error) => Err(error.context("TIDAL session refresh failed")),
        }
    }

    /// Latch needs-reconnect from outside a refresh (stream path: still
    /// rejected right after a successful refresh).
    pub(crate) fn mark_needs_reconnect(&self, reason: &str) {
        self.latch_needs_reconnect(reason);
    }

    fn latch_needs_reconnect(&self, reason: &str) {
        let was = self.inner.needs_reconnect.swap(true, Ordering::SeqCst);
        if !was {
            tracing::warn!(
                target: "noor.sync.tidal",
                event = "session_needs_reconnect",
                reason = %reason,
                "TIDAL session needs reconnect; refresh disabled until next login"
            );
            self.emit_changed();
        }
    }

    fn set_tokens_inner(&self, tokens: Option<TidalTokens>) {
        *self
            .inner
            .tokens
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = tokens;
    }

    fn emit_changed(&self) {
        if let Some(events) = &self.inner.events {
            let _ = events.send(crate::AppEvent::TidalSessionChanged);
        }
    }
}

fn fill_missing_identity(refreshed: &mut TidalTokens, previous: &TidalTokens) {
    if refreshed.user_id.is_empty() {
        refreshed.user_id = previous.user_id.clone();
    }
    if refreshed.country_code.is_empty() {
        refreshed.country_code = previous.country_code.clone();
    }
    if refreshed.auth_flow.is_none() {
        refreshed.auth_flow = previous.auth_flow.clone();
    }
}

/// Placeholder until persistence lands; keeps this step compiling.
#[derive(Clone)]
pub struct TokenStore;

impl TokenStore {
    fn save(&self, _tokens: &TidalTokens) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    pub(super) fn tokens(access: &str) -> TidalTokens {
        TidalTokens {
            access_token: access.to_string(),
            refresh_token: "refresh-1".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            user_id: "user-1".to_string(),
            country_code: "US".to_string(),
            auth_flow: Some("pkce".to_string()),
        }
    }

    pub(super) enum Outcome {
        Ok(TidalTokens),
        Rejected,
        Transient,
    }

    pub(super) struct ScriptedRefresher {
        calls: AtomicUsize,
        script: Mutex<VecDeque<Outcome>>,
        delay: std::time::Duration,
    }

    impl ScriptedRefresher {
        pub fn new(script: Vec<Outcome>) -> Arc<Self> {
            Self::with_delay(script, std::time::Duration::ZERO)
        }

        pub fn with_delay(script: Vec<Outcome>, delay: std::time::Duration) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                script: Mutex::new(script.into()),
                delay,
            })
        }

        pub fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl TokenRefresher for ScriptedRefresher {
        async fn refresh(&self, _current: &TidalTokens) -> Result<TidalTokens> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            let next = self.script.lock().unwrap().pop_front();
            match next {
                Some(Outcome::Ok(tokens)) => Ok(tokens),
                Some(Outcome::Rejected) => Err(crate::services::tidal::auth::RefreshRejected {
                    status: "400 Bad Request".to_string(),
                    body: "invalid_grant".to_string(),
                }
                .into()),
                Some(Outcome::Transient) | None => Err(anyhow::anyhow!("connection reset")),
            }
        }
    }

    pub(super) fn session_with(
        initial: Option<TidalTokens>,
        refresher: Arc<ScriptedRefresher>,
    ) -> TidalSession {
        TidalSession::new(
            TidalSessionConfig {
                api_http: reqwest::Client::new(),
                api_base: "http://127.0.0.1:9".to_string(),
                refresher,
                store: None,
                events: None,
            },
            initial,
        )
    }

    #[tokio::test]
    async fn concurrent_auth_failures_share_one_refresh() {
        let refresher = ScriptedRefresher::with_delay(
            vec![Outcome::Ok(tokens("new"))],
            std::time::Duration::from_millis(50),
        );
        let session = session_with(Some(tokens("old")), refresher.clone());
        let mut handles = Vec::new();
        for _ in 0..5 {
            let session = session.clone();
            handles.push(tokio::spawn(
                async move { session.refresh_stale("old").await },
            ));
        }
        for handle in handles {
            assert_eq!(handle.await.unwrap().unwrap().access_token, "new");
        }
        assert_eq!(refresher.call_count(), 1);
        assert_eq!(session.tokens().unwrap().access_token, "new");
    }

    #[tokio::test]
    async fn refresh_fills_identity_fields_tidal_omitted() {
        let mut bare = tokens("new");
        bare.user_id.clear();
        bare.country_code.clear();
        bare.auth_flow = None;
        let session = session_with(
            Some(tokens("old")),
            ScriptedRefresher::new(vec![Outcome::Ok(bare)]),
        );
        let refreshed = session.refresh_stale("old").await.unwrap();
        assert_eq!(refreshed.user_id, "user-1");
        assert_eq!(refreshed.country_code, "US");
        assert_eq!(refreshed.auth_flow.as_deref(), Some("pkce"));
    }

    #[tokio::test]
    async fn rejected_refresh_latches_and_stops_refreshing() {
        let refresher = ScriptedRefresher::new(vec![Outcome::Rejected, Outcome::Ok(tokens("x"))]);
        let session = session_with(Some(tokens("old")), refresher.clone());
        let first = session.refresh_stale("old").await.unwrap_err();
        assert!(is_session_expired(&first));
        assert!(session.needs_reconnect());
        assert!(!session.is_connected());
        let second = session.refresh_stale("old").await.unwrap_err();
        assert!(is_session_expired(&second));
        assert!(is_session_expired(
            &session.access_token_for_request().unwrap_err()
        ));
        assert_eq!(refresher.call_count(), 1);
    }

    #[tokio::test]
    async fn transient_refresh_failure_does_not_latch() {
        let refresher =
            ScriptedRefresher::new(vec![Outcome::Transient, Outcome::Ok(tokens("new"))]);
        let session = session_with(Some(tokens("old")), refresher.clone());
        let err = session.refresh_stale("old").await.unwrap_err();
        assert!(!is_session_expired(&err));
        assert!(!session.needs_reconnect());
        assert_eq!(
            session.refresh_stale("old").await.unwrap().access_token,
            "new"
        );
        assert_eq!(refresher.call_count(), 2);
    }

    #[tokio::test]
    async fn missing_refresh_token_latches_without_calling_tidal() {
        let mut no_refresh = tokens("old");
        no_refresh.refresh_token.clear();
        let refresher = ScriptedRefresher::new(vec![]);
        let session = session_with(Some(no_refresh), refresher.clone());
        assert!(is_session_expired(
            &session.refresh_stale("old").await.unwrap_err()
        ));
        assert!(session.needs_reconnect());
        assert_eq!(refresher.call_count(), 0);
    }

    #[tokio::test]
    async fn no_tokens_means_not_connected() {
        let session = session_with(None, ScriptedRefresher::new(vec![]));
        assert!(!session.is_connected());
        assert!(session.client().is_none());
        assert!(is_session_expired(
            &session.access_token_for_request().unwrap_err()
        ));
    }
}
