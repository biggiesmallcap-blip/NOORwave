use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::db::models::Track;
use crate::playback::player::ReconcileOutcome;

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub id: i64,
    pub status: String,
    pub members: Vec<DuplicateMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateMember {
    pub track: Track,
    pub is_preferred: bool,
    /// Why this track was grouped: "isrc" or "title_duration"
    pub match_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanStats {
    pub groups_found: usize,
    pub tracks_affected: usize,
    pub isrc_matches: usize,
    pub title_matches: usize,
}

// ── Normalisation helpers ─────────────────────────────────────────────────────

/// Lowercase, strip non-alphanumeric (keep spaces), collapse whitespace.
fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let filtered: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    filtered.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone)]
struct MatchRow {
    id: i64,
    norm_title: String,
    canonical_title: String,
    norm_artist: String,
    artist_tokens: Vec<String>,
    duration_ms: i64,
}

fn build_match_row(id: i64, title: &str, artist: &str, duration_ms: Option<i64>) -> MatchRow {
    let norm_artist = normalize(artist);
    let artist_tokens = if norm_artist.is_empty() {
        Vec::new()
    } else {
        norm_artist
            .split_whitespace()
            .map(|token| token.to_string())
            .collect()
    };

    MatchRow {
        id,
        norm_title: normalize(title),
        canonical_title: canonicalize_title(title),
        norm_artist,
        artist_tokens,
        duration_ms: duration_ms.unwrap_or(0),
    }
}

fn canonicalize_title(title: &str) -> String {
    let mut canonical = strip_ignorable_bracketed_segments(title.trim());

    loop {
        let next = strip_ignorable_suffix(&canonical);
        if next == canonical {
            break;
        }
        canonical = next;
    }

    normalize(&canonical)
}

fn strip_ignorable_bracketed_segments(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut idx = 0usize;
    let mut output = String::new();

    while idx < chars.len() {
        let open = chars[idx];
        let close = match open {
            '(' => ')',
            '[' => ']',
            _ => {
                output.push(open);
                idx += 1;
                continue;
            }
        };

        let mut end = idx + 1;
        let mut depth = 1;
        while end < chars.len() {
            if chars[end] == open {
                depth += 1;
            } else if chars[end] == close {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            end += 1;
        }

        if depth == 0 {
            let segment: String = chars[(idx + 1)..end].iter().collect();
            if is_ignorable_title_segment(&segment) {
                idx = end + 1;
                continue;
            }
        }

        output.push(open);
        idx += 1;
    }

    output
}

fn strip_ignorable_suffix(input: &str) -> String {
    for separator in [" - ", " – ", " — "] {
        if let Some((prefix, suffix)) = input.rsplit_once(separator) {
            if is_ignorable_title_segment(suffix) {
                return prefix.trim().to_string();
            }
        }
    }

    input.trim().to_string()
}

fn is_ignorable_title_segment(segment: &str) -> bool {
    let normalized = normalize(segment);
    matches!(
        normalized.as_str(),
        "feat" | "ft" | "featuring" | "original" | "explicit" | "clean" | "mono" | "stereo"
    ) || normalized.starts_with("feat ")
        || normalized.starts_with("ft ")
        || normalized.starts_with("featuring ")
}

fn variant_markers(title: &str) -> Vec<&'static str> {
    let normalized = normalize(title);
    let mut markers = Vec::new();
    for marker in [
        "remix",
        "live",
        "acoustic",
        "instrumental",
        "dub",
        "edit",
        "demo",
        "vip",
        "rework",
    ] {
        if normalized.split_whitespace().any(|token| token == marker) {
            markers.push(marker);
        }
    }
    markers
}

fn titles_compatible(left: &MatchRow, right: &MatchRow, via_isrc: bool) -> bool {
    if left.norm_title == right.norm_title || left.canonical_title == right.canonical_title {
        return true;
    }

    if variant_markers(&left.norm_title) != variant_markers(&right.norm_title) {
        return false;
    }

    if left.canonical_title.is_empty() || right.canonical_title.is_empty() {
        return false;
    }

    let similarity = strsim::jaro_winkler(&left.canonical_title, &right.canonical_title);
    if via_isrc {
        similarity >= 0.93
    } else {
        similarity >= 0.985
    }
}

fn artists_compatible(left: &MatchRow, right: &MatchRow) -> bool {
    if left.norm_artist == right.norm_artist {
        return true;
    }

    let (shorter, longer) = if left.artist_tokens.len() <= right.artist_tokens.len() {
        (&left.artist_tokens, &right.artist_tokens)
    } else {
        (&right.artist_tokens, &left.artist_tokens)
    };

    if shorter.len() >= 2
        && shorter
            .iter()
            .all(|token| longer.iter().any(|candidate| candidate == token))
    {
        return true;
    }

    strsim::jaro_winkler(&left.norm_artist, &right.norm_artist) >= 0.92
}

