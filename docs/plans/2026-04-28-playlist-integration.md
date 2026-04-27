# Playlist Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring playlists to full parity with albums/artists: shuffle, radio, and favorite actions on playlist cards; "Add to playlist" bulk-insert from album/artist context menus; a playlists row in search results backed by local library + TIDAL search.

**Architecture:** Four independent workstreams sharing a common API client update (Task 4). Backend tasks (1–3) add new DB fields, query functions, and route handlers following the existing rusqlite + axum pattern. Frontend tasks (5–7) depend on Task 4 being complete first.

**Tech Stack:** Rust (axum, rusqlite), SvelteKit, TypeScript. Backend at `noor-server/`, frontend at `frontend/`.

---

## Files

### Created
- `docs/plans/2026-04-28-playlist-integration.md` ← this file

### Modified
| File | Change |
|---|---|
| `noor-server/src/db/schema.rs` | Add MIGRATION_017 for `is_favorite` on playlists |
| `noor-server/src/db/models.rs` | Add `is_favorite: bool` to `Playlist` struct |
| `noor-server/src/db/queries.rs` | Add `toggle_playlist_favorite`, `add_tracks_to_playlist`; update `get_playlists`/`get_playlist` selects and ordering |
| `noor-server/src/services/tidal/client.rs` | Add `search_playlists` method; add `playlists` field to `TidalSearchCatalog` |
| `noor-server/src/server/routes.rs` | Add 4 new route handlers + register them |
| `frontend/src/lib/api/client.ts` | Add `is_favorite` to `Playlist`; add `TidalSearchPlaylist` type; add 4 new API methods |
| `frontend/src/lib/stores/player.ts` | Add `shufflePlaylist`, `startPlaylistRadio`, `playTidalPlaylist` |
| `frontend/src/routes/playlists/+page.svelte` | Add shuffle/radio/heart buttons to card headers |
| `frontend/src/routes/library/+page.svelte` | Add "Add to playlist" submenu to album/artist context menus |
| `frontend/src/routes/search/+page.svelte` | Add playlists row + "Playlists" mode pill |

---

## Task 1: Backend — `is_favorite` on playlists

**Files:**
- Modify: `noor-server/src/db/schema.rs`
- Modify: `noor-server/src/db/models.rs`
- Modify: `noor-server/src/db/queries.rs`

- [ ] **Step 1: Add migration**

Open `noor-server/src/db/schema.rs`. Find `MIGRATION_016` and the `MIGRATIONS` array at the bottom. Add after `MIGRATION_016`:

```rust
const MIGRATION_017: &str = r#"
ALTER TABLE playlists ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_playlists_favorite ON playlists(is_favorite);
"#;
```

Then add `MIGRATION_017` to the end of the `MIGRATIONS` slice:

```rust
pub const MIGRATIONS: &[&str] = &[
    // ... existing entries ...
    MIGRATION_016,
    MIGRATION_017,
];
```

- [ ] **Step 2: Update `Playlist` struct**

In `noor-server/src/db/models.rs`, add `is_favorite` to the `Playlist` struct. Find the struct (around line 60) and add the field:

```rust
pub struct Playlist {
    pub id: i64,
    pub tidal_uuid: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub is_smart: bool,
    pub smart_rules: Option<String>,
    pub is_synced: bool,
    pub track_count: i32,
    pub is_favorite: bool,   // ← add this
}
```

- [ ] **Step 3: Update `get_playlists` in queries.rs**

Find `pub fn get_playlists` (around line 415). Update the SQL to select `is_favorite` (column index 8) and order favorites first:

```rust
pub fn get_playlists(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_uuid, name, description, is_smart,
                smart_rules, is_synced, track_count, is_favorite
         FROM playlists
         ORDER BY is_favorite DESC, name ASC",
    )?;

    let playlists = stmt
        .query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                tidal_uuid: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                is_smart: row.get::<_, i32>(4)? != 0,
                smart_rules: row.get(5)?,
                is_synced: row.get::<_, i32>(6)? != 0,
                track_count: row.get(7)?,
                is_favorite: row.get::<_, i32>(8)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(playlists)
}
```

- [ ] **Step 4: Update `get_playlist` (single) in queries.rs**

Find `pub fn get_playlist` (around line 441). Apply the same column addition:

