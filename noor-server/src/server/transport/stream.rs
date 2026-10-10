//! Playback stream resolution behind a seam. `StreamSource` has two adapters:
//! `TidalStreamSource` (TIDAL playbackinfo over HTTP) and, in tests,
//! `ScriptedStreamSource`.

use async_trait::async_trait;
use std::sync::Arc;

use crate::SharedState;
use crate::playback::runtime as playback_runtime;
use crate::server::routes::dj_routes;
use crate::services::tidal::client::TidalClient;

use crate::services::tidal::stream as tidal_stream;

#[async_trait]
pub trait StreamSource: Send + Sync {
    async fn resolve(
        &self,
        access_token: &str,
        request: &tidal_stream::StreamRequest,
    ) -> Result<tidal_stream::StreamInfo, tidal_stream::StreamResolveError>;
}

pub(crate) struct TidalStreamSource {
    http: reqwest::Client,
}

impl TidalStreamSource {
    pub(crate) fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl StreamSource for TidalStreamSource {
    async fn resolve(
        &self,
        access_token: &str,
        request: &tidal_stream::StreamRequest,
    ) -> Result<tidal_stream::StreamInfo, tidal_stream::StreamResolveError> {
        tidal_stream::resolve_stream(&self.http, access_token, request).await
    }
}

#[cfg(test)]
type ScriptedAnswer = Result<tidal_stream::StreamInfo, tidal_stream::StreamResolveError>;

/// Test adapter: answers with scripted results in order, then behaves like
/// TIDAL rejecting an offline test token (session expired).
#[cfg(test)]
pub(crate) struct ScriptedStreamSource {
    script: std::sync::Mutex<std::collections::VecDeque<ScriptedAnswer>>,
    calls: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl ScriptedStreamSource {
    pub(crate) fn new(script: Vec<ScriptedAnswer>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            script: std::sync::Mutex::new(script.into()),
            calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub(crate) fn offline() -> std::sync::Arc<Self> {
        Self::new(Vec::new())
    }

    pub(crate) fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
#[async_trait]
impl StreamSource for ScriptedStreamSource {
    async fn resolve(
        &self,
        _access_token: &str,
        _request: &tidal_stream::StreamRequest,
    ) -> Result<tidal_stream::StreamInfo, tidal_stream::StreamResolveError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.script.lock().unwrap().pop_front().unwrap_or_else(|| {
            Err(tidal_stream::StreamResolveError::SessionExpired {
                message: "offline test stream source".to_string(),
            })
        })
    }
}

#[cfg(test)]
pub(crate) fn test_stream_info(track_id: i64) -> tidal_stream::StreamInfo {
    tidal_stream::StreamInfo {
        url: format!("https://example.invalid/{track_id}.flac"),
        segment_urls: Vec::new(),
        segment_offsets_ms: Vec::new(),
        track_id,
        audio_quality: "LOSSLESS".to_string(),
        codec: "flac".to_string(),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
    }
}

pub(crate) enum TidalPlaybackError {
    NotConnected,
    SessionRefreshFailed(String),
    StreamResolve(tidal_stream::StreamResolveError),
}

impl TidalPlaybackError {
    /// True when the failure is specific to *this track's asset*, so the right
    /// response is to skip past it and keep the queue moving. TIDAL's
    /// `4005 / "Asset is not ready"` and hard stream rejections mean this id
    /// won't play right now. Everything else (not connected, session refresh,
    /// network, rate-limit) is systemic or transient and must NOT burn through
    /// the queue one dead row at a time.
    pub(crate) fn is_track_unplayable(&self) -> bool {
        match self {
            TidalPlaybackError::StreamResolve(err) => {
                err.is_asset_not_ready() || err.is_track_specific_rejection()
            }
            _ => false,
        }
    }

    /// The narrower `4005 / asset-not-ready` case. A track that played fine
    /// before and now returns this usually had its catalog id rotated by TIDAL,
    /// so it's worth a background id re-resolve. A plain stream rejection
    /// (region lock, takedown) is not.
    pub(crate) fn is_asset_not_ready(&self) -> bool {
        matches!(self, TidalPlaybackError::StreamResolve(err) if err.is_asset_not_ready())
    }
}

#[cfg(test)]
mod unplayable_classification_tests {
    use super::*;
    use crate::services::tidal::stream::StreamResolveError;

    fn stream_resolve(err: StreamResolveError) -> TidalPlaybackError {
        TidalPlaybackError::StreamResolve(err)
    }

    #[test]
    fn asset_not_ready_4005_is_skippable_and_reresolvable() {
        let err = stream_resolve(StreamResolveError::StreamRejected {
            message: r#"TIDAL rejected playback request with 401 Unauthorized: {"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#.to_string(),
        });
        assert!(err.is_track_unplayable());
        assert!(err.is_asset_not_ready());
    }

    #[test]
    fn stream_rejected_without_4005_skips_but_does_not_reresolve() {
        let err = stream_resolve(StreamResolveError::StreamRejected {
            message: "TIDAL rejected playback request with 403 Forbidden".to_string(),
        });
        assert!(err.is_track_unplayable());
        assert!(!err.is_asset_not_ready());
    }

    #[test]
    fn rate_limit_and_request_timeout_are_not_track_unplayable() {
        for message in [
            "TIDAL rejected playback request with 429 Too Many Requests",
            "TIDAL rejected playback request with 408 Request Timeout",
        ] {
            let err = stream_resolve(StreamResolveError::StreamRejected {
                message: message.to_string(),
            });
            assert!(!err.is_track_unplayable(), "{message}");
        }
    }

    #[test]
    fn network_and_session_failures_are_not_skippable() {
        // A transient network error must not burn through the queue one row at a time.
        let network = stream_resolve(StreamResolveError::RequestFailed {
            message: "error sending request: dns error".to_string(),
        });
        assert!(!network.is_track_unplayable());
        assert!(!network.is_asset_not_ready());

        assert!(!TidalPlaybackError::NotConnected.is_track_unplayable());
        assert!(
            !TidalPlaybackError::SessionRefreshFailed("boom".to_string()).is_track_unplayable()
        );
    }
}

pub(crate) fn runtime_stream_resolver(
    state: SharedState,
) -> playback_runtime::RuntimeStreamResolver {
    let state = Arc::downgrade(&state);
    Arc::new(move |request| {
        let state = state.clone();
        Box::pin(async move {
            let state = state
                .upgrade()
                .ok_or_else(|| anyhow::anyhow!("server state is no longer available"))?;
            resolve_tidal_runtime_stream(&state, request).await
        })
    })
}

async fn resolve_tidal_runtime_stream(
    state: &SharedState,
    request: tidal_stream::StreamRequest,
) -> anyhow::Result<tidal_stream::StreamInfo> {
    ensure_tidal_content_allowed(state, request.track_id).await?;
    let session = state.read().await.tidal.clone();
    if session.needs_reconnect() {
        return Err(crate::services::tidal::session::SessionExpired.into());
    }
    let tokens = session
        .tokens()
        .ok_or_else(|| anyhow::anyhow!("TIDAL is not connected."))?;

    let stream_source = state.read().await.stream_source.clone();

    match stream_source.resolve(&tokens.access_token, &request).await {
        Ok(info) => {
            dj_routes::clear_unavailable_tidal_source(request.track_id);
            Ok(info)
        }
        Err(error) if error.is_session_expired() => {
            tracing::warn!(
                target: "noor.playback.tidal",
                event = "runtime_stream_session_expired",
                track_id = request.track_id,
                error = %error,
                "TIDAL session expired in playback decoder; refreshing session"
            );
            let refreshed = session.refresh_stale(&tokens.access_token).await?;
            match stream_source
                .resolve(&refreshed.access_token, &request)
                .await
            {
                Ok(info) => {
                    dj_routes::clear_unavailable_tidal_source(request.track_id);
                    Ok(info)
                }
                Err(retry_error) if retry_error.is_session_expired() => {
                    session.mark_needs_reconnect("runtime stream still rejected after refresh");
                    Err(anyhow::anyhow!(
                        "TIDAL session expired after refresh while resolving runtime stream: {retry_error}"
                    ))
                }
                Err(retry_error) => Err(anyhow::Error::from(retry_error)),
            }
        }
        Err(error) => Err(anyhow::Error::from(error)),
    }
}

/// Recheck late-resolved/stale queue rows before either preparation or runtime decoding.
pub(crate) async fn ensure_tidal_content_allowed(
    state: &SharedState,
    tidal_id: i64,
) -> anyhow::Result<()> {
    let (db, tidal_session, tokens) = {
        let guard = state.read().await;
        (guard.db.clone(), guard.tidal.clone(), guard.tidal.tokens())
    };
    let needs_label = db.with_conn(|conn| {
        if !crate::db::tidal_content::enabled(conn)? { return Ok(false); }
        let saved: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM tracks WHERE tidal_id=?1 AND (is_library=1 OR is_favorite=1))", [tidal_id], |row| row.get(0))?;
        let observed: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM tidal_track_labels WHERE tidal_id=?1)", [tidal_id], |row| row.get(0))?;
        Ok(!saved && !observed)
    })?;
    if needs_label && let Some(tokens) = tokens {
        let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
            .with_metadata_store(db.clone());
        // Unknown labels stay playable; a failed label lookup does not imply AI.
        let _ = client.get_track(tidal_id).await;
    }
    anyhow::ensure!(
        !db.with_conn(|conn| Ok(crate::db::tidal_content::is_blocked(conn, tidal_id)?))?,
        "Hidden by your AI-generated music filter. Saved library tracks remain available."
    );
    Ok(())
}

pub(crate) async fn resolve_tidal_playback_stream(
    state: &SharedState,
    track: &crate::db::models::Track,
    request: &tidal_stream::StreamRequest,
) -> Result<tidal_stream::StreamInfo, TidalPlaybackError> {
    if let Err(error) = ensure_tidal_content_allowed(state, request.track_id).await {
        return Err(TidalPlaybackError::StreamResolve(
            tidal_stream::StreamResolveError::StreamRejected {
                message: error.to_string(),
            },
        ));
    }
    let session = state.read().await.tidal.clone();
    if session.needs_reconnect() {
        return Err(TidalPlaybackError::NotConnected);
    }
    let tokens = session.tokens().ok_or(TidalPlaybackError::NotConnected)?;

    let stream_source = state.read().await.stream_source.clone();

    match stream_source.resolve(&tokens.access_token, request).await {
        Ok(info) => {
            dj_routes::clear_unavailable_tidal_source(request.track_id);
            Ok(info)
        }
        Err(err) if err.is_session_expired() => {
            tracing::warn!(
                target: "noor.playback.tidal",
                event = "playback_stream_session_expired",
                track_id = track.id,
                error = %err,
                "TIDAL session expired while resolving playback stream"
            );

            let refreshed = match session.refresh_stale(&tokens.access_token).await {
                Ok(tokens) => tokens,
                Err(recover_err) => {
                    // Do NOT clear the session here. A transient network error during
                    // token refresh should not log the user out permanently.
                    tracing::error!(
                        target: "noor.playback.tidal",
                        event = "playback_stream_refresh_failed",
                        track_id = track.id,
                        error = %recover_err,
                        original_error = %err,
                        "TIDAL session refresh failed while starting playback; keeping stored tokens"
                    );
                    return Err(TidalPlaybackError::SessionRefreshFailed(
                        recover_err.to_string(),
                    ));
                }
            };

            tracing::info!(
                target: "noor.playback.tidal",
                event = "playback_stream_session_recovered",
                track_id = track.id,
                "TIDAL session refreshed; retrying stream resolution"
            );

            match stream_source
                .resolve(&refreshed.access_token, request)
                .await
            {
                Ok(info) => {
                    dj_routes::clear_unavailable_tidal_source(request.track_id);
                    Ok(info)
                }
                Err(retry_err) if retry_err.is_session_expired() => {
                    // Still expired after a successful refresh: TIDAL revoked the
                    // account. Latch needs-reconnect so nothing keeps refreshing.
                    session.mark_needs_reconnect("playback stream still rejected after refresh");
                    tracing::error!(
                        target: "noor.playback.tidal",
                        event = "playback_stream_retry_session_expired",
                        track_id = track.id,
                        error = %retry_err,
                        "TIDAL stream still rejected the refreshed session; reconnect needed"
                    );
                    Err(TidalPlaybackError::SessionRefreshFailed(
                        retry_err.to_string(),
                    ))
                }
                Err(retry_err) => Err(TidalPlaybackError::StreamResolve(retry_err)),
            }
        }
        Err(err) => Err(TidalPlaybackError::StreamResolve(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scripted_source_serves_scripted_answers_then_offline_error() {
        let source = ScriptedStreamSource::new(vec![Ok(test_stream_info(7))]);
        let request = tidal_stream::StreamRequest::new(7, "LOSSLESS");
        let first = source.resolve("token", &request).await.unwrap();
        assert_eq!(first.track_id, 7);
        let second = source.resolve("token", &request).await.unwrap_err();
        assert!(second.is_session_expired());
        assert_eq!(source.call_count(), 2);
    }
}
