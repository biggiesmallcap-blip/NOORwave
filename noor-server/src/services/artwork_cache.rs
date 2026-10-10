//! On-disk cache for TIDAL artwork, so covers and artist photos the listener
//! has seen load from disk instead of the CDN.
//!
//! Files live in `artwork-cache/` next to `noor.db`, never in the database.
//! The size cap is a setting (Settings > Library, `server_config`
//! `artwork_cache.max_mb`); 0 turns the cache off and empties the folder.
//! Pictures are saved in the background the first time they are shown (that
//! showing loads straight from TIDAL) and the least recently
//! shown are dropped first. A slow background pass also fills the grid size
//! of the newest albums and their artists, up to two thirds of the cap.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension};
use tokio::sync::Semaphore;

use crate::SharedState;

const SETTING_KEY: &str = "artwork_cache.max_mb";
pub const DEFAULT_MAX_MB: u64 = 150;
/// The sizes offered in Settings. 0 is off.
pub const ALLOWED_MAX_MB: [u64; 6] = [0, 100, 150, 250, 500, 1000];
const TIDAL_IMAGES: &str = "https://resources.tidal.com/images/";
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// The grid size used by album, artist and playlist cards.
const WARM_SIZE: u32 = 320;
const MB: u64 = 1024 * 1024;

static DIR: OnceLock<PathBuf> = OnceLock::new();
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static CAP_BYTES: AtomicU64 = AtomicU64::new(DEFAULT_MAX_MB * MB);
static USED_BYTES: AtomicU64 = AtomicU64::new(0);
static TRIMMING: AtomicBool = AtomicBool::new(false);
/// Paths being saved in the background, and how many may download at once.
static FILLING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static FILL_SLOTS: Semaphore = Semaphore::const_new(4);

fn cache_dir() -> &'static Path {
    DIR.get_or_init(|| {
        crate::paths::resolve_db_path_from_env()
            .parent()
            .map(|dir| dir.join("artwork-cache"))
            .unwrap_or_else(|| PathBuf::from("artwork-cache"))
    })
}

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}

pub fn enabled() -> bool {
    CAP_BYTES.load(Ordering::Relaxed) > 0
}

pub fn used_bytes() -> u64 {
    USED_BYTES.load(Ordering::Relaxed)
}

/// A TIDAL image path as the CDN spells it: the image uuid split on its
/// dashes, then `WxH.jpg` ("ab12cd34/ef56/7890/abcd/0123456789ab/320x320.jpg").
/// Anything else is refused, so the route can never fetch another URL.
pub fn valid_tidal_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 6 {
        return false;
    }
    let hex =
        |part: &str, len: usize| part.len() == len && part.bytes().all(|b| b.is_ascii_hexdigit());
    if !(hex(parts[0], 8)
        && hex(parts[1], 4)
        && hex(parts[2], 4)
        && hex(parts[3], 4)
        && hex(parts[4], 12))
    {
        return false;
    }
    let Some(size) = parts[5].strip_suffix(".jpg") else {
        return false;
    };
    let Some((w, h)) = size.split_once('x') else {
        return false;
    };
    let dim = |d: &str| (2..=4).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit());
    dim(w) && dim(h)
}

/// The cache path for a full TIDAL artwork URL at `size`, if it is one.
pub fn tidal_path_at_size(url: &str, size: u32) -> Option<String> {
    let rest = url.strip_prefix(TIDAL_IMAGES)?;
    let (uuid, _) = rest.rsplit_once('/')?;
    let path = format!("{uuid}/{size}x{size}.jpg");
    valid_tidal_path(&path).then_some(path)
}

pub fn tidal_url(path: &str) -> String {
    format!("{TIDAL_IMAGES}{path}")
}

fn file_for(path: &str) -> PathBuf {
    cache_dir().join(&path[..2]).join(path.replace('/', "_"))
}

/// The image bytes when they are on disk.
pub async fn read_cached(path: &str) -> Option<Vec<u8>> {
    let file = file_for(path);
    let bytes = tokio::fs::read(&file).await.ok()?;
    touch(file);
    Some(bytes)
}

