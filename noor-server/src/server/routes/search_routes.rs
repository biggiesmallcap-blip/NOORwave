use crate::SharedState;
use crate::db::queries;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SEARCH_LIMIT_DEFAULT: i64 = 20;
const SEARCH_LIMIT_MAX: i64 = 50;
const AUDIO_SEARCH_LIMIT_DEFAULT: usize = 50;
const AUDIO_SEARCH_LIMIT_MAX: usize = 50;
// Shuffle builds a play queue rather than a display list, so it pulls a deeper
// random sample of the matching set than the 50-row display cap allows.
const AUDIO_SHUFFLE_LIMIT_DEFAULT: usize = 200;
const AUDIO_SHUFFLE_LIMIT_MAX: usize = 500;
const VIBE_LIMIT_DEFAULT: usize = 6;
const VIBE_LIMIT_MAX: usize = 50;
const UNDERRATED_LIMIT_DEFAULT: usize = 5;
const UNDERRATED_LIMIT_MAX: usize = 50;

#[derive(Debug, Deserialize)]
pub(super) struct SearchParams {
    q: String,
    limit: Option<i64>,
}

pub(super) async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = clamp_i64_limit(params.limit, SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX);
    let query = params.q.trim().to_string();
    if query.is_empty() {
        return Ok(empty_search_response());
    }

    let db = {
        let s = state.read().await;
        s.db.clone()
    };

    let local = db
        .with_conn(|conn| queries::search(conn, &query, limit))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(search_response(local))
}

fn search_response(local: crate::db::models::SearchResults) -> Json<Value> {
    Json(json!({
        "tracks": local.tracks,
        "albums": local.albums,
        "artists": local.artists,
        "spotify_playlists": [],
    }))
}

fn empty_search_response() -> Json<Value> {
    Json(json!({
        "tracks": [],
        "albums": [],
        "artists": [],
        "spotify_playlists": [],
    }))
}

fn clamp_i64_limit(limit: Option<i64>, default: i64, max: i64) -> i64 {
    limit.unwrap_or(default).clamp(1, max)
}

fn clamp_usize_limit(limit: Option<usize>, default: usize, max: usize) -> usize {
    limit.unwrap_or(default).clamp(1, max)
}