fn durations_compatible(
    left_ms: i64,
    right_ms: i64,
    max_diff_ms: i64,
    max_diff_percent: i64,
) -> bool {
    if left_ms <= 0 || right_ms <= 0 {
        return false;
    }

    let diff = (left_ms - right_ms).abs();
    let longer = left_ms.max(right_ms);
    diff <= max_diff_ms && diff * 100 <= longer * max_diff_percent
}

fn rows_match(left: &MatchRow, right: &MatchRow, via_isrc: bool) -> bool {
    if !artists_compatible(left, right) || !titles_compatible(left, right, via_isrc) {
        return false;
    }

    if via_isrc {
        durations_compatible(left.duration_ms, right.duration_ms, 15_000, 8)
    } else {
        durations_compatible(left.duration_ms, right.duration_ms, 3_000, 3)
    }
}

/// Union-Find (disjoint set) data structure for O(n·α(n)) connected components.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        if self.rank[ra] < self.rank[rb] {
            self.parent[ra] = rb;
        } else if self.rank[ra] > self.rank[rb] {
            self.parent[rb] = ra;
        } else {
            self.parent[rb] = ra;
            self.rank[ra] += 1;
        }
    }

    fn into_groups(self) -> Vec<Vec<usize>> {
        let mut groups: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, &p) in self.parent.iter().enumerate() {
            groups.entry(p).or_default().push(i);
        }
        groups.into_values().filter(|g| g.len() > 1).collect()
    }
}

fn connected_components<F>(rows: &[MatchRow], predicate: F) -> Vec<Vec<usize>>
where
    F: Fn(&MatchRow, &MatchRow) -> bool,
{
    let n = rows.len();
    if n <= 1 {
        return Vec::new();
    }

    let mut uf = UnionFind::new(n);

    // Build adjacency by checking all pairs — but Union-Find makes merging O(α(n))
    for i in 0..n {
        for j in (i + 1)..n {
            if predicate(&rows[i], &rows[j]) {
                uf.union(i, j);
            }
        }
    }

    uf.into_groups()
}

// ── Scan ──────────────────────────────────────────────────────────────────────

/// Full duplicate scan. Clears old pending groups then rebuilds.
/// Returns counts of what was found.
pub fn scan(conn: &Connection) -> Result<ScanStats> {
    // Clear all pending groups so re-scanning is idempotent.
    conn.execute("DELETE FROM duplicate_groups WHERE status = 'pending'", [])?;

    let mut isrc_matches = 0usize;
    let mut title_matches = 0usize;

    // ── Pass 1: ISRC matches ─────────────────────────────────────────────────
    // Any ISRC that appears on more than one track row is a duplicate.
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, COALESCE(a.name, ''), t.duration_ms, t.isrc
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         WHERE t.isrc IS NOT NULL AND t.isrc != ''",
    )?;

    let mut isrc_groups: HashMap<String, Vec<MatchRow>> = HashMap::new();
    for row in stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .filter_map(|r| r.ok())
    {
        let (id, title, artist, duration_ms, isrc) = row;
        isrc_groups.entry(isrc).or_default().push(build_match_row(
            id,
            &title,
            &artist,
            duration_ms,
        ));
    }

    for rows in isrc_groups.into_values() {
        for component in connected_components(&rows, |left, right| rows_match(left, right, true)) {
            let gid = insert_group(conn, "pending")?;
            for idx in component {
                conn.execute(
                    "INSERT OR IGNORE INTO duplicate_members (group_id, track_id, is_preferred)
                     VALUES (?1, ?2, 0)",
                    params![gid, rows[idx].id],
                )?;
            }
            mark_preferred(conn, gid)?;
            isrc_matches += 1;
        }
    }

    // ── Pass 2: title + artist + duration match ──────────────────────────────
    // Load all tracks that weren't already grouped by ISRC.
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, t.duration_ms
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         WHERE t.isrc IS NULL OR t.isrc = ''",
    )?;

    let rows: Vec<MatchRow> = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let artist: Option<String> = row.get(2)?;
            let duration_ms: Option<i64> = row.get(3)?;
            Ok((id, title, artist.unwrap_or_default(), duration_ms))
        })?
        .filter_map(|r| r.ok())
        .map(|(id, title, artist, duration_ms)| build_match_row(id, &title, &artist, duration_ms))
        .collect();

    let mut buckets: HashMap<(String, String), Vec<MatchRow>> = HashMap::new();
    for row in rows {
        if row.canonical_title.is_empty() || row.norm_artist.is_empty() {
            continue;
        }

        buckets
            .entry((row.canonical_title.clone(), row.norm_artist.clone()))
            .or_default()
            .push(row);
    }

    for rows in buckets.into_values() {
        for component in connected_components(&rows, |left, right| rows_match(left, right, false)) {
            let gid = insert_group(conn, "pending")?;
            for idx in component {
                conn.execute(
                    "INSERT OR IGNORE INTO duplicate_members (group_id, track_id, is_preferred)
                     VALUES (?1, ?2, 0)",
                    params![gid, rows[idx].id],
                )?;
            }
            mark_preferred(conn, gid)?;
            title_matches += 1;
        }
    }

    let groups_found = isrc_matches + title_matches;
    let tracks_affected = conn.query_row(
        "SELECT COUNT(DISTINCT dm.track_id)
         FROM duplicate_members dm
         JOIN duplicate_groups dg ON dg.id = dm.group_id
         WHERE dg.status = 'pending'",
        [],
        |row| row.get::<_, i64>(0),
    )? as usize;

    Ok(ScanStats {
        groups_found,
        tracks_affected,
        isrc_matches,
        title_matches,
    })
}

