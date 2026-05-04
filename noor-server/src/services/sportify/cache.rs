// Phase 1 scaffolding: every cache fn here is wired up by phases 2-4
// (resolver, route handlers, bulk resolve). The dead-code allow comes off
// once those land.
#![allow(dead_code)]

//! SQLite-backed cache for Sportify metadata + Spotify→TIDAL resolutions.
//!
//! Tables created by `MIGRATION_031`:
//!   sportify_track_meta, sportify_album_meta,
//!   sportify_artist_meta, sportify_playlist_meta — entity caches
//!   sportify_track_map     — successful Spotify→TIDAL mappings
//!   sportify_unresolved    — negative cache (gives up to N attempts)
//!   sportify_search_cache  — search-result cache keyed by query hash

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use super::client::SportifySearchKind;
use super::models::{
    SportifyAlbum, SportifyArtist, SportifyPlaylist, SportifySearchResults, SportifyTrack,
};

const DEFAULT_TTL_SECS: i64 = 30 * 24 * 60 * 60; // 30 days
const DEFAULT_RESOLVE_TTL_SECS: i64 = 30 * 24 * 60 * 60;
const DEFAULT_RETRY_AFTER_SECS: i64 = 7 * 24 * 60 * 60;
pub const DEFAULT_EAGER_N: usize = 10;
pub const DEFAULT_BULK_CONCURRENCY: usize = 6;

/// Eager-first-N + lazy-rest tunables for the discovery list endpoints.
#[derive(Debug, Clone, Copy)]
pub struct SportifyResolveConfig {
    /// How many pending tracks the request resolves synchronously before
    /// responding. The rest are spawned into a background task.
    pub eager_n: usize,
    /// Max concurrent TIDAL searches when resolving a batch.
    pub bulk_concurrency: usize,
}