fn positive_query_id(id: i64) -> Result<i64, StatusCode> {
    if id <= 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_search_response_keeps_route_payload_shape() {
        let Json(body) = empty_search_response();
        assert_eq!(
            body,
            json!({
                "tracks": [],
                "albums": [],
                "artists": [],
                "spotify_playlists": [],
            })
        );
    }

    #[test]
    fn search_response_keeps_spotify_playlist_field_non_blocking() {
        let Json(body) = search_response(crate::db::models::SearchResults {
            tracks: Vec::new(),
            albums: Vec::new(),
            artists: Vec::new(),
        });
        assert_eq!(body["spotify_playlists"], json!([]));
    }

    #[test]
    fn i64_limit_clamps_to_min_default_and_max() {
        assert_eq!(
            clamp_i64_limit(None, SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX),
            20
        );
        assert_eq!(
            clamp_i64_limit(Some(-10), SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX),
            1
        );
        assert_eq!(
            clamp_i64_limit(Some(0), SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX),
            1
        );
        assert_eq!(
            clamp_i64_limit(Some(5000), SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX),
            50
        );
    }

    #[test]
    fn usize_limit_clamps_to_min_default_and_max() {
        assert_eq!(
            clamp_usize_limit(None, AUDIO_SEARCH_LIMIT_DEFAULT, AUDIO_SEARCH_LIMIT_MAX),
            50
        );
        assert_eq!(
            clamp_usize_limit(Some(0), AUDIO_SEARCH_LIMIT_DEFAULT, AUDIO_SEARCH_LIMIT_MAX),
            1
        );
        assert_eq!(
            clamp_usize_limit(
                Some(5000),
                AUDIO_SEARCH_LIMIT_DEFAULT,
                AUDIO_SEARCH_LIMIT_MAX
            ),
            50
        );
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct AudioSearchRequest {
    free_text: Option<String>,
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    energy_min: Option<f64>,
    energy_max: Option<f64>,
    danceability_min: Option<f64>,
    danceability_max: Option<f64>,
    key_signature: Option<String>,
    camelot_key: Option<String>,
    year_min: Option<i64>,
    year_max: Option<i64>,
    genre_ids: Option<Vec<i64>>,
    // Raw user genre tokens ("rock", "hip-hop"); resolved server-side against
    // genres.slug/name and expanded to all descendants. Preferred over
    // genre_ids, which the client can only build from a possibly stale or
    // partially flattened tree.
    genre_slugs: Option<Vec<String>>,
    artist_contains: Option<String>,
    album_contains: Option<String>,
    track_type: Option<String>,
    is_instrumental: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
    // When set, return a true random sample of the full matching set (for the
    // library Shuffle button) instead of the deterministic display ranking.
    shuffle: Option<bool>,
    // Restrict matches to user-liked tracks (the Liked tab's Shuffle/Random).
    liked_only: Option<bool>,
}

pub(super) async fn search_audio(
    State(state): State<SharedState>,
    Json(body): Json<AudioSearchRequest>,
) -> Result<Json<Value>, StatusCode> {
    let free_text = body.free_text.unwrap_or_default();
    let shuffle = body.shuffle.unwrap_or(false);
    let limit = if shuffle {
        clamp_usize_limit(
            body.limit,
            AUDIO_SHUFFLE_LIMIT_DEFAULT,
            AUDIO_SHUFFLE_LIMIT_MAX,
        )
    } else {
        clamp_usize_limit(
            body.limit,
            AUDIO_SEARCH_LIMIT_DEFAULT,
            AUDIO_SEARCH_LIMIT_MAX,
        )
    };
    let offset = body.offset.unwrap_or(0);
    let genre_tokens = body.genre_slugs.unwrap_or_default();
    let explicit_genre_ids = body.genre_ids.unwrap_or_default();
    let genre_filter_requested = !genre_tokens.is_empty() || !explicit_genre_ids.is_empty();

    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let (mut genre_ids, unmatched_genres) =
                queries::resolve_genre_tokens(conn, &genre_tokens)?;
            genre_ids.extend(&explicit_genre_ids);
            let genre_ids = queries::expand_genre_descendants(conn, &genre_ids)?;

            // A requested genre filter that resolves to nothing must yield
            // zero results, not silently fall back to an unfiltered search.
            if genre_filter_requested && genre_ids.is_empty() {
                return Ok(Json(json!({
                    "tracks": [],
                    "total": 0,
                    "unmatched_genres": unmatched_genres,
                })));
            }

            let filters = queries::AudioFilters {
                bpm_min: body.bpm_min,
                bpm_max: body.bpm_max,
                energy_min: body.energy_min,
                energy_max: body.energy_max,
                danceability_min: body.danceability_min,
                danceability_max: body.danceability_max,
                key_signature: body.key_signature.clone(),
                camelot_key: body.camelot_key.clone(),
                year_min: body.year_min,
                year_max: body.year_max,
                genre_ids,
                track_type: body.track_type.clone(),
                is_instrumental: body.is_instrumental,
                liked_only: body.liked_only.unwrap_or(false),
                artist_contains: body.artist_contains.clone(),
                album_contains: body.album_contains.clone(),
            };

            let total = queries::count_audio_filter_matches(conn, &free_text, &filters)?;
            let tracks = if shuffle {
                queries::search_with_audio_filters_shuffled(conn, &free_text, &filters, limit)?
            } else {
                queries::search_with_audio_filters(conn, &free_text, &filters, limit, offset)?
            };
            Ok(Json(json!({
                "tracks": tracks,
                "total": total,
                "unmatched_genres": unmatched_genres,
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
pub(super) struct VibeParams {
    track_id: i64,
    limit: Option<usize>,
}

pub(super) async fn search_vibe(
    State(state): State<SharedState>,
    Query(params): Query<VibeParams>,
) -> Result<Json<Value>, StatusCode> {
    let track_id = positive_query_id(params.track_id)?;
    let limit = clamp_usize_limit(params.limit, VIBE_LIMIT_DEFAULT, VIBE_LIMIT_MAX);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let results = queries::get_same_vibe_tracks(conn, track_id, limit as i64)?;
            Ok(Json(json!({ "tracks": results })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
pub(super) struct UnderratedParams {
    artist_id: i64,
    limit: Option<usize>,
}

pub(super) async fn search_underrated(
    State(state): State<SharedState>,
    Query(params): Query<UnderratedParams>,
) -> Result<Json<Value>, StatusCode> {
    let artist_id = positive_query_id(params.artist_id)?;
    let limit = clamp_usize_limit(params.limit, UNDERRATED_LIMIT_DEFAULT, UNDERRATED_LIMIT_MAX);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let results = queries::get_underrated_tracks(conn, artist_id, limit as i64)?;
            Ok(Json(json!({ "tracks": results })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests_legacy {
    // Legacy mod name retained only to avoid duplicate symbol clashes if this file
    // is merged with older local work. This module intentionally has no tests.
}
