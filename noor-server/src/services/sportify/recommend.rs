//! Relationship / radio-network composer.
//!
//! Sportify only exposes primitives — search, track, album, artist,
//! artist top-tracks. "Related" rows for the discovery UI are derived by
//! composing those primitives:
//!
//! Artist related:
//!   - top tracks                  (existing top-tracks endpoint)
//!   - deep cuts                   (search artist:NAME, drop top-tracks)
//!   - recent releases             (search artist:NAME type=album, sort by date)
//!   - similar artists             (search primary genre type=artist)
//!
//! Album related:
//!   - more from this artist       (artist top-tracks, drop album tracks)
//!   - more albums by this artist  (search artist:NAME type=album, drop self)
//!
//! Track related:
//!   - more from this album        (album tracks, drop self)
//!   - more from this artist       (artist top-tracks, drop self)
//!
//! Every fetch funnels through cache helpers so a deep-page open hits at
//! most a handful of cold Sportify calls.

use anyhow::Result;
use serde::Serialize;

use crate::db::Database;

use super::cache::{self as sp_cache, SportifyCacheConfig};
use super::client::{SportifyClient, SportifySearchKind};
use super::models::{
    SportifyAlbum, SportifyArtist, SportifyPlaylist, SportifySearchResults, SportifyTrack,
};
use super::stats;

/// Cap on per-row item counts so a `/related` response stays small enough to
/// resolve eagerly without hammering TIDAL.
const ROW_LIMIT: usize = 12;
/// Sportify search fan-out limit per row.
const SEARCH_FETCH: u32 = 25;

