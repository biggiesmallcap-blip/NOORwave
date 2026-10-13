//! Atlas Picture: every map artist takes the dominant colour of its own
//! video artwork. One 80 px thumbnail per artist (the artwork of its best
//! programmable cut) is fetched slowly in the background from TIDAL's image
//! CDN, decoded and reduced to one 0xRRGGBB.
//!
//! Seeding follows the map: after every intel build each artist on it gets
//! a row with its picked video, an artist whose pick changed goes back to
//! pending and artists that left the map lose their row. The fetch pace is
//! the discovery setting's (full about 4 per second, limited one per 2 s,
//! off none). A thumbnail the CDN refuses or that does not decode spends a
//! try; after three the row is failed. When the CDN cannot be reached at all
//! the job waits instead of spending tries. Nothing here runs on a request
//! path: GET /api/videos/atlas/pictures only reads the table.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use super::job;
use super::scenes::VARIOUS_ARTISTS_ID;
use crate::SharedState;
use crate::db::Database;
use crate::services::artwork_cache;
use crate::services::video_discovery::setting::{self as discovery_setting, DiscoverySetting};

/// Tries before a row is failed for good.
pub const MAX_TRIES: i64 = 3;
const THUMB_SIZE: u32 = 80;
const BOOT_DELAY: Duration = Duration::from_secs(180);
/// Nothing to do, discovery off, or a build running: look again after this.
const IDLE: Duration = Duration::from_secs(60);
const BUILD_WAIT: Duration = Duration::from_secs(30);
const BATCH: usize = 100;
const LOG_EVERY: u64 = 500;
const MAX_THUMB_BYTES: usize = 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

static RUNNING: AtomicBool = AtomicBool::new(false);

// --------------------------------------------------------------- colour

const HUE_BUCKETS: usize = 12;
const NEAR_BLACK_V: f32 = 0.12;
const NEAR_WHITE_S: f32 = 0.08;
const NEAR_WHITE_V: f32 = 0.92;
const COLOURFUL_S: f32 = 0.15;
/// Below this share of colourful pixels (of those kept) a picture is grey.
const MIN_COLOURFUL: f64 = 0.08;

/// Hue in degrees [0, 360), saturation and value in [0, 1].
fn hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (h, s, max)
}

fn pack(r: f64, g: f64, b: f64) -> u32 {
    let channel = |c: f64| c.round().clamp(0.0, 255.0) as u32;
    (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

/// Dominant colour of packed RGB pixels as 0xRRGGBB, None for no pixels.
///
/// Near black (letterbox bars, dark stages) and near white (title cards)
/// are ignored. The rest is bucketed by hue into 12 buckets weighted by
/// saturation x value; the heaviest bucket's pixels are averaged with the
/// same weights. When fewer than 8% of the kept pixels are colourful
/// (saturation 0.15 or more) the picture is black and white and gets the
/// mean grey of the kept pixels. A picture that is all black or all white
/// gets its plain mean.
pub fn dominant_colour(rgb: &[u8]) -> Option<u32> {
    let (pixels, _) = rgb.as_chunks::<3>();
    if pixels.is_empty() {
        return None;
    }
    let mut all = [0f64; 3];
    let mut kept = 0usize;
    let mut kept_grey = 0f64;
    let mut colourful = 0usize;
    // Per bucket: weight, then the weighted r, g, b sums.
    let mut buckets = [[0f64; 4]; HUE_BUCKETS];
    for &[r, g, b] in pixels {
        all[0] += f64::from(r);
        all[1] += f64::from(g);
        all[2] += f64::from(b);
        let (h, s, v) = hsv(r, g, b);
        if v < NEAR_BLACK_V || (s < NEAR_WHITE_S && v > NEAR_WHITE_V) {
            continue;
        }
        kept += 1;
        kept_grey += (f64::from(r) + f64::from(g) + f64::from(b)) / 3.0;
        if s >= COLOURFUL_S {
            colourful += 1;
        }
        // Buckets are centred on red, orange, yellow, ...: red spans 345..15.
        let bucket = (((h + 15.0) % 360.0) / 30.0) as usize % HUE_BUCKETS;
        let weight = f64::from(s * v);
        let entry = &mut buckets[bucket];
        entry[0] += weight;
        entry[1] += weight * f64::from(r);
        entry[2] += weight * f64::from(g);
        entry[3] += weight * f64::from(b);
    }
    let count = pixels.len() as f64;
    if kept == 0 {
        return Some(pack(all[0] / count, all[1] / count, all[2] / count));
    }
    if (colourful as f64) < kept as f64 * MIN_COLOURFUL {
        let grey = kept_grey / kept as f64;
        return Some(pack(grey, grey, grey));
    }
    let heaviest = buckets
        .iter()
        .max_by(|a, b| a[0].total_cmp(&b[0]))
        .filter(|bucket| bucket[0] > 0.0)?;
    Some(pack(
        heaviest[1] / heaviest[0],
        heaviest[2] / heaviest[0],
        heaviest[3] / heaviest[0],
    ))
}

/// Decode a JPEG thumbnail and take its dominant colour.
pub fn thumb_colour(bytes: &[u8]) -> Result<u32> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)?.to_rgb8();
    dominant_colour(image.as_raw()).ok_or_else(|| anyhow::anyhow!("empty thumbnail"))
}

