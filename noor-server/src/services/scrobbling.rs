use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{SharedState, services};

const MAX_ATTEMPTS: i64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScrobblePayload {
    pub track_id: Option<i64>,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub listened_ms: Option<i64>,
    pub started_at_unix: Option<i64>,
}

impl ScrobblePayload {
    pub fn has_metadata(&self) -> bool {
        !self.artist.trim().is_empty() && !self.title.trim().is_empty()
    }
}

pub fn is_eligible_for_completed_scrobble(duration_ms: i64, listened_ms: i64) -> bool {
    services::lastfm::scrobble::is_eligible_for_scrobble(duration_ms, listened_ms)
}

pub async fn enqueue_now_playing(state: SharedState, payload: ScrobblePayload) {
    if !payload.has_metadata() {
        return;
    }
    let _ = enqueue_for_enabled_providers(state, "now_playing", payload).await;
}

pub async fn enqueue_completed(state: SharedState, payload: ScrobblePayload) {
    if !payload.has_metadata() {
        return;
    }
    let Some(duration_ms) = payload.duration_ms else {
        return;
    };
    let Some(listened_ms) = payload.listened_ms else {
        return;
    };
    if !is_eligible_for_completed_scrobble(duration_ms, listened_ms) {
        return;
    }
    let _ = enqueue_for_enabled_providers(state, "completed", payload).await;
}

pub async fn enqueue_backfill(state: SharedState, payload: ScrobblePayload) -> usize {
    if !payload.has_metadata() {
        return 0;
    }
    let Some(duration_ms) = payload.duration_ms else {
        return 0;
    };
    let Some(listened_ms) = payload.listened_ms else {
        return 0;
    };
    if !is_eligible_for_completed_scrobble(duration_ms, listened_ms) {
        return 0;
    }
    enqueue_for_enabled_providers(state, "backfill", payload).await
}

pub async fn enabled_provider_count(state: &SharedState) -> usize {
    enabled_scrobble_providers(state).await.len()
}

async fn enqueue_for_enabled_providers(
    state: SharedState,
    kind: &str,
    payload: ScrobblePayload,
) -> usize {
    let providers = enabled_scrobble_providers(&state).await;
    if providers.is_empty() {
        return 0;
    }
    let now = unix_now();
    let started_at = payload.started_at_unix.unwrap_or(now);
    let inserted = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let mut inserted = 0;
                for provider in &providers {
                    inserted +=
                        insert_scrobble_outbox(conn, provider, kind, &payload, started_at, now)?;
                }
                Ok::<_, anyhow::Error>(inserted)
            })
            .unwrap_or(0)
    };
    if inserted > 0 {
        spawn_drain(state);
    }
    inserted
}

async fn enabled_scrobble_providers(state: &SharedState) -> Vec<String> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| {
            let mut providers = Vec::new();
            if lastfm_ready(conn, &guard.master_key, guard.lastfm_api_secret.as_deref())? {
                providers.push("lastfm".to_string());
            }
            if listenbrainz_ready(conn)? {
                providers.push("listenbrainz".to_string());
            }
            Ok::<_, anyhow::Error>(providers)
        })
        .unwrap_or_default()
}

pub fn lastfm_ready(
    conn: &Connection,
    master: &services::crypto::MasterKey,
    env_secret: Option<&str>,
) -> Result<bool> {
    let creds = services::lastfm::auth::load_credentials(conn)?;
    let Some(creds) = creds.filter(|c| !c.api_key.trim().is_empty()) else {
        return Ok(false);
    };
    let session = services::lastfm::auth::load_session_key(conn, master)?;
    let has_secret = services::lastfm::auth::load_api_secret(conn, master)?
        .or_else(|| env_secret.map(str::to_string))
        .is_some();
    Ok(creds.session_user.is_some() && session.is_some() && has_secret)
}

pub fn listenbrainz_ready(conn: &Connection) -> Result<bool> {
    let creds = services::listenbrainz::load_credentials(conn)?;
    Ok(creds
        .filter(|c| c.scrobbling_enabled && c.user_name.is_some())
        .is_some()
        && services::listenbrainz::has_token(conn)?)
}

