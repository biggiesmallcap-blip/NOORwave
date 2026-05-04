// Lazy-import of TIDAL albums into the local library.
//
// Called when the user plays a TIDAL preview (an album or track that's in
// TIDAL's catalog but not yet in their library). The import upserts artist +
// album + tracks with `source = 'tidal_stream'`, which the library list
// queries filter out — so these rows exist for playback/history/analysis but
// stay invisible in Library grids until the user favorites them.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::client::{TidalClient, TidalTrack};

const TIDAL_STREAM_SOURCE: &str = "tidal_stream";

#[derive(Debug, Clone)]
pub struct ImportedAlbum {
    pub album_id: i64,
    pub tracks: Vec<ImportedTrack>,
}

#[derive(Debug, Clone)]
pub struct ImportedTrack {
    pub tidal_id: i64,
    pub local_id: i64,
}

/// Inputs for [`import_track_from_metadata`]. Most fields are optional so
/// callers can pass whatever Tidal handed them — radio resolvers get the full
/// picture (album cover + artist photo); the legacy import endpoint may not.
#[derive(Debug, Clone, Default)]
pub struct ImportTrackMetadata {
    pub tidal_id: i64,
    pub title: String,
    pub artist_name: String,
    pub artist_tidal_id: Option<i64>,
    pub artist_picture: Option<String>,
    pub album_title: Option<String>,
    pub album_tidal_id: Option<i64>,
    pub album_artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
}

pub async fn import_album(
    conn_pool: &crate::db::Database,
    tidal: &TidalClient,
    tidal_album_id: i64,
) -> Result<ImportedAlbum> {
    let tracks = tidal
        .get_album_tracks(tidal_album_id)
        .await
        .context("fetching TIDAL album tracks")?
        .items;

    if tracks.is_empty() {
        anyhow::bail!("TIDAL album has no tracks");
    }

    let album_artwork = tracks
        .iter()
        .find_map(|t| t.album.as_ref().and_then(|a| a.cover.clone()))
        .and_then(|c| TidalClient::get_artwork_url(&Some(c), 640));
    let album_title = tracks
        .first()
        .and_then(|t| t.album.as_ref())
        .map(|a| a.title.clone())
        .unwrap_or_else(|| "Unknown album".to_string());
    let primary_artist = tracks.first().map(|t| t.artist.clone()).unwrap_or_else(|| {
        super::client::TidalArtist {
            id: 0,
            name: "Unknown artist".to_string(),
            picture: None,
            extra: Default::default(),
        }
    });

    conn_pool.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;

        let primary_picture = TidalClient::get_artwork_url(&primary_artist.picture, 640);
        let artist_id = upsert_artist_tx(
            &tx,
            primary_artist.id,
            &primary_artist.name,
            primary_picture.as_deref(),
        )?;
        let album_id = upsert_album_tx(
            &tx,
            tidal_album_id,
            &album_title,
            artist_id,
            album_artwork.as_deref(),
            tracks.len() as i32,
        )?;

        let mut imported: Vec<ImportedTrack> = Vec::with_capacity(tracks.len());
        for t in &tracks {
            let track_artist_id = if t.artist.id == primary_artist.id {
                artist_id
            } else {
                let picture = TidalClient::get_artwork_url(&t.artist.picture, 640);
                upsert_artist_tx(&tx, t.artist.id, &t.artist.name, picture.as_deref())?
            };
            let local_id = upsert_track_tx(&tx, t, track_artist_id, album_id)?;
            imported.push(ImportedTrack {
                tidal_id: t.id,
                local_id,
            });
        }

        tx.commit()?;
        Ok(ImportedAlbum {
            album_id,
            tracks: imported,
        })
    })
}