impl Default for SportifyResolveConfig {
    fn default() -> Self {
        Self {
            eager_n: DEFAULT_EAGER_N,
            bulk_concurrency: DEFAULT_BULK_CONCURRENCY,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SportifyCacheConfig {
    pub meta_ttl_secs: i64,
    pub resolve_ttl_secs: i64,
    pub unresolved_retry_after_secs: i64,
}

impl Default for SportifyCacheConfig {
    fn default() -> Self {
        Self {
            meta_ttl_secs: DEFAULT_TTL_SECS,
            resolve_ttl_secs: DEFAULT_RESOLVE_TTL_SECS,
            unresolved_retry_after_secs: DEFAULT_RETRY_AFTER_SECS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTidalResolution {
    pub spotify_track_id: String,
    pub tidal_track_id: i64,
    pub confidence: f64,
    pub match_reason: Option<String>,
    pub resolved_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedRecord {
    pub spotify_track_id: String,
    pub last_attempt_at: i64,
    pub attempts: i64,
    pub reason: Option<String>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn fresh(fetched_at: i64, ttl: i64) -> bool {
    now_secs().saturating_sub(fetched_at) < ttl
}

fn hash_query(query: &str, kind: SportifySearchKind, limit: u32, offset: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(query.trim().to_lowercase().as_bytes());
    hasher.update(format!("|{}|{}", limit, offset).as_bytes());
    hex::encode(hasher.finalize())
}

// `hex` is not in the existing dep tree; we encode by hand to avoid a new dep.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let bytes = bytes.as_ref();
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
        out
    }
}

fn read_meta<T: DeserializeOwned>(
    conn: &Connection,
    table: &str,
    id_column: &str,
    id: &str,
    ttl: i64,
) -> Result<Option<T>> {
    let sql = format!(
        "SELECT payload, fetched_at FROM {} WHERE {} = ?1",
        table, id_column
    );
    let row: Option<(String, i64)> = conn
        .query_row(&sql, params![id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
        .with_context(|| format!("read {}", table))?;

    match row {
        Some((json, fetched_at)) if fresh(fetched_at, ttl) => {
            let parsed = serde_json::from_str(&json)
                .with_context(|| format!("decode cached {} payload", table))?;
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

fn write_meta<T: Serialize>(
    conn: &Connection,
    table: &str,
    id_column: &str,
    id: &str,
    payload: &T,
) -> Result<()> {
    let json = serde_json::to_string(payload)
        .with_context(|| format!("serialize {} payload", table))?;
    let sql = format!(
        "INSERT INTO {tbl} ({col}, payload, fetched_at) VALUES (?1, ?2, ?3)
         ON CONFLICT({col}) DO UPDATE SET payload = excluded.payload, fetched_at = excluded.fetched_at",
        tbl = table,
        col = id_column,
    );
    conn.execute(&sql, params![id, json, now_secs()])
        .with_context(|| format!("write {}", table))?;
    Ok(())
}

pub fn get_track_meta(
    conn: &Connection,
    cfg: &SportifyCacheConfig,
    spotify_id: &str,
) -> Result<Option<SportifyTrack>> {
    read_meta(
        conn,
        "sportify_track_meta",
        "spotify_track_id",
        spotify_id,
        cfg.meta_ttl_secs,
    )
}

pub fn put_track_meta(conn: &Connection, spotify_id: &str, payload: &SportifyTrack) -> Result<()> {
    write_meta(conn, "sportify_track_meta", "spotify_track_id", spotify_id, payload)
}

pub fn get_album_meta(
    conn: &Connection,
    cfg: &SportifyCacheConfig,
    spotify_id: &str,
) -> Result<Option<SportifyAlbum>> {
    read_meta(
        conn,
        "sportify_album_meta",
        "spotify_album_id",
        spotify_id,
        cfg.meta_ttl_secs,
    )
}

pub fn put_album_meta(conn: &Connection, spotify_id: &str, payload: &SportifyAlbum) -> Result<()> {
    write_meta(conn, "sportify_album_meta", "spotify_album_id", spotify_id, payload)
}

pub fn get_artist_meta(
    conn: &Connection,
    cfg: &SportifyCacheConfig,
    spotify_id: &str,
) -> Result<Option<SportifyArtist>> {
    read_meta(
        conn,
        "sportify_artist_meta",
        "spotify_artist_id",
        spotify_id,
        cfg.meta_ttl_secs,
    )
}

pub fn put_artist_meta(conn: &Connection, spotify_id: &str, payload: &SportifyArtist) -> Result<()> {
    write_meta(conn, "sportify_artist_meta", "spotify_artist_id", spotify_id, payload)
}

pub fn get_playlist_meta(
    conn: &Connection,
    cfg: &SportifyCacheConfig,
    spotify_id: &str,
) -> Result<Option<SportifyPlaylist>> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT payload, fetched_at FROM sportify_playlist_meta
             WHERE spotify_playlist_id = ?1",
            params![spotify_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("read sportify_playlist_meta")?;

    match row {
        Some((json, fetched_at)) if fresh(fetched_at, cfg.meta_ttl_secs) => {
            let parsed: SportifyPlaylist =
                serde_json::from_str(&json).context("decode cached playlist payload")?;
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

pub fn put_playlist_meta(
    conn: &Connection,
    spotify_id: &str,
    payload: &SportifyPlaylist,
) -> Result<()> {
    let json = serde_json::to_string(payload).context("serialize playlist")?;
    let snapshot = payload.snapshot_id.clone();
    conn.execute(
        "INSERT INTO sportify_playlist_meta
            (spotify_playlist_id, payload, snapshot_id, fetched_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(spotify_playlist_id) DO UPDATE SET
            payload = excluded.payload,
            snapshot_id = excluded.snapshot_id,
            fetched_at = excluded.fetched_at",
        params![spotify_id, json, snapshot, now_secs()],
    )
    .context("write sportify_playlist_meta")?;
    Ok(())
}

pub fn get_search(
    conn: &Connection,
    cfg: &SportifyCacheConfig,
    query: &str,
    kind: SportifySearchKind,
    limit: u32,
    offset: u32,
) -> Result<Option<SportifySearchResults>> {
    let key = hash_query(query, kind, limit, offset);
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT payload, fetched_at FROM sportify_search_cache WHERE query_hash = ?1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("read sportify_search_cache")?;

    match row {
        Some((json, fetched_at)) if fresh(fetched_at, cfg.meta_ttl_secs) => {
            let parsed: SportifySearchResults =
                serde_json::from_str(&json).context("decode cached search payload")?;
            // A previous playlist parser could cache an empty first page even
            // though Sportify returned rows in an unhandled shape. Treat that
            // specific cache entry as stale so search can recover immediately
            // after parser fixes, while preserving empty later pages.
            if matches!(kind, SportifySearchKind::Playlist)
                && offset == 0
                && parsed.playlists.is_empty()
            {
                Ok(None)
            } else {
                Ok(Some(parsed))
            }
        }
        _ => Ok(None),
    }
}

pub fn put_search(
    conn: &Connection,
    query: &str,
    kind: SportifySearchKind,
    limit: u32,
    offset: u32,
    payload: &SportifySearchResults,
) -> Result<()> {
    let key = hash_query(query, kind, limit, offset);
    let json = serde_json::to_string(payload).context("serialize search results")?;
    conn.execute(
        "INSERT INTO sportify_search_cache (query_hash, kind, payload, fetched_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(query_hash) DO UPDATE SET
            kind = excluded.kind,
            payload = excluded.payload,
            fetched_at = excluded.fetched_at",
        params![key, kind.as_str(), json, now_secs()],
    )
    .context("write sportify_search_cache")?;
    Ok(())
}

// ─── Resolution map ──────────────────────────────────────────

pub fn get_tidal_resolution(
    conn: &Connection,
    cfg: &SportifyCacheConfig,
    spotify_track_id: &str,
) -> Result<Option<CachedTidalResolution>> {
    let row: Option<(String, i64, f64, Option<String>, i64)> = conn
        .query_row(
            "SELECT spotify_track_id, tidal_track_id, confidence, match_reason, resolved_at
             FROM sportify_track_map WHERE spotify_track_id = ?1",
            params![spotify_track_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .context("read sportify_track_map")?;

    match row {
        Some((id, tidal, conf, reason, at)) if fresh(at, cfg.resolve_ttl_secs) => {
            Ok(Some(CachedTidalResolution {
                spotify_track_id: id,
                tidal_track_id: tidal,
                confidence: conf,
                match_reason: reason,
                resolved_at: at,
            }))
        }
        _ => Ok(None),
    }
}

pub fn put_tidal_resolution(
    conn: &Connection,
    spotify_track_id: &str,
    tidal_track_id: i64,
    confidence: f64,
    match_reason: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sportify_track_map
            (spotify_track_id, tidal_track_id, confidence, match_reason, resolved_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(spotify_track_id) DO UPDATE SET
            tidal_track_id = excluded.tidal_track_id,
            confidence = excluded.confidence,
            match_reason = excluded.match_reason,
            resolved_at = excluded.resolved_at",
        params![
            spotify_track_id,
            tidal_track_id,
            confidence,
            match_reason,
            now_secs()
        ],
    )
    .context("write sportify_track_map")?;
    // A successful resolution invalidates any prior unresolved record.
    conn.execute(
        "DELETE FROM sportify_unresolved WHERE spotify_track_id = ?1",
        params![spotify_track_id],
    )
    .ok();
    Ok(())
}

pub fn get_unresolved(
    conn: &Connection,
    spotify_track_id: &str,
) -> Result<Option<UnresolvedRecord>> {
    let row: Option<(String, i64, i64, Option<String>)> = conn
        .query_row(
            "SELECT spotify_track_id, last_attempt_at, attempts, reason
             FROM sportify_unresolved WHERE spotify_track_id = ?1",
            params![spotify_track_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .context("read sportify_unresolved")?;

    Ok(row.map(|(id, at, attempts, reason)| UnresolvedRecord {
        spotify_track_id: id,
        last_attempt_at: at,
        attempts,
        reason,
    }))
}

/// True if we should skip a fresh attempt (the row was tried recently).
pub fn unresolved_is_cold(record: &UnresolvedRecord, cfg: &SportifyCacheConfig) -> bool {
    now_secs().saturating_sub(record.last_attempt_at) < cfg.unresolved_retry_after_secs
}

pub fn put_unresolved(
    conn: &Connection,
    spotify_track_id: &str,
    reason: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sportify_unresolved
            (spotify_track_id, last_attempt_at, attempts, reason)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(spotify_track_id) DO UPDATE SET
            last_attempt_at = excluded.last_attempt_at,
            attempts = sportify_unresolved.attempts + 1,
            reason = excluded.reason",
        params![spotify_track_id, now_secs(), reason],
    )
    .context("write sportify_unresolved")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::run_migrations;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&conn).expect("apply migrations");
        conn
    }

    #[test]
    fn track_meta_round_trip() {
        let conn = open_test_db();
        let cfg = SportifyCacheConfig::default();
        let track = SportifyTrack {
            id: Some("abc".into()),
            name: Some("Test".into()),
            ..Default::default()
        };
        put_track_meta(&conn, "abc", &track).unwrap();
        let got = get_track_meta(&conn, &cfg, "abc").unwrap().unwrap();
        assert_eq!(got.id.as_deref(), Some("abc"));
        assert_eq!(got.name.as_deref(), Some("Test"));
    }

    #[test]
    fn track_meta_expires() {
        let conn = open_test_db();
        let cfg = SportifyCacheConfig {
            meta_ttl_secs: 0,
            ..Default::default()
        };
        let track = SportifyTrack {
            id: Some("abc".into()),
            ..Default::default()
        };
        put_track_meta(&conn, "abc", &track).unwrap();
        assert!(get_track_meta(&conn, &cfg, "abc").unwrap().is_none());
    }

    #[test]
    fn resolution_round_trip_clears_unresolved() {
        let conn = open_test_db();
        let cfg = SportifyCacheConfig::default();
        put_unresolved(&conn, "abc", Some("first try")).unwrap();
        assert!(get_unresolved(&conn, "abc").unwrap().is_some());

        put_tidal_resolution(&conn, "abc", 12345, 0.95, Some("title+artist")).unwrap();
        let got = get_tidal_resolution(&conn, &cfg, "abc").unwrap().unwrap();
        assert_eq!(got.tidal_track_id, 12345);
        assert!((got.confidence - 0.95).abs() < 1e-9);
        assert!(get_unresolved(&conn, "abc").unwrap().is_none());
    }

    #[test]
    fn unresolved_increments_attempts() {
        let conn = open_test_db();
        put_unresolved(&conn, "abc", Some("a")).unwrap();
        put_unresolved(&conn, "abc", Some("b")).unwrap();
        let rec = get_unresolved(&conn, "abc").unwrap().unwrap();
        assert_eq!(rec.attempts, 2);
        assert_eq!(rec.reason.as_deref(), Some("b"));
    }

    #[test]
    fn search_cache_round_trip() {
        let conn = open_test_db();
        let cfg = SportifyCacheConfig::default();
        let payload = SportifySearchResults::default();
        put_search(&conn, "daft punk", SportifySearchKind::Track, 10, 0, &payload).unwrap();
        assert!(
            get_search(&conn, &cfg, "daft punk", SportifySearchKind::Track, 10, 0)
                .unwrap()
                .is_some()
        );
        // Casing/trim-insensitive.
        assert!(
            get_search(&conn, &cfg, " Daft Punk ", SportifySearchKind::Track, 10, 0)
                .unwrap()
                .is_some()
        );
    }
}