```rust
pub fn get_playlist(conn: &Connection, playlist_id: i64) -> Result<Option<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_uuid, name, description, is_smart,
                smart_rules, is_synced, track_count, is_favorite
         FROM playlists
         WHERE id = ?1",
    )?;

    let mut rows = stmt.query(params![playlist_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(Playlist {
            id: row.get(0)?,
            tidal_uuid: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            is_smart: row.get::<_, i32>(4)? != 0,
            smart_rules: row.get(5)?,
            is_synced: row.get::<_, i32>(6)? != 0,
            track_count: row.get(7)?,
            is_favorite: row.get::<_, i32>(8)? != 0,
        }))
    } else {
        Ok(None)
    }
}
```

- [ ] **Step 5: Add `toggle_playlist_favorite` to queries.rs**

Add after `get_playlist`:

```rust
pub fn toggle_playlist_favorite(conn: &Connection, playlist_id: i64) -> Result<Playlist> {
    conn.execute(
        "UPDATE playlists SET is_favorite = NOT is_favorite WHERE id = ?1",
        params![playlist_id],
    )?;
    get_playlist(conn, playlist_id)?
        .ok_or_else(|| anyhow::anyhow!("playlist not found"))
}
```

- [ ] **Step 6: Add route handler in routes.rs**

Find the `get_playlist_tracks` handler block (around line 932). Add a new handler below it:

```rust
async fn toggle_playlist_favorite_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let playlist = queries::toggle_playlist_favorite(conn, id)?;
            Ok(Json(json!({ "playlist": playlist })))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })
}
```

- [ ] **Step 7: Register route**

Find the router block in `routes.rs` where `/api/playlists` routes are defined (around line 250):

```rust
.route("/api/playlists", get(get_playlists))
.route("/api/playlists/{id}/tracks", get(get_playlist_tracks))
```

Add:

```rust
.route("/api/playlists/{id}/favorite", patch(toggle_playlist_favorite_route))
```

- [ ] **Step 8: Verify it compiles**

```bash
cd noor-server && cargo check 2>&1 | head -40
```

Expected: no errors. Fix any type mismatches (e.g. `i32` vs `bool` coercions).

- [ ] **Step 9: Commit**

```bash
git add noor-server/src/db/schema.rs noor-server/src/db/models.rs noor-server/src/db/queries.rs noor-server/src/server/routes.rs
git commit -m "feat(backend): add is_favorite to playlists with toggle endpoint"
```

---

## Task 2: Backend — add tracks to playlist

**Files:**
- Modify: `noor-server/src/db/queries.rs`
- Modify: `noor-server/src/server/routes.rs`

- [ ] **Step 1: Write a test for `add_tracks_to_playlist`**

At the bottom of `noor-server/src/db/queries.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use crate::db::schema::run_migrations;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn test_add_tracks_to_playlist_deduplicates() {
        let conn = test_conn();
        // Insert a minimal artist, album, track, and synced playlist
        conn.execute_batch(r#"
            INSERT INTO artists (id, name) VALUES (1, 'Test Artist');
            INSERT INTO albums (id, title, artist_id) VALUES (1, 'Test Album', 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (1, 'Track A', 1, 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (2, 'Track B', 1, 1);
            INSERT INTO playlists (id, name, is_smart, is_synced) VALUES (1, 'My Playlist', 0, 1);
        "#).unwrap();

        // First call adds both tracks
        let added = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
        assert_eq!(added, 2);

        // Second call with same tracks returns 0 (deduplicated)
        let added_again = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
        assert_eq!(added_again, 0);
    }
}
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cd noor-server && cargo test test_add_tracks_to_playlist_deduplicates 2>&1 | tail -10
```

Expected: FAIL — `add_tracks_to_playlist` not found.

- [ ] **Step 3: Implement `add_tracks_to_playlist`**

Add after `toggle_playlist_favorite` in `queries.rs`:

```rust
/// Bulk-insert tracks into a playlist, skipping any already present.
/// Returns the number of tracks actually inserted.
pub fn add_tracks_to_playlist(
    conn: &Connection,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<usize> {
    if track_ids.is_empty() {
        return Ok(0);
    }

    // Find which tracks are already in the playlist
    let existing: std::collections::HashSet<i64> = {
        let mut stmt = conn.prepare(
            "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1",
        )?;
        stmt.query_map(params![playlist_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    };

    let to_insert: Vec<i64> = track_ids
        .iter()
        .copied()
        .filter(|id| !existing.contains(id))
        .collect();

    if to_insert.is_empty() {
        return Ok(0);
    }

    // Get the current max position
    let max_pos: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) FROM playlist_tracks WHERE playlist_id = ?1",
        params![playlist_id],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
    )?;
    for (i, &track_id) in to_insert.iter().enumerate() {
        stmt.execute(params![playlist_id, track_id, max_pos + 1 + i as i64])?;
    }

    // Keep track_count in sync
    conn.execute(
        "UPDATE playlists SET track_count = (
            SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1
         ) WHERE id = ?1",
        params![playlist_id],
    )?;

    Ok(to_insert.len())
}
```