fn upsert_artist_tx(
    tx: &rusqlite::Transaction<'_>,
    tidal_id: i64,
    name: &str,
    photo_url: Option<&str>,
) -> Result<i64> {
    if tidal_id > 0 {
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM artists WHERE tidal_id = ?1",
                params![tidal_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            backfill_artist_photo(tx, id, photo_url)?;
            return Ok(id);
        }
        tx.execute(
            "INSERT INTO artists (tidal_id, name, photo_url) VALUES (?1, ?2, ?3)",
            params![tidal_id, name, photo_url],
        )?;
        return Ok(tx.last_insert_rowid());
    }

    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM artists WHERE LOWER(name) = LOWER(?1) LIMIT 1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        backfill_artist_photo(tx, id, photo_url)?;
        return Ok(id);
    }
    tx.execute(
        "INSERT INTO artists (name, photo_url) VALUES (?1, ?2)",
        params![name, photo_url],
    )?;
    Ok(tx.last_insert_rowid())
}

fn backfill_artist_photo(
    tx: &rusqlite::Transaction<'_>,
    artist_id: i64,
    photo_url: Option<&str>,
) -> Result<()> {
    if let Some(url) = photo_url {
        tx.execute(
            "UPDATE artists SET photo_url = ?1 WHERE id = ?2 AND photo_url IS NULL",
            params![url, artist_id],
        )?;
    }
    Ok(())
}

fn upsert_album_tx(
    tx: &rusqlite::Transaction<'_>,
    tidal_id: i64,
    title: &str,
    artist_id: i64,
    artwork_url: Option<&str>,
    track_count: i32,
) -> Result<i64> {
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM albums WHERE tidal_id = ?1",
            params![tidal_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    tx.execute(
        "INSERT INTO albums (tidal_id, title, artist_id, artwork_url, track_count, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![tidal_id, title, artist_id, artwork_url, track_count, TIDAL_STREAM_SOURCE],
    )?;
    Ok(tx.last_insert_rowid())
}

pub async fn import_track_from_metadata(
    conn_pool: &crate::db::Database,
    meta: ImportTrackMetadata,
) -> Result<ImportedTrack> {
    // Reject the chart-placeholder `tidal_id = 0`: the lookup below would
    // collide with the (UNIQUE) row already holding that value and return an
    // unrelated library track, which then gets used as a radio seed. Callers
    // must resolve a real Tidal id before importing.
    if meta.tidal_id <= 0 {
        anyhow::bail!(
            "import_track_from_metadata: tidal_id must be > 0 (got {})",
            meta.tidal_id
        );
    }
    conn_pool.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;

        let artist_id = upsert_artist_tx(
            &tx,
            meta.artist_tidal_id.unwrap_or(0),
            &meta.artist_name,
            meta.artist_picture.as_deref(),
        )?;

        let album_id: Option<i64> = if meta.album_title.is_some() || meta.album_tidal_id.is_some() {
            Some(upsert_album_from_metadata_tx(
                &tx,
                meta.album_tidal_id,
                meta.album_title.as_deref(),
                artist_id,
                meta.album_artwork_url.as_deref(),
            )?)
        } else {
            None
        };

        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM tracks WHERE tidal_id = ?1",
                params![meta.tidal_id],
                |row| row.get(0),
            )
            .optional()?;

        let local_id = if let Some(id) = existing {
            id
        } else {
            tx.execute(
                "INSERT INTO tracks (
                    tidal_id, title, artist_id, album_id,
                    duration_ms, best_quality, best_source, fidelity_score,
                    is_favorite, source
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'LOSSLESS', 'tidal', 700, 0, ?6)",
                params![
                    meta.tidal_id,
                    meta.title,
                    artist_id,
                    album_id,
                    meta.duration_ms.unwrap_or(0),
                    TIDAL_STREAM_SOURCE,
                ],
            )?;
            tx.last_insert_rowid()
        };

        tx.commit()?;
        Ok(ImportedTrack {
            tidal_id: meta.tidal_id,
            local_id,
        })
    })
}

