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

/// Add an album to TIDAL favorites.
pub async fn add_favorite_album(
    http: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    album_id: i64,
    country_code: &str,
) -> Result<()> {
    crate::services::tidal::backoff::global().check()?;
    let resp = http
        .post(format!(
            "{}/users/{}/favorites/albums?countryCode={}",
            TIDAL_API_URL, user_id, country_code
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .form(&[("albumIds", album_id.to_string())])
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

// ─── Playlist edits ──────────────────────────────────────────────────────────
//
// TIDAL guards every mutating playlist call with an optimistic-concurrency
// ETag: read the playlist's current tag, send it back as `If-None-Match`, and
// TIDAL answers 412 if the playlist moved underneath you. The contract is not
// publicly documented, so a 412 is treated as "refetch the tag and retry once",
// and a second 412 as a real conflict the user resolves by refreshing.

/// A conflicting edit: the playlist changed on TIDAL between reading the ETag
/// and sending the write. Distinguished from a transport failure because the
/// fix is "refresh and try again", not "retry blindly".
#[derive(Debug)]
pub struct PlaylistConflict;

impl std::fmt::Display for PlaylistConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("playlist changed on TIDAL; refresh and try again")
    }
}

impl std::error::Error for PlaylistConflict {}

/// Read a playlist's current ETag.
///
/// `TidalClient::get_json` discards response headers, so this issues its own
/// request. `limit=1` keeps the body trivial; only the header is wanted.
pub async fn get_playlist_etag(
    http: &reqwest::Client,
    access_token: &str,
    playlist_uuid: &str,
    country_code: &str,
) -> Result<String> {
    crate::services::tidal::backoff::global().check()?;
    let resp = http
        .get(format!(
            "{}/playlists/{}/items?countryCode={}&limit=1&offset=0",
            TIDAL_API_URL, playlist_uuid, country_code
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
        anyhow::bail!("TIDAL etag error {}: {}", status, body);
    }
    resp.headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("TIDAL did not return an ETag for playlist {playlist_uuid}"))
}

/// Run a playlist mutation under ETag concurrency control, refetching the tag
/// once on a 412 before giving up. `send` receives the tag and issues the call.
async fn with_playlist_etag<F, Fut>(
    http: &reqwest::Client,
    access_token: &str,
    playlist_uuid: &str,
    country_code: &str,
    send: F,
) -> Result<()>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<reqwest::Response>>,
{
    for attempt in 0..2 {
        let etag = get_playlist_etag(http, access_token, playlist_uuid, country_code).await?;
        let resp = send(etag).await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED && attempt == 0 {
            // Stale tag. Read the current one and try exactly once more.
            continue;
        }
        let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(PlaylistConflict.into());
        }
        anyhow::bail!("TIDAL mutation error {}: {}", status, body);
    }
    Err(PlaylistConflict.into())
}

/// Remove items from a TIDAL playlist by zero-based position.
///
/// Positions, not track ids: TIDAL addresses playlist items by index, and a
/// playlist may legitimately hold the same track twice. Descending order is
/// load-bearing - removing a low index shifts everything after it down, so
/// highest-first keeps the remaining indices valid within the one call.
pub async fn remove_playlist_items(
    http: &reqwest::Client,
    access_token: &str,
    playlist_uuid: &str,
    positions: &[i64],
    country_code: &str,
) -> Result<()> {
    if positions.is_empty() {
        return Ok(());
    }
    let mut ordered: Vec<i64> = positions.to_vec();
    ordered.sort_unstable_by(|a, b| b.cmp(a));
    ordered.dedup();
    let indices = ordered
        .iter()
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(",");

    with_playlist_etag(http, access_token, playlist_uuid, country_code, |etag| {
        let indices = indices.clone();
        async move {
            Ok(http
                .delete(format!(
                    "{}/playlists/{}/items/{}?countryCode={}",
                    TIDAL_API_URL, playlist_uuid, indices, country_code
                ))
                .header("Authorization", format!("Bearer {}", access_token))
                .header("If-None-Match", etag)
                .send()
                .await?)
        }
    })
    .await
}

/// Move a playlist item from one zero-based index to another.
pub async fn move_playlist_item(
    http: &reqwest::Client,
    access_token: &str,
    playlist_uuid: &str,
    from: i64,
    to: i64,
    country_code: &str,
) -> Result<()> {
    with_playlist_etag(
        http,
        access_token,
        playlist_uuid,
        country_code,
        |etag| async move {
            Ok(http
                .post(format!(
                    "{}/playlists/{}/items/{}?countryCode={}",
                    TIDAL_API_URL, playlist_uuid, from, country_code
                ))
                .header("Authorization", format!("Bearer {}", access_token))
                .header("If-None-Match", etag)
                .form(&[("toIndex", to.to_string())])
                .send()
                .await?)
        },
    )
    .await
}

/// Rename a TIDAL playlist and/or replace its description.
pub async fn rename_playlist(
    http: &reqwest::Client,
    access_token: &str,
    playlist_uuid: &str,
    title: &str,
    description: Option<&str>,
    country_code: &str,
) -> Result<()> {
    let description = description.unwrap_or_default().to_string();
    with_playlist_etag(http, access_token, playlist_uuid, country_code, |etag| {
        let description = description.clone();
        async move {
            Ok(http
                .post(format!(
                    "{}/playlists/{}?countryCode={}",
                    TIDAL_API_URL, playlist_uuid, country_code
                ))
                .header("Authorization", format!("Bearer {}", access_token))
                .header("If-None-Match", etag)
                .form(&[("title", title.to_string()), ("description", description)])
                .send()
                .await?)
        }
    })
    .await
}

/// Delete a TIDAL playlist outright. No ETag: there is nothing left to conflict
/// with once the whole playlist is going away.
pub async fn delete_playlist(
    http: &reqwest::Client,
    access_token: &str,
    playlist_uuid: &str,
    country_code: &str,
) -> Result<()> {
    crate::services::tidal::backoff::global().check()?;
    let resp = http
        .delete(format!(
            "{}/playlists/{}?countryCode={}",
            TIDAL_API_URL, playlist_uuid, country_code
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;
    let status = resp.status();
    // A playlist that is already gone is the outcome the caller wanted.
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
    let body = resp.text().await.unwrap_or_default();
    crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
    anyhow::bail!("TIDAL mutation error {}: {}", status, body);
}