- [ ] **Step 4: Run the test to confirm it passes**

```bash
cd noor-server && cargo test test_add_tracks_to_playlist_deduplicates 2>&1 | tail -10
```

Expected: `test test_add_tracks_to_playlist_deduplicates ... ok`

- [ ] **Step 5: Add route handler in routes.rs**

Add a struct for the request body near other request structs (search for `#[derive(Deserialize)]`):

```rust
#[derive(Debug, Deserialize)]
struct AddTracksToPlaylistRequest {
    track_ids: Vec<i64>,
}
```

Add the handler:

```rust
async fn add_tracks_to_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<AddTracksToPlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let added = queries::add_tracks_to_playlist(conn, id, &payload.track_ids)?;
            Ok(Json(json!({ "added": added })))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })
}
```

- [ ] **Step 6: Register route**

In the router block, add below the favorite route:

```rust
.route("/api/playlists/{id}/tracks", post(add_tracks_to_playlist_route).get(get_playlist_tracks))
```

Note: this replaces the existing `.route("/api/playlists/{id}/tracks", get(get_playlist_tracks))` line — axum chains multiple methods on the same route via `.get(...).post(...)`.

- [ ] **Step 7: Cargo check**

```bash
cd noor-server && cargo check 2>&1 | head -40
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add noor-server/src/db/queries.rs noor-server/src/server/routes.rs
git commit -m "feat(backend): add bulk tracks-to-playlist endpoint"
```

---

## Task 3: Backend — TIDAL playlist search + tracks endpoints

**Files:**
- Modify: `noor-server/src/services/tidal/client.rs`
- Modify: `noor-server/src/server/routes.rs`

- [ ] **Step 1: Add `search_playlists` to TidalClient**

In `noor-server/src/services/tidal/client.rs`, find `search_catalog` (around line 326). Add a new method after `get_playlist_tracks`:

```rust
pub async fn search_playlists(
    &self,
    query: &str,
    limit: i32,
) -> Result<Vec<TidalPlaylist>> {
    let url = format!(
        "{}/search?query={}&countryCode={}&limit={}&types=PLAYLISTS",
        TIDAL_API_URL,
        urlencoding::encode(query),
        self.country_code,
        limit,
    );
    let payload: serde_json::Value = self.get_json(&url).await?;
    let items = payload
        .get("playlists")
        .and_then(|p| p.get("items"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<TidalPlaylist>(v).ok())
        .collect())
}
```

- [ ] **Step 2: Add TIDAL playlist search route handler in routes.rs**

Find `tidal_search` handler (around line 5511). Add two new handlers below it:

```rust
#[derive(Debug, Deserialize)]
struct TidalPlaylistSearchParams {
    q: String,
    #[serde(default)]
    limit: Option<i32>,
}

async fn tidal_playlist_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalPlaylistSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "TIDAL not connected" }))));
    };

    let limit = params.limit.unwrap_or(20).min(50);
    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let playlists = client.search_playlists(&params.q, limit).await.map_err(|e| {
        (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() })))
    })?;

    Ok(Json(json!({ "playlists": playlists })))
}

async fn tidal_playlist_tracks(
    State(state): State<SharedState>,
    Path(uuid): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "TIDAL not connected" }))));
    };

    let http_client = state.read().await.http_client.clone();
    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let resp = client.get_playlist_tracks(&uuid, 100, 0).await.map_err(|e| {
        if error_looks_like_auth(&e) {
            (StatusCode::UNAUTHORIZED, Json(json!({ "error": "TIDAL session expired" })))
        } else {
            (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() })))
        }
    })?;

    // Convert TidalTrack → TidalPlayable shape the frontend expects
    let playable: Vec<serde_json::Value> = resp.items.iter().map(|t| {
        json!({
            "tidal_id": t.id,
            "title": t.title,
            "artist_name": t.artist.as_ref().map(|a| &a.name),
            "album_title": t.album.as_ref().map(|a| &a.title),
            "artwork_url": t.album.as_ref().and_then(|a| a.cover.as_ref()).map(|c| {
                format!("https://resources.tidal.com/images/{}/320x320.jpg", c.replace('-', "/"))
            }),
            "duration_ms": t.duration.map(|d| d * 1000),
            "track_id": 0,
            "is_in_library": false,
        })
    }).collect();

    Ok(Json(json!({ "tracks": playable })))
}
```

