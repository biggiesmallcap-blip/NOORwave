//! Spotify partner-GraphQL persisted-query SHA256 hashes + TOTP secret.
//!
//! These are baked-in constants because Spotify's anonymous endpoints rotate
//! them. When a request fails with `PersistedQueryNotFound`, `client.rs`
//! calls [`refresh_from_js`] to re-extract them from the live web-player
//! bundle and persists the new values into `server_config` so a restart
//! doesn't refetch.
//!
//! ## Capture status
//!
//! Values below were captured 2026-05-17 from
//! `https://open.spotifycdn.com/cdn/build/web-player/web-player.e79b30dd.js`.
//! All three operation hashes were grepped from the live bundle and the
//! TOTP cipher (`v14`) came from `misiektoja/spotify_monitor`. Hashes
//! rotate every few weeks; when one goes stale a request returns
//! `PersistedQueryNotFound` and [`refresh_from_js`] re-extracts the new
//! values from the bundle, then [`persist`] writes them into `server_config`
//! so the next process boot doesn't have to refetch.

use anyhow::{Context, Result};
use regex::Regex;
use reqwest::Client;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// HMAC-SHA1 key for the anonymous `/api/token` TOTP challenge. The web
/// player ships a ciphered version (`SECRET_CIPHER_DICT[14]`) and applies
/// `byte_i ^= (i % 33) + 9` to derive the actual secret. The ASCII
/// representation of the concatenated decimal transform is the byte string
/// fed straight into HMAC. Captured from `misiektoja/spotify_monitor` v14;
/// when Spotify rotates to v15+ the bundle's `SECRET_CIPHER_DICT` will tell
/// us. (Lives in code because the bundle obfuscates it; without it we
/// cannot mint a token, and only the JS-bundle scrape can recover the new
/// value.)
pub const TOTP_SECRET: &[u8] = b"55601029510267381196079975060119874370686866";

/// `totpVer` query-string parameter for `/api/token`. Must match the cipher
/// version used to derive `TOTP_SECRET` (>=10 means no `sTime`/`cTime`).
pub const TOTP_VER: u32 = 14;

/// Persisted-query SHA256 for `getTrack` (per-track playcount).
pub const GET_TRACK_HASH: &str = "612585ae06ba435ad26369870deaae23b5c8800a256cd8a57e08eddc25a37294";

/// Persisted-query SHA256 for `queryArtistOverview` (followers, monthly
/// listeners, world rank, top cities).
pub const QUERY_ARTIST_OVERVIEW_HASH: &str =
    "7f86ff63e38c24973a2842b672abe44c910c1973978dc8a4a0cb648edef34527";

/// Persisted-query SHA256 for `assistedCurationSearch` (the operation we
/// piggy-back on for ISRC -> track-uri and artist-name -> artist-uri
/// resolution). The web player no longer ships a `searchModalResults`
/// operation; `assistedCurationSearch` returns the same data shape we need.
pub const SEARCH_MODAL_RESULTS_HASH: &str =
    "f78953bf9207d73493c27284103f5aeb6e728876d5793851bf79bc706127ff70";

/// Spotify exposes the operation hash via `new <Constructor>("<opName>",
/// "query","<hash>",null)`. We send this in the
/// `spotify-app-version` header for consistency with the web player and to
/// keep server-side fingerprinting happy.
pub const SPOTIFY_APP_VERSION: &str = "896000000";

/// Regex over the web-player landing page; captures the URL of the JS chunk
/// that contains the operation-hash map.
pub const WEB_PLAYER_JS_URL_PATTERN: &str = r#"<script[^>]*src="(https://open\.spotifycdn\.com/cdn/build/web-player/web-player\.[a-f0-9]+\.js)""#;

/// Patterns we use to scrape individual hashes out of the bundle. The
/// bundle constructs each persisted-query handle as
/// `new <Ctor>("<opName>","query","<sha256>",null)`. The constructor
/// identifier is minified and changes across builds; the three string
/// args stay stable. Non-greedy match between the op name and the next
/// 64-char hex literal handles both compact and spread-out minifications.
const HASH_EXTRACT_PATTERNS: &[(&str, &str)] = &[
    ("getTrack", r#""getTrack","query","([a-f0-9]{64})""#),
    (
        "queryArtistOverview",
        r#""queryArtistOverview","query","([a-f0-9]{64})""#,
    ),
    (
        "assistedCurationSearch",
        r#""assistedCurationSearch","query","([a-f0-9]{64})""#,
    ),
];

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RefreshedHashes {
    pub get_track: Option<String>,
    pub query_artist_overview: Option<String>,
    pub search_modal_results: Option<String>,
}

/// Hits open.spotify.com once, finds the web-player JS bundle, downloads it,
/// and regex-extracts whichever hashes it can find. Anything missing is left
/// as `None` so the caller can decide to keep the baked-in fallback.
pub async fn refresh_from_js(client: &Client) -> Result<RefreshedHashes> {
    let landing = client
        .get("https://open.spotify.com/")
        .send()
        .await
        .context("fetch open.spotify.com landing")?
        .error_for_status()?
        .text()
        .await?;

    let bundle_re = Regex::new(WEB_PLAYER_JS_URL_PATTERN)?;
    let Some(caps) = bundle_re.captures(&landing) else {
        warn!("spotify_public: web-player JS url pattern did not match landing page");
        return Ok(RefreshedHashes::default());
    };
    let bundle_url = caps
        .get(1)
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| anyhow::anyhow!("web-player JS url capture group missing"))?;

    let bundle = client
        .get(&bundle_url)
        .send()
        .await
        .context("fetch web-player JS bundle")?
        .error_for_status()?
        .text()
        .await?;

    let mut out = RefreshedHashes::default();
    for (op, pattern) in HASH_EXTRACT_PATTERNS {
        let re = Regex::new(pattern)?;
        if let Some(caps) = re.captures(&bundle)
            && let Some(hash) = caps.get(1).map(|m| m.as_str().to_string())
        {
            match *op {
                "getTrack" => out.get_track = Some(hash),
                "queryArtistOverview" => out.query_artist_overview = Some(hash),
                "assistedCurationSearch" => out.search_modal_results = Some(hash),
                _ => {}
            }
        }
    }

    Ok(out)
}

const CONFIG_KEY: &str = "spotify_public_hashes_json";

pub fn load_persisted(conn: &Connection) -> Option<RefreshedHashes> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [CONFIG_KEY],
            |row| row.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

pub fn persist(conn: &Connection, hashes: &RefreshedHashes) -> Result<()> {
    let json = serde_json::to_string(hashes)?;
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![CONFIG_KEY, json],
    )?;
    Ok(())
}
