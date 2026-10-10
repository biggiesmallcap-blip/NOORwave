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

    /// A completed login: persist, replace tokens, clear the latch.
    pub async fn login(&self, tokens: TidalTokens) -> Result<()> {
        let _permit = self.inner.refresh_lock.lock().await;
        if let Some(store) = &self.inner.store {
            store.save(&tokens)?;
        }
        self.set_tokens_inner(Some(tokens));
        self.inner.needs_reconnect.store(false, Ordering::SeqCst);
        self.emit_changed();
        Ok(())
    }

    /// Forget the session in memory and on disk.
    pub async fn logout(&self) -> Result<()> {
        let _permit = self.inner.refresh_lock.lock().await;
        self.set_tokens_inner(None);
        self.inner.needs_reconnect.store(false, Ordering::SeqCst);
        let cleared = match &self.inner.store {
            Some(store) => store.clear(),
            None => Ok(()),
        };
        self.emit_changed();
        cleared
    }

    /// When memory is empty, rehydrate from the store (login completed by a
    /// path that only wrote the DB). Returns the tokens now in memory.
    pub fn reload_from_store(&self) -> Result<Option<TidalTokens>> {
        if let Some(tokens) = self.tokens() {
            return Ok(Some(tokens));
        }
        let Some(store) = &self.inner.store else {
            return Ok(None);
        };
        let loaded = store.load()?;
        if loaded.is_some() {
            self.set_tokens_inner(loaded.clone());
        }
        Ok(loaded)
    }

    /// Test-only: a logged-out session with no store, no events, and a
    /// refresher that always fails transiently.
    #[cfg(test)]
    pub(crate) fn disconnected_for_tests() -> Self {
        struct NoRefresh;

        #[async_trait]
        impl TokenRefresher for NoRefresh {
            async fn refresh(&self, _current: &TidalTokens) -> Result<TidalTokens> {
                anyhow::bail!("no TIDAL refresh in tests")
            }
        }

        Self::new(
            TidalSessionConfig {
                api_http: reqwest::Client::new(),
                api_base: OFFLINE_TEST_API_BASE.to_string(),
                refresher: Arc::new(NoRefresh),
                store: None,
                events: None,
            },
            None,
        )
    }

    /// Test-only: replace tokens without persistence or events.
    #[cfg(test)]
    pub(crate) fn set_tokens_for_test(&self, tokens: Option<TidalTokens>) {
        self.set_tokens_inner(tokens);
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

/// Production adapter: TIDAL's token endpoint, then a validation call so a
/// refreshed-but-useless token is caught before callers retry with it.
pub struct AuthEndpointRefresher {
    auth_http: reqwest::Client,
    api_http: reqwest::Client,
}

impl AuthEndpointRefresher {
    pub fn new(auth_http: reqwest::Client, api_http: reqwest::Client) -> Self {
        Self {
            auth_http,
            api_http,
        }
    }
}

#[async_trait]
impl TokenRefresher for AuthEndpointRefresher {
    async fn refresh(&self, current: &TidalTokens) -> Result<TidalTokens> {
        let mut refreshed = crate::services::tidal::auth::refresh_token(
            &self.auth_http,
            &current.refresh_token,
            current.auth_flow.as_deref(),
        )
        .await?;
        fill_missing_identity(&mut refreshed, current);
        let validation = TidalClient::with_http(
            self.api_http.clone(),
            refreshed.access_token.clone(),
            refreshed.country_code.clone(),
        )
        .validate_session(&refreshed.user_id)
        .await;
        match validation {
            Ok(()) => Ok(refreshed),
            Err(error) if crate::services::tidal::client::is_auth_failure(&error) => {
                Err(crate::services::tidal::auth::RefreshRejected {
                    status: "validation".to_string(),
                    body: error.to_string(),
                }
                .into())
            }
            Err(error) => Err(error.context("Refreshed TIDAL session still failed validation")),
        }
    }
}

impl TidalSessionConfig {
    pub fn production(
        api_http: reqwest::Client,
        auth_http: reqwest::Client,
        store: TokenStore,
        events: broadcast::Sender<crate::AppEvent>,
    ) -> Self {
        Self {
            refresher: Arc::new(AuthEndpointRefresher::new(auth_http, api_http.clone())),
            api_http,
            api_base: crate::services::tidal::client::TIDAL_API_URL.to_string(),
            store: Some(store),
            events: Some(events),
        }
    }
}

/// API base for test sessions: an unsupported scheme, so any request fails
/// immediately without touching the network (a closed localhost port can
/// take seconds to refuse on Windows).
#[cfg(test)]
pub(crate) const OFFLINE_TEST_API_BASE: &str = "noor-offline://tidal";

/// Encrypted persistence of the tokens in `service_auth` (service='tidal').
#[derive(Clone)]
pub struct TokenStore {
    db: crate::db::Database,
    master_key: crate::services::crypto::MasterKey,
}

impl TokenStore {
    pub fn new(db: crate::db::Database, master_key: crate::services::crypto::MasterKey) -> Self {
        Self { db, master_key }
    }

    /// Load persisted tokens, rewriting legacy plaintext rows encrypted.
    pub fn load(&self) -> Result<Option<TidalTokens>> {
        use crate::services::tidal::auth::{
            decode_persisted_tidal_tokens, encode_persisted_tidal_tokens,
        };
        let loaded = self.db.with_conn(|conn| {
            let result = conn.query_row(
                "SELECT access_token_enc FROM service_auth WHERE service='tidal'",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            );
            Ok(match result {
                Ok(bytes) => decode_persisted_tidal_tokens(&self.master_key, &bytes)?,
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(error) => return Err(error.into()),
            })
        })?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        let needs_rewrite = loaded.needs_encrypted_rewrite();
        let tokens = loaded.into_tokens();
        if needs_rewrite {
            let blob = encode_persisted_tidal_tokens(&self.master_key, &tokens)?;
            self.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE service_auth SET access_token_enc = ?1 WHERE service = 'tidal'",
                    rusqlite::params![blob],
                )?;
                Ok(())
            })?;
        }
        Ok(Some(tokens))
    }

    pub fn save(&self, tokens: &TidalTokens) -> Result<()> {
        let blob =
            crate::services::tidal::auth::encode_persisted_tidal_tokens(&self.master_key, tokens)?;
        let token_expiry =
            (chrono::Utc::now() + chrono::Duration::seconds(tokens.expires_in.max(0))).to_rfc3339();
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO service_auth (service, access_token_enc, user_id, token_expiry, connected_at)
                 VALUES ('tidal', ?1, ?2, ?3, datetime('now'))
                 ON CONFLICT(service) DO UPDATE SET access_token_enc=excluded.access_token_enc,
                 user_id=excluded.user_id, token_expiry=excluded.token_expiry, connected_at=excluded.connected_at",
                rusqlite::params![blob, tokens.user_id, token_expiry],
            )?;
            Ok(())
        })
    }

    pub fn clear(&self) -> Result<()> {
        self.db.with_conn(|conn| {
            conn.execute("DELETE FROM service_auth WHERE service='tidal'", [])?;
            Ok(())
        })
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
                api_base: OFFLINE_TEST_API_BASE.to_string(),
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

    fn migrated_db() -> crate::db::Database {
        let db = crate::db::Database::open_in_memory().expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(crate::db::schema::run_migrations)
            .expect("schema migrations");
        db
    }

    fn stored_session(
        store: &TokenStore,
        refresher: Arc<ScriptedRefresher>,
        events: Option<broadcast::Sender<crate::AppEvent>>,
    ) -> TidalSession {
        TidalSession::new(
            TidalSessionConfig {
                api_http: reqwest::Client::new(),
                api_base: OFFLINE_TEST_API_BASE.to_string(),
                refresher,
                store: Some(store.clone()),
                events,
            },
            store.load().unwrap(),
        )
    }

    fn test_store() -> TokenStore {
        TokenStore::new(
            migrated_db(),
            crate::services::crypto::MasterKey::ephemeral(),
        )
    }

    #[tokio::test]
    async fn login_persists_clears_latch_and_emits() {
        let store = test_store();
        let (tx, mut rx) = broadcast::channel(8);
        let session = stored_session(
            &store,
            ScriptedRefresher::new(vec![Outcome::Rejected]),
            Some(tx),
        );
        session.login(tokens("a")).await.unwrap();
        session.refresh_stale("a").await.unwrap_err();
        assert!(session.needs_reconnect());
        session.login(tokens("b")).await.unwrap();
        assert!(!session.needs_reconnect());
        assert_eq!(store.load().unwrap().unwrap().access_token, "b");
        let mut changes = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, crate::AppEvent::TidalSessionChanged) {
                changes += 1;
            }
        }
        assert_eq!(changes, 3, "login, latch, login");
    }

    #[tokio::test]
    async fn refresh_persists_rotated_tokens() {
        let store = test_store();
        let mut rotated = tokens("new");
        rotated.refresh_token = "refresh-2".to_string();
        let session = stored_session(
            &store,
            ScriptedRefresher::new(vec![Outcome::Ok(rotated)]),
            None,
        );
        session.login(tokens("old")).await.unwrap();
        session.refresh_stale("old").await.unwrap();
        let persisted = store.load().unwrap().unwrap();
        assert_eq!(persisted.access_token, "new");
        assert_eq!(persisted.refresh_token, "refresh-2");
    }

    #[tokio::test]
    async fn logout_clears_memory_and_store() {
        let store = test_store();
        let session = stored_session(&store, ScriptedRefresher::new(vec![]), None);
        session.login(tokens("a")).await.unwrap();
        session.logout().await.unwrap();
        assert!(session.tokens().is_none());
        assert!(store.load().unwrap().is_none());
    }

    #[tokio::test]
    async fn reload_from_store_rehydrates_when_memory_is_empty() {
        let store = test_store();
        let session = stored_session(&store, ScriptedRefresher::new(vec![]), None);
        store.save(&tokens("persisted")).unwrap();
        assert!(session.tokens().is_none());
        assert_eq!(
            session.reload_from_store().unwrap().unwrap().access_token,
            "persisted"
        );
        assert_eq!(session.tokens().unwrap().access_token, "persisted");
    }

    type SeenAuth = Arc<Mutex<Vec<String>>>;

    /// Local fake TIDAL API: answers each request with the next scripted
    /// (status, body) and records the Authorization header it saw.
    async fn fake_tidal(script: Vec<(u16, &'static str)>) -> (String, SeenAuth) {
        let seen: SeenAuth = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(VecDeque::from(script)));
        let app = axum::Router::new().fallback({
            let seen = seen.clone();
            move |headers: axum::http::HeaderMap| {
                let seen = seen.clone();
                let script = script.clone();
                async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    seen.lock().unwrap().push(auth);
                    let (status, body) = script.lock().unwrap().pop_front().unwrap_or((500, "{}"));
                    (axum::http::StatusCode::from_u16(status).unwrap(), body)
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    const TRACK_JSON: &str =
        r#"{"id":1,"title":"Song","duration":200,"artist":{"id":2,"name":"Artist"}}"#;

    fn session_at(
        base: String,
        initial: TidalTokens,
        refresher: Arc<ScriptedRefresher>,
    ) -> TidalSession {
        TidalSession::new(
            TidalSessionConfig {
                api_http: reqwest::Client::new(),
                api_base: base,
                refresher,
                store: None,
                events: None,
            },
            Some(initial),
        )
    }

    #[tokio::test]
    async fn client_refreshes_and_retries_once_on_auth_failure() {
        let (base, seen) = fake_tidal(vec![
            (401, r#"{"status":401,"subStatus":11003}"#),
            (200, TRACK_JSON),
        ])
        .await;
        let refresher = ScriptedRefresher::new(vec![Outcome::Ok(tokens("new"))]);
        let session = session_at(base, tokens("old"), refresher.clone());
        let track = session.client().unwrap().get_track(1).await.unwrap();
        assert_eq!(track.title, "Song");
        assert_eq!(refresher.call_count(), 1);
        assert_eq!(*seen.lock().unwrap(), vec!["Bearer old", "Bearer new"]);
    }

    #[tokio::test]
    async fn asset_not_ready_401_does_not_refresh() {
        let (base, seen) = fake_tidal(vec![(401, r#"{"status":401,"subStatus":4005}"#)]).await;
        let refresher = ScriptedRefresher::new(vec![Outcome::Ok(tokens("new"))]);
        let session = session_at(base, tokens("old"), refresher.clone());
        assert!(session.client().unwrap().get_track(1).await.is_err());
        assert_eq!(refresher.call_count(), 0);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn second_auth_failure_after_refresh_is_returned_not_looped() {
        let (base, seen) =
            fake_tidal(vec![(401, r#"{"status":401}"#), (401, r#"{"status":401}"#)]).await;
        let refresher = ScriptedRefresher::new(vec![Outcome::Ok(tokens("new"))]);
        let session = session_at(base, tokens("old"), refresher.clone());
        let err = session.client().unwrap().get_track(1).await.unwrap_err();
        assert!(crate::services::tidal::client::is_auth_failure(&err));
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(refresher.call_count(), 1);
    }

    #[tokio::test]
    async fn latched_session_fails_fast_without_http() {
        let (base, seen) = fake_tidal(vec![(401, r#"{"status":401}"#)]).await;
        let session = session_at(
            base,
            tokens("old"),
            ScriptedRefresher::new(vec![Outcome::Rejected]),
        );
        let client = session.client().unwrap();
        assert!(is_session_expired(&client.get_track(1).await.unwrap_err()));
        assert!(is_session_expired(&client.get_track(1).await.unwrap_err()));
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "second call never reached TIDAL"
        );
    }

    #[tokio::test]
    async fn client_picks_up_token_refreshed_by_another_caller() {
        let (base, seen) = fake_tidal(vec![(200, TRACK_JSON)]).await;
        let session = session_at(base, tokens("old"), ScriptedRefresher::new(vec![]));
        let client = session.client().unwrap();
        session.set_tokens_for_test(Some(tokens("fresh")));
        client.get_track(1).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), vec!["Bearer fresh"]);
    }
}