// --------------------------------------------------------------- seeding

/// The image path of a TIDAL artwork URL without its size:
/// `https://resources.tidal.com/images/6a0ff657/cfc9/47f6/9270/c84d128eeb09/640x640.jpg`
/// gives `6a0ff657/cfc9/47f6/9270/c84d128eeb09`. Anything else is None.
pub fn art_path(url: &str) -> Option<String> {
    let path = artwork_cache::tidal_path_at_size(url, THUMB_SIZE)?;
    path.strip_suffix(&format!("/{THUMB_SIZE}x{THUMB_SIZE}.jpg"))
        .map(str::to_owned)
}

/// The thumbnail the job fetches for an image path.
pub fn thumb_url(art: &str) -> String {
    artwork_cache::tidal_url(&format!("{art}/{THUMB_SIZE}x{THUMB_SIZE}.jpg"))
}

/// Each map artist's picked video and its image path. The pick is the
/// artist's best programmable cut with artwork: official cuts first, then
/// the Atlas card's order (quality, popularity, newest id). An artist with
/// no such cut falls back to its best other video with artwork; artists
/// without any artwork, and Various Artists, are left out.
pub fn picks(conn: &Connection) -> Result<BTreeMap<i64, (i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT f.artist_tidal_id, f.tidal_video_id, json_extract(c.item_json, '$.artwork_url')
           FROM video_facets f
           JOIN video_artist_facets a ON a.artist_tidal_id = f.artist_tidal_id
           JOIN video_catalog c ON c.tidal_video_id = f.tidal_video_id
          WHERE a.x IS NOT NULL AND f.artist_tidal_id <> ?1
          ORDER BY f.artist_tidal_id, f.programmable DESC, f.cut = 'official' DESC,
                   f.quality DESC, COALESCE(f.popularity, -1) DESC, f.tidal_video_id DESC",
    )?;
    let mut rows = stmt.query([VARIOUS_ARTISTS_ID])?;
    let mut out = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let artist: i64 = row.get(0)?;
        if out.contains_key(&artist) {
            continue;
        }
        let url: Option<String> = row.get(2)?;
        if let Some(art) = url.as_deref().and_then(art_path) {
            out.insert(artist, (row.get(1)?, art));
        }
    }
    Ok(out)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SeedReport {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
    pub total: usize,
}

/// Bring the table in line with the map: one row per picked artist.
pub fn seed(conn: &Connection) -> Result<SeedReport> {
    let picks = picks(conn)?;
    let existing: HashMap<i64, (Option<i64>, Option<String>)> = conn
        .prepare("SELECT artist_tidal_id, video_id, art FROM video_artist_pictures")?
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
        .collect::<Result<_, _>>()?;
    let mut report = SeedReport {
        total: picks.len(),
        ..SeedReport::default()
    };
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    {
        let mut delete =
            tx.prepare("DELETE FROM video_artist_pictures WHERE artist_tidal_id = ?1")?;
        for id in existing.keys().filter(|id| !picks.contains_key(id)) {
            delete.execute([id])?;
            report.removed += 1;
        }
        let mut insert = tx.prepare(
            "INSERT INTO video_artist_pictures (artist_tidal_id, video_id, art) VALUES (?1, ?2, ?3)",
        )?;
        let mut repick = tx.prepare(
            "UPDATE video_artist_pictures
                SET video_id = ?2, art = ?3, rgb = NULL, status = 'pending', tries = 0,
                    fetched_at = NULL
              WHERE artist_tidal_id = ?1",
        )?;
        for (artist, (video, art)) in &picks {
            match existing.get(artist) {
                None => {
                    insert.execute(params![artist, video, art])?;
                    report.added += 1;
                }
                Some((old_video, old_art))
                    if *old_video != Some(*video) || old_art.as_deref() != Some(art) =>
                {
                    repick.execute(params![artist, video, art])?;
                    report.changed += 1;
                }
                Some(_) => {}
            }
        }
    }
    tx.commit()?;
    Ok(report)
}