fn insert_group(conn: &Connection, status: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO duplicate_groups (status) VALUES (?1)",
        params![status],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Sets `is_preferred = 1` on the highest fidelity_score member of a group.
fn mark_preferred(conn: &Connection, group_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE duplicate_members SET is_preferred = 1
         WHERE group_id = ?1
           AND track_id = (
               SELECT dm.track_id
               FROM duplicate_members dm
               JOIN tracks t ON dm.track_id = t.id
               WHERE dm.group_id = ?1
               ORDER BY t.fidelity_score DESC, t.is_favorite DESC
               LIMIT 1
           )",
        params![group_id],
    )?;
    Ok(())
}

// ── Query ─────────────────────────────────────────────────────────────────────

pub fn count_pending_groups(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM duplicate_groups WHERE status = 'pending'",
        [],
        |row| row.get(0),
    )?)
}

/// Paginated list of pending duplicate groups with full track data.
pub fn load_groups(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<DuplicateGroup>> {
    let group_ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM duplicate_groups WHERE status = 'pending'
             ORDER BY id ASC LIMIT ?1 OFFSET ?2",
        )?;
        stmt.query_map(params![limit, offset], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect()
    };

    let mut groups = Vec::new();
    for gid in group_ids {
        let members = load_members(conn, gid)?;
        // Determine match reason: if any two members share an ISRC, it's isrc.
        let has_isrc = members.iter().any(|m| m.track.isrc.is_some());
        let match_reason = if has_isrc {
            "isrc".to_string()
        } else {
            "title_duration".to_string()
        };
        let members: Vec<DuplicateMember> = members
            .into_iter()
            .map(|mut m| {
                m.match_reason = match_reason.clone();
                m
            })
            .collect();
        groups.push(DuplicateGroup {
            id: gid,
            status: "pending".to_string(),
            members,
        });
    }
    Ok(groups)
}

