use crate::db::queries::{ChartEntrySeed, ChartSnapshotSeed, upsert_chart_snapshot};
use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotifyDailyChartRow {
    pub rank: i64,
    pub rank_delta: Option<i64>,
    pub artist: String,
    pub title: String,
    pub spotify_track_id: Option<String>,
    pub external_url: Option<String>,
    pub streams: Option<i64>,
    pub peak_rank: Option<i64>,
    pub days_on_chart: Option<i64>,
}

impl SpotifyDailyChartRow {
    pub fn as_entry_seed(&self) -> ChartEntrySeed<'_> {
        ChartEntrySeed {
            rank: self.rank,
            rank_delta: self.rank_delta,
            artist: &self.artist,
            title: &self.title,
            entity_type: "track",
            external_track_id: self.spotify_track_id.as_deref(),
            external_url: self.external_url.as_deref(),
            streams: self.streams,
            peak_rank: self.peak_rank,
            days_on_chart: self.days_on_chart,
            raw_json: Some(json!({
                "provider": "spotify",
                "spotify_track_id": self.spotify_track_id,
                "streams": self.streams,
            })),
            ..ChartEntrySeed::track(self.rank, &self.artist, &self.title)
        }
    }
}

pub fn parse_spotify_daily_csv(input: &str) -> Result<Vec<SpotifyDailyChartRow>> {
    let mut lines = input.lines().filter(|line| !line.trim().is_empty());
    let header_line = lines
        .next()
        .ok_or_else(|| anyhow!("spotify chart csv is empty"))?;
    let headers = parse_csv_line(header_line)
        .into_iter()
        .map(|header| normalize_header(&header))
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    for (index, line) in lines.enumerate() {
        let fields = parse_csv_line(line);
        if fields.iter().all(|field| field.trim().is_empty()) {
            continue;
        }
        let field = |name: &str| -> Option<&str> {
            headers
                .iter()
                .position(|header| header == name)
                .and_then(|idx| fields.get(idx))
                .map(String::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };

        let rank = field("rank")
            .ok_or_else(|| anyhow!("missing rank at row {}", index + 2))?
            .parse::<i64>()
            .with_context(|| format!("invalid rank at row {}", index + 2))?;
        let artist = field("artist_names")
            .or_else(|| field("artist"))
            .ok_or_else(|| anyhow!("missing artist at row {}", index + 2))?
            .to_string();
        let title = field("track_name")
            .or_else(|| field("title"))
            .ok_or_else(|| anyhow!("missing title at row {}", index + 2))?
            .to_string();
        let previous_rank = field("previous_rank").and_then(parse_optional_i64);
        let peak_rank = field("peak_rank").and_then(parse_optional_i64);
        let days_on_chart = field("days_on_chart")
            .and_then(parse_optional_i64)
            .or_else(|| {
                field("weeks_on_chart")
                    .and_then(parse_optional_i64)
                    .map(|weeks| weeks * 7)
            });
        let streams = field("streams").and_then(parse_optional_i64);
        let spotify_track_id = field("uri")
            .or_else(|| field("spotify_url"))
            .or_else(|| field("track_url"))
            .and_then(extract_spotify_track_id);
        let external_url = spotify_track_id
            .as_ref()
            .map(|id| format!("https://open.spotify.com/track/{id}"));

        rows.push(SpotifyDailyChartRow {
            rank,
            rank_delta: previous_rank.map(|prev| prev - rank),
            artist,
            title,
            spotify_track_id,
            external_url,
            streams,
            peak_rank,
            days_on_chart,
        });
    }

    Ok(rows)
}

pub fn ingest_spotify_daily_csv(
    conn: &Connection,
    region: &str,
    chart_date: &str,
    fetched_at: i64,
    input: &str,
) -> Result<i64> {
    let rows = parse_spotify_daily_csv(input)?;
    let content_hash = content_sha256(input);
    let snapshot = ChartSnapshotSeed {
        source_key: "spotify_daily",
        region,
        period: "daily",
        chart_date,
        fetched_at,
        etag: None,
        content_hash: Some(&content_hash),
        status: "ok",
    };
    let entries = rows
        .iter()
        .map(SpotifyDailyChartRow::as_entry_seed)
        .collect::<Vec<_>>();

    upsert_chart_snapshot(conn, &snapshot, &entries)
}

fn content_sha256(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    // sha2 0.11's finalize() returns a hybrid_array::Array which no longer
    // implements LowerHex; hex-encode via the shared helper instead.
    crate::services::cache_util::hex_encode(hasher.finalize())
}

fn normalize_header(value: &str) -> String {
    value
        .trim()
        .trim_matches('\u{feff}')
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
}

fn parse_optional_i64(value: &str) -> Option<i64> {
    let cleaned = value.trim().replace(',', "");
    if cleaned.is_empty() || cleaned == "-" {
        return None;
    }
    cleaned.parse::<i64>().ok()
}

fn extract_spotify_track_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(id) = value.strip_prefix("spotify:track:") {
        return non_empty_id(id);
    }
    if let Some((_, tail)) = value.split_once("/track/") {
        let id = tail.split(['?', '&', '/']).next().unwrap_or("").trim();
        return non_empty_id(id);
    }
    non_empty_id(value)
}

