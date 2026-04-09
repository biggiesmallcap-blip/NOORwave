use super::client::{TidalAlbum, TidalArtist, TidalClient, TidalTrack};
use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::HashSet;
use tracing::info;

/// Sync all TIDAL favorites into local SQLite database.
pub async fn sync_library(
    client: &TidalClient,
    conn: &Connection,
    user_id: &str,
) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let mut favorite_album_ids = HashSet::new();
    let mut favorite_track_ids = HashSet::new();

    // Sync favorite artists
    info!("Syncing favorite artists...");
    let mut offset = 0;
    loop {
        let resp = client.get_favorite_artists(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        for fav in &resp.items {
            upsert_artist(conn, &fav.item)?;
            stats.artists += 1;
        }
        offset += resp.items.len() as i32;
        if resp
            .total_number_of_items
            .map_or(true, |t| offset as i64 >= t)
        {
            break;
        }
    }
    info!("Synced {} artists", stats.artists);

    // Sync favorite albums
    info!("Syncing favorite albums...");
    offset = 0;
    loop {
        let resp = client.get_favorite_albums(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        for fav in &resp.items {
            let album = &fav.item;
            upsert_artist(conn, &album.artist)?;
            upsert_album(conn, album)?;
            stats.albums += 1;
            favorite_album_ids.insert(album.id);

            // Also sync the album's tracks
            if let Ok(tracks_resp) = client.get_album_tracks(album.id).await {
                for track in &tracks_resp.items {
                    upsert_artist(conn, &track.artist)?;
                    upsert_track(conn, track, false)?;
                    stats.tracks += 1;
                }
            }
        }
        offset += resp.items.len() as i32;
        if resp
            .total_number_of_items
            .map_or(true, |t| offset as i64 >= t)
        {
            break;
        }
    }
    info!("Synced {} albums", stats.albums);

    // Sync favorite tracks (standalone favorites not in albums above)
    info!("Syncing favorite tracks...");
    offset = 0;
    loop {
        let resp = client.get_favorite_tracks(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        for fav in &resp.items {
            let track = &fav.item;
            favorite_track_ids.insert(track.id);
            upsert_artist(conn, &track.artist)?;
            if let Some(ref album_ref) = track.album {
                // Ensure album exists (minimal info from track reference)
                conn.execute(
                    "INSERT OR IGNORE INTO albums (tidal_id, title, artist_id, artwork_url, is_favorite, source)
                     VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id = ?3), ?4, 0, 'tidal')",
                    params![
                        album_ref.id,
                        album_ref.title,
                        track.artist.id,
                        TidalClient::get_artwork_url(&album_ref.cover, 640),
                    ],
                )?;
            }
            upsert_track(conn, track, true)?;
            stats.tracks += 1;
        }
        offset += resp.items.len() as i32;
        if resp
            .total_number_of_items
            .map_or(true, |t| offset as i64 >= t)
        {
            break;
        }
    }
    info!("Synced {} tracks total", stats.tracks);

    // Sync playlists
    info!("Syncing playlists...");
    let mut pl_offset = 0;
    let mut all_playlists: Vec<_> = vec![];
    loop {
        let resp = client.get_playlists(user_id, 100, pl_offset).await?;
        if resp.items.is_empty() {
            break;
        }
        let n = resp.items.len() as i32;
        all_playlists.extend(resp.items);
        pl_offset += n;
        if resp
            .total_number_of_items
            .map_or(true, |t| pl_offset as i64 >= t)
        {
            break;
        }
    }
    for playlist in &all_playlists {
        conn.execute(
            "INSERT OR REPLACE INTO playlists (tidal_uuid, name, description, track_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                playlist.uuid,
                playlist.title,
                playlist.description,
                playlist.number_of_tracks.unwrap_or(0),
            ],
        )?;

        // Get playlist ID
        let playlist_id: i64 = conn.query_row(
            "SELECT id FROM playlists WHERE tidal_uuid = ?1",
            params![playlist.uuid],
            |row| row.get(0),
        )?;

        // Clear existing playlist tracks
        conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            params![playlist_id],
        )?;

        // Sync playlist tracks
        let mut track_offset = 0;
        let mut position = 0;
        loop {
            let tracks_resp = client
                .get_playlist_tracks(&playlist.uuid, 100, track_offset)
                .await?;
            if tracks_resp.items.is_empty() {
                break;
            }
            for track in &tracks_resp.items {
                upsert_artist(conn, &track.artist)?;
                upsert_track(conn, track, false)?;

                // Link track to playlist
                let track_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM tracks WHERE tidal_id = ?1",
                        params![track.id],
                        |row| row.get(0),
                    )
                    .ok();

                if let Some(tid) = track_id {
                    conn.execute(
                        "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position)
                         VALUES (?1, ?2, ?3)",
                        params![playlist_id, tid, position],
                    )?;
                    position += 1;
                }
            }
            track_offset += tracks_resp.items.len() as i32;
            if tracks_resp
                .total_number_of_items
                .map_or(true, |t| track_offset as i64 >= t)
            {
                break;
            }
        }
        stats.playlists += 1;
    }
    info!("Synced {} playlists", stats.playlists);

    apply_tidal_favorite_flags(conn, "albums", "tidal_id", &favorite_album_ids)?;
    apply_tidal_favorite_flags(conn, "tracks", "tidal_id", &favorite_track_ids)?;

    Ok(stats)
}

