//! Cache layer for upstream TIDAL search results.
//!
//! Mirrors `services/sportify/cache.rs::{get_search, put_search}`. We cache
//! the parsed `TidalSearchCatalog` only — local-library enrichment
//! (`in_library`, `local_id`) re-runs on every read so newly-added tracks
//! show the correct badge without waiting for the cache to expire.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use super::client::TidalSearchCatalog;

const DEFAULT_TTL_SECS: i64 = 6 * 60 * 60; // 6 hours

#[derive(Debug, Clone, Copy)]
pub struct TidalSearchCacheConfig {
    pub ttl_secs: i64,
}

impl Default for TidalSearchCacheConfig {
    fn default() -> Self {
        Self {
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }
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

fn hash_query(query: &str, limit: i32, offset: i32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(query.trim().to_lowercase().as_bytes());
    hasher.update(format!("|{}|{}", limit, offset).as_bytes());
    hex::encode(hasher.finalize())
}

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

pub fn get_search(
    conn: &Connection,
    cfg: &TidalSearchCacheConfig,
    query: &str,
    limit: i32,
    offset: i32,
) -> Result<Option<TidalSearchCatalog>> {
    let key = hash_query(query, limit, offset);
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT payload, fetched_at FROM tidal_search_cache WHERE query_hash = ?1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("read tidal_search_cache")?;

    match row {
        Some((json, fetched_at)) if fresh(fetched_at, cfg.ttl_secs) => {
            let parsed: TidalSearchCatalog =
                serde_json::from_str(&json).context("decode cached tidal search payload")?;
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

pub fn put_search(
    conn: &Connection,
    query: &str,
    limit: i32,
    offset: i32,
    payload: &TidalSearchCatalog,
) -> Result<()> {
    let key = hash_query(query, limit, offset);
    let json = serde_json::to_string(payload).context("serialize tidal search results")?;
    conn.execute(
        "INSERT INTO tidal_search_cache (query_hash, payload, fetched_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(query_hash) DO UPDATE SET
            payload = excluded.payload,
            fetched_at = excluded.fetched_at",
        params![key, json, now_secs()],
    )
    .context("write tidal_search_cache")?;
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
    fn search_cache_round_trip() {
        let conn = open_test_db();
        let cfg = TidalSearchCacheConfig::default();
        let payload = TidalSearchCatalog::default();
        put_search(&conn, "daft punk", 10, 0, &payload).unwrap();
        assert!(
            get_search(&conn, &cfg, "daft punk", 10, 0)
                .unwrap()
                .is_some()
        );
        // Casing/trim-insensitive.
        assert!(
            get_search(&conn, &cfg, " Daft Punk ", 10, 0)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn search_cache_expires() {
        let conn = open_test_db();
        let cfg = TidalSearchCacheConfig { ttl_secs: 0 };
        let payload = TidalSearchCatalog::default();
        put_search(&conn, "abc", 10, 0, &payload).unwrap();
        assert!(get_search(&conn, &cfg, "abc", 10, 0).unwrap().is_none());
    }

    #[test]
    fn search_cache_distinguishes_offset() {
        let conn = open_test_db();
        let cfg = TidalSearchCacheConfig::default();
        let payload = TidalSearchCatalog::default();
        put_search(&conn, "abc", 20, 0, &payload).unwrap();
        assert!(get_search(&conn, &cfg, "abc", 20, 0).unwrap().is_some());
        assert!(get_search(&conn, &cfg, "abc", 20, 20).unwrap().is_none());
    }
}
