use crate::db::queries::{ChartEntrySeed, ChartSnapshotSeed, upsert_chart_snapshot};
use anyhow::{Context, Result, anyhow};
use regex::Regex;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use tracing::warn;

pub const KWORB_CHARTS_URL: &str = "https://kworb.net/charts";
const KWORB_COUNTRY_INDEX_URLS: &[&str] = &[
    "https://kworb.net/charts/index_a.html",
    "https://kworb.net/charts/index_c.html",
    "https://kworb.net/charts/index_n.html",
    "https://kworb.net/charts/index_u.html",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KworbMatrixCell {
    pub region: String,
    pub source_key: String,
    pub provider_label: String,
    pub artist: String,
    pub title: String,
    pub external_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KworbMatrixIngestReport {
    pub rows_seen: usize,
    pub entries_written: usize,
    pub snapshots_written: usize,
}

pub async fn fetch_kworb_matrix_html(client: &reqwest::Client) -> Result<String> {
    let mut html = fetch_kworb_page(client, KWORB_CHARTS_URL).await?;
    for url in KWORB_COUNTRY_INDEX_URLS {
        match fetch_kworb_page(client, url).await {
            Ok(page) => {
                html.push('\n');
                html.push_str(&page);
            }
            Err(err) => warn!(%url, error = %err, "kworb country chart page fetch failed"),
        }
    }
    Ok(html)
}

async fn fetch_kworb_page(client: &reqwest::Client, url: &str) -> Result<String> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetch kworb chart page {url}"))?
        .error_for_status()
        .with_context(|| format!("kworb chart page status {url}"))?;
    response
        .text()
        .await
        .with_context(|| format!("read kworb chart page {url}"))
}

pub fn parse_kworb_matrix_html(input: &str) -> Result<Vec<KworbMatrixCell>> {
    let mut cells = parse_kworb_table_cells(input).unwrap_or_default();
    cells.extend(parse_kworb_line_cells(input));

    let mut seen = BTreeSet::new();
    cells.retain(|cell| {
        seen.insert((
            cell.region.clone(),
            cell.source_key.clone(),
            cell.artist.clone(),
            cell.title.clone(),
        ))
    });

    if cells.is_empty() {
        return Err(anyhow!("kworb matrix rows not found"));
    }

    Ok(cells)
}

fn parse_kworb_table_cells(input: &str) -> Result<Vec<KworbMatrixCell>> {
    let rows = table_rows(input);
    let header_cells = rows
        .iter()
        .map(|row| table_cells(row))
        .find(|cells| {
            cells
                .first()
                .map(|cell| plain_text(&cell.html).eq_ignore_ascii_case("country"))
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow!("kworb matrix country header not found"))?;

    let providers = header_cells
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, cell)| {
            let label = plain_text(&cell.html);
            source_key_for_provider(&label).map(|source_key| (index, source_key, label))
        })
        .collect::<Vec<_>>();

    if providers.is_empty() {
        return Err(anyhow!("kworb matrix provider columns not found"));
    }

    let mut cells = Vec::new();
    for row in rows {
        let row_cells = table_cells(&row);
        if row_cells.len() < 2 {
            continue;
        }
        let country = plain_text(&row_cells[0].html);
        if country.eq_ignore_ascii_case("country") {
            continue;
        }
        let Some(region) = region_for_country(&country) else {
            continue;
        };

        for (index, source_key, provider_label) in &providers {
            let Some(cell) = row_cells.get(*index) else {
                continue;
            };
            let text = plain_text(&cell.html);
            if text.is_empty() || text == "-" {
                continue;
            }
            let (artist, title) = split_artist_title(&text);
            cells.push(KworbMatrixCell {
                region: region.to_string(),
                source_key: (*source_key).to_string(),
                provider_label: provider_label.clone(),
                artist,
                title,
                external_url: first_href(&cell.html),
            });
        }
    }

    Ok(cells)
}

fn parse_kworb_line_cells(input: &str) -> Vec<KworbMatrixCell> {
    let lines = html_lines(input);
    let mut cells = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let Some(providers) = providers_from_header(&lines[index]) else {
            index += 1;
            continue;
        };
        index += 1;

        while index < lines.len() {
            let line = &lines[index];
            if providers_from_header(line).is_some() {
                break;
            }
            if line.ends_with(':') {
                index += 1;
                continue;
            }
            let Some(region) = region_for_country(line) else {
                index += 1;
                continue;
            };
            index += 1;

            for (source_key, provider_label) in &providers {
                while index < lines.len() && lines[index].ends_with(':') {
                    index += 1;
                }
                if index >= lines.len()
                    || providers_from_header(&lines[index]).is_some()
                    || region_for_country(&lines[index]).is_some()
                {
                    break;
                }
                let text = &lines[index];
                if !text.is_empty() && text != "-" {
                    let (artist, title) = split_artist_title(text);
                    cells.push(KworbMatrixCell {
                        region: region.to_string(),
                        source_key: (*source_key).to_string(),
                        provider_label: (*provider_label).to_string(),
                        artist,
                        title,
                        external_url: None,
                    });
                }
                index += 1;
            }
        }
    }
    cells
}