fn non_empty_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                current.push('"');
                let _ = chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current.trim().to_string());
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{queries, schema};
    use rusqlite::Connection;

    #[test]
    fn spotify_daily_csv_rows_normalize_to_chart_entry_seeds() {
        let csv = r#"rank,uri,artist_names,track_name,peak_rank,previous_rank,weeks_on_chart,streams
1,spotify:track:abc123,"Sabrina Carpenter","Espresso",1,2,12,"1,234,567"
2,https://open.spotify.com/track/def456?si=share,"Kendrick Lamar, SZA","Luther",1,,5,987654
3,,PinkPantheress,"Stateside + Zara Larsson",3,-,1,-
"#;

        let rows = parse_spotify_daily_csv(csv).expect("parse csv");

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].rank, 1);
        assert_eq!(rows[0].rank_delta, Some(1));
        assert_eq!(rows[0].artist, "Sabrina Carpenter");
        assert_eq!(rows[0].title, "Espresso");
        assert_eq!(rows[0].spotify_track_id.as_deref(), Some("abc123"));
        assert_eq!(
            rows[0].external_url.as_deref(),
            Some("https://open.spotify.com/track/abc123")
        );
        assert_eq!(rows[0].streams, Some(1_234_567));
        assert_eq!(rows[0].days_on_chart, Some(84));

        assert_eq!(rows[1].artist, "Kendrick Lamar, SZA");
        assert_eq!(rows[1].spotify_track_id.as_deref(), Some("def456"));
        assert_eq!(rows[1].rank_delta, None);

        let seed = rows[2].as_entry_seed();
        assert_eq!(seed.rank, 3);
        assert_eq!(seed.artist, "PinkPantheress");
        assert_eq!(seed.title, "Stateside + Zara Larsson");
        assert_eq!(seed.entity_type, "track");
        assert_eq!(seed.streams, None);
        assert_eq!(seed.resolution_status, None);
    }

    #[test]
    fn spotify_daily_ingest_replaces_daily_snapshot_and_feeds_matrix() {
        let conn = Connection::open_in_memory().expect("open memory db");
        schema::run_migrations(&conn).expect("run migrations");
        let first_csv = r#"rank,uri,artist_names,track_name,peak_rank,previous_rank,days_on_chart,streams
1,spotify:track:first,"First Artist","First Track",1,2,8,1000
2,spotify:track:second,"Second Artist","Second Track",2,3,4,900
"#;
        let refresh_csv = r#"rank,uri,artist_names,track_name,peak_rank,previous_rank,days_on_chart,streams
1,spotify:track:updated,"Updated Artist","Updated Track",1,4,9,1200
"#;

        ingest_spotify_daily_csv(&conn, "global", "2026-05-28", 100, first_csv)
            .expect("ingest first snapshot");
        ingest_spotify_daily_csv(&conn, "global", "2026-05-28", 200, refresh_csv)
            .expect("ingest refresh snapshot");

        let latest =
            queries::get_latest_chart_snapshot(&conn, "spotify_daily", "global", "daily", 10)
                .expect("read latest snapshot")
                .expect("latest snapshot");
        assert_eq!(latest.snapshot.chart_date, "2026-05-28");
        assert_eq!(latest.snapshot.fetched_at, 200);
        assert_eq!(latest.entries.len(), 1);
        assert_eq!(latest.entries[0].rank, 1);
        assert_eq!(latest.entries[0].artist, "Updated Artist");
        assert_eq!(latest.entries[0].title, "Updated Track");
        assert_eq!(
            latest.entries[0].external_url.as_deref(),
            Some("https://open.spotify.com/track/updated")
        );
        assert_eq!(latest.entries[0].streams, Some(1200));
        assert!(latest.entries[0].tidal_id.is_none());
        assert_eq!(latest.entries[0].resolution_status, "unresolved");

        let matrix = queries::get_chart_matrix(
            &conn,
            &["global"],
            &["spotify_daily", "youtube_daily"],
            "daily",
        )
        .expect("read matrix");
        assert_eq!(
            matrix[0].cells["spotify_daily"].as_ref().unwrap().title,
            "Updated Track"
        );
        assert!(matrix[0].cells["youtube_daily"].is_none());
    }
}