Note: `TidalTrack` fields (`id`, `title`, `artist`, `album`, `duration`, `cover`) — check the actual field names in `TidalTrack` struct (around line 26 of client.rs) and adjust if needed.

- [ ] **Step 3: Register routes**

Find the TIDAL routes block (around line 356). Add:

```rust
.route("/api/tidal/playlists/search", get(tidal_playlist_search))
.route("/api/tidal/playlists/{uuid}/tracks", get(tidal_playlist_tracks))
```

- [ ] **Step 4: Cargo check**

```bash
cd noor-server && cargo check 2>&1 | head -40
```

Fix any field name mismatches in `TidalTrack` — check the struct definition if `artist`, `album`, `duration` fields differ.

- [ ] **Step 5: Smoke test TIDAL playlist search manually**

Start the server (`cargo run` or however the project runs), then:

```bash
curl "http://localhost:<port>/api/tidal/playlists/search?q=chill" | jq '.playlists | length'
```

Expected: a number > 0 (or 0 if no results, but not an error).

- [ ] **Step 6: Commit**

```bash
git add noor-server/src/services/tidal/client.rs noor-server/src/server/routes.rs
git commit -m "feat(backend): TIDAL playlist search and tracks endpoints"
```

---

## Task 4: Frontend — API client updates

**Files:**
- Modify: `frontend/src/lib/api/client.ts`

This task must be completed before Tasks 5, 6, and 7.

- [ ] **Step 1: Add `is_favorite` to the `Playlist` interface**

Find the `Playlist` interface (around line 184):

```typescript
export interface Playlist {
  id: number;
  tidal_uuid: string | null;
  name: string;
  description: string | null;
  is_smart: boolean;
  track_count: number;
  smart_rules?: string | null;
  is_favorite: boolean;   // ← add this
}
```

- [ ] **Step 2: Add `TidalSearchPlaylist` type**

Add near the other Tidal search types (near `TidalSearchAlbum`, around line 98):

```typescript
export interface TidalSearchPlaylist {
  uuid: string;
  title: string;
  description: string | null;
  number_of_tracks: number | null;
  square_image: string | null;
}
```

- [ ] **Step 3: Add `TidalPlayable` playlist track type**

The TIDAL playlist tracks endpoint returns the same shape as other TIDAL playable tracks. Verify `TidalPlayable` is already defined and has `tidal_id`, `title`, `artist_name`, `album_title`, `artwork_url`, `duration_ms`, `track_id`, `is_in_library`. If not, add it. It is likely already present (used by `startTidalSongRadio`). No change needed if it exists.

- [ ] **Step 4: Add API methods**

Find the playlist API methods section (around line 921). Add after `deleteSmartPlaylist`:

```typescript
togglePlaylistFavorite(id: number) {
  return fetchApi<{ playlist: Playlist }>(`/api/playlists/${id}/favorite`, undefined, {
    method: 'PATCH',
  });
},

addTracksToPlaylist(id: number, trackIds: number[]) {
  return fetchApi<{ added: number }>(`/api/playlists/${id}/tracks`, undefined, {
    method: 'POST',
    body: JSON.stringify({ track_ids: trackIds }),
  });
},

searchTidalPlaylists(q: string) {
  return fetchApi<{ playlists: TidalSearchPlaylist[] }>(
    `/api/tidal/playlists/search?q=${encodeURIComponent(q)}`,
  );
},

getTidalPlaylistTracks(tidalUuid: string) {
  return fetchApi<{ tracks: TidalPlayable[] }>(
    `/api/tidal/playlists/${tidalUuid}/tracks`,
  );
},
```

- [ ] **Step 5: Check TypeScript compiles**

```bash
cd frontend && npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors related to the new types/methods.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/api/client.ts
git commit -m "feat(frontend): API client — playlist favorite, add-tracks, TIDAL playlist search/tracks"
```

---

## Task 5: Frontend — playlist card shuffle/radio/heart buttons