fn upsert_artist(conn: &Connection, artist: &TidalArtist) -> Result<()> {
    let photo_url = artist.picture.as_ref().map(|p| {
        let path = p.replace('-', "/");
        format!("https://resources.tidal.com/images/{}/480x480.jpg", path)
    });

    conn.execute(
        "INSERT INTO artists (tidal_id, name, photo_url)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(tidal_id) DO UPDATE SET
            name = excluded.name,
            photo_url = COALESCE(excluded.photo_url, artists.photo_url)",
        params![artist.id, artist.name, photo_url],
    )?;
    Ok(())
}

fn upsert_album(conn: &Connection, album: &TidalAlbum) -> Result<()> {
    let artwork_url = TidalClient::get_artwork_url(&album.cover, 640);
    let year = album
        .release_date
        .as_ref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<i32>().ok());

    conn.execute(
        "INSERT INTO albums (tidal_id, title, artist_id, year, artwork_url, release_type, track_count, is_favorite, source)
         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id = ?3), ?4, ?5, ?6, ?7, 1, 'tidal')
         ON CONFLICT(tidal_id) DO UPDATE SET
            title = excluded.title,
            year = COALESCE(excluded.year, albums.year),
            artwork_url = COALESCE(excluded.artwork_url, albums.artwork_url),
            track_count = COALESCE(excluded.track_count, albums.track_count),
            is_favorite = 1",
        params![
            album.id,
            album.title,
            album.artist.id,
            year,
            artwork_url,
            album.release_type,
            album.number_of_tracks,
        ],
    )?;
    Ok(())
}

fn upsert_track(conn: &Connection, track: &TidalTrack, is_favorite: bool) -> Result<()> {
    let album_id_query = track.album.as_ref().map(|a| a.id);

    let quality = track.audio_quality.as_deref().unwrap_or("LOSSLESS");
    let fidelity_score = match quality {
        "HI_RES_LOSSLESS" => 900,
        "HI_RES" => 800,
        "LOSSLESS" => 700,
        "HIGH" => 400,
        "LOW" => 200,
        _ => 500,
    };

    conn.execute(
        "INSERT INTO tracks (
            tidal_id, title, artist_id, album_id,
            disc_number, track_number, duration_ms, isrc,
            best_quality, best_source, fidelity_score,
            is_favorite, source
         ) VALUES (
            ?1, ?2,
            (SELECT id FROM artists WHERE tidal_id = ?3),
            (SELECT id FROM albums WHERE tidal_id = ?4),
            ?5, ?6, ?7, ?8,
            ?9, 'tidal', ?10,
            ?11, 'tidal'
         )
         ON CONFLICT(tidal_id) DO UPDATE SET
            title = excluded.title,
            best_quality = excluded.best_quality,
            fidelity_score = MAX(tracks.fidelity_score, excluded.fidelity_score),
            is_favorite = MAX(tracks.is_favorite, excluded.is_favorite)",
        params![
            track.id,
            track.title,
            track.artist.id,
            album_id_query,
            track.volume_number.unwrap_or(1),
            track.track_number,
            track.duration * 1000, // TIDAL returns seconds, we store ms
            track.isrc,
            quality,
            fidelity_score,
            is_favorite as i32,
        ],
    )?;

    Ok(())
}

fn apply_tidal_favorite_flags(
    conn: &Connection,
    table: &str,
    id_column: &str,
    favorite_ids: &HashSet<i64>,
) -> Result<()> {
    let reset_sql = format!("UPDATE {table} SET is_favorite = 0 WHERE {id_column} IS NOT NULL");
    conn.execute(&reset_sql, [])?;

    let mut sorted_ids: Vec<i64> = favorite_ids.iter().copied().collect();
    sorted_ids.sort_unstable();

    for chunk in sorted_ids.chunks(800) {
        let placeholders = std::iter::repeat("?")
            .take(chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("UPDATE {table} SET is_favorite = 1 WHERE {id_column} IN ({placeholders})");
        conn.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
    }

    Ok(())
}

#[derive(Debug, Default)]
pub struct SyncStats {
    pub artists: usize,
    pub albums: usize,
    pub tracks: usize,
    pub playlists: usize,
}
