use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tracing::{info, warn};

const MB_API_BASE: &str = "https://musicbrainz.org/ws/2";
const MB_USER_AGENT: &str = "NOOR/0.1 (noor-music-app)";
/// MusicBrainz rate limit: 1 request per second (their policy).
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(1100);
const PORTABLE_SNAPSHOT_DIR: &str = "data/musicbrainz";
const PORTABLE_CHECKED_FILE: &str = "musicbrainz_checked.csv";
const PORTABLE_LASTFM_CHECKED_FILE: &str = "lastfm_checked.csv";
const PORTABLE_GENRES_FILE: &str = "musicbrainz_genres.csv";
const PORTABLE_CONTEXT_TAGS_FILE: &str = "lastfm_context_tags.csv";
const PORTABLE_MANIFEST_FILE: &str = "manifest.json";

// ── MusicBrainz API response types ───────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct MbRecordingSearch {
    recordings: Vec<MbRecording>,
}

#[derive(Debug, serde::Deserialize)]
struct MbRecording {
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    tags: Vec<MbTag>,
}

#[derive(Debug, serde::Deserialize)]
struct MbTag {
    name: String,
    count: i32,
}

// ── Track batch from DB ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrackToEnrich {
    pub id: i64,
    pub isrc: Option<String>,
    pub title: String,
    pub artist_name: Option<String>,
}