fn insert_scrobble_outbox(
    conn: &Connection,
    provider: &str,
    kind: &str,
    payload: &ScrobblePayload,
    started_at: i64,
    now: i64,
) -> Result<usize> {
    conn.execute(
        "INSERT OR IGNORE INTO scrobble_outbox (
             provider, kind, track_id, artist, title, album, duration_ms, listened_ms,
             started_at_unix, next_attempt_at, created_at, updated_at
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10)",
        params![
            provider,
            kind,
            payload.track_id,
            payload.artist.trim(),
            payload.title.trim(),
            payload.album.as_deref(),
            payload.duration_ms,
            payload.listened_ms,
            started_at,
            now
        ],
    )
    .map_err(Into::into)
}

#[derive(Debug, Clone)]
struct OutboxRow {
    id: i64,
    provider: String,
    kind: String,
    _track_id: Option<i64>,
    artist: String,
    title: String,
    album: Option<String>,
    duration_ms: Option<i64>,
    _listened_ms: Option<i64>,
    started_at_unix: Option<i64>,
    attempts: i64,
}

pub fn spawn_drain(state: SharedState) {
    tokio::spawn(async move {
        if let Err(error) = drain_due(state).await {
            tracing::warn!("scrobble outbox drain failed: {error:#}");
        }
    });
}

pub fn spawn_periodic_drain(state: SharedState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(error) = drain_due(state.clone()).await {
                tracing::warn!("periodic scrobble outbox drain failed: {error:#}");
            }
        }
    });
}

pub async fn drain_due(state: SharedState) -> Result<()> {
    let rows = {
        let guard = state.read().await;
        guard.db.with_conn(load_due_scrobbles)?
    };
    for row in rows {
        let result = submit_row(&state, &row).await;
        let guard = state.read().await;
        guard.db.with_conn(|conn| finish_row(conn, &row, result))?;
    }
    drain_feedback_due(state).await?;
    Ok(())
}

