//! Lowercased-name to artist_id resolver.
//!
//! Several taste-aware code paths need to look up `artist_id` from a string
//! (last.fm hits, discovery candidates, anywhere `TrackSimilarityResult` /
//! `DiscoveryRadioResult` lost the id during projection). The resolver
//! loads the entire `artists` table once and answers lookups against an
//! in-memory `HashMap<lowercased_name, id>`.
//!
//! Phase 2a callers build one per request and drop it; the artists table
//! is small enough that the load is cheap. Phase 2b can lift to a
//! process-wide cache if call rate justifies the invalidation surface.
//!
//! Miss policy: `lookup` returns `Option<i64>`. `None` means "unknown
//! artist". Callers decide what to do — there is no sentinel id and no
//! silent fallback.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Default)]
pub struct ArtistResolver {
    by_lowercased_name: HashMap<String, i64>,
    /// Count of name collisions observed at load time (different ids,
    /// same lowercased name). Exposed so callers can debug-log unusual
    /// counts without reaching into the implementation.
    collision_count: usize,
}

impl ArtistResolver {
    pub fn load(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare("SELECT id, name FROM artists")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut by_lowercased_name: HashMap<String, i64> = HashMap::new();
        let mut collision_count = 0usize;
        for row in rows {
            let (id, name) = row?;
            let key = name.trim().to_ascii_lowercase();
            if key.is_empty() {
                continue;
            }
            // Collision policy: smallest id wins. Stable across runs and
            // biased toward the older record, which is usually the
            // canonical one when duplicates pile up.
            match by_lowercased_name.get(&key).copied() {
                Some(existing) if existing <= id => {
                    collision_count += 1;
                }
                Some(_) => {
                    by_lowercased_name.insert(key, id);
                    collision_count += 1;
                }
                None => {
                    by_lowercased_name.insert(key, id);
                }
            }
        }

        Ok(Self {
            by_lowercased_name,
            collision_count,
        })
    }

    /// Returns `None` for unknown names. Caller decides how to handle
    /// the miss; there is no sentinel id.
    pub fn lookup(&self, name: &str) -> Option<i64> {
        let key = name.trim().to_ascii_lowercase();
        if key.is_empty() {
            return None;
        }
        self.by_lowercased_name.get(&key).copied()
    }

    /// Number of name collisions seen during `load`. Useful for a debug
    /// log line so unusual collision counts get noticed.
    #[allow(dead_code)]
    pub fn collision_count(&self) -> usize {
        self.collision_count
    }

    /// Number of unique lowercased-name entries in the resolver.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.by_lowercased_name.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with_artists(rows: &[(i64, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
        )
        .unwrap();
        for (id, name) in rows {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (?1, ?2)",
                rusqlite::params![id, name],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn lookup_resolves_exact_lowercased_match() {
        let conn = conn_with_artists(&[(1, "Boards of Canada"), (2, "Aphex Twin")]);
        let resolver = ArtistResolver::load(&conn).unwrap();
        assert_eq!(resolver.lookup("boards of canada"), Some(1));
        assert_eq!(resolver.lookup("APHEX TWIN"), Some(2));
    }

    #[test]
    fn lookup_trims_whitespace() {
        let conn = conn_with_artists(&[(1, "Sigur Rós")]);
        let resolver = ArtistResolver::load(&conn).unwrap();
        assert_eq!(resolver.lookup("  sigur rós  "), Some(1));
    }

    #[test]
    fn lookup_returns_none_for_unknown_name() {
        let conn = conn_with_artists(&[(1, "Boards of Canada")]);
        let resolver = ArtistResolver::load(&conn).unwrap();
        assert_eq!(resolver.lookup("Plaid"), None);
    }

    #[test]
    fn lookup_returns_none_for_empty_name() {
        let conn = conn_with_artists(&[(1, "Boards of Canada")]);
        let resolver = ArtistResolver::load(&conn).unwrap();
        assert_eq!(resolver.lookup(""), None);
        assert_eq!(resolver.lookup("   "), None);
    }

    #[test]
    fn collision_keeps_smallest_id() {
        let conn = conn_with_artists(&[
            (5, "Plaid"),
            (3, "plaid"), // collision: same lowercased name, smaller id wins
            (9, "PLAID"), // collision again
        ]);
        let resolver = ArtistResolver::load(&conn).unwrap();
        assert_eq!(resolver.lookup("plaid"), Some(3));
        assert_eq!(resolver.collision_count(), 2);
    }

    #[test]
    fn empty_artist_names_are_skipped() {
        let conn = conn_with_artists(&[(1, ""), (2, "Real Artist"), (3, "   ")]);
        let resolver = ArtistResolver::load(&conn).unwrap();
        assert_eq!(resolver.len(), 1);
        assert_eq!(resolver.lookup("real artist"), Some(2));
    }
}