/// The newest finished intel build, the map the table should follow.
fn latest_build(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT MAX(id) FROM video_intel_runs WHERE stage = 'build' AND finished_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

// --------------------------------------------------------------- the read

/// GET /api/videos/atlas/pictures: parallel arrays of the artists done so
/// far, by artist id.
#[derive(Debug, Default, Serialize)]
pub struct Pictures {
    pub ready: usize,
    pub total: usize,
    pub ids: Vec<i64>,
    pub rgb: Vec<u32>,
    pub art: Vec<String>,
}

pub fn read(conn: &Connection) -> Result<Pictures> {
    let mut out = Pictures::default();
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id, status = 'done' AND rgb IS NOT NULL AND art IS NOT NULL, rgb, art
           FROM video_artist_pictures ORDER BY artist_tidal_id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        out.total += 1;
        if row.get::<_, bool>(1)? {
            out.ids.push(row.get(0)?);
            out.rgb.push(row.get::<_, i64>(2)? as u32);
            out.art.push(row.get(3)?);
        }
    }
    out.ready = out.ids.len();
    Ok(out)
}

// --------------------------------------------------------------- the job

/// The wait between fetches for a discovery setting; None fetches nothing.
pub fn pace(setting: DiscoverySetting) -> Option<Duration> {
    match setting {
        DiscoverySetting::Full => Some(Duration::from_millis(250)),
        DiscoverySetting::Limited => Some(Duration::from_secs(2)),
        DiscoverySetting::Off => None,
    }
}

/// Why a thumbnail could not be had.
#[derive(Debug)]
pub enum FetchError {
    /// No usable answer (offline, timeout, rate limit, server error). The
    /// job waits; no try is spent.
    Unreachable(String),
    /// The CDN answered, but not with the image. Spends a try.
    Refused(String),
}

/// Fetches one thumbnail. The job's goes over HTTP; tests pass a fake.
pub trait Fetch {
    fn fetch(&self, url: &str) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send;
}

pub struct HttpFetch(pub reqwest::Client);

impl Fetch for HttpFetch {
    fn fetch(&self, url: &str) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send {
        let request = self.0.get(url).timeout(FETCH_TIMEOUT);
        async move {
            let response = request
                .send()
                .await
                .map_err(|err| FetchError::Unreachable(err.to_string()))?;
            let status = response.status();
            if status.as_u16() == 429 || status.is_server_error() {
                return Err(FetchError::Unreachable(status.to_string()));
            }
            if !status.is_success() {
                return Err(FetchError::Refused(status.to_string()));
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|err| FetchError::Unreachable(err.to_string()))?;
            if bytes.len() > MAX_THUMB_BYTES {
                return Err(FetchError::Refused(format!("{} bytes", bytes.len())));
            }
            Ok(bytes.to_vec())
        }
    }
}

/// One pending row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub artist_id: i64,
    pub art: String,
}