/// Saves a picture that was not on disk, without anyone waiting for it: the
/// caller sends the browser straight to TIDAL, so a page of cold pictures loads
/// from the CDN in parallel instead of queueing behind the server's fetches.
/// A picture already being fetched is not fetched twice.
pub fn fill_in_background(path: &str) {
    // Tests must not reach TIDAL or write into the real cache folder.
    if cfg!(test) || !enabled() {
        return;
    }
    let path = path.to_string();
    {
        let mut filling = FILLING.lock().unwrap_or_else(|e| e.into_inner());
        if !filling.insert(path.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        if let Ok(_permit) = FILL_SLOTS.acquire().await
            && let Ok(bytes) = fetch(&path).await
        {
            store(&file_for(&path), &bytes).await;
        }
        FILLING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&path);
    });
}

async fn fetch(path: &str) -> Result<Vec<u8>> {
    let response = client()
        .get(tidal_url(path))
        .send()
        .await?
        .error_for_status()?;
    let is_image = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("image/"));
    if !is_image {
        bail!("not an image");
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_IMAGE_BYTES {
        bail!("image too large");
    }
    Ok(bytes.to_vec())
}

async fn store(file: &Path, bytes: &[u8]) {
    if !enabled() {
        return;
    }
    let Some(parent) = file.parent() else { return };
    if tokio::fs::create_dir_all(parent).await.is_err() {
        return;
    }
    // Write then rename, so a reader never sees half a file.
    let partial = file.with_extension("part");
    if tokio::fs::write(&partial, bytes).await.is_err() {
        return;
    }
    if tokio::fs::rename(&partial, file).await.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
        return;
    }
    let used = USED_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed) + bytes.len() as u64;
    if used > CAP_BYTES.load(Ordering::Relaxed) {
        spawn_trim();
    }
}

/// Marks a hit as recently shown, so trimming drops it last.
fn touch(file: PathBuf) {
    tokio::task::spawn_blocking(move || {
        if let Ok(handle) = std::fs::OpenOptions::new().write(true).open(&file) {
            let _ = handle.set_modified(SystemTime::now());
        }
    });
}

fn cached_files() -> Vec<(SystemTime, u64, PathBuf)> {
    let mut files = Vec::new();
    let Ok(groups) = std::fs::read_dir(cache_dir()) else {
        return files;
    };
    for group in groups.flatten() {
        let Ok(entries) = std::fs::read_dir(group.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((modified, meta.len(), entry.path()));
            }
        }
    }
    files
}

/// Deletes the least recently shown files until the cache is at 80% of the
/// cap (all of them when the cap is 0).
fn trim_now() {
    let cap = CAP_BYTES.load(Ordering::Relaxed);
    let target = cap / 10 * 8;
    let mut files = cached_files();
    let mut used: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, len, path) in files {
        if used <= target && cap > 0 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            used = used.saturating_sub(len);
        }
    }
    USED_BYTES.store(used, Ordering::Relaxed);
}

fn spawn_trim() {
    if TRIMMING.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::task::spawn_blocking(|| {
        trim_now();
        TRIMMING.store(false, Ordering::Release);
    });
}

/// The stored cap in MB; an unknown value reads as the default.
pub fn load_max_mb(conn: &Connection) -> Result<u64> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [SETTING_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|mb| ALLOWED_MAX_MB.contains(mb))
        .unwrap_or(DEFAULT_MAX_MB))
}

pub fn save_max_mb(conn: &Connection, max_mb: u64) -> Result<()> {
    if !ALLOWED_MAX_MB.contains(&max_mb) {
        bail!("unsupported cache size");
    }
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SETTING_KEY, &max_mb.to_string()],
    )?;
    Ok(())
}

/// Applies a new cap: a smaller one trims now, 0 empties the folder.
pub fn apply_max_mb(max_mb: u64) {
    let before = CAP_BYTES.swap(max_mb * MB, Ordering::Relaxed);
    if max_mb * MB < before || used_bytes() > max_mb * MB {
        spawn_trim();
    }
}

