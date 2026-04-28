use anyhow::{Context, Result};

const TIDAL_API_URL: &str = "https://api.tidal.com/v1";

/// Add a track to TIDAL favorites.
pub async fn add_favorite_track(
    http: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    track_id: i64,
    country_code: &str,
) -> Result<()> {
    crate::services::tidal::backoff::global().check()?;
    http.post(format!(
        "{}/users/{}/favorites/tracks?countryCode={}",
        TIDAL_API_URL, user_id, country_code
    ))
    .header("Authorization", format!("Bearer {}", access_token))
    .form(&[("trackIds", track_id.to_string())])
    .send()
    .await?
    .error_for_status()
    .context("Failed to add favorite track")?;

    Ok(())
}

/// Remove a track from TIDAL favorites.
pub async fn remove_favorite_track(
    http: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    track_id: i64,
    country_code: &str,
) -> Result<()> {
    crate::services::tidal::backoff::global().check()?;
    http.delete(format!(
        "{}/users/{}/favorites/tracks/{}?countryCode={}",
        TIDAL_API_URL, user_id, track_id, country_code
    ))
    .header("Authorization", format!("Bearer {}", access_token))
    .send()
    .await?
    .error_for_status()
    .context("Failed to remove favorite track")?;

    Ok(())
}

/// Remove an album from TIDAL favorites.
pub async fn remove_favorite_album(
    http: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    album_id: i64,
    country_code: &str,
) -> Result<()> {
    crate::services::tidal::backoff::global().check()?;
    http.delete(format!(
        "{}/users/{}/favorites/albums/{}?countryCode={}",
        TIDAL_API_URL, user_id, album_id, country_code
    ))
    .header("Authorization", format!("Bearer {}", access_token))
    .send()
    .await?
    .error_for_status()
    .context("Failed to remove favorite album")?;

    Ok(())
}

/// Add tracks to a TIDAL playlist.
pub async fn add_to_playlist(
    http: &reqwest::Client,
    access_token: &str,
    playlist_uuid: &str,
    track_ids: &[i64],
    country_code: &str,
) -> Result<()> {
    crate::services::tidal::backoff::global().check()?;
    let ids: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    http.post(format!(
        "{}/playlists/{}/items?countryCode={}",
        TIDAL_API_URL, playlist_uuid, country_code
    ))
    .header("Authorization", format!("Bearer {}", access_token))
    .form(&[("trackIds", ids)])
    .send()
    .await?
    .error_for_status()
    .context("Failed to add tracks to playlist")?;

    Ok(())
}

pub async fn remove_favorite_tracks(
    http: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    track_ids: &[i64],
    country_code: &str,
) -> Result<usize> {
    crate::services::tidal::backoff::global().check()?;
    let mut removed = 0;
    for track_id in track_ids {
        remove_favorite_track(http, access_token, user_id, *track_id, country_code).await?;
        removed += 1;
    }
    Ok(removed)
}

pub async fn remove_favorite_albums(
    http: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    album_ids: &[i64],
    country_code: &str,
) -> Result<usize> {
    crate::services::tidal::backoff::global().check()?;
    let mut removed = 0;
    for album_id in album_ids {
        remove_favorite_album(http, access_token, user_id, *album_id, country_code).await?;
        removed += 1;
    }
    Ok(removed)
}
