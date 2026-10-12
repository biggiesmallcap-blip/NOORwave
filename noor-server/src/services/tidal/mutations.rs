use anyhow::Result;

use super::client::TidalClient;

#[derive(Debug, thiserror::Error)]
#[error(
    "The unfinished TIDAL playlist {playlist_uuid} could not be removed. Check your TIDAL playlists before retrying."
)]
pub struct UnfinishedPlaylist {
    pub playlist_uuid: String,
    #[source]
    source: anyhow::Error,
}

/// Create a named snapshot in the user's collection, preserving repeated tracks.
pub async fn create_playlist_snapshot(
    client: &TidalClient,
    title: &str,
    description: Option<&str>,
    track_ids: &[i64],
) -> Result<String> {
    check_library_writes()?;
    create_playlist_snapshot_at(client, title, description, track_ids).await
}

async fn create_playlist_snapshot_at(
    client: &TidalClient,
    title: &str,
    description: Option<&str>,
    track_ids: &[i64],
) -> Result<String> {
    anyhow::ensure!(!track_ids.is_empty(), "Cannot save an empty snapshot");
    let country_code = client.country_code();
    let response = client
        .send_authed(|http, api_base, bearer| {
            let base = api_base.trim_end_matches("/v1");
            http.put(format!(
                "{base}/v2/my-collection/playlists/folders/create-playlist"
            ))
            .header("Authorization", bearer)
            .query(&[
                ("name", title),
                ("description", description.unwrap_or_default()),
                ("folderId", "root"),
                ("countryCode", country_code),
            ])
        })
        .await?;
    let body: serde_json::Value = checked_playlist_response(response).await?.json().await?;
    let uuid = body
        .pointer("/data/uuid")
        .and_then(|value| value.as_str())
        .filter(|uuid| !uuid.is_empty())
        .ok_or_else(|| anyhow::anyhow!("TIDAL did not return the created playlist id"))?
        .to_string();
    let populate = async {
        for batch in track_ids.chunks(100) {
            let etag = get_playlist_etag(client, &uuid).await?;
            let ids = batch
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let response = client
                .send_authed(|http, api_base, bearer| {
                    http.post(format!("{api_base}/playlists/{uuid}/items"))
                        .header("Authorization", bearer)
                        .header("If-None-Match", &etag)
                        .query(&[("countryCode", country_code)])
                        .form(&[
                            ("trackIds", ids.as_str()),
                            ("onDupes", "ADD"),
                            ("onArtifactNotFound", "FAIL"),
                        ])
                })
                .await?;
            let body: serde_json::Value = checked_playlist_response(response).await?.json().await?;
            let added = body.get("addedItemIds").and_then(|value| value.as_array());
            anyhow::ensure!(
                added.is_some_and(|items| items.len() == batch.len()),
                "TIDAL did not save every track in the snapshot"
            );
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = populate {
        if delete_playlist(client, &uuid).await.is_err() {
            return Err(UnfinishedPlaylist {
                playlist_uuid: uuid,
                source: error,
            }
            .into());
        }
        return Err(error);
    }
    Ok(uuid)
}

async fn checked_playlist_response(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after = crate::services::tidal::backoff::retry_after_secs(response.headers());
    let body = response.text().await.unwrap_or_default();
    crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
    anyhow::bail!("TIDAL playlist error {status}: {body}")
}

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
    struct NoRefresh;
    #[async_trait::async_trait]
    impl crate::services::tidal::session::TokenRefresher for NoRefresh {
        async fn refresh(
            &self,
            _: &crate::services::tidal::auth::TidalTokens,
        ) -> anyhow::Result<crate::services::tidal::auth::TidalTokens> {
            anyhow::bail!("Unexpected token refresh in snapshot fixture")
        }
    }
    #[tokio::test]
    async fn playlist_snapshot_save_populates_with_etag_and_cleans_up_partial_saves() {
        use axum::{Router, body::Body, http::Request};
        use std::sync::{Arc, Mutex};
        for (partial, cleanup_fails) in [(false, false), (true, false), (true, true)] {
            let calls = Arc::new(Mutex::new(Vec::<String>::new()));
            let recorded = calls.clone();
            let app = Router::new().fallback(move |request: Request<Body>| {
                let recorded = recorded.clone();
                async move {
                    let method = request.method().to_string();
                    let uri = request.uri().to_string();
                    let etag = request
                        .headers()
                        .get("If-None-Match")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let body = axum::body::to_bytes(request.into_body(), 4096)
                        .await
                        .unwrap();
                    recorded.lock().unwrap().push(format!(
                        "{method} {uri} {etag} {}",
                        String::from_utf8_lossy(&body)
                    ));
                    let json = match method.as_str() {
                        "PUT" => r#"{"data":{"uuid":"saved-uuid"}}"#,
                        "GET" => "{}",
                        "POST" if partial => r#"{"addedItemIds":[42]}"#,
                        "POST" => r#"{"addedItemIds":[42,17,42]}"#,
                        _ => "{}",
                    };
                    let status = if method == "DELETE" && cleanup_fails {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        axum::http::StatusCode::OK
                    };
                    (
                        status,
                        [("content-type", "application/json"), ("etag", "revision-1")],
                        json,
                    )
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let session = crate::services::tidal::session::TidalSession::new(
                crate::services::tidal::session::TidalSessionConfig {
                    api_http: reqwest::Client::new(),
                    api_base: format!("http://{address}/v1"),
                    refresher: Arc::new(NoRefresh),
                    store: None,
                    events: None,
                },
                Some(crate::services::tidal::auth::TidalTokens {
                    access_token: "test-token".into(),
                    refresh_token: "test-refresh".into(),
                    token_type: "Bearer".into(),
                    expires_in: 3600,
                    user_id: "test".into(),
                    country_code: "AU".into(),
                    auth_flow: Some("pkce".into()),
                }),
            );
            let result = super::create_playlist_snapshot_at(
                &session.client().unwrap(),
                "Saved mix",
                None,
                &[42, 17, 42],
            )
            .await;
            let calls = calls.lock().unwrap();
            assert!(calls[0].contains(
                "PUT /v2/my-collection/playlists/folders/create-playlist?name=Saved+mix"
            ));
            assert!(calls[2].contains("revision-1 trackIds=42%2C17%2C42"));
            assert!(calls[2].contains("onDupes=ADD"));
            assert!(calls[2].contains("onArtifactNotFound=FAIL"));
            if partial {
                assert!(result.is_err());
                assert!(calls[3].starts_with("DELETE /v1/playlists/saved-uuid"));
                if cleanup_fails {
                    assert!(
                        result
                            .unwrap_err()
                            .downcast_ref::<super::UnfinishedPlaylist>()
                            .is_some()
                    );
                }
            } else {
                assert_eq!(result.unwrap(), "saved-uuid");
                assert_eq!(calls.len(), 3);
            }
            server.abort();
        }
    }

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
    check_library_writes()?;
    with_playlist_etag(client, playlist_uuid, |http, api_base, bearer, etag| {
        http.post(format!(
            "{api_base}/playlists/{playlist_uuid}/items?countryCode={country_code}"
        ))
        .header("Authorization", bearer)
        .header("If-None-Match", etag)
        .form(&[
            ("trackIds", ids.as_str()),
            ("onDupes", "SKIP"),
            ("onArtifactNotFound", "FAIL"),
        ])
    })
    .await
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