pub fn ingest_kworb_matrix_html(
    conn: &Connection,
    chart_date: &str,
    fetched_at: i64,
    input: &str,
) -> Result<KworbMatrixIngestReport> {
    let cells = parse_kworb_matrix_html(input)?;
    let mut grouped: BTreeMap<(String, String), Vec<KworbMatrixCell>> = BTreeMap::new();
    for cell in cells {
        grouped
            .entry((cell.source_key.clone(), cell.region.clone()))
            .or_default()
            .push(cell);
    }

    let mut entries_written = 0usize;
    let mut snapshots_written = 0usize;
    for ((source_key, region), cells) in &grouped {
        let snapshot = ChartSnapshotSeed {
            source_key,
            region,
            period: "daily",
            chart_date,
            fetched_at,
            etag: None,
            content_hash: None,
            status: "ok",
        };
        let entries = cells
            .iter()
            .enumerate()
            .map(|(index, cell)| ChartEntrySeed {
                external_url: cell.external_url.as_deref(),
                raw_json: Some(json!({
                    "provider": cell.provider_label,
                    "source": "kworb",
                })),
                ..ChartEntrySeed::track((index + 1) as i64, &cell.artist, &cell.title)
            })
            .collect::<Vec<_>>();
        upsert_chart_snapshot(conn, &snapshot, &entries)?;
        entries_written += entries.len();
        snapshots_written += 1;
    }

    Ok(KworbMatrixIngestReport {
        rows_seen: grouped.len(),
        entries_written,
        snapshots_written,
    })
}

#[derive(Debug)]
struct HtmlCell {
    html: String,
}

fn table_rows(input: &str) -> Vec<String> {
    row_regex()
        .captures_iter(input)
        .filter_map(|capture| capture.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn table_cells(row: &str) -> Vec<HtmlCell> {
    cell_regex()
        .captures_iter(row)
        .filter_map(|capture| {
            capture.get(1).map(|m| HtmlCell {
                html: m.as_str().to_string(),
            })
        })
        .collect()
}

fn plain_text(input: &str) -> String {
    let without_tags = tag_regex().replace_all(input, " ");
    decode_entities(&without_tags)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn html_lines(input: &str) -> Vec<String> {
    let with_breaks = line_break_regex().replace_all(input, "\n");
    let without_tags = tag_regex().replace_all(&with_breaks, " ");
    decode_entities(&without_tags)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&ndash;", "-")
        .replace("&mdash;", "-")
}

fn first_href(input: &str) -> Option<String> {
    href_regex()
        .captures(input)
        .and_then(|capture| capture.get(1).map(|m| decode_entities(m.as_str())))
        .map(|href| {
            if href.starts_with("http://") || href.starts_with("https://") {
                href
            } else {
                format!("https://kworb.net/{}", href.trim_start_matches('/'))
            }
        })
}

fn source_key_for_provider(label: &str) -> Option<&'static str> {
    match normalize_label(label).as_str() {
        "itunes" => Some("itunes_daily"),
        "spotify" => Some("spotify_daily"),
        "applemusic" => Some("apple_music_daily"),
        "youtube" => Some("youtube_daily"),
        "shazam" => Some("shazam_daily"),
        "deezer" => Some("deezer_daily"),
        _ => None,
    }
}

fn providers_from_header(line: &str) -> Option<Vec<(&'static str, &'static str)>> {
    let normalized = normalize_label(line);
    if !normalized.starts_with("country") {
        return None;
    }

    let mut providers = [
        ("itunes", "itunes_daily", "iTunes"),
        ("spotify", "spotify_daily", "Spotify"),
        ("applemusic", "apple_music_daily", "Apple Music"),
        ("youtube", "youtube_daily", "YouTube"),
        ("shazam", "shazam_daily", "Shazam"),
        ("deezer", "deezer_daily", "Deezer"),
    ]
    .into_iter()
    .filter_map(|(needle, source_key, label)| {
        normalized
            .find(needle)
            .map(|position| (position, source_key, label))
    })
    .collect::<Vec<_>>();

    providers.sort_by_key(|(position, _, _)| *position);
    if providers.is_empty() {
        None
    } else {
        Some(
            providers
                .into_iter()
                .map(|(_, source_key, label)| (source_key, label))
                .collect(),
        )
    }
}

fn region_for_country(label: &str) -> Option<&'static str> {
    match normalize_label(label).as_str() {
        "worldwide" | "global" => Some("global"),
        "unitedstates" | "usa" | "us" => Some("US"),
        "unitedkingdom" | "uk" => Some("UK"),
        "australia" | "au" => Some("AU"),
        "canada" | "ca" => Some("CA"),
        "newzealand" | "nz" => Some("NZ"),
        _ => None,
    }
}

fn normalize_label(label: &str) -> String {
    label
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn split_artist_title(text: &str) -> (String, String) {
    if let Some((artist, title)) = text.split_once(" - ") {
        return (artist.trim().to_string(), title.trim().to_string());
    }
    ("Unknown artist".to_string(), text.trim().to_string())
}

fn row_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"(?is)<tr[^>]*>(.*?)</tr>"#).expect("row regex"))
}

fn cell_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"(?is)<t[dh][^>]*>(.*?)</t[dh]>"#).expect("cell regex"))
}

