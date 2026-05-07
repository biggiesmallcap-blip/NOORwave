use anyhow::Result;

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
    let resp = http
        .post(format!(
            "{}/users/{}/favorites/tracks?countryCode={}",
            TIDAL_API_URL, user_id, country_code
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .form(&[("trackIds", track_id.to_string())])
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
        anyhow::bail!("TIDAL mutation error {}: {}", status, body);
    }

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
    let resp = http
        .delete(format!(
            "{}/users/{}/favorites/tracks/{}?countryCode={}",
            TIDAL_API_URL, user_id, track_id, country_code
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
        anyhow::bail!("TIDAL mutation error {}: {}", status, body);
    }

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
    let resp = http
        .delete(format!(
            "{}/users/{}/favorites/albums/{}?countryCode={}",
            TIDAL_API_URL, user_id, album_id, country_code
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
        anyhow::bail!("TIDAL mutation error {}: {}", status, body);
    }

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

    let resp = http
        .post(format!(
            "{}/playlists/{}/items?countryCode={}",
            TIDAL_API_URL, playlist_uuid, country_code
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .form(&[("trackIds", ids)])
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
        anyhow::bail!("TIDAL mutation error {}: {}", status, body);
    }

    Ok(())
}

pub async fn remove_favorite_tracks(
    http: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    track_ids: &[i64],
    country_code: &str,
) -> Result<usize> {
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
    let mut removed = 0;
    for album_id in album_ids {
        remove_favorite_album(http, access_token, user_id, *album_id, country_code).await?;
        removed += 1;
    }
    Ok(removed)
}