**Files:**
- Modify: `frontend/src/lib/stores/player.ts`
- Modify: `frontend/src/routes/playlists/+page.svelte`

- [ ] **Step 1: Add `shufflePlaylist` to player.ts**

Find `startArtistRadio` (around line 484). Add after it:

```typescript
export async function shufflePlaylist(
  id: number,
  tracks: { id: number }[],
) {
  if (!tracks.length) return;
  const shuffled = shuffleArray([...tracks]);
  await loadQueueAndPlay(shuffled.map((t) => t.id));
  showToast('Shuffling playlist', 'success');
}
```

`shuffleArray` is already imported/defined in the file — check it's in scope. If not, add:

```typescript
function shuffleArray<T>(arr: T[]): T[] {
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [arr[i], arr[j]] = [arr[j], arr[i]];
  }
  return arr;
}
```

- [ ] **Step 2: Add `startPlaylistRadio` to player.ts**

Add after `shufflePlaylist`:

```typescript
export async function startPlaylistRadio(tracks: { id: number; play_count?: number }[]) {
  if (!tracks.length) return;
  // Seed from the most-played track; fall back to first track
  const seed = [...tracks].sort((a, b) => (b.play_count ?? 0) - (a.play_count ?? 0))[0];
  await startSongRadio(seed.id);
}
```

- [ ] **Step 3: Add `playTidalPlaylist` to player.ts**

Add after `startPlaylistRadio`:

```typescript
export async function playTidalPlaylist(tidalUuid: string) {
  try {
    const { tracks } = await api.getTidalPlaylistTracks(tidalUuid);
    if (!tracks.length) {
      playerError.set('No playable tracks in this playlist.');
      return;
    }
    // Use existing TIDAL ephemeral queue path — play tracks as TidalPlayable[]
    for (const track of tracks) {
      await addTidalTrackToQueue(track);
    }
    await playTidalTrackNow(tracks[0]);
    showToast('Playing TIDAL playlist', 'success');
  } catch (error) {
    playerError.set(`Failed to load TIDAL playlist: ${error}`);
  }
}
```

Note: `addTidalTrackToQueue` and `playTidalTrackNow` are existing functions in player.ts. Verify their signatures — they take a `TidalPlayable`. If the `playTidalPlaylist` approach of adding each track individually is too slow (many tracks), an alternative is to call `replacePlaybackQueue` with local IDs only — but since TIDAL tracks have `track_id: 0`, use the TIDAL ephemeral queue helpers.

- [ ] **Step 4: Add buttons to playlist card in playlists/+page.svelte**

Find the import block at the top of `frontend/src/routes/playlists/+page.svelte`. Add the new player functions to the import:

```typescript
import { playPlaylist, shufflePlaylist, startPlaylistRadio } from '$lib/stores/player';
```

(These are in addition to whatever is already imported from player.)

Also import the API client for the favorite toggle:

```typescript
import { api } from '$lib/api/client';
import { showToast } from '$lib/stores/toasts'; // if not already imported
```

- [ ] **Step 5: Add heart + shuffle + radio buttons to the card header**

Find the `.playlist-side` or the action area of the playlist card header (around line 596 in the playlist card markup). The current header has a track count and expand chevron. Add the new buttons before the expand chevron:

```svelte
<!-- Heart / favorite -->
<button
  class="action-btn fav-btn"
  class:active={playlist.is_favorite}
  onclick={async (e) => {
    e.stopPropagation();
    const updated = await api.togglePlaylistFavorite(playlist.id);
    // Optimistic: update the local playlists array
    playlists = playlists.map(p => p.id === playlist.id ? updated.playlist : p);
  }}
  title={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
  aria-label={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
>♥</button>

<!-- Shuffle — only if tracks are loaded -->
{#if playlistTracksById[playlist.id]?.length}
  <button
    class="action-btn"
    onclick={(e) => {
      e.stopPropagation();
      void shufflePlaylist(playlist.id, playlistTracksById[playlist.id]);
    }}
    title="Shuffle playlist"
    aria-label="Shuffle playlist"
  >⤮ Shuffle</button>

  <button
    class="action-btn"
    onclick={(e) => {
      e.stopPropagation();
      void startPlaylistRadio(playlistTracksById[playlist.id]);
    }}
    title="Playlist radio"
    aria-label="Playlist radio"
  >◉ Radio</button>
{/if}
```

