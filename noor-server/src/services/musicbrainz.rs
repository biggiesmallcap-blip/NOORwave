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
const PORTABLE_GENRES_FILE: &str = "musicbrainz_genres.csv";
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
    genre_names: &[String],
    confidence: f64,
) -> Result<usize> {
    // Always mark as checked first so we never re-query this track.
    mark_checked(conn, track_id)?;

    let mut inserted = 0;
    for name in genre_names {
        let normalized = name.trim().to_ascii_lowercase();
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
                params![track_id, gid, confidence],
            )?;
            inserted += n;
        }
    }
    Ok(inserted)
}

// ── HTTP lookup ───────────────────────────────────────────────────────────────

/// Fetch genre tags for a single ISRC from MusicBrainz.
/// Uses the search endpoint with inc=tags (release-groups is a lookup-only inc).
pub async fn fetch_genres_by_isrc(client: &reqwest::Client, isrc: &str) -> Result<Vec<String>> {
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
) -> Result<Vec<String>> {
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

fn extract_tags(data: MbRecordingSearch) -> Vec<String> {
    let Some(rec) = data.recordings.into_iter().next() else {
        return vec![];
    };

    let mut tags: Vec<(i32, String)> = rec.tags.into_iter().map(|t| (t.count, t.name)).collect();
    // Sort highest count first, deduplicate, cap at 5.
    tags.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    tags.into_iter()
        .filter_map(|(_, name)| {
            let key = name.to_ascii_lowercase();
            if seen.insert(key) { Some(name) } else { None }
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
    pub genres_assigned: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PortableSnapshotFiles {
    checked: String,
    genres: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PortableSnapshotManifest {
    generated_at: String,
    db_path: String,
    checked_rows: usize,
    genre_rows: usize,
    files: PortableSnapshotFiles,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PortableSnapshotStatus {
    pub exists: bool,
    pub path: String,
    pub generated_at: Option<String>,
    pub checked_rows: usize,
    pub genre_rows: usize,
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
    pub genre_inserted: usize,
    pub track_skipped: usize,
    pub genre_skipped: usize,
}

fn resolve_db_path() -> PathBuf {
    if let Ok(path) = std::env::var("NOOR_DB") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return path;
        }
        return std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path);
    }

    std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()?
                .parent()?
                .parent()
                .map(|root| root.join("noor.db"))
        })
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("noor.db")
        })
}

fn snapshot_dir() -> PathBuf {
    resolve_db_path()
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

fn snapshot_genres_path() -> PathBuf {
    snapshot_dir().join(PORTABLE_GENRES_FILE)
}

fn parse_manifest(path: &Path) -> Result<PortableSnapshotManifest> {
    let file = File::open(path)?;
    Ok(serde_json::from_reader(BufReader::new(file))?)
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
        });
    }

    let manifest = parse_manifest(&manifest_path)?;
    Ok(PortableSnapshotStatus {
        exists: true,
        path: dir.to_string_lossy().into_owned(),
        generated_at: Some(manifest.generated_at),
        checked_rows: manifest.checked_rows,
        genre_rows: manifest.genre_rows,
    })
}

pub fn export_portable_snapshot(conn: &Connection) -> Result<PortableSnapshotExportResult> {
    let dir = snapshot_dir();
    fs::create_dir_all(&dir)?;

    let checked_path = snapshot_checked_path();
    let genres_path = snapshot_genres_path();
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

    let mut genre_rows = 0usize;
    {
        let file = File::create(&genres_path)?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "tidal_id,genre_slug,confidence")?;
        let mut stmt = conn.prepare(
            "SELECT t.tidal_id, g.slug, tg.confidence
             FROM track_genres tg
             JOIN tracks t ON t.id = tg.track_id
             JOIN genres g ON g.id = tg.genre_id
             WHERE tg.source = 'musicbrainz'
               AND t.tidal_id IS NOT NULL
             ORDER BY t.tidal_id ASC, g.slug ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;
        for row in rows {
            let (tidal_id, genre_slug, confidence) = row?;
            writeln!(writer, "{tidal_id},{genre_slug},{confidence}")?;
            genre_rows += 1;
        }
        writer.flush()?;
    }

    let manifest = PortableSnapshotManifest {
        generated_at: chrono::Utc::now().to_rfc3339(),
        db_path: resolve_db_path().to_string_lossy().into_owned(),
        checked_rows,
        genre_rows,
        files: PortableSnapshotFiles {
            checked: PORTABLE_CHECKED_FILE.to_string(),
            genres: PORTABLE_GENRES_FILE.to_string(),
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
        },
    })
}

pub fn import_portable_snapshot(conn: &Connection) -> Result<PortableSnapshotImportResult> {
    let manifest_path = snapshot_manifest_path();
    let checked_path = snapshot_checked_path();
    let genres_path = snapshot_genres_path();
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
    let mut insert_genre = tx.prepare(
        "INSERT OR IGNORE INTO track_genres (track_id, genre_id, source, confidence)
         VALUES (?1, ?2, 'musicbrainz', ?3)",
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
        checked_inserted += insert_checked.execute(params![track_id])? as usize;
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
        let mut parts = line.splitn(3, ',');
        let tidal_id: i64 = parts
            .next()
            .context("Missing tidal_id in portable MusicBrainz genre snapshot")?
            .parse()
            .with_context(|| format!("Invalid tidal_id in {}", genres_path.display()))?;
        let genre_slug = parts
            .next()
            .context("Missing genre_slug in portable MusicBrainz genre snapshot")?;
        let confidence: f64 = parts
            .next()
            .context("Missing confidence in portable MusicBrainz genre snapshot")?
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
        genre_inserted += insert_genre.execute(params![track_id, genre_id, confidence])? as usize;
    }
    drop(insert_genre);
    drop(insert_checked);
    tx.commit()?;

    let status = read_portable_snapshot_status()?;
    Ok(PortableSnapshotImportResult {
        status,
        checked_inserted,
        checked_skipped,
        genre_inserted,
        track_skipped,
        genre_skipped,
    })
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

            let confidence = if track.isrc.is_some() { 0.85 } else { 0.55 };
            let inserted = {
                let g = state.read().await;
                // write_genres calls mark_checked internally, so it's always recorded.
                g.db.with_conn(|conn| write_genres(conn, track.id, &genres, confidence))?
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
