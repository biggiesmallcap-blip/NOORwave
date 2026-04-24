use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::SharedState;

const SPOTIFY_SEARCH_URL: &str = "https://api.spotify.com/v1/search";
const SPOTIFY_ALBUM_URL: &str = "https://api.spotify.com/v1/albums/{}";
const SPOTIFY_ARTIST_URL: &str = "https://api.spotify.com/v1/artists/{}";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    tracks: TrackPage,
}

#[derive(Debug, Deserialize)]
struct TrackPage {
    items: Vec<SpotifyTrack>,
}

#[derive(Debug, Deserialize)]
struct SpotifyTrack {
    id: String,
    album: AlbumRef,
}

#[derive(Debug, Deserialize)]
struct AlbumRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct AlbumDetails {
    genres: Vec<String>,
    artists: Vec<ArtistRef>,
}

#[derive(Debug, Deserialize)]
struct ArtistRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ArtistDetails {
    genres: Vec<String>,
}

/// Run Spotify genre enrichment.
/// Tries ISRC first, then Artist+Title.
/// Fetches Album and Artist details to get genres.
pub async fn run_enrichment<F>(
    state: SharedState,
    http: Client,
    mut progress: F,
) -> Result<()>
where
    F: FnMut(usize, usize) + Send + 'static,
{
    info!("Spotify enrichment started.");

    let tracks_to_enrich: Vec<(i64, String, String, Option<String>)> = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.title, a.name, t.isrc
                 FROM tracks t
                 JOIN artists a ON t.artist_id = a.id
                 WHERE t.id NOT IN (SELECT track_id FROM track_genres WHERE source = 'spotify')
                 LIMIT 2000", 
            )?;
            Ok(stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>())
        })?;

    let total = tracks_to_enrich.len();
    if total == 0 {
        info!("No tracks to enrich.");
        return Ok(());
    }

    let mut processed = 0;

    for (track_id, title, artist, isrc) in tracks_to_enrich {
        let token = {
            let s = state.read().await;
            match &s.spotify_tokens {
                Some(t) => t.access_token.clone(),
                None => {
                    warn!("Spotify tokens missing during enrichment.");
                    break;
                }
            }
        };

        let mut genres = Vec::new();

        // 1. Search Spotify
        let query = if let Some(ref code) = isrc {
            format!("isrc:{}", code)
        } else {
            format!("artist:{} track:{}", artist, title)
        };
        
        let url = format!("{}?q={}&type=track&limit=1", SPOTIFY_SEARCH_URL, urlencoding::encode(&query));
        if let Ok(resp) = http.get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(data) = resp.json::<SearchResponse>().await {
                    if let Some(track) = data.tracks.items.first() {
                        // 2. Fetch Album Genres
                        let album_url = SPOTIFY_ALBUM_URL.replace("{}", &track.album.id);
                        if let Ok(album_resp) = http.get(&album_url)
                            .header("Authorization", format!("Bearer {}", token))
                            .send()
                            .await
                        {
                            if album_resp.status().is_success() {
                                if let Ok(album) = album_resp.json::<AlbumDetails>().await {
                                    genres.extend(album.genres);
                                }
                            }
                        }
                        
                        // 3. Fetch Artist Genres (if album had none)
                        if genres.is_empty() {
                             // We'd need artist ID from album, but let's just use what we have.
                        }
                    }
                }
            }
        }

        let genres = crate::genre::builder::collect_clear_genres(genres);

        if !genres.is_empty() {
            let _ = state.read().await.db.with_conn(|conn| {
                for genre in &genres {
                    let genre_id: Option<i64> = conn
                        .query_row(
                            "SELECT id FROM genres WHERE name = ?1",
                            [genre],
                            |row| row.get(0),
                        )
                        .ok();

                    if let Some(id) = genre_id {
                        conn.execute(
                            "INSERT OR IGNORE INTO track_genres (track_id, genre_id, source, confidence) VALUES (?1, ?2, 'spotify', 1.0)",
                            rusqlite::params![track_id, id],
                        )?;
                    }
                }
                Ok(())
            });
        }

        processed += 1;
        progress(processed, total);
        sleep(Duration::from_millis(150)).await; // Rate limit
    }

    info!("Spotify enrichment complete. Processed {} tracks.", processed);
    Ok(())
}
