// Lazy-import of TIDAL albums into the local library.
//
// Called when the user plays a TIDAL preview (an album or track that's in
// TIDAL's catalog but not yet in their library). The import upserts artist +
// album + tracks with `source = 'tidal_stream'`. These are transient rows that
// exist for playback/history/analysis but must stay invisible in Library grids.
//
// They stay hidden because they are NOT marked `is_library` (the column added
// in MIGRATION_052 defaults to 0) and the library filters (`favorite_predicate`
// / `ARTIST_LIBRARY_TRACK_WHERE` in db/queries.rs) gate the album-favorite
// branch on `is_library = 1`. Note: the filters do NOT key off `source`, so a
// `tidal_stream` row landing in a favorited album previously leaked -- the
// is_library gate is what now keeps it out. An explicit favorite promotes the
// row into the library via the favorite-toggle handler.

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

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
    pub artist_id: i64,
    pub album_id: Option<i64>,
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
        .get_all_album_tracks(tidal_album_id)
        .await
        .context("fetching TIDAL album tracks")?;

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
    let primary_artist =
        tracks
            .first()
            .map(|t| t.artist.clone())
            .unwrap_or_else(|| super::client::TidalArtist {
                id: 0,
                name: "Unknown artist".to_string(),
                picture: None,
                extra: Default::default(),
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
                artist_id: track_artist_id,
                album_id: Some(album_id),
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
        params![
            tidal_id,
            title,
            artist_id,
            artwork_url,
            track_count,
            TIDAL_STREAM_SOURCE
        ],
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

        let existing: Option<(i64, i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, artist_id, album_id FROM tracks WHERE tidal_id = ?1",
                params![meta.tidal_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        if let Some((local_id, artist_id, album_id)) = existing {
            tx.commit()?;
            return Ok(ImportedTrack {
                tidal_id: meta.tidal_id,
                local_id,
                artist_id,
                album_id,
            });
        }

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
        let local_id = tx.last_insert_rowid();

        tx.commit()?;
        Ok(ImportedTrack {
            tidal_id: meta.tidal_id,
            local_id,
            artist_id,
            album_id,
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

/// Self-healing backfill for a TIDAL-backed track row that was persisted
/// without a real duration (and usually without an album link) - see
/// [`crate::services::tidal::repair`]. Given a freshly fetched [`TidalTrack`],
/// fill only the missing pieces: never clobber a duration or album that is
/// already present.
///
/// Returns `Ok(true)` only when a previously-missing value was actually
/// filled. That precise signal is load-bearing: a bare `UPDATE ... WHERE id`
/// reports one row "changed" even when every value is identical, which would
/// let the repair sweep re-fetch an unfixable row forever. Reporting `false`
/// on a no-op keeps the sweep self-terminating.
pub fn repair_track_metadata_tx(
    conn: &rusqlite::Connection,
    local_id: i64,
    t: &crate::services::tidal::client::TidalTrack,
) -> Result<bool> {
    use crate::services::tidal::client::TidalClient;

    let tx = conn.unchecked_transaction()?;

    let (cur_duration, cur_album_id, artist_id): (Option<i64>, Option<i64>, i64) = tx.query_row(
        "SELECT duration_ms, album_id, artist_id FROM tracks WHERE id = ?1",
        params![local_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;

    // TIDAL durations are whole seconds; 0/negative means "unknown", which is
    // no better than what we already have, so treat it as nothing to write.
    let new_duration_ms: Option<i64> = if t.duration > 0 {
        Some(t.duration * 1000)
    } else {
        None
    };
    let fills_duration = new_duration_ms.is_some() && cur_duration.unwrap_or(0) == 0;

    // Only touch the album when the row has none yet. Reuse the metadata-path
    // upsert so we share/create the album row keyed on its TIDAL id and attach
    // artwork the same way a full import would.
    let new_album_id: Option<i64> = if cur_album_id.is_none() {
        if let Some(album) = t.album.as_ref() {
            let artwork_url = TidalClient::get_artwork_url(&album.cover, 640);
            Some(upsert_album_from_metadata_tx(
                &tx,
                Some(album.id),
                Some(album.title.as_str()),
                artist_id,
                artwork_url.as_deref(),
            )?)
        } else {
            None
        }
    } else {
        None
    };
    let fills_album = new_album_id.is_some();

    if !fills_duration && !fills_album {
        return Ok(false);
    }

    tx.execute(
        "UPDATE tracks SET
            duration_ms  = COALESCE(?2, duration_ms),
            album_id     = COALESCE(?3, album_id),
            track_number = COALESCE(track_number, ?4),
            isrc         = COALESCE(isrc, ?5),
            updated_at   = datetime('now')
         WHERE id = ?1",
        params![
            local_id,
            if fills_duration {
                new_duration_ms
            } else {
                None
            },
            new_album_id,
            t.track_number,
            t.isrc,
        ],
    )?;

    tx.commit()?;
    Ok(true)
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
                     is_favorite INTEGER DEFAULT 0,
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
                     is_library INTEGER NOT NULL DEFAULT 0,
                     track_number INTEGER,
                     isrc TEXT,
                     updated_at TEXT,
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

    /// Build a `TidalTrack` from JSON (they derive Deserialize but not Default),
    /// so tests don't have to spell out every nested field.
    fn tidal_track_json(
        id: i64,
        duration_secs: i64,
        album: Option<(i64, &str)>,
    ) -> crate::services::tidal::client::TidalTrack {
        let mut v = serde_json::json!({
            "id": id,
            "title": "Repaired",
            "duration": duration_secs,
            "trackNumber": 4,
            "isrc": "GB1234500042",
            "artist": { "id": 1, "name": "SOTA" },
        });
        if let Some((album_id, title)) = album {
            v["album"] = serde_json::json!({
                "id": album_id, "title": title, "cover": "aa-bb-cc-dd",
            });
        }
        serde_json::from_value(v).expect("valid TidalTrack json")
    }

    #[tokio::test]
    async fn repair_fills_zero_duration_and_links_album() {
        let db = setup_db();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'SOTA')", [])?;
            conn.execute(
                "INSERT INTO tracks (id, tidal_id, title, artist_id, album_id, duration_ms, source)
                 VALUES (500, 266425080, 'Realise', 1, NULL, 0, 'tidal_stream')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let track = tidal_track_json(266425080, 196, Some((6196, "Realise")));
        let changed = db
            .with_conn(|conn| Ok(repair_track_metadata_tx(conn, 500, &track)?))
            .unwrap();
        assert!(
            changed,
            "a zero-duration row with a fetchable album must repair"
        );

        let (dur, album_id, has_album_row, trk, isrc): (
            i64,
            Option<i64>,
            bool,
            Option<i32>,
            Option<String>,
        ) = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT duration_ms, album_id,
                            EXISTS(SELECT 1 FROM albums WHERE tidal_id = 6196),
                            track_number, isrc
                     FROM tracks WHERE id = 500",
                    [],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get::<_, i64>(2)? == 1,
                            r.get(3)?,
                            r.get(4)?,
                        ))
                    },
                )?)
            })
            .unwrap();
        assert_eq!(dur, 196_000, "TIDAL seconds converted to ms");
        assert!(album_id.is_some(), "album linked onto the track");
        assert!(has_album_row, "album row upserted by tidal_id");
        assert_eq!(trk, Some(4), "missing track_number backfilled");
        assert_eq!(
            isrc.as_deref(),
            Some("GB1234500042"),
            "missing isrc backfilled"
        );
    }

    #[tokio::test]
    async fn repair_is_noop_when_tidal_cannot_improve_the_row() {
        // The self-terminating guard: when TIDAL has no real duration and no
        // album, the writer must report `false`. A bare UPDATE would report one
        // row "changed" even on an identical write, which would make the repair
        // sweep re-fetch this id on every trigger forever.
        let db = setup_db();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'SOTA')", [])?;
            conn.execute(
                "INSERT INTO tracks (id, tidal_id, title, artist_id, album_id, duration_ms, source)
                 VALUES (501, 999, 'Ghost', 1, NULL, 0, 'tidal_stream')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let track = tidal_track_json(999, 0, None);
        let changed = db
            .with_conn(|conn| Ok(repair_track_metadata_tx(conn, 501, &track)?))
            .unwrap();
        assert!(
            !changed,
            "no duration and no album => no-op, not a false positive"
        );
    }

    #[tokio::test]
    async fn import_track_from_metadata_leaves_track_out_of_library() {
        let db = setup_db();

        // A pre-existing favorited album (the leak vector): a transient import
        // that matches it by tidal_id must NOT inherit library status.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2)",
                params![7777, "Massive Attack"],
            )?;
            let aid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO albums (tidal_id, title, artist_id, is_favorite, source)
                 VALUES (?1, ?2, ?3, 1, 'tidal')",
                params![8888, "Mezzanine", aid],
            )?;
            Ok(())
        })
        .unwrap();

        let imported = import_track_from_metadata(
            &db,
            ImportTrackMetadata {
                tidal_id: 55_001,
                title: "Teardrop".to_string(),
                artist_name: "Massive Attack".to_string(),
                artist_tidal_id: Some(7777),
                artist_picture: None,
                album_title: Some("Mezzanine".to_string()),
                album_tidal_id: Some(8888),
                album_artwork_url: None,
                duration_ms: Some(330_000),
            },
        )
        .await
        .expect("import should succeed");

        let is_library: i64 = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT is_library FROM tracks WHERE id = ?1",
                    params![imported.local_id],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            is_library, 0,
            "a transient import must stay out of the library even when it \
             attaches to a pre-existing favorited album by tidal_id"
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

        let (album_artwork, album_tidal, artist_photo): (
            Option<String>,
            Option<i64>,
            Option<String>,
        ) = db
            .with_conn(|conn| {
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

    #[tokio::test]
    async fn import_track_from_metadata_returns_existing_track_artist_and_album_ids() {
        let db = setup_db();

        let (existing_artist_id, existing_album_id, existing_track_id) = db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO artists (name) VALUES (?1)",
                    params!["Local Artist"],
                )?;
                let artist_id = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO albums (title, artist_id, source) VALUES (?1, ?2, 'local')",
                    params!["Local Album", artist_id],
                )?;
                let album_id = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO tracks (
                        tidal_id, title, artist_id, album_id, duration_ms, source
                    ) VALUES (?1, ?2, ?3, ?4, ?5, 'local')",
                    params![88001, "Known Track", artist_id, album_id, 180_000],
                )?;
                Ok((artist_id, album_id, conn.last_insert_rowid()))
            })
            .unwrap();

        let imported = import_track_from_metadata(
            &db,
            ImportTrackMetadata {
                tidal_id: 88001,
                title: "Known Track".to_string(),
                artist_name: "TIDAL Artist".to_string(),
                artist_tidal_id: Some(44001),
                album_title: Some("TIDAL Album".to_string()),
                album_tidal_id: Some(55001),
                duration_ms: Some(180_000),
                ..Default::default()
            },
        )
        .await
        .expect("existing track import should succeed");

        assert_eq!(imported.local_id, existing_track_id);
        assert_eq!(imported.artist_id, existing_artist_id);
        assert_eq!(imported.album_id, Some(existing_album_id));
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
