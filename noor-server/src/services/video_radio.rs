//! Reusable video candidates and bounded inputs for a replenishing video mix.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::video_sets::{AnchorArtist, VideoCandidate, VideoSetItem};
use crate::SharedState;
use crate::services::tidal::client::TidalClient;

const ARTIST_CACHE_DAYS: i64 = 14;
const GENRE_CACHE_DAYS: i64 = 14;
pub const UNFAMILIAR_ARTISTS_FOR_HEALTHY_QUEUE: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceLane {
    Seed,
    Direct,
    Genre,
    Library,
    Bridge,
}

impl SourceLane {
    fn score(self) -> i32 {
        match self {
            Self::Seed => 0,
            Self::Direct => 2,
            Self::Genre => 4,
            Self::Library => 6,
            Self::Bridge => 8,
        }
    }
    pub fn fetch_priority(self) -> u8 {
        match self {
            Self::Seed => 0,
            Self::Bridge => 1,
            Self::Direct => 2,
            Self::Genre => 3,
            Self::Library => 4,
        }
    }
    pub fn is_close(self) -> bool {
        matches!(self, Self::Seed | Self::Direct | Self::Genre)
    }
    fn seeded_priority(self) -> u8 {
        match self {
            Self::Seed => 0,
            Self::Direct => 1,
            Self::Genre => 2,
            Self::Library => 3,
            Self::Bridge => 4,
        }
    }
}

pub fn unfamiliar_artist_count(
    items: &[VideoSetItem],
    familiar: &HashSet<i64>,
    seed: Option<i64>,
) -> usize {
    items
        .iter()
        .filter_map(|item| item.artist_id)
        .filter(|id| Some(*id) != seed && !familiar.contains(id))
        .collect::<HashSet<_>>()
        .len()
}

pub fn queue_needs_discovery(
    total: usize,
    unfamiliar_artists: usize,
    relationship_due: bool,
    bridge_due: bool,
) -> bool {
    total < 8
        || unfamiliar_artists < UNFAMILIAR_ARTISTS_FOR_HEALTHY_QUEUE
        || relationship_due
        || bridge_due
}
const WARM_SEEDS_PER_PASS: usize = 4;
const WARM_PENDING_SEEDS_PER_PASS: usize = 2;
const WARM_RELATED_ARTISTS_PER_SEED: usize = 2;
static LIKED_GRAPH_WARM_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_LIKED_GRAPH_WARM: Mutex<Option<std::time::Instant>> = Mutex::new(None);
const LIKED_ARTIST_CTES: &str = "liked_sources AS (
    SELECT a.tidal_id AS artist_id, a.name AS artist_name
      FROM artists a JOIN tracks t ON t.artist_id = a.id AND t.is_favorite = 1
    UNION ALL
    SELECT a.tidal_id, a.name
      FROM artists a JOIN albums al ON al.artist_id = a.id AND al.is_favorite = 1
    UNION ALL
    SELECT CAST(json_extract(s.item_json, '$.artist_id') AS INTEGER),
           COALESCE(json_extract(s.item_json, '$.artist_name'), a.name, '')
      FROM saved_videos s
      LEFT JOIN artists a ON a.tidal_id = CAST(json_extract(s.item_json, '$.artist_id') AS INTEGER)
     WHERE json_type(s.item_json, '$.artist_id') = 'integer'
), liked AS (
    SELECT artist_id, MAX(artist_name) AS name, COUNT(*) AS affinity
      FROM liked_sources WHERE artist_id > 0 GROUP BY artist_id
)";

