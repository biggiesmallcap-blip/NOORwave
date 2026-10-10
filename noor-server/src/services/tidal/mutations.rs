use anyhow::Result;

use super::client::TidalClient;

fn writes_allowed(policy: Option<&str>, development: bool) -> bool {
    match policy {
        Some("allow") => true,
        Some("deny") => false,
        Some(_) => false,
        None => !development,
    }
}

pub(crate) fn check_library_writes() -> Result<()> {
    let development = cfg!(debug_assertions)
        || std::env::var_os("NOOR_DEV_PORT").is_some()
        || std::env::current_exe()
            .ok()
            .is_some_and(|p| p.components().any(|v| v.as_os_str() == "target"));
    anyhow::ensure!(
        writes_allowed(
            std::env::var("NOOR_TIDAL_LIBRARY_WRITES").ok().as_deref(),
            development
        ),
        "TIDAL library writes are disabled for this development instance"
    );
    Ok(())
}

#[cfg(test)]
mod write_policy_tests {
    #[test]
    fn development_denies_writes_and_installed_explicit_actions_remain_enabled() {
        assert!(!super::writes_allowed(None, true));
        assert!(super::writes_allowed(None, false));
        assert!(super::writes_allowed(Some("allow"), true));
        assert!(!super::writes_allowed(Some("deny"), false));
        assert!(!super::writes_allowed(Some("unknown"), false));
    }
}

/// Turn a non-success mutation answer into an error, feeding the backoff gate.
async fn ensure_success(resp: reqwest::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
    let body = resp.text().await.unwrap_or_default();
    crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
    anyhow::bail!("TIDAL mutation error {}: {}", status, body);
}

/// Add a track to TIDAL favorites.
pub async fn add_favorite_track(client: &TidalClient, user_id: &str, track_id: i64) -> Result<()> {
    check_library_writes()?;
    add_favorite_track_unchecked(client, user_id, track_id).await
}

/// `add_favorite_track` without the write-policy gate (tests call this).
pub(crate) async fn add_favorite_track_unchecked(
    client: &TidalClient,
    user_id: &str,
    track_id: i64,
) -> Result<()> {
    let country_code = client.country_code().to_string();
    let resp = client
        .send_authed(|http, api_base, bearer| {
            http.post(format!(
                "{}/users/{}/favorites/tracks?countryCode={}",
                api_base, user_id, country_code
            ))
            .header("Authorization", bearer)
            .form(&[("trackIds", track_id.to_string())])
        })
        .await?;
    ensure_success(resp).await
}

/// Remove a track from TIDAL favorites.
pub async fn remove_favorite_track(
    client: &TidalClient,
    user_id: &str,
    track_id: i64,
) -> Result<()> {
    check_library_writes()?;
    let country_code = client.country_code().to_string();
    let resp = client
        .send_authed(|http, api_base, bearer| {
            http.delete(format!(
                "{}/users/{}/favorites/tracks/{}?countryCode={}",
                api_base, user_id, track_id, country_code
            ))
            .header("Authorization", bearer)
        })
        .await?;
    ensure_success(resp).await
}

/// Add an album to TIDAL favorites.
pub async fn add_favorite_album(client: &TidalClient, user_id: &str, album_id: i64) -> Result<()> {
    check_library_writes()?;
    let country_code = client.country_code().to_string();
    let resp = client
        .send_authed(|http, api_base, bearer| {
            http.post(format!(
                "{}/users/{}/favorites/albums?countryCode={}",
                api_base, user_id, country_code
            ))
            .header("Authorization", bearer)
            .form(&[("albumIds", album_id.to_string())])
        })
        .await?;
    ensure_success(resp).await
}

/// Remove an album from TIDAL favorites.
pub async fn remove_favorite_album(
    client: &TidalClient,
    user_id: &str,
    album_id: i64,
) -> Result<()> {
    check_library_writes()?;
    let country_code = client.country_code().to_string();
    let resp = client
        .send_authed(|http, api_base, bearer| {
            http.delete(format!(
                "{}/users/{}/favorites/albums/{}?countryCode={}",
                api_base, user_id, album_id, country_code
            ))
            .header("Authorization", bearer)
        })
        .await?;
    ensure_success(resp).await
}

