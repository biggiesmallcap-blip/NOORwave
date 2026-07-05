use super::models::*;
use crate::services::discovery::DiscoveryCandidateSeed;
use anyhow::{Result, bail};
use rusqlite::{
    Connection, OptionalExtension, Row, params, params_from_iter, types::Value as SqlValue,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const DISCOVERY_ENGINE_V2: &str = "v2";
pub const DISCOVERY_ENGINE_V1: &str = "v1";
pub const DISCOVERY_ENGINE_V2_FAMILY: &str = "discovery-fusion-v2";
pub const DISCOVERY_ENGINE_V1_FAMILY: &str = "discovery-fusion";

#[derive(Debug, Clone, Serialize)]
pub struct ChartSnapshotSummary {
    pub id: i64,
    pub source_key: String,
    pub region: String,
    pub period: String,
    pub chart_date: String,
    pub fetched_at: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartSnapshotEntryRow {
    pub id: i64,
    pub rank: i64,
    pub rank_delta: Option<i64>,
    pub artist: String,
    pub title: String,
    pub entity_type: String,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub external_track_id: Option<String>,
    pub external_artist_id: Option<String>,
    pub external_video_id: Option<String>,
    pub external_url: Option<String>,
    pub streams: Option<i64>,
    pub stream_delta: Option<i64>,
    pub views: Option<i64>,
    pub likes: Option<i64>,
    pub audience: Option<f64>,
    pub audience_delta: Option<f64>,
    pub points: Option<f64>,
    pub points_delta: Option<f64>,
    pub seven_day_streams: Option<i64>,
    pub total_streams: Option<i64>,
    pub days_on_chart: Option<i64>,
    pub peak_rank: Option<i64>,
    pub provider_positions_json: Option<Value>,
    pub raw_json: Option<Value>,
    pub external_candidate_id: Option<i64>,
    pub local_track_id: Option<i64>,
    pub tidal_id: Option<i64>,
    pub resolution_status: String,
    pub resolution_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatestChartSnapshot {
    pub snapshot: ChartSnapshotSummary,
    pub entries: Vec<ChartSnapshotEntryRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartMatrixCell {
    pub snapshot_id: i64,
    pub entry_id: i64,
    pub source_key: String,
    pub region: String,
    pub chart_date: String,
    pub rank: i64,
    pub rank_delta: Option<i64>,
    pub artist: String,
    pub title: String,
    pub entity_type: String,
    pub artwork_url: Option<String>,
    pub streams: Option<i64>,
    pub views: Option<i64>,
    pub points: Option<f64>,
    pub external_url: Option<String>,
    pub tidal_id: Option<i64>,
    pub resolution_status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartMatrixRow {
    pub region: String,
    pub cells: BTreeMap<String, Option<ChartMatrixCell>>,
}

#[derive(Debug, Clone)]
pub struct ChartSnapshotSeed<'a> {
    pub source_key: &'a str,
    pub region: &'a str,
    pub period: &'a str,
    pub chart_date: &'a str,
    pub fetched_at: i64,
    pub etag: Option<&'a str>,
    pub content_hash: Option<&'a str>,
    pub status: &'a str,
}

#[derive(Debug, Clone)]
pub struct ChartEntrySeed<'a> {
    pub rank: i64,
    pub rank_delta: Option<i64>,
    pub artist: &'a str,
    pub title: &'a str,
    pub entity_type: &'a str,
    pub album: Option<&'a str>,
    pub artwork_url: Option<&'a str>,
    pub external_track_id: Option<&'a str>,
    pub external_artist_id: Option<&'a str>,
    pub external_video_id: Option<&'a str>,
    pub external_url: Option<&'a str>,
    pub streams: Option<i64>,
    pub stream_delta: Option<i64>,
    pub views: Option<i64>,
    pub likes: Option<i64>,
    pub audience: Option<f64>,
    pub audience_delta: Option<f64>,
    pub points: Option<f64>,
    pub points_delta: Option<f64>,
    pub seven_day_streams: Option<i64>,
    pub total_streams: Option<i64>,
    pub days_on_chart: Option<i64>,
    pub peak_rank: Option<i64>,
    pub provider_positions_json: Option<Value>,
    pub raw_json: Option<Value>,
    pub resolution_status: Option<&'a str>,
    pub tidal_id: Option<i64>,
    pub local_track_id: Option<i64>,
    pub external_candidate_id: Option<i64>,
    pub resolution_score: Option<f64>,
}

impl<'a> ChartEntrySeed<'a> {
    pub fn track(rank: i64, artist: &'a str, title: &'a str) -> Self {
        Self {
            rank,
            rank_delta: None,
            artist,
            title,
            entity_type: "track",
            album: None,
            artwork_url: None,
            external_track_id: None,
            external_artist_id: None,
            external_video_id: None,
            external_url: None,
            streams: None,
            stream_delta: None,
            views: None,
            likes: None,
            audience: None,
            audience_delta: None,
            points: None,
            points_delta: None,
            seven_day_streams: None,
            total_streams: None,
            days_on_chart: None,
            peak_rank: None,
            provider_positions_json: None,
            raw_json: None,
            resolution_status: None,
            tidal_id: None,
            local_track_id: None,
            external_candidate_id: None,
            resolution_score: None,
        }
    }
}

pub fn upsert_chart_snapshot(
    conn: &Connection,
    snapshot: &ChartSnapshotSeed<'_>,
    entries: &[ChartEntrySeed<'_>],
) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO chart_snapshots
            (source_key, region, period, chart_date, fetched_at, etag, content_hash, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(source_key, region, period, chart_date)
         DO UPDATE SET
            fetched_at = excluded.fetched_at,
            etag = excluded.etag,
            content_hash = excluded.content_hash,
            status = excluded.status",
        params![
            snapshot.source_key,
            snapshot.region,
            snapshot.period,
            snapshot.chart_date,
            snapshot.fetched_at,
            snapshot.etag,
            snapshot.content_hash,
            snapshot.status,
        ],
    )?;
    let snapshot_id: i64 = tx.query_row(
        "SELECT id FROM chart_snapshots
         WHERE source_key = ?1 AND region = ?2 AND period = ?3 AND chart_date = ?4",
        params![
            snapshot.source_key,
            snapshot.region,
            snapshot.period,
            snapshot.chart_date,
        ],
        |row| row.get(0),
    )?;
    tx.execute(
        "DELETE FROM chart_entries WHERE snapshot_id = ?1",
        params![snapshot_id],
    )?;

    for entry in entries {
        tx.execute(
            "INSERT INTO chart_entries
                (snapshot_id, rank, rank_delta, artist, title, entity_type, album, artwork_url,
                 external_track_id, external_artist_id, external_video_id, external_url,
                 streams, stream_delta, views, likes, audience, audience_delta, points,
                 points_delta, seven_day_streams, total_streams, days_on_chart, peak_rank,
                 provider_positions_json, raw_json)
             VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            params![
                snapshot_id,
                entry.rank,
                entry.rank_delta,
                entry.artist,
                entry.title,
                entry.entity_type,
                entry.album,
                entry.artwork_url,
                entry.external_track_id,
                entry.external_artist_id,
                entry.external_video_id,
                entry.external_url,
                entry.streams,
                entry.stream_delta,
                entry.views,
                entry.likes,
                entry.audience,
                entry.audience_delta,
                entry.points,
                entry.points_delta,
                entry.seven_day_streams,
                entry.total_streams,
                entry.days_on_chart,
                entry.peak_rank,
                entry.provider_positions_json,
                entry.raw_json,
            ],
        )?;
        let entry_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO chart_entry_resolutions
                (entry_id, external_candidate_id, local_track_id, tidal_id, status, score)
             VALUES (?1, ?2, ?3, NULLIF(?4, 0), ?5, ?6)",
            params![
                entry_id,
                entry.external_candidate_id,
                entry.local_track_id,
                entry.tidal_id,
                entry.resolution_status.unwrap_or("unresolved"),
                entry.resolution_score,
            ],
        )?;
    }

    tx.commit()?;
    Ok(snapshot_id)
}

pub fn get_latest_chart_snapshot(
    conn: &Connection,
    source_key: &str,
    region: &str,
    period: &str,
    limit: u32,
) -> Result<Option<LatestChartSnapshot>> {
    let snapshot = conn
        .query_row(
            "SELECT id, source_key, region, period, chart_date, fetched_at, status
             FROM chart_snapshots
             WHERE source_key = ?1 AND region = ?2 AND period = ?3
             ORDER BY chart_date DESC, fetched_at DESC, id DESC
             LIMIT 1",
            params![source_key, region, period],
            |row| {
                Ok(ChartSnapshotSummary {
                    id: row.get(0)?,
                    source_key: row.get(1)?,
                    region: row.get(2)?,
                    period: row.get(3)?,
                    chart_date: row.get(4)?,
                    fetched_at: row.get(5)?,
                    status: row.get(6)?,
                })
            },
        )
        .optional()?;

    let Some(snapshot) = snapshot else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        "SELECT
            e.id, e.rank, e.rank_delta, e.artist, e.title, e.entity_type,
            e.album, e.artwork_url, e.external_track_id, e.external_artist_id,
            e.external_video_id, e.external_url, e.streams, e.stream_delta,
            e.views, e.likes, e.audience, e.audience_delta, e.points,
            e.points_delta, e.seven_day_streams, e.total_streams,
            e.days_on_chart, e.peak_rank, e.provider_positions_json, e.raw_json,
            r.external_candidate_id, r.local_track_id, NULLIF(r.tidal_id, 0),
            COALESCE(r.status, 'unresolved'), r.score
         FROM chart_entries e
         LEFT JOIN chart_entry_resolutions r ON r.entry_id = e.id
         WHERE e.snapshot_id = ?1
         ORDER BY e.rank ASC
         LIMIT ?2",
    )?;

    let entries = stmt
        .query_map(params![snapshot.id, limit], |row| {
            Ok(ChartSnapshotEntryRow {
                id: row.get(0)?,
                rank: row.get(1)?,
                rank_delta: row.get(2)?,
                artist: row.get(3)?,
                title: row.get(4)?,
                entity_type: row.get(5)?,
                album: row.get(6)?,
                artwork_url: row.get(7)?,
                external_track_id: row.get(8)?,
                external_artist_id: row.get(9)?,
                external_video_id: row.get(10)?,
                external_url: row.get(11)?,
                streams: row.get(12)?,
                stream_delta: row.get(13)?,
                views: row.get(14)?,
                likes: row.get(15)?,
                audience: row.get(16)?,
                audience_delta: row.get(17)?,
                points: row.get(18)?,
                points_delta: row.get(19)?,
                seven_day_streams: row.get(20)?,
                total_streams: row.get(21)?,
                days_on_chart: row.get(22)?,
                peak_rank: row.get(23)?,
                provider_positions_json: row.get(24)?,
                raw_json: row.get(25)?,
                external_candidate_id: row.get(26)?,
                local_track_id: row.get(27)?,
                tidal_id: row.get(28)?,
                resolution_status: row.get(29)?,
                resolution_score: row.get(30)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(Some(LatestChartSnapshot { snapshot, entries }))
}

pub fn get_chart_matrix(
    conn: &Connection,
    regions: &[&str],
    source_keys: &[&str],
    period: &str,
) -> Result<Vec<ChartMatrixRow>> {
    let mut stmt = conn.prepare(
        "SELECT
            s.id, e.id, s.source_key, s.region, s.chart_date,
            e.rank, e.rank_delta, e.artist, e.title, e.entity_type,
            e.artwork_url, e.streams, e.views, e.points, e.external_url,
            NULLIF(r.tidal_id, 0), COALESCE(r.status, 'unresolved')
         FROM chart_snapshots s
         JOIN chart_entries e ON e.snapshot_id = s.id AND e.rank = 1
         LEFT JOIN chart_entry_resolutions r ON r.entry_id = e.id
         WHERE s.source_key = ?1 AND s.region = ?2 AND s.period = ?3
         ORDER BY s.chart_date DESC, s.fetched_at DESC, s.id DESC
         LIMIT 1",
    )?;

    let mut rows = Vec::with_capacity(regions.len());
    for region in regions {
        let mut cells = BTreeMap::new();
        for source_key in source_keys {
            let cell = stmt
                .query_row(params![source_key, region, period], |row| {
                    Ok(ChartMatrixCell {
                        snapshot_id: row.get(0)?,
                        entry_id: row.get(1)?,
                        source_key: row.get(2)?,
                        region: row.get(3)?,
                        chart_date: row.get(4)?,
                        rank: row.get(5)?,
                        rank_delta: row.get(6)?,
                        artist: row.get(7)?,
                        title: row.get(8)?,
                        entity_type: row.get(9)?,
                        artwork_url: row.get(10)?,
                        streams: row.get(11)?,
                        views: row.get(12)?,
                        points: row.get(13)?,
                        external_url: row.get(14)?,
                        tidal_id: row.get(15)?,
                        resolution_status: row.get(16)?,
                    })
                })
                .optional()?;
            cells.insert((*source_key).to_string(), cell);
        }
        rows.push(ChartMatrixRow {
            region: (*region).to_string(),
            cells,
        });
    }

    Ok(rows)
}

// ─── Server Config ────────────────────────────────────────

pub fn ensure_server_token(conn: &Connection) -> Result<String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key='server_token'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    // Keep the existing token only if it matches the current 6-digit PIN format.
    // Legacy hex/word-phrase tokens are auto-upgraded on next startup.
    if let Some(token) = existing
        && is_valid_pin(&token)
    {
        return Ok(token);
    }

    let token = generate_readable_token();
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('server_token', ?1)",
        params![token],
    )?;
    Ok(token)
}

fn is_valid_pin(s: &str) -> bool {
    s.len() == 6 && s.chars().all(|c| c.is_ascii_digit())
}

pub fn regenerate_server_token(conn: &Connection) -> Result<String> {
    let token = generate_readable_token();
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('server_token', ?1)",
        params![token],
    )?;
    Ok(token)
}

/// First-run onboarding flag. When the row is missing, treat an existing
/// `service_auth` TIDAL row as implicit completion and persist the flag —
/// this keeps users upgrading from earlier versions out of the onboarding
/// flow they've effectively already done.
pub fn get_onboarding_complete(conn: &Connection) -> Result<bool> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key='onboarding_complete'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(value) = stored {
        return Ok(value == "1");
    }

    let has_tidal: bool = conn
        .query_row(
            "SELECT 1 FROM service_auth WHERE service='tidal' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if has_tidal {
        set_onboarding_complete(conn)?;
        return Ok(true);
    }

    Ok(false)
}

pub fn set_onboarding_complete(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('onboarding_complete', '1')",
        [],
    )?;
    Ok(())
}

fn generate_readable_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    let n = u32::from_le_bytes(bytes) % 1_000_000;
    format!("{:06}", n)
}

// ─── Tracks ───────────────────────────────────────────────

/// Optional DSP filters for get_tracks_with_dsp()
#[derive(Debug, Clone, Default)]
pub struct DspFilters {
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub energy_min: Option<f64>,
    pub energy_max: Option<f64>,
    pub key_signature: Option<String>,
    pub instrumental_only: bool,
}

// Single source of truth for the favorite/liked WHERE predicate.
// Used by both get_tracks_with_dsp and get_track_count so they cannot drift.
//
// `favorite_only` is legacy naming: it currently means "library tracks" =
// tracks where tracks.is_favorite=1 OR the parent album has albums.is_favorite=1.
// `liked_only` is the strict "user explicitly liked this track" filter.
// liked_only takes precedence over favorite_only.
//
// All callers must alias the tracks table as `t` for this predicate to apply.
fn favorite_predicate(favorite_only: bool, liked_only: bool) -> Option<&'static str> {
    if liked_only {
        Some("t.is_favorite = 1")
    } else if favorite_only {
        // The album-favorite branch is gated on `is_library = 1` so transient
        // resolver/discovery imports that happen to land in a favorited album
        // don't leak into the library. See MIGRATION_052 and the canonical
        // sibling `ARTIST_LIBRARY_TRACK_WHERE` (keep both in sync).
        Some(
            "(t.is_favorite = 1 OR (t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1) AND t.is_library = 1))",
        )
    } else {
        None
    }
}

fn track_order_clause(sort_by: &str, sort_dir: &str) -> String {
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };
    match sort_by {
        // True random sample across the whole matching set. Direction is
        // meaningless for RANDOM(), so it's ignored. Used by the library
        // Shuffle button so the queue isn't stuck reordering the newest 200
        // rows; SQLite draws a fresh sample on every request.
        "random" => "RANDOM()".to_string(),
        "title" => format!("t.title {dir}"),
        "artist" => format!("a_artists.name {dir}"),
        "album" => format!("al.title {dir}"),
        "year" => format!("al.year {dir}"),
        "date_added" => format!("t.date_added {dir}, t.id {dir}"),
        "duration" => format!("t.duration_ms {dir}"),
        "play_count" => format!("t.play_count {dir}"),
        "fidelity" => format!("t.fidelity_score {dir}"),
        "bpm" => format!("COALESCE(a.bpm, 0) {dir}"),
        "energy" => format!("COALESCE(a.energy, 0) {dir}"),
        "danceability" => format!("COALESCE(a.danceability, 0) {dir}"),
        "last_played_at" => format!("COALESCE(t.last_played_at, '') {dir}"),
        _ => format!("t.date_added {dir}, t.id {dir}"),
    }
}

/// The 22-column track projection shared by every query whose rows are mapped
/// by `track_from_row`. Pass the alias the query uses for the joined `artists`
/// table: `a` everywhere except `get_tracks_with_dsp`, which joins
/// `audio_dsp_features` as `a` and so must alias artists as `a_artists`.
///
/// Column ORDER is load-bearing: `track_from_row` reads by index, and
/// `search_tracks_fts` does positional ORDER BY against these columns. Only
/// ever append a column here, and update `track_from_row` to match.
fn track_projection(artist_alias: &str) -> String {
    format!(
        "t.id, t.title, t.artist_id, {artist_alias}.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url"
    )
}

pub fn get_tracks(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
    liked_only: bool,
) -> Result<Vec<Track>> {
    get_tracks_with_dsp(
        conn,
        sort_by,
        sort_dir,
        limit,
        offset,
        favorite_only,
        liked_only,
        &DspFilters::default(),
    )
}

pub fn get_tracks_with_dsp(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
    liked_only: bool,
    dsp: &DspFilters,
) -> Result<Vec<Track>> {
    let has_dsp = dsp.bpm_min.is_some()
        || dsp.bpm_max.is_some()
        || dsp.energy_min.is_some()
        || dsp.energy_max.is_some()
        || dsp.key_signature.is_some()
        || dsp.instrumental_only;

    let order_clause = track_order_clause(sort_by, sort_dir);

    let mut conditions = Vec::new();
    if let Some(pred) = favorite_predicate(favorite_only, liked_only) {
        conditions.push(pred.to_string());
    }

    let join_clause = if has_dsp {
        " LEFT JOIN audio_dsp_features a ON t.id = a.track_id"
    } else {
        ""
    };

    let mut bind_values = Vec::new();
    if let Some(min) = dsp.bpm_min {
        bind_values.push(SqlValue::Real(min));
        conditions.push(format!("a.bpm >= ?{}", bind_values.len()));
    }
    if let Some(max) = dsp.bpm_max {
        bind_values.push(SqlValue::Real(max));
        conditions.push(format!("a.bpm <= ?{}", bind_values.len()));
    }
    if let Some(min) = dsp.energy_min {
        bind_values.push(SqlValue::Real(min));
        conditions.push(format!("a.energy >= ?{}", bind_values.len()));
    }
    if let Some(max) = dsp.energy_max {
        bind_values.push(SqlValue::Real(max));
        conditions.push(format!("a.energy <= ?{}", bind_values.len()));
    }
    if let Some(ref key) = dsp.key_signature {
        bind_values.push(SqlValue::Text(key.clone()));
        conditions.push(format!("a.key_signature = ?{}", bind_values.len()));
    }
    if dsp.instrumental_only {
        conditions.push("a.is_instrumental = 1".to_string());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let projection = track_projection("a_artists");
    bind_values.push(SqlValue::Integer(limit));
    let limit_param = bind_values.len();
    bind_values.push(SqlValue::Integer(offset));
    let offset_param = bind_values.len();
    let sql = format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a_artists ON t.artist_id = a_artists.id
         LEFT JOIN albums al ON t.album_id = al.id
         {join_clause}
         {where_clause}
         ORDER BY {order_clause}
         LIMIT ?{limit_param} OFFSET ?{offset_param}"
    );

    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params_from_iter(bind_values.iter()), track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_track_count(conn: &Connection, favorite_only: bool, liked_only: bool) -> Result<i64> {
    // FROM tracks t alias is required so favorite_predicate's "t."-prefixed SQL applies.
    let filter = match favorite_predicate(favorite_only, liked_only) {
        Some(pred) => format!(" WHERE {pred}"),
        None => String::new(),
    };
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM tracks t{filter}"),
        [],
        |row| row.get(0),
    )?)
}

/// Play history collapsed to one row per track, most-recently-played first.
/// Unlike `get_tracks(favorite_only=true)` (which powers the library "Recent
/// Tracks" shelf), this reflects what was actually *played*: radio, discover,
/// and other external tracks that were imported into `tracks` on play but never
/// favorited surface here too. `listen_history.track_id` is a NOT NULL FK to
/// `tracks`, so an external track appears once its first listen was recorded.
///
/// GROUP BY collapses repeat plays; `MAX(lh.started_at)` is the ordering key.
/// The bare `t.*` columns in the projection are safe under GROUP BY because the
/// join keys them all to the single track behind `lh.track_id`.
pub fn get_listen_history_tracks(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<Track>> {
    let projection = track_projection("a");
    let sql = format!(
        "SELECT {projection}, MAX(lh.started_at) AS last_listen
         FROM listen_history lh
         JOIN tracks t ON t.id = lh.track_id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         GROUP BY lh.track_id
         ORDER BY last_listen DESC
         LIMIT ?1 OFFSET ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![limit, offset], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

/// Count of distinct tracks that appear in play history (pagination total for
/// [`get_listen_history_tracks`]).
pub fn get_listen_history_track_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(DISTINCT track_id) FROM listen_history",
        [],
        |row| row.get(0),
    )?)
}

// ─── Albums ───────────────────────────────────────────────