/// Prioritize liked artists that have never had a relationship scan. Each
/// pass takes only four; the 10-minute timer and shared relationship ledger
/// leave another four scans for interactive radio starts.
pub fn liked_seeds_needing_warm(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let sql = format!(
        "WITH {LIKED_ARTIST_CTES}
         SELECT liked.artist_id, liked.name FROM liked
         LEFT JOIN video_related_scans s ON s.seed_tidal_id = liked.artist_id
         LEFT JOIN video_related_retries r ON r.seed_tidal_id = liked.artist_id
         WHERE s.scanned_at IS NULL OR s.scanned_at < datetime('now', '-7 days')
            OR r.retry_at <= datetime('now')
         ORDER BY s.scanned_at IS NOT NULL, liked.affinity DESC, liked.name
         LIMIT 4"
    );
    let mut stmt = conn.prepare(&sql)?;
    Ok(stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn related_catalog_targets(conn: &Connection, seed_id: i64) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT r.related_tidal_id, MIN(r.name)
         FROM video_related_artists r
         LEFT JOIN video_artist_scans s ON s.artist_tidal_id = r.related_tidal_id
         WHERE r.seed_tidal_id = ?1 AND r.source IN ('tidal', 'lastfm')
           AND (s.scanned_at IS NULL OR s.scanned_at < datetime('now', '-14 days'))
         GROUP BY r.related_tidal_id
         ORDER BY MIN(CASE r.source WHEN 'tidal' THEN 0 WHEN 'lastfm' THEN 1 ELSE 2 END),
                  MIN(r.rowid)
         LIMIT 2",
    )?;
    Ok(stmt
        .query_map([seed_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Revisit already linked liked artists until their direct neighbors have
/// video catalogs too. The relationship scan itself remains on its 7-day TTL.
fn liked_seeds_with_pending_catalogs(conn: &Connection) -> Result<Vec<i64>> {
    let sql = format!(
        "WITH {LIKED_ARTIST_CTES}
         SELECT r.seed_tidal_id
         FROM video_related_artists r
         JOIN liked ON liked.artist_id = r.seed_tidal_id
         LEFT JOIN video_artist_scans s ON s.artist_tidal_id = r.related_tidal_id
         WHERE r.source IN ('tidal', 'lastfm')
           AND (s.scanned_at IS NULL OR s.scanned_at < datetime('now', '-14 days'))
         GROUP BY r.seed_tidal_id
         ORDER BY MAX(liked.affinity) DESC, r.seed_tidal_id
         LIMIT 8"
    );
    let mut stmt = conn.prepare(&sql)?;
    Ok(stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

async fn warm_artist_catalog(
    db: &crate::db::Database,
    client: &TidalClient,
    artist_id: i64,
    name: String,
) -> usize {
    if !db
        .with_conn(|conn| reserve_artist_scan(conn, artist_id))
        .unwrap_or(false)
    {
        return 0;
    }
    match client.get_artist_videos(artist_id, 50, 0).await {
        Ok(page) => {
            let anchor = AnchorArtist {
                tidal_id: artist_id,
                name,
                listens: 1,
                via: None,
            };
            let videos = page
                .items
                .iter()
                .map(VideoCandidate::from)
                .collect::<Vec<_>>();
            let count = videos.len();
            if let Err(error) =
                db.with_conn(|conn| cache_groups_without_prune(conn, &[(anchor, videos)]))
            {
                tracing::warn!(target: "noor.video_radio", artist_id, %error, "artist video cache failed");
                return 0;
            }
            count
        }
        Err(error) => {
            tracing::debug!(target: "noor.video_radio", artist_id, %error, "artist video fetch failed");
            0
        }
    }
}

async fn warm_related_catalogs(
    db: &crate::db::Database,
    client: &TidalClient,
    seed_id: i64,
) -> usize {
    let related = db
        .with_conn(|conn| related_catalog_targets(conn, seed_id))
        .unwrap_or_default();
    let mut indexed = 0;
    for (artist_id, name) in related.into_iter().take(WARM_RELATED_ARTISTS_PER_SEED) {
        indexed += warm_artist_catalog(db, client, artist_id, name).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    indexed
}

/// Quietly warm liked artists and direct neighbors. Liked-track artists reuse
/// their library scan; saved-video and favorite-album artists get one catalog
/// lookup here before the related catalog pass.
pub async fn warm_liked_graph_if_idle(state: SharedState) {
    let (db, http, tidal_http, tokens, library_scan_running) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
            s.tidal_tokens.clone(),
            s.library_video_scan_running.clone(),
        )
    };
    if tokens.is_none() || library_scan_running.load(Ordering::SeqCst) {
        return;
    }
    let Ok(mut last_warm) = LAST_LIKED_GRAPH_WARM.lock() else {
        return;
    };
    if last_warm.is_some_and(|started| started.elapsed() < Duration::from_secs(600))
        || LIKED_GRAPH_WARM_RUNNING.swap(true, Ordering::SeqCst)
    {
        return;
    }
    *last_warm = Some(std::time::Instant::now());
    drop(last_warm);
    tokio::spawn(async move {
        let tokens = tokens.expect("checked above");
        let client = TidalClient::with_http(tidal_http, tokens.access_token, tokens.country_code);
        let seeds = db.with_conn(liked_seeds_needing_warm).unwrap_or_default();
        let mut warmed = 0;
        let mut indexed = 0;
        let mut visited = HashSet::new();
        for (seed_id, seed_name) in seeds.into_iter().take(WARM_SEEDS_PER_PASS) {
            if !db
                .with_conn(|conn| reserve_related_scan(conn, seed_id))
                .unwrap_or(false)
            {
                continue;
            }
            if let Err(error) = refresh_video_relations(&db, http.clone(), &client, seed_id).await {
                tracing::debug!(target: "noor.video_radio", seed_id, %error, "liked artist relationship warm failed");
                continue;
            }
            warmed += 1;
            visited.insert(seed_id);
            indexed += warm_artist_catalog(&db, &client, seed_id, seed_name).await;
            indexed += warm_related_catalogs(&db, &client, seed_id).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let pending = db
            .with_conn(liked_seeds_with_pending_catalogs)
            .unwrap_or_default();
        let mut continued = 0;
        for seed_id in pending
            .into_iter()
            .filter(|id| !visited.contains(id))
            .take(WARM_PENDING_SEEDS_PER_PASS)
        {
            indexed += warm_related_catalogs(&db, &client, seed_id).await;
            continued += 1;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if warmed > 0 || continued > 0 {
            tracing::info!(target: "noor.video_radio", warmed, continued, indexed, "liked video graph warm complete");
        }
        if let Err(error) = db.with_conn(prune_catalog) {
            tracing::warn!(target: "noor.video_radio", %error, "could not prune video catalog");
        }
        LIKED_GRAPH_WARM_RUNNING.store(false, Ordering::SeqCst);
    });
}

pub fn cache_groups(
    conn: &Connection,
    groups: &[(AnchorArtist, Vec<VideoCandidate>)],
) -> Result<()> {
    cache_groups_without_prune(conn, groups)?;
    prune_catalog(conn)
}

/// The liked-video sweep may write thousands of artists in one pass. It prunes
/// once at the end instead of scanning the entire catalog after every artist.
pub fn cache_groups_without_prune(
    conn: &Connection,
    groups: &[(AnchorArtist, Vec<VideoCandidate>)],
) -> Result<()> {
    for (anchor, videos) in groups {
        if anchor.tidal_id > 0 {
            mark_artist_scanned(conn, anchor.tidal_id)?;
        }
        for video in videos {
            let artist_id = video
                .artist_id
                .or((anchor.tidal_id > 0).then_some(anchor.tidal_id));
            let mut normalized = video.clone();
            normalized.artist_id = artist_id;
            if normalized.artist_name.is_none() && anchor.tidal_id > 0 {
                normalized.artist_name = Some(anchor.name.clone());
            }
            conn.execute(
                "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(tidal_video_id) DO UPDATE SET
                   artist_tidal_id = excluded.artist_tidal_id,
                   artist_name = excluded.artist_name,
                   item_json = excluded.item_json,
                   fetched_at = datetime('now')",
                params![
                    video.tidal_id,
                    artist_id,
                    normalized.artist_name.as_deref(),
                    serde_json::to_string(&normalized)?,
                ],
            )?;
        }
    }
    Ok(())
}

pub fn prune_catalog(conn: &Connection) -> Result<()> {
    // The catalog is a bounded working index, not a permanent copy of TIDAL.
    conn.execute(
        "DELETE FROM video_catalog WHERE fetched_at < datetime('now', '-180 days')",
        [],
    )?;
    Ok(())
}

pub fn mark_artist_scanned(conn: &Connection, artist_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO video_artist_scans (artist_tidal_id) VALUES (?1)
         ON CONFLICT(artist_tidal_id) DO UPDATE SET scanned_at = datetime('now')",
        [artist_id],
    )?;
    Ok(())
}

pub fn artist_due(conn: &Connection, artist_id: i64) -> Result<bool> {
    let age: Option<i64> = conn
        .query_row(
            "SELECT CAST(julianday('now') - julianday(scanned_at) AS INTEGER)
         FROM video_artist_scans WHERE artist_tidal_id = ?1",
            [artist_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(age.is_none_or(|days| days >= ARTIST_CACHE_DAYS))
}

pub fn reserve_artist_scan(conn: &Connection, artist_id: i64) -> Result<bool> {
    if !artist_due(conn, artist_id)? {
        return Ok(false);
    }
    mark_artist_scanned(conn, artist_id)?;
    Ok(true)
}

pub fn related_due(conn: &Connection, artist_id: i64) -> Result<bool> {
    let due: bool = conn.query_row(
        "SELECT NOT EXISTS (
             SELECT 1 FROM video_related_scans
             WHERE seed_tidal_id = ?1 AND scanned_at > datetime('now', '-7 days')
         ) OR EXISTS (
             SELECT 1 FROM video_related_retries
             WHERE seed_tidal_id = ?1 AND retry_at <= datetime('now')
         )",
        [artist_id],
        |row| row.get(0),
    )?;
    if !due {
        return Ok(false);
    }
    let recent_attempt: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM video_related_attempts
          WHERE seed_tidal_id = ?1 AND attempted_at > datetime('now', '-1 hour'))",
        [artist_id],
        |row| row.get(0),
    )?;
    if recent_attempt {
        return Ok(false);
    }
    // Four background seeds can warm in a window while leaving another four
    // relationship scans for interactive artist radios.
    let recent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_related_attempts WHERE attempted_at >= datetime('now', '-10 minutes')",
        [], |row| row.get(0),
    )?;
    Ok(recent < 8)
}

/// Reserve a seed before contacting providers. Radio and the related row can
/// start together; the second caller must reuse the first pass instead of
/// making the same Last.fm and TIDAL requests again.
pub fn reserve_related_scan(conn: &Connection, artist_id: i64) -> Result<bool> {
    if !related_due(conn, artist_id)? {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO video_related_attempts (seed_tidal_id) VALUES (?1)
         ON CONFLICT(seed_tidal_id) DO UPDATE SET attempted_at = datetime('now')",
        [artist_id],
    )?;
    Ok(true)
}

/// A healthy station still checks one direct neighbor once an hour. The
/// relationship attempt ledger applies its separate per-artist/global limits.
pub fn bridge_due(conn: &Connection, seed_id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM video_related_artists
             WHERE seed_tidal_id = ?1 AND source IN ('tidal', 'lastfm'))
         AND NOT EXISTS (SELECT 1 FROM video_bridge_scans
             WHERE seed_tidal_id = ?1 AND scanned_at > datetime('now', '-1 hour'))",
        [seed_id],
        |row| row.get::<_, bool>(0),
    )?)
}

pub fn mark_bridge_scanned(conn: &Connection, seed_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO video_bridge_scans (seed_tidal_id) VALUES (?1)
         ON CONFLICT(seed_tidal_id) DO UPDATE SET scanned_at = datetime('now')",
        [seed_id],
    )?;
    Ok(())
}