Note: shuffle/radio buttons are shown only after tracks are loaded (expanded). If you want them always visible, you'll need to trigger `expandPlaylist(playlist.id)` on click before calling the action. Choose whichever UX feels right.

- [ ] **Step 6: Add CSS for the new buttons**

Find the existing `.smart-actions` or `.playlist-side` styles in `playlists/+page.svelte` (or its `<style>` block). Add:

```css
.action-btn {
  background: var(--surface-2, #2a2a2a);
  border: none;
  color: var(--text-secondary, #ccc);
  cursor: pointer;
  font-size: 12px;
  padding: 5px 10px;
  border-radius: 5px;
  display: flex;
  align-items: center;
  gap: 4px;
  white-space: nowrap;
}
.action-btn:hover {
  background: var(--surface-3, #333);
  color: var(--text-primary, #fff);
}
.action-btn.fav-btn {
  background: none;
  font-size: 16px;
  padding: 5px;
  color: var(--text-tertiary, #666);
}
.action-btn.fav-btn.active {
  color: var(--accent, #e00055);
}
```

- [ ] **Step 7: Verify in browser**

Start the dev server (`cd frontend && npm run dev`), navigate to the playlists page. Confirm:
- Heart button appears, toggles red on click
- After expanding a playlist, Shuffle and Radio buttons appear
- Clicking Shuffle loads a shuffled queue and plays
- Clicking Radio starts song radio from the most-played track

- [ ] **Step 8: Commit**

```bash
git add frontend/src/lib/stores/player.ts frontend/src/routes/playlists/+page.svelte
git commit -m "feat(frontend): playlist shuffle, radio, and favorite buttons"
```

---

## Task 6: Frontend — Add to playlist context menus (album/artist)

**Files:**
- Modify: `frontend/src/routes/library/+page.svelte`

- [ ] **Step 1: Import dependencies**

At the top of `frontend/src/routes/library/+page.svelte`, ensure these are imported (add any missing ones):

```typescript
import { api } from '$lib/api/client';
import type { Playlist } from '$lib/api/client';
import { showToast } from '$lib/stores/toasts'; // adjust path if different
```

- [ ] **Step 2: Load playlists on mount**

Find the `onMount` block (or `$effect` / reactive initialization) in the library page. Add a playlists load:

```typescript
let playlists: Playlist[] = $state([]);

onMount(async () => {
  // ... existing mount code ...
  const { playlists: loaded } = await api.getPlaylists();
  // Exclude smart playlists (rules-based, can't accept manual inserts)
  playlists = loaded.filter(p => !p.is_smart);
});
```

If `onMount` doesn't exist, add it — import from `'svelte'`.

- [ ] **Step 3: Add `buildAddToPlaylistSubmenu` helper**

Add this helper function near `buildLocalAlbumMenu`:

```typescript
function buildAddToPlaylistSubmenu(
  getTrackIds: () => Promise<number[]>,
): MenuItem[] {
  const items: MenuItem[] = playlists
    .sort((a, b) => {
      if (a.is_favorite !== b.is_favorite) return a.is_favorite ? -1 : 1;
      return a.name.localeCompare(b.name);
    })
    .map((playlist) => ({
      label: playlist.name,
      icon: playlist.is_favorite ? '♥' : '♩',
      onSelect: async () => {
        const trackIds = await getTrackIds();
        if (!trackIds.length) return;
        const { added } = await api.addTracksToPlaylist(playlist.id, trackIds);
        showToast(`Added ${added} track${added !== 1 ? 's' : ''} to ${playlist.name}`, 'success');
      },
    }));

  return items;
}
```

- [ ] **Step 4: Update `buildLocalAlbumMenu` to include Add to playlist**

Find `buildLocalAlbumMenu` (around line 27). Add the new item:

```typescript
function buildLocalAlbumMenu(album: { id: number; title: string }): MenuItem[] {
  return [
    { label: 'Play album', icon: '▶', onSelect: () => void playAlbumStore(album.id) },
    { label: 'Shuffle album', icon: '⤮', onSelect: () => void shuffleAlbum(album.id) },
    { separator: true, label: '' },
    { label: 'Album radio', icon: '◉', onSelect: () => void startAlbumRadio(album.id) },
    { separator: true, label: '' },
    { label: 'Open album', icon: '↗', onSelect: () => void goto(`/albums/${album.id}`) },
    { separator: true, label: '' },
    {
      label: 'Add to playlist',
      icon: '＋',
      submenu: buildAddToPlaylistSubmenu(async () => {
        const { tracks } = await api.getAlbumTracks(album.id);
        return tracks.map(t => t.id);
      }),
    },
  ];
}
```