/// Artwork worth having before it is asked for: the newest albums, then the
/// photos of their artists.
fn warm_candidates(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT url FROM (
             SELECT artwork_url AS url, 0 AS kind, id FROM albums
              WHERE artwork_url LIKE 'https://resources.tidal.com/images/%'
             UNION ALL
             SELECT photo_url AS url, 1 AS kind, id FROM artists
              WHERE photo_url LIKE 'https://resources.tidal.com/images/%'
         ) ORDER BY kind, id DESC",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

/// Loads the setting, measures the folder, then fills the grid size slowly
/// (one picture every 400 ms) up to two thirds of the cap.
pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        let db = { state.read().await.db.clone() };
        let max_mb = db.with_conn(load_max_mb).unwrap_or(DEFAULT_MAX_MB);
        CAP_BYTES.store(max_mb * MB, Ordering::Relaxed);
        let used = tokio::task::spawn_blocking(|| {
            cached_files().iter().map(|(_, len, _)| len).sum::<u64>()
        })
        .await
        .unwrap_or(0);
        USED_BYTES.store(used, Ordering::Relaxed);
        if used > max_mb * MB {
            spawn_trim();
        }

        // Let startup work (sync, queue restore) go first.
        tokio::time::sleep(Duration::from_secs(90)).await;
        let Ok(urls) = db.with_conn(warm_candidates) else {
            return;
        };
        let mut fetched = 0usize;
        for url in urls {
            let budget = CAP_BYTES.load(Ordering::Relaxed) / 3 * 2;
            if budget == 0 || used_bytes() >= budget {
                break;
            }
            let Some(path) = tidal_path_at_size(&url, WARM_SIZE) else {
                continue;
            };
            let file = file_for(&path);
            if tokio::fs::try_exists(&file).await.unwrap_or(false) {
                continue;
            }
            if let Ok(bytes) = fetch(&path).await {
                store(&file, &bytes).await;
                fetched += 1;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        tracing::info!(
            fetched,
            used_mb = used_bytes() / MB,
            "artwork cache warm pass done"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str =
        "https://resources.tidal.com/images/ab12cd34/ef56/7890/abcd/0123456789ab/1280x1280.jpg";

    #[test]
    fn accepts_only_tidal_image_paths() {
        assert!(valid_tidal_path(
            "ab12cd34/ef56/7890/abcd/0123456789ab/320x320.jpg"
        ));
        assert!(!valid_tidal_path(
            "ab12cd34/ef56/7890/abcd/0123456789ab/320x320.png"
        ));
        assert!(!valid_tidal_path(
            "../ef56/7890/abcd/0123456789ab/320x320.jpg"
        ));
        assert!(!valid_tidal_path(
            "ab12cd34/ef56/7890/abcd/0123456789ab/x/320x320.jpg"
        ));
        assert!(!valid_tidal_path(
            "ab12cd34/ef56/7890/abcd/0123456789ab/99999x1.jpg"
        ));
    }

    #[test]
    fn resizes_a_tidal_url_into_a_cache_path() {
        assert_eq!(
            tidal_path_at_size(URL, 320).as_deref(),
            Some("ab12cd34/ef56/7890/abcd/0123456789ab/320x320.jpg")
        );
        assert_eq!(tidal_path_at_size("https://example.com/a.jpg", 320), None);
    }

    #[test]
    fn cache_size_setting_round_trips_and_rejects_odd_values() {
        let conn = Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        assert_eq!(load_max_mb(&conn).expect("load"), DEFAULT_MAX_MB);
        save_max_mb(&conn, 500).expect("save");
        assert_eq!(load_max_mb(&conn).expect("load"), 500);
        assert!(save_max_mb(&conn, 123).is_err());
        save_max_mb(&conn, 0).expect("off");
        assert_eq!(load_max_mb(&conn).expect("load"), 0);
    }
}