#[cfg(test)]
pub fn store_related(
    conn: &Connection,
    seed_id: i64,
    related: &[(i64, String, &'static str)],
) -> Result<()> {
    conn.execute(
        "INSERT INTO video_related_scans (seed_tidal_id) VALUES (?1)
         ON CONFLICT(seed_tidal_id) DO UPDATE SET scanned_at = datetime('now')",
        [seed_id],
    )?;
    for (id, name, source) in related {
        if *id == seed_id || *id <= 0 {
            continue;
        }
        conn.execute(
            "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(seed_tidal_id, related_tidal_id, source) DO UPDATE SET
               name = excluded.name",
            params![seed_id, id, name, source],
        )?;
    }
    Ok(())
}

/// The related row, radio, and background warmer share the same bounded
/// relationship pass. A row opening first must not suppress Last.fm links.
pub async fn refresh_video_relations(
    db: &crate::db::Database,
    http: reqwest::Client,
    client: &TidalClient,
    seed_id: i64,
) -> Result<()> {
    // The ID is authoritative. Caller-supplied display text must never drive
    // Last.fm lookups for a different TIDAL artist.
    let mut name = db.with_conn(|conn| canonical_artist_name(conn, seed_id))?;
    if name.is_none() {
        name = tokio::time::timeout(Duration::from_secs(4), client.get_artist(seed_id))
            .await
            .ok()
            .and_then(Result::ok)
            .map(|artist| artist.name);
    }
    let lastfm = name
        .as_ref()
        .and_then(|_| crate::metadata::lastfm::LastFmClient::load(http, db));
    let has_lastfm = lastfm.is_some();
    let lastfm_lookup = async {
        let (Some(name), Some(lastfm)) = (name.as_deref(), lastfm) else {
            return (None, None);
        };
        let (similar, tags) = tokio::join!(
            tokio::time::timeout(
                std::time::Duration::from_secs(6),
                lastfm.artist_get_similar(name, 20)
            ),
            tokio::time::timeout(
                std::time::Duration::from_secs(6),
                lastfm.artist_top_tags(name)
            ),
        );
        let similar = similar.ok().and_then(Result::ok);
        let genres: Option<Vec<String>> = tags
            .ok()
            .and_then(Result::ok)
            .map(|tags| tags.into_iter().take(5).map(|(name, _)| name).collect());
        (similar, genres)
    };
    let (tidal, (similar, genres)) = tokio::join!(
        tokio::time::timeout(
            std::time::Duration::from_secs(6),
            client.get_artist_similar(seed_id, 10, 0)
        ),
        lastfm_lookup,
    );
    let tidal_related = tidal.ok().and_then(Result::ok).map(|page| {
        page.items
            .into_iter()
            .map(|artist| (artist.id, artist.name))
            .collect::<Vec<_>>()
    });
    let mut lastfm_related = None;
    let mut retry_after_hours = None;
    if let Some(similar) = similar {
        let mut resolved = Vec::new();
        let mut successful_searches = 0;
        let mut attempts = 0;
        for (rank, artist) in similar.into_iter().enumerate() {
            if rank >= 8 && artist.match_score.unwrap_or_default() < 0.2 {
                continue;
            }
            let cached = db.with_conn(|conn| known_related_artist_id(conn, &artist.name))?;
            if let Some(id) = cached {
                resolved.push((id, artist.name));
                continue;
            }
            if !db.with_conn(|conn| resolution_due(conn, &artist.name))? {
                retry_after_hours.get_or_insert(24);
                continue;
            }
            if !may_resolve_external(successful_searches, attempts) {
                retry_after_hours = Some(1);
                continue;
            }
            attempts += 1;
            let search = tokio::time::timeout(
                Duration::from_secs(4),
                client.search_catalog_core(&artist.name, 3, 0),
            )
            .await;
            match search {
                Ok(Ok(found)) => {
                    if let Some(matched) = found
                        .artists
                        .into_iter()
                        .find(|item| item.name.eq_ignore_ascii_case(&artist.name))
                    {
                        successful_searches += 1;
                        resolved.push((matched.id, matched.name));
                    } else {
                        db.with_conn(|conn| mark_resolution_failed(conn, &artist.name))?;
                        retry_after_hours.get_or_insert(24);
                    }
                }
                _ => retry_after_hours = Some(1),
            }
        }
        lastfm_related = Some(resolved);
    }
    let provider_failed = tidal_related.is_none() || (has_lastfm && lastfm_related.is_none());
    if provider_failed {
        retry_after_hours = Some(1);
    }
    db.with_conn(|conn| {
        apply_related_refresh(
            conn,
            seed_id,
            tidal_related.as_deref(),
            lastfm_related.as_deref(),
            genres.as_deref(),
            retry_after_hours,
        )
    })?;
    if tidal_related.is_none() && lastfm_related.is_none() {
        anyhow::bail!("both video relationship providers failed");
    }
    Ok(())
}

fn may_resolve_external(successes: usize, attempts: usize) -> bool {
    successes < 3 && attempts < 10
}

fn canonical_artist_name(conn: &Connection, id: i64) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT COALESCE(
            (SELECT name FROM artists WHERE tidal_id = ?1),
            (SELECT artist_name FROM video_catalog WHERE artist_tidal_id = ?1 AND artist_name <> '' LIMIT 1),
            (SELECT name FROM video_related_artists WHERE related_tidal_id = ?1 AND source = 'tidal' LIMIT 1))",
        [id], |row| row.get::<_, Option<String>>(0),
    )?.filter(|name| !name.trim().is_empty() && name.len() <= 120))
}

fn known_related_artist_id(conn: &Connection, name: &str) -> Result<Option<i64>> {
    if let Some(id) = local_artist_id(conn, name)? {
        return Ok(Some(id));
    }
    Ok(conn
        .query_row(
            "SELECT related_tidal_id FROM video_related_artists
         WHERE name = ?1 COLLATE NOCASE AND source IN ('lastfm', 'tidal') LIMIT 1",
            [name],
            |row| row.get(0),
        )
        .optional()?)
}

fn resolution_due(conn: &Connection, name: &str) -> Result<bool> {
    Ok(!conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM video_artist_resolution_failures
         WHERE name = ?1 AND attempted_at > datetime('now', '-1 day'))",
        [name],
        |row| row.get::<_, bool>(0),
    )?)
}

fn mark_resolution_failed(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO video_artist_resolution_failures (name) VALUES (?1)
        ON CONFLICT(name) DO UPDATE SET attempted_at = datetime('now')",
        [name],
    )?;
    Ok(())
}

fn apply_related_refresh(
    conn: &Connection,
    seed_id: i64,
    tidal: Option<&[(i64, String)]>,
    lastfm: Option<&[(i64, String)]>,
    genres: Option<&[String]>,
    retry_after_hours: Option<i64>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (source, snapshot) in [("tidal", tidal), ("lastfm", lastfm)] {
        if let Some(snapshot) = snapshot {
            tx.execute(
                "DELETE FROM video_related_artists WHERE seed_tidal_id = ?1 AND source = ?2",
                params![seed_id, source],
            )?;
            let mut seen = HashSet::new();
            for (id, name) in snapshot {
                if *id <= 0 || *id == seed_id || !seen.insert(*id) {
                    continue;
                }
                tx.execute(
                    "INSERT INTO video_related_artists
                     (seed_tidal_id, related_tidal_id, name, source) VALUES (?1, ?2, ?3, ?4)",
                    params![seed_id, id, name, source],
                )?;
            }
        }
    }
    if let Some(genres) = genres {
        store_seed_genres(&tx, seed_id, genres)?;
    }
    if tidal.is_some() || lastfm.is_some() {
        tx.execute(
            "INSERT INTO video_related_scans (seed_tidal_id) VALUES (?1)
            ON CONFLICT(seed_tidal_id) DO UPDATE SET scanned_at = datetime('now')",
            [seed_id],
        )?;
    }
    if let Some(hours) = retry_after_hours {
        tx.execute(
            "INSERT INTO video_related_retries (seed_tidal_id, retry_at)
            VALUES (?1, datetime('now', ?2))
            ON CONFLICT(seed_tidal_id) DO UPDATE SET retry_at = excluded.retry_at",
            params![seed_id, format!("+{hours} hours")],
        )?;
    } else {
        tx.execute(
            "DELETE FROM video_related_retries WHERE seed_tidal_id = ?1",
            [seed_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn store_seed_genres(conn: &Connection, seed_id: i64, genres: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM video_seed_genres WHERE seed_tidal_id = ?1",
        [seed_id],
    )?;
    for (rank, genre) in genres.iter().take(5).enumerate() {
        conn.execute(
            "INSERT OR IGNORE INTO video_seed_genres (seed_tidal_id, genre_name, rank) VALUES (?1, ?2, ?3)",
            params![seed_id, genre, rank as i64],
        )?;
    }
    Ok(())
}

pub fn local_artist_id(conn: &Connection, name: &str) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT tidal_id FROM artists WHERE name = ?1 COLLATE NOCASE AND tidal_id IS NOT NULL LIMIT 1",
        [name], |row| row.get(0),
    ).optional()?)
}

