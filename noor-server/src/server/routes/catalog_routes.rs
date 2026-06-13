use super::{
    error_looks_like_auth, load_persisted_tidal_tokens, recover_tidal_session,
    tidal_track_playable_json,
};
use crate::SharedState;
use crate::db::queries;
use crate::services::tidal::{
    client::{TidalAlbum, TidalClient},
    import as tidal_import,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};

const CATALOG_LIST_LIMIT_MAX: i64 = 200;

fn require_positive_tidal_album_id(tidal_album_id: i64) -> Result<(), (StatusCode, Json<Value>)> {
    if tidal_album_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive TIDAL album id" })),
        ));
    }
    Ok(())
}

fn require_positive_local_id(id: i64) -> Result<(), StatusCode> {
    if id <= 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

fn require_positive_local_id_json(id: i64) -> Result<(), (StatusCode, Json<Value>)> {
    if id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive library id" })),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(super) struct ListParams {
    sort_by: Option<String>,
    sort_dir: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    // Legacy naming: despite "favorite_only", this means "library tracks" =
    // tracks where tracks.is_favorite=1 OR the parent album has albums.is_favorite=1.
    // For a strict "user explicitly liked this track" filter, use `liked_only` instead.
    favorite_only: Option<bool>,
    // Strict filter: tracks where tracks.is_favorite=1 only. Takes precedence
    // over `favorite_only` when both are set.
    liked_only: Option<bool>,
    // DSP filter params
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    energy_min: Option<f64>,
    energy_max: Option<f64>,
    key_signature: Option<String>,
    instrumental_only: Option<bool>,
}
pub(super) async fn get_tracks(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let sort_by = normalize_catalog_sort_param(params.sort_by.as_deref(), "date_added");
    let sort_dir = normalize_catalog_sort_dir(params.sort_dir.as_deref(), "desc");
    let limit = clamp_catalog_list_limit(params.limit, 50);
    let offset = clamp_catalog_offset(params.offset);
    let favorite_only = params.favorite_only.unwrap_or(false);
    let liked_only = params.liked_only.unwrap_or(false);

    let dsp = queries::DspFilters {
        bpm_min: params.bpm_min,
        bpm_max: params.bpm_max,
        energy_min: params.energy_min,
        energy_max: params.energy_max,
        key_signature: params.key_signature.clone(),
        instrumental_only: params.instrumental_only.unwrap_or(false),
    };

    state
        .db
        .with_conn(|conn| {
            let tracks = queries::get_tracks_with_dsp(
                conn,
                &sort_by,
                &sort_dir,
                limit,
                offset,
                favorite_only,
                liked_only,
                &dsp,
            )?;
            let total = queries::get_track_count(conn, favorite_only, liked_only)?;
            Ok(Json(json!({ "tracks": tracks, "total": total })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_track_count(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let favorite_only = params.favorite_only.unwrap_or(false);
    let liked_only = params.liked_only.unwrap_or(false);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let count = queries::get_track_count(conn, favorite_only, liked_only)?;
            Ok(Json(json!({ "count": count })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_albums(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let sort_by = normalize_catalog_sort_param(params.sort_by.as_deref(), "title");
    let sort_dir = normalize_catalog_sort_dir(params.sort_dir.as_deref(), "asc");
    let limit = clamp_catalog_list_limit(params.limit, 100);
    let offset = clamp_catalog_offset(params.offset);
    let favorite_only = params.favorite_only.unwrap_or(false);

    state
        .db
        .with_conn(|conn| {
            let albums =
                queries::get_albums(conn, &sort_by, &sort_dir, limit, offset, favorite_only)?;
            let total = queries::get_album_count(conn, favorite_only)?;
            Ok(Json(json!({ "albums": albums, "total": total })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_artists(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let sort_by = normalize_catalog_sort_param(params.sort_by.as_deref(), "name");
    let sort_dir = normalize_catalog_sort_dir(params.sort_dir.as_deref(), "asc");
    let limit = clamp_catalog_list_limit(params.limit, 50);
    let offset = clamp_catalog_offset(params.offset);

    state
        .db
        .with_conn(|conn| {
            let artists = queries::get_artists(conn, &sort_by, &sort_dir, limit, offset)?;
            Ok(Json(json!({ "artists": artists })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn clamp_catalog_list_limit(limit: Option<i64>, default: i64) -> i64 {
    limit.unwrap_or(default).clamp(1, CATALOG_LIST_LIMIT_MAX)
}

fn clamp_catalog_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}

fn normalize_catalog_sort_param(value: Option<&str>, default: &str) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn normalize_catalog_sort_dir(value: Option<&str>, default: &str) -> String {
    match value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .to_ascii_lowercase()
        .as_str()
    {
        "asc" => "asc".to_string(),
        "desc" => "desc".to_string(),
        _ => default.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_list_limit_is_bounded() {
        assert_eq!(clamp_catalog_list_limit(None, 50), 50);
        assert_eq!(clamp_catalog_list_limit(Some(-10), 50), 1);
        assert_eq!(clamp_catalog_list_limit(Some(0), 50), 1);
        assert_eq!(clamp_catalog_list_limit(Some(10_000), 50), 200);
    }

    #[test]
    fn catalog_offset_is_nonnegative() {
        assert_eq!(clamp_catalog_offset(None), 0);
        assert_eq!(clamp_catalog_offset(Some(-100)), 0);
        assert_eq!(clamp_catalog_offset(Some(25)), 25);
    }

    #[test]
    fn catalog_sort_params_are_normalized() {
        assert_eq!(
            normalize_catalog_sort_param(Some(" title "), "date_added"),
            "title"
        );
        assert_eq!(normalize_catalog_sort_param(Some("   "), "name"), "name");
        assert_eq!(normalize_catalog_sort_dir(Some(" ASC "), "desc"), "asc");
        assert_eq!(normalize_catalog_sort_dir(Some(" desc "), "asc"), "desc");
        assert_eq!(normalize_catalog_sort_dir(Some("sideways"), "asc"), "asc");
    }
}

pub(super) async fn get_artist_tracks(
    State(state): State<SharedState>,
    axum::extract::Path(artist_id): axum::extract::Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_local_id(artist_id)?;
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks = queries::get_artist_library_tracks(conn, artist_id)?;
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_artist(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_local_id(id)?;
    let s = state.read().await;
    let row =
        s.db.with_conn(|conn| queries::get_artist_with_counts(conn, id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let Some((artist, track_count, album_count)) = row else {
        return Err(StatusCode::NOT_FOUND);
    };

    Ok(Json(json!({
        "id": artist.id,
        "tidal_id": artist.tidal_id,
        "name": artist.name,
        "biography": artist.biography,
        "photo_url": artist.photo_url,
        "track_count": track_count,
        "album_count": album_count,
    })))
}

pub(super) async fn get_album_tracks(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_local_id(id)?;
    // Three-pass approach so the page can render the FULL album (not just
    // library coverage):
    //   1. Pull the local rows + the album's TIDAL id in one DB hit.
    //   2. If TIDAL is connected and the album maps to a TIDAL id, fetch the
    //      full TIDAL track list.
    //   3. Filter TIDAL tracks down to only those NOT already in `tracks`
    //      (deduped by tidal_id) and serialize as `tidal_tracks`.
    //
    // The frontend renders both arrays; the user gets a single coherent track
    // listing where library entries are styled as "owned" and pure-TIDAL
    // entries get a TIDAL pill.
    let (tracks, album_tidal_id) = {
        let s = state.read().await;
        let result = s.db.with_conn(|conn| {
            let tracks = queries::get_album_tracks(conn, id)?;
            let pairs = queries::get_album_tidal_ids(conn, &[id])?;
            let tidal_id = pairs.first().map(|(_, t)| *t);
            Ok::<_, anyhow::Error>((tracks, tidal_id))
        });
        match result {
            Ok((tracks, tidal_id)) => (tracks, tidal_id),
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    };

    // No TIDAL id -> can't enrich; return library tracks alone.
    let Some(tidal_album_id) = album_tidal_id else {
        return Ok(Json(json!({
            "tracks": tracks,
            "tidal_tracks": [],
            "album_tidal_id": null,
        })));
    };

    // TIDAL session needed for the catalog fetch - best-effort only.
    let (tokens, tidal_http_client) = {
        let persisted = match load_persisted_tidal_tokens(&state).await {
            Ok(p) => p,
            Err(_) => None,
        };
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Ok(Json(json!({
            "tracks": tracks,
            "tidal_tracks": [],
            "album_tidal_id": tidal_album_id,
        })));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    // Pre-fix legacy rows that landed with NULL track_number â€” `TidalTrack`
    // shipped without #[serde(rename = "trackNumber")] for a long time, so
    // every TIDAL-imported track row stored NULL and the album sort fell
    // back to alphabetical. Backfill from the live TIDAL payload on first
    // view post-fix.
    let needs_backfill = tracks
        .iter()
        .any(|t| t.track_number.is_none() && t.tidal_id.is_some());

    let tidal_tracks_payload: Vec<Value> = match client.get_all_album_tracks(tidal_album_id).await {
        Ok(tidal_tracks) => {
            if needs_backfill {
                let backfill_pairs: Vec<(i64, i32, i32)> = tidal_tracks
                    .iter()
                    .filter_map(|t| Some((t.id, t.track_number?, t.volume_number.unwrap_or(1))))
                    .collect();
                if !backfill_pairs.is_empty() {
                    let s = state.read().await;
                    let _ = s.db.with_conn(|conn| {
                        let tx = conn.unchecked_transaction()?;
                        let mut count = 0i64;
                        {
                            let mut stmt = tx.prepare(
                                "UPDATE tracks
                                 SET track_number = COALESCE(track_number, ?2),
                                     disc_number  = COALESCE(disc_number, ?3)
                                 WHERE tidal_id = ?1
                                   AND (track_number IS NULL OR disc_number IS NULL)",
                            )?;
                            for (tid, tn, dn) in &backfill_pairs {
                                count += stmt.execute(rusqlite::params![tid, tn, dn])? as i64;
                            }
                        }
                        tx.commit()?;
                        if count > 0 {
                            tracing::info!(
                                target: "noor.album",
                                event = "tracknumber_backfill",
                                album_id = id,
                                updated = count
                            );
                        }
                        Ok::<_, anyhow::Error>(())
                    });
                }
            }

            // Local rows that came from TIDAL carry a `tidal_id`; dedupe so
            // the same track doesn't appear twice (once styled as library,
            // once as TIDAL-only).
            let local_tidal_ids: std::collections::HashSet<i64> =
                tracks.iter().filter_map(|t| t.tidal_id).collect();

            tidal_tracks
                .into_iter()
                .filter(|t| !local_tidal_ids.contains(&t.id))
                .map(|t| {
                    let artwork = t
                        .album
                        .as_ref()
                        .and_then(|al| al.cover.as_ref())
                        .and_then(|c| {
                            crate::services::tidal::client::TidalClient::get_artwork_url(
                                &Some(c.clone()),
                                160,
                            )
                        });
                    json!({
                        "tidal_id": t.id,
                        "title": t.title,
                        "duration_ms": t.duration * 1000,
                        "track_number": t.track_number,
                        "disc_number": t.volume_number,
                        "artist_name": t.artist.name,
                        "artist_tidal_id": t.artist.id,
                        "album_title": t.album.as_ref().map(|al| al.title.clone()),
                        "album_tidal_id": t.album.as_ref().map(|al| al.id),
                        "artwork_url": artwork,
                    })
                })
                .collect()
        }
        Err(e) => {
            tracing::warn!(
                ?e,
                "TIDAL get_all_album_tracks failed; serving library only"
            );
            Vec::new()
        }
    };

    // Reload tracks so the response reflects backfilled track/disc numbers
    // (the original `get_album_tracks` query ordered by COALESCE(..., 999999)
    // and may have produced alphabetical fallback order before the UPDATE).
    let tracks = if needs_backfill {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_album_tracks(conn, id))
            .unwrap_or(tracks)
    } else {
        tracks
    };

    Ok(Json(json!({
        "tracks": tracks,
        "tidal_tracks": tidal_tracks_payload,
        "album_tidal_id": tidal_album_id,
    })))
}

pub(super) async fn get_album_spotify_stats(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_local_id(id)?;

    #[cfg(feature = "spotify-public")]
    {
        let (db, client, tracks) = {
            let s = state.read().await;
            let tracks =
                s.db.with_conn(|conn| queries::get_album_tracks(conn, id))
                    .unwrap_or_default();
            (s.db.clone(), s.spotify_public.clone(), tracks)
        };

        let seeds: Vec<crate::services::spotify_public::TrackSeed> = tracks
            .iter()
            .filter_map(|t| {
                let isrc = t.isrc.as_deref()?.trim();
                if isrc.is_empty() {
                    return None;
                }
                Some(crate::services::spotify_public::TrackSeed {
                    isrc: isrc.to_string(),
                    title: t.title.clone(),
                    artist_name: t.artist_name.clone().unwrap_or_default(),
                })
            })
            .collect();

        let stats =
            crate::services::spotify_public::fetch_album_playcounts(&client, &db, &seeds).await;

        let payload: Vec<Value> = stats
            .into_iter()
            .filter_map(|s| {
                s.playcount.map(|pc| {
                    json!({
                        "isrc": s.isrc,
                        "title": s.title,
                        "playcount": pc,
                    })
                })
            })
            .collect();

        Ok(Json(json!({
            "monthly_listeners": null,
            "tracks": payload,
        })))
    }
    #[cfg(not(feature = "spotify-public"))]
    {
        let _ = state;
        Ok(Json(json!({
            "monthly_listeners": null,
            "tracks": [],
        })))
    }
}

/// Pages through all entries of a single TIDAL discography filter for one
/// artist. TIDAL's `/artists/{id}/albums` returns at most 50 per call, sorted
/// newest-first; calling once would silently clip anything older than the 50th
/// most-recent release per filter (i.e. anything past page 1). Stops on a
/// short page (TIDAL's "no more" signal) or when the running count reaches
/// `total_number_of_items`. Capped at 1000 entries per filter as a safety net.
async fn fetch_artist_album_pages(
    client: &TidalClient,
    artist_id: i64,
    filter: &str,
    max_pages: i32,
) -> anyhow::Result<Vec<TidalAlbum>> {
    const PAGE: i32 = 50;
    let mut out: Vec<TidalAlbum> = Vec::new();
    let mut offset: i32 = 0;
    for _ in 0..max_pages.max(1) {
        let page = client
            .get_artist_albums(artist_id, PAGE, offset, Some(filter))
            .await?;
        let n = page.items.len() as i32;
        let total = page.total_number_of_items;
        out.extend(page.items);
        if n < PAGE {
            break;
        }
        if let Some(t) = total
            && (out.len() as i64) >= t
        {
            break;
        }
        offset += PAGE;
    }
    Ok(out)
}

async fn fetch_all_artist_albums(
    client: &TidalClient,
    artist_id: i64,
    filter: &str,
) -> anyhow::Result<Vec<TidalAlbum>> {
    const MAX_PAGES: i32 = 20;
    fetch_artist_album_pages(client, artist_id, filter, MAX_PAGES).await
}

pub(crate) fn merge_tidal_artist_album_filters(
    filters: impl IntoIterator<Item = (Vec<TidalAlbum>, &'static str)>,
) -> Vec<(TidalAlbum, &'static str)> {
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut all_albums: Vec<(TidalAlbum, &'static str)> = Vec::new();
    for (items, filter) in filters {
        for item in items {
            if seen.insert(item.id) {
                all_albums.push((item, filter));
            }
        }
    }
    all_albums
}

/// Build the rich artist payload (categorized albums, top tracks, videos,
/// similar artists, bio, picture) straight from a TIDAL artist id. Shared by
/// the library discography route (which resolves a local id first) and the
/// non-library `/api/tidal/artists/{id}` route so both render the same page.
/// Each TIDAL sub-fetch degrades on its own: a failed videos call yields an
/// empty rail instead of failing the whole route.
pub(super) async fn build_tidal_artist_payload(
    state: &SharedState,
    client: &TidalClient,
    tidal_artist_id: i64,
) -> Value {
    // Each filter is paginated separately; previously we fetched only the first
    // page (50 newest), which clipped any artist with a long catalog (e.g. a
    // 50+ year discography returned only modern compilations).
    let albums_fut = fetch_all_artist_albums(client, tidal_artist_id, "ALBUMS");
    let eps_fut = fetch_all_artist_albums(client, tidal_artist_id, "EPSANDSINGLES");
    let compilations_fut = fetch_all_artist_albums(client, tidal_artist_id, "COMPILATIONS");
    let live_fut = fetch_all_artist_albums(client, tidal_artist_id, "LIVE");
    // Top tracks raised from 10 -> 50 so the merged Top Tracks list on the
    // artist page surfaces a meaningful catalog even when the user has zero
    // library matches; 50 is TIDAL's per-page max.
    let top_fut = client.get_artist_top_tracks(tidal_artist_id, 50, 0);
    let videos_fut = client.get_artist_videos(tidal_artist_id, 50, 0);
    let similar_fut = client.get_artist_similar(tidal_artist_id, 20, 0);
    let bio_fut = client.get_artist_bio(tidal_artist_id);
    // Profile fetch in the same parallel batch - gives us the artist's
    // canonical `picture` URL so the page hero can fall back to TIDAL
    // when the local row has no `photo_url`.
    let profile_fut = client.get_artist(tidal_artist_id);

    let (
        albums_res,
        eps_res,
        comps_res,
        live_res,
        top_res,
        videos_res,
        similar_res,
        bio_res,
        profile_res,
    ) = tokio::join!(
        albums_fut,
        eps_fut,
        compilations_fut,
        live_fut,
        top_fut,
        videos_fut,
        similar_fut,
        bio_fut,
        profile_fut
    );

    // Picture URL fallback chain. TIDAL's `/artists/{id}` record is the
    // canonical source, but it ships `picture: null` for many artists.
    // We then try the artist's own `picture` as embedded in their top
    // tracks, then finally fall back to an album cover - same trick the
    // library Recently Played Artists rail uses to keep tiles populated
    // when no artist photo exists.
    let direct_picture_id = profile_res.as_ref().ok().and_then(|a| a.picture.clone());
    let top_track_picture_id = top_res.as_ref().ok().and_then(|tr| {
        tr.items
            .iter()
            .filter(|t| t.artist.id == tidal_artist_id)
            .find_map(|t| t.artist.picture.clone())
    });
    let album_cover_picture_id = [&albums_res, &eps_res, &comps_res, &live_res]
        .iter()
        .filter_map(|res| res.as_ref().ok())
        .flat_map(|list| list.iter())
        .find_map(|a| a.cover.clone());

    // TIDAL's CDN ships `640x640.jpg` reliably for album covers but not
    // for artist pictures - many artist images are stored at 320 max.
    // Pick the size that matches whichever tier resolved.
    let (resolved_picture_id, picture_size) = if let Some(id) = direct_picture_id {
        (Some(id), 320)
    } else if let Some(id) = top_track_picture_id {
        (Some(id), 320)
    } else if let Some(id) = album_cover_picture_id {
        (Some(id), 640)
    } else {
        (None, 320)
    };
    let picture_url = TidalClient::get_artwork_url(&resolved_picture_id, picture_size);
    if let Err(e) = profile_res.as_ref() {
        tracing::debug!(
            "TIDAL artist {} profile fetch failed: {}",
            tidal_artist_id,
            e
        );
    }

    // TIDAL can return the same release under multiple filters (e.g. an album
    // re-issue tagged both ALBUMS and COMPILATIONS). Dedupe by tidal_id while
    // preserving the order of first appearance. Each entry remembers the
    // filter it came from so the frontend can bucket it correctly - TIDAL's
    // per-album `release_type` body field is unreliable and was the original
    // reason Singles / Compilations sections were silently empty.
    let all_albums = merge_tidal_artist_album_filters([
        (albums_res.unwrap_or_default(), "ALBUMS"),
        (eps_res.unwrap_or_default(), "EPSANDSINGLES"),
        (comps_res.unwrap_or_default(), "COMPILATIONS"),
        (live_res.unwrap_or_default(), "LIVE"),
    ]);

    let tidal_album_ids: Vec<i64> = all_albums.iter().map(|(a, _)| a.id).collect();
    let known_map = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_known_album_tidal_ids(conn, &tidal_album_ids))
            .unwrap_or_default()
    };

    let albums_payload: Vec<Value> = all_albums
        .into_iter()
        .map(|(a, source_filter)| {
            let artwork =
                crate::services::tidal::client::TidalClient::get_artwork_url(&a.cover, 320);
            let local_id = known_map.get(&a.id).copied();
            json!({
                "tidal_id": a.id,
                "local_id": local_id,
                "title": a.title,
                "artwork_url": artwork,
                "release_date": a.release_date,
                "release_type": a.release_type,
                "source_filter": source_filter,
                "number_of_tracks": a.number_of_tracks,
                "artist_name": a.artist.name,
                "in_library": local_id.is_some()
            })
        })
        .collect();

    let top_tracks_payload: Vec<Value> = top_res
        .map(|r| {
            r.items
                .into_iter()
                .map(|t| {
                    let artwork = t
                        .album
                        .as_ref()
                        .and_then(|al| al.cover.as_ref())
                        .and_then(|c| {
                            crate::services::tidal::client::TidalClient::get_artwork_url(
                                &Some(c.clone()),
                                160,
                            )
                        });
                    json!({
                        "tidal_id": t.id,
                        "title": t.title,
                        "duration_ms": t.duration * 1000,
                        "artwork_url": artwork,
                        "album_title": t.album.as_ref().map(|al| al.title.clone()),
                        "album_tidal_id": t.album.as_ref().map(|al| al.id),
                        "track_number": t.track_number,
                        "disc_number": t.volume_number,
                        "artist_name": t.artist.name,
                        "artist_tidal_id": t.artist.id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let videos_payload: Vec<Value> = videos_res
        .map(|r| {
            r.items
                .into_iter()
                .map(|v| {
                    let artwork = crate::services::tidal::client::TidalClient::get_artwork_url(
                        &v.image_id,
                        320,
                    );
                    let artist_name = v.artist.map(|a| a.name);
                    json!({
                        "tidal_id": v.id,
                        "title": v.title,
                        "duration_ms": v.duration * 1000,
                        "artwork_url": artwork,
                        "artist_name": artist_name,
                        "album_tidal_id": v.album.as_ref().map(|al| al.id),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Resolve `local_id` per similar artist via the same lookup pattern used
    // for albums above - lets the frontend route /artists/[local_id] when
    // present (preserving library-affordances) and /tidal/artists/[id] otherwise.
    let similar_items: Vec<crate::services::tidal::client::TidalArtist> =
        similar_res.map(|r| r.items).unwrap_or_default();
    let similar_tidal_ids: Vec<i64> = similar_items.iter().map(|a| a.id).collect();
    let similar_known_map = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_known_artist_tidal_ids(conn, &similar_tidal_ids))
            .unwrap_or_default()
    };
    let similar_artists_payload: Vec<Value> = similar_items
        .into_iter()
        .map(|a| {
            let artwork =
                crate::services::tidal::client::TidalClient::get_artwork_url(&a.picture, 320);
            let local_id = similar_known_map.get(&a.id).copied();
            json!({
                "tidal_id": a.id,
                "local_id": local_id,
                "name": a.name,
                "artwork_url": artwork,
                "in_library": local_id.is_some(),
            })
        })
        .collect();

    let bio_payload = bio_res.ok().map(|b| {
        json!({
            "summary": b.summary,
            "text": b.text,
            "source": b.source,
        })
    });

    let artist_name = profile_res
        .as_ref()
        .ok()
        .map(|a| a.name.clone())
        .or_else(|| {
            top_tracks_payload.first().and_then(|t| {
                t.get("artist_name")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
        });

    json!({
        "artist_name": artist_name,
        "albums": albums_payload,
        "top_tracks": top_tracks_payload,
        "videos": videos_payload,
        "similar_artists": similar_artists_payload,
        "bio": bio_payload,
        "picture_url": picture_url,
        "available": true
    })
}

pub(super) async fn get_artist_discography(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_local_id_json(id)?;
    let tidal_artist_id = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_artist_tidal_id(conn, id))
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
            })?
    };

    let Some(tidal_artist_id) = tidal_artist_id else {
        return Ok(Json(json!({
            "albums": [],
            "top_tracks": [],
            "available": false,
            "reason": "artist_not_on_tidal"
        })));
    };

    let (tokens, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Ok(Json(json!({
            "albums": [],
            "top_tracks": [],
            "available": false,
            "reason": "tidal_not_connected"
        })));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let payload = build_tidal_artist_payload(&state, &client, tidal_artist_id).await;

    // Best-effort persistence of bio text to the local artists row so the
    // page can render it offline next time. Only writes when the local row
    // had no biography of its own.
    let bio_text = payload.get("bio").and_then(|b| {
        b.get("text")
            .and_then(|v| v.as_str())
            .or_else(|| b.get("summary").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
    });
    if let Some(text) = bio_text {
        let s = state.read().await;
        let _ = s.db.with_conn(|conn| {
            conn.execute(
                "UPDATE artists SET biography = ?1
                 WHERE id = ?2 AND (biography IS NULL OR biography = '')",
                rusqlite::params![text, id],
            )?;
            Ok(())
        });
    }

    // Backfill the local artists row's `photo_url` so other surfaces in the
    // app (Library Recently Played Artists, search results, etc.) get the
    // working URL too. Older sync runs sometimes stored sizes TIDAL no
    // longer serves (e.g. 640x640 returning AccessDenied for some artists);
    // overwriting whenever the resolved URL differs keeps the cache fresh.
    if let Some(url) = payload.get("picture_url").and_then(|v| v.as_str()) {
        let s = state.read().await;
        let _ = s.db.with_conn(|conn| {
            conn.execute(
                "UPDATE artists SET photo_url = ?1
                 WHERE id = ?2 AND (photo_url IS NULL OR photo_url != ?1)",
                rusqlite::params![url, id],
            )?;
            Ok(())
        });
    }

    Ok(Json(payload))
}

pub(super) async fn get_artist_spotify_stats(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_local_id(id)?;

    #[cfg(feature = "spotify-public")]
    {
        let (db, client, tidal_id, artist_name, seeds) = {
            let s = state.read().await;
            let tidal_id =
                s.db.with_conn(|conn| queries::get_artist_tidal_id(conn, id))
                    .unwrap_or(None);
            let artist_tracks =
                s.db.with_conn(|conn| queries::get_artist_tracks(conn, id))
                    .unwrap_or_default();

            let mut sorted = artist_tracks
                .into_iter()
                .filter(|t| t.isrc.as_deref().is_some_and(|s| !s.trim().is_empty()))
                .collect::<Vec<_>>();
            sorted.sort_by(|a, b| b.play_count.cmp(&a.play_count));
            sorted.truncate(10);

            let artist_name = sorted
                .first()
                .and_then(|t| t.artist_name.clone())
                .unwrap_or_default();

            let seeds: Vec<crate::services::spotify_public::TrackSeed> = sorted
                .into_iter()
                .map(|t| crate::services::spotify_public::TrackSeed {
                    isrc: t.isrc.unwrap_or_default(),
                    title: t.title,
                    artist_name: t.artist_name.unwrap_or_default(),
                })
                .collect();

            (
                s.db.clone(),
                s.spotify_public.clone(),
                tidal_id,
                artist_name,
                seeds,
            )
        };

        // Local-only artist (no Tidal id): no key for the map table, so just
        // serve any cached track playcounts via the album helper, no artist
        // resolution attempted, no negative-cache row written.
        let Some(tidal_id) = tidal_id else {
            let stats =
                crate::services::spotify_public::fetch_album_playcounts(&client, &db, &seeds).await;
            let tracks: Vec<Value> = stats
                .into_iter()
                .filter_map(|s| {
                    s.playcount.map(|pc| {
                        json!({
                            "isrc": s.isrc,
                            "title": s.title,
                            "playcount": pc,
                        })
                    })
                })
                .collect();
            return Ok(Json(json!({
                "monthly_listeners": null,
                "followers": null,
                "world_rank": null,
                "top_cities": [],
                "tracks": tracks,
            })));
        };

        let tidal_id_str = tidal_id.to_string();
        let result = crate::services::spotify_public::fetch_artist_stats(
            &client,
            &db,
            &tidal_id_str,
            &artist_name,
            &seeds,
        )
        .await;

        let tracks: Vec<Value> = result
            .tracks
            .into_iter()
            .filter_map(|t| {
                t.playcount.map(|pc| {
                    json!({
                        "isrc": t.isrc,
                        "title": t.title,
                        "playcount": pc,
                    })
                })
            })
            .collect();

        Ok(Json(json!({
            "monthly_listeners": result.monthly_listeners,
            "followers": result.followers,
            "world_rank": result.world_rank,
            "top_cities": result.top_cities,
            "tracks": tracks,
        })))
    }
    #[cfg(not(feature = "spotify-public"))]
    {
        let _ = state;
        Ok(Json(json!({
            "monthly_listeners": null,
            "followers": null,
            "world_rank": null,
            "top_cities": [],
            "tracks": [],
        })))
    }
}

pub(super) async fn get_tidal_album_tracks(
    State(state): State<SharedState>,
    Path(tidal_album_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_tidal_album_id(tidal_album_id)?;

    let (tokens, http_client, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let items = match client.get_all_album_tracks(tidal_album_id).await {
        Ok(items) => items,
        Err(error) if error_looks_like_auth(&error) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|refresh_error| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({
                            "error": format!("TIDAL session refresh failed: {}", refresh_error)
                        })),
                    )
                })?;
            let retry_client = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client
                .get_all_album_tracks(tidal_album_id)
                .await
                .map_err(|retry_error| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": retry_error.to_string() })),
                    )
                })?
        }
        Err(error) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": error.to_string() })),
            ));
        }
    };

    let tidal_ids: Vec<i64> = items.iter().map(|t| t.id).collect();
    let library_states = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_tidal_track_library_states(conn, &tidal_ids))
            .unwrap_or_default()
    };
    let tracks: Vec<Value> = items
        .into_iter()
        .map(|t| {
            let library_state = library_states.get(&t.id).copied();
            tidal_track_playable_json(t, library_state, 160)
        })
        .collect();

    Ok(Json(json!({ "tracks": tracks })))
}

pub(super) async fn import_tidal_album(
    State(state): State<SharedState>,
    Path(tidal_album_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_tidal_album_id(tidal_album_id)?;

    let (tokens, db, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.db.clone(),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let imported = tidal_import::import_album(&db, &client, tidal_album_id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    let tracks: Vec<Value> = imported
        .tracks
        .iter()
        .map(|t| {
            json!({
                "tidal_id": t.tidal_id,
                "local_id": t.local_id,
                "artist_id": t.artist_id,
                "album_id": t.album_id,
            })
        })
        .collect();

    Ok(Json(json!({
        "album_id": imported.album_id,
        "tracks": tracks,
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct ImportTidalTrackBody {
    tidal_id: i64,
    title: String,
    artist_name: String,
    artist_tidal_id: Option<i64>,
    album_title: Option<String>,
    album_tidal_id: Option<i64>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
}

fn tidal_track_import_bad_request(message: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message })))
}

fn require_positive_optional_tidal_id(
    value: Option<i64>,
    field: &str,
) -> Result<Option<i64>, (StatusCode, Json<Value>)> {
    if value.is_some_and(|id| id <= 0) {
        return Err(tidal_track_import_bad_request(&format!(
            "{field} must be a positive TIDAL id"
        )));
    }
    Ok(value)
}

fn require_positive_optional_duration_ms(
    value: Option<i64>,
) -> Result<Option<i64>, (StatusCode, Json<Value>)> {
    if value.is_some_and(|duration| duration <= 0) {
        return Err(tidal_track_import_bad_request(
            "duration_ms must be positive when provided",
        ));
    }
    Ok(value)
}

fn normalize_optional_nonempty_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

pub(super) async fn import_tidal_track_for_radio(
    State(state): State<SharedState>,
    Json(body): Json<ImportTidalTrackBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body.tidal_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive TIDAL track id" })),
        ));
    }

    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err(tidal_track_import_bad_request("title is required"));
    }

    let artist_name = body.artist_name.trim().to_string();
    if artist_name.is_empty() {
        return Err(tidal_track_import_bad_request("artist_name is required"));
    }

    let artist_tidal_id =
        require_positive_optional_tidal_id(body.artist_tidal_id, "artist_tidal_id")?;
    let album_tidal_id = require_positive_optional_tidal_id(body.album_tidal_id, "album_tidal_id")?;
    let duration_ms = require_positive_optional_duration_ms(body.duration_ms)?;
    let album_title = normalize_optional_nonempty_string(body.album_title);
    let artwork_url = normalize_optional_nonempty_string(body.artwork_url);

    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let imported = tidal_import::import_track_from_metadata(
        &db,
        tidal_import::ImportTrackMetadata {
            tidal_id: body.tidal_id,
            title,
            artist_name,
            artist_tidal_id,
            artist_picture: None,
            album_title,
            album_tidal_id,
            album_artwork_url: artwork_url,
            duration_ms,
        },
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;
    Ok(Json(json!({
        "tidal_id": imported.tidal_id,
        "local_id": imported.local_id,
        "artist_id": imported.artist_id,
        "album_id": imported.album_id,
    })))
}