fn load_due_scrobbles(conn: &Connection) -> Result<Vec<OutboxRow>> {
    let now = unix_now();
    conn.execute(
        "UPDATE scrobble_outbox
            SET status = 'pending', updated_at = ?1
          WHERE status = 'processing' AND updated_at <= ?2",
        params![now, now - 300],
    )?;
    let mut stmt = conn.prepare(
        "SELECT id, provider, kind, track_id, artist, title, album, duration_ms,
                listened_ms, started_at_unix, attempts
           FROM scrobble_outbox
          WHERE status = 'pending' AND next_attempt_at <= ?1
          ORDER BY id
          LIMIT 20",
    )?;
    let rows = stmt
        .query_map(params![now], |row| {
            Ok(OutboxRow {
                id: row.get(0)?,
                provider: row.get(1)?,
                kind: row.get(2)?,
                _track_id: row.get(3)?,
                artist: row.get(4)?,
                title: row.get(5)?,
                album: row.get(6)?,
                duration_ms: row.get(7)?,
                _listened_ms: row.get(8)?,
                started_at_unix: row.get(9)?,
                attempts: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in &rows {
        conn.execute(
            "UPDATE scrobble_outbox SET status = 'processing', updated_at = ?1 WHERE id = ?2 AND status = 'pending'",
            params![now, row.id],
        )?;
    }
    Ok(rows)
}

async fn submit_row(state: &SharedState, row: &OutboxRow) -> Result<()> {
    match row.provider.as_str() {
        "lastfm" => submit_lastfm(state, row).await,
        "listenbrainz" => submit_listenbrainz(state, row).await,
        other => Err(anyhow!("unknown scrobble provider {other}")),
    }
}

async fn submit_lastfm(state: &SharedState, row: &OutboxRow) -> Result<()> {
    let (http, api_key, api_secret, session_key) = {
        let guard = state.read().await;
        let (api_key, api_secret, session_key) = guard.db.with_conn(|conn| {
            let creds = services::lastfm::auth::load_credentials(conn)?
                .ok_or_else(|| anyhow!("Last.fm credentials missing"))?;
            let api_secret = services::lastfm::auth::load_api_secret(conn, &guard.master_key)?
                .or_else(|| guard.lastfm_api_secret.clone())
                .ok_or_else(|| anyhow!("Last.fm API secret missing"))?;
            let session_key = services::lastfm::auth::load_session_key(conn, &guard.master_key)?
                .ok_or_else(|| anyhow!("Last.fm session missing"))?;
            Ok::<_, anyhow::Error>((creds.api_key, api_secret, session_key))
        })?;
        (guard.http_client.clone(), api_key, api_secret, session_key)
    };

    match row.kind.as_str() {
        "now_playing" => {
            services::lastfm::scrobble::update_now_playing(
                &http,
                &api_key,
                &api_secret,
                &session_key,
                &row.artist,
                &row.title,
                row.album.as_deref(),
                row.duration_ms,
            )
            .await
        }
        "completed" | "backfill" => {
            services::lastfm::scrobble::scrobble_track(
                &http,
                &api_key,
                &api_secret,
                &session_key,
                &row.artist,
                &row.title,
                row.album.as_deref(),
                row.started_at_unix.unwrap_or_else(unix_now),
            )
            .await
        }
        other => Err(anyhow!("unsupported Last.fm scrobble kind {other}")),
    }
}

async fn submit_listenbrainz(state: &SharedState, row: &OutboxRow) -> Result<()> {
    let (http, token) = {
        let guard = state.read().await;
        let token = guard.db.with_conn(|conn| {
            services::listenbrainz::load_token(conn, &guard.master_key)?
                .ok_or_else(|| anyhow!("ListenBrainz token missing"))
        })?;
        (guard.http_client.clone(), token)
    };
    let kind = match row.kind.as_str() {
        "now_playing" => services::listenbrainz::ListenType::PlayingNow,
        "completed" => services::listenbrainz::ListenType::Single,
        "backfill" => services::listenbrainz::ListenType::Import,
        other => return Err(anyhow!("unsupported ListenBrainz scrobble kind {other}")),
    };
    let payload = services::listenbrainz::ListenPayload {
        artist: row.artist.clone(),
        title: row.title.clone(),
        album: row.album.clone(),
        duration_ms: row.duration_ms,
        listened_at: row.started_at_unix,
    };
    services::listenbrainz::submit_listen(&http, &token, kind, &payload).await
}

fn finish_row(conn: &Connection, row: &OutboxRow, result: Result<()>) -> Result<()> {
    let now = unix_now();
    match result {
        Ok(()) => {
            conn.execute(
                "UPDATE scrobble_outbox SET status = 'sent', updated_at = ?1, last_error = NULL WHERE id = ?2",
                params![now, row.id],
            )?;
        }
        Err(error) => {
            let attempts = row.attempts + 1;
            let failed = attempts >= MAX_ATTEMPTS;
            let next = now + retry_delay_secs(attempts);
            conn.execute(
                "UPDATE scrobble_outbox
                    SET status = ?1, attempts = ?2, next_attempt_at = ?3, last_error = ?4, updated_at = ?5
                  WHERE id = ?6",
                params![
                    if failed { "failed" } else { "pending" },
                    attempts,
                    next,
                    error.to_string(),
                    now,
                    row.id
                ],
            )?;
        }
    }
    Ok(())
}

fn retry_delay_secs(attempts: i64) -> i64 {
    match attempts {
        0 | 1 => 60,
        2 => 300,
        3 => 900,
        _ => 3600,
    }
}

#[derive(Debug, Clone)]
struct FeedbackRow {
    id: i64,
    provider: String,
    action: String,
    _track_id: i64,
    artist: String,
    title: String,
    mbid: Option<String>,
    attempts: i64,
}

pub async fn enqueue_favorite_love(state: SharedState, track: &crate::db::models::Track) {
    let artist = track.artist_name.clone().unwrap_or_default();
    if artist.trim().is_empty() || track.title.trim().is_empty() {
        return;
    }
    {
        let guard = state.read().await;
        let _ = guard.db.with_conn(|conn| {
            if lastfm_ready(conn, &guard.master_key, guard.lastfm_api_secret.as_deref())? {
                insert_feedback(conn, "lastfm", track.id, &artist, &track.title, None)?;
            }
            Ok::<_, anyhow::Error>(())
        });
    }
    spawn_drain(state);
}

fn insert_feedback(
    conn: &Connection,
    provider: &str,
    track_id: i64,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
) -> Result<()> {
    let now = unix_now();
    conn.execute(
        "INSERT OR IGNORE INTO provider_feedback_outbox (
             provider, action, track_id, artist, title, mbid, next_attempt_at, created_at, updated_at
         )
         VALUES (?1, 'love', ?2, ?3, ?4, ?5, ?6, ?6, ?6)",
        params![provider, track_id, artist, title, mbid, now],
    )?;
    Ok(())
}

async fn drain_feedback_due(state: SharedState) -> Result<()> {
    let rows = {
        let guard = state.read().await;
        guard.db.with_conn(load_due_feedback)?
    };
    for row in rows {
        let result = submit_feedback_row(&state, &row).await;
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| finish_feedback_row(conn, &row, result))?;
    }
    Ok(())
}

fn load_due_feedback(conn: &Connection) -> Result<Vec<FeedbackRow>> {
    let now = unix_now();
    conn.execute(
        "UPDATE provider_feedback_outbox
            SET status = 'pending', updated_at = ?1
          WHERE status = 'processing' AND updated_at <= ?2",
        params![now, now - 300],
    )?;
    let mut stmt = conn.prepare(
        "SELECT id, provider, action, track_id, artist, title, mbid, attempts
           FROM provider_feedback_outbox
          WHERE status = 'pending' AND next_attempt_at <= ?1
          ORDER BY id
          LIMIT 20",
    )?;
    let rows = stmt
        .query_map(params![now], |row| {
            Ok(FeedbackRow {
                id: row.get(0)?,
                provider: row.get(1)?,
                action: row.get(2)?,
                _track_id: row.get(3)?,
                artist: row.get(4)?,
                title: row.get(5)?,
                mbid: row.get(6)?,
                attempts: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in &rows {
        conn.execute(
            "UPDATE provider_feedback_outbox SET status = 'processing', updated_at = ?1 WHERE id = ?2 AND status = 'pending'",
            params![now, row.id],
        )?;
    }
    Ok(rows)
}

async fn submit_feedback_row(state: &SharedState, row: &FeedbackRow) -> Result<()> {
    if row.action != "love" {
        return Err(anyhow!("unsupported feedback action {}", row.action));
    }
    match row.provider.as_str() {
        "lastfm" => {
            let (http, api_key, api_secret, session_key) = {
                let guard = state.read().await;
                let (api_key, api_secret, session_key) = guard.db.with_conn(|conn| {
                    let creds = services::lastfm::auth::load_credentials(conn)?
                        .ok_or_else(|| anyhow!("Last.fm credentials missing"))?;
                    let api_secret =
                        services::lastfm::auth::load_api_secret(conn, &guard.master_key)?
                            .or_else(|| guard.lastfm_api_secret.clone())
                            .ok_or_else(|| anyhow!("Last.fm API secret missing"))?;
                    let session_key =
                        services::lastfm::auth::load_session_key(conn, &guard.master_key)?
                            .ok_or_else(|| anyhow!("Last.fm session missing"))?;
                    Ok::<_, anyhow::Error>((creds.api_key, api_secret, session_key))
                })?;
                (guard.http_client.clone(), api_key, api_secret, session_key)
            };
            services::lastfm::scrobble::love_track(
                &http,
                &api_key,
                &api_secret,
                &session_key,
                &row.artist,
                &row.title,
            )
            .await
        }
        "listenbrainz" => {
            let Some(mbid) = row.mbid.as_deref() else {
                return Err(anyhow!("ListenBrainz love requires a recording MBID"));
            };
            let (http, token) = {
                let guard = state.read().await;
                let token = guard.db.with_conn(|conn| {
                    services::listenbrainz::load_token(conn, &guard.master_key)?
                        .ok_or_else(|| anyhow!("ListenBrainz token missing"))
                })?;
                (guard.http_client.clone(), token)
            };
            services::listenbrainz::love_recording(&http, &token, mbid).await
        }
        other => Err(anyhow!("unknown feedback provider {other}")),
    }
}

fn finish_feedback_row(conn: &Connection, row: &FeedbackRow, result: Result<()>) -> Result<()> {
    let now = unix_now();
    match result {
        Ok(()) => {
            conn.execute(
                "UPDATE provider_feedback_outbox SET status = 'sent', updated_at = ?1, last_error = NULL WHERE id = ?2",
                params![now, row.id],
            )?;
        }
        Err(error) => {
            let attempts = row.attempts + 1;
            let failed = attempts >= MAX_ATTEMPTS;
            conn.execute(
                "UPDATE provider_feedback_outbox
                    SET status = ?1, attempts = ?2, next_attempt_at = ?3, last_error = ?4, updated_at = ?5
                  WHERE id = ?6",
                params![
                    if failed { "failed" } else { "pending" },
                    attempts,
                    now + retry_delay_secs(attempts),
                    error.to_string(),
                    now,
                    row.id
                ],
            )?;
        }
    }
    Ok(())
}

pub fn outbox_status(conn: &Connection) -> Result<(i64, i64)> {
    let pending = conn.query_row(
        "SELECT COUNT(*) FROM scrobble_outbox WHERE status IN ('pending', 'processing')",
        [],
        |row| row.get(0),
    )?;
    let failed = conn.query_row(
        "SELECT COUNT(*) FROM scrobble_outbox WHERE status = 'failed'",
        [],
        |row| row.get(0),
    )?;
    Ok((pending, failed))
}

pub fn recent_eligible_listens(conn: &Connection, days: i64) -> Result<Vec<ScrobblePayload>> {
    let mut stmt = conn.prepare(
        "SELECT lh.track_id, a.name, t.title, al.title, t.duration_ms,
                lh.duration_listened_ms, strftime('%s', lh.started_at)
           FROM listen_history lh
           JOIN tracks t ON t.id = lh.track_id
           LEFT JOIN artists a ON a.id = t.artist_id
           LEFT JOIN albums al ON al.id = t.album_id
          WHERE lh.started_at >= datetime('now', ?1)
            AND t.title IS NOT NULL
            AND a.name IS NOT NULL
          ORDER BY lh.started_at ASC",
    )?;
    let since = format!("-{} days", days.max(1));
    let rows = stmt
        .query_map(params![since], |row| {
            Ok(ScrobblePayload {
                track_id: row.get(0)?,
                artist: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                title: row.get(2)?,
                album: row.get(3)?,
                duration_ms: row.get(4)?,
                listened_ms: row.get(5)?,
                started_at_unix: row
                    .get::<_, Option<String>>(6)?
                    .and_then(|s| s.parse::<i64>().ok()),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter(|p| {
            p.has_metadata()
                && p.duration_ms
                    .zip(p.listened_ms)
                    .is_some_and(|(d, l)| is_eligible_for_completed_scrobble(d, l))
        })
        .collect())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE scrobble_outbox (
                id INTEGER PRIMARY KEY,
                provider TEXT NOT NULL,
                kind TEXT NOT NULL,
                track_id INTEGER,
                artist TEXT NOT NULL,
                title TEXT NOT NULL,
                album TEXT,
                duration_ms INTEGER,
                listened_ms INTEGER,
                started_at_unix INTEGER,
                status TEXT NOT NULL DEFAULT 'pending',
                attempts INTEGER NOT NULL DEFAULT 0,
                next_attempt_at INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                UNIQUE(provider, kind, track_id, started_at_unix)
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn scrobble_completed_eligibility_uses_provider_threshold() {
        assert!(!is_eligible_for_completed_scrobble(29_000, 29_000));
        assert!(is_eligible_for_completed_scrobble(60_000, 31_000));
        assert!(is_eligible_for_completed_scrobble(600_000, 240_000));
        assert!(!is_eligible_for_completed_scrobble(600_000, 239_999));
    }

    #[test]
    fn scrobble_outbox_insert_is_idempotent_for_same_provider_listen() {
        let conn = conn();
        let payload = ScrobblePayload {
            track_id: Some(1),
            artist: "A".to_string(),
            title: "T".to_string(),
            album: None,
            duration_ms: Some(60_000),
            listened_ms: Some(31_000),
            started_at_unix: Some(100),
        };
        let first =
            insert_scrobble_outbox(&conn, "lastfm", "completed", &payload, 100, 100).unwrap();
        let second =
            insert_scrobble_outbox(&conn, "lastfm", "completed", &payload, 100, 100).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM scrobble_outbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(first, 1);
        assert_eq!(second, 0);
        assert_eq!(count, 1);
    }

    #[test]
    fn scrobble_outbox_retry_marks_pending_before_max_attempts() {
        let conn = conn();
        let row = OutboxRow {
            id: 1,
            provider: "lastfm".to_string(),
            kind: "completed".to_string(),
            _track_id: Some(1),
            artist: "A".to_string(),
            title: "T".to_string(),
            album: None,
            duration_ms: Some(60_000),
            _listened_ms: Some(31_000),
            started_at_unix: Some(100),
            attempts: 1,
        };
        conn.execute(
            "INSERT INTO scrobble_outbox (id, provider, kind, track_id, artist, title, duration_ms, listened_ms, started_at_unix)
             VALUES (1, 'lastfm', 'completed', 1, 'A', 'T', 60000, 31000, 100)",
            [],
        )
        .unwrap();

        finish_row(&conn, &row, Err(anyhow!("temporary provider failure"))).unwrap();

        let (status, attempts, last_error): (String, i64, String) = conn
            .query_row(
                "SELECT status, attempts, last_error FROM scrobble_outbox WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(attempts, 2);
        assert!(last_error.contains("temporary provider failure"));
    }
}