/// Presence in the catalog is not familiarity: imported discovery artists may
/// have local rows. Only deliberate listens and saves count as known taste.
pub fn familiar_artist_ids(conn: &Connection) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT a.tidal_id FROM artists a
         JOIN tracks t ON t.artist_id = a.id
         WHERE a.tidal_id IS NOT NULL AND t.is_favorite = 1
         UNION
         SELECT DISTINCT a.tidal_id FROM artists a
         JOIN albums al ON al.artist_id = a.id
         WHERE a.tidal_id IS NOT NULL AND al.is_favorite = 1
         UNION
         SELECT DISTINCT a.tidal_id FROM listen_history lh
         JOIN tracks t ON t.id = lh.track_id
         JOIN artists a ON a.id = t.artist_id
         WHERE a.tidal_id IS NOT NULL AND COALESCE(lh.source, '') NOT IN ('radio', 'automix')",
    )?;
    Ok(stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?)
}

/// One seed genre for the external video-search lane. Library tags win; tags
/// fetched from Last.fm fill in unfamiliar artists with no local tracks.
pub fn seed_genre(conn: &Connection, seed_id: i64) -> Result<Option<String>> {
    let local = conn
        .query_row(
            "SELECT g.name FROM artists a
         JOIN tracks t ON t.artist_id = a.id
         JOIN track_genres tg ON tg.track_id = t.id
         JOIN genres g ON g.id = tg.genre_id
         WHERE a.tidal_id = ?1 GROUP BY g.id ORDER BY COUNT(*) DESC LIMIT 1",
            [seed_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if local.is_some() {
        return Ok(local);
    }
    Ok(conn
        .query_row(
            "SELECT vg.genre_name FROM video_seed_genres vg
             JOIN genres g ON g.name = vg.genre_name COLLATE NOCASE
             WHERE vg.seed_tidal_id = ?1 ORDER BY vg.rank LIMIT 1",
            [seed_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

pub fn genre_due(conn: &Connection, genre: &str) -> Result<bool> {
    let age: Option<i64> = conn
        .query_row(
            "SELECT CAST(julianday('now') - julianday(scanned_at) AS INTEGER)
         FROM video_genre_scans WHERE genre_name = ?1",
            [genre],
            |row| row.get(0),
        )
        .optional()?;
    if age.is_some_and(|days| days < GENRE_CACHE_DAYS) {
        return Ok(false);
    }
    let recent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_genre_scans WHERE scanned_at >= datetime('now', '-30 minutes')",
        [],
        |row| row.get(0),
    )?;
    Ok(recent == 0)
}

pub fn mark_genre_scanned(conn: &Connection, genre: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO video_genre_scans (genre_name) VALUES (?1)
         ON CONFLICT(genre_name) DO UPDATE SET scanned_at = datetime('now')",
        [genre],
    )?;
    Ok(())
}

/// First the seed and its direct relationships, then genre neighbours and
/// library taste when the caller requests a general mix. Artist radio passes
/// no recent seeds or library anchors so its source artist stays fixed.
pub fn artist_pool(
    conn: &Connection,
    seed_id: Option<i64>,
    recent_seeds: &[i64],
    library_anchors: &[AnchorArtist],
) -> Result<Vec<(i64, String, SourceLane)>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if let Some(id) = seed_id.filter(|id| *id > 0) {
        let name: Option<String> = conn.query_row(
            "SELECT COALESCE((SELECT name FROM artists WHERE tidal_id = ?1),
                             (SELECT artist_name FROM video_catalog WHERE artist_tidal_id = ?1 LIMIT 1))",
            [id], |row| row.get(0),
        ).optional()?.flatten();
        out.push((id, name.unwrap_or_default(), SourceLane::Seed));
        seen.insert(id);
        let mut stmt = conn.prepare(
            "SELECT related_tidal_id, MIN(name) FROM video_related_artists
             WHERE seed_tidal_id = ?1 AND source IN ('lastfm', 'tidal')
             GROUP BY related_tidal_id
             ORDER BY MIN(CASE source WHEN 'lastfm' THEN 0 ELSE 1 END),
                      MIN(rowid) LIMIT 30",
        )?;
        for row in stmt.query_map([id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (related_id, name) = row?;
            if seen.insert(related_id) {
                out.push((related_id, name, SourceLane::Direct));
            }
        }
        // A second-hop artist only enters the station when two independent
        // direct neighbors recommend them. This expands sparse video catalogs
        // without turning one loose similarity link into a genre-wide mix.
        let mut stmt = conn.prepare(
            "SELECT hop.related_tidal_id, MIN(hop.name)
             FROM video_related_artists direct
             JOIN video_related_artists hop ON hop.seed_tidal_id = direct.related_tidal_id
             WHERE direct.seed_tidal_id = ?1
               AND direct.source IN ('tidal', 'lastfm')
               AND hop.source IN ('tidal', 'lastfm')
               AND hop.related_tidal_id <> ?1
             GROUP BY hop.related_tidal_id
             HAVING COUNT(DISTINCT hop.seed_tidal_id) >= 2
             ORDER BY COUNT(DISTINCT hop.seed_tidal_id) DESC, hop.related_tidal_id
             LIMIT 12",
        )?;
        for row in stmt.query_map([id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (bridge_id, name) = row?;
            if seen.insert(bridge_id) {
                out.push((bridge_id, name, SourceLane::Bridge));
            }
        }
        // One dominant local genre (or the first Last.fm tag that maps to our
        // taxonomy) is stronger evidence than overlap on any broad tag. Two
        // tagged tracks keep one-off or mislabeled tracks out of the radio.
        if let Some(genre) = seed_genre(conn, id)? {
            let mut stmt = conn.prepare(
                "SELECT a.tidal_id, a.name FROM artists a
                 JOIN tracks t ON t.artist_id = a.id
                 JOIN track_genres tg ON tg.track_id = t.id
                 JOIN genres g ON g.id = tg.genre_id
                 WHERE a.tidal_id IS NOT NULL AND g.name = ?1 COLLATE NOCASE
                 GROUP BY a.id HAVING COUNT(DISTINCT t.id) >= 2
                 ORDER BY COUNT(DISTINCT t.id) DESC, a.name LIMIT 20",
            )?;
            for row in stmt.query_map([genre], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })? {
                let (genre_id, name) = row?;
                if seen.insert(genre_id) {
                    out.push((genre_id, name, SourceLane::Genre));
                }
            }
        }
    }
    let mut prior_seeds = HashSet::new();
    for id in recent_seeds
        .iter()
        .rev()
        .copied()
        .filter(|id| *id > 0 && Some(*id) != seed_id)
    {
        if !prior_seeds.insert(id) {
            continue;
        }
        let mut stmt = conn.prepare(
            "SELECT related_tidal_id, MIN(name) FROM video_related_artists
             WHERE seed_tidal_id = ?1 AND source IN ('lastfm', 'tidal')
             GROUP BY related_tidal_id ORDER BY related_tidal_id LIMIT 20",
        )?;
        for row in stmt.query_map([id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (related_id, name) = row?;
            if seen.insert(related_id) {
                out.push((related_id, name, SourceLane::Direct));
            }
        }
        if prior_seeds.len() >= 3 {
            break;
        }
    }
    for anchor in library_anchors.iter().take(30) {
        if seen.insert(anchor.tidal_id) {
            out.push((anchor.tidal_id, anchor.name.clone(), SourceLane::Library));
        }
    }
    Ok(out)
}

/// Expand one direct neighbor per refill when the current station is thin.
/// The shared relationship ledger still enforces a seven-day TTL and a global
/// request budget, including background warming and other radio sessions.
pub fn next_bridge_seed(conn: &Connection, seed_id: i64) -> Result<Option<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT related_tidal_id, MIN(name) FROM video_related_artists
         WHERE seed_tidal_id = ?1 AND source IN ('lastfm', 'tidal')
         GROUP BY related_tidal_id
         ORDER BY MIN(CASE source WHEN 'lastfm' THEN 0 ELSE 1 END), MIN(rowid)
         LIMIT 20",
    )?;
    let neighbors = stmt
        .query_map([seed_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, name) in neighbors {
        if related_due(conn, id)? {
            return Ok(Some((id, name)));
        }
    }
    Ok(None)
}

pub fn load_candidates(
    conn: &Connection,
    artists: &[(i64, String, SourceLane)],
) -> Result<Vec<(VideoCandidate, SourceLane)>> {
    let ids: Vec<String> = artists.iter().map(|a| a.0.to_string()).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let priority = artists
        .iter()
        .enumerate()
        .map(|(rank, (id, _, _))| format!("WHEN {id} THEN {rank}"))
        .collect::<Vec<_>>()
        .join(" ");
    let lane: HashMap<i64, SourceLane> = artists.iter().map(|a| (a.0, a.2)).collect();
    let lead = artists[0].0;
    let sql = format!(
        "SELECT artist_tidal_id, item_json FROM video_catalog
         WHERE artist_tidal_id IN ({})
         ORDER BY CASE WHEN artist_tidal_id = {lead} THEN 0 ELSE 1 END,
                  CASE artist_tidal_id {priority} ELSE 999 END,
                  fetched_at DESC, tidal_video_id DESC",
        ids.join(","),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let (artist_id, json) = row?;
        if let (Some(priority), Ok(video)) = (
            artist_id.and_then(|id| lane.get(&id)),
            serde_json::from_str::<VideoCandidate>(&json),
        ) {
            if seen.insert(video.tidal_id) {
                out.push((video, *priority));
            }
        }
    }
    // Liked-song video matches are already indexed by the library scanner.
    // Reuse them directly; duplicating those rows in video_catalog adds no value.
    let sql = format!(
        "SELECT a.tidal_id, a.name, lv.tidal_video_id, lv.video_title,
                lv.duration_seconds, lv.image_id, lv.release_year
         FROM library_videos lv
         JOIN tracks t ON t.id = lv.track_id
         JOIN artists a ON a.id = t.artist_id
         WHERE lv.suppressed = 0 AND a.tidal_id IN ({})
         ORDER BY CASE a.tidal_id {priority} ELSE 999 END, lv.match_score DESC",
        ids.join(","),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<i32>>(6)?,
        ))
    })?;
    for row in rows {
        let (artist_id, artist_name, video_id, title, duration_s, image_id, release_year) = row?;
        if !seen.insert(video_id) {
            continue;
        }
        out.push((
            VideoCandidate {
                tidal_id: video_id,
                title,
                duration_s,
                artist_id: Some(artist_id),
                artist_name: Some(artist_name),
                album_tidal_id: None,
                artwork_url: crate::services::tidal::client::TidalClient::get_artwork_url(
                    &image_id, 640,
                ),
                release_year,
            },
            *lane.get(&artist_id).unwrap_or(&SourceLane::Library),
        ));
    }
    Ok(out)
}