/// Up to `limit` pending rows, so the whole map colours in within
/// minutes: fewest tries first, then round-robin across scenes by rank in
/// scene (every scene's biggest artist, then every scene's second, ...),
/// ties by video count; uncharted artists (no scene) last.
pub fn pending(conn: &Connection, limit: usize) -> Result<Vec<Pending>> {
    let mut stmt = conn.prepare(
        "SELECT p.artist_tidal_id, p.art FROM video_artist_pictures p
           LEFT JOIN (SELECT artist_tidal_id, scene_id, videos,
                             ROW_NUMBER() OVER (PARTITION BY scene_id
                                                ORDER BY videos DESC, artist_tidal_id) AS rank
                        FROM video_artist_facets) f
             ON f.artist_tidal_id = p.artist_tidal_id
          WHERE p.status = 'pending' AND p.art IS NOT NULL
          ORDER BY p.tries, f.scene_id IS NULL, f.rank, COALESCE(f.videos, 0) DESC,
                   p.artist_tidal_id
          LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |row| {
            Ok(Pending {
                artist_id: row.get(0)?,
                art: row.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done(u32),
    /// Spent a try; still pending.
    Retry,
    /// Spent the last try.
    Failed,
    /// Re-seeded to another picture while the fetch ran; nothing stored.
    Stale,
}

/// Store one answered fetch: the colour when the thumbnail decodes, else
/// one more try. Guarded by `art` so a re-seed in between wins.
pub fn record(
    conn: &Connection,
    item: &Pending,
    fetched: Result<Vec<u8>, String>,
) -> Result<Outcome> {
    let colour = fetched.and_then(|bytes| thumb_colour(&bytes).map_err(|err| format!("{err:#}")));
    let outcome = match colour {
        Ok(rgb) => {
            let updated = conn.execute(
                "UPDATE video_artist_pictures
                    SET rgb = ?3, status = 'done', fetched_at = datetime('now')
                  WHERE artist_tidal_id = ?1 AND art = ?2 AND status = 'pending'",
                params![item.artist_id, item.art, rgb],
            )?;
            if updated == 0 {
                Outcome::Stale
            } else {
                Outcome::Done(rgb)
            }
        }
        Err(error) => {
            tracing::debug!(target: "noor.video_intel", artist = item.artist_id, %error, "picture fetch failed");
            let status: Option<String> = conn
                .query_row(
                    "UPDATE video_artist_pictures
                        SET tries = tries + 1, fetched_at = datetime('now'),
                            status = CASE WHEN tries + 1 >= ?3 THEN 'failed' ELSE 'pending' END
                      WHERE artist_tidal_id = ?1 AND art = ?2 AND status = 'pending'
                  RETURNING status",
                    params![item.artist_id, item.art, MAX_TRIES],
                    |row| row.get(0),
                )
                .optional()?;
            match status.as_deref() {
                None => Outcome::Stale,
                Some("failed") => Outcome::Failed,
                Some(_) => Outcome::Retry,
            }
        }
    };
    Ok(outcome)
}

/// (done, pending, failed) rows, for the progress log.
fn status_counts(conn: &Connection) -> Result<(i64, i64, i64)> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FILTER (WHERE status = 'done'),
                COUNT(*) FILTER (WHERE status = 'pending'),
                COUNT(*) FILTER (WHERE status = 'failed')
           FROM video_artist_pictures",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?)
}

/// The job's own connection, moved onto a blocking thread for each use.
/// In-memory test databases fall back to the shared connection.
struct JobDb {
    db: Database,
    conn: Option<Connection>,
}

impl JobDb {
    fn new(db: Database) -> Self {
        Self { db, conn: None }
    }

    async fn run<T, F>(&mut self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let db = self.db.clone();
        let conn = self.conn.take();
        let (conn, out) = tokio::task::spawn_blocking(move || {
            let conn = conn.or_else(|| db.open_isolated().ok());
            let out = match &conn {
                Some(conn) => f(conn),
                None => db.with_conn(f),
            };
            (conn, out)
        })
        .await?;
        self.conn = conn;
        out
    }
}

/// The background worker: what it seeded from and how it is doing.
pub struct Worker<F> {
    db: JobDb,
    fetch: F,
    /// `pace` and `job::is_running` outside tests, which skip the waits
    /// and never see a build.
    pace: fn(DiscoverySetting) -> Option<Duration>,
    building: fn() -> bool,
    seeded_build: Option<i64>,
    done: u64,
    unreachable: u32,
}

impl<F: Fetch> Worker<F> {
    pub fn new(db: Database, fetch: F) -> Self {
        Self {
            db: JobDb::new(db),
            fetch,
            pace,
            building: job::is_running,
            seeded_build: None,
            done: 0,
            unreachable: 0,
        }
    }

    /// Re-seed when a build finished since the last seed.
    async fn follow_builds(&mut self) -> Result<()> {
        let seeded = self.seeded_build;
        let seeded_now = self
            .db
            .run(move |conn| {
                let Some(build) = latest_build(conn)? else {
                    return Ok(None);
                };
                if seeded == Some(build) {
                    return Ok(None);
                }
                Ok(Some((build, seed(conn)?)))
            })
            .await?;
        if let Some((build, report)) = seeded_now {
            self.seeded_build = Some(build);
            tracing::info!(
                target: "noor.video_intel",
                build,
                added = report.added,
                changed = report.changed,
                removed = report.removed,
                total = report.total,
                "atlas pictures seeded"
            );
        }
        Ok(())
    }

    /// One round: follow the map, then fetch a batch at the setting's pace.
    /// Returns how long to wait before the next round.
    pub async fn tick(&mut self) -> Result<Duration> {
        if (self.building)() {
            return Ok(BUILD_WAIT);
        }
        // Seeding is local work: the table follows the map even with
        // discovery off.
        self.follow_builds().await?;
        let setting = self.db.run(discovery_setting::load).await?;
        if (self.pace)(setting).is_none() {
            return Ok(IDLE);
        }
        let batch = self.db.run(|conn| pending(conn, BATCH)).await?;
        if batch.is_empty() {
            return Ok(IDLE);
        }
        for item in batch {
            if (self.building)() {
                return Ok(BUILD_WAIT);
            }
            let fetched = match self.fetch.fetch(&thumb_url(&item.art)).await {
                Ok(bytes) => Ok(bytes),
                Err(FetchError::Refused(reason)) => Err(reason),
                Err(FetchError::Unreachable(reason)) => {
                    self.unreachable += 1;
                    tracing::debug!(target: "noor.video_intel", %reason, "picture CDN unreachable");
                    // 1, 2, ... 10 minutes while it stays down.
                    return Ok(IDLE * self.unreachable.min(10));
                }
            };
            self.unreachable = 0;
            let log = self.done + 1;
            let (outcome, setting, counts) = self
                .db
                .run(move |conn| {
                    let outcome = record(conn, &item, fetched)?;
                    let counts = (matches!(outcome, Outcome::Done(_))
                        && log.is_multiple_of(LOG_EVERY))
                    .then(|| status_counts(conn))
                    .transpose()?;
                    Ok((outcome, discovery_setting::load(conn)?, counts))
                })
                .await?;
            if matches!(outcome, Outcome::Done(_)) {
                self.done += 1;
                if let Some((ready, pending, failed)) = counts {
                    tracing::info!(
                        target: "noor.video_intel",
                        done = self.done,
                        ready,
                        pending,
                        failed,
                        "atlas pictures progress"
                    );
                }
            }
            // The setting is read after every fetch, so turning discovery
            // off stops the job at once.
            let Some(spacing) = (self.pace)(setting) else {
                return Ok(IDLE);
            };
            tokio::time::sleep(spacing).await;
        }
        Ok(Duration::ZERO)
    }
}

/// Start the background picture job once.
pub fn spawn(state: SharedState) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(BOOT_DELAY).await;
        let (db, client) = {
            let s = state.read().await;
            (s.db.clone(), s.http_client.clone())
        };
        let mut worker = Worker::new(db, HttpFetch(client));
        loop {
            let wait = match worker.tick().await {
                Ok(wait) => wait,
                Err(error) => {
                    tracing::warn!(target: "noor.video_intel", error = %format!("{error:#}"), "atlas pictures round failed");
                    IDLE
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::services::video_intel::build::build_all;
    use crate::services::video_intel::songs::tests::conn;

    fn frame(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut out = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&paint(x, y));
            }
        }
        out
    }

    fn rgb(colour: u32) -> [u8; 3] {
        [(colour >> 16) as u8, (colour >> 8) as u8, colour as u8]
    }

    fn jpeg(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let image = image::RgbImage::from_raw(width, height, frame(width, height, paint)).unwrap();
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_letterboxed_red_frame_is_red() {
        // Black bars top and bottom take 40% of the picture.
        let pixels = frame(80, 80, |_, y| {
            if !(16..64).contains(&y) {
                [0, 0, 0]
            } else {
                [200, 30, 40]
            }
        });
        assert_eq!(dominant_colour(&pixels), Some(0xC81E28));
    }

    #[test]
    fn a_greyscale_frame_stays_grey() {
        let pixels = frame(80, 80, |x, _| {
            let v = (40 + x * 2) as u8;
            [v, v, v]
        });
        let colour = rgb(dominant_colour(&pixels).unwrap());
        assert_eq!(colour[0], colour[1]);
        assert_eq!(colour[1], colour[2]);
        // Mean of 40..198 in steps of 2.
        assert_eq!(colour[0], 119);
    }

    #[test]
    fn a_white_card_with_a_small_blue_logo_is_blue() {
        // A 16 x 12 logo: 3% of the card.
        let pixels = frame(80, 80, |x, y| {
            if (32..48).contains(&x) && (34..46).contains(&y) {
                [20, 60, 220]
            } else {
                [250, 250, 248]
            }
        });
        assert_eq!(dominant_colour(&pixels), Some(0x143CDC));
    }

    #[test]
    fn a_few_colourful_pixels_on_a_grey_picture_keep_it_grey() {
        let pixels = frame(80, 80, |x, y| {
            if x < 4 && y < 4 {
                [255, 0, 0]
            } else {
                [100, 100, 100]
            }
        });
        assert_eq!(dominant_colour(&pixels), Some(0x646464));
    }

    #[test]
    fn the_heaviest_hue_wins_and_red_wraps_around_zero() {
        // Two reds either side of hue 0 share a bucket and outweigh a
        // larger but paler green.
        let pixels = frame(80, 80, |x, _| match x {
            0..20 => [220, 20, 40],
            20..40 => [220, 40, 20],
            _ => [150, 190, 150],
        });
        let colour = rgb(dominant_colour(&pixels).unwrap());
        assert!(
            colour[0] > 200 && colour[1] < 50 && colour[2] < 50,
            "{colour:?}"
        );
        assert_eq!(dominant_colour(&[]), None);
        assert_eq!(dominant_colour(&[0, 0, 0, 0, 0, 0]), Some(0));
    }

    #[test]
    fn a_jpeg_thumbnail_decodes_to_its_colour() {
        let bytes = jpeg(80, 80, |_, y| {
            if !(10..70).contains(&y) {
                [0, 0, 0]
            } else {
                [30, 160, 60]
            }
        });
        let colour = rgb(thumb_colour(&bytes).unwrap());
        assert!(
            colour[1] > 140 && colour[0] < 60 && colour[2] < 90,
            "{colour:?}"
        );
        assert!(thumb_colour(b"not a jpeg").is_err());
    }

    #[test]
    fn art_paths_drop_the_size_and_refuse_other_urls() {
        assert_eq!(
            art_path("https://resources.tidal.com/images/6a0ff657/cfc9/47f6/9270/c84d128eeb09/640x640.jpg")
                .as_deref(),
            Some("6a0ff657/cfc9/47f6/9270/c84d128eeb09")
        );
        assert_eq!(art_path("https://example.test/1.jpg"), None);
        assert_eq!(
            art_path("https://resources.tidal.com/images/nope/640x640.jpg"),
            None
        );
        assert_eq!(
            thumb_url("6a0ff657/cfc9/47f6/9270/c84d128eeb09"),
            "https://resources.tidal.com/images/6a0ff657/cfc9/47f6/9270/c84d128eeb09/80x80.jpg"
        );
    }

    #[test]
    fn pace_follows_the_discovery_setting() {
        assert_eq!(
            pace(DiscoverySetting::Full),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            pace(DiscoverySetting::Limited),
            Some(Duration::from_secs(2))
        );
        assert_eq!(pace(DiscoverySetting::Off), None);
    }

    fn art(n: u32) -> String {
        format!("{n:08x}/aaaa/bbbb/cccc/0123456789ab")
    }

    /// A catalog video with TIDAL artwork, its facet row hand-set.
    fn video(
        conn: &Connection,
        id: i64,
        artist: i64,
        cut: &str,
        programmable: bool,
        quality: f64,
        artwork: Option<&str>,
    ) {
        let json = serde_json::json!({
            "tidal_id": id, "title": format!("Song {id}"), "duration_s": 200,
            "artist_id": artist, "artist_name": format!("Artist {artist}"),
            "artwork_url": artwork.map(|a| format!("https://resources.tidal.com/images/{a}/640x640.jpg")),
        });
        conn.execute(
            "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, artist, format!("Artist {artist}"), json.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO video_facets (tidal_video_id, artist_tidal_id, cut, programmable, song_key, quality)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, artist, cut, programmable, format!("k{id}"), quality],
        )
        .unwrap();
    }

    fn on_map(conn: &Connection, artists: &[i64]) {
        conn.execute("DELETE FROM video_artist_facets", []).unwrap();
        for artist in artists {
            conn.execute(
                "INSERT INTO video_artist_facets (artist_tidal_id, name, x, y, videos)
                 VALUES (?1, ?2, 1.0, 1.0, ?3)",
                params![artist, format!("Artist {artist}"), artist % 10],
            )
            .unwrap();
        }
    }

    fn row(conn: &Connection, artist: i64) -> Option<(i64, String, String, i64)> {
        conn.query_row(
            "SELECT video_id, art, status, tries FROM video_artist_pictures WHERE artist_tidal_id = ?1",
            [artist],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn seeding_picks_the_best_programmable_cut_and_follows_the_map() {
        let conn = conn();
        // Artist 1: the official cut beats a better live one; the
        // interview is not programmable; the best official has no art.
        video(&conn, 10, 1, "live", true, 0.9, Some(&art(10)));
        video(&conn, 11, 1, "official", true, 0.5, Some(&art(11)));
        video(&conn, 12, 1, "official", true, 0.4, Some(&art(12)));
        video(&conn, 13, 1, "interview", false, 1.0, Some(&art(13)));
        video(&conn, 14, 1, "official", true, 0.95, None);
        // Artist 2 has only an interview; artist 3 has no artwork at all;
        // artist 4 is off the map; Various Artists is never pictured.
        video(&conn, 20, 2, "interview", false, 0.5, Some(&art(20)));
        video(&conn, 30, 3, "official", true, 0.5, None);
        video(&conn, 40, 4, "official", true, 0.5, Some(&art(40)));
        video(
            &conn,
            50,
            VARIOUS_ARTISTS_ID,
            "official",
            true,
            0.5,
            Some(&art(50)),
        );
        on_map(&conn, &[1, 2, 3, VARIOUS_ARTISTS_ID]);

        let report = seed(&conn).unwrap();
        assert_eq!(
            report,
            SeedReport {
                added: 2,
                changed: 0,
                removed: 0,
                total: 2
            }
        );
        assert_eq!(row(&conn, 1), Some((11, art(11), "pending".into(), 0)));
        assert_eq!(row(&conn, 2), Some((20, art(20), "pending".into(), 0)));
        assert_eq!(row(&conn, 3), None, "no artwork");
        assert_eq!(row(&conn, 4), None, "off the map");
        assert_eq!(row(&conn, VARIOUS_ARTISTS_ID), None);

        // Done rows stay done when nothing changed.
        conn.execute(
            "UPDATE video_artist_pictures SET status = 'done', rgb = 255, tries = 1",
            [],
        )
        .unwrap();
        assert_eq!(seed(&conn).unwrap().changed, 0);
        assert_eq!(row(&conn, 1).unwrap().2, "done");

        // A new better cut sends artist 1 back to pending; artist 2 leaves
        // the map; artist 4 joins it.
        video(&conn, 15, 1, "official", true, 0.8, Some(&art(15)));
        on_map(&conn, &[1, 4]);
        let report = seed(&conn).unwrap();
        assert_eq!(
            report,
            SeedReport {
                added: 1,
                changed: 1,
                removed: 1,
                total: 2
            }
        );
        assert_eq!(row(&conn, 1), Some((15, art(15), "pending".into(), 0)));
        assert_eq!(row(&conn, 2), None);
        assert_eq!(row(&conn, 4).unwrap().2, "pending");
    }

    #[test]
    fn the_read_lists_done_rows_only_in_artist_order() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO video_artist_pictures (artist_tidal_id, video_id, art, rgb, status) VALUES
                 (30, 3, 'c', 3355443, 'done'),
                 (10, 1, 'a', 16737095, 'done'),
                 (20, 2, 'b', NULL, 'pending'),
                 (40, 4, 'd', NULL, 'failed');",
        )
        .unwrap();
        let pictures = read(&conn).unwrap();
        assert_eq!(
            serde_json::to_value(&pictures).unwrap(),
            serde_json::json!({
                "ready": 2, "total": 4,
                "ids": [10, 30], "rgb": [16737095, 3355443], "art": ["a", "c"],
            })
        );
    }

    /// Answers from a script per URL; records every URL asked for.
    struct FakeFetch {
        good: Vec<u8>,
        asked: Mutex<Vec<String>>,
        down: bool,
    }

    impl Fetch for FakeFetch {
        fn fetch(&self, url: &str) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send {
            self.asked.lock().unwrap().push(url.to_owned());
            let answer = if self.down {
                Err(FetchError::Unreachable("offline".into()))
            } else if url.contains(&art(1)) {
                Ok(self.good.clone())
            } else if url.contains(&art(2)) {
                Err(FetchError::Refused("404 Not Found".into()))
            } else {
                Ok(b"garbage".to_vec())
            };
            async move { answer }
        }
    }

    fn picture_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db.with_conn(|conn| {
            for (artist, n) in [(1i64, 1u32), (2, 2), (3, 3)] {
                video(
                    conn,
                    artist * 10,
                    artist,
                    "official",
                    true,
                    0.5,
                    Some(&art(n)),
                );
            }
            on_map(conn, &[1, 2, 3]);
            Ok(())
        })
        .unwrap();
        db
    }

    fn statuses(db: &Database) -> Vec<(i64, String, i64, Option<i64>)> {
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT artist_tidal_id, status, tries, rgb FROM video_artist_pictures ORDER BY 1",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
    }

    #[tokio::test]
    async fn the_worker_stores_colours_and_fails_after_three_tries() {
        let db = picture_db();
        // Nothing is seeded before the first build.
        let fetch = FakeFetch {
            good: jpeg(80, 80, |_, _| [200, 30, 40]),
            asked: Mutex::new(Vec::new()),
            down: false,
        };
        let mut worker = Worker::new(db.clone(), fetch);
        worker.pace = |setting| pace(setting).map(|_| Duration::ZERO);
        worker.building = || false;
        assert_eq!(worker.tick().await.unwrap(), IDLE);
        assert!(statuses(&db).is_empty());

        db.with_conn(|conn| {
            // A build marker without rebuilding the hand-set facets.
            conn.execute(
                "INSERT INTO video_intel_runs (stage, finished_at) VALUES ('build', datetime('now'))",
                [],
            )?;
            discovery_setting::save(conn, DiscoverySetting::Limited)
        })
        .unwrap();
        assert_eq!(worker.tick().await.unwrap(), Duration::ZERO);
        let after_one = statuses(&db);
        assert_eq!(after_one[0].1, "done");
        let colour = rgb(after_one[0].3.unwrap() as u32);
        assert!(colour[0] > 180 && colour[1] < 60, "{colour:?}");
        assert_eq!(
            (after_one[1].1.as_str(), after_one[1].2),
            ("pending", 1),
            "404"
        );
        assert_eq!(
            (after_one[2].1.as_str(), after_one[2].2),
            ("pending", 1),
            "undecodable"
        );

        worker.tick().await.unwrap();
        worker.tick().await.unwrap();
        let after_three = statuses(&db);
        assert_eq!((after_three[1].1.as_str(), after_three[1].2), ("failed", 3));
        assert_eq!((after_three[2].1.as_str(), after_three[2].2), ("failed", 3));
        assert_eq!(worker.fetch.asked.lock().unwrap().len(), 7);
        assert_eq!(worker.tick().await.unwrap(), IDLE, "nothing left");

        let pictures = db.with_conn(read).unwrap();
        assert_eq!((pictures.ready, pictures.total), (1, 3));
        assert_eq!(pictures.ids, vec![1]);
        assert_eq!(pictures.art, vec![art(1)]);
    }

    #[tokio::test]
    async fn an_unreachable_cdn_spends_no_tries_and_off_fetches_nothing() {
        let db = picture_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO video_intel_runs (stage, finished_at) VALUES ('build', datetime('now'))",
                [],
            )?;
            discovery_setting::save(conn, DiscoverySetting::Off)
        })
        .unwrap();
        let fetch = FakeFetch {
            good: Vec::new(),
            asked: Mutex::new(Vec::new()),
            down: true,
        };
        let mut worker = Worker::new(db.clone(), fetch);
        worker.building = || false;
        assert_eq!(worker.tick().await.unwrap(), IDLE, "off");
        assert!(worker.fetch.asked.lock().unwrap().is_empty());

        db.with_conn(|conn| discovery_setting::save(conn, DiscoverySetting::Full))
            .unwrap();
        assert_eq!(worker.tick().await.unwrap(), IDLE);
        assert_eq!(worker.tick().await.unwrap(), IDLE * 2, "backing off");
        assert!(
            statuses(&db)
                .iter()
                .all(|row| row.1 == "pending" && row.2 == 0)
        );
    }

    #[test]
    fn pending_rows_go_round_robin_across_scenes_biggest_first() {
        let conn = conn();
        // Scene 1: 11 (100 videos), 12 (50), 13 (10); scene 2: 21 (80),
        // 22 (5); 31 is uncharted however big; 41 (scene 3) has a retry
        // behind it.
        conn.execute_batch(
            "INSERT INTO video_artist_facets (artist_tidal_id, scene_id, videos, x, y) VALUES
                 (11, 1, 100, 0, 0), (12, 1, 50, 0, 0), (13, 1, 10, 0, 0),
                 (21, 2, 80, 0, 0), (22, 2, 5, 0, 0), (31, NULL, 500, 0, 0),
                 (41, 3, 900, 0, 0);
             INSERT INTO video_artist_pictures (artist_tidal_id, art, tries) VALUES
                 (11, 'a', 0), (12, 'a', 0), (13, 'a', 0), (21, 'a', 0), (22, 'a', 0),
                 (31, 'a', 0), (41, 'a', 1);
             INSERT INTO video_artist_pictures (artist_tidal_id, art, status) VALUES (50, 'a', 'done');",
        )
        .unwrap();
        let order: Vec<i64> = pending(&conn, 10)
            .unwrap()
            .iter()
            .map(|p| p.artist_id)
            .collect();
        assert_eq!(order, vec![11, 21, 12, 22, 13, 31, 41]);
        assert_eq!(pending(&conn, 2).unwrap().len(), 2);
    }

    #[test]
    fn a_reseed_during_a_fetch_wins() {
        let conn = conn();
        video(&conn, 10, 1, "official", true, 0.5, Some(&art(1)));
        on_map(&conn, &[1]);
        seed(&conn).unwrap();
        let item = pending(&conn, 10).unwrap().remove(0);
        video(&conn, 11, 1, "official", true, 0.9, Some(&art(2)));
        seed(&conn).unwrap();
        let bytes = jpeg(16, 16, |_, _| [0, 0, 200]);
        assert_eq!(record(&conn, &item, Ok(bytes)).unwrap(), Outcome::Stale);
        assert_eq!(row(&conn, 1).unwrap().2, "pending");
    }

    #[test]
    fn a_real_build_seeds_from_its_facets() {
        let conn = conn();
        for (id, artist) in [(1i64, 7i64), (2, 7), (3, 8)] {
            video_catalog_only(&conn, id, artist);
        }
        build_all(&conn).unwrap();
        seed(&conn).unwrap();
        let pending = pending(&conn, 10).unwrap();
        let artists: Vec<i64> = pending.iter().map(|p| p.artist_id).collect();
        assert_eq!(artists.len(), 2, "{artists:?}");
    }

    fn video_catalog_only(conn: &Connection, id: i64, artist: i64) {
        let json = serde_json::json!({
            "tidal_id": id, "title": format!("Song {id}"), "duration_s": 200,
            "artist_id": artist, "artist_name": format!("Artist {artist}"),
            "artwork_url": format!("https://resources.tidal.com/images/{}/640x640.jpg", art(id as u32)),
            "video_type": "Music Video",
        });
        conn.execute(
            "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, artist, format!("Artist {artist}"), json.to_string()],
        )
        .unwrap();
    }
}