/// Album upsert tailored to the metadata-only import path: prefer matching by
/// Tidal id (so we share rows with full album imports), fall back to
/// (artist_id, title), and backfill `tidal_id` / `artwork_url` on rows that
/// were created without them.
fn upsert_album_from_metadata_tx(
    tx: &rusqlite::Transaction<'_>,
    album_tidal_id: Option<i64>,
    album_title: Option<&str>,
    artist_id: i64,
    artwork_url: Option<&str>,
) -> Result<i64> {
    if let Some(tid) = album_tidal_id.filter(|t| *t > 0) {
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM albums WHERE tidal_id = ?1",
                params![tid],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            backfill_album_artwork(tx, id, artwork_url)?;
            return Ok(id);
        }
    }

    if let Some(title) = album_title {
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM albums WHERE artist_id = ?1 AND title = ?2 LIMIT 1",
                params![artist_id, title],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            if let Some(tid) = album_tidal_id.filter(|t| *t > 0) {
                tx.execute(
                    "UPDATE albums SET tidal_id = ?1 WHERE id = ?2 AND tidal_id IS NULL",
                    params![tid, id],
                )?;
            }
            backfill_album_artwork(tx, id, artwork_url)?;
            return Ok(id);
        }
    }

    let title = album_title.unwrap_or("Unknown album");
    tx.execute(
        "INSERT INTO albums (tidal_id, title, artist_id, artwork_url, source)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            album_tidal_id.filter(|t| *t > 0),
            title,
            artist_id,
            artwork_url,
            TIDAL_STREAM_SOURCE,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn backfill_album_artwork(
    tx: &rusqlite::Transaction<'_>,
    album_id: i64,
    artwork_url: Option<&str>,
) -> Result<()> {
    if let Some(url) = artwork_url {
        tx.execute(
            "UPDATE albums SET artwork_url = ?1 WHERE id = ?2 AND artwork_url IS NULL",
            params![url, album_id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TABLE artists (
                     id       INTEGER PRIMARY KEY,
                     tidal_id INTEGER UNIQUE,
                     name     TEXT NOT NULL,
                     photo_url TEXT
                 );
                 CREATE TABLE albums (
                     id        INTEGER PRIMARY KEY,
                     tidal_id  INTEGER UNIQUE,
                     title     TEXT NOT NULL,
                     artist_id INTEGER,
                     artwork_url TEXT,
                     source    TEXT NOT NULL DEFAULT 'tidal_stream'
                 );
                 CREATE TABLE tracks (
                     id        INTEGER PRIMARY KEY,
                     tidal_id  INTEGER UNIQUE,
                     title     TEXT NOT NULL,
                     artist_id INTEGER NOT NULL,
                     album_id  INTEGER,
                     duration_ms INTEGER,
                     best_quality TEXT,
                     best_source TEXT,
                     fidelity_score INTEGER DEFAULT 0,
                     is_favorite INTEGER DEFAULT 0,
                     source    TEXT NOT NULL DEFAULT 'tidal_stream'
                 );",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        db
    }

    fn meta(tidal_id: i64) -> ImportTrackMetadata {
        ImportTrackMetadata {
            tidal_id,
            title: "Teardrop".to_string(),
            artist_name: "Massive Attack".to_string(),
            album_title: Some("Mezzanine".to_string()),
            duration_ms: Some(330_000),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn import_track_from_metadata_is_idempotent_on_tidal_id() {
        let db = setup_db();

        let first = import_track_from_metadata(&db, meta(99001))
            .await
            .expect("first import should succeed");

        let second = import_track_from_metadata(&db, meta(99001))
            .await
            .expect("second import with same tidal_id should succeed");

        assert_eq!(
            first.local_id, second.local_id,
            "both calls must return the same local_id — the UNIQUE constraint and \
             SELECT-before-INSERT guarantee this for sequential calls; concurrent \
             calls are safe via SQLite single-writer + UNIQUE constraint backstop"
        );
    }

    #[tokio::test]
    async fn import_track_from_metadata_persists_album_artwork() {
        let db = setup_db();

        let imported = import_track_from_metadata(
            &db,
            ImportTrackMetadata {
                tidal_id: 42_001,
                title: "Common People".to_string(),
                artist_name: "Pulp".to_string(),
                artist_tidal_id: Some(1234),
                artist_picture: Some("https://resources.tidal.com/images/artist.jpg".to_string()),
                album_title: Some("Different Class".to_string()),
                album_tidal_id: Some(9876),
                album_artwork_url: Some("https://resources.tidal.com/images/cover.jpg".to_string()),
                duration_ms: Some(238_000),
            },
        )
        .await
        .expect("import should succeed");

        let (album_artwork, artist_photo): (Option<String>, Option<String>) = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT al.artwork_url, a.photo_url
                     FROM tracks t
                     JOIN artists a ON t.artist_id = a.id
                     LEFT JOIN albums al ON t.album_id = al.id
                     WHERE t.id = ?1",
                    params![imported.local_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .unwrap();

        assert_eq!(
            album_artwork.as_deref(),
            Some("https://resources.tidal.com/images/cover.jpg"),
            "album artwork must be persisted so the queue row can render it"
        );
        assert_eq!(
            artist_photo.as_deref(),
            Some("https://resources.tidal.com/images/artist.jpg"),
            "artist photo must be persisted so the artist page can render it"
        );
    }

    #[tokio::test]
    async fn import_track_from_metadata_backfills_existing_rows() {
        let db = setup_db();

        // Pre-existing artist + album rows without artwork (e.g. created by an
        // earlier metadata import before this fix landed).
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2)",
                params![5555, "Pulp"],
            )?;
            let aid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO albums (title, artist_id, source) VALUES (?1, ?2, 'tidal_stream')",
                params!["This Is Hardcore", aid],
            )?;
            Ok(())
        })
        .unwrap();

        import_track_from_metadata(
            &db,
            ImportTrackMetadata {
                tidal_id: 7777,
                title: "Help The Aged".to_string(),
                artist_name: "Pulp".to_string(),
                artist_tidal_id: Some(5555),
                artist_picture: Some("artist.jpg".to_string()),
                album_title: Some("This Is Hardcore".to_string()),
                album_tidal_id: Some(3333),
                album_artwork_url: Some("cover.jpg".to_string()),
                duration_ms: Some(269_000),
            },
        )
        .await
        .expect("import should succeed");

        let (album_artwork, album_tidal, artist_photo): (Option<String>, Option<i64>, Option<String>) =
            db.with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT al.artwork_url, al.tidal_id, a.photo_url
                     FROM albums al JOIN artists a ON al.artist_id = a.id
                     WHERE al.title = 'This Is Hardcore'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })
            .unwrap();

        assert_eq!(album_artwork.as_deref(), Some("cover.jpg"));
        assert_eq!(album_tidal, Some(3333));
        assert_eq!(artist_photo.as_deref(), Some("artist.jpg"));
    }
}

fn upsert_track_tx(
    tx: &rusqlite::Transaction<'_>,
    t: &TidalTrack,
    artist_id: i64,
    album_id: i64,
) -> Result<i64> {
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM tracks WHERE tidal_id = ?1",
            params![t.id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }

    let quality = t.audio_quality.as_deref().unwrap_or("LOSSLESS");
    let fidelity: i32 = match quality {
        "HI_RES_LOSSLESS" => 900,
        "HI_RES" => 800,
        "LOSSLESS" => 700,
        "HIGH" => 400,
        _ => 200,
    };
    let duration_ms = t.duration * 1000;

    tx.execute(
        "INSERT INTO tracks (
            tidal_id, title, artist_id, album_id,
            disc_number, track_number, duration_ms, isrc,
            best_quality, best_source, fidelity_score,
            is_favorite, source
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'tidal', ?10, 0, ?11)",
        params![
            t.id,
            t.title,
            artist_id,
            album_id,
            t.volume_number.unwrap_or(1),
            t.track_number,
            duration_ms,
            t.isrc,
            quality,
            fidelity,
            TIDAL_STREAM_SOURCE,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}