/// Collapse alternate cuts of the same song while keeping the artist in the
/// key. A live/visualizer suffix should not make one song fill the queue twice.
pub fn video_song_key(artist_id: Option<i64>, artist_name: Option<&str>, title: &str) -> String {
    let artist = artist_id
        .map(|id| format!("id:{id}"))
        .unwrap_or_else(|| format!("name:{}", artist_name.unwrap_or("").trim().to_lowercase()));
    let base = title.split(['(', '[']).next().unwrap_or(title);
    let normalize = |text: &str| {
        text.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let normalized_title = normalize(base);
    let normalized_title = if normalized_title.is_empty() {
        normalize(title)
    } else {
        normalized_title
    };
    format!("{artist}:{normalized_title}")
}

/// Artist radio stays inside the seed's own videos and its direct graph.
/// Related artists provide discovery; broad library anchors never fill gaps.
pub fn select_seeded_batch(
    candidates: &[(VideoCandidate, SourceLane)],
    excluded_ids: &HashSet<i64>,
    recent_song_keys: &HashSet<String>,
    watched: &HashSet<i64>,
    recent_artists: &[i64],
    limit: usize,
) -> Vec<VideoSetItem> {
    let mut scored: Vec<_> = candidates
        .iter()
        .filter(|(video, lane)| {
            *lane != SourceLane::Library && !excluded_ids.contains(&video.tidal_id)
        })
        .filter_map(|(video, lane)| {
            let key = video_song_key(video.artist_id, video.artist_name.as_deref(), &video.title);
            (!recent_song_keys.contains(&key)).then_some((video, *lane, key))
        })
        .collect();
    scored.sort_by_key(|(video, lane, _)| {
        (
            watched.contains(&video.tidal_id) as u8,
            lane.seeded_priority(),
            (*lane != SourceLane::Seed
                && video
                    .artist_id
                    .is_some_and(|id| recent_artists.contains(&id))) as u8,
            video.tidal_id,
        )
    });
    let mut out = Vec::new();
    let mut used_songs = HashSet::<String>::new();
    let mut artist_counts = HashMap::<i64, usize>::new();
    let mut genre_count = 0;
    let mut bridge_count = 0;
    while out.len() < limit {
        let lanes = match out.len() % 4 {
            0 | 2 => [
                SourceLane::Seed,
                SourceLane::Direct,
                SourceLane::Bridge,
                SourceLane::Genre,
            ],
            1 => [
                SourceLane::Direct,
                SourceLane::Seed,
                SourceLane::Bridge,
                SourceLane::Genre,
            ],
            _ => [
                SourceLane::Bridge,
                SourceLane::Seed,
                SourceLane::Direct,
                SourceLane::Genre,
            ],
        };
        let last_artist = out
            .last()
            .and_then(|video: &VideoSetItem| video.artist_id)
            .or_else(|| recent_artists.last().copied());
        let mut chosen = None;
        for allow_watched in [false, true] {
            for lane in lanes {
                if (lane == SourceLane::Genre && genre_count >= 2)
                    || (lane == SourceLane::Bridge && bridge_count >= 4)
                {
                    continue;
                }
                for avoid_adjacent in [true, false] {
                    for artist_cap in [2, 4, limit] {
                        chosen = scored.iter().find(|(video, candidate_lane, key)| {
                            let artist = video.artist_id.unwrap_or(-video.tidal_id);
                            *candidate_lane == lane
                                && (allow_watched || !watched.contains(&video.tidal_id))
                                && !used_songs.contains(key.as_str())
                                && (lane == SourceLane::Seed
                                    || artist_counts.get(&artist).copied().unwrap_or(0)
                                        < artist_cap)
                                && (!avoid_adjacent || Some(artist) != last_artist)
                        });
                        if chosen.is_some() {
                            break;
                        }
                    }
                    if chosen.is_some() {
                        break;
                    }
                }
                if chosen.is_some() {
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let Some((video, lane, key)) = chosen else {
            break;
        };
        used_songs.insert((*key).clone());
        let artist = video.artist_id.unwrap_or(-video.tidal_id);
        *artist_counts.entry(artist).or_default() += 1;
        if *lane == SourceLane::Genre {
            genre_count += 1;
        } else if *lane == SourceLane::Bridge {
            bridge_count += 1;
        }
        out.push(VideoSetItem {
            tidal_id: video.tidal_id,
            title: video.title.clone(),
            duration_ms: video.duration_s.map(|duration| duration * 1000),
            artist_id: video.artist_id,
            artist_name: video.artist_name.clone(),
            album_tidal_id: video.album_tidal_id,
            artwork_url: video.artwork_url.clone(),
            quality: None,
            explicit: None,
            kind: "Music Video".into(),
            why: match lane {
                SourceLane::Seed => "More from this artist",
                SourceLane::Direct => "Related artist",
                SourceLane::Genre => "Shared genre",
                SourceLane::Bridge => "Recommended by related artists",
                SourceLane::Library => unreachable!("seeded selection excludes library"),
            }
            .into(),
        });
    }
    out
}

pub fn select_batch(
    candidates: &[(VideoCandidate, SourceLane)],
    excluded: &HashSet<i64>,
    watched: &HashSet<i64>,
    recent_artists: &[i64],
    familiar_artists: &HashSet<i64>,
    limit: usize,
) -> Vec<VideoSetItem> {
    let recent: HashSet<i64> = recent_artists.iter().copied().collect();
    let mut scored: Vec<_> = candidates
        .iter()
        .filter(|(v, _)| !excluded.contains(&v.tidal_id))
        .map(|(v, lane)| {
            let watched_penalty = if watched.contains(&v.tidal_id) { 10 } else { 0 };
            let artist_penalty = if v.artist_id.is_some_and(|id| recent.contains(&id)) {
                5
            } else {
                0
            };
            (v, lane.score() + watched_penalty + artist_penalty)
        })
        .collect();
    scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.tidal_id.cmp(&b.0.tidal_id)));
    let mut out = Vec::new();
    let mut artist_count = HashMap::<i64, usize>::new();
    let mut used = HashSet::new();
    // Familiarity is the spine of the mix, with a related/genre discovery pick
    // in every third slot. Relax the artist cap before switching lanes so a
    // large unfamiliar pool cannot crowd out the familiar share.
    while out.len() < limit {
        let last_artist = out
            .last()
            .and_then(|v: &VideoSetItem| v.artist_id)
            .or_else(|| recent_artists.last().copied());
        let prefer_unfamiliar = out.len() % 3 == 2;
        let mut next = None;
        for avoid_adjacent in [true, false] {
            for want_unfamiliar in [prefer_unfamiliar, !prefer_unfamiliar] {
                for cap in [1, 2, 3] {
                    next = scored.iter().find(|(video, _)| {
                        let artist = video.artist_id.unwrap_or(-video.tidal_id);
                        let unfamiliar = video
                            .artist_id
                            .is_some_and(|id| !familiar_artists.contains(&id));
                        !used.contains(&video.tidal_id)
                            && artist_count.get(&artist).copied().unwrap_or(0) < cap
                            && unfamiliar == want_unfamiliar
                            && (!avoid_adjacent || Some(artist) != last_artist)
                    });
                    if next.is_some() {
                        break;
                    }
                }
                if next.is_some() {
                    break;
                }
            }
            if next.is_some() {
                break;
            }
        }
        let Some((video, _)) = next else {
            break;
        };
        used.insert(video.tidal_id);
        let artist = video.artist_id.unwrap_or(-video.tidal_id);
        *artist_count.entry(artist).or_default() += 1;
        out.push(VideoSetItem {
            tidal_id: video.tidal_id,
            title: video.title.clone(),
            duration_ms: video.duration_s.map(|d| d * 1000),
            artist_id: video.artist_id,
            artist_name: video.artist_name.clone(),
            album_tidal_id: video.album_tidal_id,
            artwork_url: video.artwork_url.clone(),
            quality: None,
            explicit: None,
            kind: "Music Video".into(),
            why: String::new(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_video_and_relationship_are_reused_without_a_scan() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let anchor = AnchorArtist {
            tidal_id: 42,
            name: "Seed".into(),
            listens: 1,
            via: None,
        };
        let video = VideoCandidate {
            tidal_id: 901,
            title: "Live".into(),
            duration_s: Some(220),
            artist_id: Some(42),
            artist_name: Some("Seed".into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: Some(2024),
        };
        cache_groups(&conn, &[(anchor, vec![video])]).unwrap();
        store_related(&conn, 42, &[(43, "Neighbour".into(), "tidal")]).unwrap();
        assert!(!artist_due(&conn, 42).unwrap());
        assert!(!related_due(&conn, 42).unwrap());
        let pool = artist_pool(&conn, Some(42), &[], &[]).unwrap();
        assert!(pool.iter().any(|(id, _, _)| *id == 43));
        let flowing_pool = artist_pool(&conn, Some(44), &[42], &[]).unwrap();
        assert!(flowing_pool.iter().any(|(id, _, _)| *id == 43));
        let cached = load_candidates(&conn, &pool).unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].0.tidal_id, 901);
    }

    #[test]
    fn all_cached_neighbor_videos_remain_candidates() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let candidate = |id, artist, name: &str| VideoCandidate {
            tidal_id: id,
            title: format!("Video {id}"),
            duration_s: Some(180),
            artist_id: Some(artist),
            artist_name: Some(name.into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
        };
        cache_groups(
            &conn,
            &[(
                AnchorArtist {
                    tidal_id: 10,
                    name: "Seed".into(),
                    listens: 1,
                    via: None,
                },
                vec![candidate(1, 10, "Seed")],
            )],
        )
        .unwrap();
        let neighbor_videos = (1000..=1600)
            .map(|id| candidate(id, 20, "Neighbor"))
            .collect();
        cache_groups(
            &conn,
            &[(
                AnchorArtist {
                    tidal_id: 20,
                    name: "Neighbor".into(),
                    listens: 1,
                    via: None,
                },
                neighbor_videos,
            )],
        )
        .unwrap();
        let candidates = load_candidates(
            &conn,
            &[
                (10, "Seed".into(), SourceLane::Seed),
                (20, "Neighbor".into(), SourceLane::Direct),
            ],
        )
        .unwrap();
        assert_eq!(candidates.len(), 602);
        assert!(candidates.iter().any(|(video, _)| video.tidal_id == 1));
    }

    #[test]
    fn lastfm_genre_tag_can_find_a_local_neighbor_with_cached_video() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO genres (id, name, slug) VALUES (1, 'Electronic', 'electronic')",
            [],
        )
        .unwrap();
        store_seed_genres(&conn, 42, &["Seen live".into(), "Electronic".into()]).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES
                (1, 45, 'New Artist'), (2, 46, 'One-off tag');
             INSERT INTO tracks (id, artist_id, title) VALUES
                (1, 1, 'Discovery'), (2, 1, 'Another'), (3, 2, 'Noise');
             INSERT INTO track_genres (track_id, genre_id) VALUES
                (1, 1), (2, 1), (3, 1);",
        )
        .unwrap();
        assert_eq!(
            seed_genre(&conn, 42).unwrap().as_deref(),
            Some("Electronic")
        );
        let video = VideoCandidate {
            tidal_id: 902,
            title: "Discovery".into(),
            duration_s: Some(210),
            artist_id: Some(45),
            artist_name: Some("New Artist".into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
        };
        cache_groups(
            &conn,
            &[(
                AnchorArtist {
                    tidal_id: -1,
                    name: "Electronic music video".into(),
                    listens: 1,
                    via: None,
                },
                vec![video.clone()],
            )],
        )
        .unwrap();
        assert!(genre_due(&conn, "Electronic").unwrap());
        let pool = artist_pool(&conn, Some(42), &[], &[]).unwrap();
        assert!(
            pool.iter()
                .any(|(id, _, lane)| *id == 45 && *lane == SourceLane::Genre)
        );
        assert!(!pool.iter().any(|(id, _, _)| *id == 46));
        assert!(
            load_candidates(&conn, &pool)
                .unwrap()
                .iter()
                .any(|(video, _)| video.tidal_id == 902)
        );
    }

    #[test]
    fn green_day_style_genre_fallback_excludes_broad_rock_only_artists() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug) VALUES
                (1, 'Punk Rock', 'punk-rock'), (2, 'Rock', 'rock');
             INSERT INTO artists (id, tidal_id, name) VALUES
                (1, 10, 'Green Day'), (2, 20, 'Punk Neighbor'), (3, 30, 'Broad Rock');
             INSERT INTO tracks (id, artist_id, title) VALUES
                (1, 1, 'Seed One'), (2, 1, 'Seed Two'),
                (3, 2, 'Punk One'), (4, 2, 'Punk Two'),
                (5, 3, 'Rock One'), (6, 3, 'Rock Two');
             INSERT INTO track_genres (track_id, genre_id) VALUES
                (1, 1), (2, 1), (1, 2),
                (3, 1), (4, 1), (5, 2), (6, 2);",
        )
        .unwrap();
        let pool = artist_pool(&conn, Some(10), &[], &[]).unwrap();
        assert!(
            pool.iter()
                .any(|(id, _, lane)| *id == 20 && *lane == SourceLane::Genre)
        );
        assert!(!pool.iter().any(|(id, _, _)| *id == 30));
    }

    #[test]
    fn batch_prefers_new_artists_and_excludes_queue() {
        let candidate = |id, artist| VideoCandidate {
            tidal_id: id,
            title: format!("Video {id}"),
            duration_s: Some(180),
            artist_id: Some(artist),
            artist_name: Some(format!("Artist {artist}")),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
        };
        let pool = vec![
            (candidate(1, 1), SourceLane::Seed),
            (candidate(2, 1), SourceLane::Seed),
            (candidate(3, 2), SourceLane::Direct),
            (candidate(4, 3), SourceLane::Genre),
        ];
        let selected = select_batch(
            &pool,
            &HashSet::from([1]),
            &HashSet::from([3]),
            &[1],
            &HashSet::new(),
            3,
        );
        assert_eq!(
            selected.iter().map(|v| v.tidal_id).collect::<Vec<_>>(),
            vec![4, 2, 3]
        );
    }

    #[test]
    fn familiar_videos_lead_and_unfamiliar_artists_are_interleaved() {
        let candidates: Vec<_> = (1..=12)
            .map(|artist| {
                (
                    VideoCandidate {
                        tidal_id: 100 + artist,
                        title: format!("Video {artist}"),
                        duration_s: Some(180),
                        artist_id: Some(artist),
                        artist_name: Some(format!("Artist {artist}")),
                        album_tidal_id: None,
                        artwork_url: None,
                        release_year: None,
                    },
                    if artist <= 8 {
                        SourceLane::Seed
                    } else {
                        SourceLane::Direct
                    },
                )
            })
            .collect();
        let familiar: HashSet<i64> = (1..=8).collect();
        let items = select_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::new(),
            &[],
            &familiar,
            12,
        );
        assert_eq!(items.len(), 12);
        for (index, item) in items.iter().enumerate() {
            let is_unfamiliar = !familiar.contains(&item.artist_id.unwrap());
            assert_eq!(is_unfamiliar, index % 3 == 2, "slot {index}");
        }
        assert_eq!(
            items
                .iter()
                .map(|item| item.tidal_id)
                .collect::<HashSet<_>>()
                .len(),
            12
        );
    }

    #[test]
    fn seeded_radio_stays_with_seed_and_direct_neighbors_without_repeating_songs() {
        let candidate = |id, artist, title: &str, lane| {
            (
                VideoCandidate {
                    tidal_id: id,
                    title: title.into(),
                    duration_s: Some(180),
                    artist_id: Some(artist),
                    artist_name: Some(format!("Artist {artist}")),
                    album_tidal_id: None,
                    artwork_url: None,
                    release_year: None,
                },
                lane,
            )
        };
        let candidates = vec![
            candidate(1, 10, "Basket Case (Live)", SourceLane::Seed),
            candidate(2, 10, "Basket Case [Official Video]", SourceLane::Seed),
            candidate(3, 10, "Holiday", SourceLane::Seed),
            candidate(4, 10, "American Idiot", SourceLane::Seed),
            candidate(5, 10, "Jesus of Suburbia", SourceLane::Seed),
            candidate(6, 20, "Related one", SourceLane::Direct),
            candidate(7, 20, "Related two", SourceLane::Direct),
            candidate(8, 30, "Another neighbor", SourceLane::Direct),
            candidate(9, 40, "Genre neighbor", SourceLane::Genre),
            candidate(12, 60, "Corroborated neighbor", SourceLane::Bridge),
            candidate(10, 50, "Unrelated favorite", SourceLane::Library),
            candidate(11, 51, "Another unrelated favorite", SourceLane::Library),
        ];
        let recent_song = video_song_key(Some(10), None, "American Idiot (Visualizer)");
        let selected = select_seeded_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::from([recent_song]),
            &HashSet::new(),
            &[10],
            12,
        );
        let ids: HashSet<_> = selected.iter().map(|video| video.tidal_id).collect();
        assert!(!ids.contains(&4), "a recently queued song must stay out");
        assert!(
            !(ids.contains(&1) && ids.contains(&2)),
            "alternate cuts of one song must collapse"
        );
        assert!(
            !ids.contains(&10) && !ids.contains(&11),
            "library anchors are unrelated to this seed"
        );
        assert!(
            selected
                .iter()
                .filter(|video| video.artist_id == Some(10))
                .count()
                >= 3
        );
        assert!(selected.iter().any(|video| video.artist_id == Some(20)));
        assert!(
            selected
                .iter()
                .any(|video| video.why == "Recommended by related artists")
        );
        assert!(
            selected
                .iter()
                .filter(|video| video.why == "Shared genre")
                .count()
                <= 2
        );
    }

    #[test]
    fn corroborated_artists_are_blended_through_a_seeded_mix() {
        let candidates = (1..=36)
            .map(|id| {
                let (artist, lane) = if id <= 12 {
                    (10, SourceLane::Seed)
                } else if id <= 24 {
                    (20, SourceLane::Direct)
                } else {
                    (30, SourceLane::Bridge)
                };
                (
                    VideoCandidate {
                        tidal_id: id,
                        title: format!("Song {id}"),
                        duration_s: Some(180),
                        artist_id: Some(artist),
                        artist_name: Some(format!("Artist {artist}")),
                        album_tidal_id: None,
                        artwork_url: None,
                        release_year: None,
                    },
                    lane,
                )
            })
            .collect::<Vec<_>>();
        let selected = select_seeded_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[],
            12,
        );
        assert_eq!(selected.len(), 12);
        assert!(
            selected
                .iter()
                .filter(|video| video.artist_id == Some(10))
                .count()
                >= 5
        );
        assert_eq!(
            selected
                .iter()
                .filter(|video| video.artist_id == Some(30))
                .count(),
            3
        );
    }

    #[test]
    fn a_related_artist_does_not_dominate_when_other_neighbors_have_videos() {
        let candidates: Vec<_> = (1..=32)
            .map(|id| {
                let (artist, lane) = if id <= 8 {
                    (10, SourceLane::Seed)
                } else if id <= 20 {
                    (20, SourceLane::Direct)
                } else {
                    (30, SourceLane::Direct)
                };
                (
                    VideoCandidate {
                        tidal_id: id,
                        title: format!("Song {id}"),
                        duration_s: Some(180),
                        artist_id: Some(artist),
                        artist_name: Some(format!("Artist {artist}")),
                        album_tidal_id: None,
                        artwork_url: None,
                        release_year: None,
                    },
                    lane,
                )
            })
            .collect();
        let selected = select_seeded_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[],
            12,
        );
        assert_eq!(selected.len(), 12);
        assert!(
            selected
                .iter()
                .filter(|item| item.artist_id == Some(10))
                .count()
                >= 5
        );
        assert!(
            selected
                .iter()
                .filter(|item| item.artist_id == Some(20))
                .count()
                <= 4
        );
        assert!(selected.iter().any(|item| item.artist_id == Some(30)));
    }

    #[test]
    fn relationship_budget_allows_several_manual_artist_starts() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        for seed in 1..=8 {
            assert!(related_due(&conn, seed).unwrap());
            assert!(reserve_related_scan(&conn, seed).unwrap());
            assert!(
                !reserve_related_scan(&conn, seed).unwrap(),
                "concurrent callers reuse one pass"
            );
        }
        assert!(!related_due(&conn, 1).unwrap());
        assert!(
            !related_due(&conn, 9).unwrap(),
            "the ninth new seed waits for the budget window"
        );
    }

    #[test]
    fn queue_health_counts_distinct_unfamiliar_artists_and_checks_bridges() {
        let item = |artist_id| VideoSetItem {
            tidal_id: artist_id * 10,
            title: "Video".into(),
            duration_ms: None,
            artist_id: Some(artist_id),
            artist_name: None,
            album_tidal_id: None,
            artwork_url: None,
            quality: None,
            explicit: None,
            kind: "Music Video".into(),
            why: String::new(),
        };
        let one_artist = vec![item(20), item(20), item(20), item(20)];
        assert_eq!(
            unfamiliar_artist_count(&one_artist, &HashSet::new(), Some(10)),
            1
        );
        assert!(queue_needs_discovery(12, 1, false, false));
        let three_artists = vec![item(20), item(30), item(40), item(10)];
        assert_eq!(
            unfamiliar_artist_count(&three_artists, &HashSet::new(), Some(10)),
            3
        );
        assert!(!queue_needs_discovery(12, 3, false, false));
        assert!(queue_needs_discovery(12, 3, false, true));

        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        assert!(!bridge_due(&conn, 10).unwrap());
        store_related(&conn, 10, &[(20, "Neighbor".into(), "tidal")]).unwrap();
        assert!(bridge_due(&conn, 10).unwrap());
        mark_bridge_scanned(&conn, 10).unwrap();
        assert!(!bridge_due(&conn, 10).unwrap());
        conn.execute(
            "UPDATE video_bridge_scans SET scanned_at = datetime('now', '-2 hours')",
            [],
        )
        .unwrap();
        assert!(bridge_due(&conn, 10).unwrap());
    }

    #[test]
    fn provider_snapshots_replace_only_successful_sources() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        apply_related_refresh(
            &conn,
            10,
            Some(&[(20, "Old TIDAL".into()), (30, "Shared".into())]),
            Some(&[(30, "Shared".into()), (40, "Old Last.fm".into())]),
            None,
            None,
        )
        .unwrap();
        // Last.fm failed; a successful TIDAL response drops only its stale edge.
        apply_related_refresh(
            &conn,
            10,
            Some(&[(30, "Shared".into())]),
            None,
            None,
            Some(1),
        )
        .unwrap();
        let count = |source: &str| -> i64 {
            conn.query_row(
            "SELECT COUNT(*) FROM video_related_artists WHERE seed_tidal_id = 10 AND source = ?1",
            [source], |row| row.get(0)).unwrap()
        };
        assert_eq!(count("tidal"), 1);
        assert_eq!(count("lastfm"), 2);
        // A valid empty response removes the old Last.fm links, preserving TIDAL.
        apply_related_refresh(&conn, 10, None, Some(&[]), None, None).unwrap();
        assert_eq!(count("tidal"), 1);
        assert_eq!(count("lastfm"), 0);
    }

    #[test]
    fn failed_resolution_retries_before_relationship_ttl() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        mark_resolution_failed(&conn, "Unresolved").unwrap();
        assert!(!resolution_due(&conn, "Unresolved").unwrap());
        apply_related_refresh(&conn, 10, Some(&[]), Some(&[]), None, Some(24)).unwrap();
        assert!(!related_due(&conn, 10).unwrap());
        conn.execute(
            "UPDATE video_related_retries SET retry_at = datetime('now', '-1 minute')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE video_related_attempts SET attempted_at = datetime('now', '-2 hours')",
            [],
        )
        .unwrap();
        assert!(related_due(&conn, 10).unwrap());
        conn.execute(
            "UPDATE video_artist_resolution_failures SET attempted_at = datetime('now', '-2 days')",
            [],
        )
        .unwrap();
        assert!(resolution_due(&conn, "Unresolved").unwrap());
    }

    #[test]
    fn failed_exact_matches_do_not_exhaust_success_budget() {
        assert!(may_resolve_external(0, 3));
        assert!(may_resolve_external(2, 9));
        assert!(!may_resolve_external(3, 3));
        assert!(!may_resolve_external(0, 10));
    }

    #[test]
    fn canonical_artist_name_comes_from_id_not_request_text() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO artists (tidal_id, name) VALUES (10, 'Green Day')",
            [],
        )
        .unwrap();
        assert_eq!(
            canonical_artist_name(&conn, 10).unwrap().as_deref(),
            Some("Green Day")
        );
    }

    #[test]
    fn second_hop_requires_two_direct_artist_recommendations() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        store_related(
            &conn,
            10,
            &[
                (20, "Neighbor A".into(), "lastfm"),
                (30, "Neighbor B".into(), "tidal"),
            ],
        )
        .unwrap();
        assert_eq!(
            next_bridge_seed(&conn, 10).unwrap(),
            Some((20, "Neighbor A".into()))
        );
        store_related(
            &conn,
            20,
            &[
                (40, "Agreed artist".into(), "lastfm"),
                (50, "Loose artist".into(), "tidal"),
            ],
        )
        .unwrap();
        let first = artist_pool(&conn, Some(10), &[], &[]).unwrap();
        assert!(!first.iter().any(|(id, _, _)| *id == 40 || *id == 50));
        assert_eq!(
            next_bridge_seed(&conn, 10).unwrap(),
            Some((30, "Neighbor B".into()))
        );
        store_related(&conn, 30, &[(40, "Agreed artist".into(), "tidal")]).unwrap();
        let expanded = artist_pool(&conn, Some(10), &[], &[]).unwrap();
        assert!(
            expanded
                .iter()
                .any(|(id, _, lane)| *id == 40 && *lane == SourceLane::Bridge)
        );
        assert!(!expanded.iter().any(|(id, _, _)| *id == 50));
    }

    #[test]
    fn large_seed_catalog_does_not_hide_new_artist_videos() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let seed = AnchorArtist {
            tidal_id: 10,
            name: "Seed".into(),
            listens: 1,
            via: None,
        };
        let neighbor = AnchorArtist {
            tidal_id: 20,
            name: "Neighbor".into(),
            listens: 1,
            via: None,
        };
        let make_video = |id, artist_id, name: &str| VideoCandidate {
            tidal_id: id,
            title: format!("Song {id}"),
            duration_s: Some(180),
            artist_id: Some(artist_id),
            artist_name: Some(name.into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
        };
        let seed_videos = (1..=650).map(|id| make_video(id, 10, "Seed")).collect();
        cache_groups_without_prune(
            &conn,
            &[
                (seed, seed_videos),
                (neighbor, vec![make_video(999, 20, "Neighbor")]),
            ],
        )
        .unwrap();
        let candidates = load_candidates(
            &conn,
            &[
                (10, "Seed".into(), SourceLane::Seed),
                (20, "Neighbor".into(), SourceLane::Direct),
            ],
        )
        .unwrap();
        assert_eq!(candidates.len(), 651);
        assert!(
            candidates
                .iter()
                .any(|(video, lane)| video.tidal_id == 999 && *lane == SourceLane::Direct)
        );
    }

    #[test]
    fn liked_graph_warm_prioritizes_likes_and_reuses_scanned_neighbors() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES
                (1, 100, 'Favorite'), (2, 200, 'Other');
             INSERT INTO tracks (id, artist_id, title, is_favorite) VALUES
                (1, 1, 'First', 1), (2, 1, 'Second', 1), (3, 2, 'Third', 1);",
        )
        .unwrap();
        assert_eq!(
            liked_seeds_needing_warm(&conn).unwrap(),
            vec![(100, "Favorite".into()), (200, "Other".into())]
        );
        store_related(
            &conn,
            100,
            &[
                (301, "Last.fm neighbor".into(), "lastfm"),
                (302, "TIDAL neighbor".into(), "tidal"),
                (303, "Unverified old genre hit".into(), "genre"),
            ],
        )
        .unwrap();
        assert_eq!(
            liked_seeds_needing_warm(&conn).unwrap(),
            vec![(200, "Other".into())]
        );
        conn.execute(
            "INSERT INTO video_related_retries (seed_tidal_id, retry_at)
            VALUES (100, datetime('now', '-1 minute'))",
            [],
        )
        .unwrap();
        assert!(
            liked_seeds_needing_warm(&conn)
                .unwrap()
                .iter()
                .any(|(id, _)| *id == 100)
        );
        conn.execute(
            "DELETE FROM video_related_retries WHERE seed_tidal_id = 100",
            [],
        )
        .unwrap();
        assert_eq!(
            related_catalog_targets(&conn, 100).unwrap(),
            vec![
                (302, "TIDAL neighbor".into()),
                (301, "Last.fm neighbor".into())
            ]
        );
        assert_eq!(liked_seeds_with_pending_catalogs(&conn).unwrap(), vec![100]);
        assert!(
            !artist_pool(&conn, Some(100), &[], &[])
                .unwrap()
                .iter()
                .any(|(id, _, _)| *id == 303)
        );
        mark_artist_scanned(&conn, 302).unwrap();
        assert_eq!(
            related_catalog_targets(&conn, 100).unwrap(),
            vec![(301, "Last.fm neighbor".into())]
        );
        mark_artist_scanned(&conn, 301).unwrap();
        assert!(liked_seeds_with_pending_catalogs(&conn).unwrap().is_empty());
    }

    #[test]
    fn saved_video_and_favorite_album_artists_are_warm_seeds() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES (1, 300, 'Album Artist');
             INSERT INTO albums (id, artist_id, title, is_favorite)
                VALUES (1, 1, 'Favorite Album', 1);
             INSERT INTO saved_videos (tidal_video_id, item_json)
                VALUES (400, '{\"tidal_id\":400,\"artist_id\":400,\"artist_name\":\"Saved Video Artist\"}');",
        )
        .unwrap();
        let seeds = liked_seeds_needing_warm(&conn).unwrap();
        assert!(seeds.contains(&(300, "Album Artist".into())));
        assert!(seeds.contains(&(400, "Saved Video Artist".into())));
    }
}