fn tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"(?is)<[^>]+>"#).expect("tag regex"))
}

fn href_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"(?is)<a\s+[^>]*href=["']([^"']+)["']"#).expect("href regex"))
}

fn line_break_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?is)</(?:a|td|th|tr|p|div|li|h[1-6])>|<br\s*/?>"#).expect("line break regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{queries, schema};

    #[test]
    fn kworb_matrix_ingest_writes_provider_snapshots_for_supported_regions() {
        let conn = Connection::open_in_memory().expect("open memory db");
        schema::run_migrations(&conn).expect("run migrations");
        let html = r#"
<table>
  <tr><th>Country</th><th>iTunes</th><th>Spotify</th><th>Apple Music</th><th>YouTube</th><th>Shazam</th><th>Deezer</th></tr>
  <tr>
    <td>Worldwide</td>
    <td><a href="/itunes/song/a.html">Artist A - Song A</a></td>
    <td><a href="/spotify/track/b.html">Artist B - Song B</a></td>
    <td>Artist C - Song C</td>
    <td>Artist D - Song D</td>
    <td>Artist E - Song E</td>
    <td>Artist F - Song F</td>
  </tr>
  <tr>
    <td>Australia</td>
    <td>AU Artist - AU iTunes</td>
    <td>AU Artist - AU Spotify</td>
    <td>-</td>
    <td>AU Video - AU YouTube</td>
    <td>AU Shazam - AU Song</td>
    <td>AU Deezer - AU Song</td>
  </tr>
</table>
"#;

        let report =
            ingest_kworb_matrix_html(&conn, "2026-05-28", 1234, html).expect("ingest matrix");

        assert_eq!(report.entries_written, 11);
        assert_eq!(report.snapshots_written, 11);
        let matrix = queries::get_chart_matrix(
            &conn,
            &["global", "AU"],
            &[
                "itunes_daily",
                "spotify_daily",
                "apple_music_daily",
                "youtube_daily",
            ],
            "daily",
        )
        .expect("read matrix");
        assert_eq!(
            matrix[0].cells["itunes_daily"].as_ref().unwrap().title,
            "Song A"
        );
        assert_eq!(
            matrix[0].cells["itunes_daily"]
                .as_ref()
                .unwrap()
                .external_url
                .as_deref(),
            Some("https://kworb.net/itunes/song/a.html")
        );
        assert_eq!(
            matrix[0].cells["spotify_daily"].as_ref().unwrap().artist,
            "Artist B"
        );
        assert_eq!(
            matrix[1].cells["spotify_daily"].as_ref().unwrap().title,
            "AU Spotify"
        );
        assert!(matrix[1].cells["apple_music_daily"].is_none());
        assert_eq!(
            matrix[1].cells["youtube_daily"].as_ref().unwrap().title,
            "AU YouTube"
        );
    }

    #[test]
    fn kworb_line_matrix_parser_handles_rendered_country_sequences() {
        let text = r#"
Country iTunes Spotify Apple Music YouTube Shazam Deezer
Worldwide
Muse - Hexagons
Michael Jackson - Billie Jean
Drake - Janice STFU
ZEROBASEONE - TOP 5
The Chemical Brothers - Go
Ella Langley - Choosin' Texas
United States
Ella Langley - Choosin' Texas
Drake - Janice STFU
Drake - Janice STFU
Drake - Janice STFU
Ella Langley - Choosin' Texas
Ella Langley - Choosin' Texas
Australia
The Chemical Brothers - Go
Ella Langley - Choosin' Texas
Drake - Janice STFU
HUNTR/X - Golden
The Chemical Brothers - Go
Olivia Rodrigo - drop dead
"#;

        let cells = parse_kworb_matrix_html(text).expect("parse rendered text");

        let au_spotify = cells
            .iter()
            .find(|cell| cell.region == "AU" && cell.source_key == "spotify_daily")
            .expect("AU Spotify");
        assert_eq!(au_spotify.artist, "Ella Langley");
        assert_eq!(au_spotify.title, "Choosin' Texas");
        let us_youtube = cells
            .iter()
            .find(|cell| cell.region == "US" && cell.source_key == "youtube_daily")
            .expect("US YouTube");
        assert_eq!(us_youtube.artist, "Drake");
        assert_eq!(us_youtube.title, "Janice STFU");
    }
}
