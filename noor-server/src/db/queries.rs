use super::models::*;
use crate::services::discovery::DiscoveryCandidateSeed;
use anyhow::{Result, bail};
use rusqlite::{
    Connection, OptionalExtension, Row, params, params_from_iter, types::Value as SqlValue,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

mod albums;
mod artists;
mod audio_features;
mod audio_search;
mod genre_analytics;
mod genres;
mod library_search;
mod playlists;
mod server_config;
mod similarity;
mod sync_metadata;
mod tracks;
pub use albums::*;
pub use artists::*;
pub use audio_features::*;
pub use audio_search::*;
pub use genre_analytics::*;
pub use genres::*;
pub use library_search::*;
pub use playlists::*;
pub use server_config::*;
pub use similarity::*;
pub use sync_metadata::*;
pub use tracks::*;

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

#[cfg(test)]
mod tests;
