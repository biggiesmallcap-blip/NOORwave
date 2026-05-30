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
const MAX_KWORB_DETAIL_ROWS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KworbMatrixCell {
    pub region: String,
    pub source_key: String,
    pub provider_label: String,
    pub artist: String,
    pub title: String,
    pub external_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KworbChartEntry {
    pub rank: i64,
    pub rank_delta: Option<i64>,
    pub artist: String,
    pub title: String,
    pub external_url: Option<String>,
    pub streams: Option<i64>,
    pub views: Option<i64>,
    pub points: Option<i64>,
    pub seven_day_streams: Option<i64>,
    pub total_streams: Option<i64>,
    pub days_on_chart: Option<i64>,
    pub peak_rank: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KworbMatrixIngestReport {
    pub rows_seen: usize,
    pub entries_written: usize,
    pub snapshots_written: usize,
}

#[derive(Debug, Clone)]
pub struct KworbChartPages {
    pub matrix_html: String,
    pub detail_pages: BTreeMap<String, String>,
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

pub async fn fetch_kworb_chart_pages(client: &reqwest::Client) -> Result<KworbChartPages> {
    let matrix_html = fetch_kworb_matrix_html(client).await?;
    let detail_urls = kworb_detail_urls_for_matrix_html(&matrix_html).unwrap_or_default();
    let mut detail_pages = BTreeMap::new();
    let detail_fetches = detail_urls.into_iter().map(|url| async move {
        let result = fetch_kworb_page(client, &url).await;
        (url, result)
    });

    for (url, result) in futures::future::join_all(detail_fetches).await {
        match result {
            Ok(page) => {
                detail_pages.insert(url, page);
            }
            Err(err) => warn!(%url, error = %err, "kworb chart detail page fetch failed"),
        }
    }

    Ok(KworbChartPages {
        matrix_html,
        detail_pages,
    })
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

pub fn parse_kworb_detail_html(source_key: &str, input: &str) -> Result<Vec<KworbChartEntry>> {
    let mut entries = parse_kworb_detail_table_entries(source_key, input);

    let mut seen = BTreeSet::new();
    entries.retain(|entry| {
        seen.insert((
            entry.rank,
            entry.artist.to_ascii_lowercase(),
            entry.title.to_ascii_lowercase(),
        ))
    });
    entries.sort_by_key(|entry| entry.rank);

    if entries.is_empty() {
        return Err(anyhow!("kworb chart detail rows not found"));
    }

    Ok(entries)
}

fn parse_kworb_detail_table_entries(source_key: &str, input: &str) -> Vec<KworbChartEntry> {
    let mut entries = Vec::new();
    let mut headers: Vec<String> = Vec::new();

    for row in table_rows(input) {
        let row_cells = table_cells(&row);
        if row_cells.len() < 2 {
            continue;
        }

        let values = row_cells
            .iter()
            .map(|cell| plain_text(&cell.html))
            .collect::<Vec<_>>();
        if values.iter().any(|value| {
            let normalized = normalize_label(value);
            normalized == "artistandtitle" || normalized == "artisttitle"
        }) {
            headers = values.iter().map(|value| normalize_label(value)).collect();
            continue;
        }

        let Some((rank_index, rank)) = values
            .iter()
            .enumerate()
            .find_map(|(index, value)| parse_rank(value).map(|rank| (index, rank)))
        else {
            continue;
        };

        let Some(title_index) = detail_title_cell_index(&values, rank_index) else {
            continue;
        };
        let (artist, title) = split_artist_title(&values[title_index]);
        if title.is_empty() {
            continue;
        }

        let rank_delta = metric_by_header(&headers, &values, &["p"])
            .and_then(|value| parse_rank_delta(&value))
            .or_else(|| {
                values
                    .get(rank_index + 1)
                    .and_then(|value| parse_rank_delta(value))
            });
        let streams = if source_key == "spotify_daily" {
            metric_by_header(&headers, &values, &["streams"])
                .and_then(|value| parse_integer_metric(&value))
        } else {
            None
        };
        let views = if source_key == "youtube_daily" {
            metric_by_header(&headers, &values, &["views"])
                .and_then(|value| parse_integer_metric(&value))
        } else {
            None
        };
        let points = if source_key != "spotify_daily" && source_key != "youtube_daily" {
            metric_by_header(&headers, &values, &["points", "streams", "views"])
                .and_then(|value| parse_integer_metric(&value))
        } else {
            None
        };
        let seven_day_streams = metric_by_header(&headers, &values, &["7day"])
            .and_then(|value| parse_integer_metric(&value));
        let total_streams = metric_by_header(&headers, &values, &["total"])
            .and_then(|value| parse_integer_metric(&value));
        let days_on_chart = metric_by_header(&headers, &values, &["days"])
            .and_then(|value| parse_integer_metric(&value));
        let peak_rank = metric_by_header(&headers, &values, &["pk", "peak"])
            .and_then(|value| parse_rank(&value));

        entries.push(KworbChartEntry {
            rank,
            rank_delta,
            artist,
            title,
            external_url: first_href(&row_cells[title_index].html).or_else(|| first_href(&row)),
            streams,
            views,
            points,
            seven_day_streams,
            total_streams,
            days_on_chart,
            peak_rank,
        });
    }

    entries
}

pub fn ingest_kworb_matrix_html(
    conn: &Connection,
    chart_date: &str,
    fetched_at: i64,
    input: &str,
) -> Result<KworbMatrixIngestReport> {
    ingest_kworb_matrix_html_with_details(conn, chart_date, fetched_at, input, &BTreeMap::new())
}

pub fn ingest_kworb_chart_pages(
    conn: &Connection,
    chart_date: &str,
    fetched_at: i64,
    pages: &KworbChartPages,
) -> Result<KworbMatrixIngestReport> {
    ingest_kworb_matrix_html_with_details(
        conn,
        chart_date,
        fetched_at,
        &pages.matrix_html,
        &pages.detail_pages,
    )
}

pub fn ingest_kworb_matrix_html_with_details(
    conn: &Connection,
    chart_date: &str,
    fetched_at: i64,
    input: &str,
    detail_pages: &BTreeMap<String, String>,
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
        let detail_url = cells.iter().find_map(|cell| cell.external_url.as_deref());
        let detail_entries = detail_url
            .and_then(|url| detail_pages.get(url).map(|html| (url, html)))
            .and_then(|(url, html)| {
                parse_kworb_detail_html(source_key, html)
                    .ok()
                    .map(|rows| (url, rows))
            });
        let entries = match detail_entries.as_ref() {
            Some((url, rows)) if !rows.is_empty() => rows
                .iter()
                .take(MAX_KWORB_DETAIL_ROWS)
                .map(|entry| {
                    let mut seed = ChartEntrySeed::track(entry.rank, &entry.artist, &entry.title);
                    seed.rank_delta = entry.rank_delta;
                    seed.external_url = entry.external_url.as_deref();
                    seed.streams = entry.streams;
                    seed.views = entry.views;
                    seed.points = entry.points.map(|value| value as f64);
                    seed.seven_day_streams = entry.seven_day_streams;
                    seed.total_streams = entry.total_streams;
                    seed.days_on_chart = entry.days_on_chart;
                    seed.peak_rank = entry.peak_rank;
                    seed.raw_json = Some(json!({
                        "provider": cells[0].provider_label,
                        "source": "kworb",
                        "detail_url": *url,
                    }));
                    seed
                })
                .collect::<Vec<_>>(),
            _ => cells
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
                .collect::<Vec<_>>(),
        };
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
    tr_start_regex()
        .split(input)
        .skip(1)
        .filter_map(|part| {
            let row = part.split("</tr>").next().unwrap_or(part);
            if row.contains("<td") || row.contains("<th") {
                Some(row.to_string())
            } else {
                None
            }
        })
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

fn kworb_detail_urls_for_matrix_html(input: &str) -> Result<BTreeSet<String>> {
    let cells = parse_kworb_matrix_html(input)?;
    Ok(cells
        .into_iter()
        .filter_map(|cell| cell.external_url)
        .filter(|url| url.starts_with("https://kworb.net/"))
        .collect())
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

fn detail_title_cell_index(values: &[String], rank_index: usize) -> Option<usize> {
    values
        .iter()
        .enumerate()
        .skip(rank_index + 1)
        .find(|(_, value)| value.contains(" - "))
        .map(|(index, _)| index)
        .or_else(|| {
            values
                .iter()
                .enumerate()
                .skip(rank_index + 1)
                .find(|(_, value)| {
                    let normalized = normalize_label(value);
                    !value.is_empty()
                        && !is_metric_like(value)
                        && normalized != "re"
                        && normalized != "new"
                        && normalized != "steady"
                })
                .map(|(index, _)| index)
        })
}

fn metric_by_header(headers: &[String], values: &[String], names: &[&str]) -> Option<String> {
    headers
        .iter()
        .enumerate()
        .find(|(_, header)| names.iter().any(|name| header == name))
        .and_then(|(index, _)| values.get(index).cloned())
}

fn parse_rank(value: &str) -> Option<i64> {
    let digits = value
        .trim()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok()
    }
}

fn parse_rank_delta(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed == "=" {
        return Some(0);
    }
    if trimmed.eq_ignore_ascii_case("RE") || trimmed.eq_ignore_ascii_case("NEW") {
        return None;
    }
    let signed = trimmed.replace(',', "");
    let parsed = signed.parse::<i64>().ok()?;
    Some(-parsed)
}

fn parse_integer_metric(value: &str) -> Option<i64> {
    let normalized = value.trim().trim_start_matches('+').replace(',', "");
    normalized.parse::<i64>().ok()
}

fn is_metric_like(value: &str) -> bool {
    let value = value.trim();
    value == "="
        || value.eq_ignore_ascii_case("RE")
        || value.eq_ignore_ascii_case("NEW")
        || value
            .trim_start_matches(['+', '-'])
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == ',' || ch == '.')
}

fn tr_start_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"(?is)<tr[^>]*>"#).expect("tr start regex"))
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
    fn kworb_detail_parser_reads_ranked_track_rows() {
        let html = r#"
<table>
  <tr><th>Pos</th><th>P+</th><th>Artist and Title</th><th>Days</th><th>Pk</th><th>Streams</th><th>7Day</th><th>Total</th></tr>
  <tr><td>1</td><td>=</td><td><a href="/spotify/track/a.html">Justin Bieber - Beauty And A Beat (feat. Nicki Minaj)</a></td><td>24</td><td>1</td><td>1,234,567</td><td>7,654,321</td><td>45,000,000</td></tr>
  <tr><td>2</td><td>+3</td><td><a href="/spotify/track/b.html">Drake - ICEMAN</a></td><td>3</td><td>2</td><td>999,999</td><td>2,000,000</td><td>3,000,000</td></tr>
</table>
"#;

        let entries = parse_kworb_detail_html("spotify_daily", html).expect("parse detail rows");

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].rank, 1);
        assert_eq!(entries[0].artist, "Justin Bieber");
        assert_eq!(entries[0].title, "Beauty And A Beat (feat. Nicki Minaj)");
        assert_eq!(entries[0].streams, Some(1_234_567));
        assert_eq!(entries[0].seven_day_streams, Some(7_654_321));
        assert_eq!(entries[0].total_streams, Some(45_000_000));
        assert_eq!(entries[0].days_on_chart, Some(24));
        assert_eq!(entries[0].peak_rank, Some(1));
        assert_eq!(entries[0].rank_delta, Some(0));
        assert_eq!(entries[1].rank_delta, Some(-3));
        assert_eq!(
            entries[1].external_url.as_deref(),
            Some("https://kworb.net/spotify/track/b.html")
        );
    }

    #[test]
    fn kworb_matrix_ingest_prefers_detail_rows_for_chart_snapshots() {
        let conn = Connection::open_in_memory().expect("open memory db");
        schema::run_migrations(&conn).expect("run migrations");
        let index_html = r#"
<table>
  <tr><th>Country</th><th>Spotify</th></tr>
  <tr><td>Worldwide</td><td><a href="/spotify/country/global_daily.html">Justin Bieber - Beauty And A Beat (feat. Nicki Minaj)</a></td></tr>
</table>
"#;
        let detail_html = r#"
<table>
  <tr><th>Pos</th><th>P+</th><th>Artist and Title</th><th>Streams</th></tr>
  <tr><td>1</td><td>=</td><td><a href="/spotify/track/a.html">Justin Bieber - Beauty And A Beat (feat. Nicki Minaj)</a></td><td>1,234,567</td></tr>
  <tr><td>2</td><td>-1</td><td><a href="/spotify/track/b.html">Drake - ICEMAN</a></td><td>999,999</td></tr>
  <tr><td>3</td><td>+4</td><td><a href="/spotify/track/c.html">Olivia Rodrigo - the cure</a></td><td>888,888</td></tr>
</table>
"#;
        let detail_pages = BTreeMap::from([(
            "https://kworb.net/spotify/country/global_daily.html".to_string(),
            detail_html.to_string(),
        )]);

        let report = ingest_kworb_matrix_html_with_details(
            &conn,
            "2026-05-29",
            1234,
            index_html,
            &detail_pages,
        )
        .expect("ingest detail snapshot");

        assert_eq!(report.entries_written, 3);
        assert_eq!(report.snapshots_written, 1);
        let snapshot =
            queries::get_latest_chart_snapshot(&conn, "spotify_daily", "global", "daily", 20)
                .expect("read snapshot")
                .expect("snapshot exists");
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(snapshot.entries[1].title, "ICEMAN");
        assert_eq!(snapshot.entries[2].rank, 3);
        assert_eq!(snapshot.entries[2].streams, Some(888_888));
        let matrix = queries::get_chart_matrix(&conn, &["global"], &["spotify_daily"], "daily")
            .expect("read matrix");
        assert_eq!(
            matrix[0].cells["spotify_daily"].as_ref().unwrap().title,
            "Beauty And A Beat (feat. Nicki Minaj)"
        );
    }

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
    fn kworb_matrix_ingest_handles_kworb_rows_without_closing_tr_tags() {
        let conn = Connection::open_in_memory().expect("open memory db");
        schema::run_migrations(&conn).expect("run migrations");
        let html = r#"
<table>
<thead><tr><th>Country</th><th>iTunes</th><th>Spotify</th><th>Apple Music</th><th>YouTube</th><th>Shazam</th><th>Deezer</th></tr></thead><tbody>
<tr><td><div>Worldwide</div></td><td><div><a href="/ww/index.html">The Chemical Brothers - Go</a></div></td><td><div><a href="/spotify/country/global_daily.html">Justin Bieber - Beauty And A Beat (feat. Nicki Minaj)</a></div></td><td><div><a href="/apple_songs/index.html">Justin Bieber - Beauty and a Beat</a></div></td><td><div><a href="/youtube/index.html">ALPHA DRIVE ONE 'OMG!' MV</a></div></td><td><div><a href="/charts/shazam/ww.html">The Chemical Brothers - Go</a></div></td><td><div><a href="/charts/deezer/ww.html">Ella Langley - Choosin' Texas</a></div></td>
<tr><td><div>United States</div></td><td><div><a href="/charts/itunes/us.html">Ella Langley - Choosin' Texas</a></div></td><td><div><a href="/spotify/country/us_daily.html">Drake - Janice STFU</a></div></td><td><div><a href="/charts/apple_s/us.html">Drake - Janice STFU</a></div></td><td><div><a href="/youtube/insights/us_daily.html">Ella Langley - Choosin' Texas</a></div></td><td><div><a href="/charts/shazam/us.html">Drake - Janice STFU</a></div></td><td><div><a href="/charts/deezer/us.html">Ella Langley - Choosin' Texas</a></div></td>
<tr><td><div>Australia</div></td><td><div><a href="/charts/itunes/au.html">Ella Langley - Choosin' Texas</a></div></td><td><div><a href="/spotify/country/au_daily.html">Olivia Rodrigo - the cure</a></div></td><td><div><a href="/charts/apple_s/au.html">Olivia Rodrigo - the cure</a></div></td><td><div><a href="/youtube/insights/au_daily.html">HUNTR/X - Golden</a></div></td><td><div><a href="/charts/shazam/au.html">Josh Fawaz - Like a Prayer</a></div></td><td><div><a href="/charts/deezer/au.html">Sabrina Carpenter - When Did You Get Hot?</a></div></td>
</tbody></table>
"#;

        let report =
            ingest_kworb_matrix_html(&conn, "2026-05-29", 1234, html).expect("ingest matrix");

        assert_eq!(report.entries_written, 18);
        assert_eq!(report.snapshots_written, 18);
        let matrix = queries::get_chart_matrix(
            &conn,
            &["global", "US", "AU"],
            &["spotify_daily", "youtube_daily", "deezer_daily"],
            "daily",
        )
        .expect("read matrix");
        assert_eq!(
            matrix[1].cells["spotify_daily"].as_ref().unwrap().title,
            "Janice STFU"
        );
        assert_eq!(
            matrix[1].cells["youtube_daily"].as_ref().unwrap().artist,
            "Ella Langley"
        );
        assert_eq!(
            matrix[2].cells["deezer_daily"].as_ref().unwrap().title,
            "When Did You Get Hot?"
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