#[derive(Debug, Clone, Serialize, Default)]
pub struct ArtistRelated {
    pub top_tracks: Vec<SportifyTrack>,
    pub deep_cuts: Vec<SportifyTrack>,
    pub recent_releases: Vec<SportifyAlbum>,
    pub similar_artists: Vec<SportifyArtist>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AlbumRelated {
    pub more_from_artist: Vec<SportifyTrack>,
    pub more_albums_by_artist: Vec<SportifyAlbum>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct TrackRelated {
    pub more_from_album: Vec<SportifyTrack>,
    pub more_from_artist: Vec<SportifyTrack>,
}

// ─── Cache-aware fetchers ────────────────────────────────────

pub async fn cached_track(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    id: &str,
) -> Result<SportifyTrack> {
    if let Some(t) = db.with_conn(|conn| sp_cache::get_track_meta(conn, cfg, id))? {
        return Ok(t);
    }
    let fetched = client.track(id).await?;
    db.with_conn(|conn| {
        sp_cache::put_track_meta(conn, id, &fetched)?;
        // World-playcount writeback. Best-effort — a missing field never
        // fails the surrounding fetch.
        stats::write_track_playcount(conn, &fetched);
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(fetched)
}

pub async fn cached_album(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    id: &str,
) -> Result<SportifyAlbum> {
    if let Some(a) = db.with_conn(|conn| sp_cache::get_album_meta(conn, cfg, id))? {
        return Ok(a);
    }
    let fetched = client.album(id).await?;
    db.with_conn(|conn| {
        sp_cache::put_album_meta(conn, id, &fetched)?;
        stats::write_track_playcounts(conn, &fetched.tracks);
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(fetched)
}

pub async fn cached_artist(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    id: &str,
) -> Result<SportifyArtist> {
    if let Some(a) = db.with_conn(|conn| sp_cache::get_artist_meta(conn, cfg, id))? {
        return Ok(a);
    }
    let fetched = client.artist(id).await?;
    db.with_conn(|conn| {
        sp_cache::put_artist_meta(conn, id, &fetched)?;
        stats::write_artist_monthly_listeners(conn, &fetched);
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(fetched)
}

pub async fn cached_playlist(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    id: &str,
) -> Result<SportifyPlaylist> {
    if let Some(p) = db.with_conn(|conn| sp_cache::get_playlist_meta(conn, cfg, id))? {
        return Ok(p);
    }
    let fetched = client.playlist(id).await?;
    db.with_conn(|conn| {
        sp_cache::put_playlist_meta(conn, id, &fetched)?;
        stats::write_track_playcounts(conn, &fetched.tracks);
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(fetched)
}

pub async fn cached_artist_top_tracks(
    client: &SportifyClient,
    db: &Database,
    _cfg: &SportifyCacheConfig,
    id: &str,
) -> Result<Vec<SportifyTrack>> {
    // Top-tracks isn't cached as a unit — the per-track meta cache absorbs
    // repeats. Always make the call but write each track through.
    let tracks = client.artist_top_tracks(id).await?;
    db.with_conn(|conn| {
        for t in &tracks {
            if let Some(track_id) = t.id.as_deref() {
                let _ = sp_cache::put_track_meta(conn, track_id, t);
            }
        }
        stats::write_track_playcounts(conn, &tracks);
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(tracks)
}

pub async fn cached_search(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    q: &str,
    kind: SportifySearchKind,
    limit: u32,
    offset: u32,
) -> Result<SportifySearchResults> {
    if let Some(s) = db.with_conn(|conn| sp_cache::get_search(conn, cfg, q, kind, limit, offset))? {
        return Ok(s);
    }
    let fetched = client.search(q, kind, limit, offset).await?;
    db.with_conn(|conn| sp_cache::put_search(conn, q, kind, limit, offset, &fetched))?;
    Ok(fetched)
}

// ─── Composed `related` rows ─────────────────────────────────

/// Build "radio network" rows for an artist. Sub-fetches run concurrently
/// where they don't depend on one another.
pub async fn artist_related(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    artist_id: &str,
) -> Result<ArtistRelated> {
    let artist = cached_artist(client, db, cfg, artist_id).await?;
    let artist_name = artist.name.clone().unwrap_or_default();
    let primary_genre = artist.genres.first().cloned();

    if artist_name.trim().is_empty() {
        return Ok(ArtistRelated::default());
    }

    let (top_res, deep_search_res, album_search_res, similar_res) = tokio::join!(
        cached_artist_top_tracks(client, db, cfg, artist_id),
        cached_search(
            client,
            db,
            cfg,
            &artist_name,
            SportifySearchKind::Track,
            SEARCH_FETCH,
            0
        ),
        cached_search(
            client,
            db,
            cfg,
            &artist_name,
            SportifySearchKind::Album,
            SEARCH_FETCH,
            0
        ),
        async {
            match primary_genre.as_deref() {
                Some(g) if !g.trim().is_empty() => {
                    cached_search(
                        client,
                        db,
                        cfg,
                        g,
                        SportifySearchKind::Artist,
                        SEARCH_FETCH,
                        0,
                    )
                    .await
                }
                _ => Ok(SportifySearchResults::default()),
            }
        },
    );

    let top_tracks: Vec<SportifyTrack> = top_res
        .unwrap_or_default()
        .into_iter()
        .take(ROW_LIMIT)
        .collect();
    let top_ids: std::collections::HashSet<String> =
        top_tracks.iter().filter_map(|t| t.id.clone()).collect();

    let deep_cuts: Vec<SportifyTrack> = deep_search_res
        .unwrap_or_default()
        .tracks
        .into_iter()
        .filter(|t| {
            // Same primary artist + not already in top tracks.
            primary_artist_matches(t, &artist_name)
                && t.id.as_deref().is_none_or(|id| !top_ids.contains(id))
        })
        .take(ROW_LIMIT)
        .collect();

    let mut recent_releases: Vec<SportifyAlbum> = album_search_res
        .unwrap_or_default()
        .albums
        .into_iter()
        .filter(|a| {
            a.artists
                .first()
                .and_then(|x| x.name.as_deref())
                .map(|n| n.eq_ignore_ascii_case(artist_name.trim()))
                .unwrap_or(false)
        })
        .collect();
    recent_releases.sort_by(|a, b| b.release_date.cmp(&a.release_date));
    recent_releases.truncate(ROW_LIMIT);

    let similar_artists: Vec<SportifyArtist> = similar_res
        .unwrap_or_default()
        .artists
        .into_iter()
        .filter(|a| {
            // Drop the seed artist itself.
            a.id.as_deref() != Some(artist_id)
        })
        .take(ROW_LIMIT)
        .collect();

    Ok(ArtistRelated {
        top_tracks,
        deep_cuts,
        recent_releases,
        similar_artists,
    })
}

pub async fn album_related(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    album_id: &str,
) -> Result<AlbumRelated> {
    let album = cached_album(client, db, cfg, album_id).await?;
    let primary_artist = album.artists.first().cloned();
    let artist_id = primary_artist.as_ref().and_then(|a| a.id.clone());
    let artist_name = primary_artist
        .as_ref()
        .and_then(|a| a.name.clone())
        .unwrap_or_default();

    if artist_name.trim().is_empty() {
        return Ok(AlbumRelated::default());
    }

    let album_track_ids: std::collections::HashSet<String> =
        album.tracks.iter().filter_map(|t| t.id.clone()).collect();

    let top_fut = async {
        match artist_id.as_deref() {
            Some(id) => cached_artist_top_tracks(client, db, cfg, id).await,
            None => Ok(Vec::new()),
        }
    };
    let albums_fut = cached_search(
        client,
        db,
        cfg,
        &artist_name,
        SportifySearchKind::Album,
        SEARCH_FETCH,
        0,
    );

    let (top_res, albums_res) = tokio::join!(top_fut, albums_fut);

    let more_from_artist: Vec<SportifyTrack> = top_res
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            t.id.as_deref()
                .is_none_or(|id| !album_track_ids.contains(id))
        })
        .take(ROW_LIMIT)
        .collect();

    let mut more_albums_by_artist: Vec<SportifyAlbum> = albums_res
        .unwrap_or_default()
        .albums
        .into_iter()
        .filter(|a| {
            // Same primary artist, not the seed album itself.
            let same_artist = a
                .artists
                .first()
                .and_then(|x| x.name.as_deref())
                .map(|n| n.eq_ignore_ascii_case(artist_name.trim()))
                .unwrap_or(false);
            same_artist && (a.id.as_deref() != Some(album_id))
        })
        .collect();
    more_albums_by_artist.sort_by(|a, b| b.release_date.cmp(&a.release_date));
    more_albums_by_artist.truncate(ROW_LIMIT);

    Ok(AlbumRelated {
        more_from_artist,
        more_albums_by_artist,
    })
}

pub async fn track_related(
    client: &SportifyClient,
    db: &Database,
    cfg: &SportifyCacheConfig,
    track_id: &str,
) -> Result<TrackRelated> {
    let track = cached_track(client, db, cfg, track_id).await?;
    let album_id = track.album.as_ref().and_then(|a| a.id.clone());
    let primary_artist_id = track.artists.first().and_then(|a| a.id.clone());

    let album_fut = async {
        match album_id.as_deref() {
            Some(id) => cached_album(client, db, cfg, id).await,
            None => Ok(SportifyAlbum::default()),
        }
    };
    let top_fut = async {
        match primary_artist_id.as_deref() {
            Some(id) => cached_artist_top_tracks(client, db, cfg, id).await,
            None => Ok(Vec::new()),
        }
    };
    let (album_res, top_res) = tokio::join!(album_fut, top_fut);

    let more_from_album: Vec<SportifyTrack> = album_res
        .unwrap_or_default()
        .tracks
        .into_iter()
        .filter(|t| t.id.as_deref() != Some(track_id))
        .take(ROW_LIMIT)
        .collect();

    let more_from_artist: Vec<SportifyTrack> = top_res
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.id.as_deref() != Some(track_id))
        .take(ROW_LIMIT)
        .collect();

    Ok(TrackRelated {
        more_from_album,
        more_from_artist,
    })
}

fn primary_artist_matches(track: &SportifyTrack, artist_name: &str) -> bool {
    track
        .artists
        .first()
        .and_then(|a| a.name.as_deref())
        .map(|n| n.eq_ignore_ascii_case(artist_name.trim()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sportify::models::{SportifyArtistRef, SportifyTrack};

    #[test]
    fn primary_artist_matches_case_insensitive() {
        let t = SportifyTrack {
            artists: vec![SportifyArtistRef {
                name: Some("Daft Punk".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(primary_artist_matches(&t, "daft punk"));
        assert!(primary_artist_matches(&t, "  Daft Punk  "));
        assert!(!primary_artist_matches(&t, "Stardust"));
    }

    #[test]
    fn primary_artist_matches_handles_no_artist() {
        let t = SportifyTrack::default();
        assert!(!primary_artist_matches(&t, "Daft Punk"));
    }
}