/// Tracks not yet in `musicbrainz_checked` (never processed).
pub fn load_unenriched_tracks(
    conn: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<TrackToEnrich>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.isrc, t.title, a.name
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         WHERE NOT EXISTS (
             SELECT 1 FROM musicbrainz_checked mc WHERE mc.track_id = t.id
         )
         ORDER BY t.play_count DESC, t.date_added DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(TrackToEnrich {
                id: row.get(0)?,
                isrc: row.get(1)?,
                title: row.get(2)?,
                artist_name: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn count_unenriched_tracks(conn: &Connection) -> Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks t
         WHERE NOT EXISTS (
             SELECT 1 FROM musicbrainz_checked mc WHERE mc.track_id = t.id
         )",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

/// Mark a track as checked (processed). Always called after a lookup attempt,
/// regardless of whether genres were found.
pub fn mark_checked(conn: &Connection, track_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO musicbrainz_checked (track_id) VALUES (?1)",
        params![track_id],
    )?;
    Ok(())
}

/// Write genre tags for a track into `track_genres` (source = 'musicbrainz').
/// Skips genres that don't match any known genre in the `genres` table.
/// Returns count of genre rows inserted.
pub fn write_genres(
    conn: &Connection,
    track_id: i64,
    genre_tags: &[(String, Option<u32>)],
) -> Result<usize> {
    // Always mark as checked first so we never re-query this track.
    mark_checked(conn, track_id)?;

    use crate::genre::scorer::{MIN_SCORE_FLOOR, TagInput, TagLevel, TagSource, score_genre_tags};
    use crate::tags::context::{TagContext, classify_tag_context};

    let catalog = crate::genre::builder::embedded_builder().catalog();
    let mut inputs = Vec::new();
    for (name, count) in genre_tags {
        let is_known = catalog.resolve_single(name).is_some();
        let classified = classify_tag_context(name, is_known);
        if classified.context == TagContext::Genre {
            inputs.push(TagInput {
                name: name.clone(),
                source: TagSource::MusicBrainzTag,
                level: TagLevel::Recording,
                count: *count,
            });
        }
    }

    let result = score_genre_tags(&inputs, MIN_SCORE_FLOOR);

    let mut inserted = 0;
    for scored in &result.genres {
        let normalized = scored.canonical.trim().to_ascii_lowercase();
        let genre_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM genres WHERE slug = ?1 OR LOWER(name) = ?1 LIMIT 1",
                params![normalized],
                |row| row.get(0),
            )
            .ok();

        if let Some(gid) = genre_id {
            let n = conn.execute(
                "INSERT OR IGNORE INTO track_genres (track_id, genre_id, source, confidence)
                 VALUES (?1, ?2, 'musicbrainz', ?3)",
                params![track_id, gid, scored.score],
            )?;
            inserted += n;
        }
    }
    Ok(inserted)
}

// ── HTTP lookup ───────────────────────────────────────────────────────────────

/// Fetch genre tags for a single ISRC from MusicBrainz.
/// Uses the search endpoint with inc=tags (release-groups is a lookup-only inc).
pub async fn fetch_genres_by_isrc(
    client: &reqwest::Client,
    isrc: &str,
) -> Result<Vec<(String, Option<u32>)>> {
    let url = format!("{MB_API_BASE}/recording?fmt=json&query=isrc:{isrc}&inc=tags&limit=1");
    let resp = client
        .get(&url)
        .header("User-Agent", MB_USER_AGENT)
        .send()
        .await
        .context("MusicBrainz ISRC request failed")?
        .error_for_status()
        .context("MusicBrainz ISRC returned error status")?;

    let data: MbRecordingSearch = resp
        .json()
        .await
        .context("failed to parse MusicBrainz ISRC response")?;

    Ok(extract_tags(data))
}

/// Fetch genres using an artist+title fallback when no ISRC is available.
pub async fn fetch_genres_by_title(
    client: &reqwest::Client,
    artist: &str,
    title: &str,
) -> Result<Vec<(String, Option<u32>)>> {
    // Escape Lucene special characters in artist/title to avoid parse errors.
    let safe_artist = lucene_escape(artist);
    let safe_title = lucene_escape(title);
    let query = format!("artist:{safe_artist} AND recording:{safe_title}");
    let encoded = urlencoding::encode(&query);
    let url = format!("{MB_API_BASE}/recording?fmt=json&query={encoded}&inc=tags&limit=1");

    let resp = client
        .get(&url)
        .header("User-Agent", MB_USER_AGENT)
        .send()
        .await
        .context("MusicBrainz title request failed")?
        .error_for_status()
        .context("MusicBrainz title returned error status")?;

    let data: MbRecordingSearch = resp
        .json()
        .await
        .context("failed to parse MusicBrainz title response")?;

    Ok(extract_tags(data))
}

fn extract_tags(data: MbRecordingSearch) -> Vec<(String, Option<u32>)> {
    let Some(rec) = data.recordings.into_iter().next() else {
        return vec![];
    };

    let mut tags: Vec<(i32, String)> = rec.tags.into_iter().map(|t| (t.count, t.name)).collect();
    // Sort highest count first, deduplicate, cap at 5.
    tags.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    tags.into_iter()
        .filter_map(|(count, name)| {
            let key = name.to_ascii_lowercase();
            if seen.insert(key) {
                let count = if count > 0 { Some(count as u32) } else { None };
                Some((name, count))
            } else {
                None
            }
        })
        .take(5)
        .collect()
}

/// Escape Lucene special characters so artist/title strings don't break the query.
fn lucene_escape(s: &str) -> String {
    let specials = [
        '+', '-', '&', '|', '!', '(', ')', '{', '}', '[', ']', '^', '"', '~', '*', '?', ':', '\\',
        '/',
    ];
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        if specials.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

// ── Progress reporting ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EnrichmentProgress {
    pub processed: usize,
    pub total: usize,
    #[allow(dead_code)]
    pub genres_assigned: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PortableSnapshotFiles {
    checked: String,
    genres: String,
    #[serde(default)]
    lastfm_checked: Option<String>,
    #[serde(default)]
    context_tags: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PortableSnapshotManifest {
    generated_at: String,
    db_path: String,
    checked_rows: usize,
    genre_rows: usize,
    #[serde(default)]
    lastfm_checked_rows: usize,
    #[serde(default)]
    context_tag_rows: usize,
    files: PortableSnapshotFiles,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PortableSnapshotStatus {
    pub exists: bool,
    pub path: String,
    pub generated_at: Option<String>,
    pub checked_rows: usize,
    pub genre_rows: usize,
    pub lastfm_checked_rows: usize,
    pub context_tag_rows: usize,
}

#[derive(Debug, Clone)]
pub struct PortableSnapshotExportResult {
    pub status: PortableSnapshotStatus,
}

#[derive(Debug, Clone)]
pub struct PortableSnapshotImportResult {
    pub status: PortableSnapshotStatus,
    pub checked_inserted: usize,
    pub checked_skipped: usize,
    pub lastfm_checked_inserted: usize,
    pub lastfm_checked_skipped: usize,
    pub genre_inserted: usize,
    pub track_skipped: usize,
    pub genre_skipped: usize,
    pub context_tag_inserted: usize,
    pub context_tag_skipped: usize,
}

fn snapshot_dir() -> PathBuf {
    crate::paths::resolve_db_path_from_env()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(PORTABLE_SNAPSHOT_DIR)
}

fn snapshot_manifest_path() -> PathBuf {
    snapshot_dir().join(PORTABLE_MANIFEST_FILE)
}

fn snapshot_checked_path() -> PathBuf {
    snapshot_dir().join(PORTABLE_CHECKED_FILE)
}

fn snapshot_lastfm_checked_path() -> PathBuf {
    snapshot_dir().join(PORTABLE_LASTFM_CHECKED_FILE)
}

fn snapshot_genres_path() -> PathBuf {
    snapshot_dir().join(PORTABLE_GENRES_FILE)
}

fn snapshot_context_tags_path() -> PathBuf {
    snapshot_dir().join(PORTABLE_CONTEXT_TAGS_FILE)
}

fn parse_manifest(path: &Path) -> Result<PortableSnapshotManifest> {
    let file = File::open(path)?;
    Ok(serde_json::from_reader(BufReader::new(file))?)
}

fn csv_escape_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn parse_csv_line(line: &str) -> Result<Vec<String>> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => {
                in_quotes = !in_quotes;
            }
            ',' if !in_quotes => {
                fields.push(field);
                field = String::new();
            }
            _ => field.push(ch),
        }
    }

    if in_quotes {
        anyhow::bail!("Unterminated quoted field in portable snapshot CSV");
    }

    fields.push(field);
    Ok(fields)
}

fn require_source(value: &str) -> Result<&str> {
    match value {
        "musicbrainz" | "lastfm" => Ok(value),
        _ => anyhow::bail!("Unsupported portable genre source: {value}"),
    }
}

pub fn read_portable_snapshot_status() -> Result<PortableSnapshotStatus> {
    let dir = snapshot_dir();
    let manifest_path = snapshot_manifest_path();
    let checked_path = snapshot_checked_path();
    let genres_path = snapshot_genres_path();
    if !manifest_path.exists() || !checked_path.exists() || !genres_path.exists() {
        return Ok(PortableSnapshotStatus {
            exists: false,
            path: dir.to_string_lossy().into_owned(),
            generated_at: None,
            checked_rows: 0,
            genre_rows: 0,
            lastfm_checked_rows: 0,
            context_tag_rows: 0,
        });
    }

    let manifest = parse_manifest(&manifest_path)?;
    Ok(PortableSnapshotStatus {
        exists: true,
        path: dir.to_string_lossy().into_owned(),
        generated_at: Some(manifest.generated_at),
        checked_rows: manifest.checked_rows,
        genre_rows: manifest.genre_rows,
        lastfm_checked_rows: manifest.lastfm_checked_rows,
        context_tag_rows: manifest.context_tag_rows,
    })
}

pub fn export_portable_snapshot(conn: &Connection) -> Result<PortableSnapshotExportResult> {
    let dir = snapshot_dir();
    fs::create_dir_all(&dir)?;

    let checked_path = snapshot_checked_path();
    let lastfm_checked_path = snapshot_lastfm_checked_path();
    let genres_path = snapshot_genres_path();
    let context_tags_path = snapshot_context_tags_path();
    let manifest_path = snapshot_manifest_path();

    let mut checked_rows = 0usize;
    {
        let file = File::create(&checked_path)?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "tidal_id")?;
        let mut stmt = conn.prepare(
            "SELECT t.tidal_id
             FROM musicbrainz_checked mc
             JOIN tracks t ON t.id = mc.track_id
             WHERE t.tidal_id IS NOT NULL
             ORDER BY t.tidal_id ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        for row in rows {
            writeln!(writer, "{}", row?)?;
            checked_rows += 1;
        }
        writer.flush()?;
    }

    let mut lastfm_checked_rows = 0usize;
    {
        let file = File::create(&lastfm_checked_path)?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "tidal_id")?;
        let mut stmt = conn.prepare(
            "SELECT t.tidal_id
             FROM lastfm_checked lc
             JOIN tracks t ON t.id = lc.track_id
             WHERE t.tidal_id IS NOT NULL
             ORDER BY t.tidal_id ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        for row in rows {
            writeln!(writer, "{}", row?)?;
            lastfm_checked_rows += 1;
        }
        writer.flush()?;
    }

    let mut genre_rows = 0usize;
    {
        let file = File::create(&genres_path)?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "tidal_id,source,genre_slug,confidence")?;
        let mut stmt = conn.prepare(
            "SELECT t.tidal_id, tg.source, g.slug, tg.confidence
             FROM track_genres tg
             JOIN tracks t ON t.id = tg.track_id
             JOIN genres g ON g.id = tg.genre_id
             WHERE tg.source IN ('musicbrainz', 'lastfm')
               AND t.tidal_id IS NOT NULL
             ORDER BY t.tidal_id ASC, tg.source ASC, g.slug ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        for row in rows {
            let (tidal_id, source, genre_slug, confidence) = row?;
            writeln!(
                writer,
                "{tidal_id},{},{},{}",
                csv_escape_field(&source),
                csv_escape_field(&genre_slug),
                confidence
            )?;
            genre_rows += 1;
        }
        writer.flush()?;
    }

    let mut context_tag_rows = 0usize;
    {
        let file = File::create(&context_tags_path)?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "tidal_id,tag,normalized_tag,context,confidence")?;
        let mut stmt = conn.prepare(
            "SELECT t.tidal_id, tct.tag, tct.normalized_tag, tct.context, tct.confidence
             FROM track_context_tags tct
             JOIN tracks t ON t.id = tct.track_id
             WHERE tct.source = 'lastfm'
               AND t.tidal_id IS NOT NULL
             ORDER BY t.tidal_id ASC, tct.context ASC, tct.normalized_tag ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?;
        for row in rows {
            let (tidal_id, tag, normalized_tag, context, confidence) = row?;
            writeln!(
                writer,
                "{tidal_id},{},{},{},{}",
                csv_escape_field(&tag),
                csv_escape_field(&normalized_tag),
                csv_escape_field(&context),
                confidence
            )?;
            context_tag_rows += 1;
        }
        writer.flush()?;
    }

    let manifest = PortableSnapshotManifest {
        generated_at: chrono::Utc::now().to_rfc3339(),
        db_path: crate::paths::resolve_db_path_from_env()
            .to_string_lossy()
            .into_owned(),
        checked_rows,
        genre_rows,
        lastfm_checked_rows,
        context_tag_rows,
        files: PortableSnapshotFiles {
            checked: PORTABLE_CHECKED_FILE.to_string(),
            genres: PORTABLE_GENRES_FILE.to_string(),
            lastfm_checked: Some(PORTABLE_LASTFM_CHECKED_FILE.to_string()),
            context_tags: Some(PORTABLE_CONTEXT_TAGS_FILE.to_string()),
        },
    };
    let manifest_file = File::create(&manifest_path)?;
    serde_json::to_writer_pretty(BufWriter::new(manifest_file), &manifest)?;

    Ok(PortableSnapshotExportResult {
        status: PortableSnapshotStatus {
            exists: true,
            path: dir.to_string_lossy().into_owned(),
            generated_at: Some(manifest.generated_at),
            checked_rows,
            genre_rows,
            lastfm_checked_rows,
            context_tag_rows,
        },
    })
}

pub fn import_portable_snapshot(conn: &Connection) -> Result<PortableSnapshotImportResult> {
    let manifest_path = snapshot_manifest_path();
    let checked_path = snapshot_checked_path();
    let lastfm_checked_path = snapshot_lastfm_checked_path();
    let genres_path = snapshot_genres_path();
    let context_tags_path = snapshot_context_tags_path();
    if !manifest_path.exists() || !checked_path.exists() || !genres_path.exists() {
        anyhow::bail!(
            "No portable MusicBrainz snapshot was found at {}",
            snapshot_dir().to_string_lossy()
        );
    }

    let track_map = {
        let mut stmt =
            conn.prepare("SELECT tidal_id, id FROM tracks WHERE tidal_id IS NOT NULL")?;
        stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?
    };
    let genre_map = {
        let mut stmt = conn.prepare("SELECT slug, id FROM genres")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?
    };

    let tx = conn.unchecked_transaction()?;
    let mut insert_checked =
        tx.prepare("INSERT OR IGNORE INTO musicbrainz_checked (track_id) VALUES (?1)")?;
    let mut insert_lastfm_checked =
        tx.prepare("INSERT OR IGNORE INTO lastfm_checked (track_id) VALUES (?1)")?;
    let mut insert_genre = tx.prepare(
        "INSERT OR IGNORE INTO track_genres (track_id, genre_id, source, confidence)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut insert_context_tag = tx.prepare(
        "INSERT INTO track_context_tags
             (track_id, tag, normalized_tag, context, source, confidence)
         VALUES (?1, ?2, ?3, ?4, 'lastfm', ?5)
         ON CONFLICT(track_id, normalized_tag, context, source) DO UPDATE SET
             confidence = MAX(confidence, excluded.confidence)",
    )?;

    let mut checked_inserted = 0usize;
    let mut checked_skipped = 0usize;
    for (line_number, line) in BufReader::new(File::open(&checked_path)?)
        .lines()
        .enumerate()
    {
        let line = line?;
        if line_number == 0 || line.trim().is_empty() {
            continue;
        }
        let tidal_id: i64 = line
            .trim()
            .parse()
            .with_context(|| format!("Invalid tidal_id in {}", checked_path.display()))?;
        let Some(track_id) = track_map.get(&tidal_id) else {
            checked_skipped += 1;
            continue;
        };
        checked_inserted += insert_checked.execute(params![track_id])?;
    }

    let mut lastfm_checked_inserted = 0usize;
    let mut lastfm_checked_skipped = 0usize;
    if lastfm_checked_path.exists() {
        for (line_number, line) in BufReader::new(File::open(&lastfm_checked_path)?)
            .lines()
            .enumerate()
        {
            let line = line?;
            if line_number == 0 || line.trim().is_empty() {
                continue;
            }
            let tidal_id: i64 = line.trim().parse().with_context(|| {
                format!("Invalid tidal_id in {}", lastfm_checked_path.display())
            })?;
            let Some(track_id) = track_map.get(&tidal_id) else {
                lastfm_checked_skipped += 1;
                continue;
            };
            lastfm_checked_inserted += insert_lastfm_checked.execute(params![track_id])?;
        }
    }

    let mut genre_inserted = 0usize;
    let mut track_skipped = 0usize;
    let mut genre_skipped = 0usize;
    for (line_number, line) in BufReader::new(File::open(&genres_path)?)
        .lines()
        .enumerate()
    {
        let line = line?;
        if line_number == 0 || line.trim().is_empty() {
            continue;
        }
        let parts = parse_csv_line(&line)?;
        let (tidal_id_raw, source, genre_slug, confidence_raw) = match parts.as_slice() {
            [tidal_id, genre_slug, confidence] => (
                tidal_id.as_str(),
                "musicbrainz",
                genre_slug.as_str(),
                confidence.as_str(),
            ),
            [tidal_id, source, genre_slug, confidence] => (
                tidal_id.as_str(),
                require_source(source)?,
                genre_slug.as_str(),
                confidence.as_str(),
            ),
            _ => anyhow::bail!("Invalid portable MusicBrainz genre snapshot row"),
        };
        let tidal_id: i64 = tidal_id_raw
            .parse()
            .with_context(|| format!("Invalid tidal_id in {}", genres_path.display()))?;
        let confidence: f64 = confidence_raw
            .parse()
            .with_context(|| format!("Invalid confidence in {}", genres_path.display()))?;

        let Some(track_id) = track_map.get(&tidal_id) else {
            track_skipped += 1;
            continue;
        };
        let Some(genre_id) = genre_map.get(genre_slug) else {
            genre_skipped += 1;
            continue;
        };
        genre_inserted += insert_genre.execute(params![track_id, genre_id, source, confidence])?;
    }

    let mut context_tag_inserted = 0usize;
    let mut context_tag_skipped = 0usize;
    if context_tags_path.exists() {
        for (line_number, line) in BufReader::new(File::open(&context_tags_path)?)
            .lines()
            .enumerate()
        {
            let line = line?;
            if line_number == 0 || line.trim().is_empty() {
                continue;
            }
            let parts = parse_csv_line(&line)?;
            let [tidal_id_raw, tag, normalized_tag, context, confidence_raw] = parts.as_slice()
            else {
                anyhow::bail!("Invalid Last.fm context tag snapshot row");
            };
            let tidal_id: i64 = tidal_id_raw
                .parse()
                .with_context(|| format!("Invalid tidal_id in {}", context_tags_path.display()))?;
            let confidence: f64 = confidence_raw.parse().with_context(|| {
                format!("Invalid confidence in {}", context_tags_path.display())
            })?;
            let Some(track_id) = track_map.get(&tidal_id) else {
                context_tag_skipped += 1;
                continue;
            };
            context_tag_inserted += insert_context_tag.execute(params![
                track_id,
                tag,
                normalized_tag,
                context,
                confidence
            ])?;
        }
    }
    drop(insert_context_tag);
    drop(insert_genre);
    drop(insert_lastfm_checked);
    drop(insert_checked);
    tx.commit()?;

    let status = read_portable_snapshot_status()?;
    Ok(PortableSnapshotImportResult {
        status,
        checked_inserted,
        checked_skipped,
        lastfm_checked_inserted,
        lastfm_checked_skipped,
        genre_inserted,
        track_skipped,
        genre_skipped,
        context_tag_inserted,
        context_tag_skipped,
    })
}

#[cfg(test)]
mod portable_snapshot_tests {
    use super::*;
    use std::{
        fs,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn snapshot_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_db_path(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("noorwave-{name}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("noor.db")
    }

    fn set_snapshot_db_path(path: &Path) {
        unsafe {
            std::env::set_var("NOOR_DB", path);
        }
    }

    fn clear_snapshot_db_path() {
        unsafe {
            std::env::remove_var("NOOR_DB");
        }
    }

    fn create_snapshot_schema(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                tidal_id INTEGER
            );
            CREATE TABLE genres (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL
            );
            CREATE TABLE musicbrainz_checked (
                track_id INTEGER PRIMARY KEY
            );
            CREATE TABLE lastfm_checked (
                track_id INTEGER PRIMARY KEY
            );
            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT DEFAULT 'tidal',
                confidence REAL DEFAULT 1.0,
                PRIMARY KEY (track_id, genre_id)
            );
            CREATE TABLE track_context_tags (
                track_id INTEGER NOT NULL,
                tag TEXT NOT NULL,
                normalized_tag TEXT NOT NULL,
                context TEXT NOT NULL,
                source TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.5,
                PRIMARY KEY (track_id, normalized_tag, context, source)
            );
            "#,
        )
        .unwrap();
    }

    fn seed_snapshot_source(conn: &Connection) {
        conn.execute_batch(
            r#"
            INSERT INTO tracks (id, tidal_id) VALUES (1, 101), (2, 202), (3, NULL);
            INSERT INTO genres (id, name, slug)
            VALUES (1, 'Ambient', 'ambient'), (2, 'Dream Pop', 'dream-pop');
            INSERT INTO musicbrainz_checked (track_id) VALUES (1);
            INSERT INTO lastfm_checked (track_id) VALUES (2), (3);
            INSERT INTO track_genres (track_id, genre_id, source, confidence)
            VALUES (1, 1, 'musicbrainz', 0.91), (2, 2, 'lastfm', 0.42);
            INSERT INTO track_context_tags
                (track_id, tag, normalized_tag, context, source, confidence)
            VALUES (2, 'late night', 'late night', 'time_of_day', 'lastfm', 0.64);
            "#,
        )
        .unwrap();
    }

    #[test]
    fn portable_snapshot_export_writes_lastfm_checked_genres_and_context_tags() {
        let _guard = snapshot_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let db_path = temp_db_path("portable-export");
        set_snapshot_db_path(&db_path);

        let conn = Connection::open_in_memory().unwrap();
        create_snapshot_schema(&conn);
        seed_snapshot_source(&conn);

        let result = export_portable_snapshot(&conn).unwrap();
        let dir = snapshot_dir();

        let genre_csv = fs::read_to_string(dir.join(PORTABLE_GENRES_FILE)).unwrap();
        assert!(genre_csv.contains("tidal_id,source,genre_slug,confidence"));
        assert!(genre_csv.contains("101,musicbrainz,ambient,0.91"));
        assert!(genre_csv.contains("202,lastfm,dream-pop,0.42"));

        let lastfm_checked_csv = fs::read_to_string(dir.join("lastfm_checked.csv")).unwrap();
        assert_eq!(lastfm_checked_csv, "tidal_id\n202\n");

        let context_csv = fs::read_to_string(dir.join("lastfm_context_tags.csv")).unwrap();
        assert!(context_csv.contains("tidal_id,tag,normalized_tag,context,confidence"));
        assert!(context_csv.contains("202,late night,late night,time_of_day,0.64"));

        assert_eq!(result.status.checked_rows, 1);
        assert_eq!(result.status.genre_rows, 2);

        clear_snapshot_db_path();
        fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
    }

    #[test]
    fn portable_snapshot_import_restores_lastfm_checked_genres_and_context_tags() {
        let _guard = snapshot_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let db_path = temp_db_path("portable-import");
        set_snapshot_db_path(&db_path);

        let source = Connection::open_in_memory().unwrap();
        create_snapshot_schema(&source);
        seed_snapshot_source(&source);
        export_portable_snapshot(&source).unwrap();

        let target = Connection::open_in_memory().unwrap();
        create_snapshot_schema(&target);
        target
            .execute_batch(
                r#"
                INSERT INTO tracks (id, tidal_id) VALUES (10, 101), (20, 202);
                INSERT INTO genres (id, name, slug)
                VALUES (10, 'Ambient', 'ambient'), (20, 'Dream Pop', 'dream-pop');
                "#,
            )
            .unwrap();

        let imported = import_portable_snapshot(&target).unwrap();

        let musicbrainz_checked: i64 = target
            .query_row("SELECT COUNT(*) FROM musicbrainz_checked", [], |row| {
                row.get(0)
            })
            .unwrap();
        let lastfm_checked: i64 = target
            .query_row("SELECT COUNT(*) FROM lastfm_checked", [], |row| row.get(0))
            .unwrap();
        let lastfm_genres: i64 = target
            .query_row(
                "SELECT COUNT(*) FROM track_genres WHERE source = 'lastfm'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let context_tags: i64 = target
            .query_row("SELECT COUNT(*) FROM track_context_tags", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(musicbrainz_checked, 1);
        assert_eq!(lastfm_checked, 1);
        assert_eq!(lastfm_genres, 1);
        assert_eq!(context_tags, 1);
        assert_eq!(imported.checked_inserted, 1);
        assert_eq!(imported.genre_inserted, 2);

        clear_snapshot_db_path();
        fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
    }
}

// ── Main enrichment runner ────────────────────────────────────────────────────

/// Run the full enrichment job. Calls `on_progress` every `report_every` tracks.
/// Respects MusicBrainz 1 req/sec rate limit.
pub async fn run_enrichment(
    state: crate::SharedState,
    client: reqwest::Client,
    on_progress: impl Fn(EnrichmentProgress) + Send + 'static,
    report_every: usize,
) -> Result<EnrichmentProgress> {
    let total = {
        let g = state.read().await;
        g.db.with_conn(count_unenriched_tracks)?
    };
    info!("MusicBrainz enrichment: {total} tracks to process");

    let mut processed = 0;
    let mut genres_assigned = 0;
    let mut last_request = Instant::now()
        .checked_sub(MIN_REQUEST_INTERVAL)
        .unwrap_or(Instant::now());
    let batch_size = 200;

    loop {
        // Re-query from offset 0 each iteration — rows move out of the set as they're marked checked.
        let batch = {
            let g = state.read().await;
            g.db.with_conn(|conn| load_unenriched_tracks(conn, batch_size, 0))?
        };
        if batch.is_empty() {
            break;
        }

        for track in batch {
            // Rate limit.
            let elapsed = last_request.elapsed();
            if elapsed < MIN_REQUEST_INTERVAL {
                tokio::time::sleep(MIN_REQUEST_INTERVAL - elapsed).await;
            }
            last_request = Instant::now();

            let genres = if let Some(isrc) = &track.isrc {
                match fetch_genres_by_isrc(&client, isrc).await {
                    Ok(g) => g,
                    Err(err) => {
                        warn!(
                            "MusicBrainz ISRC lookup failed for track {}: {err:#}",
                            track.id
                        );
                        vec![]
                    }
                }
            } else if let Some(artist) = &track.artist_name {
                match fetch_genres_by_title(&client, artist, &track.title).await {
                    Ok(g) => g,
                    Err(err) => {
                        warn!(
                            "MusicBrainz title lookup failed for track {}: {err:#}",
                            track.id
                        );
                        vec![]
                    }
                }
            } else {
                // No ISRC and no artist name — mark checked with no genres.
                let g = state.read().await;
                if let Err(err) = g.db.with_conn(|conn| mark_checked(conn, track.id)) {
                    warn!("Failed to mark track {} as MB checked: {err:#}", track.id);
                }
                processed += 1;
                continue;
            };

            let inserted = {
                let g = state.read().await;
                // write_genres calls mark_checked internally, so it's always recorded.
                g.db.with_conn(|conn| write_genres(conn, track.id, &genres))?
            };
            genres_assigned += inserted;
            processed += 1;

            if processed % report_every == 0 {
                on_progress(EnrichmentProgress {
                    processed,
                    total,
                    genres_assigned,
                });
            }
        }
    }

    let final_progress = EnrichmentProgress {
        processed,
        total,
        genres_assigned,
    };
    on_progress(final_progress.clone());
    info!(
        "MusicBrainz enrichment complete: {} tracks processed, {} genre assignments",
        processed, genres_assigned
    );
    Ok(final_progress)
}
