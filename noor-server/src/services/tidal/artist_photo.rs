//! Lazy backfill of `artists.photo_url` for rows imported via the Last.fm
//! radio resolver path.
//!
//! TIDAL's per-track artist payload often omits `picture`, so newly-imported
//! artists land in the DB with `photo_url = NULL` and render as greyed-out
//! placeholders in the queue / automix cockpit. This helper fetches the
//! artist's full TIDAL record (`/artists/{id}`) which carries the canonical
//! cover, and writes it back. Idempotent: any non-NULL `photo_url` is left
//! alone.

use super::auth::TidalTokens;
use super::client::TidalClient;
use crate::db::Database;
use rusqlite::OptionalExtension;

/// Spawn-friendly: fire-and-forget. Logs at info on success, debug on miss,
/// and warn only when the TIDAL call returns a hard error. Returns quickly
/// when the artist already has a photo so re-calls during a hot import burst
/// are cheap.
pub async fn ensure_photo_url(
    http: reqwest::Client,
    tokens: TidalTokens,
    db: Database,
    local_artist_id: i64,
    tidal_artist_id: i64,
) {
    if tidal_artist_id <= 0 {
        return;
    }

    // Read current state; skip if already populated.
    let needs_backfill = db.with_conn(|conn| {
        conn.query_row(
            "SELECT photo_url IS NULL FROM artists WHERE id = ?1",
            rusqlite::params![local_artist_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(anyhow::Error::from)
    });
    if !matches!(needs_backfill, Ok(Some(true))) {
        return;
    }

    let client = TidalClient::with_http(http, tokens.access_token, tokens.country_code);
    let artist = match client.get_artist(tidal_artist_id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!(
                local_artist_id,
                tidal_artist_id,
                "artist photo fetch failed: {}",
                e
            );
            return;
        }
    };

    let Some(url) = TidalClient::get_artwork_url(&artist.picture, 640) else {
        tracing::debug!(
            local_artist_id,
            tidal_artist_id,
            "TIDAL artist record has no picture"
        );
        return;
    };

    let written = db.with_conn(|conn| {
        conn.execute(
            "UPDATE artists SET photo_url = ?1 WHERE id = ?2 AND photo_url IS NULL",
            rusqlite::params![url, local_artist_id],
        )
        .map_err(anyhow::Error::from)
    });
    match written {
        Ok(n) if n > 0 => tracing::info!(local_artist_id, tidal_artist_id, "artist photo backfilled"),
        Ok(_) => {} // race: someone else filled it between our SELECT and UPDATE
        Err(e) => tracing::warn!(local_artist_id, "artist photo UPDATE failed: {}", e),
    }
}