pub fn get_albums(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
) -> Result<Vec<Album>> {
    let order_col = match sort_by {
        "title" => "al.title",
        "artist" => "a.name",
        "year" => "al.year",
        _ => "al.title",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let fav_filter = if favorite_only {
        " WHERE al.is_favorite = 1"
    } else {
        ""
    };

    let sql = format!(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name as artist_name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         {fav_filter}
         ORDER BY {order_col} {dir}
         LIMIT ?1 OFFSET ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let albums = stmt
        .query_map(params![limit, offset], |row| {
            Ok(Album {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                title: row.get(3)?,
                artist_id: row.get(4)?,
                artist_name: row.get(5)?,
                year: row.get(6)?,
                artwork_url: row.get(7)?,
                release_type: row.get(8)?,
                label: row.get(9)?,
                track_count: row.get(10)?,
                is_favorite: row.get::<_, i32>(11)? != 0,
                source: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(albums)
}

pub fn get_album_count(conn: &Connection, favorite_only: bool) -> Result<i64> {
    let filter = if favorite_only {
        " WHERE is_favorite = 1"
    } else {
        ""
    };
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM albums{filter}"), [], |row| {
            row.get(0)
        })?,
    )
}

pub fn get_album_tracks(conn: &Connection, album_id: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.album_id = ?1
         ORDER BY
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC",
        track_projection("a")
    ))?;

    let tracks = stmt
        .query_map(params![album_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

// ─── Artists ──────────────────────────────────────────────

// ─── Spotify public stats (richer cache reads/writes) ─────

#[derive(Debug, Clone, Default)]
pub struct CachedTrackStatsRow {
    pub spotify_track_id: Option<String>,
    pub playcount: Option<i64>,
    pub stats_fetched_at: Option<i64>,
    pub null_cached_at: Option<i64>,
}

/// Read raw spotify cache state for a batch of ISRCs.
///
/// Returns one row per *input* ISRC (including unknowns, so callers can tell
/// "never seen" apart from "negative-cached"). TTL policy is the caller's
/// responsibility - this is intentionally just a window into the tables.
pub fn get_cached_spotify_track_stats_for_isrcs(
    conn: &Connection,
    isrcs: &[String],
) -> Result<HashMap<String, CachedTrackStatsRow>> {
    const CHUNK: usize = 500;
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for isrc in isrcs {
        let trimmed = isrc.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        keys.push(trimmed.to_string());
    }
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut out: HashMap<String, CachedTrackStatsRow> = keys
        .iter()
        .map(|k| (k.clone(), CachedTrackStatsRow::default()))
        .collect();

    for chunk in keys.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(",");

        // ISRC -> spotify_track_id + (optional) playcount/fetched_at via LEFT JOIN
        let sql = format!(
            "SELECT m.isrc, m.spotify_track_id, s.playcount, s.fetched_at
             FROM spotify_isrc_map m
             LEFT JOIN spotify_track_stats s ON s.spotify_track_id = m.spotify_track_id
             WHERE m.isrc IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        for r in rows {
            let (isrc, tid, pc, fa) = r?;
            if let Some(slot) = out.get_mut(&isrc) {
                slot.spotify_track_id = Some(tid);
                slot.playcount = pc;
                slot.stats_fetched_at = fa;
            }
        }

        // Negative cache lookup (no join: spotify_null_cache is keyed by ISRC).
        let null_sql = format!(
            "SELECT isrc, cached_at FROM spotify_null_cache WHERE isrc IN ({placeholders})"
        );
        let mut null_stmt = conn.prepare(&null_sql)?;
        let null_rows = null_stmt.query_map(params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for r in null_rows {
            let (isrc, cached_at) = r?;
            if let Some(slot) = out.get_mut(&isrc) {
                slot.null_cached_at = Some(cached_at);
            }
        }
    }

    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CachedArtistStatsRow {
    pub monthly_listeners: Option<i64>,
    pub followers: Option<i64>,
    pub world_rank: Option<i64>,
    pub top_cities_json: Option<String>,
    pub fetched_at: i64,
}

pub fn get_cached_spotify_artist_stats(
    conn: &Connection,
    spotify_artist_id: &str,
) -> Result<Option<CachedArtistStatsRow>> {
    let row = conn
        .query_row(
            "SELECT monthly_listeners, followers, world_rank, top_cities_json, fetched_at
             FROM spotify_artist_stats
             WHERE spotify_artist_id = ?1",
            params![spotify_artist_id],
            |row| {
                Ok(CachedArtistStatsRow {
                    monthly_listeners: row.get(0)?,
                    followers: row.get(1)?,
                    world_rank: row.get(2)?,
                    top_cities_json: row.get(3)?,
                    fetched_at: row.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Returns `(Option<spotify_artist_id>, resolved_at)`. `Some(None)` (i.e. row
/// exists with NULL spotify_artist_id) means negative-cached.
pub fn get_spotify_artist_map(
    conn: &Connection,
    tidal_artist_id: &str,
) -> Result<Option<(Option<String>, i64)>> {
    let row = conn
        .query_row(
            "SELECT spotify_artist_id, resolved_at FROM spotify_artist_map
             WHERE tidal_artist_id = ?1",
            params![tidal_artist_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(row)
}

pub fn upsert_spotify_isrc_map(
    conn: &Connection,
    isrc: &str,
    spotify_track_id: &str,
    resolved_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_isrc_map (isrc, spotify_track_id, resolved_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(isrc) DO UPDATE SET
            spotify_track_id = excluded.spotify_track_id,
            resolved_at      = excluded.resolved_at",
        params![isrc, spotify_track_id, resolved_at],
    )?;
    Ok(())
}

/// Store playcount as-is (zero is preserved; the resolver may treat zero as
/// "retry sooner" but the cache layer doesn't second-guess the upstream).
pub fn upsert_spotify_track_stats(
    conn: &Connection,
    spotify_track_id: &str,
    playcount: i64,
    fetched_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_track_stats (spotify_track_id, playcount, fetched_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(spotify_track_id) DO UPDATE SET
            playcount  = excluded.playcount,
            fetched_at = excluded.fetched_at",
        params![spotify_track_id, playcount, fetched_at],
    )?;
    Ok(())
}

pub fn upsert_spotify_null_cache(conn: &Connection, isrc: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_null_cache (isrc, cached_at)
         VALUES (?1, ?2)
         ON CONFLICT(isrc) DO UPDATE SET cached_at = excluded.cached_at",
        params![isrc, now],
    )?;
    Ok(())
}

pub fn clear_spotify_null_cache(conn: &Connection, isrc: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM spotify_null_cache WHERE isrc = ?1",
        params![isrc],
    )?;
    Ok(())
}

pub fn upsert_spotify_artist_map(
    conn: &Connection,
    tidal_artist_id: &str,
    spotify_artist_id: Option<&str>,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_artist_map (tidal_artist_id, spotify_artist_id, resolved_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(tidal_artist_id) DO UPDATE SET
            spotify_artist_id = excluded.spotify_artist_id,
            resolved_at       = excluded.resolved_at",
        params![tidal_artist_id, spotify_artist_id, now],
    )?;
    Ok(())
}

/// Upsert artist stats; `COALESCE(excluded.col, col)` preserves any previously
/// known non-null value when the incoming fetch omitted that field (Spotify
/// drops `monthly_listeners` for some artists, but we don't want to forget a
/// number we'd already learned).
///
/// On a fresh INSERT this writes whatever the caller passes for each column,
/// including explicit NULL when the caller passes `None`. The schema declares
/// those columns nullable with no DEFAULT, so explicit-NULL and default-NULL
/// produce identical rows today. If a future migration adds a non-NULL DEFAULT
/// to any of these columns, callers that pass `None` will clobber that default
/// with NULL; the helper would then need a parallel "partial upsert" variant.
pub fn upsert_spotify_artist_stats(
    conn: &Connection,
    spotify_artist_id: &str,
    monthly_listeners: Option<i64>,
    followers: Option<i64>,
    world_rank: Option<i64>,
    top_cities_json: Option<&str>,
    fetched_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_artist_stats
            (spotify_artist_id, monthly_listeners, followers, world_rank, top_cities_json, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(spotify_artist_id) DO UPDATE SET
            monthly_listeners = COALESCE(excluded.monthly_listeners, spotify_artist_stats.monthly_listeners),
            followers         = COALESCE(excluded.followers,         spotify_artist_stats.followers),
            world_rank        = COALESCE(excluded.world_rank,        spotify_artist_stats.world_rank),
            top_cities_json   = COALESCE(excluded.top_cities_json,   spotify_artist_stats.top_cities_json),
            fetched_at        = excluded.fetched_at",
        params![
            spotify_artist_id,
            monthly_listeners,
            followers,
            world_rank,
            top_cities_json,
            fetched_at
        ],
    )?;
    Ok(())
}

// Canonical sibling of `favorite_predicate`'s favorite_only branch (hand-
// duplicated because the artist queries join `albums` as `al`). Keep the two in
// sync: the album-favorite branch is gated on `is_library = 1` so transient
// resolver/discovery imports don't leak into artist-detail library surfaces.
const ARTIST_LIBRARY_TRACK_WHERE: &str =
    "(t.is_favorite = 1 OR (COALESCE(al.is_favorite, 0) = 1 AND t.is_library = 1))";

fn artist_library_track_predicate() -> &'static str {
    ARTIST_LIBRARY_TRACK_WHERE
}

pub fn get_artists(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Artist>> {
    let order_col = match sort_by {
        "name" => "a.name",
        _ => "a.name",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let sql = format!(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         ORDER BY {order_col} {dir}
         LIMIT ?1 OFFSET ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let artists = stmt
        .query_map(params![limit, offset], |row| {
            Ok(Artist {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                soundcloud_id: row.get(3)?,
                name: row.get(4)?,
                name_sort: row.get(5)?,
                biography: row.get(6)?,
                photo_url: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(artists)
}

/// Single artist row plus library-side counts (tracks belonging to this
/// artist, distinct albums those tracks span). Counts reflect the local
/// library only. TIDAL-side totals come from the discography handler.
pub fn get_artist_with_counts(
    conn: &Connection,
    artist_id: i64,
) -> Result<Option<(Artist, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         WHERE a.id = ?1",
    )?;
    let artist = stmt
        .query_row(params![artist_id], |row| {
            Ok(Artist {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                soundcloud_id: row.get(3)?,
                name: row.get(4)?,
                name_sort: row.get(5)?,
                biography: row.get(6)?,
                photo_url: row.get(7)?,
            })
        })
        .optional()?;

    let Some(artist) = artist else {
        return Ok(None);
    };

    let track_count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM tracks t
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE t.artist_id = ?1 AND {}",
            artist_library_track_predicate()
        ),
        params![artist_id],
        |row| row.get(0),
    )?;
    let album_count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT t.album_id) FROM tracks t
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE t.artist_id = ?1
               AND t.album_id IS NOT NULL
               AND {}",
            artist_library_track_predicate()
        ),
        params![artist_id],
        |row| row.get(0),
    )?;

    Ok(Some((artist, track_count, album_count)))
}

pub fn get_artist_tidal_id(conn: &Connection, artist_id: i64) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT tidal_id FROM artists WHERE id = ?1")?;
    let tidal_id = stmt
        .query_row(params![artist_id], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten();
    Ok(tidal_id)
}

pub fn get_known_artist_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!("SELECT tidal_id, id FROM artists WHERE tidal_id IN ({placeholders})");
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (tidal_id, local_id) = row?;
        map.insert(tidal_id, local_id);
    }
    Ok(map)
}

pub fn get_known_album_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!("SELECT tidal_id, id FROM albums WHERE tidal_id IN ({placeholders})");
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (tidal_id, local_id) = row?;
        map.insert(tidal_id, local_id);
    }
    Ok(map)
}

fn get_artist_tracks_matching(
    conn: &Connection,
    artist_id: i64,
    extra_where: &str,
) -> Result<Vec<Track>> {
    let projection = track_projection("a");
    let sql = format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.artist_id = ?1{extra_where}
         ORDER BY
            al.year ASC,
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC"
    );
    let mut stmt = conn.prepare(&sql)?;

    let tracks = stmt
        .query_map(params![artist_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

// ─── Playlists ────────────────────────────────────────────

pub fn get_artist_tracks(conn: &Connection, artist_id: i64) -> Result<Vec<Track>> {
    get_artist_tracks_matching(conn, artist_id, "")
}

pub fn get_artist_library_tracks(conn: &Connection, artist_id: i64) -> Result<Vec<Track>> {
    get_artist_tracks_matching(
        conn,
        artist_id,
        &format!(" AND {}", artist_library_track_predicate()),
    )
}

pub fn get_playlists(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_uuid, name, description, is_smart,
                smart_rules, is_synced, track_count, is_favorite,
                created_at, updated_at
         FROM playlists
         ORDER BY is_favorite DESC, name ASC",
    )?;

    let playlists = stmt
        .query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                tidal_uuid: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                is_smart: row.get::<_, i32>(4)? != 0,
                smart_rules: row.get(5)?,
                is_synced: row.get::<_, i32>(6)? != 0,
                track_count: row.get(7)?,
                is_favorite: row.get::<_, i32>(8)? != 0,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(playlists)
}

pub fn get_playlist(conn: &Connection, playlist_id: i64) -> Result<Option<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_uuid, name, description, is_smart,
                smart_rules, is_synced, track_count, is_favorite,
                created_at, updated_at
         FROM playlists
         WHERE id = ?1",
    )?;

    let mut rows = stmt.query(params![playlist_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(Playlist {
            id: row.get(0)?,
            tidal_uuid: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            is_smart: row.get::<_, i32>(4)? != 0,
            smart_rules: row.get(5)?,
            is_synced: row.get::<_, i32>(6)? != 0,
            track_count: row.get(7)?,
            is_favorite: row.get::<_, i32>(8)? != 0,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn toggle_playlist_favorite(conn: &Connection, playlist_id: i64) -> Result<Playlist> {
    conn.execute(
        "UPDATE playlists SET is_favorite = NOT is_favorite WHERE id = ?1",
        params![playlist_id],
    )?;
    get_playlist(conn, playlist_id)?.ok_or_else(|| anyhow::anyhow!("playlist not found"))
}

/// Bulk-insert tracks into a playlist, skipping any already present.
/// Returns the number of tracks actually inserted.
pub fn add_tracks_to_playlist(
    conn: &Connection,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<usize> {
    if track_ids.is_empty() {
        return Ok(0);
    }

    // Find which tracks are already in the playlist
    let existing: std::collections::HashSet<i64> = {
        let mut stmt =
            conn.prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")?;
        stmt.query_map(params![playlist_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    };

    let to_insert: Vec<i64> = {
        let mut seen = std::collections::HashSet::new();
        track_ids
            .iter()
            .copied()
            .filter(|id| !existing.contains(id) && seen.insert(*id))
            .collect()
    };

    if to_insert.is_empty() {
        return Ok(0);
    }

    // Get the current max position
    let max_pos: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) FROM playlist_tracks WHERE playlist_id = ?1",
        params![playlist_id],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
    )?;
    for (i, &track_id) in to_insert.iter().enumerate() {
        stmt.execute(params![playlist_id, track_id, max_pos + 1 + i as i64])?;
    }

    // Keep track_count in sync and bump updated_at so "Recently updated"
    // sorts reflect content changes, not just smart-rule edits.
    conn.execute(
        "UPDATE playlists SET track_count = (
            SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1
         ),
         updated_at = datetime('now')
         WHERE id = ?1",
        params![playlist_id],
    )?;

    Ok(to_insert.len())
}

/// Up to `limit` distinct album-artwork URLs for a regular playlist, ordered
/// by the earliest position the URL appears at. Built for the `/cover-sample`
/// endpoint - returning four URLs as a JSON array is cheaper than evaluating
/// the playlist and discarding everything but the first four.
pub fn sample_playlist_artwork(
    conn: &Connection,
    playlist_id: i64,
    limit: i64,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT al.artwork_url, MIN(pt.position) AS first_pos
         FROM playlist_tracks pt
         JOIN tracks t ON pt.track_id = t.id
         JOIN albums al ON t.album_id = al.id
         WHERE pt.playlist_id = ?1 AND al.artwork_url IS NOT NULL
         GROUP BY al.artwork_url
         ORDER BY first_pos ASC
         LIMIT ?2",
    )?;
    let urls = stmt
        .query_map(params![playlist_id, limit], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(urls)
}

/// Lightweight artist-name search for autocomplete: returns `(id, name)` pairs
/// only. Reuses the FTS-then-LIKE fallback that powers the global `search`
/// endpoint so it picks up the same matches.
pub fn search_library_artist_names(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<(i64, String)>> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.max(1);
    let fts_query = to_fts_query(&normalized);
    let artists = search_artists_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_artists_like(conn, &normalized, limit).unwrap_or_default());
    Ok(artists.into_iter().map(|a| (a.id, a.name)).collect())
}

pub fn get_playlist_tracks(conn: &Connection, playlist_id: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM playlist_tracks pt
         JOIN tracks t ON pt.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE pt.playlist_id = ?1
         ORDER BY pt.position ASC",
        track_projection("a")
    ))?;

    let tracks = stmt
        .query_map(params![playlist_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_all_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY t.date_added DESC, t.id DESC",
        track_projection("a")
    ))?;

    let tracks = stmt
        .query_map([], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

/// Provenance of a [`ResolvedGenre`] — distinguishes ground-truth track-level
/// data from album/artist fallback rescues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenreSource {
    /// Direct row from `track_genres`.
    Track,
    /// Aggregated from sibling tracks on the same single-artist album.
    AlbumFallback,
    /// Aggregated from other tracks by the same artist.
    ArtistFallback,
}

impl GenreSource {
    fn from_sql_source(value: &str) -> Self {
        match value {
            "album_fallback" => GenreSource::AlbumFallback,
            "artist_fallback" => GenreSource::ArtistFallback,
            _ => GenreSource::Track,
        }
    }
}

/// One genre path string for a track, with provenance. Path is the same
/// `"Parent > Leaf"` shape `get_genres_for_tracks` returns.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ResolvedGenre {
    pub path: String,
    pub source: GenreSource,
}

impl ResolvedGenre {
    /// Adapter for callers that consume only the path strings (e.g.
    /// `weighted_genre_set` in the Phase-2b Jaccard scorer).
    pub fn paths_only(rows: &[ResolvedGenre]) -> Vec<String> {
        rows.iter().map(|r| r.path.clone()).collect()
    }
}

/// Variant of [`get_genres_for_tracks`] that fills empty tracks via
/// album-then-artist fallback. Tracks with at least one row in
/// `track_genres` are returned untouched at [`GenreSource::Track`]. Tracks
/// with no rows get rescued from siblings on the same single-artist album
/// ([`GenreSource::AlbumFallback`]) or, failing that, from other tracks by
/// the same artist ([`GenreSource::ArtistFallback`]). Multi-artist albums
/// (compilations) are skipped at the album tier to avoid cross-artist
/// contamination.
///
/// Top-[`crate::genre::filter::FALLBACK_ROWS_PER_TRACK`] fallback rows per
/// tier per track. Sibling rows are taken from `track_genres` directly
/// (the inner rule is `GalaxyFilterRule::All`) — Path A consumers (radio
/// Jaccard, JSON exports) want the full sibling material.
///
/// See the parent module docs and the `filter_subquery_with_fallback`
/// implementation for cascade semantics.
pub fn get_genres_for_tracks_with_fallback(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, Vec<ResolvedGenre>>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Use the per-track-narrowed cascade builder. Without it, every call would
    // enumerate all 35k tracks in `needs_fallback` and scan the whole
    // track_genres table for primary rows. The narrow form inlines an
    // IN(?,?,...) filter so SQLite touches only the requested track ids.
    let cascade = crate::genre::filter::filter_subquery_with_fallback_for_tracks(
        crate::genre::filter::GalaxyFilterRule::All,
        track_ids.len(),
    );
    let sql = format!(
        "WITH RECURSIVE genre_paths(id, parent_id, path) AS (
            SELECT id, parent_id, name
            FROM genres
            WHERE parent_id IS NULL
            UNION ALL
            SELECT g.id, g.parent_id, genre_paths.path || ' > ' || g.name
            FROM genres g
            JOIN genre_paths ON g.parent_id = genre_paths.id
        )
        SELECT cr.track_id, genre_paths.path, cr.source
        FROM ({cascade}) cr
        JOIN genre_paths ON genre_paths.id = cr.genre_id
        ORDER BY cr.track_id, genre_paths.path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params_iter = rusqlite::params_from_iter(track_ids.iter().copied());
    let mut rows = stmt.query(params_iter)?;

    let mut by_track: HashMap<i64, Vec<ResolvedGenre>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let track_id: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let src: String = row.get(2)?;
        by_track.entry(track_id).or_default().push(ResolvedGenre {
            path,
            source: GenreSource::from_sql_source(&src),
        });
    }

    Ok(by_track)
}

/// Whole-library variant of [`get_genres_for_tracks_with_fallback`]. Returns
/// the same `(track_id → Vec<ResolvedGenre>)` map for every track in the
/// library, including tracks rescued via album/artist fallback. Used by the
/// galaxy/discovery JSON-export endpoints.
///
/// More expensive than the per-track form — the cascade processes the
/// whole library. Profile via `EXPLAIN QUERY PLAN` if perf becomes a
/// concern.
pub fn get_track_genre_paths_with_fallback(
    conn: &Connection,
) -> Result<HashMap<i64, Vec<ResolvedGenre>>> {
    let cascade = crate::genre::filter::filter_subquery_with_fallback(
        crate::genre::filter::GalaxyFilterRule::All,
    );
    let sql = format!(
        "WITH RECURSIVE genre_paths(id, parent_id, path) AS (
            SELECT id, parent_id, name
            FROM genres
            WHERE parent_id IS NULL
            UNION ALL
            SELECT g.id, g.parent_id, genre_paths.path || ' > ' || g.name
            FROM genres g
            JOIN genre_paths ON g.parent_id = genre_paths.id
        )
        SELECT cr.track_id, genre_paths.path, cr.source
        FROM ({cascade}) cr
        JOIN genre_paths ON genre_paths.id = cr.genre_id
        ORDER BY cr.track_id, genre_paths.path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;

    let mut by_track: HashMap<i64, Vec<ResolvedGenre>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let track_id: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let src: String = row.get(2)?;
        by_track.entry(track_id).or_default().push(ResolvedGenre {
            path,
            source: GenreSource::from_sql_source(&src),
        });
    }

    Ok(by_track)
}

pub fn create_smart_playlist(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
    rules_json: &str,
) -> Result<Playlist> {
    conn.execute(
        "INSERT INTO playlists (name, description, is_smart, smart_rules, is_synced, track_count)
         VALUES (?1, ?2, 1, ?3, 0, 0)",
        params![name, description, rules_json],
    )?;
    let id = conn.last_insert_rowid();
    get_playlist(conn, id)?.ok_or_else(|| anyhow::anyhow!("playlist not found after insert"))
}

pub fn update_smart_playlist(
    conn: &Connection,
    id: i64,
    name: &str,
    description: Option<&str>,
    rules_json: &str,
) -> Result<Playlist> {
    let rows = conn.execute(
        "UPDATE playlists
         SET name = ?1, description = ?2, smart_rules = ?3, updated_at = datetime('now')
         WHERE id = ?4 AND is_smart = 1",
        params![name, description, rules_json, id],
    )?;
    if rows == 0 {
        return Err(anyhow::anyhow!("smart playlist not found or not editable"));
    }
    get_playlist(conn, id)?.ok_or_else(|| anyhow::anyhow!("playlist not found after update"))
}

pub fn delete_smart_playlist(conn: &Connection, id: i64) -> Result<()> {
    let rows = conn.execute(
        "DELETE FROM playlists WHERE id = ?1 AND is_smart = 1",
        params![id],
    )?;
    if rows == 0 {
        return Err(anyhow::anyhow!("smart playlist not found"));
    }
    Ok(())
}

pub fn get_playlist_memberships(conn: &Connection) -> Result<HashMap<i64, HashSet<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT playlist_id, track_id
         FROM playlist_tracks
         ORDER BY playlist_id, position ASC",
    )?;

    let mut rows = stmt.query([])?;
    let mut memberships: HashMap<i64, HashSet<i64>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let playlist_id: i64 = row.get(0)?;
        let track_id: i64 = row.get(1)?;
        memberships.entry(playlist_id).or_default().insert(track_id);
    }

    Ok(memberships)
}

// ─── Genres ───────────────────────────────────────────────

pub fn get_genres_filtered(
    conn: &Connection,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<Genre>> {
    let sub = crate::genre::filter::filter_subquery(filter);
    let sql = format!(
        "SELECT g.id, g.name, g.slug, g.parent_id, COUNT(tg.track_id) AS track_count
         FROM genres g
         LEFT JOIN ({sub}) tg ON tg.genre_id = g.id
         GROUP BY g.id, g.name, g.slug, g.parent_id
         ORDER BY COALESCE(g.parent_id, g.id), g.name ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let genres = stmt
        .query_map([], genre_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(genres)
}

pub fn get_genre_tree_filtered(
    conn: &Connection,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<Genre>> {
    let genres = get_genres_filtered(conn, filter)?;
    Ok(build_genre_tree(genres))
}

pub fn get_genre_heat_filtered(
    conn: &Connection,
    days: i64,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<GenreHeat>> {
    let sub = crate::genre::filter::filter_subquery(filter);
    let sql = format!(
        "WITH RECURSIVE closure(ancestor_id, genre_id) AS (
            SELECT id, id
            FROM genres
            UNION ALL
            SELECT closure.ancestor_id, g.id
            FROM closure
            JOIN genres g ON g.parent_id = closure.genre_id
        )
        SELECT
            g.id,
            g.name,
            COUNT(lh.id) AS listen_count,
            COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
        FROM genres g
        LEFT JOIN closure ON closure.ancestor_id = g.id
        LEFT JOIN ({sub}) tg ON tg.genre_id = closure.genre_id
        LEFT JOIN listen_history lh
            ON lh.track_id = tg.track_id
           AND lh.started_at >= datetime('now', printf('-%d days', ?1))
        GROUP BY g.id, g.name
        ORDER BY COALESCE(g.parent_id, g.id), g.name ASC"
    );
    let mut stmt = conn.prepare(&sql)?;

    let heat = stmt
        .query_map(params![days.max(1)], |row| {
            Ok(GenreHeat {
                genre_id: row.get(0)?,
                genre_name: row.get(1)?,
                listen_count: row.get(2)?,
                total_listened_ms: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(heat)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct GenreSummary {
    pub genre_id: i64,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i64>,
    pub direct_track_count: i64,
    pub total_track_count: i64,
    pub child_count: usize,
}

#[allow(dead_code)]
pub fn get_genre_summary(conn: &Connection, genre_id: i64) -> Result<Option<GenreSummary>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE selected_genres(id) AS (
            SELECT id FROM genres WHERE id = ?1
            UNION ALL
            SELECT g.id
            FROM genres g
            JOIN selected_genres sg ON g.parent_id = sg.id
        )
        SELECT
            g.id,
            g.name,
            g.slug,
            g.parent_id,
            COUNT(DISTINCT tg.track_id) AS direct_track_count,
            (
                SELECT COUNT(DISTINCT tg2.track_id)
                FROM selected_genres sg2
                JOIN track_genres tg2 ON tg2.genre_id = sg2.id
            ) AS total_track_count,
            (
                SELECT COUNT(*)
                FROM genres child
                WHERE child.parent_id = g.id
            ) AS child_count
        FROM genres g
        LEFT JOIN track_genres tg ON tg.genre_id = g.id
        WHERE g.id = ?1
        GROUP BY g.id, g.name, g.slug, g.parent_id",
    )?;

    let mut rows = stmt.query(params![genre_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(GenreSummary {
            genre_id: row.get(0)?,
            name: row.get(1)?,
            slug: row.get(2)?,
            parent_id: row.get(3)?,
            direct_track_count: row.get(4)?,
            total_track_count: row.get(5)?,
            child_count: row.get(6)?,
        }))
    } else {
        Ok(None)
    }
}

#[allow(dead_code)]
pub fn get_genre_path(conn: &Connection, genre_id: i64) -> Result<Vec<Genre>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE ancestry(id, name, slug, parent_id, depth) AS (
            SELECT id, name, slug, parent_id, 0
            FROM genres
            WHERE id = ?1
            UNION ALL
            SELECT g.id, g.name, g.slug, g.parent_id, ancestry.depth + 1
            FROM genres g
            JOIN ancestry ON ancestry.parent_id = g.id
        )
        SELECT id, name, slug, parent_id, 0 AS child_count
        FROM ancestry
        ORDER BY depth DESC",
    )?;

    let path = stmt
        .query_map(params![genre_id], |row| {
            Ok(Genre {
                id: row.get(0)?,
                name: row.get(1)?,
                slug: row.get(2)?,
                parent_id: row.get(3)?,
                children: Vec::new(),
                track_count: Some(0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(path)
}

pub fn get_tracks_by_genre_filtered(
    conn: &Connection,
    genre_id: i64,
    include_descendants: bool,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<Track>> {
    if !genre_exists(conn, genre_id)? {
        return Ok(Vec::new());
    }

    let sub = crate::genre::filter::filter_subquery(filter);
    let projection = track_projection("a");
    // The Spotify-dominance EXISTS check still queries raw `track_genres` —
    // it's a "did Spotify ever tag this track at all" predicate, independent
    // of the confidence filter that decides which clusters the track is
    // visible in. The MAIN membership join uses the filtered rowset.
    let sql = if include_descendants {
        format!(
            "WITH RECURSIVE selected_genres(id) AS (
                SELECT id FROM genres WHERE id = ?1
                UNION ALL
                SELECT g.id
                FROM genres g
                JOIN selected_genres sg ON g.parent_id = sg.id
            )
            SELECT DISTINCT {projection}
             FROM selected_genres sg
             JOIN ({sub}) tg ON tg.genre_id = sg.id
             JOIN tracks t ON tg.track_id = t.id
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE (
                 NOT EXISTS (
                     SELECT 1 FROM track_genres tg_sp
                     WHERE tg_sp.track_id = t.id AND tg_sp.source = 'spotify'
                 )
                 OR EXISTS (
                     SELECT 1 FROM track_genres tg_sp
                     WHERE tg_sp.track_id = t.id
                       AND tg_sp.source = 'spotify'
                       AND tg_sp.genre_id IN (SELECT id FROM selected_genres)
                 )
             )
             ORDER BY
                COALESCE(a.name, '') COLLATE NOCASE ASC,
                COALESCE(al.title, '') COLLATE NOCASE ASC,
                COALESCE(t.disc_number, 1) ASC,
                COALESCE(t.track_number, 999999) ASC,
                t.title COLLATE NOCASE ASC"
        )
    } else {
        format!(
            "SELECT DISTINCT {projection}
             FROM ({sub}) tg
             JOIN tracks t ON tg.track_id = t.id
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE tg.genre_id = ?1
               AND (
                   NOT EXISTS (
                       SELECT 1 FROM track_genres tg_sp
                       WHERE tg_sp.track_id = t.id AND tg_sp.source = 'spotify'
                   )
                   OR EXISTS (
                       SELECT 1 FROM track_genres tg_sp
                       WHERE tg_sp.track_id = t.id
                         AND tg_sp.source = 'spotify'
                         AND tg_sp.genre_id = ?1
                   )
               )
             ORDER BY
                COALESCE(a.name, '') COLLATE NOCASE ASC,
                COALESCE(al.title, '') COLLATE NOCASE ASC,
                COALESCE(t.disc_number, 1) ASC,
                COALESCE(t.track_number, 999999) ASC,
                t.title COLLATE NOCASE ASC"
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![genre_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn genre_exists(conn: &Connection, genre_id: i64) -> Result<bool> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM genres WHERE id = ?1)",
        params![genre_id],
        |row| row.get(0),
    )?;
    Ok(exists)
}

#[allow(dead_code)]
pub fn count_genre_tracks(
    conn: &Connection,
    genre_id: i64,
    include_descendants: bool,
) -> Result<i64> {
    if include_descendants {
        conn.query_row(
            "WITH RECURSIVE selected_genres(id) AS (
                SELECT id FROM genres WHERE id = ?1
                UNION ALL
                SELECT g.id
                FROM genres g
                JOIN selected_genres sg ON g.parent_id = sg.id
            )
            SELECT COUNT(DISTINCT tg.track_id)
            FROM selected_genres sg
            JOIN track_genres tg ON tg.genre_id = sg.id",
            params![genre_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    } else {
        conn.query_row(
            "SELECT COUNT(DISTINCT track_id)
             FROM track_genres
             WHERE genre_id = ?1",
            params![genre_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }
}

pub fn assign_genre_to_tracks(
    conn: &Connection,
    genre_id: i64,
    track_ids: &[i64],
    source: &str,
) -> Result<usize> {
    if track_ids.is_empty() {
        return Ok(0);
    }

    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM genres WHERE id = ?1",
            params![genre_id],
            |row| row.get(0),
        )
        .ok();
    if exists.is_none() {
        anyhow::bail!("genre not found");
    }

    let mut affected = 0;
    for track_id in track_ids {
        affected += conn.execute(
            "INSERT OR REPLACE INTO track_genres (track_id, genre_id, source, confidence)
             VALUES (?1, ?2, ?3, 1.0)",
            params![track_id, genre_id, source],
        )?;
    }

    Ok(affected)
}

pub fn replace_track_source_genres(
    conn: &Connection,
    track_id: i64,
    canonical_names: &[String],
    source: &str,
    confidence: f64,
) -> Result<usize> {
    conn.execute(
        "DELETE FROM track_genres WHERE track_id = ?1 AND source = ?2",
        params![track_id, source],
    )?;

    if canonical_names.is_empty() {
        return Ok(0);
    }

    let mut affected = 0usize;
    for canonical_name in canonical_names {
        let genre_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM genres WHERE name = ?1",
                params![canonical_name],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(genre_id) = genre_id {
            conn.execute(
                "INSERT OR REPLACE INTO track_genres (track_id, genre_id, source, confidence)
                 VALUES (?1, ?2, ?3, ?4)",
                params![track_id, genre_id, source, confidence],
            )?;
            affected += 1;
        }
    }

    Ok(affected)
}

pub fn get_track_tidal_ids(conn: &Connection, track_ids: &[i64]) -> Result<Vec<(i64, i64)>> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT id, tidal_id
         FROM tracks
         WHERE id IN ({})
           AND tidal_id IS NOT NULL",
        placeholders(track_ids.len())
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(track_ids.iter().copied()), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_album_tidal_ids(conn: &Connection, album_ids: &[i64]) -> Result<Vec<(i64, i64)>> {
    if album_ids.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT id, tidal_id
         FROM albums
         WHERE id IN ({})
           AND tidal_id IS NOT NULL",
        placeholders(album_ids.len())
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(album_ids.iter().copied()), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_analytics_overview(conn: &Connection) -> Result<AnalyticsOverview> {
    Ok(AnalyticsOverview {
        tracks: conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?,
        albums: conn.query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))?,
        artists: conn.query_row("SELECT COUNT(*) FROM artists", [], |row| row.get(0))?,
        playlists: conn.query_row("SELECT COUNT(*) FROM playlists", [], |row| row.get(0))?,
        smart_playlists: conn.query_row(
            "SELECT COUNT(*) FROM playlists WHERE is_smart = 1",
            [],
            |row| row.get(0),
        )?,
        tagged_tracks: conn.query_row(
            "SELECT COUNT(DISTINCT track_id) FROM track_genres",
            [],
            |row| row.get(0),
        )?,
        total_listens: conn
            .query_row("SELECT COUNT(*) FROM listen_history", [], |row| row.get(0))?,
        favorite_tracks: conn.query_row(
            "SELECT COUNT(*) FROM tracks WHERE is_favorite = 1",
            [],
            |row| row.get(0),
        )?,
    })
}

pub fn record_listen_history(
    conn: &Connection,
    track_id: i64,
    started_at: &str,
    duration_listened_ms: i64,
    completed: bool,
    session_id: Option<&str>,
    source: Option<ListenSource>,
    position_in_session: Option<i32>,
    transition_from_track_id: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO listen_history
            (track_id, started_at, duration_listened_ms, completed,
             session_id, source, position_in_session, transition_from_track_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            track_id,
            started_at,
            duration_listened_ms.max(0),
            completed as i32,
            session_id,
            source.map(|s| s.as_str()),
            position_in_session,
            transition_from_track_id,
        ],
    )?;
    Ok(())
}

pub fn increment_track_play_summary(
    conn: &Connection,
    track_id: i64,
    started_at: &str,
    completed: bool,
) -> Result<()> {
    if completed {
        conn.execute(
            "UPDATE tracks
             SET play_count = play_count + 1,
                 last_played_at = ?2
             WHERE id = ?1",
            params![track_id, started_at],
        )?;
    } else {
        // Always stamp last_played_at even for partial listens so freshness
        // weighting can distinguish "heard recently" from "never heard."
        conn.execute(
            "UPDATE tracks SET last_played_at = ?2 WHERE id = ?1",
            params![track_id, started_at],
        )?;
    }
    Ok(())
}

pub fn get_recent_listens(conn: &Connection, limit: i64) -> Result<Vec<ListenHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT lh.id, lh.track_id, t.title, a.name, al.title, al.artwork_url,
                lh.started_at, COALESCE(lh.duration_listened_ms, 0), lh.completed,
                lh.session_id, lh.source, lh.position_in_session, lh.transition_from_track_id
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY lh.started_at DESC, lh.id DESC
         LIMIT ?1",
    )?;

    let listens = stmt
        .query_map(params![limit], |row| {
            let source_raw: Option<String> = row.get(10)?;
            Ok(ListenHistoryEntry {
                id: row.get(0)?,
                track_id: row.get(1)?,
                track_title: row.get(2)?,
                artist_name: row.get(3)?,
                album_title: row.get(4)?,
                artwork_url: row.get(5)?,
                started_at: row.get(6)?,
                duration_listened_ms: row.get(7)?,
                completed: row.get(8)?,
                session_id: row.get(9)?,
                source: source_raw.as_deref().and_then(ListenSource::parse),
                position_in_session: row.get(11)?,
                transition_from_track_id: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(listens)
}

pub fn get_top_tracks_by_history(conn: &Connection, limit: i64) -> Result<Vec<AnalyticsTopTrack>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url,
                COUNT(lh.id) AS listens,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         GROUP BY t.id, t.title, a.name, al.title, al.artwork_url
         ORDER BY listens DESC, total_listened_ms DESC, t.title ASC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(AnalyticsTopTrack {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                listens: row.get(5)?,
                completed_listens: row.get(6)?,
                total_listened_ms: row.get(7)?,
                completion_rate: None,
                share_of_window_listened_ms: None,
                previous_rank: None,
                rank_delta: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_top_artists_by_history(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<AnalyticsTopArtist>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name,
                COUNT(lh.id) AS listens,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COUNT(DISTINCT t.id) AS unique_tracks,
                COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         JOIN artists a ON t.artist_id = a.id
         GROUP BY a.id, a.name
         ORDER BY listens DESC, total_listened_ms DESC, a.name ASC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(AnalyticsTopArtist {
                artist_id: row.get(0)?,
                artist_name: row.get(1)?,
                listens: row.get(2)?,
                completed_listens: row.get(3)?,
                unique_tracks: row.get(4)?,
                total_listened_ms: row.get(5)?,
                completion_rate: None,
                share_of_window_listened_ms: None,
                previous_rank: None,
                rank_delta: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_top_genres_by_history(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<AnalyticsGenreShare>> {
    let mut stmt = conn.prepare(
        "SELECT g.name,
                COUNT(lh.id) AS listens
         FROM listen_history lh
         JOIN track_genres tg ON lh.track_id = tg.track_id
         JOIN genres g ON tg.genre_id = g.id
         GROUP BY g.id, g.name
         ORDER BY listens DESC, g.name ASC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(AnalyticsGenreShare {
                genre_name: row.get(0)?,
                listens: row.get(1)?,
                share_of_window_listens: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_listen_activity(conn: &Connection, days: i64) -> Result<Vec<AnalyticsActivityPoint>> {
    let mut stmt = conn.prepare(
        "SELECT DATE(started_at, 'localtime') AS day,
                COUNT(*) AS listens,
                COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COALESCE(SUM(duration_listened_ms), 0) AS listened_ms
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY DATE(started_at, 'localtime')
         ORDER BY day ASC",
    )?;

    let rows = stmt
        .query_map(params![days.max(1)], |row| {
            Ok(AnalyticsActivityPoint {
                day: row.get(0)?,
                listens: row.get(1)?,
                completed_listens: row.get(2)?,
                listened_ms: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_behavior_metrics(conn: &Connection) -> Result<AnalyticsBehavior> {
    let (total_listened_ms, total_listens, completed_listens, unique_tracks, active_days): (
        Option<i64>,
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        "SELECT
            COALESCE(SUM(duration_listened_ms), 0),
            COUNT(*),
            COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0),
            COUNT(DISTINCT track_id),
            COUNT(DISTINCT DATE(started_at))
         FROM listen_history",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let repeat_track_count: Option<i64> = conn.query_row(
        "SELECT COUNT(*)
         FROM (
            SELECT track_id
            FROM listen_history
            GROUP BY track_id
            HAVING COUNT(*) > 1
         )",
        [],
        |row| row.get(0),
    )?;

    let total_listened_ms = total_listened_ms.unwrap_or(0);
    let completed_listens = completed_listens.unwrap_or(0);
    let unique_tracks = unique_tracks.unwrap_or(0);
    let repeat_track_count = repeat_track_count.unwrap_or(0);
    let active_days = active_days.unwrap_or(0);
    let skipped_listens = total_listens.saturating_sub(completed_listens);
    let completion_rate = if total_listens == 0 {
        0.0
    } else {
        completed_listens as f64 / total_listens as f64
    };
    let average_listen_ms = if total_listens == 0 {
        0
    } else {
        total_listened_ms / total_listens
    };

    Ok(AnalyticsBehavior {
        total_listened_ms,
        total_listens,
        completed_listens,
        skipped_listens,
        completion_rate,
        average_listen_ms,
        unique_tracks,
        repeat_track_count,
        active_days,
    })
}

// ─── Sync Metadata ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncInfo {
    pub service: String,
    pub last_sync_at: String,
    pub auto_sync_daily: bool,
    pub last_sync_track_count: i64,
    pub last_sync_album_count: i64,
    pub last_full_sync_at: Option<String>,
    pub last_sync_kind: Option<String>,
    pub tidal_favorite_artist_cursor: Option<String>,
    pub tidal_favorite_album_cursor: Option<String>,
    pub tidal_favorite_track_cursor: Option<String>,
}

pub fn get_sync_info(conn: &Connection, service: &str) -> Result<Option<SyncInfo>> {
    let mut stmt = conn.prepare(
        "SELECT service, last_sync_at, auto_sync_daily, last_sync_track_count, last_sync_album_count,
                last_full_sync_at, last_sync_kind,
                tidal_favorite_artist_cursor, tidal_favorite_album_cursor, tidal_favorite_track_cursor
         FROM sync_metadata WHERE service = ?1",
    )?;
    let result = stmt
        .query_row([service], |row| {
            Ok(SyncInfo {
                service: row.get(0)?,
                last_sync_at: row.get(1)?,
                auto_sync_daily: row.get::<_, i64>(2)? != 0,
                last_sync_track_count: row.get(3)?,
                last_sync_album_count: row.get(4)?,
                last_full_sync_at: row.get(5)?,
                last_sync_kind: row.get(6)?,
                tidal_favorite_artist_cursor: row.get(7)?,
                tidal_favorite_album_cursor: row.get(8)?,
                tidal_favorite_track_cursor: row.get(9)?,
            })
        })
        .optional()?;
    Ok(result)
}

pub fn update_sync_timestamp_with_metadata(
    conn: &Connection,
    service: &str,
    track_count: i64,
    album_count: i64,
    sync_kind: &str,
    artist_cursor: Option<&str>,
    album_cursor: Option<&str>,
    track_cursor: Option<&str>,
) -> Result<()> {
    let last_full_sync_expr = if sync_kind == "full" {
        "datetime('now')"
    } else {
        "sync_metadata.last_full_sync_at"
    };
    conn.execute(
        &format!(
            "INSERT INTO sync_metadata (
                service, last_sync_at, auto_sync_daily,
                last_sync_track_count, last_sync_album_count,
                last_full_sync_at, last_sync_kind,
                tidal_favorite_artist_cursor, tidal_favorite_album_cursor, tidal_favorite_track_cursor
             )
             VALUES (
                ?1, datetime('now'), 0,
                ?2, ?3,
                CASE WHEN ?4 = 'full' THEN datetime('now') ELSE NULL END,
                ?4, ?5, ?6, ?7
             )
         ON CONFLICT(service) DO UPDATE SET
             last_sync_at = datetime('now'),
             last_sync_track_count = ?2,
             last_sync_album_count = ?3,
             last_full_sync_at = {last_full_sync_expr},
             last_sync_kind = ?4,
             tidal_favorite_artist_cursor = COALESCE(?5, sync_metadata.tidal_favorite_artist_cursor),
             tidal_favorite_album_cursor = COALESCE(?6, sync_metadata.tidal_favorite_album_cursor),
             tidal_favorite_track_cursor = COALESCE(?7, sync_metadata.tidal_favorite_track_cursor)"
        ),
        rusqlite::params![
            service,
            track_count,
            album_count,
            sync_kind,
            artist_cursor,
            album_cursor,
            track_cursor
        ],
    )?;
    Ok(())
}

pub fn set_auto_sync_daily(conn: &Connection, service: &str, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE sync_metadata SET auto_sync_daily = ?1 WHERE service = ?2",
        rusqlite::params![if enabled { 1 } else { 0 }, service],
    )?;
    Ok(())
}

pub fn get_auto_sync_services(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT service FROM sync_metadata WHERE auto_sync_daily = 1")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// True if `service` has a recorded `last_sync_at` within `window_secs`.
/// Used by the boot-time auto-sync to honour the "daily" promise instead of
/// running on every server start.
pub fn sync_within_window(conn: &Connection, service: &str, window_secs: i64) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sync_metadata
         WHERE service = ?1
           AND last_sync_at IS NOT NULL
           AND last_sync_at != ''
           AND (strftime('%s','now') - strftime('%s', last_sync_at)) < ?2",
        rusqlite::params![service, window_secs],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod sync_metadata_tests {
    use super::*;
    use crate::db::schema;

    fn migrated_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn
    }

    #[test]
    fn sync_info_exposes_full_marker_kind_and_favorite_cursors() {
        let conn = migrated_db();

        update_sync_timestamp_with_metadata(
            &conn,
            "tidal",
            42,
            7,
            "full",
            Some("2026-05-01T01:00:00Z"),
            Some("2026-05-02T01:00:00Z"),
            Some("2026-05-03T01:00:00Z"),
        )
        .expect("metadata update");

        let info = get_sync_info(&conn, "tidal")
            .expect("sync info")
            .expect("tidal row");
        assert_eq!(info.last_sync_kind.as_deref(), Some("full"));
        assert!(info.last_full_sync_at.is_some());
        assert_eq!(
            info.tidal_favorite_artist_cursor.as_deref(),
            Some("2026-05-01T01:00:00Z")
        );
        assert_eq!(
            info.tidal_favorite_album_cursor.as_deref(),
            Some("2026-05-02T01:00:00Z")
        );
        assert_eq!(
            info.tidal_favorite_track_cursor.as_deref(),
            Some("2026-05-03T01:00:00Z")
        );
    }

    #[test]
    fn incremental_sync_metadata_does_not_replace_last_full_marker() {
        let conn = migrated_db();
        update_sync_timestamp_with_metadata(
            &conn,
            "tidal",
            20,
            3,
            "full",
            Some("2026-05-01T01:00:00Z"),
            Some("2026-05-02T01:00:00Z"),
            Some("2026-05-03T01:00:00Z"),
        )
        .expect("full update");
        let full_at = get_sync_info(&conn, "tidal")
            .expect("sync info")
            .expect("tidal row")
            .last_full_sync_at;

        update_sync_timestamp_with_metadata(
            &conn,
            "tidal",
            2,
            1,
            "incremental",
            Some("2026-05-04T01:00:00Z"),
            Some("2026-05-05T01:00:00Z"),
            Some("2026-05-06T01:00:00Z"),
        )
        .expect("incremental update");

        let info = get_sync_info(&conn, "tidal")
            .expect("sync info")
            .expect("tidal row");
        assert_eq!(info.last_sync_kind.as_deref(), Some("incremental"));
        assert_eq!(info.last_full_sync_at, full_at);
        assert_eq!(
            info.tidal_favorite_track_cursor.as_deref(),
            Some("2026-05-06T01:00:00Z")
        );
    }
}

// ─── Genre Co-Occurrence (co-listening pairs) ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreCoOccurrence {
    pub genre_a_id: i64,
    pub genre_a_name: String,
    pub genre_b_id: i64,
    pub genre_b_name: String,
    pub co_listen_count: i64,
    pub jaccard: f64,
}

/// Find genre-genre pairs that are co-listened within the same session window.
/// Two genres "co-occur" if a user listened to tracks from both genres within
/// `window_minutes` of each other (default 30 min). Returns pairs with at least
/// `min_count` co-occurrences, sorted by Jaccard similarity.
pub fn get_genre_co_occurrence_filtered(
    conn: &Connection,
    _days: i64,
    _window_minutes: i64,
    min_count: i64,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<GenreCoOccurrence>> {
    let sub = crate::genre::filter::filter_subquery(filter);
    // Same query as before but built against the filtered rowset. The
    // subquery is inlined twice rather than CTE'd because SQLite doesn't
    // share materialization between CTE references reliably.
    let sql = format!(
        "WITH track_genre_pairs AS (
            SELECT a.genre_id AS genre_a, b.genre_id AS genre_b
            FROM ({sub}) a
            JOIN ({sub}) b ON b.track_id = a.track_id AND b.genre_id > a.genre_id
        ),
        pair_counts AS (
            SELECT genre_a, genre_b, COUNT(*) AS co_count
            FROM track_genre_pairs
            GROUP BY genre_a, genre_b
            HAVING co_count >= ?1
        ),
        genre_totals AS (
            SELECT genre_id, COUNT(DISTINCT track_id) AS total_tracks
            FROM ({sub})
            GROUP BY genre_id
        )
        SELECT
            ga.id, ga.name,
            gb.id, gb.name,
            pc.co_count,
            CAST(pc.co_count AS REAL) /
                MAX(1, gt_a.total_tracks + gt_b.total_tracks - pc.co_count) AS jaccard
        FROM pair_counts pc
        JOIN genres ga ON ga.id = pc.genre_a
        JOIN genres gb ON gb.id = pc.genre_b
        JOIN genre_totals gt_a ON gt_a.genre_id = pc.genre_a
        JOIN genre_totals gt_b ON gt_b.genre_id = pc.genre_b
        ORDER BY jaccard DESC, pc.co_count DESC"
    );
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map(params![min_count], |row| {
        Ok(GenreCoOccurrence {
            genre_a_id: row.get(0)?,
            genre_a_name: row.get(1)?,
            genre_b_id: row.get(2)?,
            genre_b_name: row.get(3)?,
            co_listen_count: row.get(4)?,
            jaccard: row.get(5)?,
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ─── Genre Cohorts (personal clusters from time-based listening) ─────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreCohort {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub genre_ids: Vec<i64>,
    pub listen_count: i64,
    pub total_listened_ms: i64,
}

/// Derive personal listening cohorts by analyzing time-of-day and day-of-week
/// patterns. Groups genres into clusters like "Late Night", "Morning Commute",
/// "Weekend", "Deep Focus", etc.
pub fn get_genre_cohorts_filtered(
    conn: &Connection,
    days: i64,
    filter: crate::genre::filter::GalaxyFilterRule,
    with_fallback: bool,
) -> Result<Vec<GenreCohort>> {
    let _ = days; // bound via ?1 below
    let sub = if with_fallback {
        crate::genre::filter::filter_subquery_with_fallback(filter)
    } else {
        crate::genre::filter::filter_subquery(filter)
    };
    // We bucket listens into 4 time-of-day slots + weekend/weekday
    // Slot 0: 0-6 (Night), Slot 1: 6-12 (Morning), Slot 2: 12-18 (Afternoon), Slot 3: 18-24 (Evening)
    // Then find genres that dominate each slot.
    let sql = format!(
        "WITH recent AS (
            SELECT
                lh.id AS listen_id,
                lh.track_id,
                lh.started_at,
                lh.duration_listened_ms,
                CAST(strftime('%H', lh.started_at, 'localtime') AS INTEGER) AS hour,
                CAST(strftime('%w', lh.started_at, 'localtime') AS INTEGER) AS dow
            FROM listen_history lh
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
        ),
        genre_buckets AS (
            SELECT
                tg.genre_id,
                g.name AS genre_name,
                CASE
                    WHEN r.hour < 6 THEN 'night'
                    WHEN r.hour < 12 THEN 'morning'
                    WHEN r.hour < 18 THEN 'afternoon'
                    ELSE 'evening'
                END AS time_slot,
                CASE
                    WHEN r.dow = 0 OR r.dow = 6 THEN 'weekend'
                    ELSE 'weekday'
                END AS day_type,
                COUNT(*) AS listens,
                COALESCE(SUM(r.duration_listened_ms), 0) AS listened_ms
            FROM recent r
            JOIN ({sub}) tg ON tg.track_id = r.track_id
            JOIN genres g ON g.id = tg.genre_id
            GROUP BY tg.genre_id, time_slot, day_type
        ),
        dominant AS (
            SELECT
                genre_id,
                genre_name,
                time_slot,
                day_type,
                listens,
                listened_ms,
                ROW_NUMBER() OVER (PARTITION BY genre_id ORDER BY listens DESC) AS rn
            FROM genre_buckets
        )
        SELECT genre_id, genre_name, time_slot, day_type, listens, listened_ms
        FROM dominant
        WHERE rn = 1
        ORDER BY listens DESC"
    );
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map(params![days], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;

    let entries: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;

    // Build cohorts from the dominant assignments
    let mut cohort_map: std::collections::HashMap<String, GenreCohort> =
        std::collections::HashMap::new();

    for (genre_id, _genre_name, time_slot, day_type, listens, listened_ms) in entries {
        let (id, label, icon) = match (time_slot.as_str(), day_type.as_str()) {
            ("night", _) => ("night_owl", "Night Owl", "🌙"),
            ("morning", "weekday") => ("morning_commute", "Morning Commute", "☀"),
            ("morning", "weekend") => ("lazy_morning", "Weekend Morning", "🌤"),
            ("afternoon", "weekday") => ("afternoon_drift", "Afternoon Drift", "☁"),
            ("afternoon", "weekend") => ("weekend_afternoon", "Weekend Afternoon", "🌿"),
            ("evening", "weekday") => ("evening_wind_down", "Evening Wind-Down", "🌆"),
            ("evening", "weekend") => ("weekend_evening", "Weekend Evening", "🎶"),
            _ => ("other", "Other", "✦"),
        };

        let cohort = cohort_map
            .entry(id.to_string())
            .or_insert_with(|| GenreCohort {
                id: id.to_string(),
                label: label.to_string(),
                icon: icon.to_string(),
                genre_ids: vec![],
                listen_count: 0,
                total_listened_ms: 0,
            });

        cohort.genre_ids.push(genre_id);
        cohort.listen_count += listens;
        cohort.total_listened_ms += listened_ms;
    }

    let mut cohorts: Vec<_> = cohort_map.into_values().collect();
    cohorts.sort_by(|a, b| b.listen_count.cmp(&a.listen_count));

    Ok(cohorts)
}

/// Map track IDs to their dominant cohort (id, label) using `get_genre_cohorts`.
/// Each genre belongs to at most one cohort (enforced by `get_genre_cohorts`).
/// For a track tagged with multiple genres mapped to *different* cohorts, the
/// helper picks the first matching genre row returned by SQLite (no `ORDER BY`),
/// which is effectively undefined order. Acceptable for now since cohorts are a
/// soft signal; revisit if cohort labels need to be deterministic per track.
pub fn get_track_cohort_assignments(
    conn: &Connection,
    track_ids: &[i64],
    days: i64,
) -> Result<std::collections::HashMap<i64, (String, String)>> {
    if track_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let cohorts = get_genre_cohorts_filtered(
        conn,
        days,
        crate::genre::filter::GalaxyFilterRule::default_rule(),
        true, // with_fallback: cohort labels need to cover empty-genre tracks
    )?;
    if cohorts.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build genre_id → (cohort_id, cohort_label), preferring earlier (higher-rank) cohorts.
    let mut genre_to_cohort: std::collections::HashMap<i64, (String, String)> =
        std::collections::HashMap::new();
    for cohort in &cohorts {
        for gid in &cohort.genre_ids {
            genre_to_cohort
                .entry(*gid)
                .or_insert((cohort.id.clone(), cohort.label.clone()));
        }
    }

    // Pull all (track_id, genre_id) pairs for the requested tracks.
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT track_id, genre_id FROM track_genres WHERE track_id IN ({ids_csv})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;

    let mut assignments: std::collections::HashMap<i64, (String, String)> =
        std::collections::HashMap::new();
    for r in rows {
        let (track_id, genre_id) = r?;
        if assignments.contains_key(&track_id) {
            continue;
        }
        if let Some(pair) = genre_to_cohort.get(&genre_id) {
            assignments.insert(track_id, pair.clone());
        }
    }

    Ok(assignments)
}

/// Album release year per track, for the discovery era filter. Tracks whose
/// album has no year are simply absent from the map (the filter passes them).
pub fn get_album_years_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT t.id, al.year
         FROM tracks t
         JOIN albums al ON al.id = t.album_id
         WHERE t.id IN ({ids_csv}) AND al.year IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    let mut map = HashMap::new();
    for r in rows {
        let (track_id, year) = r?;
        map.insert(track_id, year);
    }
    Ok(map)
}

/// Minimal DSP triple (bpm, camelot, energy) per track, batched for discovery
/// ranking. Only rows that exist come back; absent tracks mean "unanalyzed".
pub fn get_dsp_lite_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, (Option<f64>, Option<String>, Option<f64>)>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT track_id, bpm, camelot_key, energy
         FROM audio_dsp_features WHERE track_id IN ({ids_csv})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<f64>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<f64>>(3)?,
        ))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (track_id, bpm, camelot, energy) = r?;
        map.insert(track_id, (bpm, camelot, energy));
    }
    Ok(map)
}

/// Flat genre names per track, batched for discovery ranking's weighted
/// Jaccard. Names, not paths: plain-name sets forgo the ancestor bonus but
/// avoid a second, heavier path-resolution query on the interactive path.
pub fn get_genre_names_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT tg.track_id, g.name
         FROM track_genres tg
         JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id IN ({ids_csv})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    for r in rows {
        let (track_id, name) = r?;
        map.entry(track_id).or_default().push(name);
    }
    Ok(map)
}

/// Genre tag lists for external track candidates (parsed from their
/// `genre_tags_json` sidecar column), batched by candidate id.
pub fn get_external_candidate_genre_tags(
    conn: &Connection,
    candidate_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>> {
    if candidate_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = candidate_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, genre_tags_json FROM external_track_candidates
         WHERE id IN ({ids_csv}) AND genre_tags_json IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (id, raw) = r?;
        if let Ok(tags) = serde_json::from_str::<Vec<String>>(&raw) {
            if !tags.is_empty() {
                map.insert(id, tags);
            }
        }
    }
    Ok(map)
}

/// Track ids heard in a listening session, for the discovery exclude-heard
/// filter. `session_id: None` falls back to the most recent session so the
/// filter still means something when the client has not minted one yet.
pub fn get_session_heard_track_ids(
    conn: &Connection,
    session_id: Option<&str>,
) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT track_id FROM listen_history
         WHERE session_id = COALESCE(
             ?1,
             (SELECT session_id FROM listen_history
              WHERE session_id IS NOT NULL
              ORDER BY started_at DESC LIMIT 1)
         )",
    )?;
    let rows = stmt.query_map(params![session_id], |row| row.get::<_, i64>(0))?;
    for r in rows {
        out.insert(r?);
    }
    Ok(out)
}

// ─── Genre Evolution (time-sliced heat for temporal trails) ──────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreEvolutionPoint {
    pub genre_id: i64,
    pub genre_name: String,
    pub period_start: String,
    pub listen_count: i64,
    pub total_listened_ms: i64,
}

/// Return genre heat broken into weekly time slices over the past N days.
/// Each (genre_id, week_start) pair is one evolution point.
pub fn get_genre_evolution(conn: &Connection, days: i64) -> Result<Vec<GenreEvolutionPoint>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE closure(ancestor_id, genre_id) AS (
            SELECT id, id FROM genres
            UNION ALL
            SELECT closure.ancestor_id, g.id
            FROM closure JOIN genres g ON g.parent_id = closure.genre_id
        ),
        weekly AS (
            SELECT
                tg.genre_id,
                g.name AS genre_name,
                date(lh.started_at, 'weekday 0', '-6 days') AS period_start,
                COUNT(DISTINCT lh.id) AS listen_count,
                COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
            FROM listen_history lh
            JOIN track_genres tg ON tg.track_id = lh.track_id
            JOIN genres g ON g.id = tg.genre_id
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
            GROUP BY tg.genre_id, period_start
        )
        SELECT genre_id, genre_name, period_start, listen_count, total_listened_ms
        FROM weekly
        ORDER BY genre_id, period_start",
    )?;

    let rows = stmt.query_map(params![days], |row| {
        Ok(GenreEvolutionPoint {
            genre_id: row.get(0)?,
            genre_name: row.get(1)?,
            period_start: row.get(2)?,
            listen_count: row.get(3)?,
            total_listened_ms: row.get(4)?,
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_discovery_candidate_tracks(conn: &Connection, limit: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY t.is_favorite DESC, t.play_count DESC, t.date_added DESC, t.title ASC
         LIMIT ?1",
        track_projection("a")
    ))?;

    let tracks = stmt
        .query_map(params![limit.max(1)], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

/// Variant with an optional LIMIT for automix candidate selection.
/// When `max_candidates` > 0, only the top N tracks (by the default ordering) are returned,
/// dramatically reducing memory usage for automix which would otherwise load all 32k tracks.
pub fn get_tracks_excluding_with_limit(
    conn: &Connection,
    excluded_track_ids: &[i64],
    max_candidates: usize,
) -> Result<Vec<Track>> {
    let mut sql = format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id",
        track_projection("a")
    );

    if !excluded_track_ids.is_empty() {
        sql.push_str(" WHERE t.id NOT IN (");
        sql.push_str(&placeholders(excluded_track_ids.len()));
        sql.push(')');
    }

    sql.push_str(" ORDER BY t.is_favorite DESC, t.play_count ASC, t.fidelity_score DESC, t.date_added DESC, t.title ASC");

    if max_candidates > 0 {
        sql.push_str(&format!(" LIMIT {}", max_candidates));
    }

    let mut stmt = conn.prepare(&sql)?;
    let params = params_from_iter(excluded_track_ids.iter().copied());
    let tracks = stmt
        .query_map(params, track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

/// Artist-diverse recall over the seed's genres: one representative track per
/// artist that shares any genre with the seed. Used to widen an automix pool that
/// has collapsed to a handful of artists - a precomputed similar-pool is often the
/// seed's own catalogue plus one over-connected neighbour, and no per-artist cap
/// can create diversity the pool doesn't contain. Returning a single track per
/// artist makes this a breadth-first artist sample (no deep runs), which the
/// scorer's shared-genre boost then keeps on-vibe. Empty when the seed is
/// untagged. Caller dedupes against the existing pool and exclusions.
pub fn get_genre_diverse_candidates(
    conn: &Connection,
    seed_track_id: i64,
    limit: usize,
) -> Result<Vec<Track>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {proj}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.id IN (
             SELECT MIN(t2.id)
             FROM tracks t2
             JOIN track_genres tg ON tg.track_id = t2.id
             WHERE tg.genre_id IN (
                 SELECT genre_id FROM track_genres WHERE track_id = ?1
             )
             GROUP BY t2.artist_id
         )
         ORDER BY t.id
         LIMIT {limit}",
        proj = track_projection("a"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![seed_track_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn get_existing_tidal_track_ids(conn: &Connection, tidal_ids: &[i64]) -> Result<HashSet<i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let placeholders = placeholders(tidal_ids.len());
    let query = format!(
        "SELECT tidal_id
         FROM tracks
         WHERE tidal_id IN ({placeholders})"
    );
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&query)?;
    let ids = stmt
        .query_map(params, |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?;

    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TidalTrackLibraryState {
    pub local_id: i64,
    pub is_favorite: bool,
}

pub fn get_tidal_track_library_states(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, TidalTrackLibraryState>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql =
        format!("SELECT tidal_id, id, is_favorite FROM tracks WHERE tidal_id IN ({placeholders})");
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, i64>(0)?,
            TidalTrackLibraryState {
                local_id: row.get::<_, i64>(1)?,
                is_favorite: row.get::<_, i64>(2)? != 0,
            },
        ))
    })?;
    for row in rows {
        let (tidal_id, state) = row?;
        map.insert(tidal_id, state);
    }
    Ok(map)
}

pub fn get_tidal_track_local_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    Ok(get_tidal_track_library_states(conn, tidal_ids)?
        .into_iter()
        .map(|(tidal_id, state)| (tidal_id, state.local_id))
        .collect())
}

pub fn get_artist_photos_by_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, String>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!(
        "SELECT tidal_id, photo_url FROM artists WHERE tidal_id IN ({placeholders}) AND photo_url IS NOT NULL"
    );
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (tid, photo) = row?;
        map.insert(tid, photo);
    }
    Ok(map)
}

pub fn list_discovery_presets(conn: &Connection) -> Result<Vec<DiscoveryPreset>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, words, mode, services, created_at
         FROM discovery_presets
         ORDER BY created_at DESC, id DESC",
    )?;

    let presets = stmt
        .query_map([], |row| {
            let services_raw: String = row.get(4)?;
            Ok(DiscoveryPreset {
                id: row.get(0)?,
                name: row.get(1)?,
                prompt: row.get(2)?,
                mode: row.get(3)?,
                services: parse_discovery_services(&services_raw),
                created_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(presets)
}

pub fn create_discovery_preset(
    conn: &Connection,
    name: &str,
    prompt: &str,
    mode: &str,
    services_json: &str,
) -> Result<DiscoveryPreset> {
    conn.execute(
        "INSERT INTO discovery_presets (name, words, mode, services)
         VALUES (?1, ?2, ?3, ?4)",
        params![name, prompt, mode, services_json],
    )?;

    let id = conn.last_insert_rowid();
    let created_at: String = conn.query_row(
        "SELECT created_at FROM discovery_presets WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;

    Ok(DiscoveryPreset {
        id,
        name: name.to_string(),
        prompt: prompt.to_string(),
        mode: mode.to_string(),
        services: parse_discovery_services(services_json),
        created_at,
    })
}

pub fn cache_discovery_results(
    conn: &Connection,
    preset_id: Option<i64>,
    results: &[DiscoveryPreviewResult],
) -> Result<()> {
    for result in results {
        conn.execute(
            "INSERT INTO discovery_results (
                preset_id, track_title, artist_name, service, service_track_id,
                relevance_score, preview_url
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                preset_id,
                result.title,
                result.artist_name,
                result.service,
                result.service_track_id,
                result.score as f64 / 100.0,
                result.artwork_url,
            ],
        )?;
    }

    Ok(())
}

fn parse_discovery_services(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw)
        .ok()
        .filter(|values| !values.is_empty())
        .or_else(|| {
            serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|value| match value {
                    Value::String(single) => Some(vec![single]),
                    _ => None,
                })
        })
        .unwrap_or_else(|| vec!["tidal".to_string()])
}

// ─── Search (FTS5 + LIKE fallback) ────────────────────────────────────────

/// Strip FTS5 special chars and append `*` to each token for prefix matching.
///
/// The apostrophe is treated as a separator, NOT preserved: FTS5 parses a bare
/// `'` as the start of a string literal, so keeping it turned every query with
/// an apostrophe ("Don't", "Guns N' Roses") into an `fts5: syntax error` that
/// silently dropped search into the full-table LIKE scan (~13x slower here).
/// The unicode61 tokenizer already splits indexed text on apostrophes ("Don't"
/// -> "don","t"), so mapping `'` to a space ("don't" -> "don* t*") both parses
/// cleanly and matches how the content was indexed.
fn to_fts_query(input: &str) -> String {
    let clean: String = input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    clean
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("{}*", t))
        .collect::<Vec<_>>()
        .join(" ")
}

fn search_tracks_fts(conn: &Connection, fts_query: &str, limit: i64) -> Result<Vec<Track>> {
    // Positional ORDER BY (17/18/16/2) instead of named columns: SQLite rejects
    // bare column names in compound-SELECT (UNION) ORDER BY when the SELECTs
    // contain JOINs, with "1st ORDER BY term does not match any column in the
    // result set". Positional indices sidestep the resolver entirely. Mapping:
    //   2  = t.title
    //   16 = t.fidelity_score
    //   17 = t.is_favorite
    //   18 = t.play_count
    let projection = track_projection("a");
    let mut stmt = conn.prepare(&format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         JOIN tracks_fts ON tracks_fts.rowid = t.id
         WHERE tracks_fts MATCH ?1
         UNION
         SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         JOIN artists_fts ON artists_fts.rowid = t.artist_id
         WHERE artists_fts MATCH ?1
         ORDER BY 17 DESC, 18 DESC, 16 DESC, 2 ASC
         LIMIT ?2"
    ))?;
    stmt.query_map(params![fts_query, limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn search_tracks_like(conn: &Connection, normalized: &str, limit: i64) -> Result<Vec<Track>> {
    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE LOWER(t.title) LIKE ?1
            OR LOWER(COALESCE(a.name, '')) LIKE ?1
            OR LOWER(COALESCE(al.title, '')) LIKE ?1
         ORDER BY
            CASE
                WHEN LOWER(COALESCE(a.name, '')) = ?2 THEN 0
                WHEN LOWER(t.title) = ?2 THEN 1
                WHEN LOWER(COALESCE(al.title, '')) = ?2 THEN 2
                WHEN LOWER(COALESCE(a.name, '')) LIKE ?3 THEN 3
                WHEN LOWER(t.title) LIKE ?3 THEN 4
                WHEN LOWER(COALESCE(al.title, '')) LIKE ?3 THEN 5
                ELSE 6
            END,
            t.is_favorite DESC,
            t.play_count DESC,
            t.fidelity_score DESC,
            t.title ASC
         LIMIT ?4",
        track_projection("a")
    ))?;
    stmt.query_map(
        params![contains_pattern, normalized, prefix_pattern, limit],
        track_from_row,
    )?
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_artists_fts(conn: &Connection, fts_query: &str, limit: i64) -> Result<Vec<Artist>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         JOIN artists_fts ON artists_fts.rowid = a.id
         WHERE artists_fts MATCH ?1
         ORDER BY a.name ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![fts_query, limit], |row| {
        Ok(Artist {
            id: row.get(0)?,
            tidal_id: row.get(1)?,
            ytmusic_id: row.get(2)?,
            soundcloud_id: row.get(3)?,
            name: row.get(4)?,
            name_sort: row.get(5)?,
            biography: row.get(6)?,
            photo_url: row.get(7)?,
        })
    })?
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_artists_like(conn: &Connection, normalized: &str, limit: i64) -> Result<Vec<Artist>> {
    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
    let mut stmt = conn.prepare(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         WHERE LOWER(a.name) LIKE ?1
         ORDER BY
            CASE
                WHEN LOWER(a.name) = ?2 THEN 0
                WHEN LOWER(a.name) LIKE ?3 THEN 1
                ELSE 2
            END,
            a.name ASC
         LIMIT ?4",
    )?;
    stmt.query_map(
        params![contains_pattern, normalized, prefix_pattern, limit],
        |row| {
            Ok(Artist {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                soundcloud_id: row.get(3)?,
                name: row.get(4)?,
                name_sort: row.get(5)?,
                biography: row.get(6)?,
                photo_url: row.get(7)?,
            })
        },
    )?
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_albums_fts(conn: &Connection, fts_query: &str, limit: i64) -> Result<Vec<Album>> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON a.id = al.artist_id
         JOIN albums_fts ON albums_fts.rowid = al.id
         WHERE albums_fts MATCH ?1
         UNION
         SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON a.id = al.artist_id
         JOIN artists_fts ON artists_fts.rowid = al.artist_id
         WHERE artists_fts MATCH ?1
         ORDER BY title ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![fts_query, limit], |row| {
        Ok(Album {
            id: row.get(0)?,
            tidal_id: row.get(1)?,
            ytmusic_id: row.get(2)?,
            title: row.get(3)?,
            artist_id: row.get(4)?,
            artist_name: row.get(5)?,
            year: row.get(6)?,
            artwork_url: row.get(7)?,
            release_type: row.get(8)?,
            label: row.get(9)?,
            track_count: row.get(10)?,
            is_favorite: row.get::<_, i32>(11)? != 0,
            source: row.get(12)?,
        })
    })?
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_albums_like(conn: &Connection, normalized: &str, limit: i64) -> Result<Vec<Album>> {
    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
    let mut stmt = conn.prepare(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         WHERE LOWER(al.title) LIKE ?1
            OR LOWER(COALESCE(a.name, '')) LIKE ?1
         ORDER BY
            CASE
                WHEN LOWER(COALESCE(a.name, '')) = ?2 THEN 0
                WHEN LOWER(al.title) = ?2 THEN 1
                WHEN LOWER(COALESCE(a.name, '')) LIKE ?3 THEN 2
                WHEN LOWER(al.title) LIKE ?3 THEN 3
                ELSE 4
            END,
            al.is_favorite DESC,
            al.year DESC,
            al.title ASC
         LIMIT ?4",
    )?;
    stmt.query_map(
        params![contains_pattern, normalized, prefix_pattern, limit],
        |row| {
            Ok(Album {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                title: row.get(3)?,
                artist_id: row.get(4)?,
                artist_name: row.get(5)?,
                year: row.get(6)?,
                artwork_url: row.get(7)?,
                release_type: row.get(8)?,
                label: row.get(9)?,
                track_count: row.get(10)?,
                is_favorite: row.get::<_, i32>(11)? != 0,
                source: row.get(12)?,
            })
        },
    )?
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

pub fn search(conn: &Connection, query: &str, limit: i64) -> Result<SearchResults> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(SearchResults {
            tracks: Vec::new(),
            albums: Vec::new(),
            artists: Vec::new(),
        });
    }

    let limit = limit.max(1);
    let fts_query = to_fts_query(&normalized);

    // Try FTS first; fall back to LIKE on any error.
    let tracks = search_tracks_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_tracks_like(conn, &normalized, limit).unwrap_or_default());
    let artists = search_artists_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_artists_like(conn, &normalized, limit).unwrap_or_default());
    let albums = search_albums_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_albums_like(conn, &normalized, limit).unwrap_or_default());

    Ok(SearchResults {
        tracks,
        artists,
        albums,
    })
}

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        title: row.get(1)?,
        artist_id: row.get(2)?,
        artist_name: row.get(3)?,
        album_id: row.get(4)?,
        album_title: row.get(5)?,
        disc_number: row.get(6)?,
        track_number: row.get(7)?,
        duration_ms: row.get(8)?,
        isrc: row.get(9)?,
        tidal_id: row.get(10)?,
        artist_tidal_id: None,
        album_tidal_id: None,
        ytmusic_id: row.get(11)?,
        soundcloud_id: row.get(12)?,
        best_quality: row.get(13)?,
        best_source: row.get(14)?,
        fidelity_score: row.get(15)?,
        is_favorite: row.get(16)?,
        play_count: row.get(17)?,
        last_played_at: row.get(18)?,
        date_added: row.get(19)?,
        source: row.get(20)?,
        artwork_url: row.get(21)?,
    })
}

fn genre_from_row(row: &Row<'_>) -> rusqlite::Result<Genre> {
    Ok(Genre {
        id: row.get(0)?,
        name: row.get(1)?,
        slug: row.get(2)?,
        parent_id: row.get(3)?,
        children: Vec::new(),
        track_count: Some(row.get(4)?),
    })
}

fn build_genre_tree(genres: Vec<Genre>) -> Vec<Genre> {
    let mut children_by_parent: HashMap<Option<i64>, Vec<Genre>> = HashMap::new();
    for genre in genres {
        children_by_parent
            .entry(genre.parent_id)
            .or_default()
            .push(genre);
    }

    fn attach_children(
        parent_id: Option<i64>,
        children_by_parent: &mut HashMap<Option<i64>, Vec<Genre>>,
    ) -> Vec<Genre> {
        let mut children = children_by_parent.remove(&parent_id).unwrap_or_default();
        children.sort_by(|left, right| left.name.cmp(&right.name));

        for child in &mut children {
            child.children = attach_children(Some(child.id), children_by_parent);
            child.track_count = Some(aggregate_track_count(child));
        }

        children
    }

    attach_children(None, &mut children_by_parent)
}

fn aggregate_track_count(node: &Genre) -> i64 {
    node.track_count.unwrap_or(0) + node.children.iter().map(aggregate_track_count).sum::<i64>()
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

// ─── Track Similarity (Similar Radio) ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackSimilarityResult {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub similarity_score: f64,
    pub co_listen_score: f64,
    pub co_album_score: f64,
    pub co_artist_score: f64,
    pub genre_proximity: f64,
}

/// Compute similarity scores for all track pairs in the library.
/// Build pre-computed similarity pairs for the radio feature.
/// Fixes: Stage 1 now enumerates ALL pairs per album/artist (not just MIN/MAX).
///        Stage 2 uses indexed temp tables so scores merge correctly.
pub fn compute_track_similarity(conn: &Connection) -> Result<usize> {
    // Do the expensive work in temporary tables so the background rebuild does
    // not hold SQLite's single writer slot while playback is trying to update
    // playback_state. The real table is swapped in one short transaction at
    // the end.
    conn.execute_batch(
        "
        DROP TABLE IF EXISTS _track_similarity_build;
        CREATE TEMP TABLE _track_similarity_build (
            track_a INTEGER NOT NULL,
            track_b INTEGER NOT NULL,
            similarity_score REAL NOT NULL DEFAULT 0,
            co_listen_score REAL DEFAULT 0,
            co_album_score REAL DEFAULT 0,
            co_artist_score REAL DEFAULT 0,
            genre_proximity REAL DEFAULT 0,
            duration_proximity REAL DEFAULT 0,
            era_proximity REAL DEFAULT 0,
            computed_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (track_a, track_b),
            CHECK (track_a < track_b)
        );
    ",
    )?;

    // ── Stage 1: candidate pairs ─────────────────────────────────────────────
    // Each INSERT must satisfy CHECK (track_a < track_b), ensured by `b.id > a.id`.

    conn.execute_batch(
        "
        -- 1a: Same-album pairs (all combinations, not just min/max)
        INSERT OR IGNORE INTO _track_similarity_build (track_a, track_b)
        SELECT a.id, b.id
        FROM tracks a
        JOIN tracks b ON b.album_id = a.album_id AND b.id > a.id
        WHERE a.album_id IS NOT NULL;

        -- 1b: Same-artist pairs (cap at artists with <=100 tracks)
        INSERT OR IGNORE INTO _track_similarity_build (track_a, track_b)
        SELECT a.id, b.id
        FROM tracks a
        JOIN tracks b ON b.artist_id = a.artist_id AND b.id > a.id
        WHERE a.artist_id IN (
            SELECT artist_id FROM tracks GROUP BY artist_id HAVING COUNT(*) <= 100
        );

        -- 1c: Shared-genre pairs (deduplicated by GROUP BY, limited to avoid explosion)
        INSERT OR IGNORE INTO _track_similarity_build (track_a, track_b)
        SELECT a.track_id, b.track_id
        FROM track_genres a
        JOIN track_genres b ON b.genre_id = a.genre_id AND b.track_id > a.track_id
        GROUP BY a.track_id, b.track_id
        LIMIT 300000;
    ",
    )?;

    // ── Stage 2: aggregate signals into indexed temp tables ──────────────────

    // Per-(track, genre) weight = genre rarity (IDF) x tag confidence.
    //
    //   1. IDF: weight each genre by ln(total_tracks / members) so a rare genre (a
    //      tight cluster) dominates a broad one (almost no signal - knowing two
    //      tracks are both "Hip-Hop" in a hip-hop-heavy library says little). A
    //      genre covering the whole library weighs 0.
    //   2. Confidence: scale by track_genres.confidence, the scorer's per-tag trust
    //      (source strength x folksonomy vote count). A weakly-attested tag - a
    //      single-vote MusicBrainz "jazz" bleeding onto an emo-rap track - then
    //      contributes little, while a well-attested genre counts fully. Clamped to
    //      1.0 so an unusually high accumulated score can't let one genre dominate
    //      by raw magnitude.
    //
    // This replaces an earlier family-vote damping heuristic: once confidence is
    // calibrated (the count-saturation fix in genre/scorer.rs stops a lone vote from
    // scoring 1.0), confidence is the honest signal and the vote-count proxy is
    // unnecessary. Takes full effect once a MusicBrainz re-enrichment recomputes
    // stale confidences and this table is rebuilt.
    //
    // Computed in Rust and staged in a temp table because the bundled SQLite has no ln().
    let total_tracks: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS _track_genre_weight;
         CREATE TEMP TABLE _track_genre_weight (
             track_id INTEGER NOT NULL,
             genre_id INTEGER NOT NULL,
             weight REAL NOT NULL,
             PRIMARY KEY (track_id, genre_id)
         );",
    )?;
    if total_tracks > 0 {
        // genre_id -> IDF weight
        let mut idf: HashMap<i64, f64> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT genre_id, COUNT(DISTINCT track_id) FROM track_genres GROUP BY genre_id",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            for (genre_id, members) in rows {
                let weight = if members > 0 {
                    (total_tracks as f64 / members as f64).ln().max(0.0)
                } else {
                    0.0
                };
                idf.insert(genre_id, weight);
            }
        }

        // weight each (track, genre) by IDF x clamped confidence.
        let tags: Vec<(i64, i64, f64)> = {
            let mut stmt =
                conn.prepare("SELECT track_id, genre_id, confidence FROM track_genres")?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        let mut insert = conn.prepare(
            "INSERT OR IGNORE INTO _track_genre_weight (track_id, genre_id, weight)
             VALUES (?1, ?2, ?3)",
        )?;
        for (track_id, genre_id, confidence) in tags {
            let base = idf.get(&genre_id).copied().unwrap_or(0.0);
            let trust = confidence.clamp(0.0, 1.0);
            insert.execute(params![track_id, genre_id, base * trust])?;
        }
    }

    conn.execute_batch(
        "
        DROP TABLE IF EXISTS _co_listen;
        CREATE TEMP TABLE _co_listen AS
        SELECT
            MIN(a.track_id, b.track_id) AS ta,
            MAX(a.track_id, b.track_id) AS tb,
            CAST(COUNT(*) AS REAL) AS n
        FROM listen_history a
        JOIN listen_history b
            ON b.track_id != a.track_id
            AND b.started_at BETWEEN a.started_at AND datetime(a.started_at, '+30 minutes')
        WHERE a.started_at >= datetime('now', '-90 days')
        GROUP BY ta, tb
        HAVING COUNT(*) >= 2;
        CREATE INDEX _co_listen_idx ON _co_listen(ta, tb);

        -- A shared genre bridges the pair only as strongly as the weaker side
        -- believes it: MIN(a, b) means a damped mis-tag on one track can't inflate
        -- the match even if the other track holds that genre firmly.
        DROP TABLE IF EXISTS _genre_shared;
        CREATE TEMP TABLE _genre_shared AS
        SELECT a.track_id AS ta, b.track_id AS tb, SUM(MIN(a.weight, b.weight)) AS shared
        FROM _track_genre_weight a
        JOIN _track_genre_weight b ON b.genre_id = a.genre_id AND b.track_id > a.track_id
        GROUP BY a.track_id, b.track_id;
        CREATE INDEX _genre_shared_idx ON _genre_shared(ta, tb);

        -- Track → release year, for era_proximity. Albums table holds the year.
        DROP TABLE IF EXISTS _track_year;
        CREATE TEMP TABLE _track_year AS
        SELECT t.id AS track_id, al.year AS year
        FROM tracks t
        JOIN albums al ON al.id = t.album_id
        WHERE al.year IS NOT NULL;
        CREATE INDEX _track_year_idx ON _track_year(track_id);
    ",
    )?;

    // ── Stage 3: score each component ────────────────────────────────────────

    // co_album: 1.0 if same album
    conn.execute(
        "
        UPDATE _track_similarity_build SET co_album_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = _track_similarity_build.track_a
              AND b.id = _track_similarity_build.track_b
              AND a.album_id IS NOT NULL
              AND a.album_id = b.album_id
        )
    ",
        [],
    )?;

    // co_artist: 1.0 if same artist
    conn.execute(
        "
        UPDATE _track_similarity_build SET co_artist_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = _track_similarity_build.track_a
              AND b.id = _track_similarity_build.track_b
              AND a.artist_id IS NOT NULL
              AND a.artist_id = b.artist_id
        )
    ",
        [],
    )?;

    // genre_proximity: summed genre rarity for the pair, normalized by the highest
    // summed rarity across all pairs, so two tracks sharing rare genres score near
    // 1 and two sharing only a broad genre score near 0.
    conn.execute(
        "
        UPDATE _track_similarity_build SET genre_proximity = COALESCE((
            SELECT gs.shared / NULLIF((SELECT MAX(shared) FROM _genre_shared), 0)
            FROM _genre_shared gs
            WHERE gs.ta = _track_similarity_build.track_a AND gs.tb = _track_similarity_build.track_b
        ), 0)
    ",
        [],
    )?;

    // duration_proximity: 1 - |dur_a - dur_b| / 180s, clamped 0-1
    conn.execute(
        "
        UPDATE _track_similarity_build SET duration_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(a.duration_ms - b.duration_ms) AS REAL) / 180000.0, 1.0)
            FROM tracks a, tracks b
            WHERE a.id = _track_similarity_build.track_a AND b.id = _track_similarity_build.track_b
              AND a.duration_ms IS NOT NULL AND b.duration_ms IS NOT NULL
        ), 0)
    ",
        [],
    )?;

    // co_listen: normalized co-occurrence count
    conn.execute(
        "
        UPDATE _track_similarity_build SET co_listen_score = COALESCE((
            SELECT cl.n / NULLIF((SELECT MAX(n) FROM _co_listen), 0)
            FROM _co_listen cl
            WHERE cl.ta = _track_similarity_build.track_a AND cl.tb = _track_similarity_build.track_b
        ), 0)
    ",
        [],
    )?;

    // era_proximity: 1 - |year_a - year_b| / 25, clamped 0-1. Zero when either year is unknown.
    conn.execute(
        "
        UPDATE _track_similarity_build SET era_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(ya.year - yb.year) AS REAL) / 25.0, 1.0)
            FROM _track_year ya, _track_year yb
            WHERE ya.track_id = _track_similarity_build.track_a
              AND yb.track_id = _track_similarity_build.track_b
        ), 0)
    ",
        [],
    )?;

    // Final weighted score. era_proximity replaces some duration_proximity weight
    // because era is a stronger taste signal than song length.
    conn.execute(
        "
        UPDATE _track_similarity_build SET similarity_score =
            co_listen_score    * 0.30 +
            co_album_score     * 0.20 +
            co_artist_score    * 0.20 +
            genre_proximity    * 0.15 +
            era_proximity      * 0.10 +
            duration_proximity * 0.05
    ",
        [],
    )?;

    conn.execute_batch(
        "
        DROP TABLE IF EXISTS _co_listen;
        DROP TABLE IF EXISTS _genre_shared;
        DROP TABLE IF EXISTS _track_year;
        DROP TABLE IF EXISTS _track_genre_weight;
    ",
    )?;

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM _track_similarity_build", [], |row| {
        row.get(0)
    })?;

    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM track_similarity", [])?;
    tx.execute(
        "
        INSERT INTO track_similarity (
            track_a,
            track_b,
            similarity_score,
            co_listen_score,
            co_album_score,
            co_artist_score,
            genre_proximity,
            duration_proximity,
            era_proximity,
            computed_at
        )
        SELECT
            track_a,
            track_b,
            similarity_score,
            co_listen_score,
            co_album_score,
            co_artist_score,
            genre_proximity,
            duration_proximity,
            era_proximity,
            computed_at
        FROM _track_similarity_build
        ",
        [],
    )?;
    tx.commit()?;
    conn.execute_batch("DROP TABLE IF EXISTS _track_similarity_build;")?;
    Ok(count as usize)
}

