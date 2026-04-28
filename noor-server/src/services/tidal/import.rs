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

        let artist_id = upsert_artist_tx(&tx, primary_artist.id, &primary_artist.name)?;
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
                upsert_artist_tx(&tx, t.artist.id, &t.artist.name)?
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
            return Ok(id);
        }
        tx.execute(
            "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2)",
            params![tidal_id, name],
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
        return Ok(id);
    }
    tx.execute("INSERT INTO artists (name) VALUES (?1)", params![name])?;
    Ok(tx.last_insert_rowid())
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
    tidal_id: i64,
    title: String,
    artist_name: String,
    artist_tidal_id: Option<i64>,
    album_title: Option<String>,
    duration_ms: Option<i64>,
) -> Result<ImportedTrack> {
    conn_pool.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;

        let artist_id = upsert_artist_tx(&tx, artist_tidal_id.unwrap_or(0), &artist_name)?;

        let album_id: Option<i64> = if let Some(ref atitle) = album_title {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM albums WHERE artist_id = ?1 AND title = ?2 LIMIT 1",
                    params![artist_id, atitle],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                Some(id)
            } else {
                tx.execute(
                    "INSERT INTO albums (title, artist_id, source) VALUES (?1, ?2, ?3)",
                    params![atitle, artist_id, TIDAL_STREAM_SOURCE],
                )?;
                Some(tx.last_insert_rowid())
            }
        } else {
            None
        };

        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM tracks WHERE tidal_id = ?1",
                params![tidal_id],
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
                    tidal_id,
                    title,
                    artist_id,
                    album_id,
                    duration_ms.unwrap_or(0),
                    TIDAL_STREAM_SOURCE,
                ],
            )?;
            tx.last_insert_rowid()
        };

        tx.commit()?;
        Ok(ImportedTrack { tidal_id, local_id })
    })
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