- [ ] **Step 5: Update `buildLocalArtistMenu` to include Add to playlist**

Find `buildLocalArtistMenu` (around line 38). Add the new item:

```typescript
function buildLocalArtistMenu(artistId: number): MenuItem[] {
  return [
    { label: 'Open artist', icon: '↗', onSelect: () => void goto(`/artists/${artistId}`) },
    { separator: true, label: '' },
    { label: 'Artist radio', icon: '◉', onSelect: () => void startArtistRadio(artistId) },
    { separator: true, label: '' },
    {
      label: 'Add to playlist',
      icon: '＋',
      submenu: buildAddToPlaylistSubmenu(async () => {
        const { tracks } = await api.getArtistTracks(artistId);
        return tracks.map(t => t.id);
      }),
    },
  ];
}
```

- [ ] **Step 6: Verify submenus render correctly**

Check the context menu component (`$lib/components/ContextMenu.svelte` or similar) renders `submenu` items. Since `MenuItem` already has `submenu?: MenuItem[]`, the context menu UI should already support it — but verify it actually renders a nested menu on hover/click. If not, that component needs updating (look for how it maps `MenuItem[]` to DOM).

- [ ] **Step 7: Verify in browser**

Open the library page. Right-click an album card. Confirm:
- "Add to playlist" appears at the bottom
- Hovering/clicking shows the playlist submenu
- Favorited playlists appear first (♥ icon)
- Clicking a playlist name triggers the bulk insert and shows a toast

Repeat for artist cards.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/routes/library/+page.svelte
git commit -m "feat(frontend): add-to-playlist context menu on album and artist cards"
```

---

## Task 7: Frontend — Playlists in search results

**Files:**
- Modify: `frontend/src/routes/search/+page.svelte`

- [ ] **Step 1: Add playlists state and load on mount**

Find the top of the script block in `search/+page.svelte`. Add:

```typescript
import type { Playlist, TidalSearchPlaylist } from '$lib/api/client';

let localPlaylists: Playlist[] = $state([]);
let tidalPlaylistResults: TidalSearchPlaylist[] = $state([]);

