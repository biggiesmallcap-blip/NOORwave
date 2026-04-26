pub mod models;
pub mod queries;
pub mod schema;

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Performance settings
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = -64000;",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn run_migrations(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        schema::run_migrations(&conn)
    }

    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }

    /// Seed the genres table from the embedded taxonomy and clean up orphan
    /// auto-grown genres from previous Last.fm enrichment runs.
    ///
    /// Idempotent: safe to call on every startup. Parents are inserted before
    /// children so FK references resolve correctly in a single pass.
    pub fn seed_genres_from_taxonomy(&self) -> Result<()> {
        use crate::genre::builder::embedded_builder;
        use crate::genre::mappings::slugify;

        let catalog = embedded_builder().catalog();
        let all_names = catalog.canonical_names().to_vec();

        self.with_conn(|conn| {
            // Build (name, slug, parent_name) sorted by path depth so parents
            // are always inserted before their children.
            let mut entries: Vec<(String, String, Option<String>)> = all_names
                .iter()
                .filter_map(|name| {
                    let path = catalog.path_for(name)?.to_vec();
                    let parent = if path.len() >= 2 {
                        path.get(path.len() - 2).cloned()
                    } else {
                        None
                    };
                    Some((name.clone(), slugify(name), parent))
                })
                .collect();
            entries.sort_by_key(|(_, _, parent)| if parent.is_none() { 0usize } else { 1 });

            let mut seeded = 0usize;
            let mut fixed = 0usize;

            for (name, slug, parent_name) in &entries {
                let parent_id: Option<i64> = parent_name.as_deref().and_then(|p| {
                    conn.query_row(
                        "SELECT id FROM genres WHERE name = ?1",
                        [p],
                        |row| row.get(0),
                    )
                    .ok()
                });

                let rows = conn.execute(
                    "INSERT OR IGNORE INTO genres (name, slug, parent_id) VALUES (?1, ?2, ?3)",
                    rusqlite::params![name, slug, parent_id],
                )?;
                if rows > 0 {
                    seeded += 1;
                }

                // Fix genres that exist but had the wrong (or missing) parent_id.
                if let Some(pid) = parent_id {
                    let fixed_rows = conn.execute(
                        "UPDATE genres SET parent_id = ?1, slug = ?2 WHERE name = ?3 AND (parent_id IS NULL OR parent_id != ?1)",
                        rusqlite::params![pid, slug, name],
                    )?;
                    fixed += fixed_rows;
                }
            }

            info!("Taxonomy seed: {} inserted, {} parent_id fixes.", seeded, fixed);

            // Collect taxonomy name set for orphan detection.
            let taxonomy_names: std::collections::HashSet<&str> =
                entries.iter().map(|(n, _, _)| n.as_str()).collect();

            // Root-level taxonomy genres that legitimately have parent_id = NULL.
            let root_names: std::collections::HashSet<&str> = entries
                .iter()
                .filter(|(_, _, parent)| parent.is_none())
                .map(|(n, _, _)| n.as_str())
                .collect();

            // Find genres with parent_id = NULL that are not taxonomy roots
            // (these are orphan auto-grown rows from old Last.fm enrichment).
            let orphan_candidates: Vec<(i64, String)> = {
                let mut stmt =
                    conn.prepare("SELECT id, name FROM genres WHERE parent_id IS NULL")?;
                stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
                    .filter_map(|r| r.ok())
                    .filter(|(_, name)| !root_names.contains(name.as_str()))
                    .collect()
            };

            let mut deleted = 0usize;
            for (genre_id, name) in &orphan_candidates {
                if taxonomy_names.contains(name.as_str()) {
                    continue;
                }
                // Keep if there are non-lastfm associations (e.g. Tidal/MB gave this genre).
                let has_real_assoc: bool = conn
                    .query_row(
                        "SELECT COUNT(*) FROM track_genres WHERE genre_id = ?1 AND source != 'lastfm'",
                        [genre_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                    > 0;

                if !has_real_assoc {
                    conn.execute("DELETE FROM track_genres WHERE genre_id = ?1", [genre_id])?;
                    conn.execute("DELETE FROM genres WHERE id = ?1", [genre_id])?;
                    deleted += 1;
                }
            }

            if deleted > 0 {
                info!("Taxonomy seed: removed {} orphan genres.", deleted);
            }

            Ok(())
        })
    }
}