/// Get similar tracks to a given track, ordered by similarity.
/// Returns up to `limit` tracks with similarity scores.
pub fn get_similar_tracks(
    conn: &Connection,
    track_id: i64,
    limit: i64,
    exclude_ids: &[i64],
) -> Result<Vec<TrackSimilarityResult>> {
    // For simplicity, handle exclude via post-filtering (limit is small, ~20-50)
    let sql = "SELECT t.id, t.title, a.name, al.title, al.artwork_url,
                      t.duration_ms, t.best_quality,
                      ts.similarity_score, ts.co_listen_score, ts.co_album_score,
                      ts.co_artist_score, ts.genre_proximity
               FROM track_similarity ts
               JOIN tracks t ON t.id = CASE
                   WHEN ts.track_a = ?1 THEN ts.track_b
                   ELSE ts.track_a
               END
               LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
               WHERE (ts.track_a = ?1 OR ts.track_b = ?1)
                 AND t.id != ?1
               ORDER BY ts.similarity_score DESC
               LIMIT ?2";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![track_id, limit], |row| {
        Ok(TrackSimilarityResult {
            track_id: row.get(0)?,
            title: row.get(1)?,
            artist_name: row.get(2)?,
            album_title: row.get(3)?,
            artwork_url: row.get(4)?,
            duration_ms: row.get(5)?,
            best_quality: row.get(6)?,
            similarity_score: row.get(7)?,
            co_listen_score: row.get(8)?,
            co_album_score: row.get(9)?,
            co_artist_score: row.get(10)?,
            genre_proximity: row.get(11)?,
        })
    })?;

    let mut results: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;

    // Post-filter excluded IDs
    if !exclude_ids.is_empty() {
        let exclude_set: HashSet<i64> = exclude_ids.iter().copied().collect();
        results.retain(|r| !exclude_set.contains(&r.track_id));
    }

    Ok(results)
}

/// Get similarity computation status
pub fn get_similarity_computed_at(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT MAX(computed_at) FROM track_similarity", [], |row| {
            row.get(0)
        })
        .optional()?)
}

/// Row count of the precomputed `track_similarity` index.
pub fn count_track_similarity(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM track_similarity", [], |row| {
            row.get(0)
        })?,
    )
}

