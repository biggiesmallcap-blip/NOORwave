use crate::SharedState;
use crate::db::catalog_name::{names_overlap, normalize_catalog_name};
use crate::db::queries;
use crate::metadata::lastfm::{
    LastFmChartAlbum, LastFmChartArtist, LastFmChartTrack, LastFmClient,
};
use axum::{extract::State, http::StatusCode, response::Json};
use futures::StreamExt;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashSet;
/// Get new album releases from AllMusic RSS
pub(super) async fn get_home_releases(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;

    // Pull api_key from the existing Last.fm credentials row. If Last.fm
    // isn't configured, we 503 so the frontend renders the connect/empty
    // state instead of falling back to the old AllMusic RSS feed.
    let (http, api_key) = {
        let s = state.read().await;
        let api_key =
            s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
                .ok()
                .flatten()
                .map(|c| c.api_key);
        (s.http_client.clone(), api_key)
    };
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    match lastfm::releases::fetch_new_releases_cached(&http, &api_key).await {
        Ok(releases) => Ok(Json(json!({
            "releases": releases,
            "source": "lastfm_api",
        }))),
        Err(e) => {
            tracing::warn!("Last.fm new-releases pipeline failed: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

/// Get daily picks curated from user's library using learning model
pub(super) async fn get_home_picks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    const PICKS_TTL: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);
    let state_guard = state.read().await;

    // Serve from the in-process TTL cache when fresh. Avoids the ORDER BY RANDOM()
    // full scan (and the top-played query) on every home remount / tab switch.
    {
        let cache = state_guard.home_picks_cache.lock().unwrap();
        if let Some((computed_at, payload)) = cache.as_ref() {
            if computed_at.elapsed() < PICKS_TTL {
                return Ok(Json(payload.clone()));
            }
        }
    }

    let db = &state_guard.db;

    // Get top tracks from listening history with variety
    let picks = db
        .with_conn(|conn| {
            // Fetch recent top tracks that aren't played in last 7 days (rediscovery)
            let tracks = queries::get_tracks(conn, "play_count", "desc", 20, 0, false, false)?;

            // Get tracks from different genres for variety
            let mut genre_tracks = conn.prepare(
                "SELECT t.*, g.name as genre_name
             FROM tracks t
             JOIN track_genres tg ON t.id = tg.track_id
             JOIN genres g ON tg.genre_id = g.id
             WHERE t.play_count > 0
             ORDER BY RANDOM()
             LIMIT 10",
            )?;

            let genre_picks: Vec<serde_json::Value> = genre_tracks
                .query_map([], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, i64>(0)?,
                        "title": row.get::<_, String>(1)?,
                        "artist_name": row.get::<_, Option<String>>(2)?,
                        "album_title": row.get::<_, Option<String>>(3)?,
                        "artwork_url": row.get::<_, Option<String>>(4)?,
                        "duration_ms": row.get::<_, Option<i64>>(5)?,
                        "play_count": row.get::<_, i64>(6)?,
                        "genre": row.get::<_, String>(7)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();

            Ok((tracks, genre_picks))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (top_tracks, genre_picks) = picks;

    let payload = json!({
        "top_picks": top_tracks.iter().take(10).map(|t| serde_json::json!({
            "id": t.id,
            "title": t.title,
            "artist_name": t.artist_name,
            "album_title": t.album_title,
            "artwork_url": t.artwork_url,
            "duration_ms": t.duration_ms,
            "play_count": t.play_count,
            "reason": "Most played"
        })).collect::<Vec<_>>(),
        "genre_variety": genre_picks,
        "source": "library_curation"
    });

    {
        let mut cache = state_guard.home_picks_cache.lock().unwrap();
        *cache = Some((std::time::Instant::now(), payload.clone()));
    }

    Ok(Json(payload))
}

/// How long one shuffle-pick sample stays put. Matches the client's panel
/// refresh bucket, so a remount inside the window repaints the same murals
/// instead of reshuffling under the user.
const HOME_SHUFFLE_BUCKET_SECS: i64 = 5 * 60;
const HOME_SHUFFLE_DEFAULT_LIMIT: i64 = 12;

#[derive(Debug, serde::Deserialize)]
pub(super) struct HomeShuffleQuery {
    limit: Option<i64>,
}

/// GET /api/home/shuffle-picks - the "Random tracks" / "Random albums" murals.
///
/// One request, two queries, no dependency on how far the client's library
/// store has paged in. The client used to derive random offsets from the track
/// and album totals and then issue one single-row paginated request per pick
/// (24 round trips), which could not start until the library store had loaded
/// its first page - so the murals always popped in late. The sample is keyed to
/// a five-minute bucket so it is stable across remounts.
pub(super) async fn get_home_shuffle_picks(
    State(state): State<SharedState>,
    axum::extract::Query(query): axum::extract::Query<HomeShuffleQuery>,
) -> Result<Json<Value>, StatusCode> {
    let limit = query
        .limit
        .unwrap_or(HOME_SHUFFLE_DEFAULT_LIMIT)
        .clamp(1, 50);
    let salt = unix_now_secs() / HOME_SHUFFLE_BUCKET_SECS;

    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let (tracks, albums) = db
        .with_conn(move |conn| {
            let tracks = queries::get_shuffled_tracks(conn, salt, limit, true)?;
            // Offset the album salt so the two murals don't rotate in lockstep
            // on the same underlying id ordering.
            let albums = queries::get_shuffled_albums(conn, salt + 7, limit, true)?;
            Ok((tracks, albums))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "tracks": tracks, "albums": albums })))
}

pub(super) async fn get_home_recommendations(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let lastfm = load_or_fetch_recommendation_shelf(state.clone(), "lastfm").await;
    let listenbrainz = load_or_fetch_recommendation_shelf(state.clone(), "listenbrainz").await;
    Ok(Json(json!({
        "shelves": [
            recommendation_shelf_json("lastfm", "Last.fm recommended tracks", Some("track"), &lastfm),
            recommendation_shelf_json("lastfm", "Last.fm recommended artists", Some("artist"), &lastfm),
            recommendation_shelf_json("lastfm", "Last.fm recommended albums", Some("album"), &lastfm),
            recommendation_shelf_json("listenbrainz", "ListenBrainz recommends", Some("track"), &listenbrainz),
        ]
    })))
}

// v7: folded-name resolution, the Last.fm placeholder filter and the artist
// photo backfill all change what a resolved item looks like. Without a bump,
// an existing install would keep serving v6 payloads - built by the old
// exact-match resolver, complete with grey stars and unplayable rows - for the
// full six hours before anything improved.
// v8: the artist and album caps went from 20 to 50. A cached v7 payload only
// holds twenty, so without a bump the bigger rails would not appear until the
// six-hour lease expired.
// v9: album items now carry a resolved tidal_album_id and the ones with no
// album behind them are dropped. A v8 payload still holds those dead tiles.
// v10: two album items that resolved to the same record are collapsed. A v9
// payload can hold that duplicate pair, and on an installed build it would keep
// blanking Home for the rest of the six-hour lease.
const RECOMMENDATION_HOME_CACHE_KEY: &str = "home:v10";

/// Cap for the track shelf, which is still rendered as the mural. Twenty is not
/// arbitrary here: `layout-count-20` in ChartMural.svelte is a 10x2 grid, so a
/// twenty-first item would add a row and reshape the mosaic.
const LASTFM_HOME_RECOMMENDATION_LIMIT: usize = 20;
const LASTFM_HOME_SEED_LIMIT: usize = 12;
const LASTFM_HOME_PROFILE_SOURCE_LIMIT: usize = 30;
const LASTFM_HOME_RECENT_SEED_TARGET: usize = 8;
const LASTFM_HOME_LOVED_SEED_TARGET: usize = 8;
const LASTFM_HOME_TOP_SEED_TARGET: usize = 6;
const LASTFM_HOME_SIMILAR_LIMIT: usize = 20;

/// Output caps for the two rail shelves.
///
/// These were 20 because all three shelves used to render as a ChartMural, and
/// that mosaic is a fixed 10x2 grid - twenty cells, no more. Artists and albums
/// are rails now, and a rail just scrolls, so the old ceiling was measuring the
/// wrong thing.
///
/// Raising them costs no extra upstream calls. The fan-out already generates
/// far more than it keeps: artists is 12 seeds x `LASTFM_HOME_SIMILAR_LIMIT`
/// (up to 240), albums is 12 seeds x `LASTFM_HOME_ALBUM_SIMILAR_ARTIST_LIMIT` x
/// `LASTFM_HOME_ALBUMS_PER_ARTIST_LIMIT` (up to 480). Everything past the cap
/// was fetched, deduped, and then thrown away. The added cost is server-side
/// artwork resolution for the extra items, which is buffered and shares the
/// 6h TIDAL search cache.
///
/// The track shelf stays at `LASTFM_HOME_RECOMMENDATION_LIMIT` because it is
/// still the mural.
const LASTFM_HOME_ARTIST_LIMIT: usize = 50;
const LASTFM_HOME_ALBUM_LIMIT: usize = 50;
const LASTFM_HOME_ALBUM_SIMILAR_ARTIST_LIMIT: usize = 8;

/// Concurrent Last.fm calls during the home fan-out. Their documented ceiling
/// is around five requests a second per key, and these bursts are short, so
/// this stays deliberately modest: the win is removing the serial stall, not
/// saturating the API.
const LASTFM_FANOUT_CONCURRENCY: usize = 6;
const LASTFM_HOME_ALBUMS_PER_ARTIST_LIMIT: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastFmTrackSeed {
    pub(crate) artist: String,
    pub(crate) title: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastFmArtistSeed {
    pub(crate) name: String,
    pub(crate) reason: String,
}

fn recommendation_shelf_json(
    provider: &str,
    title: &str,
    entity_type: Option<&str>,
    result: &anyhow::Result<Vec<Value>>,
) -> Value {
    match result {
        Ok(items) => {
            let filtered = filter_recommendation_items(items, entity_type);
            json!({
                "provider": provider,
                "title": title,
                "entity_type": entity_type.unwrap_or("track"),
                "status": if filtered.is_empty() { "empty" } else { "ok" },
                "items": filtered,
            })
        }
        Err(error) => json!({
            "provider": provider,
            "title": title,
            "entity_type": entity_type.unwrap_or("track"),
            "status": "error",
            "message": error.to_string(),
            "items": [],
        }),
    }
}

fn filter_recommendation_items(items: &[Value], entity_type: Option<&str>) -> Vec<Value> {
    let wanted = entity_type.unwrap_or("track");
    items
        .iter()
        .filter(|item| {
            item.get("entity_type")
                .and_then(Value::as_str)
                .unwrap_or("track")
                == wanted
        })
        .cloned()
        .collect()
}

async fn load_or_fetch_recommendation_shelf(
    state: SharedState,
    provider: &str,
) -> anyhow::Result<Vec<Value>> {
    if let Some(cached) = read_recommendation_cache(&state, provider).await {
        return Ok(cached);
    }
    let mut items = match provider {
        "lastfm" => fetch_lastfm_home_recommendations(&state).await?,
        "listenbrainz" => fetch_listenbrainz_home_recommendations(&state).await?,
        _ => Vec::new(),
    };
    // Before the cache write, so the artwork and the album ids are stored with
    // the shelf rather than re-resolved on every cache hit.
    resolve_missing_artwork(&state, &mut items).await;
    drop_unresolvable_albums(&mut items);
    drop_duplicate_albums(&mut items);
    write_recommendation_cache(&state, provider, &items).await;
    Ok(items)
}

/// Concurrent TIDAL searches while filling in missing artwork. Most of these
/// are cache hits, so this only really bounds the cold path.
const ARTWORK_RESOLVE_CONCURRENCY: usize = 6;

/// Search query for an item that has no artwork yet: artists are looked up by
/// name, everything else by artist plus title. Matches how the client composes
/// the same query, so the two share cache rows.
fn artwork_query(item: &Value) -> Option<String> {
    let entity = item
        .get("entity_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if entity == "artist" {
        return (!title.is_empty()).then(|| title.to_string());
    }
    let artist = item
        .get("artist_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if artist.is_empty() {
        return None;
    }
    Some(if title.is_empty() {
        artist.to_string()
    } else {
        format!("{artist} {title}")
    })
}

/// The TIDAL album a recommended album item refers to, or None.
///
/// A wrong album is worse than no album, so this refuses to guess: the artist
/// has to fold equal, and the title has to fold equal or overlap at a word
/// boundary (which is what lets "Hurt So Good (Bonus Track Edition)" find "Hurt
/// So Good"). Same rules as `findAlbumMatch` in
/// `frontend/src/lib/components/home/recommendation_navigation.ts`, and the same
/// reason: Last.fm recommends singles, regional pressings and anthologies that
/// TIDAL does not carry as albums, and a sole album by the right artist is not
/// evidence that it is the album asked for.
fn album_id_from_catalog(
    item: &Value,
    catalog: &crate::services::tidal::client::TidalSearchCatalog,
) -> Option<i64> {
    let wanted_title = normalize_catalog_name(item.get("title").and_then(Value::as_str)?);
    let wanted_artist = normalize_catalog_name(
        item.get("artist_name")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    if wanted_title.is_empty() || wanted_artist.is_empty() {
        return None;
    }

    let same_artist = catalog.albums.iter().filter(|album| {
        normalize_catalog_name(album.artist_name.as_deref().unwrap_or("")) == wanted_artist
    });
    let mut same_artist: Vec<_> = same_artist.collect();
    if same_artist.is_empty() {
        return None;
    }
    if let Some(exact) = same_artist
        .iter()
        .find(|album| normalize_catalog_name(&album.title) == wanted_title)
    {
        return Some(exact.id);
    }
    // Prefer the shortest overlapping title, so an anthology whose name happens
    // to start with the album name does not win over the album itself.
    same_artist.sort_by_key(|album| album.title.len());
    same_artist
        .iter()
        .find(|album| names_overlap(&normalize_catalog_name(&album.title), &wanted_title))
        .map(|album| album.id)
}

/// The TIDAL track behind an "album" that is really a single.
///
/// Last.fm's top-albums feed does not distinguish an album from a single, so a
/// famous 7" like Alton Ellis's "Cry Tough" arrives as an album that TIDAL has
/// no album for - but does have the track for. Rather than dropping it, the item
/// keeps its place and the card seeds song radio from that track, which is the
/// closest thing to "listen to this" that a single supports.
///
/// Same refusal to guess as `album_id_from_catalog`: artist has to fold equal,
/// title has to fold equal or overlap at a word boundary.
fn single_id_from_catalog(
    item: &Value,
    catalog: &crate::services::tidal::client::TidalSearchCatalog,
) -> Option<i64> {
    let wanted_title = normalize_catalog_name(item.get("title").and_then(Value::as_str)?);
    let wanted_artist = normalize_catalog_name(
        item.get("artist_name")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    if wanted_title.is_empty() || wanted_artist.is_empty() {
        return None;
    }

    let mut same_artist: Vec<_> = catalog
        .tracks
        .iter()
        .filter(|track| {
            normalize_catalog_name(track.artist_name.as_deref().unwrap_or("")) == wanted_artist
        })
        .collect();
    if same_artist.is_empty() {
        return None;
    }
    if let Some(exact) = same_artist
        .iter()
        .find(|track| normalize_catalog_name(&track.title) == wanted_title)
    {
        return Some(exact.id);
    }
    same_artist.sort_by_key(|track| track.title.len());
    same_artist
        .iter()
        .find(|track| names_overlap(&normalize_catalog_name(&track.title), &wanted_title))
        .map(|track| track.id)
}

/// Pick the artwork a search result offers for this kind of item. An artist
/// wants the artist photo, which the catalogue carries directly; anything else
/// wants a cover.
fn artwork_from_catalog(
    entity: &str,
    catalog: &crate::services::tidal::client::TidalSearchCatalog,
) -> Option<String> {
    if entity == "artist" {
        if let Some(url) = catalog
            .artists
            .first()
            .and_then(|a| a.artwork_url.clone().or_else(|| a.picture.clone()))
        {
            return Some(url);
        }
    }
    catalog
        .tracks
        .first()
        .and_then(|t| t.artwork_url.clone())
        .or_else(|| catalog.albums.first().and_then(|a| a.artwork_url.clone()))
}

/// Fill in artwork, and the TIDAL album id, for items the local library could
/// not supply.
///
/// Without this the client did it: one `/api/tidal/search` per artwork-less
/// tile, sixty of them on a full Home, funnelled through a four-wide in-flight
/// cap - fifteen serial waves of round trips, which is precisely the staggered
/// fill-in users see. Doing it here means the payload arrives complete, and
/// since both this endpoint and the TIDAL search cache hold results for six
/// hours, a warm cache makes it nearly free. The client keeps its lazy lookup
/// as a fallback for whatever is still missing.
///
/// Album items are searched even when Last.fm gave them a cover, because the
/// id is what decides whether the card can do anything at all. Measured over a
/// full album shelf, 20 of 50 items had no TIDAL album behind them: Last.fm
/// recommends singles and regional anthologies freely. Resolving here is what
/// lets `drop_unresolvable_albums` take those out before they reach the rail,
/// and it means a card that is shown opens instantly, with no click-time search.
///
/// Best-effort throughout: TIDAL not connected, a failed search or a query we
/// cannot build all leave the item exactly as it was.
async fn resolve_missing_artwork(state: &SharedState, items: &mut [Value]) {
    let (tokens, tidal_http, db) = {
        let s = state.read().await;
        (
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
            s.db.clone(),
        )
    };
    let Some(tokens) = tokens else {
        return;
    };

    // `album` carries a copy of the item only when its TIDAL album id still has
    // to be matched out of the search response, so the buffered futures do not
    // hold a borrow on `items`.
    let pending: Vec<(usize, String, String, Option<Value>)> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.get("artwork_url").is_none_or(Value::is_null) || needs_album_id(item)
        })
        .filter_map(|(index, item)| {
            let entity = item
                .get("entity_type")
                .and_then(Value::as_str)
                .unwrap_or("track")
                .to_string();
            let album = needs_album_id(item).then(|| item.clone());
            artwork_query(item).map(|query| (index, entity, query, album))
        })
        .collect();

    if pending.is_empty() {
        return;
    }

    let client = crate::services::tidal::client::TidalClient::with_http(
        tidal_http,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let cache_cfg = crate::services::tidal::cache::TidalSearchCacheConfig::default();

    let resolved: Vec<(usize, Option<String>, Option<AlbumLink>)> = futures::stream::iter(pending)
        .map(|(index, entity, query, album)| {
            let client = client.clone();
            let db = db.clone();
            async move {
                // Shared with /api/tidal/search, so anything the client has
                // already looked up costs nothing here.
                let cached = db
                    .with_conn(|conn| {
                        crate::services::tidal::cache::get_search(
                            conn,
                            &cache_cfg,
                            &query,
                            ARTWORK_SEARCH_LIMIT,
                            0,
                        )
                    })
                    .ok()
                    .flatten();

                let catalog = match cached {
                    Some(hit) => Some(hit),
                    None => match client.search_catalog(&query, ARTWORK_SEARCH_LIMIT, 0).await {
                        Ok(fetched) => {
                            let to_cache = fetched.clone();
                            let q = query.clone();
                            let _ = db.with_conn(move |conn| {
                                crate::services::tidal::cache::put_search(
                                    conn,
                                    &q,
                                    ARTWORK_SEARCH_LIMIT,
                                    0,
                                    &to_cache,
                                )
                            });
                            Some(fetched)
                        }
                        Err(e) => {
                            tracing::debug!(target: "noor.home_artwork", query, error = %e, "artwork search failed");
                            None
                        }
                    },
                };

                let Some(catalog) = catalog else {
                    return (index, None, None);
                };
                let link = album.and_then(|item| {
                    // Album first, single only as the fallback: an album that
                    // exists is always the better answer for an album card.
                    album_id_from_catalog(&item, &catalog)
                        .map(AlbumLink::Album)
                        .or_else(|| single_id_from_catalog(&item, &catalog).map(AlbumLink::Single))
                });
                (index, artwork_from_catalog(&entity, &catalog), link)
            }
        })
        .buffer_unordered(ARTWORK_RESOLVE_CONCURRENCY)
        .collect()
        .await;

    let mut filled = 0usize;
    let mut albums = 0usize;
    let mut singles = 0usize;
    for (index, url, link) in resolved {
        let Some(slot) = items.get_mut(index) else {
            continue;
        };
        let Some(obj) = slot.as_object_mut() else {
            continue;
        };
        if let Some(url) = url
            && obj.get("artwork_url").is_none_or(Value::is_null)
        {
            obj.insert("artwork_url".to_string(), Value::String(url));
            filled += 1;
        }
        match link {
            Some(AlbumLink::Album(id)) => {
                obj.insert("tidal_album_id".to_string(), Value::from(id));
                // Playable now means something for this item: there is an album
                // behind it whose tracklist can be opened and queued.
                obj.insert("playable".to_string(), Value::Bool(true));
                albums += 1;
            }
            Some(AlbumLink::Single(id)) => {
                obj.insert("tidal_id".to_string(), Value::from(id));
                obj.insert("is_single".to_string(), Value::Bool(true));
                obj.insert("playable".to_string(), Value::Bool(true));
                singles += 1;
            }
            None => {}
        }
    }
    if filled > 0 || albums > 0 || singles > 0 {
        tracing::debug!(target: "noor.home_artwork", filled, albums, singles, "resolved recommendation artwork and album links");
    }
}

/// What a recommended "album" turned out to be on TIDAL.
enum AlbumLink {
    Album(i64),
    /// Last.fm recommended a single. There is no album to open, but the track
    /// exists, so the card can seed song radio from it.
    Single(i64),
}

/// True when this is an album item with no album behind it yet.
///
/// Both ids are checked: a library album needs nothing, and an item that already
/// carries a TIDAL id (because the library row had one) is done too.
fn needs_album_id(item: &Value) -> bool {
    item.get("entity_type").and_then(Value::as_str) == Some("album")
        && item.get("local_album_id").is_none_or(Value::is_null)
        && item.get("tidal_album_id").is_none_or(Value::is_null)
}

/// True when an album card would do nothing at all if clicked: no album to open
/// and no single to seed radio from.
fn album_item_is_dead(item: &Value) -> bool {
    needs_album_id(item) && item.get("tidal_id").is_none_or(Value::is_null)
}

/// Drop album items that resolved to nothing at all.
///
/// Last.fm's top-albums-per-artist feed is full of singles, regional pressings
/// and anthologies that TIDAL does not carry, and a card for one of those cannot
/// open, play or queue: it is a dead tile that looks exactly like a live one.
/// Measured over a full shelf: 30 of 50 resolved to an album, 5 more to the
/// single behind the title, and 15 - all of them compilations and anthologies -
/// to nothing. The shelf caps are 50 against a fan-out that generates several
/// hundred candidates, so what is left is still a full-looking rail.
///
/// Only albums. A track item has its own resolution path and an artist item is
/// browsable by name, so neither is dead in the same way.
fn drop_unresolvable_albums(items: &mut Vec<Value>) {
    let before = items.len();
    items.retain(|item| !album_item_is_dead(item));
    let dropped = before - items.len();
    if dropped > 0 {
        tracing::debug!(target: "noor.home_artwork", dropped, "dropped album recommendations with no TIDAL album");
    }
}

/// Collapse album items that resolved to the same record.
///
/// The upstream dedupe runs on artist plus title, before resolution, so two
/// spellings of one title survive it: Last.fm returned "Dunyala" twice for one
/// artist with the diaeresis decomposed in one copy and precomposed in the
/// other, which are different strings and the same album. Resolution then gave
/// both the same `tidal_album_id`, and the shelf shipped one record as two
/// cards.
///
/// That is worth fixing on its own - a rail that shows the same album twice is
/// wrong - but it also took Home down. The client keys its cards on the resolved
/// id, and a duplicate key makes Svelte throw mid-render, which leaves the shelf
/// stuck on its loading state and blanks the page on any client-side navigation
/// back to it. The client no longer keys on the id alone; this stops the
/// duplicate reaching it in the first place.
///
/// Runs after resolution and after the dead-album drop, so the ids being
/// compared are the final ones. Items with nothing to compare (no resolved id)
/// are left alone rather than folded together.
fn drop_duplicate_albums(items: &mut Vec<Value>) {
    let before = items.len();
    let mut seen: HashSet<String> = HashSet::new();
    items.retain(|item| {
        let Some(key) = resolved_album_key(item) else {
            return true;
        };
        seen.insert(key)
    });
    let dropped = before - items.len();
    if dropped > 0 {
        tracing::debug!(target: "noor.home_artwork", dropped, "dropped album recommendations that resolved to an album already on the shelf");
    }
}

/// Identity of the record an album card actually points at, or `None` when it
/// points at nothing comparable yet.
///
/// A library album and a TIDAL album are separate namespaces, and a card that
/// resolved to a single is identified by that track, not by an album.
fn resolved_album_key(item: &Value) -> Option<String> {
    if item.get("entity_type").and_then(Value::as_str) != Some("album") {
        return None;
    }
    if let Some(id) = item.get("local_album_id").and_then(Value::as_i64) {
        return Some(format!("local:{id}"));
    }
    if let Some(id) = item.get("tidal_album_id").and_then(Value::as_i64) {
        return Some(format!("tidal:{id}"));
    }
    item.get("tidal_id")
        .and_then(Value::as_i64)
        .map(|id| format!("single:{id}"))
}

/// Matches the canonical search bucket, so these lookups share cache rows with
/// every other search of the same name.
const ARTWORK_SEARCH_LIMIT: i32 = 12;

/// A shelf this short means the fan-out was cut off, not that the user has a
/// small taste profile: a healthy fetch fills every shelf to its cap.
///
/// Deliberately an absolute count rather than a fraction of the cap, so raising
/// the artist and album caps cannot turn a perfectly good shelf into one that
/// keeps re-fetching on a ten-minute lease.
const RECOMMENDATION_HEALTHY_FLOOR: usize = 12;

/// How long a short result is allowed to stick around. Long enough to absorb a
/// burst of remounts, short enough that the next visit retries rather than
/// living with it for six hours.
const RECOMMENDATION_SHORT_TTL_SECS: i64 = 10 * 60;

/// Full cache lifetime for a shelf that came back healthy.
const RECOMMENDATION_FULL_TTL_SECS: i64 = 6 * 60 * 60;

/// Pick the TTL for a freshly-fetched payload.
///
/// Every upstream Last.fm call in the fan-out is `.unwrap_or_default()`, so a
/// rate-limited or slow window does not fail - it silently returns fewer items.
/// Writing that at the full six-hour TTL pinned a half-empty Home until the
/// cache expired, which is the "only a few tracks" symptom. Short results now
/// get a short lease so the next visit tries again.
fn recommendation_cache_ttl(items: &[Value]) -> i64 {
    if items.len() >= RECOMMENDATION_HEALTHY_FLOOR {
        RECOMMENDATION_FULL_TTL_SECS
    } else {
        RECOMMENDATION_SHORT_TTL_SECS
    }
}

async fn read_recommendation_cache(state: &SharedState, provider: &str) -> Option<Vec<Value>> {
    let now = unix_now_secs();
    let s = state.read().await;
    s.db.with_conn(|conn| {
        conn.query_row(
            "SELECT payload_json FROM provider_recommendation_cache
                  WHERE provider = ?1 AND cache_key = ?2 AND expires_at > ?3",
            params![provider, RECOMMENDATION_HOME_CACHE_KEY, now],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
    })
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
}

async fn write_recommendation_cache(state: &SharedState, provider: &str, items: &[Value]) {
    let now = unix_now_secs();
    let expires = now + recommendation_cache_ttl(items);
    let Ok(payload) = serde_json::to_string(items) else {
        return;
    };
    let s = state.read().await;
    let _ = s.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO provider_recommendation_cache (provider, cache_key, payload_json, fetched_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(provider, cache_key) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 fetched_at = excluded.fetched_at,
                 expires_at = excluded.expires_at",
            params![provider, RECOMMENDATION_HOME_CACHE_KEY, payload, now, expires],
        )?;
        Ok::<_, anyhow::Error>(())
    });
}

pub(crate) fn recommendation_seed_window() -> usize {
    (unix_now_secs() / (6 * 60 * 60)) as usize
}

pub(crate) fn rotate_take<T: Clone>(items: &[T], limit: usize, salt: usize) -> Vec<T> {
    if items.is_empty() || limit == 0 {
        return Vec::new();
    }
    let offset = salt % items.len();
    items
        .iter()
        .cycle()
        .skip(offset)
        .take(limit.min(items.len()))
        .cloned()
        .collect()
}

pub(crate) fn merge_lastfm_track_seeds(
    recent: Vec<LastFmChartTrack>,
    loved: Vec<LastFmChartTrack>,
    top: Vec<LastFmChartTrack>,
    salt: usize,
    limit: usize,
) -> Vec<LastFmTrackSeed> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push_track = |track: LastFmChartTrack, reason: String| {
        if out.len() >= limit {
            return;
        }
        let key = crate::services::radio::normalize_for_dedup(&track.artist, &track.title);
        if key.is_empty() || !seen.insert(key) {
            return;
        }
        out.push(LastFmTrackSeed {
            artist: track.artist,
            title: track.title,
            reason,
        });
    };

    for track in rotate_take(&recent, LASTFM_HOME_RECENT_SEED_TARGET, salt) {
        let reason = format!("Because you played {} recently", track.title);
        push_track(track, reason);
    }
    for track in rotate_take(&loved, LASTFM_HOME_LOVED_SEED_TARGET, salt + 3) {
        let reason = format!("Because you loved {}", track.title);
        push_track(track, reason);
    }
    for track in rotate_take(&top, LASTFM_HOME_TOP_SEED_TARGET, salt + 7) {
        let reason = format!("Near your top track {}", track.title);
        push_track(track, reason);
    }

    out
}

pub(crate) fn merge_lastfm_artist_seeds(
    track_seeds: &[LastFmTrackSeed],
    top_artists: Vec<LastFmChartArtist>,
    top_albums: Vec<LastFmChartAlbum>,
    salt: usize,
    limit: usize,
) -> Vec<LastFmArtistSeed> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push_artist = |name: String, reason: String| {
        if out.len() >= limit {
            return;
        }
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        let key = trimmed.to_ascii_lowercase();
        if !seen.insert(key) {
            return;
        }
        out.push(LastFmArtistSeed {
            name: trimmed.to_string(),
            reason,
        });
    };

    for seed in rotate_take(track_seeds, LASTFM_HOME_RECENT_SEED_TARGET, salt) {
        push_artist(seed.artist.clone(), seed.reason.clone());
    }
    for artist in rotate_take(&top_artists, LASTFM_HOME_TOP_SEED_TARGET, salt + 5) {
        let reason = format!("Near your top artist {}", artist.name);
        push_artist(artist.name, reason);
    }
    for album in rotate_take(&top_albums, LASTFM_HOME_TOP_SEED_TARGET, salt + 11) {
        let reason = format!("Because you play albums by {}", album.artist);
        push_artist(album.artist, reason);
    }

    out
}

async fn load_lastfm_track_seeds(client: &LastFmClient, user: &str) -> Vec<LastFmTrackSeed> {
    let recent = client
        .user_recent_tracks(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    let loved = client
        .user_loved_tracks(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    let top = client
        .user_top_tracks(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    merge_lastfm_track_seeds(
        recent,
        loved,
        top,
        recommendation_seed_window(),
        LASTFM_HOME_SEED_LIMIT,
    )
}

async fn load_lastfm_artist_seeds(
    client: &LastFmClient,
    user: &str,
    track_seeds: &[LastFmTrackSeed],
) -> Vec<LastFmArtistSeed> {
    let top_artists = client
        .user_top_artists(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    let top_albums = client
        .user_top_albums(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    merge_lastfm_artist_seeds(
        track_seeds,
        top_artists,
        top_albums,
        recommendation_seed_window(),
        LASTFM_HOME_SEED_LIMIT,
    )
}

async fn fetch_lastfm_home_recommendations(state: &SharedState) -> anyhow::Result<Vec<Value>> {
    let (http, db, user) = {
        let s = state.read().await;
        let user = s.db.with_conn(|conn| {
            Ok::<_, anyhow::Error>(
                crate::services::lastfm::auth::load_credentials(conn)?.and_then(|c| c.session_user),
            )
        })?;
        (s.http_client.clone(), s.db.clone(), user)
    };
    let Some(user) = user else {
        return Ok(Vec::new());
    };
    let Some(client) = LastFmClient::load(http, &db) else {
        return Ok(Vec::new());
    };
    let track_seeds = load_lastfm_track_seeds(&client, &user).await;
    let artist_seeds = load_lastfm_artist_seeds(&client, &user, &track_seeds).await;
    let mut out = Vec::new();
    out.extend(fetch_lastfm_track_recommendations(state, &client, &track_seeds).await?);
    out.extend(fetch_lastfm_artist_recommendations(state, &client, &artist_seeds).await?);
    out.extend(fetch_lastfm_album_recommendations(state, &client, &artist_seeds).await?);
    Ok(out)
}

async fn fetch_lastfm_track_recommendations(
    state: &SharedState,
    client: &LastFmClient,
    seeds: &[LastFmTrackSeed],
) -> anyhow::Result<Vec<Value>> {
    // Fetch every seed's similar list up front, then walk the results in seed
    // order. `buffered`, not `buffer_unordered`: dedup is first-come and the
    // shelf stops at a limit, so out-of-order completion would make the
    // contents depend on network timing.
    let seed_keys: Vec<(String, String)> = seeds
        .iter()
        .map(|seed| (seed.artist.clone(), seed.title.clone()))
        .collect();
    let similar_by_seed: Vec<Vec<_>> = futures::stream::iter(seed_keys)
        .map(|(artist, title)| async move {
            client
                .track_get_similar_with_artist_fallback(&artist, &title, LASTFM_HOME_SIMILAR_LIMIT)
                .await
                .unwrap_or_default()
        })
        .buffered(LASTFM_FANOUT_CONCURRENCY)
        .collect()
        .await;

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (seed, similars) in seeds.iter().zip(similar_by_seed) {
        for similar in similars {
            let key = crate::services::radio::normalize_for_dedup(&similar.artist, &similar.title);
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            if let Some(item) = resolve_recommendation_item(
                state,
                "lastfm",
                &similar.artist,
                &similar.title,
                None,
                Some(similar.match_score),
                &seed.reason,
            )
            .await
            {
                out.push(item);
            } else {
                out.push(recommendation_placeholder_item(
                    "lastfm",
                    &similar.artist,
                    &similar.title,
                    similar.mbid.as_deref(),
                    Some(similar.match_score),
                    &seed.reason,
                ));
            }
            if out.len() >= LASTFM_HOME_RECOMMENDATION_LIMIT {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

async fn fetch_lastfm_artist_recommendations(
    state: &SharedState,
    client: &LastFmClient,
    seeds: &[LastFmArtistSeed],
) -> anyhow::Result<Vec<Value>> {
    let seed_names: Vec<String> = seeds.iter().map(|seed| seed.name.clone()).collect();
    let similar_by_seed: Vec<Vec<_>> = futures::stream::iter(seed_names)
        .map(|name| async move {
            client
                .artist_get_similar(&name, LASTFM_HOME_SIMILAR_LIMIT)
                .await
                .unwrap_or_default()
        })
        .buffered(LASTFM_FANOUT_CONCURRENCY)
        .collect()
        .await;

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (seed, similars) in seeds.iter().zip(similar_by_seed) {
        for artist in similars {
            let key = artist.name.trim().to_ascii_lowercase();
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            out.push(
                resolve_recommendation_artist_item(
                    state,
                    "lastfm",
                    &artist.name,
                    artist.mbid.as_deref(),
                    artist.match_score,
                    &seed.reason,
                    artist.image_url.as_deref(),
                )
                .await,
            );
            if out.len() >= LASTFM_HOME_ARTIST_LIMIT {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

async fn fetch_lastfm_album_recommendations(
    state: &SharedState,
    client: &LastFmClient,
    seeds: &[LastFmArtistSeed],
) -> anyhow::Result<Vec<Value>> {
    // The worst of the three: one similar-artists call per seed, then one
    // top-albums call per similar artist, all in series - up to 12 x (1 + 8)
    // round trips at an 8s timeout each. Both levels now run buffered, so the
    // shelf is bounded by the slowest few calls rather than their sum.
    let seed_names: Vec<String> = seeds.iter().map(|seed| seed.name.clone()).collect();
    let similar_by_seed: Vec<Vec<_>> = futures::stream::iter(seed_names)
        .map(|name| async move {
            client
                .artist_get_similar(&name, LASTFM_HOME_ALBUM_SIMILAR_ARTIST_LIMIT)
                .await
                .unwrap_or_default()
        })
        .buffered(LASTFM_FANOUT_CONCURRENCY)
        .collect()
        .await;

    // Flatten to (seed index, artist) so the album fetches are one wide pass
    // instead of a nested one, then regroup in the original order.
    let pairs: Vec<(usize, LastFmChartArtist)> = similar_by_seed
        .into_iter()
        .enumerate()
        .flat_map(|(seed_index, artists)| {
            artists.into_iter().map(move |artist| (seed_index, artist))
        })
        .collect();

    let pair_names: Vec<String> = pairs
        .iter()
        .map(|(_, artist)| artist.name.clone())
        .collect();
    let albums_by_pair: Vec<Vec<_>> = futures::stream::iter(pair_names)
        .map(|name| async move {
            client
                .artist_top_albums(&name, LASTFM_HOME_ALBUMS_PER_ARTIST_LIMIT)
                .await
                .unwrap_or_default()
        })
        .buffered(LASTFM_FANOUT_CONCURRENCY)
        .collect()
        .await;

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for ((seed_index, artist), albums) in pairs.into_iter().zip(albums_by_pair) {
        let seed = &seeds[seed_index];
        {
            for album in albums {
                let key = crate::services::radio::normalize_for_dedup(&album.artist, &album.title);
                if key.is_empty() || !seen.insert(key) {
                    continue;
                }
                out.push(
                    resolve_recommendation_album_item(
                        state,
                        "lastfm",
                        &album.artist,
                        &album.title,
                        album.mbid.as_deref(),
                        artist
                            .match_score
                            .or_else(|| album.playcount.map(|count| count as f64)),
                        &seed.reason,
                        album.image_url.as_deref(),
                    )
                    .await,
                );
                if out.len() >= LASTFM_HOME_ALBUM_LIMIT {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

async fn fetch_listenbrainz_home_recommendations(
    state: &SharedState,
) -> anyhow::Result<Vec<Value>> {
    let (http, token, user) = {
        let s = state.read().await;
        let (token, user) = s.db.with_conn(|conn| {
            let token = crate::services::listenbrainz::load_token(conn, &s.master_key)?;
            let user =
                crate::services::listenbrainz::load_credentials(conn)?.and_then(|c| c.user_name);
            Ok::<_, anyhow::Error>((token, user))
        })?;
        (s.http_client.clone(), token, user)
    };
    let Some(user) = user else {
        return Ok(Vec::new());
    };
    let recs =
        crate::services::listenbrainz::user_recommendations(&http, &user, token.as_deref()).await?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for rec in recs {
        let key = crate::services::radio::normalize_for_dedup(&rec.artist, &rec.title);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        if let Some(item) = resolve_recommendation_item(
            state,
            "listenbrainz",
            &rec.artist,
            &rec.title,
            rec.mbid.as_deref(),
            rec.score,
            "Collaborative filtering",
        )
        .await
        {
            out.push(item);
        } else {
            out.push(recommendation_placeholder_item(
                "listenbrainz",
                &rec.artist,
                &rec.title,
                rec.mbid.as_deref(),
                rec.score,
                "Collaborative filtering",
            ));
        }
        if out.len() >= 12 {
            break;
        }
    }
    Ok(out)
}

async fn resolve_recommendation_item(
    state: &SharedState,
    provider: &str,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
) -> Option<Value> {
    let normalized_title = normalize_catalog_name(title);
    let normalized_artist = normalize_catalog_name(artist);
    let s = state.read().await;
    s.db
        .with_conn(|conn| {
            if let Some(mbid_value) = mbid.filter(|v| !v.trim().is_empty()) {
                let by_mbid = conn
                    .query_row(
                        "SELECT t.id, t.tidal_id, t.title, a.name, al.title, t.artwork_url
                           FROM external_track_candidates c
                           JOIN tracks t
                             ON t.id = c.resolved_track_id
                             OR (c.tidal_id IS NOT NULL AND t.tidal_id = c.tidal_id)
                           LEFT JOIN artists a ON a.id = t.artist_id
                           LEFT JOIN albums al ON al.id = t.album_id
                          WHERE c.mbid = ?1
                          ORDER BY (c.resolved_track_id IS NULL), t.is_favorite DESC, t.play_count DESC
                          LIMIT 1",
                        params![mbid_value],
                        |row| {
                            Ok(json!({
                                "provider": provider,
                                "entity_type": "track",
                                "local_track_id": row.get::<_, i64>(0)?,
                                "tidal_id": row.get::<_, Option<i64>>(1)?,
                                "title": row.get::<_, String>(2)?,
                                "artist_name": row.get::<_, Option<String>>(3)?,
                                "album_title": row.get::<_, Option<String>>(4)?,
                                "artwork_url": row.get::<_, Option<String>>(5)?,
                                "mbid": mbid,
                                "score": score,
                                "reason": reason,
                                "playable": true,
                            }))
                        },
                    )
                    .optional()?;
                if by_mbid.is_some() {
                    return Ok::<_, anyhow::Error>(by_mbid);
                }
            }

            conn.query_row(
                "SELECT t.id, t.tidal_id, t.title, a.name, al.title, t.artwork_url
                   FROM tracks t
                   LEFT JOIN artists a ON a.id = t.artist_id
                   LEFT JOIN albums al ON al.id = t.album_id
                  WHERE (LOWER(t.title) = LOWER(?1)
                         OR (?3 <> '' AND t.title_normalized IS NOT NULL AND t.title_normalized = ?3))
                    AND (LOWER(COALESCE(a.name, '')) = LOWER(?2)
                         OR (?4 <> '' AND a.name_normalized IS NOT NULL AND a.name_normalized = ?4))
                  ORDER BY LOWER(t.title) = LOWER(?1) DESC,
                           t.is_favorite DESC, t.play_count DESC
                  LIMIT 1",
                params![title, artist, normalized_title, normalized_artist],
                |row| {
                    Ok(json!({
                        "provider": provider,
                        "entity_type": "track",
                        "local_track_id": row.get::<_, i64>(0)?,
                        "tidal_id": row.get::<_, Option<i64>>(1)?,
                        "title": row.get::<_, String>(2)?,
                        "artist_name": row.get::<_, Option<String>>(3)?,
                        "album_title": row.get::<_, Option<String>>(4)?,
                        "artwork_url": row.get::<_, Option<String>>(5)?,
                        "mbid": mbid,
                        "score": score,
                        "reason": reason,
                        "playable": true,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
        })
        .ok()
        .flatten()
}

/// Match a provider's artist spelling against a local row: exact name first,
/// then the folded name.
///
/// Providers disagree on accents, ampersands and punctuation, so the old
/// exact-only match left "Sigur Ros", "Beyonce" and "Tyler, The Creator"
/// unresolved and therefore unplayable. `name_normalized` is NULL until the
/// backfill reaches the row, and the folded comparison simply does not fire
/// while it is, so this degrades to the previous behaviour rather than
/// misbehaving. Exact hits still sort first, so a folded near-miss can never
/// displace a real one.
///
/// Held as a const so the test exercises the query the route actually runs.
///
/// The `?2 <> ''` guard is load-bearing. The fold keeps only ASCII
/// alphanumerics, so a name written entirely in another script - Cyrillic,
/// CJK, or pure symbols - folds to the empty string. Without the guard every
/// such row matches every other one, and a recommendation for a Cyrillic
/// artist resolves to an unrelated CJK artist that happens to fold the same
/// way. The exact `LOWER(name)` branch still matches those names correctly,
/// so refusing the empty fold costs nothing and only removes the collision.
const ARTIST_BY_NAME_SQL: &str = "SELECT id, tidal_id, name, photo_url
                   FROM artists
                  WHERE LOWER(name) = LOWER(?1)
                     OR (?2 <> '' AND name_normalized IS NOT NULL AND name_normalized = ?2)
                  ORDER BY LOWER(name) = LOWER(?1) DESC, tidal_id IS NULL, id ASC
                  LIMIT 1";

async fn resolve_recommendation_artist_item(
    state: &SharedState,
    provider: &str,
    artist: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
    image_url: Option<&str>,
) -> Value {
    let normalized = normalize_catalog_name(artist);
    let s = state.read().await;
    let resolved = s
        .db
        .with_conn(|conn| {
            conn.query_row(
                ARTIST_BY_NAME_SQL,
                params![artist, normalized],
                |row| {
                    Ok(json!({
                        "provider": provider,
                        "entity_type": "artist",
                        "local_artist_id": row.get::<_, i64>(0)?,
                        "tidal_artist_id": row.get::<_, Option<i64>>(1)?,
                        "local_track_id": null,
                        "tidal_id": null,
                        "title": row.get::<_, String>(2)?,
                        "artist_name": row.get::<_, String>(2)?,
                        "album_title": null,
                        "artwork_url": row.get::<_, Option<String>>(3)?.or_else(|| image_url.map(str::to_string)),
                        "mbid": mbid,
                        "score": score,
                        "reason": reason,
                        "playable": true,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
        })
        .ok()
        .flatten();

    // A matched artist with a TIDAL id but no stored photo is the common shape
    // for rows imported through the radio path, and an artist rail full of
    // letter tiles is exactly what that produces. The backfill is idempotent
    // and already exists; it just was never called from here. Fire and forget
    // so this request does not wait on TIDAL - the photo lands for next time.
    if let Some(item) = resolved.as_ref()
        && item.get("artwork_url").is_some_and(Value::is_null)
        && let Some(local_artist_id) = item.get("local_artist_id").and_then(Value::as_i64)
        && let Some(tidal_artist_id) = item.get("tidal_artist_id").and_then(Value::as_i64)
        && let Some(tokens) = s.tidal_tokens.clone()
    {
        let http = s.tidal_http_client.clone();
        let db = s.db.clone();
        tokio::spawn(async move {
            crate::services::tidal::artist_photo::ensure_photo_url(
                http,
                tokens,
                db,
                local_artist_id,
                tidal_artist_id,
            )
            .await;
        });
    }

    resolved.unwrap_or_else(|| {
        json!({
            "provider": provider,
            "entity_type": "artist",
            "local_artist_id": null,
            "tidal_artist_id": null,
            "local_track_id": null,
            "tidal_id": null,
            "title": artist,
            "artist_name": artist,
            "album_title": null,
            "artwork_url": image_url,
            "mbid": mbid,
            "score": score,
            "reason": reason,
            "playable": false,
        })
    })
}

async fn resolve_recommendation_album_item(
    state: &SharedState,
    provider: &str,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
    image_url: Option<&str>,
) -> Value {
    let normalized_title = normalize_catalog_name(title);
    let normalized_artist = normalize_catalog_name(artist);
    let s = state.read().await;
    s.db
        .with_conn(|conn| {
            // Title and artist each match on either spelling. Both halves still
            // have to agree, so a folded match cannot pull in another artist's
            // album of the same name.
            conn.query_row(
                "SELECT al.id, al.tidal_id, al.title, a.id, a.tidal_id, a.name, al.artwork_url
                   FROM albums al
                   LEFT JOIN artists a ON a.id = al.artist_id
                  WHERE (LOWER(al.title) = LOWER(?1)
                         OR (?3 <> '' AND al.title_normalized IS NOT NULL AND al.title_normalized = ?3))
                    AND (LOWER(COALESCE(a.name, '')) = LOWER(?2)
                         OR (?4 <> '' AND a.name_normalized IS NOT NULL AND a.name_normalized = ?4))
                  ORDER BY LOWER(al.title) = LOWER(?1) DESC, al.tidal_id IS NULL, al.id ASC
                  LIMIT 1",
                params![title, artist, normalized_title, normalized_artist],
                |row| {
                    Ok(json!({
                        "provider": provider,
                        "entity_type": "album",
                        "local_album_id": row.get::<_, i64>(0)?,
                        "tidal_album_id": row.get::<_, Option<i64>>(1)?,
                        "local_artist_id": row.get::<_, Option<i64>>(3)?,
                        "tidal_artist_id": row.get::<_, Option<i64>>(4)?,
                        "local_track_id": null,
                        "tidal_id": null,
                        "title": row.get::<_, String>(2)?,
                        "artist_name": row.get::<_, Option<String>>(5)?,
                        "album_title": row.get::<_, String>(2)?,
                        "artwork_url": row.get::<_, Option<String>>(6)?.or_else(|| image_url.map(str::to_string)),
                        "mbid": mbid,
                        "score": score,
                        "reason": reason,
                        "playable": true,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            json!({
                "provider": provider,
                "entity_type": "album",
                "local_album_id": null,
                "tidal_album_id": null,
                "local_artist_id": null,
                "tidal_artist_id": null,
                "local_track_id": null,
                "tidal_id": null,
                "title": title,
                "artist_name": artist,
                "album_title": title,
                "artwork_url": image_url,
                "mbid": mbid,
                "score": score,
                "reason": reason,
                "playable": false,
            })
        })
}

fn recommendation_placeholder_item(
    provider: &str,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
) -> Value {
    json!({
        "provider": provider,
        "entity_type": "track",
        "local_track_id": null,
        "tidal_id": 0,
        "title": title,
        "artist_name": artist,
        "album_title": null,
        "artwork_url": null,
        "mbid": mbid,
        "score": score,
        "reason": reason,
        "playable": false,
    })
}

pub(crate) fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Get weekly articles from AllMusic RSS
pub(super) async fn get_home_articles(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let articles = aggregator.get_articles().await;

    Ok(Json(json!({
        "articles": articles,
        "source": "allmusic_rss"
    })))
}

/// Get music industry news from multiple RSS sources
pub(super) async fn get_home_news(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let news = aggregator.get_news().await;

    Ok(Json(json!({
        "news": news,
        "sources": ["billboard", "nme", "spin", "pitchfork", "rolling_stone", "consequence", "the_guardian"],
        "source": "aggregated_rss"
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seeded_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO artists (id, name) VALUES
                (1, 'Sigur Rós'),
                (2, 'Beyoncé'),
                (3, 'Simon & Garfunkel')",
            [],
        )
        .unwrap();
        conn
    }

    fn lookup(conn: &Connection, spelling: &str) -> Option<String> {
        conn.query_row(
            ARTIST_BY_NAME_SQL,
            params![spelling, normalize_catalog_name(spelling)],
            |row| row.get::<_, String>(2),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn provider_spellings_miss_before_the_fold_and_match_after() {
        let conn = seeded_conn();

        // Migration 060 leaves the folded column NULL, so until the backfill
        // runs the resolver behaves exactly as it did before any of this: the
        // exact spelling matches and the provider's does not.
        assert_eq!(lookup(&conn, "Sigur Rós").as_deref(), Some("Sigur Rós"));
        assert_eq!(lookup(&conn, "Sigur Ros"), None);
        assert_eq!(lookup(&conn, "Beyonce"), None);
        assert_eq!(lookup(&conn, "Simon and Garfunkel"), None);

        crate::db::catalog_name::run_backfill_to_completion(&conn, 8).unwrap();

        // These are the exact spellings Last.fm hands back.
        assert_eq!(lookup(&conn, "Sigur Ros").as_deref(), Some("Sigur Rós"));
        assert_eq!(lookup(&conn, "Beyonce").as_deref(), Some("Beyoncé"));
        assert_eq!(
            lookup(&conn, "Simon and Garfunkel").as_deref(),
            Some("Simon & Garfunkel")
        );
        // And the exact spelling still works.
        assert_eq!(lookup(&conn, "Sigur Rós").as_deref(), Some("Sigur Rós"));
    }

    #[test]
    fn non_latin_names_do_not_collide_on_the_empty_fold() {
        let conn = seeded_conn();
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (4, '鄧麗君'), (5, 'Грибы')",
            [],
        )
        .unwrap();
        crate::db::catalog_name::run_backfill_to_completion(&conn, 8).unwrap();

        // The fold keeps ASCII alphanumerics only, so both of these store ''.
        assert_eq!(normalize_catalog_name("鄧麗君"), "");
        assert_eq!(normalize_catalog_name("Грибы"), "");

        // Which is why the folded branch has to stay out of it. Each name
        // still resolves to itself through the exact branch, and neither
        // resolves to the other.
        assert_eq!(lookup(&conn, "鄧麗君").as_deref(), Some("鄧麗君"));
        assert_eq!(lookup(&conn, "Грибы").as_deref(), Some("Грибы"));
        assert_eq!(lookup(&conn, "Вектор А"), None);
    }

    #[test]
    fn an_exact_hit_outranks_a_folded_one() {
        let conn = seeded_conn();
        // Two rows that fold to the same value; only one matches exactly.
        conn.execute("INSERT INTO artists (id, name) VALUES (4, 'Motorhead')", [])
            .unwrap();
        conn.execute("INSERT INTO artists (id, name) VALUES (5, 'Motörhead')", [])
            .unwrap();
        crate::db::catalog_name::run_backfill_to_completion(&conn, 8).unwrap();

        assert_eq!(lookup(&conn, "Motorhead").as_deref(), Some("Motorhead"));
        assert_eq!(lookup(&conn, "Motörhead").as_deref(), Some("Motörhead"));
    }

    #[test]
    fn artwork_queries_match_how_the_client_composes_them() {
        // Same shape as the client's composeTidalArtQuery, so a lookup here and
        // a lookup there land on the same cache row rather than each paying for
        // its own upstream search.
        let artist =
            json!({ "entity_type": "artist", "title": "Lenzman", "artist_name": "Lenzman" });
        assert_eq!(artwork_query(&artist).as_deref(), Some("Lenzman"));

        let track =
            json!({ "entity_type": "track", "title": "The Trot", "artist_name": "Calibre" });
        assert_eq!(artwork_query(&track).as_deref(), Some("Calibre The Trot"));

        let album =
            json!({ "entity_type": "album", "title": "Shelflife", "artist_name": "Calibre" });
        assert_eq!(artwork_query(&album).as_deref(), Some("Calibre Shelflife"));

        // Nothing searchable: a track with no artist would match anything.
        let anonymous = json!({ "entity_type": "track", "title": "Untitled", "artist_name": null });
        assert_eq!(artwork_query(&anonymous), None);
        let empty_artist = json!({ "entity_type": "artist", "title": "  " });
        assert_eq!(artwork_query(&empty_artist), None);
    }

    #[test]
    fn artists_take_the_artist_photo_and_everything_else_takes_a_cover() {
        use crate::services::tidal::client::{
            TidalSearchAlbum, TidalSearchArtist, TidalSearchCatalog, TidalSearchTrack,
        };

        let mut catalog = TidalSearchCatalog::default();
        catalog.artists.push(TidalSearchArtist {
            id: 1,
            name: "Lenzman".into(),
            picture: Some("pic".into()),
            artwork_url: Some("https://img/artist.jpg".into()),
            extra: Default::default(),
        });
        catalog.tracks.push(TidalSearchTrack {
            artwork_url: Some("https://img/cover.jpg".into()),
            ..Default::default()
        });

        // An artist gets the photo, not the cover of one of their tracks -
        // reading tracks[0] for an artist is what put album art on artist
        // tiles in the first place.
        assert_eq!(
            artwork_from_catalog("artist", &catalog).as_deref(),
            Some("https://img/artist.jpg")
        );
        assert_eq!(
            artwork_from_catalog("track", &catalog).as_deref(),
            Some("https://img/cover.jpg")
        );

        // No artist hit: fall back to a cover rather than showing nothing.
        let mut coverless = TidalSearchCatalog::default();
        coverless.albums.push(TidalSearchAlbum {
            artwork_url: Some("https://img/album.jpg".into()),
            ..Default::default()
        });
        assert_eq!(
            artwork_from_catalog("artist", &coverless).as_deref(),
            Some("https://img/album.jpg")
        );
        assert_eq!(
            artwork_from_catalog("track", &TidalSearchCatalog::default()),
            None
        );
    }

    #[test]
    fn album_ids_come_only_from_an_album_that_is_actually_the_one_asked_for() {
        use crate::services::tidal::client::{TidalSearchAlbum, TidalSearchCatalog};

        let album = |id: i64, title: &str, artist: &str| TidalSearchAlbum {
            id,
            title: title.into(),
            artist_name: Some(artist.into()),
            ..Default::default()
        };
        let item = |title: &str, artist: &str| json!({ "entity_type": "album", "title": title, "artist_name": artist });

        let mut catalog = TidalSearchCatalog::default();
        catalog
            .albums
            .push(album(1, "Hurt So Good", "Althea & Donna"));
        catalog.albums.push(album(2, "Untrue", "Burial"));

        // Exact fold, and the edition suffix Last.fm often carries.
        assert_eq!(
            album_id_from_catalog(&item("Hurt So Good", "Althea and Donna"), &catalog),
            Some(1)
        );
        assert_eq!(
            album_id_from_catalog(&item("Untrue (Deluxe Edition)", "Burial"), &catalog),
            Some(2)
        );

        // The single Last.fm recommended, which TIDAL has no album for. The old
        // client-side rule took the artist's only album here, so two different
        // titles both opened "Hurt So Good".
        assert_eq!(
            album_id_from_catalog(&item("Uptown Top Ranking", "Althea & Donna"), &catalog),
            None
        );
        // Right title, wrong artist.
        assert_eq!(
            album_id_from_catalog(&item("Untrue", "Althea & Donna"), &catalog),
            None
        );
        // A name that folds to nothing (non-Latin) must not match everything.
        assert_eq!(
            album_id_from_catalog(&item("黑膠", "鄧麗君"), &catalog),
            None
        );
    }

    #[test]
    fn album_recommendations_with_no_tidal_album_never_reach_the_shelf() {
        let mut items = vec![
            json!({ "entity_type": "album", "title": "Owned", "local_album_id": 7, "tidal_album_id": null }),
            json!({ "entity_type": "album", "title": "Resolved", "local_album_id": null, "tidal_album_id": 42 }),
            json!({ "entity_type": "album", "title": "Dead", "local_album_id": null, "tidal_album_id": null }),
            // Not albums: a track resolves through its own path and an artist is
            // browsable by name, so neither is dead the way a dead album is.
            json!({ "entity_type": "track", "title": "Track", "local_track_id": null }),
            json!({ "entity_type": "artist", "title": "Artist", "local_artist_id": null }),
        ];
        drop_unresolvable_albums(&mut items);
        let titles: Vec<&str> = items
            .iter()
            .map(|i| i.get("title").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(titles, vec!["Owned", "Resolved", "Track", "Artist"]);
    }

    #[test]
    fn one_record_reaching_the_shelf_twice_becomes_one_card() {
        // The payload that blanked Home: the same album under a decomposed and a
        // precomposed spelling of the same title. Both survive the pre-resolution
        // title dedupe and both resolve to TIDAL album 167919206.
        let mut items = vec![
            json!({ "entity_type": "album", "title": "Du\u{0308}nyala", "artist_name": "Aykut Bilir", "tidal_album_id": 167919206 }),
            json!({ "entity_type": "album", "title": "D\u{00fc}nyala", "artist_name": "Aykut Bilir", "tidal_album_id": 167919206 }),
            // A different album by the same artist stays.
            json!({ "entity_type": "album", "title": "Other", "artist_name": "Aykut Bilir", "tidal_album_id": 42 }),
            // Library and TIDAL ids are separate namespaces, so these are two
            // records even though the numbers match.
            json!({ "entity_type": "album", "title": "Owned", "local_album_id": 42 }),
            // Singles are identified by their track, not by an album.
            json!({ "entity_type": "album", "title": "Single", "tidal_id": 909, "is_single": true }),
            json!({ "entity_type": "album", "title": "Same single", "tidal_id": 909, "is_single": true }),
            // Not albums: keyed by their own path, never folded here.
            json!({ "entity_type": "track", "title": "Track", "tidal_id": 909 }),
            json!({ "entity_type": "artist", "title": "Artist" }),
        ];
        drop_duplicate_albums(&mut items);
        let titles: Vec<&str> = items
            .iter()
            .map(|i| i.get("title").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Du\u{0308}nyala",
                "Other",
                "Owned",
                "Single",
                "Track",
                "Artist"
            ]
        );
    }

    #[test]
    fn a_single_keeps_its_card_and_a_compilation_does_not() {
        use crate::services::tidal::client::{TidalSearchCatalog, TidalSearchTrack};

        let mut catalog = TidalSearchCatalog::default();
        catalog.tracks.push(TidalSearchTrack {
            id: 909,
            title: "Cry Tough".into(),
            artist_name: Some("Alton Ellis".into()),
            ..Default::default()
        });

        let single =
            json!({ "entity_type": "album", "title": "Cry Tough", "artist_name": "Alton Ellis" });
        assert_eq!(single_id_from_catalog(&single, &catalog), Some(909));

        // A compilation TIDAL has neither the album nor a matching track for.
        let compilation = json!({
            "entity_type": "album",
            "title": "Treasure Isle Collection Vol. 1",
            "artist_name": "Alton Ellis",
        });
        assert_eq!(single_id_from_catalog(&compilation, &catalog), None);

        // The single survives the drop because its card can still seed radio;
        // the compilation cannot do anything at all, so it goes.
        let mut items = vec![
            json!({ "entity_type": "album", "title": "Cry Tough", "tidal_id": 909, "is_single": true }),
            json!({ "entity_type": "album", "title": "Treasure Isle Collection Vol. 1" }),
        ];
        drop_unresolvable_albums(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].get("title").and_then(Value::as_str),
            Some("Cry Tough")
        );
    }

    #[test]
    fn a_short_shelf_gets_a_short_cache_lease() {
        // Every upstream call in the fan-out is unwrap_or_default, so a
        // rate-limited window returns fewer items rather than an error. Writing
        // that at the full six-hour TTL pinned a half-empty Home until it
        // expired, which is what "only a few tracks" looked like.
        let short: Vec<Value> = (0..4).map(|i| json!({ "i": i })).collect();
        assert_eq!(
            recommendation_cache_ttl(&short),
            RECOMMENDATION_SHORT_TTL_SECS
        );
        assert_eq!(recommendation_cache_ttl(&[]), RECOMMENDATION_SHORT_TTL_SECS);

        let healthy: Vec<Value> = (0..LASTFM_HOME_RECOMMENDATION_LIMIT)
            .map(|i| json!({ "i": i }))
            .collect();
        assert_eq!(
            recommendation_cache_ttl(&healthy),
            RECOMMENDATION_FULL_TTL_SECS
        );

        // Exactly at the floor counts as healthy.
        let floor: Vec<Value> = (0..RECOMMENDATION_HEALTHY_FLOOR)
            .map(|i| json!({ "i": i }))
            .collect();
        assert_eq!(
            recommendation_cache_ttl(&floor),
            RECOMMENDATION_FULL_TTL_SECS
        );
    }

    #[test]
    fn folding_does_not_match_unrelated_artists() {
        let conn = seeded_conn();
        crate::db::catalog_name::run_backfill_to_completion(&conn, 8).unwrap();
        assert_eq!(lookup(&conn, "Sigur"), None);
        assert_eq!(lookup(&conn, "Beyonce Knowles"), None);
    }
}