// Load local playlists once on mount (for client-side name filtering)
onMount(async () => {
  const { playlists } = await api.getPlaylists();
  localPlaylists = playlists;
});
```

- [ ] **Step 2: Add TIDAL playlist search to the search function**

Find where the main search is triggered (the function/reactive block that calls `api.searchTidal` or similar). Add a parallel TIDAL playlist fetch:

```typescript
// Inside the search handler, alongside the existing TIDAL search:
if (query.trim()) {
  try {
    const { playlists } = await api.searchTidalPlaylists(query);
    tidalPlaylistResults = playlists;
  } catch {
    tidalPlaylistResults = [];
  }
} else {
  tidalPlaylistResults = [];
}
```

- [ ] **Step 3: Add filtered playlists derived value**

Add a derived computation for the playlists row (client-side name filter + library-boosted ordering):

```typescript
const filteredPlaylists = $derived(() => {
  if (!query.trim()) return [];
  const q = query.trim().toLowerCase();

  const matched = localPlaylists.filter(p =>
    p.name.toLowerCase().includes(q)
  );

  // Merge: local matches first, then TIDAL results not matching a local name
  const localNames = new Set(matched.map(p => p.name.toLowerCase()));
  const tidalOnly = tidalPlaylistResults.filter(
    tp => !localNames.has(tp.title.toLowerCase())
  );

  return { local: matched, tidal: tidalOnly };
});
```

- [ ] **Step 4: Add `FilterMode` update**

Find the `FilterMode` type definition (around the mode pills). Add `'playlists'`:

```typescript
type FilterMode = 'all' | 'artists' | 'albums' | 'tracks' | 'library' | 'playlists';
```

- [ ] **Step 5: Add "Playlists" mode pill**

Find the filter pills array (around line 512):

```svelte
{#each [
  { id: 'all', label: 'All' },
  { id: 'artists', label: 'Artists' },
  { id: 'albums', label: 'Albums' },
  { id: 'tracks', label: 'Tracks' },
  { id: 'library', label: 'In Library' },
  { id: 'playlists', label: 'Playlists' },   // ← add this
] as pill (pill.id)}
```

- [ ] **Step 6: Add playlist visibility logic**

Find where `applyFilter` / section visibility is computed. Add:

```typescript
const showPlaylists = filterMode === 'all' || filterMode === 'playlists';
```

- [ ] **Step 7: Add TIDAL playlist play handler**

Add the handler function (alongside `playTidalAlbum` etc.):

```typescript
import { playTidalPlaylist } from '$lib/stores/player';
```

(It was added in Task 5 — just import it here.)

- [ ] **Step 8: Add playlists row markup**

Find the Albums section markup (around line 689). Add the playlists row immediately after it (before the Tracks section):

```svelte
{#if showPlaylists && (filteredPlaylists().local.length > 0 || filteredPlaylists().tidal.length > 0)}
  <section class="results-section">
    <h3 class="section-label">Playlists</h3>
    <div class="albums-row" use:wheelToHorizontal>

      {#each filteredPlaylists().local as playlist (playlist.id)}
        <a
          class="album-card in-library"
          href="/playlists"
          onclick={(e) => {
            // Navigate to playlists page — individual playlist deep-link if available
          }}
          oncontextmenu={(e) => { e.preventDefault(); }}
        >
          <div class="art-wrap">
            <div class="album-art fallback" style="background: {letterColor(playlist.name)}">
              <span>♫</span>
            </div>
            <span class="lib-badge" aria-label="In your library"></span>
          </div>
          <p class="album-title">{playlist.name}</p>
          <p class="album-artist">{playlist.is_smart ? 'Smart playlist' : 'Playlist'} · {playlist.track_count} tracks</p>
        </a>
      {/each}

      {#each filteredPlaylists().tidal as playlist (playlist.uuid)}
        <button
          class="album-card"
          onclick={() => void playTidalPlaylist(playlist.uuid)}
          type="button"
        >
          <div class="art-wrap">
            {#if playlist.square_image}
              <div
                class="album-art"
                style="background-image: url('https://resources.tidal.com/images/{playlist.square_image.replace(/-/g, '/')}/320x320.jpg')"
              ></div>
            {:else}
              <div class="album-art fallback" style="background: {letterColor(playlist.title)}">
                <span>♫</span>
              </div>
            {/if}
            <button
              class="art-play-overlay"
              onclick={(e) => { e.stopPropagation(); void playTidalPlaylist(playlist.uuid); }}
              aria-label="Play {playlist.title}"
            >▶</button>
          </div>
          <p class="album-title">{playlist.title}</p>
          <p class="album-artist">TIDAL · {playlist.number_of_tracks ?? '?'} tracks</p>
        </button>
      {/each}

    </div>
  </section>
{/if}
```

Note: `letterColor` is already used in the Albums row — reuse it. `wheelToHorizontal` is already used — reuse it. The `.album-card`, `.art-wrap`, `.album-art`, `.lib-badge`, `.art-play-overlay` classes are all already defined for the Albums row — no new CSS needed.

- [ ] **Step 9: Verify in browser**

Open the search page, type a query. Confirm:
- A "Playlists" row appears between Albums and Tracks when there are results
- Local playlists show with the red in-library badge
- TIDAL playlists show without the badge
- Clicking a TIDAL playlist starts playback
- "Playlists" mode pill filters to show only the playlists row
- The row doesn't appear when there are no results

- [ ] **Step 10: Commit**

```bash
git add frontend/src/routes/search/+page.svelte
git commit -m "feat(frontend): playlists row in search results with TIDAL playlist playback"
```

---

## Self-Review Notes

- `is_favorite` coercion: Rust side uses `i32 != 0` pattern (consistent with existing `is_smart`, `is_synced`)
- `add_tracks_to_playlist` deduplication is done in Rust, not SQL, because the PK is `(playlist_id, position)` not `(playlist_id, track_id)` — checking existing track IDs first is correct
- TIDAL `square_image` field contains hyphens in the UUID; replaced with slashes for the image URL (same pattern as TIDAL album artwork elsewhere in the codebase — verify this pattern)
- Smart playlists excluded from "Add to playlist" submenu (they evaluate rules dynamically, manual inserts would be silently dropped on next evaluation)
- `playTidalPlaylist` adds tracks to queue individually using existing TIDAL ephemeral helpers — if the TIDAL queue helpers batch or have rate limits, consider batching; verify with actual TIDAL track count
- Task 6 step 6: verify the context menu component renders `submenu` items — this is the most likely point of unexpected behavior if submenus haven't been exercised before