/// Start timestamp of the last successful radio similarity rebuild. Recorded in
/// `server_config` independently of row count — a valid library can produce
/// zero similarity pairs, so an empty table is not the same as "never built".
pub fn get_radio_similarity_built_at(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'radio_similarity_built_at'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

/// True when a discovery training run is in progress. The radio similarity
/// rebuild must not run alongside training: training writes heavily through the
/// shared connection, and the rebuild's long write transaction would starve it
/// past the busy timeout and fail the run.
pub fn is_discovery_training_running(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM training_runs WHERE status = 'running')",
        [],
        |row| row.get(0),
    )?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingTrackRow {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub source: String,
    pub play_count: i32,
    pub is_favorite: bool,
    pub playlist_memberships: i64,
    pub genre_paths: Vec<String>,
    // DSP features (None if not yet analyzed)
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub camelot_key: Option<String>,
    pub danceability: Option<f64>,
    pub beat_strength: Option<f64>,
    pub loudness_lufs: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingNeighborRow {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub score: f64,
    pub behavioral_score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
    pub confidence: f64,
    pub support_count: i64,
    pub support_transition: f64,
    pub support_colisten: f64,
    pub support_structure: f64,
    pub support_metadata: f64,
    pub candidate_in_degree: i64,
    pub candidate_in_degree_percentile: f64,
    pub play_count_seed: i64,
    pub play_count_candidate: i64,
    pub primary_reason: Option<String>,
}

// Trainer write payload. Replaces the 9-tuple that replace_track_neighbors used
// to take — at 16 fields, named struct fields are necessary for any chance at
// not mixing up arguments.
#[derive(Debug, Clone)]
pub struct NeighborWriteRow {
    pub track_id: i64,
    pub neighbor_track_id: i64,
    pub rank: i32,
    pub score: f64,
    pub behavioral_score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
    pub primary_reason: Option<String>,
    pub confidence: f64,
    pub support_count: i64,
    pub support_transition: f64,
    pub support_colisten: f64,
    pub support_structure: f64,
    pub support_metadata: f64,
    pub candidate_in_degree: i64,
    pub candidate_in_degree_percentile: f64,
    pub play_count_seed: i64,
    pub play_count_candidate: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTrackCandidateRow {
    pub id: i64,
    pub tidal_id: Option<i64>,
    pub mbid: Option<String>,
    pub dedupe_key: String,
    pub title: String,
    pub artist_name: String,
    pub genre_tags_json: Option<String>,
    pub duration_ms: Option<i64>,
    pub expires_at: String,
    pub updated_at: String,
    pub source_tags_json: Option<String>,
    pub resolved_track_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedLastfmExternalSightingRow {
    pub seed_track_id: i64,
    pub resolved_track_id: i64,
    pub similarity: f64,
    pub source_payload_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTidalResolutionCandidateRow {
    pub id: i64,
    pub title: String,
    pub artist_name: String,
    pub duration_ms: Option<i64>,
    pub sighting_count: i64,
    pub max_similarity: Option<f64>,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTidalSimilarSeedRow {
    pub track_id: i64,
    pub artist_tidal_id: i64,
}

#[derive(Debug, Clone)]
struct ExternalCandidateFallbackIdentity {
    normalized_artist_name: String,
    normalized_title: String,
    duration_bucket: i64,
}

#[derive(Debug, Clone)]
pub struct ExternalTrackCandidateUpsert {
    pub tidal_id: Option<i64>,
    pub mbid: Option<String>,
    pub dedupe_key: String,
    pub title: String,
    pub artist_name: String,
    pub genre_tags_json: Option<String>,
    pub duration_ms: Option<i64>,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct ExternalCandidateTidalResolution {
    pub tidal_id: i64,
    pub genre_tags_json: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ExternalCandidateSightingUpsert {
    pub candidate_id: i64,
    pub seed_track_id: i64,
    pub source: String,
    pub source_payload_json: Option<String>,
    pub similarity: Option<f64>,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct ExternalCandidateNeighborWriteRow {
    pub candidate_id: i64,
    pub rank: i32,
    pub score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCandidateNeighborRow {
    pub candidate_id: i64,
    pub tidal_id: Option<i64>,
    pub title: String,
    pub artist_name: String,
    pub duration_ms: Option<i64>,
    pub rank: i32,
    pub score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEmbeddingRow {
    pub track_id: i64,
    pub vector_blob: Vec<u8>,
    pub l2_norm: f64,
}

#[allow(dead_code)]
pub fn upsert_embedding_model(
    conn: &Connection,
    model_key: &str,
    family: &str,
    dimension: i32,
    status: &str,
    config_json: Option<&str>,
) -> Result<EmbeddingModel> {
    conn.execute(
        "INSERT INTO embedding_models (model_key, family, dimension, status, config_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(model_key) DO UPDATE SET
             family = excluded.family,
             dimension = excluded.dimension,
             status = excluded.status,
             config_json = excluded.config_json",
        params![model_key, family, dimension, status, config_json],
    )?;

    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models WHERE model_key = ?1",
        params![model_key],
        |row| {
            Ok(EmbeddingModel {
                id: row.get(0)?,
                model_key: row.get(1)?,
                family: row.get(2)?,
                dimension: row.get(3)?,
                status: row.get(4)?,
                is_active: row.get(5)?,
                trained_at: row.get(6)?,
                config_json: row.get(7)?,
                metrics_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn create_embedding_model(
    conn: &Connection,
    model_key: &str,
    family: &str,
    dimension: i32,
    status: &str,
    config_json: Option<&str>,
) -> Result<EmbeddingModel> {
    conn.execute(
        "INSERT INTO embedding_models (model_key, family, dimension, status, config_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![model_key, family, dimension, status, config_json],
    )?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models WHERE id = ?1",
        params![id],
        |row| {
            Ok(EmbeddingModel {
                id: row.get(0)?,
                model_key: row.get(1)?,
                family: row.get(2)?,
                dimension: row.get(3)?,
                status: row.get(4)?,
                is_active: row.get(5)?,
                trained_at: row.get(6)?,
                config_json: row.get(7)?,
                metrics_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn update_embedding_model_metrics(
    conn: &Connection,
    model_id: i64,
    status: &str,
    metrics_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE embedding_models
         SET status = ?2, metrics_json = ?3, trained_at = datetime('now')
         WHERE id = ?1",
        params![model_id, status, metrics_json],
    )?;
    Ok(())
}

pub fn fail_embedding_model(conn: &Connection, model_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE embedding_models
         SET status = 'failed'
         WHERE id = ?1",
        params![model_id],
    )?;
    Ok(())
}

fn read_embedding_model(row: &Row<'_>) -> rusqlite::Result<EmbeddingModel> {
    Ok(EmbeddingModel {
        id: row.get(0)?,
        model_key: row.get(1)?,
        family: row.get(2)?,
        dimension: row.get(3)?,
        status: row.get(4)?,
        is_active: row.get(5)?,
        trained_at: row.get(6)?,
        config_json: row.get(7)?,
        metrics_json: row.get(8)?,
        created_at: row.get(9)?,
    })
}

pub fn get_ready_embedding_model_for_family(
    conn: &Connection,
    family: &str,
) -> Result<Option<EmbeddingModel>> {
    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models
         WHERE family = ?1 AND status = 'ready'
         ORDER BY is_active DESC, trained_at DESC, id DESC
         LIMIT 1",
        params![family],
        read_embedding_model,
    )
    .optional()
    .map_err(Into::into)
}

pub fn discovery_model_family_for_engine(engine: &str) -> &'static str {
    match engine.trim().to_ascii_lowercase().as_str() {
        DISCOVERY_ENGINE_V1 => DISCOVERY_ENGINE_V1_FAMILY,
        _ => DISCOVERY_ENGINE_V2_FAMILY,
    }
}

pub fn selected_discovery_engine(conn: &Connection) -> Result<String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'discovery_engine'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let normalized = match raw.as_deref().map(str::trim) {
        Some(DISCOVERY_ENGINE_V1) => DISCOVERY_ENGINE_V1,
        _ => DISCOVERY_ENGINE_V2,
    };
    Ok(normalized.to_string())
}

pub fn get_selected_discovery_embedding_model(conn: &Connection) -> Result<Option<EmbeddingModel>> {
    let engine = selected_discovery_engine(conn)?;
    get_ready_embedding_model_for_family(conn, discovery_model_family_for_engine(&engine))
}

pub fn deactivate_embedding_models(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE embedding_models SET is_active = 0", [])?;
    Ok(())
}

pub fn activate_embedding_model(conn: &Connection, model_id: i64) -> Result<()> {
    deactivate_embedding_models(conn)?;
    conn.execute(
        "UPDATE embedding_models SET is_active = 1, status = 'ready', trained_at = datetime('now')
         WHERE id = ?1",
        params![model_id],
    )?;
    Ok(())
}

pub fn create_training_run(
    conn: &Connection,
    model_id: Option<i64>,
    stage: &str,
    status: &str,
) -> Result<DiscoveryTrainingRun> {
    conn.execute(
        "INSERT INTO training_runs (model_id, stage, status)
         VALUES (?1, ?2, ?3)",
        params![model_id, stage, status],
    )?;
    let id = conn.last_insert_rowid();
    get_training_run(conn, id)?.ok_or_else(|| anyhow::anyhow!("training run missing after insert"))
}

pub fn update_training_run_model(conn: &Connection, run_id: i64, model_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE training_runs SET model_id = ?2 WHERE id = ?1",
        params![run_id, model_id],
    )?;
    Ok(())
}

pub fn get_training_run(conn: &Connection, run_id: i64) -> Result<Option<DiscoveryTrainingRun>> {
    conn.query_row(
        "SELECT id, model_id, stage, status, progress, items_total, items_done, started_at, finished_at, error_text
         FROM training_runs WHERE id = ?1",
        params![run_id],
        |row| {
            Ok(DiscoveryTrainingRun {
                id: row.get(0)?,
                model_id: row.get(1)?,
                stage: row.get(2)?,
                status: row.get(3)?,
                progress: row.get(4)?,
                items_total: row.get(5)?,
                items_done: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                error_text: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_latest_training_run(conn: &Connection) -> Result<Option<DiscoveryTrainingRun>> {
    conn.query_row(
        "SELECT id, model_id, stage, status, progress, items_total, items_done, started_at, finished_at, error_text
         FROM training_runs
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
        [],
        |row| {
            Ok(DiscoveryTrainingRun {
                id: row.get(0)?,
                model_id: row.get(1)?,
                stage: row.get(2)?,
                status: row.get(3)?,
                progress: row.get(4)?,
                items_total: row.get(5)?,
                items_done: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                error_text: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn update_training_run_progress(
    conn: &Connection,
    run_id: i64,
    stage: &str,
    status: &str,
    progress: f64,
    items_total: Option<i64>,
    items_done: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET stage = ?2, status = ?3, progress = ?4, items_total = ?5, items_done = ?6
         WHERE id = ?1",
        params![run_id, stage, status, progress, items_total, items_done],
    )?;
    Ok(())
}

pub fn finish_training_run(conn: &Connection, run_id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = ?2, progress = 1.0, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, status],
    )?;
    Ok(())
}

pub fn finish_training_run_with_error(
    conn: &Connection,
    run_id: i64,
    status: &str,
    error_text: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = ?2, progress = 1.0, error_text = ?3, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, status, error_text],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn fail_training_run(conn: &Connection, run_id: i64, error_text: &str) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = 'failed', error_text = ?2, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, error_text],
    )?;
    Ok(())
}

pub fn replace_track_embeddings(
    conn: &Connection,
    model_id: i64,
    embeddings: &[(i64, Vec<u8>, f64)],
) -> Result<()> {
    // Refuse to wipe an already-populated model with an empty payload. The
    // trainer is supposed to bail before reaching here when it produces no
    // output (cancel checks at every stage), but a logic bug or panic-recovery
    // path could still get us here with an empty slice — and a silent wipe
    // turns a recoverable issue into a "discovery engine just died" bug for
    // the user. Leave the prior rows in place; tracing makes the skip visible.
    if embeddings.is_empty() {
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM track_embeddings WHERE model_id = ?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            tracing::warn!(
                target: "noor.discovery.training",
                model_id,
                existing_rows = existing,
                "skipping embedding wipe: trainer returned 0 vectors but model has prior data"
            );
            return Ok(());
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_embeddings WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_embeddings (track_id, model_id, vector_blob, l2_norm)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (track_id, blob, norm) in embeddings {
            stmt.execute(params![track_id, model_id, blob, norm])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Cached row from `track_audio_features`, used by Incremental Refresh to
/// avoid recomputing the audio-proxy stage when the prior run's features are
/// still valid. Caller is responsible for filtering rows whose unpacked
/// vector dimension doesn't match the current intensity tier.
pub struct CachedAudioFeatureRow {
    pub track_id: i64,
    pub feature_version: String,
    pub vector_blob: Vec<u8>,
    pub clip_start_ms: i64,
    pub clip_duration_ms: i64,
}

pub fn get_cached_audio_features(conn: &Connection) -> Result<Vec<CachedAudioFeatureRow>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms
         FROM track_audio_features",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CachedAudioFeatureRow {
                track_id: row.get(0)?,
                feature_version: row.get(1)?,
                vector_blob: row.get(2)?,
                clip_start_ms: row.get(3)?,
                clip_duration_ms: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn replace_track_audio_features(
    conn: &Connection,
    features: &[(i64, String, Vec<u8>, i64, i64)],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_audio_features (track_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(track_id) DO UPDATE SET
                 feature_version = excluded.feature_version,
                 vector_blob = excluded.vector_blob,
                 clip_start_ms = excluded.clip_start_ms,
                 clip_duration_ms = excluded.clip_duration_ms,
                 computed_at = datetime('now')",
        )?;
        for (track_id, version, blob, start_ms, duration_ms) in features {
            stmt.execute(params![track_id, version, blob, start_ms, duration_ms])?;
        }
    }
    tx.commit()?;
    Ok(())
}

// Per-reason held-out hit-rate row, mirroring the structure emitted by the
// trainer. Kept as a separate struct on the queries side so the trainer module
// doesn't need to depend on rusqlite param plumbing.
pub struct ReasonHitRateRow {
    pub primary_reason: String,
    pub impressions: i64,
    pub hits: i64,
    pub hit_rate: f64,
    pub mean_rank: Option<f64>,
    pub mrr_contribution: f64,
    pub insufficient_data: bool,
}

// Replaces all per-reason hit-rate rows for a model. Wrapped in a transaction
// so a partial replacement can't leave stale rows from a prior training run.
pub fn replace_discovery_diagnostics(
    conn: &Connection,
    model_id: i64,
    rates: &[ReasonHitRateRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM discovery_diagnostics WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO discovery_diagnostics
             (model_id, primary_reason, impressions, hits, hit_rate,
              mean_rank, mrr_contribution, insufficient_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for r in rates {
            stmt.execute(params![
                model_id,
                r.primary_reason,
                r.impressions,
                r.hits,
                r.hit_rate,
                r.mean_rank,
                r.mrr_contribution,
                r.insufficient_data as i32,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[allow(dead_code)]
pub fn get_per_reason_hit_rates(conn: &Connection, model_id: i64) -> Result<Vec<ReasonHitRateRow>> {
    let mut stmt = conn.prepare(
        "SELECT primary_reason, impressions, hits, hit_rate,
                mean_rank, mrr_contribution, insufficient_data
         FROM discovery_diagnostics
         WHERE model_id = ?1
         ORDER BY impressions DESC",
    )?;
    let rows = stmt
        .query_map(params![model_id], |row| {
            Ok(ReasonHitRateRow {
                primary_reason: row.get(0)?,
                impressions: row.get(1)?,
                hits: row.get(2)?,
                hit_rate: row.get(3)?,
                mean_rank: row.get(4)?,
                mrr_contribution: row.get(5)?,
                insufficient_data: row.get::<_, i32>(6)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn replace_track_neighbors(
    conn: &Connection,
    model_id: i64,
    neighbors: &[NeighborWriteRow],
) -> Result<()> {
    // Single transaction: ~2M+ INSERTs auto-committing one-by-one is what makes
    // training appear to hang. Batching also makes the DELETE+INSERT atomic so a
    // killed process can't leave the table half-populated.
    //
    // Same defensive skip as `replace_track_embeddings`: an empty slice on a
    // populated model leaves the prior graph intact rather than wiping it.
    if neighbors.is_empty() {
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM track_neighbors WHERE model_id = ?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            tracing::warn!(
                target: "noor.discovery.training",
                model_id,
                existing_rows = existing,
                "skipping neighbor wipe: trainer returned 0 edges but model has prior data"
            );
            return Ok(());
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_neighbors WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_neighbors
             (track_id, neighbor_track_id, model_id, rank, score,
              behavioral_score, audio_score, metadata_score, reason_json, primary_reason,
              confidence, support_count, support_transition, support_colisten, support_structure,
              support_metadata, candidate_in_degree, candidate_in_degree_percentile,
              play_count_seed, play_count_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        )?;
        for n in neighbors {
            stmt.execute(params![
                n.track_id,
                n.neighbor_track_id,
                model_id,
                n.rank,
                n.score,
                n.behavioral_score,
                n.audio_score,
                n.metadata_score,
                n.reason_json,
                n.primary_reason,
                n.confidence,
                n.support_count,
                n.support_transition,
                n.support_colisten,
                n.support_structure,
                n.support_metadata,
                n.candidate_in_degree,
                n.candidate_in_degree_percentile,
                n.play_count_seed,
                n.play_count_candidate,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Replace neighbor rows for a single seed track only — used by the background
/// per-seed refresh so it doesn't wipe every other track's neighbors.
pub fn replace_seed_neighbors(
    conn: &Connection,
    model_id: i64,
    seed_id: i64,
    rows: &[NeighborWriteRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_neighbors WHERE model_id = ?1 AND track_id = ?2",
        params![model_id, seed_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_neighbors
             (track_id, neighbor_track_id, model_id, rank, score,
              behavioral_score, audio_score, metadata_score, reason_json, primary_reason,
              confidence, support_count, support_transition, support_colisten, support_structure,
              support_metadata, candidate_in_degree, candidate_in_degree_percentile,
              play_count_seed, play_count_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        )?;
        for n in rows {
            stmt.execute(params![
                n.track_id,
                n.neighbor_track_id,
                model_id,
                n.rank,
                n.score,
                n.behavioral_score,
                n.audio_score,
                n.metadata_score,
                n.reason_json,
                n.primary_reason,
                n.confidence,
                n.support_count,
                n.support_transition,
                n.support_colisten,
                n.support_structure,
                n.support_metadata,
                n.candidate_in_degree,
                n.candidate_in_degree_percentile,
                n.play_count_seed,
                n.play_count_candidate,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_track_neighbors(
    conn: &Connection,
    model_id: i64,
    track_id: i64,
    limit: i64,
    exclude_ids: &[i64],
) -> Result<Vec<EmbeddingNeighborRow>> {
    let sql =
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.best_quality,
                      n.score, n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json,
                      n.confidence, n.support_count, n.candidate_in_degree,
                      n.candidate_in_degree_percentile, n.play_count_seed, n.play_count_candidate,
                      n.primary_reason, n.support_transition, n.support_colisten,
                      n.support_structure, n.support_metadata
               FROM track_neighbors n
               JOIN tracks t ON t.id = n.neighbor_track_id
               LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
               WHERE n.model_id = ?1 AND n.track_id = ?2
               ORDER BY n.rank ASC
               LIMIT ?3";
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt
        .query_map(params![model_id, track_id, limit.max(1)], |row| {
            Ok(EmbeddingNeighborRow {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                duration_ms: row.get(5)?,
                best_quality: row.get(6)?,
                score: row.get(7)?,
                behavioral_score: row.get(8)?,
                audio_score: row.get(9)?,
                metadata_score: row.get(10)?,
                reason_json: row.get(11)?,
                confidence: row.get(12)?,
                support_count: row.get(13)?,
                candidate_in_degree: row.get(14)?,
                candidate_in_degree_percentile: row.get(15)?,
                play_count_seed: row.get(16)?,
                play_count_candidate: row.get(17)?,
                primary_reason: row.get(18)?,
                support_transition: row.get(19)?,
                support_colisten: row.get(20)?,
                support_structure: row.get(21)?,
                support_metadata: row.get(22)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !exclude_ids.is_empty() {
        let exclude = exclude_ids.iter().copied().collect::<HashSet<_>>();
        rows.retain(|row| !exclude.contains(&row.track_id));
    }
    Ok(rows)
}

pub fn get_track_neighbors_for_seeds(
    conn: &Connection,
    model_id: i64,
    seed_ids: &[i64],
    limit_per_seed: i64,
) -> Result<HashMap<i64, Vec<EmbeddingNeighborRow>>> {
    if seed_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = (0..seed_ids.len())
        .map(|idx| format!("?{}", idx + 3))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT n.track_id, t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.best_quality,
                n.score, n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json,
                n.confidence, n.support_count, n.candidate_in_degree,
                n.candidate_in_degree_percentile, n.play_count_seed, n.play_count_candidate,
                n.primary_reason, n.support_transition, n.support_colisten,
                n.support_structure, n.support_metadata
         FROM track_neighbors n
         JOIN tracks t ON t.id = n.neighbor_track_id
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE n.model_id = ?1
           AND n.rank <= ?2
           AND n.track_id IN ({placeholders})
         ORDER BY n.track_id ASC, n.rank ASC"
    );
    let mut values = vec![model_id, limit_per_seed.max(1)];
    values.extend(seed_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(values.iter()), |row| {
            let seed_id = row.get::<_, i64>(0)?;
            let neighbor = EmbeddingNeighborRow {
                track_id: row.get(1)?,
                title: row.get(2)?,
                artist_name: row.get(3)?,
                album_title: row.get(4)?,
                artwork_url: row.get(5)?,
                duration_ms: row.get(6)?,
                best_quality: row.get(7)?,
                score: row.get(8)?,
                behavioral_score: row.get(9)?,
                audio_score: row.get(10)?,
                metadata_score: row.get(11)?,
                reason_json: row.get(12)?,
                confidence: row.get(13)?,
                support_count: row.get(14)?,
                candidate_in_degree: row.get(15)?,
                candidate_in_degree_percentile: row.get(16)?,
                play_count_seed: row.get(17)?,
                play_count_candidate: row.get(18)?,
                primary_reason: row.get(19)?,
                support_transition: row.get(20)?,
                support_colisten: row.get(21)?,
                support_structure: row.get(22)?,
                support_metadata: row.get(23)?,
            };
            Ok((seed_id, neighbor))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<EmbeddingNeighborRow>> = HashMap::new();
    for (seed_id, neighbor) in rows {
        grouped.entry(seed_id).or_default().push(neighbor);
    }
    Ok(grouped)
}

/// Artist-level "hub-ness": for each requested artist, the highest in-degree
/// percentile any of that artist's tracks carries as a neighbour, across every
/// model. In most libraries a few artists end up over-connected in the similarity
/// graph (heavy co-listen history, a large catalogue, broad genre tags) and get
/// listed as a neighbour for a huge share of seeds. That pollution is artist-wide,
/// not per-track: an artist's deep cuts can have a low individual in-degree yet
/// still ride into every pool because the *artist* is a top neighbour everywhere.
/// Keying on the artist's max in-degree lets one genuinely hubby track flag the
/// whole catalogue, which is what catches that low-in-degree filler. Artists with
/// no neighbour rows are omitted, so the caller treats a missing entry as 0.
pub fn get_artist_hub_percentiles(
    conn: &Connection,
    artist_ids: &[i64],
) -> Result<HashMap<i64, f64>> {
    let mut out = HashMap::new();
    if artist_ids.is_empty() {
        return Ok(out);
    }
    // Chunk well under SQLite's bound-parameter ceiling.
    for chunk in artist_ids.chunks(400) {
        let placeholders = (0..chunk.len())
            .map(|idx| format!("?{}", idx + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT t.artist_id, MAX(n.candidate_in_degree_percentile)
             FROM track_neighbors n
             JOIN tracks t ON t.id = n.neighbor_track_id
             WHERE t.artist_id IN ({placeholders})
             GROUP BY t.artist_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?))
        })?;
        for row in rows {
            let (artist_id, pct) = row?;
            if let Some(pct) = pct {
                out.insert(artist_id, pct);
            }
        }
    }
    Ok(out)
}

fn get_external_track_candidate_by_id(
    conn: &Connection,
    id: i64,
) -> Result<ExternalTrackCandidateRow> {
    conn.query_row(
        "SELECT id, tidal_id, mbid, dedupe_key, title, artist_name, genre_tags_json,
                duration_ms, expires_at, updated_at, NULL AS source_tags_json, resolved_track_id
         FROM external_track_candidates
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(ExternalTrackCandidateRow {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                mbid: row.get(2)?,
                dedupe_key: row.get(3)?,
                title: row.get(4)?,
                artist_name: row.get(5)?,
                genre_tags_json: row.get(6)?,
                duration_ms: row.get(7)?,
                expires_at: row.get(8)?,
                updated_at: row.get(9)?,
                source_tags_json: row.get(10)?,
                resolved_track_id: row.get(11)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn get_external_track_candidates_for_training(
    conn: &Connection,
    now: &str,
    limit: i64,
) -> Result<Vec<ExternalTrackCandidateRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_id, mbid, dedupe_key, title, artist_name, genre_tags_json,
                duration_ms, expires_at, updated_at,
                (
                    SELECT json_group_array(tag)
                    FROM (
                        SELECT DISTINCT source AS tag
                        FROM external_track_candidate_sightings s
                        WHERE s.candidate_id = c.id
                          AND s.expires_at > ?1
                        UNION
                        SELECT DISTINCT
                            CASE
                                WHEN s.source = 'lastfm_similar'
                                 AND s.source_payload_json LIKE '%\"branch_from\":%'
                                 AND s.source_payload_json NOT LIKE '%\"branch_from\":null%'
                                THEN 'lastfm_branch'
                                WHEN s.source = 'lastfm_similar'
                                THEN 'lastfm_direct'
                            END AS tag
                        FROM external_track_candidate_sightings s
                        WHERE s.candidate_id = c.id
                          AND s.expires_at > ?1
                          AND s.source = 'lastfm_similar'
                        ORDER BY tag
                    )
                    WHERE tag IS NOT NULL
                ) AS source_tags_json,
                resolved_track_id
         FROM external_track_candidates c
         WHERE c.expires_at > ?1
           AND c.resolved_track_id IS NULL
         ORDER BY c.updated_at DESC, c.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit.max(1)], |row| {
            Ok(ExternalTrackCandidateRow {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                mbid: row.get(2)?,
                dedupe_key: row.get(3)?,
                title: row.get(4)?,
                artist_name: row.get(5)?,
                genre_tags_json: row.get(6)?,
                duration_ms: row.get(7)?,
                expires_at: row.get(8)?,
                updated_at: row.get(9)?,
                source_tags_json: row.get(10)?,
                resolved_track_id: row.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_resolved_lastfm_external_sightings_for_training(
    conn: &Connection,
    now: &str,
    limit: i64,
) -> Result<Vec<ResolvedLastfmExternalSightingRow>> {
    let mut stmt = conn.prepare(
        "SELECT s.seed_track_id,
                c.resolved_track_id,
                COALESCE(s.similarity, 0.0),
                s.source_payload_json
         FROM external_track_candidate_sightings s
         JOIN external_track_candidates c ON c.id = s.candidate_id
         WHERE s.source = 'lastfm_similar'
           AND s.expires_at > ?1
           AND c.expires_at > ?1
           AND c.resolved_track_id IS NOT NULL
           AND c.resolved_track_id <> s.seed_track_id
         ORDER BY COALESCE(s.similarity, 0.0) DESC,
                  s.expires_at DESC,
                  s.candidate_id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit.max(1)], |row| {
            Ok(ResolvedLastfmExternalSightingRow {
                seed_track_id: row.get(0)?,
                resolved_track_id: row.get(1)?,
                similarity: row.get(2)?,
                source_payload_json: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_unresolved_lastfm_external_candidates_for_tidal_resolution(
    conn: &Connection,
    now: &str,
    limit: i64,
) -> Result<Vec<ExternalTidalResolutionCandidateRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.title, c.artist_name, c.duration_ms,
                COUNT(s.seed_track_id) AS sighting_count,
                MAX(s.similarity) AS max_similarity,
                c.expires_at
         FROM external_track_candidates c
         JOIN external_track_candidate_sightings s ON s.candidate_id = c.id
         WHERE c.expires_at > ?1
           AND c.resolved_track_id IS NULL
           AND c.tidal_id IS NULL
           AND s.source = 'lastfm_similar'
           AND s.expires_at > ?1
         GROUP BY c.id
         ORDER BY sighting_count DESC,
                  COALESCE(max_similarity, 0) DESC,
                  c.expires_at DESC,
                  c.updated_at DESC,
                  c.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit.max(1)], |row| {
            Ok(ExternalTidalResolutionCandidateRow {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                duration_ms: row.get(3)?,
                sighting_count: row.get(4)?,
                max_similarity: row.get(5)?,
                expires_at: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn count_playable_external_candidates(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM external_track_candidates
         WHERE tidal_id IS NOT NULL
           AND resolved_track_id IS NULL
           AND expires_at > datetime('now')",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn get_tidal_similar_seed_rows(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<ExternalTidalSimilarSeedRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, ar.tidal_id
         FROM tracks t
         JOIN artists ar ON ar.id = t.artist_id
         WHERE t.tidal_id IS NOT NULL
           AND ar.tidal_id IS NOT NULL
         ORDER BY t.play_count DESC, t.last_played_at DESC, t.id DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(ExternalTidalSimilarSeedRow {
                track_id: row.get(0)?,
                artist_tidal_id: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn upsert_external_track_candidate(
    conn: &Connection,
    input: &ExternalTrackCandidateUpsert,
) -> Result<ExternalTrackCandidateRow> {
    let fallback_identity =
        external_candidate_fallback_identity(&input.artist_name, &input.title, input.duration_ms);
    let existing_id = if let Some(tidal_id) = input.tidal_id {
        conn.query_row(
            "SELECT id FROM external_track_candidates WHERE tidal_id = ?1",
            params![tidal_id],
            |row| row.get(0),
        )
        .optional()?
    } else if let Some(mbid) = input.mbid.as_deref() {
        conn.query_row(
            "SELECT id FROM external_track_candidates WHERE mbid = ?1",
            params![mbid],
            |row| row.get(0),
        )
        .optional()?
    } else {
        conn.query_row(
            "SELECT id FROM external_track_candidates
             WHERE dedupe_key = ?1
                OR (
                    tidal_id IS NULL
                    AND mbid IS NULL
                    AND
                    normalized_artist_name = ?2
                    AND normalized_title = ?3
                    AND duration_bucket = ?4
                )
             LIMIT 1",
            params![
                input.dedupe_key,
                &fallback_identity.normalized_artist_name,
                &fallback_identity.normalized_title,
                fallback_identity.duration_bucket,
            ],
            |row| row.get(0),
        )
        .optional()?
    };

    let id = if let Some(id) = existing_id {
        conn.execute(
            "UPDATE external_track_candidates
             SET tidal_id = COALESCE(?2, tidal_id),
                 mbid = COALESCE(?3, mbid),
                 dedupe_key = ?4,
                 normalized_artist_name = ?5,
                 normalized_title = ?6,
                 duration_bucket = ?7,
                 title = ?8,
                 artist_name = ?9,
                 genre_tags_json = ?10,
                 duration_ms = ?11,
                 expires_at = ?12,
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![
                id,
                input.tidal_id,
                input.mbid,
                input.dedupe_key,
                &fallback_identity.normalized_artist_name,
                &fallback_identity.normalized_title,
                fallback_identity.duration_bucket,
                input.title,
                input.artist_name,
                input.genre_tags_json,
                input.duration_ms,
                input.expires_at,
            ],
        )?;
        id
    } else {
        conn.execute(
            "INSERT INTO external_track_candidates
             (tidal_id, mbid, dedupe_key, normalized_artist_name, normalized_title,
              duration_bucket, title, artist_name, genre_tags_json, duration_ms, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                input.tidal_id,
                input.mbid,
                input.dedupe_key,
                &fallback_identity.normalized_artist_name,
                &fallback_identity.normalized_title,
                fallback_identity.duration_bucket,
                input.title,
                input.artist_name,
                input.genre_tags_json,
                input.duration_ms,
                input.expires_at,
            ],
        )?;
        conn.last_insert_rowid()
    };

    get_external_track_candidate_by_id(conn, id)
}

pub fn resolve_external_candidate_tidal_metadata(
    conn: &Connection,
    candidate_id: i64,
    input: &ExternalCandidateTidalResolution,
) -> Result<ExternalTrackCandidateRow> {
    if input.tidal_id <= 0 {
        bail!("external candidate tidal_id must be positive");
    }

    if let Some(existing_id) = conn
        .query_row(
            "SELECT id FROM external_track_candidates WHERE tidal_id = ?1 AND id <> ?2",
            params![input.tidal_id, candidate_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        conn.execute(
            "UPDATE external_track_candidates
             SET genre_tags_json = COALESCE(genre_tags_json, ?2),
                 duration_ms = COALESCE(duration_ms, ?3),
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![existing_id, input.genre_tags_json, input.duration_ms],
        )?;
        merge_external_track_candidates(conn, existing_id, candidate_id)?;
        return get_external_track_candidate_by_id(conn, existing_id);
    }

    conn.execute(
        "UPDATE external_track_candidates
         SET tidal_id = ?2,
             dedupe_key = ?3,
             genre_tags_json = COALESCE(genre_tags_json, ?4),
             duration_ms = COALESCE(duration_ms, ?5),
             updated_at = datetime('now')
         WHERE id = ?1",
        params![
            candidate_id,
            input.tidal_id,
            format!("tidal:{}", input.tidal_id),
            input.genre_tags_json,
            input.duration_ms,
        ],
    )?;
    get_external_track_candidate_by_id(conn, candidate_id)
}

fn external_candidate_fallback_identity(
    artist_name: &str,
    title: &str,
    duration_ms: Option<i64>,
) -> ExternalCandidateFallbackIdentity {
    ExternalCandidateFallbackIdentity {
        normalized_artist_name: normalize_external_candidate_text(artist_name),
        normalized_title: normalize_external_candidate_text(title),
        duration_bucket: duration_ms.map(|value| value / 30_000).unwrap_or(0),
    }
}

fn normalize_external_candidate_text(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn upsert_external_candidate_sighting(
    conn: &Connection,
    input: &ExternalCandidateSightingUpsert,
) -> Result<()> {
    conn.execute(
        "INSERT INTO external_track_candidate_sightings
         (candidate_id, seed_track_id, source, source_payload_json, similarity, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(candidate_id, seed_track_id, source) DO UPDATE SET
             source_payload_json = excluded.source_payload_json,
             similarity = excluded.similarity,
             seen_at = datetime('now'),
             expires_at = excluded.expires_at",
        params![
            input.candidate_id,
            input.seed_track_id,
            input.source,
            input.source_payload_json,
            input.similarity,
            input.expires_at,
        ],
    )?;
    Ok(())
}

pub fn replace_external_candidate_neighbors(
    conn: &Connection,
    model_id: i64,
    library_track_id: i64,
    rows: &[ExternalCandidateNeighborWriteRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM external_track_candidate_neighbors
         WHERE model_id = ?1 AND library_track_id = ?2",
        params![model_id, library_track_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO external_track_candidate_neighbors
             (library_track_id, candidate_id, model_id, rank, score, audio_score, metadata_score, reason_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for row in rows {
            stmt.execute(params![
                library_track_id,
                row.candidate_id,
                model_id,
                row.rank,
                row.score,
                row.audio_score,
                row.metadata_score,
                row.reason_json,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_external_candidate_neighbors(
    conn: &Connection,
    model_id: i64,
    library_track_id: i64,
    limit: i64,
    require_tidal: bool,
) -> Result<Vec<ExternalCandidateNeighborRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.tidal_id, c.title, c.artist_name, c.duration_ms,
                n.rank, n.score, n.audio_score, n.metadata_score, n.reason_json
         FROM external_track_candidate_neighbors n
         JOIN external_track_candidates c ON c.id = n.candidate_id
         WHERE n.model_id = ?1
           AND n.library_track_id = ?2
           AND (?3 = 0 OR c.tidal_id IS NOT NULL)
         ORDER BY n.rank ASC
         LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(
            params![
                model_id,
                library_track_id,
                require_tidal as i32,
                limit.max(1)
            ],
            |row| {
                Ok(ExternalCandidateNeighborRow {
                    candidate_id: row.get(0)?,
                    tidal_id: row.get(1)?,
                    title: row.get(2)?,
                    artist_name: row.get(3)?,
                    duration_ms: row.get(4)?,
                    rank: row.get(5)?,
                    score: row.get(6)?,
                    audio_score: row.get(7)?,
                    metadata_score: row.get(8)?,
                    reason_json: row.get(9)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn mark_external_candidate_resolved(
    conn: &Connection,
    tidal_id: Option<i64>,
    title: &str,
    artist_name: &str,
    resolved_track_id: i64,
) -> Result<usize> {
    let mut changed = 0usize;
    if let Some(tidal_id) = tidal_id.filter(|id| *id > 0) {
        changed += conn.execute(
            "UPDATE external_track_candidates
             SET resolved_track_id = ?2
             WHERE tidal_id = ?1",
            params![tidal_id, resolved_track_id],
        )?;
        if changed > 0 {
            return Ok(changed);
        }
    }

    changed += conn.execute(
        "UPDATE OR IGNORE external_track_candidates
         SET tidal_id = COALESCE(?1, tidal_id),
             resolved_track_id = ?4
         WHERE resolved_track_id IS NULL
           AND tidal_id IS NULL
           AND lower(trim(title)) = lower(trim(?2))
           AND lower(trim(artist_name)) = lower(trim(?3))",
        params![tidal_id, title, artist_name, resolved_track_id],
    )?;
    Ok(changed)
}

pub fn merge_external_track_candidates(
    conn: &Connection,
    winner_id: i64,
    loser_id: i64,
) -> Result<()> {
    if winner_id == loser_id {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_sightings
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_sightings WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_audio_features
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_audio_features WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_embeddings
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_embeddings WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_neighbors
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_neighbors WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "DELETE FROM external_track_candidates WHERE id = ?1",
        params![loser_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn get_model_embeddings(conn: &Connection, model_id: i64) -> Result<Vec<ModelEmbeddingRow>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, vector_blob, l2_norm
         FROM track_embeddings
         WHERE model_id = ?1",
    )?;
    stmt.query_map(params![model_id], |row| {
        Ok(ModelEmbeddingRow {
            track_id: row.get(0)?,
            vector_blob: row.get(1)?,
            l2_norm: row.get(2)?,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(Into::into)
}

pub fn get_embedding_track_rows(conn: &Connection) -> Result<Vec<EmbeddingTrackRow>> {
    // Discovery embedding training pulls in album/artist fallback genres so
    // niche tracks (no track-level enrichment yet) cluster near coherent
    // peers in the embedding instead of isolating. Cost ~2s on training start;
    // training is periodic, not per-request, so the whole-library scan is fine.
    let genre_paths_with_provenance = get_track_genre_paths_with_fallback(conn)?;
    let genre_paths: HashMap<i64, Vec<String>> = genre_paths_with_provenance
        .into_iter()
        .map(|(id, rows)| (id, ResolvedGenre::paths_only(&rows)))
        .collect();
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, t.duration_ms, t.best_quality, t.source,
                t.play_count, t.is_favorite,
                (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.track_id = t.id) AS playlist_memberships,
                d.bpm, d.energy, d.camelot_key, d.danceability, d.beat_strength, d.loudness_lufs
         FROM tracks t
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id
         WHERE t.tidal_id IS NOT NULL OR t.file_path IS NOT NULL OR t.ytmusic_id IS NOT NULL OR t.soundcloud_id IS NOT NULL",
    )?;
    let mut rows = stmt
        .query_map([], |row| {
            let track_id = row.get::<_, i64>(0)?;
            Ok(EmbeddingTrackRow {
                track_id,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                duration_ms: row.get(4)?,
                best_quality: row.get(5)?,
                source: row.get(6)?,
                play_count: row.get(7)?,
                is_favorite: row.get(8)?,
                playlist_memberships: row.get(9)?,
                genre_paths: genre_paths.get(&track_id).cloned().unwrap_or_default(),
                bpm: row.get(10)?,
                energy: row.get(11)?,
                camelot_key: row.get(12)?,
                danceability: row.get(13)?,
                beat_strength: row.get(14)?,
                loudness_lufs: row.get(15)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.sort_by_key(|row| row.track_id);
    Ok(rows)
}

pub fn get_discovery_status(conn: &Connection) -> Result<DiscoveryStatus> {
    let selected_engine = selected_discovery_engine(conn)?;
    let selected_engine_family = discovery_model_family_for_engine(&selected_engine).to_string();
    let active_model = get_selected_discovery_embedding_model(conn)?;
    let latest_run = get_latest_training_run(conn)?;
    let playable_tracks: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM tracks
         WHERE tidal_id IS NOT NULL OR file_path IS NOT NULL OR ytmusic_id IS NOT NULL OR soundcloud_id IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    let embedded_tracks: i64 = match active_model.as_ref() {
        Some(model) => conn.query_row(
            "SELECT COUNT(*) FROM track_embeddings WHERE model_id = ?1",
            params![model.id],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let neighbor_tracks: i64 = match active_model.as_ref() {
        Some(model) => conn.query_row(
            "SELECT COUNT(DISTINCT track_id) FROM track_neighbors WHERE model_id = ?1",
            params![model.id],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let clip_cache_tracks: i64 =
        conn.query_row("SELECT COUNT(*) FROM track_audio_features", [], |row| {
            row.get(0)
        })?;
    let coverage_ratio = if playable_tracks == 0 {
        0.0
    } else {
        neighbor_tracks as f64 / playable_tracks as f64
    };

    let selected_engine_trainable = selected_engine == DISCOVERY_ENGINE_V2;
    Ok(DiscoveryStatus {
        fallback_active: active_model.is_none(),
        active_model,
        selected_engine,
        selected_engine_family,
        selected_engine_trainable,
        latest_run,
        coverage_ratio,
        playable_tracks,
        embedded_tracks,
        neighbor_tracks,
        clip_cache_tracks,
    })
}

pub fn record_playback_transition(
    conn: &Connection,
    from_track_id: i64,
    to_track_id: i64,
    transition_source: &str,
    completed_prev: bool,
    gap_ms: i64,
) -> Result<()> {
    if from_track_id <= 0 || to_track_id <= 0 || from_track_id == to_track_id {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO playback_transitions
         (from_track_id, to_track_id, transition_source, completed_prev, gap_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            from_track_id,
            to_track_id,
            transition_source,
            completed_prev,
            gap_ms
        ],
    )?;
    Ok(())
}

pub fn record_discovery_feedback(
    conn: &Connection,
    seed_track_id: i64,
    candidate_track_id: i64,
    action: &str,
    surface: &str,
    context_json: Option<&str>,
    session_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO discovery_feedback
         (seed_track_id, candidate_track_id, action, surface, context_json, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            seed_track_id,
            candidate_track_id,
            action,
            surface,
            context_json,
            session_id
        ],
    )?;
    Ok(())
}

/// Most recent feedback rows for a discovery session, newest first:
/// `(candidate_track_id, action, artist_id)`. The artist id comes along via a
/// join so the rerank taste builder needs only one extra batched genre lookup.
pub fn get_discovery_feedback_for_session(
    conn: &Connection,
    session_id: &str,
    limit: i64,
) -> Result<Vec<(i64, String, Option<i64>)>> {
    let mut stmt = conn.prepare(
        "SELECT df.candidate_track_id, df.action, t.artist_id
         FROM discovery_feedback df
         LEFT JOIN tracks t ON t.id = df.candidate_track_id
         WHERE df.session_id = ?1
         ORDER BY df.created_at DESC, df.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![session_id, limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[allow(dead_code)]
pub fn get_playback_transition_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT from_track_id, to_track_id
         FROM playback_transitions
         ORDER BY created_at ASC, id ASC",
    )?;
    let pairs = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().map(|(a, b)| vec![a, b]).collect())
}

#[derive(Debug, Clone)]
pub struct WeightedTrackPairRow {
    pub event_id: String,
    pub from_track_id: i64,
    pub to_track_id: i64,
    pub weight: f64,
    pub source: Option<String>,
    pub completed_prev: Option<bool>,
}

pub fn get_playback_transition_edges(conn: &Connection) -> Result<Vec<WeightedTrackPairRow>> {
    let mut stmt = conn.prepare(
        "SELECT
            'playback_transition:' || id,
            from_track_id,
            to_track_id,
            1.0,
            transition_source,
            completed_prev
         FROM playback_transitions
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WeightedTrackPairRow {
                event_id: row.get(0)?,
                from_track_id: row.get(1)?,
                to_track_id: row.get(2)?,
                weight: row.get(3)?,
                source: row.get(4)?,
                completed_prev: Some(row.get::<_, bool>(5)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_completion_weighted_listen_edges(
    conn: &Connection,
    session_window_minutes: i64,
) -> Result<Vec<WeightedTrackPairRow>> {
    let mut stmt = conn.prepare(
        "WITH weighted AS (
            SELECT
                lh.id,
                lh.track_id,
                lh.started_at,
                lh.session_id,
                lh.source,
                CASE
                    WHEN t.duration_ms IS NOT NULL AND t.duration_ms > 0 THEN
                        MIN(1.0, CAST(COALESCE(lh.duration_listened_ms, 0) AS REAL) / CAST(t.duration_ms AS REAL))
                    WHEN COALESCE(lh.completed, 0) = 1 THEN 1.0
                    ELSE 0.25
                END AS completion_weight
            FROM listen_history lh
            JOIN tracks t ON t.id = lh.track_id
        )
        SELECT
            'listen_history_pair:' || a.id || ':' || b.id,
            a.track_id,
            b.track_id,
            MIN(a.completion_weight, b.completion_weight),
            COALESCE(b.source, a.source)
        FROM weighted a
        JOIN weighted b
            ON b.id > a.id
           AND b.track_id != a.track_id
           AND (
                (a.session_id IS NOT NULL AND a.session_id = b.session_id)
                OR (
                    (a.session_id IS NULL OR b.session_id IS NULL)
                    AND b.started_at BETWEEN a.started_at AND datetime(a.started_at, printf('+%d minutes', ?1))
                )
           )
        ORDER BY a.started_at ASC, a.id ASC, b.started_at ASC, b.id ASC",
    )?;
    let rows = stmt
        .query_map(params![session_window_minutes.max(1)], |row| {
            Ok(WeightedTrackPairRow {
                event_id: row.get(0)?,
                from_track_id: row.get(1)?,
                to_track_id: row.get(2)?,
                weight: row.get(3)?,
                source: row.get(4)?,
                completed_prev: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_listen_history_transition_edges(conn: &Connection) -> Result<Vec<WeightedTrackPairRow>> {
    let mut stmt = conn.prepare(
        "SELECT
            'listen_history:' || lh.id,
            lh.transition_from_track_id,
            lh.track_id,
            CASE
                WHEN t.duration_ms IS NOT NULL AND t.duration_ms > 0 THEN
                    MIN(1.0, CAST(COALESCE(lh.duration_listened_ms, 0) AS REAL) / CAST(t.duration_ms AS REAL))
                WHEN COALESCE(lh.completed, 0) = 1 THEN 1.0
                ELSE 0.25
            END AS completion_weight,
            lh.source
         FROM listen_history lh
         JOIN tracks t ON t.id = lh.track_id
         WHERE lh.transition_from_track_id IS NOT NULL
           AND lh.transition_from_track_id != lh.track_id
         ORDER BY lh.started_at ASC, lh.id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WeightedTrackPairRow {
                event_id: row.get(0)?,
                from_track_id: row.get(1)?,
                to_track_id: row.get(2)?,
                weight: row.get(3)?,
                source: row.get(4)?,
                completed_prev: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[allow(dead_code)]
pub fn get_listen_history_sequences(
    conn: &Connection,
    session_window_minutes: i64,
) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, started_at
         FROM listen_history
         ORDER BY started_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut sequences = Vec::new();
    let mut current = Vec::new();
    let mut previous_at: Option<chrono::DateTime<chrono::Utc>> = None;
    for (track_id, started_at) in rows {
        let parsed = chrono::DateTime::parse_from_rfc3339(&format!(
            "{}{}",
            started_at,
            if started_at.ends_with('Z') { "" } else { "Z" }
        ))
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&started_at, "%Y-%m-%d %H:%M:%S")
                .map(|dt| dt.and_utc())
        })
        .ok();
        if let Some(prev) = previous_at {
            if let Some(next) = parsed {
                if (next - prev).num_minutes() > session_window_minutes {
                    if current.len() > 1 {
                        sequences.push(current.clone());
                    }
                    current.clear();
                }
                previous_at = Some(next);
            }
        } else if let Some(next) = parsed {
            previous_at = Some(next);
        }
        current.push(track_id);
    }
    if current.len() > 1 {
        sequences.push(current);
    }
    Ok(sequences)
}

pub fn get_playlist_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT playlist_id, track_id
         FROM playlist_tracks
         ORDER BY playlist_id ASC, position ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (playlist_id, track_id) in rows {
        grouped.entry(playlist_id).or_default().push(track_id);
    }
    Ok(grouped.into_values().filter(|seq| seq.len() > 1).collect())
}

pub fn get_album_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT album_id, id
         FROM tracks
         WHERE album_id IS NOT NULL
         ORDER BY album_id ASC, disc_number ASC, track_number ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (album_id, track_id) in rows {
        grouped.entry(album_id).or_default().push(track_id);
    }
    Ok(grouped.into_values().filter(|seq| seq.len() > 1).collect())
}

pub fn get_artist_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT artist_id, id
         FROM tracks
         WHERE artist_id IS NOT NULL
         ORDER BY artist_id ASC, play_count DESC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (artist_id, track_id) in rows {
        grouped.entry(artist_id).or_default().push(track_id);
    }
    // Truncate large artists rather than dropping them — an artist with 200 tracks
    // should still contribute co-occurrence signal for the tracks it includes.
    let mut seqs: Vec<Vec<i64>> = grouped.into_values().filter(|seq| seq.len() > 1).collect();
    for seq in &mut seqs {
        if seq.len() > 80 {
            seq.truncate(80); // keep top-80 by play_count (already sorted DESC)
        }
    }
    Ok(seqs)
}

pub fn get_genre_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT tg.genre_id, tg.track_id
         FROM track_genres tg
         JOIN tracks t ON t.id = tg.track_id
         ORDER BY tg.genre_id ASC, t.play_count DESC, t.id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (genre_id, track_id) in rows {
        grouped.entry(genre_id).or_default().push(track_id);
    }
    let mut seqs: Vec<Vec<i64>> = grouped.into_values().filter(|seq| seq.len() > 1).collect();
    for seq in &mut seqs {
        if seq.len() > 80 {
            seq.truncate(80);
        }
    }
    Ok(seqs)
}

pub fn get_favorite_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn
        .prepare("SELECT id FROM tracks WHERE is_favorite = 1 ORDER BY play_count DESC, id ASC")?;
    stmt.query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

// ─── Audio DSP Features ─────────────────────────────────────────────────────

pub fn upsert_audio_dsp_features(conn: &Connection, f: &AudioDspFeatures) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dsp_features
         (track_id, bpm, key_signature, camelot_key, loudness_lufs, energy, danceability,
          beat_strength, spectral_centroid, stereo_width, is_instrumental,
          analysis_source, analysis_offset_ms, samples_analyzed, analyzed_at, analysis_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(track_id) DO UPDATE SET
             bpm = excluded.bpm,
             key_signature = excluded.key_signature,
             camelot_key = excluded.camelot_key,
             loudness_lufs = excluded.loudness_lufs,
             energy = excluded.energy,
             danceability = excluded.danceability,
             beat_strength = excluded.beat_strength,
             spectral_centroid = excluded.spectral_centroid,
             stereo_width = excluded.stereo_width,
             is_instrumental = excluded.is_instrumental,
             analysis_source = excluded.analysis_source,
             analysis_offset_ms = excluded.analysis_offset_ms,
             samples_analyzed = excluded.samples_analyzed,
             analyzed_at = excluded.analyzed_at,
             analysis_version = excluded.analysis_version",
        params![
            f.track_id,
            f.bpm,
            f.key_signature,
            f.camelot_key,
            f.loudness_lufs,
            f.energy,
            f.danceability,
            f.beat_strength,
            f.spectral_centroid,
            f.stereo_width,
            f.is_instrumental as i32,
            f.analysis_source,
            f.analysis_offset_ms,
            f.samples_analyzed,
            f.analyzed_at,
            f.analysis_version,
        ],
    )?;
    Ok(())
}

pub fn get_audio_dsp_features(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<AudioDspFeatures>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, bpm, key_signature, camelot_key, loudness_lufs,
                energy, danceability, beat_strength, spectral_centroid, stereo_width,
                is_instrumental, analysis_source, analysis_offset_ms, samples_analyzed,
                analyzed_at, analysis_version
         FROM audio_dsp_features
         WHERE track_id = ?1",
    )?;
    let result = stmt
        .query_row(params![track_id], |row| {
            Ok(AudioDspFeatures {
                track_id: row.get(0)?,
                bpm: row.get(1)?,
                key_signature: row.get(2)?,
                camelot_key: row.get(3)?,
                loudness_lufs: row.get(4)?,
                energy: row.get(5)?,
                danceability: row.get(6)?,
                beat_strength: row.get(7)?,
                spectral_centroid: row.get(8)?,
                stereo_width: row.get(9)?,
                is_instrumental: row.get::<_, i32>(10)? != 0,
                analysis_source: row.get(11)?,
                analysis_offset_ms: row.get(12)?,
                samples_analyzed: row.get(13)?,
                analyzed_at: row.get(14)?,
                analysis_version: row.get(15)?,
            })
        })
        .optional()?;
    Ok(result)
}

/// Batch-fetch just the harmonic inputs (camelot key + bpm) for many tracks in a
/// single query. The radio/automix re-ranker only needs these two fields; fetching
/// the full `AudioDspFeatures` row per candidate was N serialized single-row
/// queries under the DB mutex. Returns a map keyed by track_id; tracks with no
/// `audio_dsp_features` row are simply absent (callers treat that as unanalyzed).
pub fn get_dsp_harmonic_keys_batch(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<std::collections::HashMap<i64, (Option<String>, Option<f64>)>> {
    let mut out: std::collections::HashMap<i64, (Option<String>, Option<f64>)> =
        std::collections::HashMap::new();
    if track_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = std::iter::repeat("?")
        .take(track_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT track_id, camelot_key, bpm FROM audio_dsp_features WHERE track_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(track_ids.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<f64>>(2)?,
        ))
    })?;
    for row in rows {
        let (id, camelot, bpm) = row?;
        out.insert(id, (camelot, bpm));
    }
    Ok(out)
}

pub fn get_tracks_missing_dsp_features(conn: &Connection, limit: i64) -> Result<Vec<Track>> {
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let projection = track_projection("a");
    let sql = format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN audio_dsp_features dsp ON t.id = dsp.track_id
         WHERE (dsp.track_id IS NULL OR dsp.analysis_version != '{}')
           AND COALESCE(dsp.manual_override, 0) = 0
         LIMIT ?1",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn get_audio_features_stats(conn: &Connection) -> Result<AudioFeaturesStats> {
    let (total_analyzed, avg_bpm, avg_energy): (i64, Option<f64>, Option<f64>) = conn.query_row(
        "SELECT COUNT(*), AVG(bpm), AVG(energy) FROM audio_dsp_features",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;

    // Top key (most common)
    let top_key: Option<String> = conn
        .query_row(
            "SELECT key_signature
         FROM audio_dsp_features
         WHERE key_signature IS NOT NULL
         GROUP BY key_signature
         ORDER BY COUNT(*) DESC
         LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;

    // Key distribution
    let mut stmt = conn.prepare(
        "SELECT key_signature, COUNT(*)
         FROM audio_dsp_features
         WHERE key_signature IS NOT NULL
         GROUP BY key_signature
         ORDER BY COUNT(*) DESC",
    )?;
    let key_rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let key_distribution: HashMap<String, i64> = key_rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .collect();

    Ok(AudioFeaturesStats {
        total_analyzed,
        avg_bpm,
        top_key,
        avg_energy,
        key_distribution,
    })
}

/// Bulk-load DSP features for every analyzed track. Used by smart playlist evaluation
/// so a single scan populates the evaluation context for all rules at once.
pub fn get_all_audio_dsp_features(
    conn: &Connection,
) -> Result<
    Vec<(
        i64,
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
        bool,
    )>,
> {
    let mut stmt = conn.prepare(
        "SELECT track_id, bpm, key_signature, camelot_key, energy, danceability, is_instrumental
         FROM audio_dsp_features",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<i32>>(6)?.unwrap_or(0) != 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Track IDs that have a stored audio fingerprint.
pub fn get_track_ids_with_fingerprint(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT track_id FROM audio_fingerprints")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn count_audio_dsp_features(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM audio_dsp_features", [], |row| {
        row.get(0)
    })
    .map_err(Into::into)
}

pub fn delete_all_audio_dsp_features(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM audio_dsp_features", [])?;
    Ok(())
}

pub fn get_genre_audio_metrics(conn: &Connection) -> Result<Vec<GenreAudioMetrics>> {
    let mut stmt = conn.prepare(
        "SELECT g.id, g.name,
                AVG(a.bpm) AS avg_bpm,
                AVG(a.energy) AS avg_energy,
                AVG(a.danceability) AS avg_danceability,
                COUNT(DISTINCT a.track_id) AS analyzed_count
         FROM genres g
         JOIN track_genres tg ON tg.genre_id = g.id
         JOIN audio_dsp_features a ON a.track_id = tg.track_id
         GROUP BY g.id, g.name
         ORDER BY analyzed_count DESC, g.name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(GenreAudioMetrics {
            genre_id: row.get(0)?,
            genre_name: row.get(1)?,
            avg_bpm: row.get(2)?,
            avg_energy: row.get(3)?,
            avg_danceability: row.get(4)?,
            analyzed_count: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[allow(dead_code)]
pub fn upsert_fingerprint(conn: &Connection, track_id: i64, fp: &AudioFingerprint) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_fingerprints (track_id, hashes_blob, peak_count)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(track_id) DO UPDATE SET
             hashes_blob = excluded.hashes_blob,
             peak_count = excluded.peak_count",
        params![track_id, fp.hashes_blob, fp.peak_count],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn insert_fingerprint_hashes(
    conn: &Connection,
    track_id: i64,
    hashes: &[(u32, u32)],
) -> Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }

    // Wrap the whole payload in an explicit transaction; chunk inserts so a
    // very large hash list doesn't hold a single statement open for too long.
    const CHUNK: usize = 1000;
    conn.execute_batch("BEGIN;")?;
    let insert_result: Result<()> = (|| {
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO fingerprint_hashes (hash, track_id, time_offset)
             VALUES (?1, ?2, ?3)",
        )?;
        for chunk in hashes.chunks(CHUNK) {
            for (hash, time_offset) in chunk {
                stmt.execute(params![*hash as i64, track_id, *time_offset])?;
            }
        }
        Ok(())
    })();

    match insert_result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

/// Run `PRAGMA optimize; ANALYZE fingerprint_hashes;` after a bulk fingerprint scan.
/// Failures are logged but not fatal.
pub fn optimize_fingerprint_hashes(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA optimize; ANALYZE fingerprint_hashes;")?;
    Ok(())
}

// ── Duplicate group helpers (fingerprint-driven dedup) ───────────────────────

/// Find an existing duplicate_group that already contains BOTH `a` and `b` as members.
pub fn find_duplicate_group_for_tracks(conn: &Connection, a: i64, b: i64) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT ma.group_id
         FROM duplicate_members ma
         JOIN duplicate_members mb ON mb.group_id = ma.group_id
         WHERE ma.track_id = ?1 AND mb.track_id = ?2
         LIMIT 1",
        params![a, b],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Create a new empty duplicate_group and return its id.
pub fn create_duplicate_group(conn: &Connection) -> Result<i64> {
    conn.execute(
        "INSERT INTO duplicate_groups (status) VALUES ('pending')",
        [],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a member into a duplicate_group. Idempotent (ON CONFLICT IGNORE).
pub fn add_duplicate_member(conn: &Connection, gid: i64, tid: i64, preferred: bool) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO duplicate_members (group_id, track_id, is_preferred)
         VALUES (?1, ?2, ?3)",
        params![gid, tid, if preferred { 1 } else { 0 }],
    )?;
    Ok(())
}

/// Tag a duplicate_group with its source (e.g. 'fingerprint') and a confidence value.
pub fn set_duplicate_group_source(
    conn: &Connection,
    gid: i64,
    source: &str,
    confidence: f64,
) -> Result<()> {
    conn.execute(
        "UPDATE duplicate_groups SET source = ?2, confidence = ?3 WHERE id = ?1",
        params![gid, source, confidence],
    )?;
    Ok(())
}

// ── Analysis quality & stale detection ───────────────────────────────────────

/// Snapshot of DSP-analysis coverage across the library.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioFeaturesQuality {
    pub total_tracks: i64,
    pub analyzed: i64,
    pub analysis_current: i64,
    pub analysis_stale: i64,
    pub low_confidence_bpm: i64,
    pub low_confidence_key: i64,
    pub no_preview_url: i64,
    pub fingerprinted: i64,
}

pub fn get_audio_features_quality(conn: &Connection) -> Result<AudioFeaturesQuality> {
    let total_tracks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap_or(0);
    let analyzed: i64 = conn
        .query_row("SELECT COUNT(*) FROM audio_dsp_features", [], |r| r.get(0))
        .unwrap_or(0);
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let analyzed_current_sql = format!(
        "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version = '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let analysis_current: i64 = conn
        .query_row(&analyzed_current_sql, [], |r| r.get(0))
        .unwrap_or(0);
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let analysis_stale_sql = format!(
        "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version != '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let analysis_stale: i64 = conn
        .query_row(&analysis_stale_sql, [], |r| r.get(0))
        .unwrap_or(0);
    let low_confidence_bpm: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE bpm IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let low_confidence_key: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE key_signature IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    // "No preview URL" = tracks we can't currently pull preview audio for.
    // We treat tracks lacking a tidal_id AND file_path as having no preview source.
    let no_preview_url: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tracks
             WHERE tidal_id IS NULL
               AND (file_path IS NULL OR file_path = '')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let fingerprinted: i64 = conn
        .query_row("SELECT COUNT(*) FROM audio_fingerprints", [], |r| r.get(0))
        .unwrap_or(0);

    Ok(AudioFeaturesQuality {
        total_tracks,
        analyzed,
        analysis_current,
        analysis_stale,
        low_confidence_bpm,
        low_confidence_key,
        no_preview_url,
        fingerprinted,
    })
}

/// Return the ids of all tracks whose stored analysis_version is not the current
/// `CURRENT_ANALYSIS_VERSION`. Used by the re-analyze admin endpoint.
pub fn get_stale_analysis_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let sql = format!(
        "SELECT track_id FROM audio_dsp_features WHERE analysis_version != '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn find_tracks_by_hash(conn: &Connection, hashes: &[u32]) -> Result<Vec<(i64, u32, u32)>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = hashes.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT track_id, hash, time_offset
         FROM fingerprint_hashes
         WHERE hash IN ({})
         ORDER BY hash",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let hash_params: Vec<i64> = hashes.iter().map(|h| *h as i64).collect();
    let rows = stmt.query_map(params_from_iter(hash_params.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)? as u32,
            row.get::<_, i64>(2)? as u32,
        ))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn upsert_audio_dj_profile(conn: &Connection, row: &AudioDjProfileRow) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dj_profiles (
            media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id, profile_version,
            beat_grid_blob, downbeats_blob, phrase_boundaries_blob, mix_in_blob, mix_out_blob,
            intro_end_seconds, outro_start_seconds, breakdown_blob, drop_blob,
            safe_transition_windows_blob, energy_contour_blob, vocal_presence_blob,
            vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp, beat_confidence,
            profile_confidence, analysis_scope_ms, is_temporary, source, computed_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28
        )
        ON CONFLICT(media_ref_kind, media_ref_id) DO UPDATE SET
            track_id = excluded.track_id,
            queue_item_id = excluded.queue_item_id,
            tidal_id = excluded.tidal_id,
            profile_version = excluded.profile_version,
            beat_grid_blob = excluded.beat_grid_blob,
            downbeats_blob = excluded.downbeats_blob,
            phrase_boundaries_blob = excluded.phrase_boundaries_blob,
            mix_in_blob = excluded.mix_in_blob,
            mix_out_blob = excluded.mix_out_blob,
            intro_end_seconds = excluded.intro_end_seconds,
            outro_start_seconds = excluded.outro_start_seconds,
            breakdown_blob = excluded.breakdown_blob,
            drop_blob = excluded.drop_blob,
            safe_transition_windows_blob = excluded.safe_transition_windows_blob,
            energy_contour_blob = excluded.energy_contour_blob,
            vocal_presence_blob = excluded.vocal_presence_blob,
            vocal_density_blob = excluded.vocal_density_blob,
            waveform_peaks_blob = excluded.waveform_peaks_blob,
            lufs_loud_body = excluded.lufs_loud_body,
            true_peak_dbtp = excluded.true_peak_dbtp,
            beat_confidence = excluded.beat_confidence,
            profile_confidence = excluded.profile_confidence,
            analysis_scope_ms = excluded.analysis_scope_ms,
            is_temporary = excluded.is_temporary,
            source = excluded.source,
            computed_at = excluded.computed_at",
        params![
            row.media_ref_kind,
            row.media_ref_id,
            row.track_id,
            row.queue_item_id,
            row.tidal_id,
            row.profile_version,
            row.beat_grid_blob,
            row.downbeats_blob,
            row.phrase_boundaries_blob,
            row.mix_in_blob,
            row.mix_out_blob,
            row.intro_end_seconds,
            row.outro_start_seconds,
            row.breakdown_blob,
            row.drop_blob,
            row.safe_transition_windows_blob,
            row.energy_contour_blob,
            row.vocal_presence_blob,
            row.vocal_density_blob,
            row.waveform_peaks_blob,
            row.lufs_loud_body,
            row.true_peak_dbtp,
            row.beat_confidence,
            row.profile_confidence,
            row.analysis_scope_ms,
            if row.is_temporary { 1 } else { 0 },
            row.source,
            row.computed_at,
        ],
    )?;
    Ok(())
}

pub fn get_audio_dj_profile(
    conn: &Connection,
    key: &AudioDjProfileKey,
) -> Result<Option<AudioDjProfileRow>> {
    conn.query_row(
        "SELECT media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id,
            profile_version, beat_grid_blob, downbeats_blob, phrase_boundaries_blob,
            mix_in_blob, mix_out_blob, intro_end_seconds, outro_start_seconds,
            breakdown_blob, drop_blob, safe_transition_windows_blob, energy_contour_blob,
            vocal_presence_blob, vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp,
            beat_confidence, profile_confidence, analysis_scope_ms, is_temporary,
            source, computed_at
         FROM audio_dj_profiles
         WHERE media_ref_kind = ?1 AND media_ref_id = ?2",
        params![key.media_ref_kind, key.media_ref_id],
        audio_dj_profile_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_audio_dj_profile_for_track(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<AudioDjProfileRow>> {
    conn.query_row(
        "SELECT media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id,
            profile_version, beat_grid_blob, downbeats_blob, phrase_boundaries_blob,
            mix_in_blob, mix_out_blob, intro_end_seconds, outro_start_seconds,
            breakdown_blob, drop_blob, safe_transition_windows_blob, energy_contour_blob,
            vocal_presence_blob, vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp,
            beat_confidence, profile_confidence, analysis_scope_ms, is_temporary,
            source, computed_at
         FROM audio_dj_profiles
         WHERE track_id = ?1
         ORDER BY computed_at DESC
         LIMIT 1",
        params![track_id],
        audio_dj_profile_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn promote_temporary_audio_dj_profile(
    conn: &Connection,
    temporary_key: &AudioDjProfileKey,
    stable_key: &AudioDjProfileKey,
    tidal_id: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dj_profiles (
            media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id, profile_version,
            beat_grid_blob, downbeats_blob, phrase_boundaries_blob, mix_in_blob, mix_out_blob,
            intro_end_seconds, outro_start_seconds, breakdown_blob, drop_blob,
            safe_transition_windows_blob, energy_contour_blob, vocal_presence_blob,
            vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp, beat_confidence,
            profile_confidence, analysis_scope_ms, is_temporary, source, computed_at
        )
        SELECT ?3, ?4, track_id, queue_item_id, COALESCE(?5, tidal_id), profile_version,
            beat_grid_blob, downbeats_blob, phrase_boundaries_blob, mix_in_blob, mix_out_blob,
            intro_end_seconds, outro_start_seconds, breakdown_blob, drop_blob,
            safe_transition_windows_blob, energy_contour_blob, vocal_presence_blob,
            vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp, beat_confidence,
            profile_confidence, analysis_scope_ms, 0, source, datetime('now')
        FROM audio_dj_profiles
        WHERE media_ref_kind = ?1 AND media_ref_id = ?2
        ON CONFLICT(media_ref_kind, media_ref_id) DO UPDATE SET
            track_id = excluded.track_id,
            queue_item_id = excluded.queue_item_id,
            tidal_id = excluded.tidal_id,
            profile_version = excluded.profile_version,
            beat_grid_blob = excluded.beat_grid_blob,
            downbeats_blob = excluded.downbeats_blob,
            phrase_boundaries_blob = excluded.phrase_boundaries_blob,
            mix_in_blob = excluded.mix_in_blob,
            mix_out_blob = excluded.mix_out_blob,
            intro_end_seconds = excluded.intro_end_seconds,
            outro_start_seconds = excluded.outro_start_seconds,
            breakdown_blob = excluded.breakdown_blob,
            drop_blob = excluded.drop_blob,
            safe_transition_windows_blob = excluded.safe_transition_windows_blob,
            energy_contour_blob = excluded.energy_contour_blob,
            vocal_presence_blob = excluded.vocal_presence_blob,
            vocal_density_blob = excluded.vocal_density_blob,
            waveform_peaks_blob = excluded.waveform_peaks_blob,
            lufs_loud_body = excluded.lufs_loud_body,
            true_peak_dbtp = excluded.true_peak_dbtp,
            beat_confidence = excluded.beat_confidence,
            profile_confidence = excluded.profile_confidence,
            analysis_scope_ms = excluded.analysis_scope_ms,
            is_temporary = excluded.is_temporary,
            source = excluded.source,
            computed_at = excluded.computed_at",
        params![
            temporary_key.media_ref_kind,
            temporary_key.media_ref_id,
            stable_key.media_ref_kind,
            stable_key.media_ref_id,
            tidal_id,
        ],
    )?;
    Ok(())
}

pub fn upsert_audio_dj_profile_correction(
    conn: &Connection,
    row: &AudioDjProfileCorrectionRow,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dj_profile_corrections (
            media_ref_kind, media_ref_id, bpm_multiplier, downbeat_offset_beats,
            phrase_offset_bars, safe_crossfade_only, transition_speed_bias, notes,
            manual_drop_blob, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(media_ref_kind, media_ref_id) DO UPDATE SET
            bpm_multiplier = excluded.bpm_multiplier,
            downbeat_offset_beats = excluded.downbeat_offset_beats,
            phrase_offset_bars = excluded.phrase_offset_bars,
            safe_crossfade_only = excluded.safe_crossfade_only,
            transition_speed_bias = excluded.transition_speed_bias,
            notes = excluded.notes,
            manual_drop_blob = excluded.manual_drop_blob,
            updated_at = excluded.updated_at",
        params![
            row.media_ref_kind,
            row.media_ref_id,
            row.bpm_multiplier,
            row.downbeat_offset_beats,
            row.phrase_offset_bars,
            if row.safe_crossfade_only { 1 } else { 0 },
            row.transition_speed_bias,
            row.notes,
            row.manual_drop_blob,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

pub fn get_audio_dj_profile_correction(
    conn: &Connection,
    key: &AudioDjProfileKey,
) -> Result<Option<AudioDjProfileCorrectionRow>> {
    conn.query_row(
        "SELECT media_ref_kind, media_ref_id, bpm_multiplier, downbeat_offset_beats,
            phrase_offset_bars, safe_crossfade_only, transition_speed_bias, notes,
            manual_drop_blob, created_at, updated_at
         FROM audio_dj_profile_corrections
         WHERE media_ref_kind = ?1 AND media_ref_id = ?2",
        params![key.media_ref_kind, key.media_ref_id],
        |row| {
            Ok(AudioDjProfileCorrectionRow {
                media_ref_kind: row.get(0)?,
                media_ref_id: row.get(1)?,
                bpm_multiplier: row.get(2)?,
                downbeat_offset_beats: row.get(3)?,
                phrase_offset_bars: row.get(4)?,
                safe_crossfade_only: row.get::<_, i64>(5)? != 0,
                transition_speed_bias: row.get(6)?,
                notes: row.get(7)?,
                manual_drop_blob: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn is_dj_engine_enabled(conn: &Connection) -> Result<bool> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'dj_engine_enabled'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(matches!(value.as_deref(), Some("1") | Some("true")))
}

pub fn set_dj_engine_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value)
         VALUES ('dj_engine_enabled', ?1)",
        params![if enabled { "1" } else { "0" }],
    )?;
    Ok(())
}

pub fn get_dj_global_policy(conn: &Connection) -> Result<(String, String)> {
    let mix_intent = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'dj_mix_intent'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "balanced".to_string());
    let transition_speed_bias = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'dj_transition_speed_bias'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "neutral".to_string());
    Ok((mix_intent, transition_speed_bias))
}

pub fn set_dj_global_policy(
    conn: &Connection,
    mix_intent: &str,
    transition_speed_bias: &str,
) -> Result<()> {
    if !matches!(mix_intent, "safe" | "balanced" | "bold") {
        bail!("unknown DJ mix intent: {mix_intent}");
    }
    if !matches!(transition_speed_bias, "slower" | "neutral" | "faster") {
        bail!("unknown DJ transition speed bias: {transition_speed_bias}");
    }
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value)
         VALUES ('dj_mix_intent', ?1)",
        params![mix_intent],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value)
         VALUES ('dj_transition_speed_bias', ?1)",
        params![transition_speed_bias],
    )?;
    Ok(())
}

fn validate_dj_fallback_reason(reason: Option<&str>) -> Result<()> {
    if let Some(reason) = reason
        && !matches!(
            reason,
            "disabled"
                | "current_profile_missing"
                | "next_profile_missing"
                | "profile_low_confidence"
                | "next_not_resolved"
                | "fetch_failed"
                | "decode_late"
                | "analysis_late"
                | "program_invalid"
                | "queue_changed"
                | "safety_override_safe"
                | "template_not_renderable"
                | "timing_unstable"
        )
    {
        bail!("unknown DJ fallback reason: {reason}");
    }
    Ok(())
}

fn validate_dj_timing_source(source: Option<&str>) -> Result<()> {
    if let Some(source) = source
        && !matches!(source, "downbeat_sync" | "beat_sync" | "fallback_overlap")
    {
        bail!("unknown DJ timing source: {source}");
    }
    Ok(())
}

fn validate_dj_timing_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status
        && !matches!(status, "armed" | "fired" | "late" | "missed")
    {
        bail!("unknown DJ timing status: {status}");
    }
    Ok(())
}

fn validate_dj_runtime_renderer_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status
        && !matches!(
            status,
            "rendered_handoff" | "rendered_overlay" | "legacy_overlap" | "boundary_fallback"
        )
    {
        bail!("unknown DJ runtime renderer status: {status}");
    }
    Ok(())
}

fn validate_dj_runtime_renderer_reason(reason: Option<&str>) -> Result<()> {
    if let Some(reason) = reason
        && !matches!(
            reason,
            "none"
                | "prepared_mixer_missing"
                | "lookahead_pair_mismatch"
                | "program_not_mixer_renderable"
                | "active_deck_not_decoded"
                | "next_deck_not_decoded"
                | "mixer_rejected"
                | "active_track_changed"
                | "next_track_changed"
                | "render_buffer_failed"
                | "buffer_lock_failed"
                | "dj_disabled"
                | "next_decode_late_at_fire"
                | "next_deck_missing_at_fire"
                | "transition_plan_missing_at_fire"
                | "sync_window_not_signaled"
                | "manual_seek_suppressed"
        )
    {
        bail!("unknown DJ runtime renderer reason: {reason}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_dj_transition_event(
    conn: &Connection,
    from_track_id: Option<i64>,
    to_track_id: Option<i64>,
    from_media_ref_kind: Option<&str>,
    from_media_ref_id: Option<&str>,
    to_media_ref_kind: Option<&str>,
    to_media_ref_id: Option<&str>,
    template: &str,
    program_json: &str,
    rejected_alternatives_json: Option<&str>,
    planner_version: &str,
    fallback_reason: Option<&str>,
    planned_start_ms: Option<i64>,
    timing_source: Option<&str>,
    timing_status: Option<&str>,
) -> Result<i64> {
    validate_dj_fallback_reason(fallback_reason)?;
    validate_dj_timing_source(timing_source)?;
    validate_dj_timing_status(timing_status)?;
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_track_id, to_track_id, from_media_ref_kind, from_media_ref_id,
            to_media_ref_kind, to_media_ref_id, template, program_json,
            rejected_alternatives_json, planner_version, fallback_reason,
            planned_start_ms, timing_source, timing_status
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            from_track_id,
            to_track_id,
            from_media_ref_kind,
            from_media_ref_id,
            to_media_ref_kind,
            to_media_ref_id,
            template,
            program_json,
            rejected_alternatives_json,
            planner_version,
            fallback_reason,
            planned_start_ms,
            timing_source,
            timing_status,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_dj_transition_outcome(
    conn: &Connection,
    id: i64,
    outcome: &str,
    skip_within_30s: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE dj_transition_events
         SET outcome = ?2,
             outcome_at = datetime('now'),
             skip_within_30s = ?3
         WHERE id = ?1",
        params![id, outcome, if skip_within_30s { 1 } else { 0 }],
    )?;
    Ok(())
}

pub fn replace_armed_dj_transition_event(
    conn: &Connection,
    id: i64,
    template: &str,
    program_json: &str,
    rejected_alternatives_json: Option<&str>,
    planner_version: &str,
    fallback_reason: Option<&str>,
    planned_start_ms: Option<i64>,
    timing_source: Option<&str>,
) -> Result<()> {
    validate_dj_fallback_reason(fallback_reason)?;
    validate_dj_timing_source(timing_source)?;
    conn.execute(
        "UPDATE dj_transition_events
         SET template = ?2,
             program_json = ?3,
             rejected_alternatives_json = ?4,
             planner_version = ?5,
             fallback_reason = ?6,
             planned_start_ms = ?7,
             actual_start_ms = NULL,
             timing_delta_ms = NULL,
             timing_source = ?8,
             runtime_rendered_dj_mixer = NULL,
             runtime_renderer_status = NULL,
             runtime_renderer_reason = NULL
         WHERE id = ?1
           AND timing_status = 'armed'
           AND actual_start_ms IS NULL
           AND outcome IS NULL",
        params![
            id,
            template,
            program_json,
            rejected_alternatives_json,
            planner_version,
            fallback_reason,
            planned_start_ms,
            timing_source,
        ],
    )?;
    Ok(())
}

pub fn update_dj_transition_fire_timing(
    conn: &Connection,
    id: i64,
    actual_start_ms: i64,
    timing_status: &str,
    runtime_rendered_dj_mixer: bool,
    runtime_renderer_status: &str,
    runtime_renderer_reason: &str,
) -> Result<()> {
    validate_dj_timing_status(Some(timing_status))?;
    validate_dj_runtime_renderer_status(Some(runtime_renderer_status))?;
    validate_dj_runtime_renderer_reason(Some(runtime_renderer_reason))?;
    conn.execute(
        "UPDATE dj_transition_events
         SET actual_start_ms = CASE
                 WHEN ?3 = 'missed' THEN NULL
                 ELSE ?2
             END,
             timing_delta_ms = CASE
                 WHEN ?3 = 'missed' THEN NULL
                 WHEN planned_start_ms IS NULL THEN NULL
                 ELSE ?2 - planned_start_ms
             END,
             timing_status = ?3,
             runtime_rendered_dj_mixer = ?4,
             runtime_renderer_status = ?5,
             runtime_renderer_reason = ?6
         WHERE id = ?1",
        params![
            id,
            actual_start_ms,
            timing_status,
            if runtime_rendered_dj_mixer { 1 } else { 0 },
            runtime_renderer_status,
            runtime_renderer_reason,
        ],
    )?;
    conn.execute(
        "UPDATE dj_transition_events
         SET timing_status = 'missed'
         WHERE id <> ?1
           AND timing_status = 'armed'
           AND EXISTS (
               SELECT 1
               FROM dj_transition_events fired
               WHERE fired.id = ?1
                 AND fired.from_media_ref_kind IS dj_transition_events.from_media_ref_kind
                 AND fired.from_media_ref_id IS dj_transition_events.from_media_ref_id
                 AND fired.to_media_ref_kind IS dj_transition_events.to_media_ref_kind
                 AND fired.to_media_ref_id IS dj_transition_events.to_media_ref_id
           )",
        params![id],
    )?;
    Ok(())
}

pub fn mark_dj_transition_timing_status_for_pair(
    conn: &Connection,
    from_media_ref_kind: &str,
    from_media_ref_id: &str,
    to_media_ref_kind: &str,
    to_media_ref_id: &str,
    timing_status: &str,
) -> Result<usize> {
    validate_dj_timing_status(Some(timing_status))?;
    conn.execute(
        "UPDATE dj_transition_events
         SET timing_status = ?5
         WHERE id = (
             SELECT id
             FROM dj_transition_events
             WHERE from_media_ref_kind = ?1
               AND from_media_ref_id = ?2
               AND to_media_ref_kind = ?3
               AND to_media_ref_id = ?4
               AND outcome IS NULL
               AND timing_status = 'armed'
               AND NOT EXISTS (
                   SELECT 1
                   FROM dj_transition_events fired
                   WHERE fired.from_media_ref_kind IS dj_transition_events.from_media_ref_kind
                      AND fired.from_media_ref_id IS dj_transition_events.from_media_ref_id
                      AND fired.to_media_ref_kind IS dj_transition_events.to_media_ref_kind
                      AND fired.to_media_ref_id IS dj_transition_events.to_media_ref_id
                      AND fired.id > dj_transition_events.id
                      AND fired.timing_status = 'fired'
                )
              ORDER BY started_at DESC, id DESC
              LIMIT 1
         )",
        params![
            from_media_ref_kind,
            from_media_ref_id,
            to_media_ref_kind,
            to_media_ref_id,
            timing_status,
        ],
    )
    .map_err(Into::into)
}

pub fn mark_dj_transition_manual_seek_suppressed_for_pair(
    conn: &Connection,
    from_media_ref_kind: &str,
    from_media_ref_id: &str,
    to_media_ref_kind: &str,
    to_media_ref_id: &str,
) -> Result<usize> {
    validate_dj_timing_status(Some("missed"))?;
    validate_dj_runtime_renderer_status(Some("boundary_fallback"))?;
    validate_dj_runtime_renderer_reason(Some("manual_seek_suppressed"))?;
    conn.execute(
        "UPDATE dj_transition_events
         SET timing_status = 'missed',
             outcome = COALESCE(outcome, 'manual_seek_suppressed'),
             outcome_at = COALESCE(outcome_at, datetime('now')),
             runtime_rendered_dj_mixer = 0,
             runtime_renderer_status = 'boundary_fallback',
             runtime_renderer_reason = 'manual_seek_suppressed'
         WHERE id = (
             SELECT id
             FROM dj_transition_events
             WHERE from_media_ref_kind = ?1
               AND from_media_ref_id = ?2
               AND to_media_ref_kind = ?3
               AND to_media_ref_id = ?4
               AND outcome IS NULL
               AND timing_status = 'armed'
              ORDER BY started_at DESC, id DESC
              LIMIT 1
         )",
        params![
            from_media_ref_kind,
            from_media_ref_id,
            to_media_ref_kind,
            to_media_ref_id,
        ],
    )
    .map_err(Into::into)
}

pub fn count_recent_bad_dj_feedback_for_ref(
    conn: &Connection,
    key: &AudioDjProfileKey,
    limit: i64,
) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM (
            SELECT user_rating
            FROM dj_transition_events
            WHERE user_rating IS NOT NULL
              AND (
                (from_media_ref_kind = ?1 AND from_media_ref_id = ?2)
                OR (to_media_ref_kind = ?1 AND to_media_ref_id = ?2)
              )
            ORDER BY started_at DESC, id DESC
            LIMIT ?3
         )
         WHERE user_rating < 0",
        params![key.media_ref_kind, key.media_ref_id, limit.max(0)],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn audio_dj_profile_from_row(row: &Row<'_>) -> rusqlite::Result<AudioDjProfileRow> {
    Ok(AudioDjProfileRow {
        media_ref_kind: row.get(0)?,
        media_ref_id: row.get(1)?,
        track_id: row.get(2)?,
        queue_item_id: row.get(3)?,
        tidal_id: row.get(4)?,
        profile_version: row.get(5)?,
        beat_grid_blob: row.get(6)?,
        downbeats_blob: row.get(7)?,
        phrase_boundaries_blob: row.get(8)?,
        mix_in_blob: row.get(9)?,
        mix_out_blob: row.get(10)?,
        intro_end_seconds: row.get(11)?,
        outro_start_seconds: row.get(12)?,
        breakdown_blob: row.get(13)?,
        drop_blob: row.get(14)?,
        safe_transition_windows_blob: row.get(15)?,
        energy_contour_blob: row.get(16)?,
        vocal_presence_blob: row.get(17)?,
        vocal_density_blob: row.get(18)?,
        waveform_peaks_blob: row.get(19)?,
        lufs_loud_body: row.get(20)?,
        true_peak_dbtp: row.get(21)?,
        beat_confidence: row.get(22)?,
        profile_confidence: row.get(23)?,
        analysis_scope_ms: row.get(24)?,
        is_temporary: row.get::<_, i64>(25)? != 0,
        source: row.get(26)?,
        computed_at: row.get(27)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    fn read_onboarding_value(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT value FROM server_config WHERE key='onboarding_complete'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .expect("query server_config")
    }

    #[test]
    fn compute_track_similarity_swaps_from_temp_build_table() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, year) VALUES (1, 'Album', 1, 1999)",
            [],
        )
        .expect("album");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms)
             VALUES
                (1, 'One', 1, 1, 180000),
                (2, 'Two', 1, 1, 181000),
                (3, 'Three', 1, 1, 220000)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO track_similarity (track_a, track_b, similarity_score)
             VALUES (1, 2, 0.01)",
            [],
        )
        .expect("stale similarity");

        let count = compute_track_similarity(&conn).expect("compute similarity");
        assert_eq!(count, 3);

        let persisted_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM track_similarity", [], |row| {
                row.get(0)
            })
            .expect("persisted count");
        assert_eq!(persisted_count, 3);
        let score: f64 = conn
            .query_row(
                "SELECT similarity_score FROM track_similarity WHERE track_a = 1 AND track_b = 2",
                [],
                |row| row.get(0),
            )
            .expect("score");
        assert!(score > 0.01, "rebuild should replace stale similarity rows");
        assert!(
            conn.query_row("SELECT COUNT(*) FROM _track_similarity_build", [], |row| {
                row.get::<_, i64>(0)
            },)
                .is_err(),
            "temporary build table should be dropped after swap"
        );
    }

    #[test]
    fn compute_track_similarity_weights_rare_genres_above_broad_ones() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        schema::run_migrations(&conn).expect("migrations");
        // Distinct artists, no albums or listens, so genre_proximity is the only
        // non-zero signal and the test isolates the IDF weighting.
        for id in 1..=12 {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (?1, ?2)",
                params![id, format!("A{id}")],
            )
            .expect("artist");
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, ?2, ?1, 180000)",
                params![id, format!("T{id}")],
            )
            .expect("track");
        }
        conn.execute(
            "INSERT INTO genres (id, name, slug) VALUES (1,'Broad','broad'),(2,'Rare','rare')",
            [],
        )
        .expect("genres");
        // Broad genre covers 10 of 12 tracks; rare genre covers 2.
        for id in 1..=10 {
            conn.execute(
                "INSERT INTO track_genres (track_id, genre_id) VALUES (?1, 1)",
                params![id],
            )
            .expect("broad genre");
        }
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id) VALUES (11,2),(12,2)",
            [],
        )
        .expect("rare genre");

        compute_track_similarity(&conn).expect("compute similarity");

        let broad: f64 = conn
            .query_row(
                "SELECT genre_proximity FROM track_similarity WHERE track_a=1 AND track_b=2",
                [],
                |row| row.get(0),
            )
            .expect("broad pair");
        let rare: f64 = conn
            .query_row(
                "SELECT genre_proximity FROM track_similarity WHERE track_a=11 AND track_b=12",
                [],
                |row| row.get(0),
            )
            .expect("rare pair");

        assert!(
            rare > broad,
            "rare-genre pair ({rare}) should outscore broad-genre pair ({broad})"
        );
        assert!(
            broad > 0.0,
            "a broad-genre pair still carries some proximity"
        );
        assert!(
            (rare - 1.0).abs() < 1e-9,
            "the rarest shared-genre pair normalizes to 1.0"
        );
    }

    #[test]
    fn compute_track_similarity_weights_genre_bridges_by_confidence() {
        // A weakly-attested genre tag (a single-vote MusicBrainz "jazz" bleeding
        // onto a track, stored at low confidence after the scorer fix) must bridge
        // to genuine holders of that genre far more weakly than two confident
        // holders bridge to each other - the consumer side of the data-layer fix.
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        schema::run_migrations(&conn).expect("migrations");
        // Distinct artists, no albums or listens, so genre_proximity is isolated.
        for id in 1..=30 {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (?1, ?2)",
                params![id, format!("A{id}")],
            )
            .expect("artist");
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES (?1, ?2, ?1, 180000)",
                params![id, format!("T{id}")],
            )
            .expect("track");
        }
        conn.execute(
            "INSERT INTO genres (id, name, slug) VALUES (1,'Niche','niche')",
            [],
        )
        .expect("genre");
        // Tracks 16-18 genuinely hold the niche genre at full confidence; track 3
        // carries it only as a low-confidence bleed.
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
                (16,1,'lastfm',1.0),(17,1,'lastfm',1.0),(18,1,'lastfm',1.0),
                (3,1,'musicbrainz',0.2)",
            [],
        )
        .expect("niche genre tags");

        compute_track_similarity(&conn).expect("compute similarity");

        let gp = |a: i64, b: i64| -> f64 {
            conn.query_row(
                "SELECT genre_proximity FROM track_similarity WHERE track_a=?1 AND track_b=?2",
                params![a, b],
                |row| row.get(0),
            )
            .unwrap_or(0.0)
        };
        let genuine = gp(16, 17); // two confident holders
        let bleed = gp(3, 16); // low-confidence tag bridging to a confident holder

        assert!(
            bleed < genuine,
            "a low-confidence tag must bridge weaker ({bleed}) than two confident holders ({genuine})"
        );
        assert!(
            (genuine - 1.0).abs() < 1e-9,
            "the strongest genuine pair normalizes to 1.0"
        );
        // MIN() picks the weaker believer, so the bleed bridge is exactly the tag's
        // confidence fraction (0.2) of the genuine bridge.
        assert!(
            (bleed / genuine - 0.2).abs() < 1e-9,
            "the low-confidence bridge should scale with the tag's confidence, got {bleed}"
        );
    }

    #[test]
    fn get_genre_diverse_candidates_samples_one_track_per_artist() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1,'A'),(2,'B'),(3,'C'),(9,'Seed')",
            [],
        )
        .expect("artists");
        conn.execute(
            "INSERT INTO genres (id, name, slug) VALUES (1,'Shared','shared'),(2,'Other','other')",
            [],
        )
        .expect("genres");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms) VALUES
                (10,'seed',9,1000),
                (101,'a-low',1,1000),(102,'a-high',1,1000),
                (201,'b-low',2,1000),(202,'b-high',2,1000),
                (301,'c-only',3,1000),
                (401,'off-genre',9,1000)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id) VALUES
                (10,1),(101,1),(102,1),(201,1),(202,1),(301,1),(401,2)",
            [],
        )
        .expect("track_genres");

        let out = get_genre_diverse_candidates(&conn, 10, 100).expect("query");
        let artist_ids: HashSet<i64> = out.iter().map(|t| t.artist_id).collect();
        // One representative per artist that shares genre 1 - never two from one.
        assert_eq!(out.len(), artist_ids.len(), "no artist appears twice");
        assert!(artist_ids.contains(&1) && artist_ids.contains(&2) && artist_ids.contains(&3));
        // The representative is the lowest-id track per artist.
        assert!(out.iter().any(|t| t.id == 101) && !out.iter().any(|t| t.id == 102));
        assert!(out.iter().any(|t| t.id == 201) && !out.iter().any(|t| t.id == 202));
        // A track tagged only with a different genre must not leak in.
        assert!(!out.iter().any(|t| t.id == 401));
    }

    mod dj_transition_event {
        use super::*;

        fn setup_conn() -> Connection {
            let conn = Connection::open_in_memory().expect("in-memory db");
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .expect("foreign keys");
            schema::run_migrations(&conn).expect("migrations");
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
                .expect("artist");
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, tidal_id)
                 VALUES (1, 'Track 1', 1, 1001), (2, 'Track 2', 1, 1002)",
                [],
            )
            .expect("tracks");
            conn
        }

        fn insert_event(conn: &Connection) -> i64 {
            insert_dj_transition_event(
                conn,
                Some(1),
                Some(2),
                Some("library_track"),
                Some("1"),
                Some("tidal_track"),
                Some("200"),
                "SafeCrossfade",
                r#"{"template":"SafeCrossfade"}"#,
                None,
                "dj-v1",
                None,
                Some(172_000),
                Some("downbeat_sync"),
                Some("armed"),
            )
            .expect("insert event")
        }

        #[test]
        fn insert_dj_transition_event_round_trips() {
            let conn = setup_conn();
            let id = insert_event(&conn);

            let row = conn
                .query_row(
                    "SELECT from_track_id, to_track_id, template, program_json, planner_version,
                            planned_start_ms, timing_source, timing_status
                     FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                        ))
                    },
                )
                .expect("event");

            assert_eq!(row.0, Some(1));
            assert_eq!(row.1, Some(2));
            assert_eq!(row.2, "SafeCrossfade");
            assert_eq!(row.3, r#"{"template":"SafeCrossfade"}"#);
            assert_eq!(row.4, "dj-v1");
            assert_eq!(row.5, Some(172_000));
            assert_eq!(row.6.as_deref(), Some("downbeat_sync"));
            assert_eq!(row.7.as_deref(), Some("armed"));
        }

        #[test]
        fn insert_dj_transition_event_round_trips_external_refs() {
            let conn = setup_conn();
            let id = insert_dj_transition_event(
                &conn,
                None,
                None,
                Some("queue_item"),
                Some("44"),
                Some("tidal_track"),
                Some("555"),
                "SafeCrossfade",
                "{}",
                None,
                "dj-v1",
                None,
                None,
                None,
                None,
            )
            .expect("insert external refs");

            let refs = conn
                .query_row(
                    "SELECT from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id
                     FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .expect("refs");

            assert_eq!(refs.0.as_deref(), Some("queue_item"));
            assert_eq!(refs.1.as_deref(), Some("44"));
            assert_eq!(refs.2.as_deref(), Some("tidal_track"));
            assert_eq!(refs.3.as_deref(), Some("555"));
        }

        #[test]
        fn insert_dj_transition_event_round_trips_rejected_alternatives() {
            let conn = setup_conn();
            let rejected = r#"[{"template":"SlamCut","score":0.2,"reason":"low_confidence"}]"#;
            let id = insert_dj_transition_event(
                &conn,
                Some(1),
                Some(2),
                Some("library_track"),
                Some("1"),
                Some("library_track"),
                Some("2"),
                "SafeCrossfade",
                "{}",
                Some(rejected),
                "dj-v1",
                None,
                None,
                None,
                None,
            )
            .expect("insert rejected");

            let loaded: String = conn
                .query_row(
                    "SELECT rejected_alternatives_json FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .expect("rejected");
            assert_eq!(loaded, rejected);
        }

        #[test]
        fn insert_dj_transition_event_accepts_known_fallback_reasons() {
            let conn = setup_conn();
            let reasons = [
                "disabled",
                "current_profile_missing",
                "next_profile_missing",
                "profile_low_confidence",
                "next_not_resolved",
                "fetch_failed",
                "decode_late",
                "analysis_late",
                "program_invalid",
                "queue_changed",
                "safety_override_safe",
            ];

            for reason in reasons {
                insert_dj_transition_event(
                    &conn,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    "SafeCrossfade",
                    "{}",
                    None,
                    "dj-v1",
                    Some(reason),
                    None,
                    None,
                    None,
                )
                .unwrap_or_else(|error| panic!("reason {reason} rejected: {error}"));
            }
            assert!(
                insert_dj_transition_event(
                    &conn,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    "SafeCrossfade",
                    "{}",
                    None,
                    "dj-v1",
                    Some("unknown"),
                    None,
                    None,
                    None,
                )
                .is_err()
            );
        }

        #[test]
        fn update_dj_transition_outcome_sets_timestamp() {
            let conn = setup_conn();
            let id = insert_event(&conn);

            update_dj_transition_outcome(&conn, id, "bad", true).expect("update");

            let row = conn
                .query_row(
                    "SELECT outcome, outcome_at, skip_within_30s
                     FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .expect("outcome");
            assert_eq!(row.0.as_deref(), Some("bad"));
            assert!(row.1.is_some());
            assert_eq!(row.2, 1);
        }

        #[test]
        fn manual_skip_outcome_does_not_write_actual_timing() {
            let conn = setup_conn();
            let id = insert_event(&conn);

            update_dj_transition_outcome(&conn, id, "skip_within_30s", true).expect("update");

            let actual_start_ms: Option<i64> = conn
                .query_row(
                    "SELECT actual_start_ms FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .expect("actual");
            assert_eq!(actual_start_ms, None);
        }

        #[test]
        fn update_dj_transition_fire_timing_sets_delta_and_status() {
            let conn = setup_conn();
            let id = insert_event(&conn);

            update_dj_transition_fire_timing(
                &conn,
                id,
                172_144,
                "fired",
                true,
                "rendered_handoff",
                "none",
            )
            .expect("timing");

            let row = conn
                .query_row(
                    "SELECT actual_start_ms, timing_delta_ms, timing_status,
                            runtime_rendered_dj_mixer, runtime_renderer_status,
                            runtime_renderer_reason
                     FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .expect("timing row");
            assert_eq!(row.0, Some(172_144));
            assert_eq!(row.1, Some(144));
            assert_eq!(row.2.as_deref(), Some("fired"));
            assert_eq!(row.3, Some(1));
            assert_eq!(row.4.as_deref(), Some("rendered_handoff"));
            assert_eq!(row.5.as_deref(), Some("none"));
        }

        #[test]
        fn update_dj_transition_fire_timing_leaves_missed_fire_timing_empty() {
            let conn = setup_conn();
            let id = insert_event(&conn);

            update_dj_transition_fire_timing(
                &conn,
                id,
                199_465,
                "missed",
                false,
                "boundary_fallback",
                "prepared_mixer_missing",
            )
            .expect("timing");

            let row = conn
                .query_row(
                    "SELECT actual_start_ms, timing_delta_ms, timing_status,
                            runtime_rendered_dj_mixer, runtime_renderer_status,
                            runtime_renderer_reason
                     FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .expect("timing row");

            assert_eq!(row.0, None);
            assert_eq!(row.1, None);
            assert_eq!(row.2.as_deref(), Some("missed"));
            assert_eq!(row.3, Some(0));
            assert_eq!(row.4.as_deref(), Some("boundary_fallback"));
            assert_eq!(row.5.as_deref(), Some("prepared_mixer_missing"));
        }

        #[test]
        fn update_dj_transition_fire_timing_accepts_precise_miss_reasons() {
            let conn = setup_conn();

            for reason in [
                "next_decode_late_at_fire",
                "next_deck_missing_at_fire",
                "transition_plan_missing_at_fire",
                "sync_window_not_signaled",
            ] {
                let id = insert_event(&conn);
                update_dj_transition_fire_timing(
                    &conn,
                    id,
                    199_465,
                    "missed",
                    false,
                    "boundary_fallback",
                    reason,
                )
                .expect("timing");

                let stored_reason: Option<String> = conn
                    .query_row(
                        "SELECT runtime_renderer_reason FROM dj_transition_events WHERE id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .expect("stored reason");
                assert_eq!(stored_reason.as_deref(), Some(reason));
            }
        }

        #[test]
        fn update_dj_transition_fire_timing_accepts_manual_seek_reason() {
            let conn = setup_conn();
            let id = insert_event(&conn);

            update_dj_transition_fire_timing(
                &conn,
                id,
                180_000,
                "late",
                false,
                "boundary_fallback",
                "manual_seek_suppressed",
            )
            .expect("manual seek timing");

            let reason: Option<String> = conn
                .query_row(
                    "SELECT runtime_renderer_reason FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .expect("reason");
            assert_eq!(reason.as_deref(), Some("manual_seek_suppressed"));
        }

        #[test]
        fn update_dj_transition_fire_timing_closes_duplicate_armed_pair_rows() {
            let conn = setup_conn();
            let older_id = insert_event(&conn);
            let fired_id = insert_event(&conn);

            update_dj_transition_fire_timing(
                &conn,
                fired_id,
                172_144,
                "fired",
                false,
                "legacy_overlap",
                "next_deck_not_decoded",
            )
            .expect("timing");

            let rows: Vec<(i64, Option<String>)> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, timing_status
                         FROM dj_transition_events
                         WHERE id IN (?1, ?2)
                         ORDER BY id",
                    )
                    .expect("prepare");
                stmt.query_map(params![older_id, fired_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                })
                .expect("query")
                .collect::<rusqlite::Result<_>>()
                .expect("rows")
            };

            assert_eq!(rows[0], (older_id, Some("missed".to_string())));
            assert_eq!(rows[1], (fired_id, Some("fired".to_string())));
        }

        #[test]
        fn mark_dj_transition_timing_status_for_pair_updates_only_armed_pair() {
            let conn = setup_conn();
            let updated = mark_dj_transition_timing_status_for_pair(
                &conn,
                "library_track",
                "1",
                "tidal_track",
                "200",
                "missed",
            )
            .expect("mark before insert");
            assert_eq!(updated, 0);

            let id = insert_event(&conn);
            let updated = mark_dj_transition_timing_status_for_pair(
                &conn,
                "library_track",
                "1",
                "tidal_track",
                "200",
                "missed",
            )
            .expect("mark");
            assert_eq!(updated, 1);

            let status: Option<String> = conn
                .query_row(
                    "SELECT timing_status FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .expect("status");
            assert_eq!(status.as_deref(), Some("missed"));
        }

        #[test]
        fn mark_dj_transition_manual_seek_suppressed_closes_armed_pair() {
            let conn = setup_conn();
            let id = insert_event(&conn);

            let updated = mark_dj_transition_manual_seek_suppressed_for_pair(
                &conn,
                "library_track",
                "1",
                "tidal_track",
                "200",
            )
            .expect("mark manual seek suppressed");
            assert_eq!(updated, 1);

            let row = conn
                .query_row(
                    "SELECT timing_status, outcome, runtime_rendered_dj_mixer,
                            runtime_renderer_status, runtime_renderer_reason
                     FROM dj_transition_events WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    },
                )
                .expect("manual seek row");

            assert_eq!(row.0.as_deref(), Some("missed"));
            assert_eq!(row.1.as_deref(), Some("manual_seek_suppressed"));
            assert_eq!(row.2, Some(0));
            assert_eq!(row.3.as_deref(), Some("boundary_fallback"));
            assert_eq!(row.4.as_deref(), Some("manual_seek_suppressed"));

            let second = mark_dj_transition_manual_seek_suppressed_for_pair(
                &conn,
                "library_track",
                "1",
                "tidal_track",
                "200",
            )
            .expect("mark manual seek suppressed again");
            assert_eq!(second, 0);
        }

        #[test]
        fn mark_dj_transition_timing_status_for_pair_updates_new_attempt_after_old_fired_pair() {
            let conn = setup_conn();
            let fired_id = insert_event(&conn);
            update_dj_transition_fire_timing(
                &conn,
                fired_id,
                172_040,
                "fired",
                false,
                "legacy_overlap",
                "prepared_mixer_missing",
            )
            .expect("mark fired");
            let armed_id = insert_event(&conn);

            let updated = mark_dj_transition_timing_status_for_pair(
                &conn,
                "library_track",
                "1",
                "tidal_track",
                "200",
                "missed",
            )
            .expect("mark");

            assert_eq!(updated, 1);
            let status: Option<String> = conn
                .query_row(
                    "SELECT timing_status FROM dj_transition_events WHERE id = ?1",
                    params![armed_id],
                    |row| row.get(0),
                )
                .expect("status");
            assert_eq!(status.as_deref(), Some("missed"));
        }
    }

    mod audio_dj_profile {
        use super::*;

        fn setup_conn() -> Connection {
            let conn = Connection::open_in_memory().expect("in-memory db");
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .expect("foreign keys");
            schema::run_migrations(&conn).expect("migrations");
            conn
        }

        fn key(kind: &str, id: &str) -> AudioDjProfileKey {
            AudioDjProfileKey {
                media_ref_kind: kind.to_string(),
                media_ref_id: id.to_string(),
            }
        }

        fn seed_track(conn: &Connection) -> i64 {
            conn.execute("INSERT INTO artists (name) VALUES ('Artist')", [])
                .expect("artist");
            let artist_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO tracks (title, artist_id, tidal_id) VALUES ('Track', ?1, 12345)",
                params![artist_id],
            )
            .expect("track");
            conn.last_insert_rowid()
        }

        fn profile_row(kind: &str, id: &str) -> AudioDjProfileRow {
            AudioDjProfileRow {
                media_ref_kind: kind.to_string(),
                media_ref_id: id.to_string(),
                track_id: None,
                queue_item_id: None,
                tidal_id: None,
                profile_version: "dj_profile_v1".to_string(),
                beat_grid_blob: vec![1, 2, 3],
                downbeats_blob: vec![4, 5],
                phrase_boundaries_blob: vec![6],
                mix_in_blob: vec![7],
                mix_out_blob: vec![8],
                intro_end_seconds: Some(16.0),
                outro_start_seconds: Some(180.0),
                breakdown_blob: vec![9],
                drop_blob: vec![10],
                safe_transition_windows_blob: vec![11],
                energy_contour_blob: vec![12],
                vocal_presence_blob: vec![13],
                vocal_density_blob: vec![14],
                waveform_peaks_blob: vec![15],
                lufs_loud_body: Some(-12.0),
                true_peak_dbtp: Some(-1.0),
                beat_confidence: Some(0.9),
                profile_confidence: 0.85,
                analysis_scope_ms: 90_000,
                is_temporary: false,
                source: "test".to_string(),
                computed_at: "2026-05-21T00:00:00Z".to_string(),
            }
        }

        fn correction_row(kind: &str, id: &str) -> AudioDjProfileCorrectionRow {
            AudioDjProfileCorrectionRow {
                media_ref_kind: kind.to_string(),
                media_ref_id: id.to_string(),
                bpm_multiplier: Some(2.0),
                downbeat_offset_beats: Some(1),
                phrase_offset_bars: Some(-2),
                safe_crossfade_only: true,
                transition_speed_bias: Some("faster".to_string()),
                manual_drop_blob: vec![20, 21, 22],
                notes: Some("user correction".to_string()),
                created_at: "2026-05-21T00:00:00Z".to_string(),
                updated_at: "2026-05-21T00:00:01Z".to_string(),
            }
        }

        fn table_exists(conn: &Connection, table: &str) -> bool {
            conn.query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |_| Ok(()),
            )
            .optional()
            .expect("table lookup")
            .is_some()
        }

        #[test]
        fn migration_043_creates_audio_dj_profiles() {
            let conn = setup_conn();
            assert!(table_exists(&conn, "audio_dj_profiles"));
            assert!(table_exists(&conn, "dj_transition_events"));
        }

        #[test]
        fn migration_043_creates_audio_dj_profile_corrections() {
            let conn = setup_conn();
            assert!(table_exists(&conn, "audio_dj_profile_corrections"));
        }

        #[test]
        fn upsert_audio_dj_profile_round_trips_library_key() {
            let conn = setup_conn();
            let track_id = seed_track(&conn);
            let mut row = profile_row("library_track", &track_id.to_string());
            row.track_id = Some(track_id);

            upsert_audio_dj_profile(&conn, &row).expect("upsert");
            let loaded = get_audio_dj_profile(&conn, &key("library_track", &track_id.to_string()))
                .expect("get")
                .expect("profile");
            assert_eq!(loaded.track_id, Some(track_id));
            assert_eq!(loaded.media_ref_kind, "library_track");

            let by_track = get_audio_dj_profile_for_track(&conn, track_id)
                .expect("get track")
                .expect("track profile");
            assert_eq!(by_track.media_ref_id, track_id.to_string());
        }

        #[test]
        fn upsert_audio_dj_profile_round_trips_tidal_key() {
            let conn = setup_conn();
            let mut row = profile_row("tidal_track", "98765");
            row.tidal_id = Some(98_765);

            upsert_audio_dj_profile(&conn, &row).expect("upsert");
            let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "98765"))
                .expect("get")
                .expect("profile");
            assert_eq!(loaded.tidal_id, Some(98_765));
            assert_eq!(loaded.media_ref_id, "98765");
        }

        #[test]
        fn audio_dj_profile_allows_external_queue_item_without_track_id() {
            let conn = setup_conn();
            conn.execute(
                "INSERT INTO queue (position, source, pending_artist, pending_title)
                 VALUES (1, 'radio_pending', 'Artist', 'Title')",
                [],
            )
            .expect("queue item");
            let queue_item_id = conn.last_insert_rowid();
            let mut row = profile_row("queue_item", &queue_item_id.to_string());
            row.queue_item_id = Some(queue_item_id);
            row.is_temporary = true;

            upsert_audio_dj_profile(&conn, &row).expect("upsert");
            let loaded =
                get_audio_dj_profile(&conn, &key("queue_item", &queue_item_id.to_string()))
                    .expect("get")
                    .expect("profile");
            assert_eq!(loaded.track_id, None);
            assert_eq!(loaded.queue_item_id, Some(queue_item_id));
            assert!(loaded.is_temporary);
        }

        #[test]
        fn audio_dj_profile_stores_confidence_and_scope() {
            let conn = setup_conn();
            let mut row = profile_row("tidal_track", "1");
            row.profile_confidence = 0.4;
            row.analysis_scope_ms = 30_000;

            upsert_audio_dj_profile(&conn, &row).expect("upsert");
            let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "1"))
                .expect("get")
                .expect("profile");
            assert_eq!(loaded.profile_confidence, 0.4);
            assert_eq!(loaded.analysis_scope_ms, 30_000);
        }

        #[test]
        fn audio_dj_profile_peak_blob_round_trips() {
            let conn = setup_conn();
            let mut row = profile_row("tidal_track", "1");
            row.waveform_peaks_blob = vec![1, 2, 3, 4, 5];

            upsert_audio_dj_profile(&conn, &row).expect("upsert");
            let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "1"))
                .expect("get")
                .expect("profile");

            assert_eq!(loaded.waveform_peaks_blob, vec![1, 2, 3, 4, 5]);
        }

        #[test]
        fn audio_dj_profile_empty_peak_blob_loads() {
            let conn = setup_conn();
            let mut row = profile_row("tidal_track", "1");
            row.waveform_peaks_blob = Vec::new();

            upsert_audio_dj_profile(&conn, &row).expect("upsert");
            let loaded = get_audio_dj_profile(&conn, &key("tidal_track", "1"))
                .expect("get")
                .expect("profile");

            assert!(loaded.waveform_peaks_blob.is_empty());
        }

        #[test]
        fn promote_temporary_audio_dj_profile_copies_to_stable_key() {
            let conn = setup_conn();
            let mut row = profile_row("queue_item", "44");
            row.is_temporary = true;
            upsert_audio_dj_profile(&conn, &row).expect("upsert temp");

            promote_temporary_audio_dj_profile(
                &conn,
                &key("queue_item", "44"),
                &key("tidal_track", "555"),
                Some(555),
            )
            .expect("promote");

            let stable = get_audio_dj_profile(&conn, &key("tidal_track", "555"))
                .expect("get stable")
                .expect("stable");
            let temporary = get_audio_dj_profile(&conn, &key("queue_item", "44"))
                .expect("get temp")
                .expect("temp");
            assert_eq!(stable.beat_grid_blob, temporary.beat_grid_blob);
            assert_eq!(stable.tidal_id, Some(555));
            assert!(!stable.is_temporary);
            assert!(temporary.is_temporary);
        }

        #[test]
        fn upsert_audio_dj_profile_correction_round_trips() {
            let conn = setup_conn();
            let row = correction_row("tidal_track", "1");
            upsert_audio_dj_profile_correction(&conn, &row).expect("upsert correction");

            let loaded = get_audio_dj_profile_correction(&conn, &key("tidal_track", "1"))
                .expect("get")
                .expect("correction");
            assert_eq!(loaded.bpm_multiplier, Some(2.0));
            assert_eq!(loaded.downbeat_offset_beats, Some(1));
            assert_eq!(loaded.phrase_offset_bars, Some(-2));
            assert!(loaded.safe_crossfade_only);
            assert_eq!(loaded.transition_speed_bias.as_deref(), Some("faster"));
            assert_eq!(loaded.manual_drop_blob, vec![20, 21, 22]);
        }

        #[test]
        fn audio_dj_profile_correction_rejects_unknown_transition_speed_bias() {
            let conn = setup_conn();
            let mut row = correction_row("tidal_track", "1");
            row.transition_speed_bias = Some("sideways".to_string());

            assert!(upsert_audio_dj_profile_correction(&conn, &row).is_err());
        }

        #[test]
        fn dj_engine_enabled_defaults_false() {
            let conn = setup_conn();
            assert!(!is_dj_engine_enabled(&conn).expect("enabled"));
        }

        #[test]
        fn set_dj_engine_enabled_round_trips() {
            let conn = setup_conn();
            set_dj_engine_enabled(&conn, true).expect("enable");
            assert!(is_dj_engine_enabled(&conn).expect("enabled"));
            set_dj_engine_enabled(&conn, false).expect("disable");
            assert!(!is_dj_engine_enabled(&conn).expect("disabled"));
        }

        #[test]
        fn dj_global_policy_defaults_balanced_neutral() {
            let conn = setup_conn();
            assert_eq!(
                get_dj_global_policy(&conn).expect("policy"),
                ("balanced".to_string(), "neutral".to_string())
            );
        }

        #[test]
        fn set_dj_global_policy_round_trips() {
            let conn = setup_conn();
            set_dj_global_policy(&conn, "bold", "faster").expect("set policy");
            assert_eq!(
                get_dj_global_policy(&conn).expect("policy"),
                ("bold".to_string(), "faster".to_string())
            );
        }

        #[test]
        fn set_dj_global_policy_rejects_unknown_values() {
            let conn = setup_conn();
            assert!(set_dj_global_policy(&conn, "chaos", "neutral").is_err());
            assert!(set_dj_global_policy(&conn, "safe", "sideways").is_err());
        }

        #[test]
        fn count_recent_bad_dj_feedback_for_ref_counts_from_and_to_roles() {
            let conn = setup_conn();
            let key = key("tidal_track", "1");
            conn.execute(
                "INSERT INTO dj_transition_events (
                    from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                    template, program_json, planner_version, user_rating, started_at
                 ) VALUES (?1, ?2, 'tidal_track', '2', 'SafeCrossfade', '{}', 'v1', -1, '2026-05-21T00:00:00Z')",
                params![key.media_ref_kind, key.media_ref_id],
            )
            .expect("from event");
            conn.execute(
                "INSERT INTO dj_transition_events (
                    from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                    template, program_json, planner_version, user_rating, started_at
                 ) VALUES ('tidal_track', '3', ?1, ?2, 'SafeCrossfade', '{}', 'v1', -1, '2026-05-21T00:00:01Z')",
                params![key.media_ref_kind, key.media_ref_id],
            )
            .expect("to event");
            conn.execute(
                "INSERT INTO dj_transition_events (
                    from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                    template, program_json, planner_version, user_rating, started_at
                 ) VALUES (?1, ?2, 'tidal_track', '4', 'SafeCrossfade', '{}', 'v1', 1, '2026-05-21T00:00:02Z')",
                params![key.media_ref_kind, key.media_ref_id],
            )
            .expect("good event");

            assert_eq!(
                count_recent_bad_dj_feedback_for_ref(&conn, &key, 10).expect("count"),
                2
            );
            assert_eq!(
                count_recent_bad_dj_feedback_for_ref(&conn, &key, 1).expect("count"),
                0
            );
        }
    }

    #[test]
    fn onboarding_unset_no_tidal_returns_false() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        assert!(!get_onboarding_complete(&conn).expect("read flag"));
        assert!(
            read_onboarding_value(&conn).is_none(),
            "must not write a row when nothing implies completion"
        );
    }

    #[test]
    fn onboarding_unset_with_tidal_writes_flag_and_returns_true() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO service_auth (service, user_id) VALUES ('tidal', 'u-123')",
            [],
        )
        .expect("seed tidal auth");

        assert!(get_onboarding_complete(&conn).expect("read flag"));
        assert_eq!(read_onboarding_value(&conn).as_deref(), Some("1"));
    }

    #[test]
    fn create_embedding_model_inserts_run_scoped_rows_without_overwriting_active_model() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let active = create_embedding_model(
            &conn,
            "discovery-fusion-v2:1",
            "discovery-fusion-v2",
            64,
            "ready",
            Some(r#"{"run":1}"#),
        )
        .expect("create active");
        activate_embedding_model(&conn, active.id).expect("activate active");

        let candidate = create_embedding_model(
            &conn,
            "discovery-fusion-v2:2",
            "discovery-fusion-v2",
            64,
            "training",
            Some(r#"{"run":2}"#),
        )
        .expect("create candidate");

        assert_ne!(active.id, candidate.id);
        let still_active = get_selected_discovery_embedding_model(&conn)
            .expect("selected lookup")
            .expect("selected model");
        assert_eq!(still_active.id, active.id);
        assert_eq!(still_active.model_key, "discovery-fusion-v2:1");
    }

    #[test]
    fn neighbor_support_breakdown_round_trips_through_full_and_seed_replacement() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id, duration_ms)
             VALUES (1, 'Seed', 1, 101, 180000),
                    (2, 'Neighbor', 1, 102, 181000),
                    (3, 'Refresh', 1, 103, 182000)",
            [],
        )
        .expect("seed tracks");
        let model = create_embedding_model(
            &conn,
            "discovery-fusion-v2:1",
            "discovery-fusion-v2",
            64,
            "ready",
            None,
        )
        .expect("create model");

        replace_track_neighbors(
            &conn,
            model.id,
            &[NeighborWriteRow {
                track_id: 1,
                neighbor_track_id: 2,
                rank: 1,
                score: 0.91,
                behavioral_score: 0.4,
                audio_score: 0.3,
                metadata_score: 0.2,
                reason_json: None,
                primary_reason: Some("direct_transition".to_string()),
                confidence: 0.8,
                support_count: 4,
                support_transition: 2.5,
                support_colisten: 1.25,
                support_structure: 0.75,
                support_metadata: 0.5,
                candidate_in_degree: 7,
                candidate_in_degree_percentile: 0.7,
                play_count_seed: 10,
                play_count_candidate: 3,
            }],
        )
        .expect("replace neighbors");

        let rows = get_track_neighbors(&conn, model.id, 1, 10, &[]).expect("read neighbors");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].support_count, 4);
        assert_eq!(rows[0].support_transition, 2.5);
        assert_eq!(rows[0].support_colisten, 1.25);
        assert_eq!(rows[0].support_structure, 0.75);
        assert_eq!(rows[0].support_metadata, 0.5);

        replace_seed_neighbors(
            &conn,
            model.id,
            1,
            &[NeighborWriteRow {
                track_id: 1,
                neighbor_track_id: 3,
                rank: 1,
                score: 0.88,
                behavioral_score: 0.35,
                audio_score: 0.35,
                metadata_score: 0.18,
                reason_json: None,
                primary_reason: Some("session_colisten".to_string()),
                confidence: 0.77,
                support_count: 3,
                support_transition: 0.0,
                support_colisten: 2.0,
                support_structure: 1.0,
                support_metadata: 0.25,
                candidate_in_degree: 5,
                candidate_in_degree_percentile: 0.6,
                play_count_seed: 10,
                play_count_candidate: 4,
            }],
        )
        .expect("replace seed neighbors");

        let refreshed =
            get_track_neighbors(&conn, model.id, 1, 10, &[]).expect("read refreshed neighbors");
        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].track_id, 3);
        assert_eq!(refreshed[0].support_count, 3);
        assert_eq!(refreshed[0].support_transition, 0.0);
        assert_eq!(refreshed[0].support_colisten, 2.0);
        assert_eq!(refreshed[0].support_structure, 1.0);
        assert_eq!(refreshed[0].support_metadata, 0.25);
    }

    #[test]
    fn selected_discovery_model_lookup_uses_configured_engine_family() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let legacy = create_embedding_model(
            &conn,
            "discovery-fusion:legacy",
            "discovery-fusion",
            96,
            "ready",
            None,
        )
        .expect("create legacy model");
        let v2 = create_embedding_model(
            &conn,
            "discovery-fusion-v2:default",
            "discovery-fusion-v2",
            64,
            "ready",
            None,
        )
        .expect("create v2 model");
        activate_embedding_model(&conn, v2.id).expect("activate v2");

        let selected = get_selected_discovery_embedding_model(&conn)
            .expect("selected lookup")
            .expect("selected default model");
        assert_eq!(selected.id, v2.id);

        conn.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES ('discovery_engine', 'v1')",
            [],
        )
        .expect("select legacy engine");

        let selected = get_selected_discovery_embedding_model(&conn)
            .expect("selected lookup")
            .expect("selected legacy model");
        assert_eq!(selected.id, legacy.id);
        assert_eq!(selected.family, "discovery-fusion");
    }

    #[test]
    fn is_discovery_training_running_tracks_run_status() {
        // Regression: the radio similarity rebuild gates on this so it can't run
        // a multi-minute write transaction alongside discovery training.
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        assert!(
            !is_discovery_training_running(&conn).expect("query"),
            "no runs => not training"
        );

        let run = create_training_run(&conn, None, "behavioral", "running").expect("create run");
        assert!(
            is_discovery_training_running(&conn).expect("query"),
            "a running row => training in progress, rebuild must defer"
        );

        finish_training_run(&conn, run.id, "completed").expect("finish run");
        assert!(
            !is_discovery_training_running(&conn).expect("query"),
            "completed run => training done, rebuild may proceed"
        );
    }

    #[test]
    fn finish_training_run_with_error_preserves_cancel_reason() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        let run = create_training_run(&conn, None, "behavioral", "running").expect("create run");

        finish_training_run_with_error(
            &conn,
            run.id,
            "cancelled",
            "Laptop safety timeout stopped discovery training.",
        )
        .expect("finish with reason");

        let stored = get_training_run(&conn, run.id)
            .expect("load run")
            .expect("stored run");
        assert_eq!(stored.status, "cancelled");
        assert_eq!(
            stored.error_text.as_deref(),
            Some("Laptop safety timeout stopped discovery training.")
        );
    }

    #[test]
    fn bulk_neighbor_loading_groups_by_seed_and_preserves_support_columns() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id)
             VALUES (1, 'Seed A', 1, 201),
                    (2, 'Seed B', 1, 202),
                    (3, 'Candidate A', 1, 203),
                    (4, 'Candidate B', 1, 204)",
            [],
        )
        .expect("seed tracks");
        let model = create_embedding_model(
            &conn,
            "discovery-fusion-v2:bulk",
            "discovery-fusion-v2",
            64,
            "ready",
            None,
        )
        .expect("create model");

        let mk = |track_id, neighbor_track_id, rank, support_transition: f64| NeighborWriteRow {
            track_id,
            neighbor_track_id,
            rank,
            score: 0.9,
            behavioral_score: 0.4,
            audio_score: 0.3,
            metadata_score: 0.2,
            reason_json: None,
            primary_reason: None,
            confidence: 0.8,
            support_count: support_transition.round() as i64,
            support_transition,
            support_colisten: 0.0,
            support_structure: 0.0,
            support_metadata: 0.0,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
            play_count_seed: 0,
            play_count_candidate: 0,
        };
        replace_track_neighbors(&conn, model.id, &[mk(1, 3, 1, 2.0), mk(2, 4, 1, 3.0)])
            .expect("replace neighbors");

        let grouped =
            get_track_neighbors_for_seeds(&conn, model.id, &[1, 2], 10).expect("bulk load");
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped.get(&1).unwrap()[0].track_id, 3);
        assert_eq!(grouped.get(&1).unwrap()[0].support_transition, 2.0);
        assert_eq!(grouped.get(&2).unwrap()[0].track_id, 4);
        assert_eq!(grouped.get(&2).unwrap()[0].support_transition, 3.0);
    }

    #[test]
    fn completion_weighted_listen_edges_downweight_skipped_tracks() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id, duration_ms)
             VALUES (1, 'Half Listen', 1, 301, 180000),
                    (2, 'Complete Listen', 1, 302, 180000)",
            [],
        )
        .expect("seed tracks");
        conn.execute(
            "INSERT INTO listen_history
                (id, track_id, started_at, duration_listened_ms, completed, session_id, source, position_in_session)
             VALUES
                (1, 1, '2026-01-01 00:00:00', 90000, 0, 's1', 'manual', 1),
                (2, 2, '2026-01-01 00:03:00', 180000, 1, 's1', 'manual', 2)",
            [],
        )
        .expect("seed listens");

        let rows = get_completion_weighted_listen_edges(&conn, 45).expect("weighted edges");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].from_track_id, 1);
        assert_eq!(rows[0].to_track_id, 2);
        assert!((rows[0].weight - 0.5).abs() < 1e-9);
        assert_eq!(rows[0].source.as_deref(), Some("manual"));
    }

    #[test]
    fn listen_history_transition_edges_preserve_source_and_completion_weight() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id, duration_ms)
             VALUES (1, 'Before', 1, 401, 200000),
                    (2, 'After', 1, 402, 200000)",
            [],
        )
        .expect("seed tracks");
        conn.execute(
            "INSERT INTO listen_history
                (id, track_id, started_at, duration_listened_ms, completed, source, transition_from_track_id)
             VALUES
                (10, 2, '2026-01-01 00:04:00', 50000, 0, 'automix-new', 1)",
            [],
        )
        .expect("seed transition listen");

        let rows = get_listen_history_transition_edges(&conn).expect("transition edges");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_id, "listen_history:10");
        assert_eq!(rows[0].from_track_id, 1);
        assert_eq!(rows[0].to_track_id, 2);
        assert!((rows[0].weight - 0.25).abs() < 1e-9);
        assert_eq!(rows[0].source.as_deref(), Some("automix-new"));
    }

    #[test]
    fn external_candidate_upsert_dedupes_unresolved_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let first = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "artist:unknown|title:signal|dur:180".to_string(),
                title: "Signal".to_string(),
                artist_name: "Unknown Artist".to_string(),
                genre_tags_json: Some(r#"["electronic"]"#.to_string()),
                duration_ms: Some(180_000),
                expires_at: "2026-02-01 00:00:00".to_string(),
            },
        )
        .expect("insert candidate");
        let second = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "artist:unknown|title:signal|dur:180".to_string(),
                title: "Signal".to_string(),
                artist_name: "Unknown Artist".to_string(),
                genre_tags_json: Some(r#"["electronic","fresh"]"#.to_string()),
                duration_ms: Some(180_000),
                expires_at: "2026-02-02 00:00:00".to_string(),
            },
        )
        .expect("upsert candidate");

        assert_eq!(first.id, second.id);
        assert_eq!(
            second.genre_tags_json.as_deref(),
            Some(r#"["electronic","fresh"]"#)
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidates",
                [],
                |row| row.get(0),
            )
            .expect("count candidates");
        assert_eq!(count, 1);
    }

    #[test]
    fn external_candidate_upsert_dedupes_unresolved_rows_by_normalized_identity() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let first = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "provider-a:signal".to_string(),
                title: "Signal!".to_string(),
                artist_name: "Unknown Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(181_000),
                expires_at: "2026-02-01 00:00:00".to_string(),
            },
        )
        .expect("insert candidate");
        let second = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "provider-b:signal".to_string(),
                title: "signal".to_string(),
                artist_name: "unknown-artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(185_000),
                expires_at: "2026-02-02 00:00:00".to_string(),
            },
        )
        .expect("upsert candidate");

        assert_eq!(first.id, second.id);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidates",
                [],
                |row| row.get(0),
            )
            .expect("count candidates");
        assert_eq!(count, 1);
    }

    #[test]
    fn external_sightings_and_neighbors_replace_without_stale_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id)
             VALUES (1, 'Seed', 1, 501)",
            [],
        )
        .expect("seed track");
        let model = create_embedding_model(
            &conn,
            "discovery-fusion-v2:external",
            "discovery-fusion-v2",
            64,
            "ready",
            None,
        )
        .expect("create model");
        let candidate = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: Some(9001),
                mbid: Some("mbid-9001".to_string()),
                dedupe_key: "tidal:9001".to_string(),
                title: "External".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(200_000),
                expires_at: "2026-02-01 00:00:00".to_string(),
            },
        )
        .expect("candidate");

        upsert_external_candidate_sighting(
            &conn,
            &ExternalCandidateSightingUpsert {
                candidate_id: candidate.id,
                seed_track_id: 1,
                source: "lastfm_similar".to_string(),
                source_payload_json: Some(r#"{"match":0.9}"#.to_string()),
                similarity: Some(0.9),
                expires_at: "2026-02-01 00:00:00".to_string(),
            },
        )
        .expect("insert sighting");
        upsert_external_candidate_sighting(
            &conn,
            &ExternalCandidateSightingUpsert {
                candidate_id: candidate.id,
                seed_track_id: 1,
                source: "lastfm_similar".to_string(),
                source_payload_json: Some(r#"{"match":0.95}"#.to_string()),
                similarity: Some(0.95),
                expires_at: "2026-02-02 00:00:00".to_string(),
            },
        )
        .expect("update sighting");
        let sighting_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidate_sightings",
                [],
                |row| row.get(0),
            )
            .expect("count sightings");
        assert_eq!(sighting_count, 1);

        replace_external_candidate_neighbors(
            &conn,
            model.id,
            1,
            &[ExternalCandidateNeighborWriteRow {
                candidate_id: candidate.id,
                rank: 1,
                score: 0.91,
                audio_score: 0.8,
                metadata_score: 0.11,
                reason_json: Some(r#"[{"key":"lastfm_similar"}]"#.to_string()),
            }],
        )
        .expect("write neighbor");
        replace_external_candidate_neighbors(&conn, model.id, 1, &[])
            .expect("remove stale neighbors");
        let neighbor_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidate_neighbors",
                [],
                |row| row.get(0),
            )
            .expect("count neighbors");
        assert_eq!(neighbor_count, 0);
    }

    #[test]
    fn external_candidate_merge_moves_sidecar_rows_before_deleting_loser() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id)
             VALUES (1, 'Seed', 1, 701)",
            [],
        )
        .expect("seed track");
        let model = create_embedding_model(
            &conn,
            "discovery-fusion-v2:external-merge",
            "discovery-fusion-v2",
            2,
            "ready",
            None,
        )
        .expect("create model");
        let winner = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: Some(9100),
                mbid: None,
                dedupe_key: "tidal:9100".to_string(),
                title: "Winner".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(100_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("winner");
        let loser = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "fallback:winner".to_string(),
                title: "Winner".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(100_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("loser");
        upsert_external_candidate_sighting(
            &conn,
            &ExternalCandidateSightingUpsert {
                candidate_id: loser.id,
                seed_track_id: 1,
                source: "lastfm_similar".to_string(),
                source_payload_json: None,
                similarity: Some(0.8),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("sighting");
        let feature_blob = [1_u8, 2];
        conn.execute(
            "INSERT INTO external_track_candidate_audio_features
             (candidate_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms)
             VALUES (?1, 'v', ?2, 0, 1)",
            params![loser.id, &feature_blob[..]],
        )
        .expect("feature");
        let embedding_blob = [3_u8, 4];
        conn.execute(
            "INSERT INTO external_track_candidate_embeddings
             (candidate_id, model_id, vector_blob, l2_norm)
             VALUES (?1, ?2, ?3, 1.0)",
            params![loser.id, model.id, &embedding_blob[..]],
        )
        .expect("embedding");
        replace_external_candidate_neighbors(
            &conn,
            model.id,
            1,
            &[ExternalCandidateNeighborWriteRow {
                candidate_id: loser.id,
                rank: 1,
                score: 0.7,
                audio_score: 0.7,
                metadata_score: 0.0,
                reason_json: None,
            }],
        )
        .expect("neighbor");

        merge_external_track_candidates(&conn, winner.id, loser.id).expect("merge");

        let loser_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidates WHERE id = ?1",
                params![loser.id],
                |row| row.get(0),
            )
            .expect("loser count");
        let moved_sightings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidate_sightings WHERE candidate_id = ?1",
                params![winner.id],
                |row| row.get(0),
            )
            .expect("sighting count");
        let moved_features: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidate_audio_features WHERE candidate_id = ?1",
                params![winner.id],
                |row| row.get(0),
            )
            .expect("feature count");
        let moved_embeddings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidate_embeddings WHERE candidate_id = ?1",
                params![winner.id],
                |row| row.get(0),
            )
            .expect("embedding count");
        let moved_neighbors: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM external_track_candidate_neighbors WHERE candidate_id = ?1",
                params![winner.id],
                |row| row.get(0),
            )
            .expect("neighbor count");
        assert_eq!(loser_count, 0);
        assert_eq!(moved_sightings, 1);
        assert_eq!(moved_features, 1);
        assert_eq!(moved_embeddings, 1);
        assert_eq!(moved_neighbors, 1);
    }

    #[test]
    fn external_candidates_for_training_skip_expired_and_resolved_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id)
             VALUES (1, 'Resolved Track', 1, 801)",
            [],
        )
        .expect("seed resolved track");
        for (key, title, expires_at, resolved_track_id) in [
            ("fresh", "Fresh", "2026-03-01 00:00:00", None),
            ("expired", "Expired", "2026-01-01 00:00:00", None),
            ("resolved", "Resolved", "2026-03-01 00:00:00", Some(1)),
        ] {
            conn.execute(
                "INSERT INTO external_track_candidates
                 (dedupe_key, title, artist_name, expires_at, resolved_track_id)
                 VALUES (?1, ?2, 'Outside', ?3, ?4)",
                params![key, title, expires_at, resolved_track_id],
            )
            .expect("seed candidate");
        }
        conn.execute(
            "INSERT INTO external_track_candidate_sightings
             (candidate_id, seed_track_id, source, expires_at)
             SELECT id, 1, 'lastfm_similar', '2026-03-01 00:00:00'
             FROM external_track_candidates
             WHERE dedupe_key = 'fresh'",
            [],
        )
        .expect("seed sighting");

        let rows = get_external_track_candidates_for_training(&conn, "2026-02-01 00:00:00", 10)
            .expect("training candidates");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dedupe_key, "fresh");
        assert_eq!(
            rows[0].source_tags_json.as_deref(),
            Some(r#"["lastfm_direct","lastfm_similar"]"#)
        );
    }

    #[test]
    fn external_training_tags_derive_lastfm_branch_from_cached_payload() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id) VALUES (1, 'Seed', 1)",
            [],
        )
        .expect("seed track");
        let candidate = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "branch-candidate".to_string(),
                title: "Branch".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("candidate");
        upsert_external_candidate_sighting(
            &conn,
            &ExternalCandidateSightingUpsert {
                candidate_id: candidate.id,
                seed_track_id: 1,
                source: "lastfm_similar".to_string(),
                source_payload_json: Some(
                    r#"{"match":0.42,"branch_from":"Parent Artist - Parent Track"}"#.to_string(),
                ),
                similarity: Some(0.42),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("sighting");

        let rows = get_external_track_candidates_for_training(&conn, "2026-02-01 00:00:00", 10)
            .expect("training candidates");

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].source_tags_json.as_deref(),
            Some(r#"["lastfm_branch","lastfm_similar"]"#)
        );
    }

    #[test]
    fn resolved_lastfm_sightings_for_training_include_cached_payload() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id)
             VALUES (1, 'Seed', 1), (2, 'Resolved', 1)",
            [],
        )
        .expect("seed tracks");
        let candidate = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: Some(9002),
                mbid: None,
                dedupe_key: "resolved-lastfm".to_string(),
                title: "Resolved".to_string(),
                artist_name: "Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("candidate");
        mark_external_candidate_resolved(&conn, Some(9002), "Resolved", "Artist", 2)
            .expect("mark resolved");
        upsert_external_candidate_sighting(
            &conn,
            &ExternalCandidateSightingUpsert {
                candidate_id: candidate.id,
                seed_track_id: 1,
                source: "lastfm_similar".to_string(),
                source_payload_json: Some(r#"{"match":0.77}"#.to_string()),
                similarity: Some(0.77),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("sighting");

        let rows =
            get_resolved_lastfm_external_sightings_for_training(&conn, "2026-02-01 00:00:00", 10)
                .expect("resolved sightings");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seed_track_id, 1);
        assert_eq!(rows[0].resolved_track_id, 2);
        assert_eq!(rows[0].similarity, 0.77);
        assert_eq!(
            rows[0].source_payload_json.as_deref(),
            Some(r#"{"match":0.77}"#)
        );
    }

    #[test]
    fn external_neighbor_lookup_returns_only_tidal_resolved_candidates_by_default() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id)
             VALUES (1, 'Seed', 1, 901)",
            [],
        )
        .expect("seed track");
        let model = create_embedding_model(
            &conn,
            "discovery-fusion-v2:external-read",
            "discovery-fusion-v2",
            2,
            "ready",
            None,
        )
        .expect("create model");
        let unresolved = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "unresolved-read".to_string(),
                title: "Unresolved".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(100_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("unresolved");
        let resolved = upsert_external_track_candidate(
            &conn,
            &ExternalTrackCandidateUpsert {
                tidal_id: Some(9901),
                mbid: None,
                dedupe_key: "tidal:9901".to_string(),
                title: "Resolved".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(100_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .expect("resolved");
        replace_external_candidate_neighbors(
            &conn,
            model.id,
            1,
            &[
                ExternalCandidateNeighborWriteRow {
                    candidate_id: unresolved.id,
                    rank: 1,
                    score: 0.95,
                    audio_score: 0.95,
                    metadata_score: 0.0,
                    reason_json: None,
                },
                ExternalCandidateNeighborWriteRow {
                    candidate_id: resolved.id,
                    rank: 2,
                    score: 0.9,
                    audio_score: 0.9,
                    metadata_score: 0.0,
                    reason_json: None,
                },
            ],
        )
        .expect("write neighbors");

        let rows = get_external_candidate_neighbors(&conn, model.id, 1, 10, true)
            .expect("read external neighbors");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].candidate_id, resolved.id);
        assert_eq!(rows[0].tidal_id, Some(9901));
    }

    #[test]
    fn tidal_resolution_candidate_lookup_prioritizes_sightings_similarity_and_cap() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, tidal_id)
             VALUES (1, 'Seed A', 1, 901), (2, 'Seed B', 1, 902)",
            [],
        )
        .expect("seed tracks");
        for (key, title) in [
            ("one-sighting", "One"),
            ("two-sightings", "Two"),
            ("resolved", "Resolved"),
            ("already-tidal", "Already Tidal"),
        ] {
            conn.execute(
                "INSERT INTO external_track_candidates
                 (dedupe_key, title, artist_name, expires_at, tidal_id)
                 VALUES (?1, ?2, 'Outside', '2099-01-01 00:00:00',
                         CASE WHEN ?1 = 'already-tidal' THEN 9901 ELSE NULL END)",
                params![key, title],
            )
            .expect("candidate");
        }
        conn.execute(
            "UPDATE external_track_candidates SET resolved_track_id = 1 WHERE dedupe_key = 'resolved'",
            [],
        )
        .expect("mark resolved");
        conn.execute(
            "INSERT INTO external_track_candidate_sightings
             (candidate_id, seed_track_id, source, similarity, expires_at)
             SELECT id, 1, 'lastfm_similar', 0.60, '2099-01-01 00:00:00'
             FROM external_track_candidates WHERE dedupe_key = 'one-sighting'",
            [],
        )
        .expect("one sighting");
        conn.execute(
            "INSERT INTO external_track_candidate_sightings
             (candidate_id, seed_track_id, source, similarity, expires_at)
             SELECT id, 1, 'lastfm_similar', 0.70, '2099-01-01 00:00:00'
             FROM external_track_candidates WHERE dedupe_key = 'two-sightings'",
            [],
        )
        .expect("two sighting a");
        conn.execute(
            "INSERT INTO external_track_candidate_sightings
             (candidate_id, seed_track_id, source, similarity, expires_at)
             SELECT id, 2, 'lastfm_similar', 0.90, '2099-01-01 00:00:00'
             FROM external_track_candidates WHERE dedupe_key = 'two-sightings'",
            [],
        )
        .expect("two sighting b");

        let rows = get_unresolved_lastfm_external_candidates_for_tidal_resolution(
            &conn,
            "2026-02-01 00:00:00",
            1,
        )
        .expect("resolution candidates");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Two");
        assert_eq!(rows[0].sighting_count, 2);
        assert_eq!(rows[0].max_similarity, Some(0.90));
    }

    #[test]
    fn onboarding_flag_present_returns_true_without_tidal() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        set_onboarding_complete(&conn).expect("set flag");

        assert!(get_onboarding_complete(&conn).expect("read flag"));
        assert_eq!(read_onboarding_value(&conn).as_deref(), Some("1"));
    }

    #[test]
    fn discovery_presets_round_trip_mode() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let created = create_discovery_preset(
            &conn,
            "After Hours",
            "glassy synths",
            "reference",
            r#"["tidal","soundcloud"]"#,
        )
        .expect("preset created");

        assert_eq!(created.mode, "reference");

        let presets = list_discovery_presets(&conn).expect("preset list");
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].mode, "reference");
        assert_eq!(presets[0].services, vec!["tidal", "soundcloud"]);
    }

    #[test]
    fn search_matches_artist_names_for_tracks_and_albums() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'The Cure')", [])
            .expect("artist inserted");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, is_favorite, source) VALUES (1, 'Disintegration', 1, 1, 'tidal')",
            [],
        )
        .expect("album inserted");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source
            ) VALUES (1, 'Pictures of You', 1, 1, 420000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
            [],
        )
        .expect("track inserted");

        let results = search(&conn, "the cure", 10).expect("search results");

        assert_eq!(results.artists.len(), 1);
        assert_eq!(results.artists[0].name, "The Cure");
        assert_eq!(results.albums.len(), 1);
        assert_eq!(results.albums[0].title, "Disintegration");
        assert_eq!(results.tracks.len(), 1);
        assert_eq!(results.tracks[0].title, "Pictures of You");
    }

    #[test]
    fn to_fts_query_treats_apostrophe_as_separator() {
        // A bare apostrophe is a string-literal opener in FTS5 and used to throw
        // "fts5: syntax error", forcing the full-table LIKE fallback. It must be
        // mapped to a separator so the query parses.
        assert_eq!(to_fts_query("don't"), "don* t*");
        assert_eq!(to_fts_query("Guns N' Roses"), "Guns* N* Roses*");
        assert!(!to_fts_query("rock 'n' roll").contains('\''));
    }

    #[test]
    fn search_handles_apostrophe_queries_via_fts() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Guns N Roses')",
            [],
        )
        .expect("artist inserted");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source
            ) VALUES (1, 'Don''t Cry', 1, 300000, 201, 'LOSSLESS', 'tidal', 10, 0, 'tidal')",
            [],
        )
        .expect("track inserted");

        // The unicode61 tokenizer splits "Don't" into "don" + "t", so an
        // apostrophe query must still find it. Before the fix this errored and
        // fell back to a full-table LIKE scan.
        let results = search(&conn, "don't", 10).expect("apostrophe search must not error");
        assert!(
            results.tracks.iter().any(|t| t.title == "Don't Cry"),
            "expected to find \"Don't Cry\", got {:?}",
            results.tracks.iter().map(|t| &t.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn genre_heat_rolls_descendant_listens_up_to_ancestors() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Electronic', 'electronic', NULL),
                (2, 'Drum and Bass', 'drum-and-bass', 1)",
            [],
        )
        .expect("genres inserted");
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Rufige Kru')",
            [],
        )
        .expect("artist inserted");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, duration_ms, tidal_id, best_quality, best_source, fidelity_score, is_favorite, source
            ) VALUES (1, 'Terminator', 1, 360000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
            [],
        )
        .expect("track inserted");
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id, source, confidence)
             VALUES (1, 2, 'musicbrainz', 1.0)",
            [],
        )
        .expect("track genre inserted");
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
             VALUES (1, datetime('now', '-10 days'), 120000, 1)",
            [],
        )
        .expect("listen inserted");

        let heat = get_genre_heat_filtered(&conn, 90, crate::genre::filter::GalaxyFilterRule::All)
            .expect("genre heat");
        let electronic = heat
            .iter()
            .find(|entry| entry.genre_id == 1)
            .expect("electronic heat");
        let dnb = heat
            .iter()
            .find(|entry| entry.genre_id == 2)
            .expect("dnb heat");

        assert_eq!(electronic.listen_count, 1);
        assert_eq!(electronic.total_listened_ms, 120000);
        assert_eq!(dnb.listen_count, 1);
        assert_eq!(dnb.total_listened_ms, 120000);
    }

    #[test]
    fn genre_heat_returns_zero_rows_for_cold_genres() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Electronic', 'electronic', NULL),
                (2, 'Ambient', 'ambient', 1)",
            [],
        )
        .expect("genres inserted");

        let heat = get_genre_heat_filtered(&conn, 90, crate::genre::filter::GalaxyFilterRule::All)
            .expect("genre heat");
        assert_eq!(heat.len(), 2);
        assert!(heat.iter().all(|entry| entry.listen_count == 0));
        assert!(heat.iter().all(|entry| entry.total_listened_ms == 0));
    }

    #[test]
    fn test_add_tracks_to_playlist_deduplicates() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute_batch(
            r#"
            INSERT INTO artists (id, name) VALUES (1, 'Test Artist');
            INSERT INTO albums (id, title, artist_id) VALUES (1, 'Test Album', 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (1, 'Track A', 1, 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (2, 'Track B', 1, 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (3, 'Track C', 1, 1);
            INSERT INTO playlists (id, name, is_smart, is_synced) VALUES (1, 'My Playlist', 0, 1);
        "#,
        )
        .unwrap();

        // First call adds both tracks
        let added = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
        assert_eq!(added, 2);

        // Second call with same tracks returns 0 (already present)
        let added_again = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
        assert_eq!(added_again, 0);

        // Duplicate IDs within a single call: [1, 1] — track 1 already present, so 0 added
        let added_dup = add_tracks_to_playlist(&conn, 1, &[1, 1]).unwrap();
        assert_eq!(added_dup, 0);

        // Mixed: [1, 3] — track 1 already present, track 3 is new → 1 added
        let added_mixed = add_tracks_to_playlist(&conn, 1, &[1, 3]).unwrap();
        assert_eq!(added_mixed, 1);

        // [3, 3] — track 3 now present, duplicate in input → 0 added
        let added_dup_present = add_tracks_to_playlist(&conn, 1, &[3, 3]).unwrap();
        assert_eq!(added_dup_present, 0);
    }

    // ─── liked_only vs favorite_only regression ───────────────────────────
    //
    // Bug being guarded: favorite_only=true used to silently mean "library tracks"
    // (liked tracks ∪ tracks from favorited albums), so saved-album tracks leaked
    // into what the UI presented as "liked". liked_only must be strict.
    fn seed_album_with_one_liked_track(conn: &Connection) {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Brooks & Dunn')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, is_favorite, source)
             VALUES (1, '#1s ... and then some', 1, 1, 'tidal')",
            [],
        )
        .unwrap();
        // Three tracks in the favorited album; only "Neon Blue" has tracks.is_favorite = 1.
        // All three are is_library = 1: this is a genuine TIDAL favorited-album
        // sync (insert_tidal_track marks every synced track as library), so the
        // two un-liked siblings must still count as library tracks.
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source, is_library)
             VALUES (1, 'Neon Blue', 1, 1, 200000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source, is_library)
             VALUES (2, 'Brand New Man', 1, 1, 180000, 102, 'LOSSLESS', 'tidal', 10, 0, 'tidal', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source, is_library)
             VALUES (3, 'Boot Scootin Boogie', 1, 1, 198000, 103, 'LOSSLESS', 'tidal', 10, 0, 'tidal', 1)",
            [],
        ).unwrap();
    }

    #[test]
    fn liked_only_excludes_album_favorited_tracks() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks = get_tracks(&conn, "title", "asc", 100, 0, false, true).expect("liked tracks");
        assert_eq!(
            tracks.len(),
            1,
            "liked_only must return only truly-liked tracks"
        );
        assert_eq!(tracks[0].title, "Neon Blue");

        let count = get_track_count(&conn, false, true).expect("liked count");
        assert_eq!(count, 1, "count must match liked-only data query");
    }

    #[test]
    fn favorite_only_preserves_legacy_union_behavior() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks =
            get_tracks(&conn, "title", "asc", 100, 0, true, false).expect("library tracks");
        assert_eq!(
            tracks.len(),
            3,
            "favorite_only must keep returning all tracks from favorited albums"
        );

        let count = get_track_count(&conn, true, false).expect("library count");
        assert_eq!(count, 3, "count must match favorite_only data query");
    }

    // Regression for the resolver/discovery leak: a transient import
    // (is_library = 0) that attaches to a favorited album by tidal_id must NOT
    // surface in the library, while genuine is_library = 1 siblings still do.
    #[test]
    fn favorite_only_hides_transient_import_in_favorited_album() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        // A discovery/resolver track injected into the same favorited album:
        // not liked, not library (the "House Work" shape).
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source, is_library)
             VALUES (4, 'Injected Leak', 1, 1, 157000, 104, 'LOSSLESS', 'tidal', 10, 0, 'tidal', 0)",
            [],
        )
        .unwrap();
        // A resolver lazy-import (tidal_stream) that also landed here, unliked.
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source, is_library)
             VALUES (5, 'Stream Leak', 1, 1, 160000, 105, 'LOSSLESS', 'tidal', 10, 0, 'tidal_stream', 0)",
            [],
        )
        .unwrap();

        let tracks =
            get_tracks(&conn, "title", "asc", 100, 0, true, false).expect("library tracks");
        let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert!(
            !titles.contains(&"Injected Leak") && !titles.contains(&"Stream Leak"),
            "transient is_library=0 tracks must stay out of the library, got {titles:?}"
        );
        assert_eq!(tracks.len(), 3, "only the 3 genuine library tracks remain");

        let count = get_track_count(&conn, true, false).expect("library count");
        assert_eq!(count, 3, "count must exclude the transient imports");

        // An explicit like on the injected track promotes it back in.
        conn.execute(
            "UPDATE tracks SET is_favorite = 1, is_library = 1 WHERE id = 4",
            [],
        )
        .unwrap();
        let after = get_track_count(&conn, true, false).expect("library count");
        assert_eq!(
            after, 4,
            "explicit like promotes a transient track to library"
        );
    }

    #[test]
    fn date_added_desc_uses_newest_row_as_tiebreaker() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Artist')", [])
            .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source, date_added)
             VALUES (1, 'First', 1, 200000, 201, 'LOSSLESS', 'tidal', 10, 1, 'tidal', '2026-05-01T00:00:00Z'),
                    (2, 'Second', 1, 200000, 202, 'LOSSLESS', 'tidal', 10, 1, 'tidal', '2026-05-01T00:00:00Z'),
                    (3, 'Third', 1, 200000, 203, 'LOSSLESS', 'tidal', 10, 1, 'tidal', '2026-05-01T00:00:00Z')",
            [],
        )
        .expect("seed tracks");

        let tracks = get_tracks(&conn, "date_added", "desc", 100, 0, true, false)
            .expect("date sorted tracks");

        assert_eq!(
            tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn date_added_order_clause_has_explicit_id_tiebreaker() {
        assert_eq!(
            track_order_clause("date_added", "desc"),
            "t.date_added DESC, t.id DESC"
        );
        assert_eq!(
            track_order_clause("date_added", "asc"),
            "t.date_added ASC, t.id ASC"
        );
    }

    #[test]
    fn random_order_clause_ignores_direction() {
        // Shuffle relies on this: the library Shuffle button sends sort_by=random
        // so the queue is a fresh random slice of the whole library, not the
        // newest-N prefix reshuffled.
        assert_eq!(track_order_clause("random", "desc"), "RANDOM()");
        assert_eq!(track_order_clause("random", "asc"), "RANDOM()");
    }

    #[test]
    fn artist_library_tracks_and_counts_use_library_union_behavior() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, is_favorite, source)
             VALUES (2, 'Not Saved', 1, 0, 'tidal')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source)
             VALUES (4, 'Cache Only', 1, 2, 199000, 104, 'LOSSLESS', 'tidal', 10, 0, 'tidal')",
            [],
        )
        .unwrap();

        let all_tracks = get_artist_tracks(&conn, 1).expect("all artist tracks");
        assert_eq!(all_tracks.len(), 4);

        let library_tracks = get_artist_library_tracks(&conn, 1).expect("library artist tracks");
        assert_eq!(library_tracks.len(), 3);

        let (_, track_count, album_count) = get_artist_with_counts(&conn, 1)
            .expect("artist counts")
            .expect("artist exists");
        assert_eq!(track_count, 3);
        assert_eq!(album_count, 1);
    }

    #[test]
    fn liked_only_takes_precedence_over_favorite_only() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks = get_tracks(&conn, "title", "asc", 100, 0, true, true).expect("strict tracks");
        assert_eq!(tracks.len(), 1, "liked_only must override favorite_only");
        assert_eq!(tracks[0].title, "Neon Blue");

        let count = get_track_count(&conn, true, true).expect("strict count");
        assert_eq!(count, 1);
    }

    #[test]
    fn tidal_track_library_states_include_liked_flags() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let states = get_tidal_track_library_states(&conn, &[101, 102, 999]).expect("tidal states");

        assert_eq!(states.len(), 2);
        assert_eq!(states.get(&101).map(|s| s.local_id), Some(1));
        assert_eq!(states.get(&101).map(|s| s.is_favorite), Some(true));
        assert_eq!(states.get(&102).map(|s| s.local_id), Some(2));
        assert_eq!(states.get(&102).map(|s| s.is_favorite), Some(false));
        assert!(!states.contains_key(&999));
    }

    #[test]
    fn no_filter_returns_everything() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks = get_tracks(&conn, "title", "asc", 100, 0, false, false).expect("all tracks");
        assert_eq!(tracks.len(), 3);

        let count = get_track_count(&conn, false, false).expect("all count");
        assert_eq!(count, 3);
    }

    // ─── FTS-first library search tests ──────────────────────────────────────

    #[test]
    fn library_search_multi_token_and_within_column_non_contiguous() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // FTS5 AND-prefix semantics: tokens must all appear in the same indexed
        // column, but NOT necessarily contiguously and NOT necessarily in order.
        //   1001 — title "The Long Strokes": both tokens present, non-contiguous.
        //          (Today's LIKE on "the strokes" would MISS this — substring fail.)
        //   1002 — title "The Anthem": only "the". Missing "strokes". Should NOT match.
        //   1003 — title "Strokes": only "strokes". Missing "the". Should NOT match.
        conn.execute("INSERT INTO artists (id, name) VALUES (1001, 'Test')", [])
            .expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (1001, 'Plain', 1001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (1001, 'The Long Strokes', 1001, 1001, 200000, 1001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (1002, 'The Anthem',       1001, 1001, 200000, 1002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (1003, 'Strokes',          1001, 1001, 200000, 1003, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");

        let results =
            search_with_audio_filters(&conn, "the strokes", &AudioFilters::default(), 50, 0)
                .expect("library search");

        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![1001],
            "expected only 1001 (both tokens in title, non-contiguous); got {ids:?}"
        );
    }

    #[test]
    fn library_search_returns_track_when_album_title_matches() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (2001, 'Frank Ocean')",
            [],
        )
        .expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (2001, 'Blonde', 2001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES (2001, 'Pink + White', 2001, 2001, 200000, 2001, 'LOSSLESS', 'tidal', 8, 0, 'tidal', 0)",
            [],
        )
        .expect("track");

        let results = search_with_audio_filters(&conn, "blonde", &AudioFilters::default(), 50, 0)
            .expect("search");
        let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
        assert!(
            titles.contains(&"Pink + White"),
            "expected 'Pink + White' (album 'Blonde' matches); got {titles:?}"
        );
    }

    #[test]
    fn library_search_audio_filter_composes_with_text() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (3001, 'Miles Davis')",
            [],
        )
        .expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (3001, 'Kind of Blue', 3001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (3001, 'So What (fast)', 3001, 3001, 540000, 3001, 'LOSSLESS', 'tidal', 9, 0, 'tidal', 0),
                (3002, 'Blue in Green',  3001, 3001, 330000, 3002, 'LOSSLESS', 'tidal', 9, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (3001, 120.0), (3002, 80.0)",
            [],
        )
        .expect("dsp features");

        let filters = AudioFilters {
            bpm_min: Some(100.0),
            ..Default::default()
        };

        let results = search_with_audio_filters(&conn, "miles", &filters, 50, 0).expect("search");
        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![3001],
            "expected only the 120-BPM track; got {ids:?}"
        );
    }

    #[test]
    fn library_search_empty_query_with_filters_returns_filtered_set() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (4001, 'Test')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (4001, 'A', 4001, 'tidal')",
            [],
        )
        .expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (4001, 'Fast', 4001, 4001, 200000, 4001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (4002, 'Slow', 4001, 4001, 200000, 4002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (4001, 130.0), (4002, 70.0)",
            [],
        )
        .expect("dsp");

        let filters = AudioFilters {
            bpm_min: Some(120.0),
            ..Default::default()
        };

        let results = search_with_audio_filters(&conn, "", &filters, 50, 0).expect("search");
        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![4001]);
    }

    #[test]
    fn shuffled_audio_search_covers_full_matching_set() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (4101, 'Test')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (4101, 'A', 4101, 'tidal')",
            [],
        )
        .expect("album");
        // 8 matching tracks with varied play_count so the deterministic ranking
        // would order them; shuffle must still surface every matching id.
        for i in 0..8i64 {
            let id = 4101 + i;
            conn.execute(
                &format!(
                    "INSERT INTO tracks (
                        id, title, artist_id, album_id, duration_ms, tidal_id, best_quality,
                        best_source, fidelity_score, is_favorite, source, play_count
                     ) VALUES ({id}, 'T{i}', 4101, 4101, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', {i})"
                ),
                [],
            )
            .expect("track");
            conn.execute(
                &format!("INSERT INTO audio_dsp_features (track_id, bpm) VALUES ({id}, 130.0)"),
                [],
            )
            .expect("dsp");
        }

        let filters = AudioFilters {
            bpm_min: Some(120.0),
            ..Default::default()
        };

        let results =
            search_with_audio_filters_shuffled(&conn, "", &filters, 200).expect("shuffle search");
        let mut ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, (4101..4109).collect::<Vec<_>>());
    }

    #[test]
    fn shuffled_audio_search_respects_filters_and_limit() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (4201, 'Test')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (4201, 'A', 4201, 'tidal')",
            [],
        )
        .expect("album");
        // Two fast (matching) tracks, two slow (non-matching).
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (4201, 'Fast A', 4201, 4201, 200000, 4201, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (4202, 'Fast B', 4201, 4201, 200000, 4202, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (4203, 'Slow A', 4201, 4201, 200000, 4203, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (4204, 'Slow B', 4201, 4201, 200000, 4204, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES
                (4201, 140.0), (4202, 135.0), (4203, 70.0), (4204, 60.0)",
            [],
        )
        .expect("dsp");

        let filters = AudioFilters {
            bpm_min: Some(120.0),
            ..Default::default()
        };

        let results =
            search_with_audio_filters_shuffled(&conn, "", &filters, 1).expect("shuffle search");
        assert_eq!(results.len(), 1, "limit must cap the shuffled sample");
        assert!(
            matches!(results[0].id, 4201 | 4202),
            "only the fast tracks should match, got {}",
            results[0].id
        );
    }

    #[test]
    fn liked_only_audio_search_restricts_to_favorites() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (4301, 'Test')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (4301, 'A', 4301, 'tidal')",
            [],
        )
        .expect("album");
        // Two liked, two not liked; all match the filter.
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (4301, 'Liked A',   4301, 4301, 200000, 4301, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 0),
                (4302, 'Liked B',   4301, 4301, 200000, 4302, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 0),
                (4303, 'Unliked A', 4301, 4301, 200000, 4303, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (4304, 'Unliked B', 4301, 4301, 200000, 4304, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");

        let filters = AudioFilters {
            liked_only: true,
            ..Default::default()
        };

        let mut ids: Vec<i64> = search_with_audio_filters_shuffled(&conn, "", &filters, 200)
            .expect("shuffle search")
            .iter()
            .map(|r| r.id)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![4301, 4302], "liked_only must drop non-favorites");
    }

    #[test]
    fn library_search_empty_query_no_filters_respects_limit() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (5001, 'A')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (5001, 'A', 5001, 'tidal')",
            [],
        )
        .expect("album");
        for i in 0..5 {
            let id = 5001 + i;
            conn.execute(
                &format!(
                    "INSERT INTO tracks (
                        id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                        fidelity_score, is_favorite, source, play_count
                     ) VALUES ({id}, 'T{i}', 5001, 5001, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
                ),
                [],
            )
            .expect("track");
        }

        let results =
            search_with_audio_filters(&conn, "", &AudioFilters::default(), 3, 0).expect("search");
        assert_eq!(results.len(), 3, "expected limit=3 to cap results");
    }

    #[test]
    fn library_search_favorites_lead_over_play_count() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (6001, 'Miles Davis')",
            [],
        )
        .expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (6001, 'Kind of Blue', 6001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (6001, 'Miles A', 6001, 6001, 200000, 6001, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 0),
                (6002, 'Miles B', 6001, 6001, 200000, 6002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 1000)",
            [],
        )
        .expect("tracks");

        // Old ordering (play_count DESC, last_played_at DESC) → B leads.
        // New ordering (is_favorite DESC, ...) → A leads.
        let results = search_with_audio_filters(&conn, "miles", &AudioFilters::default(), 50, 0)
            .expect("search");
        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(
            ids.first(),
            Some(&6001),
            "favorited track should lead despite zero plays; got {ids:?}"
        );
    }

    #[test]
    fn library_search_non_track_track_type_returns_empty() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (7001, 'Anyone')", [])
            .expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (7001, 'Anything', 7001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES (7001, 'Anything', 7001, 7001, 200000, 7001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("track");

        let filters = AudioFilters {
            track_type: Some("album".to_string()),
            ..Default::default()
        };

        let results =
            search_with_audio_filters(&conn, "anything", &filters, 50, 0).expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn library_search_strips_fts_special_characters() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // Punctuation in user queries (?, /, -) must not cause FTS to error.
        // to_fts_query strips non-alphanumerics; tokenization happens within a
        // single column, so fixtures keep all match tokens together in one column.
        conn.execute("INSERT INTO artists (id, name) VALUES (8001, 'AC/DC')", [])
            .expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (8001, 'AC/DC Live', 8001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (8001, 'Thunderstruck', 8001, 8001, 200000, 8001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (8002, 'Love Remix',    8001, 8001, 200000, 8002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");

        // Query "AC/DC live?" → strips to "AC DC live" → tokens AC, DC, live must
        // all appear in some indexed column. Album "AC/DC Live" tokenizes to
        // ["ac", "dc", "live"] (unicode61 splits on /), satisfying all three.
        let r1 = search_with_audio_filters(&conn, "AC/DC live?", &AudioFilters::default(), 50, 0)
            .expect("'AC/DC live?' must not error");
        assert!(
            r1.iter().any(|r| r.id == 8001),
            "expected Thunderstruck (album 'AC/DC Live' has all tokens); got ids {:?}",
            r1.iter().map(|r| r.id).collect::<Vec<_>>()
        );

        // Query "love - remix" → "love remix" → tokens must both appear in same
        // column. Track 8002's title "Love Remix" satisfies that.
        let r2 = search_with_audio_filters(&conn, "love - remix", &AudioFilters::default(), 50, 0)
            .expect("'love - remix' must not error");
        assert!(
            r2.iter().any(|r| r.id == 8002),
            "expected 'Love Remix' to match; got ids {:?}",
            r2.iter().map(|r| r.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn global_search_tracks_fts_does_not_error_on_artist_match() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // Setup designed to exercise the artists_fts UNION arm: track title
        // contains nothing of the query, but the artist name does.
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'The Cure')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (1, 'Disintegration', 1, 'tidal')",
            [],
        )
        .expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES (1, 'Pictures of You', 1, 1, 420000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal', 0)",
            [],
        )
        .expect("track");

        // Calls search_tracks_fts directly so the LIKE fallback in search() can't
        // mask an FTS-side error. If the UNION+ORDER-BY SQL is broken, this errors.
        let tracks = search_tracks_fts(&conn, "the* cure*", 10)
            .expect("search_tracks_fts must run without SQL errors");
        let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert!(
            titles.contains(&"Pictures of You"),
            "FTS path should return the track via artists_fts arm; got {titles:?}"
        );
    }

    #[test]
    fn library_search_limit_is_respected_with_filters() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (9101, 'Miles Davis')",
            [],
        )
        .expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (9101, 'Kind of Blue', 9101, 'tidal')", []).expect("album");
        for i in 0..3 {
            let id = 9101 + i;
            conn.execute(
                &format!(
                    "INSERT INTO tracks (
                        id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                        fidelity_score, is_favorite, source, play_count
                     ) VALUES ({id}, 'Miles {i}', 9101, 9101, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
                ),
                [],
            )
            .expect("track");
        }
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (9101, 120.0), (9102, 121.0), (9103, 122.0)",
            [],
        )
        .expect("dsp");

        let filters = AudioFilters {
            bpm_min: Some(100.0),
            ..Default::default()
        };

        let results = search_with_audio_filters(&conn, "miles", &filters, 2, 0).expect("search");
        assert_eq!(
            results.len(),
            2,
            "limit=2 with both FTS bind and audio-filter binds; off-by-one would return 0 or 3"
        );
    }

    #[test]
    fn audio_search_offset_pages_and_count_reports_full_set() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (9201, 'Pager')", [])
            .expect("artist");
        for i in 0..5i64 {
            let id = 9201 + i;
            conn.execute(
                &format!(
                    "INSERT INTO tracks (
                        id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
                        fidelity_score, is_favorite, source, play_count
                     ) VALUES ({id}, 'P{i}', 9201, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', {pc})",
                    pc = 100 - i
                ),
                [],
            )
            .expect("track");
        }

        let filters = AudioFilters::default();
        let total = count_audio_filter_matches(&conn, "", &filters).expect("count");
        assert_eq!(total, 5);

        let page1 = search_with_audio_filters(&conn, "", &filters, 2, 0).expect("page1");
        let page2 = search_with_audio_filters(&conn, "", &filters, 2, 2).expect("page2");
        let page3 = search_with_audio_filters(&conn, "", &filters, 2, 4).expect("page3");
        let ids: Vec<i64> = page1
            .iter()
            .chain(page2.iter())
            .chain(page3.iter())
            .map(|r| r.id)
            .collect();
        // play_count DESC ranking: 9201 (100) .. 9205 (96); pages must not
        // overlap or skip.
        assert_eq!(ids, vec![9201, 9202, 9203, 9204, 9205]);
    }

    #[test]
    fn resolve_genre_tokens_matches_slug_and_name_case_insensitive() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Rock', 'rock', NULL),
                (2, 'Hip Hop', 'hip-hop', NULL);",
        )
        .expect("genres");

        let tokens = vec![
            "Rock".to_string(),
            "hip-hop".to_string(),
            "HIP HOP".to_string(),
            "polka".to_string(),
        ];
        let (ids, unmatched) = resolve_genre_tokens(&conn, &tokens).expect("resolve");
        assert_eq!(ids, vec![1, 2, 2]);
        assert_eq!(unmatched, vec!["polka".to_string()]);
    }

    #[test]
    fn expand_genre_descendants_walks_full_subtree() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Rock', 'rock', NULL),
                (2, 'Alternative Rock', 'alternative-rock', 1),
                (3, 'Indie Rock', 'indie-rock', 2),
                (4, 'Jazz', 'jazz', NULL);",
        )
        .expect("genres");

        let mut expanded = expand_genre_descendants(&conn, &[1]).expect("expand");
        expanded.sort_unstable();
        assert_eq!(expanded, vec![1, 2, 3], "grandchildren must be included");

        // Unknown ids drop out instead of leaking into the SQL filter.
        assert!(
            expand_genre_descendants(&conn, &[999])
                .expect("expand")
                .is_empty()
        );
        assert!(
            expand_genre_descendants(&conn, &[])
                .expect("expand")
                .is_empty()
        );
    }

    #[test]
    fn genre_filter_uses_curated_rowset_not_raw_tags() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute_batch(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Rock', 'rock', NULL),
                (2, 'Indie Rock', 'indie-rock', 1),
                (3, 'Psytrance', 'psytrance', NULL);
             INSERT INTO artists (id, name) VALUES (9301, 'G');",
        )
        .expect("seed");
        for i in 0..4i64 {
            let id = 9301 + i;
            conn.execute(
                &format!(
                    "INSERT INTO tracks (
                        id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
                        fidelity_score, is_favorite, source, play_count
                     ) VALUES ({id}, 'G{i}', 9301, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
                ),
                [],
            )
            .expect("track");
        }
        conn.execute_batch(
            "INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
                (9301, 1, 'musicbrainz', 0.8),  -- solid rock tag: matches
                (9302, 2, 'musicbrainz', 0.8),  -- indie rock (descendant): matches
                (9303, 1, 'lastfm', 0.3),        -- weak-only tag: rescued, matches
                (9304, 3, 'musicbrainz', 0.9),  -- psytrance track...
                (9304, 1, 'lastfm', 0.29);       -- ...with junk rock tag: must NOT match",
        )
        .expect("tags");

        // Route-equivalent flow: resolve token, expand descendants, filter.
        let (ids, unmatched) = resolve_genre_tokens(&conn, &["rock".to_string()]).expect("resolve");
        assert!(unmatched.is_empty());
        let expanded = expand_genre_descendants(&conn, &ids).expect("expand");

        let filters = AudioFilters {
            genre_ids: expanded,
            ..Default::default()
        };
        let mut result_ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
            .expect("search")
            .iter()
            .map(|r| r.id)
            .collect();
        result_ids.sort_unstable();
        assert_eq!(
            result_ids,
            vec![9301, 9302, 9303],
            "curated rowset must admit strong + descendant + rescued tags and drop junk"
        );

        let total = count_audio_filter_matches(&conn, "", &filters).expect("count");
        assert_eq!(total, 3, "count must apply the same genre rowset");
    }

    #[test]
    fn normalize_key_signature_canonicalizes_user_input() {
        assert_eq!(normalize_key_signature("Am"), "Am");
        assert_eq!(normalize_key_signature("am"), "Am");
        assert_eq!(normalize_key_signature("A"), "Amaj");
        assert_eq!(normalize_key_signature("a major"), "Amaj");
        assert_eq!(normalize_key_signature("Bb"), "A#maj");
        assert_eq!(normalize_key_signature("bbm"), "A#m");
        assert_eq!(normalize_key_signature("f# minor"), "F#m");
        assert_eq!(normalize_key_signature("Cb"), "Bmaj");
        // Unrecognized input passes through for the NOCASE match to try.
        assert_eq!(normalize_key_signature("8A"), "8A");
        assert_eq!(normalize_key_signature("Axyz"), "Axyz");
    }

    #[test]
    fn key_filter_matches_case_insensitively_with_enharmonics() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (9401, 'K')", [])
            .expect("artist");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (9401, 'MinorTrack', 9401, 200000, 9401, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (9402, 'SharpTrack', 9401, 200000, 9402, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, key_signature, camelot_key) VALUES
                (9401, 'Am', '8A'), (9402, 'A#maj', '6B')",
            [],
        )
        .expect("dsp");

        let by_key = |key: &str| -> Vec<i64> {
            let filters = AudioFilters {
                key_signature: Some(key.to_string()),
                ..Default::default()
            };
            search_with_audio_filters(&conn, "", &filters, 50, 0)
                .expect("search")
                .iter()
                .map(|r| r.id)
                .collect()
        };
        assert_eq!(by_key("am"), vec![9401], "lowercase minor");
        assert_eq!(by_key("Bb"), vec![9402], "flat spelling of A#maj");

        let filters = AudioFilters {
            camelot_key: Some("8a".to_string()),
            ..Default::default()
        };
        let ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
            .expect("search")
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![9401], "camelot NOCASE");
    }

    #[test]
    fn artist_and_album_contains_filters_match_substrings() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (9501, 'Radiohead'), (9502, 'Portishead');
             INSERT INTO albums (id, title, artist_id, source) VALUES
                (9501, 'OK Computer', 9501, 'tidal'),
                (9502, 'Dummy', 9502, 'tidal');
             INSERT INTO tracks
                (id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                 fidelity_score, is_favorite, source, play_count)
             VALUES
                (9501, 'Airbag', 9501, 9501, 200000, 9501, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (9502, 'Roads', 9502, 9502, 200000, 9502, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0);",
        )
        .expect("seed");

        let filters = AudioFilters {
            artist_contains: Some("RADIO".to_string()),
            ..Default::default()
        };
        let ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
            .expect("search")
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![9501]);

        let filters = AudioFilters {
            album_contains: Some("dummy".to_string()),
            ..Default::default()
        };
        let ids: Vec<i64> = search_with_audio_filters(&conn, "", &filters, 50, 0)
            .expect("search")
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![9502]);
    }

    /// End-to-end Path A read API: cascade joins through `genre_paths` and
    /// returns ancestor-expanded paths with provenance. Mirrors the Path B
    /// fixture (in genre/filter.rs) — same artist/album/comp shape.
    #[test]
    fn get_genres_for_tracks_with_fallback_returns_provenance() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // Genre tree: Electronic > Drum and Bass; Jazz; Rock.
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Electronic', 'electronic', NULL),
                (2, 'Drum and Bass', 'drum-and-bass', 1),
                (3, 'Jazz', 'jazz', NULL),
                (4, 'Rock', 'rock', NULL);
             INSERT INTO artists (id, name) VALUES
                (1, 'CoherentArtist'),
                (2, 'PartiallyTaggedArtist'),
                (3, 'CompContributorA'),
                (4, 'CompContributorB');
             INSERT INTO albums (id, title, artist_id, source) VALUES
                (10, 'CoherentAlbum', 1, 'tidal'),
                (20, 'PartialAlbumA', 2, 'tidal'),
                (21, 'PartialAlbumB', 2, 'tidal'),
                (30, 'MultiArtistComp', 3, 'tidal');
             INSERT INTO tracks
                (id, title, artist_id, album_id, duration_ms, best_quality, best_source, fidelity_score, source)
             VALUES
                (100, 'Tagged on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (101, 'Empty on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (200, 'Tagged on partial A', 2, 20, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (201, 'Empty on partial B', 2, 21, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (300, 'Tagged on comp (artist 3)', 3, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (301, 'Empty on comp (artist 4)', 4, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal');
             INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
                (100, 2, 'musicbrainz', 1.0),
                (200, 3, 'musicbrainz', 0.9),
                (300, 4, 'lastfm', 0.7);",
        )
        .expect("seed fixtures");

        let result =
            get_genres_for_tracks_with_fallback(&conn, &[100, 101, 201, 301]).expect("query");

        // Track 100: direct genre (Drum and Bass), ancestor-expanded into two paths
        // ("Electronic" via the parent walk and "Electronic > Drum and Bass" via the leaf).
        let r100 = result.get(&100).expect("track 100 present");
        assert!(r100.iter().all(|g| g.source == GenreSource::Track));
        assert!(
            r100.iter().any(|g| g.path == "Electronic > Drum and Bass"),
            "expected leaf path for direct genre, got {r100:?}"
        );

        // Track 101: empty, rescued from album sibling (track 100, genre 2).
        let r101 = result.get(&101).expect("track 101 present");
        assert!(r101.iter().all(|g| g.source == GenreSource::AlbumFallback));
        assert!(r101.iter().any(|g| g.path == "Electronic > Drum and Bass"));

        // Track 201: empty, no album sibling, rescued from artist (track 200, genre 3 = Jazz).
        let r201 = result.get(&201).expect("track 201 present");
        assert!(r201.iter().all(|g| g.source == GenreSource::ArtistFallback));
        assert!(r201.iter().any(|g| g.path == "Jazz"));

        // Track 301: empty on multi-artist comp; album tier MUST skip;
        // artist 4 has no other tagged tracks. Track stays unrescued, so it
        // should be absent from the returned map (per existing function's
        // contract: tracks with no genres are absent rather than empty Vec).
        assert!(
            !result.contains_key(&301),
            "track 301 must NOT inherit comp-mate genres; got {:?}",
            result.get(&301)
        );
    }

    #[test]
    fn paths_only_drops_provenance() {
        let rows = vec![
            ResolvedGenre {
                path: "Electronic > House".to_string(),
                source: GenreSource::Track,
            },
            ResolvedGenre {
                path: "Pop".to_string(),
                source: GenreSource::AlbumFallback,
            },
        ];
        let paths = ResolvedGenre::paths_only(&rows);
        assert_eq!(paths, vec!["Electronic > House", "Pop"]);
    }

    /// Seed one track with a distinct value in every projected column, so a
    /// row-shape test can catch any column drift in `track_projection` /
    /// `track_from_row`.
    fn seed_fully_populated_track(conn: &Connection) {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (7, 'Projection Artist')",
            [],
        )
        .expect("seed artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source, artwork_url)
             VALUES (3, 'Projection Album', 7, 'tidal', 'http://art/proj.jpg')",
            [],
        )
        .expect("seed album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, disc_number, track_number,
                duration_ms, isrc, tidal_id, ytmusic_id, soundcloud_id,
                best_quality, best_source, fidelity_score, is_favorite,
                play_count, last_played_at, date_added, source
             ) VALUES (
                42, 'Projection Track', 7, 3, 2, 11,
                234567, 'ISRCPROJ01', 99887, 'ytproj', 55443,
                'HI_RES', 'tidal', 88, 1,
                17, '2026-05-01T12:00:00Z', '2026-04-01T00:00:00Z', 'tidal'
             )",
            [],
        )
        .expect("seed track");
    }

    /// Every field on `Track` must round-trip through the shared projection.
    /// `assert_track_is_fully_populated_seed` is reused by the two query-path
    /// tests below that previously had no direct row-shape coverage.
    fn assert_track_is_fully_populated_seed(track: &Track) {
        assert_eq!(track.id, 42);
        assert_eq!(track.title, "Projection Track");
        assert_eq!(track.artist_id, 7);
        assert_eq!(track.artist_name.as_deref(), Some("Projection Artist"));
        assert_eq!(track.album_id, Some(3));
        assert_eq!(track.album_title.as_deref(), Some("Projection Album"));
        assert_eq!(track.disc_number, Some(2));
        assert_eq!(track.track_number, Some(11));
        assert_eq!(track.duration_ms, Some(234567));
        assert_eq!(track.isrc.as_deref(), Some("ISRCPROJ01"));
        assert_eq!(track.tidal_id, Some(99887));
        assert_eq!(track.ytmusic_id.as_deref(), Some("ytproj"));
        assert_eq!(track.soundcloud_id, Some(55443));
        assert_eq!(track.best_quality.as_deref(), Some("HI_RES"));
        assert_eq!(track.best_source.as_deref(), Some("tidal"));
        assert_eq!(track.fidelity_score, 88);
        assert!(track.is_favorite);
        assert_eq!(track.play_count, 17);
        assert_eq!(
            track.last_played_at.as_deref(),
            Some("2026-05-01T12:00:00Z")
        );
        assert_eq!(track.date_added.as_deref(), Some("2026-04-01T00:00:00Z"));
        assert_eq!(track.source, "tidal");
        assert_eq!(track.artwork_url.as_deref(), Some("http://art/proj.jpg"));
    }

    #[test]
    fn get_discovery_candidate_tracks_maps_every_projected_column() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_fully_populated_track(&conn);

        let tracks = get_discovery_candidate_tracks(&conn, 10).expect("candidates");
        assert_eq!(tracks.len(), 1);
        assert_track_is_fully_populated_seed(&tracks[0]);
    }

    #[test]
    fn get_tracks_excluding_with_limit_maps_every_projected_column() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_fully_populated_track(&conn);

        // Empty exclusion list + a generous cap returns the seed track.
        let tracks = get_tracks_excluding_with_limit(&conn, &[], 50).expect("candidates");
        assert_eq!(tracks.len(), 1);
        assert_track_is_fully_populated_seed(&tracks[0]);

        // And the exclusion path still filters correctly.
        let excluded = get_tracks_excluding_with_limit(&conn, &[42], 50).expect("excluded");
        assert!(excluded.is_empty(), "id 42 must be excluded");
    }
}