/// Add tracks to a TIDAL playlist.
pub async fn add_to_playlist(
    client: &TidalClient,
    playlist_uuid: &str,
    track_ids: &[i64],
) -> Result<()> {
    let ids: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let country_code = client.country_code().to_string();
    let resp = client
        .send_authed(|http, api_base, bearer| {
            http.post(format!(
                "{}/playlists/{}/items?countryCode={}",
                api_base, playlist_uuid, country_code
            ))
            .header("Authorization", bearer)
            .form(&[("trackIds", ids.clone())])
        })
        .await?;
    ensure_success(resp).await
}

pub async fn remove_favorite_tracks(
    client: &TidalClient,
    user_id: &str,
    track_ids: &[i64],
) -> Result<usize> {
    let mut removed = 0;
    for track_id in track_ids {
        remove_favorite_track(client, user_id, *track_id).await?;
        removed += 1;
    }
    Ok(removed)
}

pub async fn remove_favorite_albums(
    client: &TidalClient,
    user_id: &str,
    album_ids: &[i64],
) -> Result<usize> {
    let mut removed = 0;
    for album_id in album_ids {
        remove_favorite_album(client, user_id, *album_id).await?;
        removed += 1;
    }
    Ok(removed)
}

// --- Playlist edits ----------------------------------------------------------
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
pub async fn get_playlist_etag(client: &TidalClient, playlist_uuid: &str) -> Result<String> {
    let country_code = client.country_code().to_string();
    let resp = client
        .send_authed(|http, api_base, bearer| {
            http.get(format!(
                "{}/playlists/{}/items?countryCode={}&limit=1&offset=0",
                api_base, playlist_uuid, country_code
            ))
            .header("Authorization", bearer)
        })
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
/// once on a 412 before giving up. `build(http, api_base, bearer, etag)`
/// builds the request.
async fn with_playlist_etag<F>(client: &TidalClient, playlist_uuid: &str, build: F) -> Result<()>
where
    F: Fn(&reqwest::Client, &str, &str, &str) -> reqwest::RequestBuilder,
{
    for attempt in 0..2 {
        let etag = get_playlist_etag(client, playlist_uuid).await?;
        let resp = client
            .send_authed(|http, api_base, bearer| build(http, api_base, bearer, &etag))
            .await?;
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
    client: &TidalClient,
    playlist_uuid: &str,
    positions: &[i64],
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
    let country_code = client.country_code().to_string();
    with_playlist_etag(client, playlist_uuid, |http, api_base, bearer, etag| {
        http.delete(format!(
            "{}/playlists/{}/items/{}?countryCode={}",
            api_base, playlist_uuid, indices, country_code
        ))
        .header("Authorization", bearer)
        .header("If-None-Match", etag)
    })
    .await
}

/// Move a playlist item from one zero-based index to another.
pub async fn move_playlist_item(
    client: &TidalClient,
    playlist_uuid: &str,
    from: i64,
    to: i64,
) -> Result<()> {
    let country_code = client.country_code().to_string();
    with_playlist_etag(client, playlist_uuid, |http, api_base, bearer, etag| {
        http.post(format!(
            "{}/playlists/{}/items/{}?countryCode={}",
            api_base, playlist_uuid, from, country_code
        ))
        .header("Authorization", bearer)
        .header("If-None-Match", etag)
        .form(&[("toIndex", to.to_string())])
    })
    .await
}

/// Rename a TIDAL playlist and/or replace its description.
pub async fn rename_playlist(
    client: &TidalClient,
    playlist_uuid: &str,
    title: &str,
    description: Option<&str>,
) -> Result<()> {
    let description = description.unwrap_or_default().to_string();
    let country_code = client.country_code().to_string();
    with_playlist_etag(client, playlist_uuid, |http, api_base, bearer, etag| {
        http.post(format!(
            "{}/playlists/{}?countryCode={}",
            api_base, playlist_uuid, country_code
        ))
        .header("Authorization", bearer)
        .header("If-None-Match", etag)
        .form(&[
            ("title", title.to_string()),
            ("description", description.clone()),
        ])
    })
    .await
}

/// Delete a TIDAL playlist outright. No ETag: there is nothing left to conflict
/// with once the whole playlist is going away.
pub async fn delete_playlist(client: &TidalClient, playlist_uuid: &str) -> Result<()> {
    let country_code = client.country_code().to_string();
    let resp = client
        .send_authed(|http, api_base, bearer| {
            http.delete(format!(
                "{}/playlists/{}?countryCode={}",
                api_base, playlist_uuid, country_code
            ))
            .header("Authorization", bearer)
        })
        .await?;
    // A playlist that is already gone is the outcome the caller wanted.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    ensure_success(resp).await
}