fn load_members(conn: &Connection, group_id: i64) -> Result<Vec<DuplicateMember>> {
    let mut stmt = conn.prepare(
        "SELECT dm.is_preferred,
                t.id, t.title, t.artist_id, a.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM duplicate_members dm
         JOIN tracks t ON dm.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE dm.group_id = ?1
         ORDER BY t.fidelity_score DESC, t.is_favorite DESC",
    )?;

    let members = stmt
        .query_map(params![group_id], |row| {
            Ok(DuplicateMember {
                is_preferred: row.get::<_, i32>(0)? != 0,
                match_reason: String::new(),
                track: Track {
                    id: row.get(1)?,
                    title: row.get(2)?,
                    artist_id: row.get(3)?,
                    artist_name: row.get(4)?,
                    album_id: row.get(5)?,
                    album_title: row.get(6)?,
                    disc_number: row.get(7)?,
                    track_number: row.get(8)?,
                    duration_ms: row.get(9)?,
                    isrc: row.get(10)?,
                    tidal_id: row.get(11)?,
                    ytmusic_id: row.get(12)?,
                    soundcloud_id: row.get(13)?,
                    best_quality: row.get(14)?,
                    best_source: row.get(15)?,
                    fidelity_score: row.get(16)?,
                    is_favorite: row.get::<_, i32>(17)? != 0,
                    play_count: row.get(18)?,
                    last_played_at: row.get(19)?,
                    date_added: row.get(20)?,
                    source: row.get(21)?,
                    artwork_url: row.get(22)?,
                },
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(members)
}

// ── Resolve ───────────────────────────────────────────────────────────────────

pub struct ResolveResult {
    /// IDs of tracks that were removed from the DB.
    pub removed_track_ids: Vec<i64>,
    /// TIDAL IDs of tracks to unfavorite via API (caller handles the API call).
    pub tidal_ids_to_unfavorite: Vec<i64>,
    /// Outcome of queue + playback_state reconciliation after the deletes.
    /// Caller broadcasts events based on these flags.
    pub reconcile: ReconcileOutcome,
}

/// Keep `preferred_track_id`, dismiss or delete the rest.
/// Marks the group as 'resolved'.
pub fn resolve_group(
    conn: &Connection,
    group_id: i64,
    preferred_track_id: i64,
) -> Result<ResolveResult> {
    // Update preferred flag.
    conn.execute(
        "UPDATE duplicate_members SET is_preferred = 0 WHERE group_id = ?1",
        params![group_id],
    )?;
    conn.execute(
        "UPDATE duplicate_members SET is_preferred = 1
         WHERE group_id = ?1 AND track_id = ?2",
        params![group_id, preferred_track_id],
    )?;

    // Collect non-preferred members.
    let mut stmt = conn.prepare(
        "SELECT dm.track_id, t.tidal_id
         FROM duplicate_members dm
         JOIN tracks t ON dm.track_id = t.id
         WHERE dm.group_id = ?1 AND dm.is_preferred = 0",
    )?;
    let to_remove: Vec<(i64, Option<i64>)> = stmt
        .query_map(params![group_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let removed_track_ids: Vec<i64> = to_remove.iter().map(|(id, _)| *id).collect();
    let tidal_ids_to_unfavorite: Vec<i64> = to_remove.iter().filter_map(|(_, tid)| *tid).collect();

    // Reconcile queue + playback_state BEFORE deleting the tracks themselves.
    // The helper drops affected queue rows, advances current_track_id /
    // current_queue_item_id to the next surviving row, or stops playback if
    // no survivor exists. Hand-rolled `DELETE FROM queue` here would leave
    // playback_state.current_track_id pointing at a row we're about to drop.
    let reconcile = crate::playback::player::reconcile_after_track_delete(conn, &removed_track_ids)?;

    // Remove non-preferred tracks from the DB.
    // Must clean up dependent rows explicitly since FKs lack ON DELETE CASCADE.
    // Queue cleanup is handled above by reconcile_after_track_delete.
    for &track_id in &removed_track_ids {
        conn.execute(
            "DELETE FROM listen_history WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM playlist_tracks WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM shuffle_state WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute(
            "DELETE FROM duplicate_members WHERE track_id = ?1",
            params![track_id],
        )?;
        // track_genres already has ON DELETE CASCADE, but we delete it explicitly for clarity.
        conn.execute(
            "DELETE FROM track_genres WHERE track_id = ?1",
            params![track_id],
        )?;
        conn.execute("DELETE FROM tracks WHERE id = ?1", params![track_id])?;
    }

    // Mark group resolved.
    conn.execute(
        "UPDATE duplicate_groups SET status = 'resolved' WHERE id = ?1",
        params![group_id],
    )?;

    Ok(ResolveResult {
        removed_track_ids,
        tidal_ids_to_unfavorite,
        reconcile,
    })
}

/// Mark group as dismissed without deleting anything.
pub fn dismiss_group(conn: &Connection, group_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE duplicate_groups SET status = 'dismissed' WHERE id = ?1",
        params![group_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open test db");
        conn.execute_batch(
            "
            CREATE TABLE artists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE TABLE albums (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artwork_url TEXT
            );

            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                disc_number INTEGER,
                track_number INTEGER,
                duration_ms INTEGER,
                isrc TEXT,
                tidal_id INTEGER,
                ytmusic_id TEXT,
                soundcloud_id INTEGER,
                best_quality TEXT,
                best_source TEXT,
                fidelity_score INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played_at TEXT,
                date_added TEXT,
                source TEXT NOT NULL DEFAULT 'tidal'
            );

            CREATE TABLE duplicate_groups (
                id INTEGER PRIMARY KEY,
                status TEXT DEFAULT 'pending'
            );

            CREATE TABLE duplicate_members (
                group_id INTEGER NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
                track_id INTEGER NOT NULL REFERENCES tracks(id),
                is_preferred INTEGER DEFAULT 0,
                PRIMARY KEY (group_id, track_id)
            );

            CREATE TABLE listen_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                duration_listened_ms INTEGER DEFAULT 0,
                completed INTEGER DEFAULT 0
            );

            CREATE TABLE playlist_tracks (
                playlist_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                position INTEGER NOT NULL
            );

            CREATE TABLE queue (
                id               INTEGER PRIMARY KEY,
                track_id         INTEGER,
                position         INTEGER NOT NULL,
                source           TEXT    DEFAULT 'user',
                reason           TEXT,
                pending_artist   TEXT,
                pending_title    TEXT,
                pending_at       TIMESTAMP,
                resolving_at     TIMESTAMP,
                resolved_at      TIMESTAMP,
                tidal_match_score REAL,
                tidal_id_hint    INTEGER
            );

            CREATE TABLE shuffle_state (
                track_id INTEGER PRIMARY KEY,
                position INTEGER NOT NULL
            );

            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT,
                confidence REAL DEFAULT 1.0
            );

            CREATE TABLE playback_state (
                id INTEGER PRIMARY KEY,
                current_track_id INTEGER,
                current_queue_item_id INTEGER,
                position_ms INTEGER NOT NULL DEFAULT 0,
                is_playing INTEGER NOT NULL DEFAULT 0,
                volume REAL NOT NULL DEFAULT 1.0,
                shuffle_mode TEXT NOT NULL DEFAULT 'off',
                repeat_mode TEXT NOT NULL DEFAULT 'off',
                automix_enabled INTEGER NOT NULL DEFAULT 0,
                crossfade_ms INTEGER NOT NULL DEFAULT 0,
                automix_discover_new INTEGER NOT NULL DEFAULT 0,
                automix_use_learning INTEGER NOT NULL DEFAULT 1,
                automix_allow_external INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .expect("create schema");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Test Artist')",
            [],
        )
        .expect("insert artist");
        conn.execute(
            "INSERT INTO playback_state (
                id, current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms
            ) VALUES (1, NULL, 0, 0, 1.0, 'off', 'off', 0, 0)",
            [],
        )
        .expect("seed playback_state");
        conn
    }

    fn insert_track(conn: &Connection, id: i64, title: &str, duration_ms: i64, isrc: Option<&str>) {
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, duration_ms, isrc, fidelity_score, source
             ) VALUES (?1, ?2, 1, ?3, ?4, 100, 'tidal')",
            params![id, title, duration_ms, isrc],
        )
        .expect("insert track");
    }

    #[test]
    fn scan_rejects_shared_isrc_with_large_duration_gap() {
        let conn = test_conn();
        insert_track(&conn, 1, "My Barn My Rules", 127_000, Some("DEU672200178"));
        insert_track(&conn, 2, "My Barn My Rules", 266_000, Some("DEU672200178"));

        let stats = scan(&conn).expect("scan duplicates");

        assert_eq!(stats.groups_found, 0);
        assert_eq!(stats.tracks_affected, 0);
    }

    #[test]
    fn scan_rejects_remix_vs_original_even_with_shared_isrc() {
        let conn = test_conn();
        insert_track(
            &conn,
            1,
            "Tarlabasi (Be Svendsen Remix)",
            546_000,
            Some("DEHM81600158"),
        );
        insert_track(&conn, 2, "Tarlabasi", 545_000, Some("DEHM81600158"));

        let stats = scan(&conn).expect("scan duplicates");

        assert_eq!(stats.groups_found, 0);
        assert_eq!(stats.tracks_affected, 0);
    }

    #[test]
    fn scan_keeps_feature_credit_variants_as_duplicates() {
        let conn = test_conn();
        insert_track(
            &conn,
            1,
            "Cachaca (feat. Tom Scott)",
            291_000,
            Some("AUI441600195"),
        );
        insert_track(&conn, 2, "Cachaca", 290_000, Some("AUI441600195"));

        let stats = scan(&conn).expect("scan duplicates");
        let groups = load_groups(&conn, 10, 0).expect("load duplicate groups");

        assert_eq!(stats.groups_found, 1);
        assert_eq!(stats.tracks_affected, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
    }

    #[test]
    fn scan_counts_only_pending_tracks_in_stats() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO duplicate_groups (id, status) VALUES (99, 'resolved')",
            [],
        )
        .expect("insert resolved group");
        insert_track(&conn, 1, "Only Old Group", 180_000, Some("OLDISRC"));
        conn.execute(
            "INSERT INTO duplicate_members (group_id, track_id, is_preferred) VALUES (99, 1, 1)",
            [],
        )
        .expect("insert resolved membership");

        let stats = scan(&conn).expect("scan duplicates");

        assert_eq!(stats.groups_found, 0);
        assert_eq!(stats.tracks_affected, 0);
    }
}