/// Load enough metadata about a library track to seed external Tidal discovery.
/// Returns None if the track id isn't found.
///
/// `provider_track_id` is set from `tracks.tidal_id` if available; otherwise the
/// library `id` is used as a string. `normalized_genres` is the top 5 genres
/// for the track ordered by descending confidence.
pub fn load_external_seed_from_track(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<DiscoveryCandidateSeed>> {
    let row = conn.query_row(
        "SELECT t.id, t.tidal_id, t.title, ar.name, al.title
         FROM tracks t
         LEFT JOIN artists ar ON t.artist_id = ar.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    );

    let (id, tidal_id, title, artist_name, album_title) = match row {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let mut stmt = conn.prepare(
        "SELECT g.name
         FROM track_genres tg
         JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id = ?1
         ORDER BY COALESCE(tg.confidence, 0) DESC
         LIMIT 5",
    )?;
    let genres: Vec<String> = stmt
        .query_map(params![track_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(DiscoveryCandidateSeed {
        provider_track_id: tidal_id
            .map(|t| t.to_string())
            .unwrap_or_else(|| id.to_string()),
        title,
        artist_name,
        album_title,
        normalized_genres: genres,
    }))
}

// ─── Audio Feature Search ─────────────────────────────────

#[derive(Debug, Default)]
pub struct AudioFilters {
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub energy_min: Option<f64>,
    pub energy_max: Option<f64>,
    pub danceability_min: Option<f64>,
    pub danceability_max: Option<f64>,
    pub key_signature: Option<String>, // exact match
    pub camelot_key: Option<String>,   // exact match
    pub year_min: Option<i64>,
    pub year_max: Option<i64>,
    pub genre_ids: Vec<i64>,             // track must belong to at least one
    pub track_type: Option<String>,      // placeholder, always "track"
    pub is_instrumental: Option<bool>,   // true → vocal:false filter
    pub liked_only: bool,                // restrict to user-liked tracks (Liked tab)
    pub artist_contains: Option<String>, // substring match on artist name
    pub album_contains: Option<String>,  // substring match on album title
}

#[derive(Debug, Serialize)]
pub struct AudioSearchResult {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub danceability: Option<f64>,
    pub key_signature: Option<String>,
    pub camelot_key: Option<String>,
    pub play_count: i64,
    pub is_favorite: bool,
    pub tidal_id: Option<i64>,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct VibeTrack {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub bpm: Option<f64>,
    pub camelot_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BasicTrack {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
}

pub fn get_same_vibe_tracks(
    conn: &Connection,
    track_id: i64,
    limit: i64,
) -> Result<Vec<VibeTrack>> {
    let src = conn.query_row(
        "SELECT d.bpm, d.camelot_key FROM audio_dsp_features d WHERE d.track_id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, Option<f64>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    );
    let (bpm, camelot_key) = match src {
        Ok(v) => v,
        Err(_) => return Ok(vec![]),
    };
    let (Some(bpm), Some(camelot_key)) = (bpm, camelot_key) else {
        return Ok(vec![]);
    };

    let camelot_num: i64 = camelot_key
        .trim_end_matches(|c: char| c.is_alphabetic())
        .parse()
        .unwrap_or(0);
    let camelot_letter = camelot_key.chars().last().unwrap_or('A');

    let adjacent_nums: Vec<i64> = vec![
        if camelot_num == 1 {
            12
        } else {
            camelot_num - 1
        },
        camelot_num,
        if camelot_num == 12 {
            1
        } else {
            camelot_num + 1
        },
    ];
    let camelot_patterns: Vec<String> = adjacent_nums
        .iter()
        .map(|n| format!("{}{}%", n, camelot_letter))
        .collect();
    let camelot_clause = camelot_patterns
        .iter()
        .map(|p| format!("d.camelot_key LIKE '{}'", p.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(" OR ");

    let sql = format!(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, d.bpm, d.camelot_key
         FROM tracks t
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         JOIN audio_dsp_features d ON d.track_id = t.id
         WHERE t.id != ?1
           AND d.bpm BETWEEN ?2 AND ?3
           AND ({camelot_clause})
         ORDER BY t.play_count DESC
         LIMIT ?4"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![track_id, bpm - 10.0, bpm + 10.0, limit], |row| {
        Ok(VibeTrack {
            id: row.get(0)?,
            title: row.get(1)?,
            artist_name: row.get(2)?,
            album_title: row.get(3)?,
            artwork_url: row.get(4)?,
            duration_ms: row.get(5)?,
            bpm: row.get(6)?,
            camelot_key: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_underrated_tracks(
    conn: &Connection,
    artist_id: i64,
    limit: i64,
) -> Result<Vec<BasicTrack>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms
         FROM tracks t
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE t.artist_id = ?1 AND t.play_count = 0
         ORDER BY RANDOM()
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![artist_id, limit], |row| {
        Ok(BasicTrack {
            id: row.get(0)?,
            title: row.get(1)?,
            artist_name: row.get(2)?,
            album_title: row.get(3)?,
            artwork_url: row.get(4)?,
            duration_ms: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// SQL subquery returning track ids that match `?1` via tracks_fts (title),
/// artists_fts (name), or albums_fts (title). Used inline as `t.id IN (...)`.
/// `?1` is reused in all three UNION arms — bind once.
fn track_fts_candidate_subquery() -> &'static str {
    "SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?1 \
     UNION \
     SELECT t2.id FROM tracks t2 JOIN artists_fts ON artists_fts.rowid = t2.artist_id WHERE artists_fts MATCH ?1 \
     UNION \
     SELECT t3.id FROM tracks t3 JOIN albums_fts  ON albums_fts.rowid  = t3.album_id  WHERE albums_fts  MATCH ?1"
}

/// Canonicalize a user-typed key filter into the analyzer's vocabulary
/// (sharps only, "Am" for minor / "Amaj" for major — see
/// services/audio_analysis/key.rs). "Bb" -> "A#maj", "bbm" -> "A#m",
/// "f# minor" -> "F#m", bare "A" -> "Amaj". Unrecognized input is returned
/// trimmed so the NOCASE equality match still gets a shot at it.
fn normalize_key_signature(input: &str) -> String {
    let trimmed = input.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return trimmed.to_string();
    };
    let letter = first.to_ascii_uppercase();
    if !('A'..='G').contains(&letter) {
        return trimmed.to_string();
    }
    let rest: String = chars.collect();
    let (accidental, suffix) = match rest.chars().next() {
        Some('#') => ("#", &rest[1..]),
        Some('b') | Some('B') => ("b", &rest[1..]),
        _ => ("", rest.as_str()),
    };
    let note = match (letter, accidental) {
        (l, "") => l.to_string(),
        (l, "#") => match l {
            'E' => "F".to_string(),
            'B' => "C".to_string(),
            l => format!("{l}#"),
        },
        (l, "b") => match l {
            'C' => "B".to_string(),
            'F' => "E".to_string(),
            'D' => "C#".to_string(),
            'E' => "D#".to_string(),
            'G' => "F#".to_string(),
            'A' => "G#".to_string(),
            'B' => "A#".to_string(),
            l => l.to_string(),
        },
        (l, _) => l.to_string(),
    };
    match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "maj" | "major" => format!("{note}maj"),
        "m" | "min" | "minor" => format!("{note}m"),
        _ => trimmed.to_string(),
    }
}

/// Resolve user-typed genre tokens (from `genre:` filters) to genre ids.
/// Matches slug or name, case-insensitively; hyphens in the token also match
/// spaces in the name ("hip-hop" -> "Hip Hop"). Returns (matched ids,
/// unmatched tokens) so the route can surface unknown genres instead of
/// silently searching unfiltered.
pub fn resolve_genre_tokens(
    conn: &Connection,
    tokens: &[String],
) -> Result<(Vec<i64>, Vec<String>)> {
    let mut ids = Vec::new();
    let mut unmatched = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT id FROM genres \
         WHERE slug = ?1 OR LOWER(name) = ?1 OR LOWER(name) = REPLACE(?1, '-', ' ')",
    )?;
    for token in tokens {
        let normalized = token.trim().to_lowercase();
        if normalized.is_empty() {
            continue;
        }
        let found: Option<i64> = stmt
            .query_row(params![normalized], |row| row.get(0))
            .optional()?;
        match found {
            Some(id) => ids.push(id),
            None => unmatched.push(token.clone()),
        }
    }
    Ok((ids, unmatched))
}

/// Expand genre ids to the full descendant subtree (children, grandchildren,
/// ...) via the parent_id closure. Ids not present in `genres` drop out.
pub fn expand_genre_descendants(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // i64 ids — safe to inline.
    let id_list: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    let sql = format!(
        "WITH RECURSIVE closure(id) AS (\
            SELECT id FROM genres WHERE id IN ({ids}) \
            UNION \
            SELECT g.id FROM genres g JOIN closure c ON g.parent_id = c.id\
         ) SELECT id FROM closure",
        ids = id_list.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let expanded = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(expanded)
}

/// SQL fragment + bind params produced by `build_audio_filter_sql`. `next_idx`
/// is the first free `?N` slot — caller binds `LIMIT ?{next_idx}`.
struct AudioFilterSql {
    sql: String,
    params: Vec<Box<dyn rusqlite::ToSql>>,
    next_idx: usize,
}

/// Builds the audio-filter portion of the WHERE clause and its bind params,
/// starting bind indices at `start_idx`. Does NOT emit `LIMIT` — the caller does.
/// Shared by both the FTS-first path and the LIKE fallback so the two cannot
/// drift on filter handling.
fn build_audio_filter_sql(filters: &AudioFilters, start_idx: usize) -> AudioFilterSql {
    let mut sql = String::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut idx = start_idx;

    if let Some(v) = filters.bpm_min {
        sql.push_str(&format!(" AND d.bpm >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.bpm_max {
        sql.push_str(&format!(" AND d.bpm <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.energy_min {
        sql.push_str(&format!(" AND d.energy >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.energy_max {
        sql.push_str(&format!(" AND d.energy <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.danceability_min {
        sql.push_str(&format!(" AND d.danceability >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.danceability_max {
        sql.push_str(&format!(" AND d.danceability <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(ref v) = filters.key_signature {
        // NOCASE + canonicalization so "key:am", "key:A", "key:Bb" all hit the
        // analyzer's "Am"/"Amaj"/"A#maj" vocabulary instead of matching nothing.
        sql.push_str(&format!(" AND d.key_signature = ?{idx} COLLATE NOCASE"));
        params.push(Box::new(normalize_key_signature(v)));
        idx += 1;
    }
    if let Some(ref v) = filters.camelot_key {
        sql.push_str(&format!(" AND d.camelot_key = ?{idx} COLLATE NOCASE"));
        params.push(Box::new(v.trim().to_string()));
        idx += 1;
    }
    if let Some(v) = filters.year_min {
        sql.push_str(&format!(" AND al.year >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.year_max {
        sql.push_str(&format!(" AND al.year <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if !filters.genre_ids.is_empty() {
        // i64 ids — safe to inline. Match against the same curated rowset the
        // genre galaxy uses (confidence floor + weakest-tag rescue) instead of
        // raw track_genres, so low-confidence junk tags (e.g. "Psychedelic
        // Rock" @ 0.29 on psytrance tracks) don't leak into genre searches.
        let id_list: Vec<String> = filters.genre_ids.iter().map(|id| id.to_string()).collect();
        let rowset = crate::genre::filter::filter_subquery(
            crate::genre::filter::GalaxyFilterRule::default_rule(),
        );
        sql.push_str(&format!(
            " AND t.id IN (SELECT track_id FROM ({rowset}) WHERE genre_id IN ({}))",
            id_list.join(", ")
        ));
    }
    if let Some(instrumental) = filters.is_instrumental {
        sql.push_str(&format!(" AND d.is_instrumental = ?{idx}"));
        params.push(Box::new(if instrumental { 1i64 } else { 0i64 }));
        idx += 1;
    }
    if let Some(ref v) = filters.artist_contains {
        sql.push_str(&format!(" AND LOWER(COALESCE(a.name, '')) LIKE ?{idx}"));
        params.push(Box::new(format!("%{}%", v.trim().to_lowercase())));
        idx += 1;
    }
    if let Some(ref v) = filters.album_contains {
        sql.push_str(&format!(" AND LOWER(COALESCE(al.title, '')) LIKE ?{idx}"));
        params.push(Box::new(format!("%{}%", v.trim().to_lowercase())));
        idx += 1;
    }
    if filters.liked_only {
        // Mirrors the Liked tab's client-side is_favorite filter so a filtered
        // Shuffle on that tab samples only liked tracks, not the whole match set.
        sql.push_str(" AND t.is_favorite = 1");
    }

    AudioFilterSql {
        sql,
        params,
        next_idx: idx,
    }
}

/// Deterministic display ranking. `offset` pages past the 50-row display cap
/// ("Show more"); pass 0 for the first page.
pub fn search_with_audio_filters(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
) -> Result<Vec<AudioSearchResult>> {
    search_with_audio_filters_ordered(conn, free_text, filters, limit, offset, false)
}

/// Total number of tracks matching the query + filters, independent of the
/// display LIMIT, so the UI can say "top 50 of N" instead of looking capped.
pub fn count_audio_filter_matches(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
) -> Result<i64> {
    match count_audio_filter_matches_inner(conn, free_text, filters, true) {
        Ok(count) => Ok(count),
        Err(err) => {
            tracing::warn!(?err, query = %free_text, "FTS count failed; falling back to LIKE");
            count_audio_filter_matches_inner(conn, free_text, filters, false)
        }
    }
}

fn count_audio_filter_matches_inner(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    use_fts: bool,
) -> Result<i64> {
    if filters.track_type.as_deref().is_some_and(|t| t != "track") {
        return Ok(0);
    }

    let normalized = free_text.trim().to_ascii_lowercase();

    let mut sql = String::from(
        "SELECT COUNT(*) \
         FROM tracks t \
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id \
         LEFT JOIN artists a ON a.id = t.artist_id \
         LEFT JOIN albums al ON al.id = t.album_id \
         WHERE 1=1",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut start_idx: usize = 1;

    if !normalized.is_empty() {
        if use_fts {
            sql.push_str(&format!(
                " AND t.id IN ({sub})",
                sub = track_fts_candidate_subquery()
            ));
            params.push(Box::new(to_fts_query(&normalized)));
        } else {
            let pattern = format!("%{normalized}%");
            sql.push_str(&format!(
                " AND (LOWER(t.title) LIKE ?{0} \
                   OR LOWER(COALESCE(a.name, '')) LIKE ?{0} \
                   OR LOWER(COALESCE(al.title, '')) LIKE ?{0})",
                start_idx
            ));
            params.push(Box::new(pattern));
        }
        start_idx += 1;
    }

    let filter_sql = build_audio_filter_sql(filters, start_idx);
    sql.push_str(&filter_sql.sql);
    params.extend(filter_sql.params);

    let mut stmt = conn.prepare(&sql)?;
    let count: i64 = stmt
        .query_row(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
            row.get(0)
        })?;
    Ok(count)
}

/// Same matching as `search_with_audio_filters`, but returns a true random
/// sample of the full matching set (`ORDER BY RANDOM()`) instead of the
/// deterministic favorite / play-count ranking. Backs the library Shuffle
/// button on a filtered view, so Shuffle randomizes across every matching track
/// instead of reshuffling the same top rows the display query returns.
pub fn search_with_audio_filters_shuffled(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
) -> Result<Vec<AudioSearchResult>> {
    search_with_audio_filters_ordered(conn, free_text, filters, limit, 0, true)
}

fn search_with_audio_filters_ordered(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
    shuffle: bool,
) -> Result<Vec<AudioSearchResult>> {
    match search_with_audio_filters_fts(conn, free_text, filters, limit, offset, shuffle) {
        Ok(results) => Ok(results),
        Err(err) => {
            tracing::warn!(?err, query = %free_text, "FTS library search failed; falling back to LIKE");
            search_with_audio_filters_like_fallback(
                conn, free_text, filters, limit, offset, shuffle,
            )
        }
    }
}

fn search_with_audio_filters_fts(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
    shuffle: bool,
) -> Result<Vec<AudioSearchResult>> {
    if filters.track_type.as_deref().is_some_and(|t| t != "track") {
        return Ok(Vec::new());
    }

    let normalized = free_text.trim().to_ascii_lowercase();
    let has_text = !normalized.is_empty();

    let mut sql = String::from(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, \
         d.bpm, d.energy, d.danceability, d.key_signature, d.camelot_key, \
         t.play_count, t.is_favorite, t.tidal_id, t.source \
         FROM tracks t \
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id \
         LEFT JOIN artists a ON a.id = t.artist_id \
         LEFT JOIN albums al ON al.id = t.album_id \
         WHERE 1=1",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut start_idx: usize = 1;

    if has_text {
        sql.push_str(&format!(
            " AND t.id IN ({sub})",
            sub = track_fts_candidate_subquery()
        ));
        params.push(Box::new(to_fts_query(&normalized)));
        start_idx += 1;
    }

    let filter_sql = build_audio_filter_sql(filters, start_idx);
    let limit_idx = filter_sql.next_idx;
    sql.push_str(&filter_sql.sql);
    params.extend(filter_sql.params);

    let order = if shuffle {
        "RANDOM()"
    } else {
        "t.is_favorite DESC, t.play_count DESC, t.fidelity_score DESC, t.title ASC"
    };
    sql.push_str(&format!(
        " ORDER BY {order} LIMIT ?{limit_idx} OFFSET ?{offset_idx}",
        offset_idx = limit_idx + 1
    ));
    params.push(Box::new(limit as i64));
    params.push(Box::new(offset as i64));

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
            Ok(AudioSearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                duration_ms: row.get(5)?,
                bpm: row.get(6)?,
                energy: row.get(7)?,
                danceability: row.get(8)?,
                key_signature: row.get(9)?,
                camelot_key: row.get(10)?,
                play_count: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                is_favorite: row.get::<_, i64>(12)? != 0,
                tidal_id: row.get(13)?,
                source: row.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Verbatim copy of the pre-FTS LIKE-based library search, kept as a fallback
/// when FTS errors. Substring-contiguous LIKE on title/artist/album, ordered by
/// today's existing `t.play_count DESC, t.last_played_at DESC`. Renamed (not just
/// "fallback") so a future reader knows this is the OLD semantics, deliberately.
fn search_with_audio_filters_like_fallback(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
    shuffle: bool,
) -> Result<Vec<AudioSearchResult>> {
    let normalized = free_text.trim().to_ascii_lowercase();

    if filters
        .track_type
        .as_deref()
        .is_some_and(|track_type| track_type != "track")
    {
        return Ok(Vec::new());
    }

    let mut sql = String::from(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, \
         d.bpm, d.energy, d.danceability, d.key_signature, d.camelot_key, \
         t.play_count, t.is_favorite, t.tidal_id, t.source \
         FROM tracks t \
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id \
         LEFT JOIN artists a ON a.id = t.artist_id \
         LEFT JOIN albums al ON al.id = t.album_id \
         WHERE 1=1",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut start_idx: usize = 1;

    if !normalized.is_empty() {
        let pattern = format!("%{normalized}%");
        sql.push_str(&format!(
            " AND (LOWER(t.title) LIKE ?{0} \
               OR LOWER(COALESCE(a.name, '')) LIKE ?{0} \
               OR LOWER(COALESCE(al.title, '')) LIKE ?{0})",
            start_idx
        ));
        params.push(Box::new(pattern));
        start_idx += 1;
    }

    let filter_sql = build_audio_filter_sql(filters, start_idx);
    let limit_idx = filter_sql.next_idx;
    sql.push_str(&filter_sql.sql);
    params.extend(filter_sql.params);

    let order = if shuffle {
        "RANDOM()"
    } else {
        "t.play_count DESC, t.last_played_at DESC"
    };
    sql.push_str(&format!(
        " ORDER BY {order} LIMIT ?{limit_idx} OFFSET ?{offset_idx}",
        offset_idx = limit_idx + 1
    ));
    params.push(Box::new(limit as i64));
    params.push(Box::new(offset as i64));

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
            Ok(AudioSearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                duration_ms: row.get(5)?,
                bpm: row.get(6)?,
                energy: row.get(7)?,
                danceability: row.get(8)?,
                key_signature: row.get(9)?,
                camelot_key: row.get(10)?,
                play_count: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                is_favorite: row.get::<_, i64>(12)? != 0,
                tidal_id: row.get(13)?,
                source: row.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}
