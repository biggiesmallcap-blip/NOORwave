# Tidal Search Page + Non-Library Playback — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a full Tidal catalogue search page and enable ephemeral playback of non-library Tidal tracks throughout the app (artist page, Tidal album/artist preview pages).

**Architecture:** A new `ephemeral_tidal_track` slot in `AppState` stores a synthetic `Track` when a non-library Tidal track is playing; the state endpoint overlays it onto the snapshot so the frontend sees it as `current_track` without any DB writes. Three new backend endpoints (`/api/tidal/search`, `/api/tidal/play`, `/api/tidal/artists/{id}`) expose existing `TidalClient` methods. The frontend gains a `TidalPlayable` interface and mirrored play functions in the player store.

**Tech Stack:** Rust / Axum (backend), Svelte 5 / TypeScript (frontend), SQLite via rusqlite, existing `TidalClient` in `noor-server/src/services/tidal/client.rs`.

---

## File Map

### Created
- `frontend/src/routes/search/+page.svelte` — Tidal search page
- `frontend/src/routes/tidal/artists/[id]/+page.svelte` — Tidal artist profile page

### Modified
- `noor-server/src/main.rs` — add `ephemeral_tidal_track` field to `AppState`
- `noor-server/src/server/routes.rs` — 3 new endpoints + state overlay + radio extension
- `frontend/src/lib/api/client.ts` — new Tidal API methods + new types
- `frontend/src/lib/stores/player.ts` — `TidalPlayable` type + play functions
- `frontend/src/lib/player/track_menu.ts` — Tidal track menu builder
- `frontend/src/routes/+layout.svelte` — add Search nav item
- `frontend/src/routes/tidal/albums/[id]/+page.svelte` — play buttons + Play All
- `frontend/src/routes/artists/[id]/+page.svelte` — filter input + album play overlay

---

## Task 1: Backend — Expose `/api/tidal/search`

**Files:**
- Modify: `noor-server/src/server/routes.rs`

- [ ] **Step 1: Add the query params struct, response structs, and handler near the other Tidal routes (search for `tidal/sync` to find the right region)**

  In `noor-server/src/server/routes.rs`, add after the existing Tidal route handlers:

  ```rust
  #[derive(Deserialize)]
  struct TidalSearchParams {
      q: String,
      limit: Option<i32>,
  }

  // Response structs rename fields to match frontend conventions:
  // `id` → `tidal_id`, `duration` (seconds) → `duration_ms`
  #[derive(Serialize)]
  struct TidalSearchTrackResp {
      tidal_id: i64,
      title: String,
      duration_ms: i64,
      artist_id: i64,
      artist_name: String,
      album_title: String,
      artwork_url: String,
      audio_quality: String,
      stream_ready: bool,
  }

  #[derive(Serialize)]
  struct TidalSearchAlbumResp {
      tidal_id: i64,
      title: String,
      artist_name: String,
      artwork_url: String,
  }

  #[derive(Serialize)]
  struct TidalSearchArtistResp {
      tidal_id: i64,
      name: String,
      artwork_url: String,
  }

  async fn tidal_search(
      State(state): State<SharedState>,
      Query(params): Query<TidalSearchParams>,
  ) -> Result<Json<serde_json::Value>, AppError> {
      let state_guard = state.read().await;
      let tokens = state_guard
          .tidal_tokens
          .as_ref()
          .ok_or(AppError::TidalNotConnected)?;
      let client = services::tidal::client::TidalClient::new(
          state_guard.http_client.clone(),
          tokens.access_token.clone(),
          tokens.country_code.clone(),
      );
      let limit = params.limit.unwrap_or(20);
      let results = client.search_catalog(&params.q, limit).await?;

      // Map to frontend-friendly field names
      let tracks: Vec<TidalSearchTrackResp> = results.tracks.into_iter().map(|t| TidalSearchTrackResp {
          tidal_id: t.id,
          title: t.title,
          duration_ms: t.duration * 1000, // Tidal API returns seconds
          artist_id: t.artist_id,
          artist_name: t.artist_name,
          album_title: t.album_title,
          artwork_url: t.artwork_url,
          audio_quality: t.audio_quality,
          stream_ready: t.stream_ready,
      }).collect();
      let albums: Vec<TidalSearchAlbumResp> = results.albums.into_iter().map(|a| TidalSearchAlbumResp {
          tidal_id: a.id,
          title: a.title,
          artist_name: a.artist_name,
          artwork_url: a.artwork_url,
      }).collect();
      let artists: Vec<TidalSearchArtistResp> = results.artists.into_iter().map(|a| TidalSearchArtistResp {
          tidal_id: a.id,
          name: a.name,
          artwork_url: a.artwork_url,
      }).collect();

      Ok(Json(serde_json::json!({ "tracks": tracks, "albums": albums, "artists": artists })))
  }
  ```

  > **Note:** Verify the actual field names on `TidalSearchTrack`, `TidalSearchAlbum`, `TidalSearchArtist` in `client.rs` (around line 84-121) before using them. If the field name is `picture` instead of `artwork_url` on artists, adjust accordingly.

- [ ] **Step 2: Register the route in the router (find `.route("/api/tidal/status"` and add below it)**

  ```rust
  .route("/api/tidal/search", get(tidal_search))
  ```

- [ ] **Step 3: Verify compilation**

  ```bash
  cd noor-server && cargo check 2>&1 | tail -20
  ```

  Expected: no errors (warnings OK).

- [ ] **Step 4: Smoke-test with curl (server must be running)**

  ```bash
  curl -s -H "Authorization: Bearer $(cat ~/.noor_token 2>/dev/null || echo test)" \
    "http://localhost:7734/api/tidal/search?q=bicep&limit=5" | head -c 500
  ```

  Expected: JSON with `tracks`, `albums`, `artists` arrays.

- [ ] **Step 5: Commit**

  ```bash
  git add noor-server/src/server/routes.rs
  git commit -m "feat(backend): expose GET /api/tidal/search endpoint"
  ```

---

## Task 2: Backend — Tidal artist profile endpoint

**Files:**
- Modify: `noor-server/src/server/routes.rs`

The `TidalClient` already has `get_artist_top_tracks` and `get_artist_albums` — this task just wires them up.

- [ ] **Step 1: Add handler**

  ```rust
  async fn tidal_artist_profile(
      State(state): State<SharedState>,
      Path(tidal_artist_id): Path<i64>,
  ) -> Result<Json<serde_json::Value>, AppError> {
      let state_guard = state.read().await;
      let tokens = state_guard
          .tidal_tokens
          .as_ref()
          .ok_or(AppError::TidalNotConnected)?;
      let client = services::tidal::client::TidalClient::new(
          state_guard.http_client.clone(),
          tokens.access_token.clone(),
          tokens.country_code.clone(),
      );
      let (top_tracks_page, albums_page) = tokio::try_join!(
          client.get_artist_top_tracks(tidal_artist_id, 10, 0),
          client.get_artist_albums(tidal_artist_id, 50, 0, Some("ALBUMS")),
      )?;
      // Extract artist name from first top track as fallback (no dedicated artist name endpoint)
      let artist_name = top_tracks_page.items.first()
          .and_then(|t| t.artist_name.as_deref())
          .map(|s| s.to_string());

      Ok(Json(serde_json::json!({
          "artist_name": artist_name,
          "top_tracks": top_tracks_page.items,
          "albums": albums_page.items,
      })))
  }
  ```

- [ ] **Step 2: Register route**

  ```rust
  .route("/api/tidal/artists/:tidal_id", get(tidal_artist_profile))
  ```

- [ ] **Step 3: Verify compilation**

  ```bash
  cd noor-server && cargo check 2>&1 | tail -20
  ```

  Expected: no errors. If `TidalClient::new` signature doesn't match, check `client.rs` for the actual constructor and adjust.

- [ ] **Step 4: Commit**

  ```bash
  git add noor-server/src/server/routes.rs
  git commit -m "feat(backend): expose GET /api/tidal/artists/:id profile endpoint"
  ```

---

## Task 3: Backend — Ephemeral Tidal playback (`AppState` extension + `/api/tidal/play`)

**Files:**
- Modify: `noor-server/src/main.rs`
- Modify: `noor-server/src/server/routes.rs`

This is the core infrastructure task. We add an ephemeral track slot to `AppState`, overlay it in the state snapshot, and add the play endpoint.

- [ ] **Step 1: Add `ephemeral_tidal_track` to `AppState` in `main.rs`**

  Find the `AppState` struct definition (around line 32) and add one field:

  ```rust
  pub ephemeral_tidal_track: Option<db::models::Track>,
  ```

  Then find where `AppState` is constructed (the `AppState { ... }` literal) and add the field with value `None`:

  ```rust
  ephemeral_tidal_track: None,
  ```

- [ ] **Step 2: Overlay ephemeral track in state snapshot**

  In `routes.rs`, find the handler that calls `player::load_snapshot` and returns the playback state (search for `load_snapshot`). After the snapshot is loaded, add the overlay:

  ```rust
  // After: let mut snapshot = state_guard.db.with_conn(|conn| player::load_snapshot(conn))?;
  if let Some(ephemeral) = &state_guard.ephemeral_tidal_track {
      snapshot.state.current_track = Some(ephemeral.clone());
      snapshot.state.is_playing = true;
  }
  ```

  Also clear the ephemeral slot in any handler that calls `player::play_track_now` (search for `play_track_now` calls) — add after each one:

  ```rust
  state_guard.ephemeral_tidal_track = None;
  ```

- [ ] **Step 3: Add the play request struct and handler**

  ```rust
  #[derive(Deserialize)]
  struct PlayTidalRequest {
      tidal_track_id: i64,
      title: String,
      artist_name: Option<String>,
      album_title: Option<String>,
      artwork_url: Option<String>,
      duration_ms: Option<i64>,
  }

  async fn play_tidal_ephemeral(
      State(state): State<SharedState>,
      Json(body): Json<PlayTidalRequest>,
  ) -> Result<Json<serde_json::Value>, AppError> {
      let state_guard = state.read().await;
      let tokens = state_guard
          .tidal_tokens
          .as_ref()
          .ok_or(AppError::TidalNotConnected)?;

      // Build stream request and resolve URL
      let stream_req = services::tidal::stream::StreamRequest::new(
          body.tidal_track_id,
          "LOSSLESS",
      );
      let stream_info = services::tidal::stream::resolve_stream(
          &state_guard.http_client,
          &tokens.access_token,
          &stream_req,
      )
      .await?;

      // Build synthetic Track (negative id avoids any DB collision)
      let synthetic = db::models::Track {
          id: -body.tidal_track_id,
          title: body.title.clone(),
          artist_id: 0,
          artist_name: body.artist_name.clone(),
          album_id: None,
          album_title: body.album_title.clone(),
          disc_number: None,
          track_number: None,
          duration_ms: body.duration_ms,
          isrc: None,
          tidal_id: Some(body.tidal_track_id),
          ytmusic_id: None,
          soundcloud_id: None,
          best_quality: Some("LOSSLESS".to_string()),
          best_source: Some("tidal".to_string()),
          fidelity_score: 0,
          is_favorite: false,
          play_count: 0,
          last_played_at: None,
          date_added: None,
          source: "tidal_ephemeral".to_string(),
          artwork_url: body.artwork_url.clone(),
      };

      // Build and start playback job
      let crossfade_ms = {
          state_guard
              .db
              .with_conn(|conn| playback::player::current_crossfade_ms(conn))
              .unwrap_or(0)
      };
      let job = playback::player::build_playback_preparation(
          &synthetic,
          Some(&stream_info),
          crossfade_ms,
      );
      let runtime = playback::runtime::ensure_playback_runtime(&*state_guard).await?;
      runtime.play(job)?;

      // Store for state overlay and clear DB current_track_id
      drop(state_guard);
      let mut state_guard = state.write().await;
      state_guard.ephemeral_tidal_track = Some(synthetic);
      state_guard
          .db
          .with_conn(|conn| {
              conn.execute(
                  "UPDATE playback_state SET current_track_id = NULL, position_ms = 0, is_playing = 1 WHERE id = 1",
                  [],
              )
          })?;

      Ok(Json(serde_json::json!({ "ok": true })))
  }
  ```

  > **Note:** If `playback::player::current_crossfade_ms` doesn't exist as a standalone fn, look at how other endpoints get crossfade_ms from the DB (search `crossfade_ms` in routes.rs) and replicate that pattern. If `ensure_playback_runtime` doesn't exist without a Track argument, use `ensure_playback_runtime_for_track(&*state_guard, &synthetic)`.

- [ ] **Step 4: Register route**

  ```rust
  .route("/api/tidal/play", post(play_tidal_ephemeral))
  ```

- [ ] **Step 5: Verify compilation**

  ```bash
  cd noor-server && cargo check 2>&1 | tail -30
  ```

  Fix any field-name mismatches by checking the actual `Track` struct fields in `noor-server/src/db/mod.rs` or `models.rs`.

- [ ] **Step 6: Commit**

  ```bash
  git add noor-server/src/main.rs noor-server/src/server/routes.rs
  git commit -m "feat(backend): ephemeral Tidal playback via POST /api/tidal/play"
  ```

---

## Task 4: Backend — Extend radio for Tidal seed

**Files:**
- Modify: `noor-server/src/server/routes.rs`

The radio endpoint is `POST /api/discovery/radio`. It takes `RadioRequest { seed_track_id: i64, ... }`. We add an optional `seed_tidal_id` that looks up the track in the DB by its `tidal_id` column if no local `seed_track_id` is provided.

- [ ] **Step 1: Extend `RadioRequest` struct**

  Find `struct RadioRequest` in `routes.rs` and add the new field:

  ```rust
  seed_tidal_id: Option<i64>,
  ```

- [ ] **Step 2: Add resolution logic at the top of the radio handler**

  Find the `get_radio_tracks` handler. At the start, before it uses `payload.seed_track_id`, add:

  ```rust
  // Resolve tidal seed to local track ID if needed
  let seed_track_id = if payload.seed_track_id > 0 {
      payload.seed_track_id
  } else if let Some(tidal_id) = payload.seed_tidal_id {
      state_guard
          .db
          .with_conn(|conn| {
              conn.query_row(
                  "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
                  rusqlite::params![tidal_id],
                  |row| row.get::<_, i64>(0),
              )
              .optional()
          })?
          .ok_or(AppError::NotFound("No local track matches that Tidal ID".into()))?
  } else {
      return Err(AppError::BadRequest("seed_track_id or seed_tidal_id required".into()));
  };
  ```

  Then replace `payload.seed_track_id` usages in the handler with `seed_track_id`.

- [ ] **Step 3: Verify compilation**

  ```bash
  cd noor-server && cargo check 2>&1 | tail -20
  ```

- [ ] **Step 4: Commit**

  ```bash
  git add noor-server/src/server/routes.rs
  git commit -m "feat(backend): radio accepts optional seed_tidal_id for non-library tracks"
  ```

---

## Task 5: Frontend — New API client types and methods

**Files:**
- Modify: `frontend/src/lib/api/client.ts`

- [ ] **Step 1: Add new types near the existing Tidal types (around line 80)**

  ```typescript
  export interface TidalSearchTrack {
    tidal_id: number
    title: string
    duration_ms: number
    artist_id: number
    artist_name: string
    album_title: string
    artwork_url: string | null
    audio_quality: string
    stream_ready: boolean
  }

  export interface TidalSearchAlbum {
    tidal_id: number
    title: string
    artist_name: string
    artwork_url: string | null
  }

  export interface TidalSearchArtist {
    tidal_id: number
    name: string
    artwork_url: string | null
  }

  export interface TidalSearchResults {
    tracks: TidalSearchTrack[]
    albums: TidalSearchAlbum[]
    artists: TidalSearchArtist[]
  }

  export interface TidalArtistProfile {
    artist_name: string | null
    top_tracks: TidalDiscographyTrack[]
    albums: TidalDiscographyAlbum[]
  }

  /** Minimal shape accepted by all ephemeral Tidal play functions */
  export interface TidalPlayable {
    tidal_id: number
    title: string
    artist_name: string | null
    album_title: string | null
    artwork_url: string | null
    duration_ms: number | null
  }
  ```

- [ ] **Step 2: Add new methods to the `api` object**

  Find the `api` object (around line 657) and add these methods:

  ```typescript
  searchTidal(q: string, limit = 20): Promise<TidalSearchResults> {
    return fetchApi<TidalSearchResults>('/api/tidal/search', { q, limit: String(limit) })
  },

  playTidalTrack(track: TidalPlayable): Promise<void> {
    return fetchApi<void>('/api/tidal/play', undefined, {
      method: 'POST',
      body: JSON.stringify({
        tidal_track_id: track.tidal_id,
        title: track.title,
        artist_name: track.artist_name,
        album_title: track.album_title,
        artwork_url: track.artwork_url,
        duration_ms: track.duration_ms,
      }),
    })
  },

  getTidalArtistProfile(tidalArtistId: number): Promise<TidalArtistProfile> {
    return fetchApi<TidalArtistProfile>(`/api/tidal/artists/${tidalArtistId}`)
  },

  startSongRadioFromTidal(tidalId: number): Promise<PlaybackSnapshot> {
    return fetchApi<PlaybackSnapshot>('/api/discovery/radio', undefined, {
      method: 'POST',
      body: JSON.stringify({ seed_track_id: 0, seed_tidal_id: tidalId }),
    })
  },
  ```

  > **Note:** Check how other POST methods in the api object pass the body (some use `fetchApi` with options, some use a dedicated helper). Mirror the existing pattern exactly.

- [ ] **Step 3: Verify TypeScript**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -30
  ```

  Expected: no new errors.

- [ ] **Step 4: Commit**

  ```bash
  git add frontend/src/lib/api/client.ts
  git commit -m "feat(frontend): add Tidal search/play/artist API client methods"
  ```

---

## Task 6: Frontend — Player store Tidal play functions

**Files:**
- Modify: `frontend/src/lib/stores/player.ts`

- [ ] **Step 1: Import the new type at the top of the file**

  ```typescript
  import type { TidalPlayable } from '$lib/api/client'
  ```

  (Add alongside existing imports from `$lib/api/client`.)

- [ ] **Step 2: Add play functions after the existing `playTrackNow` function (~line 142)**

  ```typescript
  export async function playTidalTrackNow(track: TidalPlayable) {
    try {
      await api.playTidalTrack(track)
      // Optimistically set current track so UI updates immediately
      currentTrack.set({
        id: -track.tidal_id,
        title: track.title,
        artist_id: 0,
        artist_name: track.artist_name,
        album_id: null,
        album_title: track.album_title,
        disc_number: null,
        track_number: null,
        duration_ms: track.duration_ms,
        isrc: null,
        tidal_id: track.tidal_id,
        best_quality: 'LOSSLESS',
        best_source: 'tidal',
        fidelity_score: 0,
        is_favorite: false,
        play_count: 0,
        last_played_at: null,
        date_added: null,
        source: 'tidal_ephemeral',
        artwork_url: track.artwork_url,
      })
      isPlaying.set(true)
      playerError.set(null)
    } catch (error) {
      playerError.set(`Tidal playback failed: ${error}`)
    }
  }

  export async function playTidalTrackNext(track: TidalPlayable) {
    // Queue the track then move it to next position — best-effort, no queue UI for ephemeral
    playerError.set(null)
    // For now: ephemeral tracks can only play-now or be added to queue
    await playTidalTrackNow(track)
  }

  export async function addTidalTrackToQueue(track: TidalPlayable) {
    // Play-now is the only ephemeral queue action in v1
    await playTidalTrackNow(track)
  }

  export async function playTidalAlbum(tidalAlbumId: number) {
    try {
      const { tracks } = await api.getTidalAlbumTracks(tidalAlbumId)
      if (tracks.length === 0) return
      await playTidalTrackNow(tracks[0])
      playerError.set(null)
    } catch (error) {
      playerError.set(`Tidal album playback failed: ${error}`)
    }
  }

  export async function startTidalSongRadio(track: TidalPlayable) {
    try {
      const snapshot = await api.startSongRadioFromTidal(track.tidal_id)
      hydratePlayback(snapshot)
      playerError.set(null)
    } catch (error) {
      playerError.set(`Tidal radio failed: ${error}`)
    }
  }
  ```

  > **Note on `playTidalTrackNext` and `addTidalTrackToQueue`:** True queue insertion for ephemeral tracks requires backend queue support that doesn't exist yet. These functions fall back to play-now for v1 — the menu items will be present but act as play-now. Add a TODO comment noting this limitation.

  > **Note on `api.getTidalAlbumTracks`:** This should already exist as `api.getTidalAlbumTracks(id)` in `client.ts`. If not, find the equivalent function that fetches tracks for a Tidal album by Tidal ID and use that name.

  > **Note on `api.startSongRadioFromTidal`:** Add this to `client.ts` in Task 5 if missed — it calls `POST /api/discovery/radio` with `{ seed_tidal_id: tidalId, seed_track_id: 0 }`.

- [ ] **Step 3: Verify TypeScript**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -30
  ```

- [ ] **Step 4: Commit**

  ```bash
  git add frontend/src/lib/stores/player.ts
  git commit -m "feat(frontend): add Tidal ephemeral play functions to player store"
  ```

---

## Task 7: Frontend — Tidal track context menu

**Files:**
- Modify: `frontend/src/lib/player/track_menu.ts`

- [ ] **Step 1: Import new player functions at the top of `track_menu.ts`**

  ```typescript
  import {
    playTidalTrackNow,
    playTidalTrackNext,
    addTidalTrackToQueue,
    startTidalSongRadio,
  } from '$lib/stores/player'
  import type { TidalPlayable } from '$lib/api/client'
  ```

- [ ] **Step 2: Add the Tidal menu builder function after `buildTrackMenu`**

  ```typescript
  export function buildTidalTrackMenu(track: TidalPlayable): MenuItem[] {
    return [
      {
        label: 'Play now',
        icon: '▶',
        onSelect: () => playTidalTrackNow(track),
      },
      {
        label: 'Play next',
        icon: '⏭',
        onSelect: () => playTidalTrackNext(track),
      },
      {
        label: 'Add to queue',
        icon: '+',
        onSelect: () => addTidalTrackToQueue(track),
      },
      { separator: true },
      {
        label: 'Song radio',
        icon: '◎',
        onSelect: () => startTidalSongRadio(track),
      },
      {
        label: 'Automix from here',
        icon: '⟁',
        // Automix uses the same radio seed path for non-library tracks in v1
        onSelect: () => startTidalSongRadio(track),
      },
    ]
  }
  ```

- [ ] **Step 3: Verify TypeScript**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -30
  ```

- [ ] **Step 4: Commit**

  ```bash
  git add frontend/src/lib/player/track_menu.ts
  git commit -m "feat(frontend): add buildTidalTrackMenu for non-library track actions"
  ```

---

## Task 8: Frontend — Add Search to nav sidebar

**Files:**
- Modify: `frontend/src/routes/+layout.svelte`

- [ ] **Step 1: Add Search to the Atlas nav zone**

  Find the `navZones` constant (around line 108). In the `Atlas` zone's `items` array, add the Search entry after Library:

  ```typescript
  { path: '/search', label: 'Search', icon: '🔍' },
  ```

  (Place it after `{ path: '/library', label: 'Library', icon: '♫' }`.)

- [ ] **Step 2: Verify TypeScript**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -20
  ```

- [ ] **Step 3: Commit**

  ```bash
  git add frontend/src/routes/+layout.svelte
  git commit -m "feat(frontend): add Search link to sidebar nav"
  ```

---

## Task 9: Frontend — Tidal search page

**Files:**
- Create: `frontend/src/routes/search/+page.svelte`

- [ ] **Step 1: Create the page file**

  ```svelte
  <script lang="ts">
    import { api, type TidalSearchResults, type TidalSearchTrack, type TidalSearchAlbum, type TidalSearchArtist } from '$lib/api/client'
    import { buildTidalTrackMenu } from '$lib/player/track_menu'
    import { openContextMenu } from '$lib/stores/context_menu'
    import { playTidalTrackNow } from '$lib/stores/player'
    import { formatDuration } from '$lib/utils'

    let query = $state('')
    let results = $state<TidalSearchResults | null>(null)
    let loading = $state(false)
    let error = $state<string | null>(null)
    let debounceTimer: ReturnType<typeof setTimeout>

    function onInput() {
      clearTimeout(debounceTimer)
      if (!query.trim()) {
        results = null
        return
      }
      loading = true
      debounceTimer = setTimeout(async () => {
        try {
          results = await api.searchTidal(query.trim())
          error = null
        } catch (e) {
          error = String(e)
        } finally {
          loading = false
        }
      }, 300)
    }

    const isEmpty = $derived(
      results && results.tracks.length === 0 && results.albums.length === 0 && results.artists.length === 0
    )
  </script>

  <div class="search-page">
    <div class="search-header">
      <input
        class="search-input"
        type="text"
        placeholder="Search Tidal's full catalogue"
        bind:value={query}
        oninput={onInput}
        autofocus
      />
    </div>

    {#if !query.trim()}
      <p class="search-hint">Start typing to search Tidal's full catalogue</p>
    {:else if loading}
      <p class="search-hint">Searching…</p>
    {:else if error}
      <p class="search-hint search-error">{error}</p>
    {:else if isEmpty}
      <p class="search-hint">No results for "{query}"</p>
    {:else if results}

      {#if results.artists.length > 0}
        <section class="results-section">
          <h3 class="section-label">Artists</h3>
          <div class="artists-row">
            {#each results.artists as artist (artist.tidal_id)}
              <a
                class="artist-card"
                href={`/tidal/artists/${artist.tidal_id}`}
              >
                <div
                  class="artist-avatar"
                  style={artist.artwork_url ? `background-image: url('${artist.artwork_url}')` : ''}
                ></div>
                <span class="artist-name">{artist.name}</span>
              </a>
            {/each}
          </div>
        </section>
      {/if}

      {#if results.albums.length > 0}
        <section class="results-section">
          <h3 class="section-label">Albums</h3>
          <div class="albums-row">
            {#each results.albums as album (album.tidal_id)}
              <a class="album-card" href={`/tidal/albums/${album.tidal_id}`}>
                <div
                  class="album-art"
                  style={album.artwork_url ? `background-image: url('${album.artwork_url}')` : ''}
                ></div>
                <p class="album-title">{album.title}</p>
                <p class="album-artist">{album.artist_name}</p>
              </a>
            {/each}
          </div>
        </section>
      {/if}

      {#if results.tracks.length > 0}
        <section class="results-section">
          <h3 class="section-label">Tracks</h3>
          <ul class="tracks-list">
            {#each results.tracks as track (track.tidal_id)}
              <li
                class="track-row"
                ondblclick={() => playTidalTrackNow(track)}
                oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTidalTrackMenu(track)) }}
                role="row"
              >
                <div
                  class="track-art"
                  style={track.artwork_url ? `background-image: url('${track.artwork_url}')` : ''}
                ></div>
                <div class="track-meta">
                  <p class="track-title">{track.title}</p>
                  <p class="track-artist">{track.artist_name} — {track.album_title}</p>
                </div>
                <span class="track-duration">{formatDuration(track.duration_ms)}</span>
                <button
                  class="play-btn"
                  onclick={() => playTidalTrackNow(track)}
                  aria-label="Play {track.title}"
                >▶</button>
                <button
                  class="menu-btn"
                  onclick={(e) => openContextMenu(e, buildTidalTrackMenu(track))}
                  aria-label="More options"
                >⋯</button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}

    {/if}
  </div>

  <style>
    .search-page {
      padding: 32px 40px;
      max-width: 900px;
    }
    .search-header {
      margin-bottom: 32px;
    }
    .search-input {
      width: 100%;
      max-width: 560px;
      background: var(--surface-2, #1a1a1a);
      border: 1px solid var(--border, #2a2a2a);
      border-radius: 24px;
      padding: 10px 20px;
      font-size: 15px;
      color: var(--text-primary, #fff);
      outline: none;
    }
    .search-input:focus {
      border-color: var(--accent, #7b2ff7);
    }
    .search-hint {
      color: var(--text-muted, #555);
      font-size: 14px;
      margin-top: 48px;
      text-align: center;
    }
    .search-error { color: var(--danger, #e74c3c); }
    .section-label {
      font-size: 11px;
      text-transform: uppercase;
      letter-spacing: 1px;
      color: var(--accent, #7b2ff7);
      margin-bottom: 12px;
    }
    .results-section { margin-bottom: 36px; }
    /* Artists */
    .artists-row {
      display: flex;
      gap: 20px;
      overflow-x: auto;
      padding-bottom: 8px;
    }
    .artist-card {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 8px;
      text-decoration: none;
      flex-shrink: 0;
      width: 80px;
    }
    .artist-avatar {
      width: 72px;
      height: 72px;
      border-radius: 50%;
      background: var(--surface-2, #222);
      background-size: cover;
      background-position: center;
    }
    .artist-name {
      font-size: 11px;
      color: var(--text-secondary, #aaa);
      text-align: center;
      width: 80px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    /* Albums */
    .albums-row {
      display: flex;
      gap: 16px;
      overflow-x: auto;
      padding-bottom: 8px;
    }
    .album-card {
      text-decoration: none;
      flex-shrink: 0;
      width: 120px;
    }
    .album-art {
      width: 120px;
      height: 120px;
      border-radius: 6px;
      background: var(--surface-2, #222);
      background-size: cover;
      background-position: center;
      margin-bottom: 6px;
    }
    .album-title {
      font-size: 12px;
      color: var(--text-primary, #fff);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .album-artist {
      font-size: 11px;
      color: var(--text-muted, #666);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    /* Tracks */
    .tracks-list {
      list-style: none;
      padding: 0;
      margin: 0;
    }
    .track-row {
      display: grid;
      grid-template-columns: 40px 1fr auto 32px 32px;
      align-items: center;
      gap: 12px;
      padding: 8px 4px;
      border-radius: 6px;
      cursor: pointer;
    }
    .track-row:hover { background: var(--surface-hover, #1a1a1a); }
    .track-art {
      width: 36px;
      height: 36px;
      border-radius: 4px;
      background: var(--surface-2, #222);
      background-size: cover;
      background-position: center;
      flex-shrink: 0;
    }
    .track-title {
      font-size: 13px;
      color: var(--text-primary, #fff);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .track-artist {
      font-size: 11px;
      color: var(--text-muted, #666);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .track-duration {
      font-size: 12px;
      color: var(--text-muted, #666);
      white-space: nowrap;
    }
    .play-btn, .menu-btn {
      background: none;
      border: none;
      color: var(--text-muted, #666);
      cursor: pointer;
      font-size: 14px;
      padding: 4px;
      border-radius: 4px;
      opacity: 0;
    }
    .track-row:hover .play-btn,
    .track-row:hover .menu-btn { opacity: 1; }
    .play-btn:hover, .menu-btn:hover { color: var(--text-primary, #fff); }
  </style>
  ```

  > **Note:** Check existing pages for the exact CSS variable names used (e.g., `--surface-2`, `--accent`, `--text-muted`). Open `frontend/src/app.css` or any existing page's `<style>` and adjust to match.

  > **Note on `formatDuration`:** Check if this utility exists in `$lib/utils` or is named differently. If `TidalSearchTrack.duration_ms` is in milliseconds, pass it directly. If the backend returns seconds instead, multiply by 1000 before passing.

- [ ] **Step 2: Verify TypeScript**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -30
  ```

- [ ] **Step 3: Build check**

  ```bash
  cd frontend && npm run build 2>&1 | tail -20
  ```

  Expected: build succeeds with no errors.

- [ ] **Step 4: Commit**

  ```bash
  git add frontend/src/routes/search/+page.svelte
  git commit -m "feat(frontend): add /search Tidal catalogue search page"
  ```

---

## Task 10: Frontend — Tidal artist profile page

**Files:**
- Create: `frontend/src/routes/tidal/artists/[id]/+page.svelte`

- [ ] **Step 1: Create the page**

  ```svelte
  <script lang="ts">
    import { page } from '$app/stores'
    import { api, type TidalArtistProfile, type TidalDiscographyTrack, type TidalDiscographyAlbum } from '$lib/api/client'
    import { buildTidalTrackMenu } from '$lib/player/track_menu'
    import { openContextMenu } from '$lib/stores/context_menu'
    import { playTidalTrackNow } from '$lib/stores/player'
    import { formatDuration } from '$lib/utils'

    let tidalArtistId = $derived(Number($page.params.id))
    let profile = $state<TidalArtistProfile | null>(null)
    let loading = $state(true)
    let filterQuery = $state('')

    $effect(() => {
      loading = true
      api.getTidalArtistProfile(tidalArtistId)
        .then((p) => { profile = p })
        .finally(() => { loading = false })
    })

    const filteredTracks = $derived(
      profile?.top_tracks.filter((t) =>
        filterQuery
          ? t.title.toLowerCase().includes(filterQuery.toLowerCase()) ||
            (t.artist_name ?? '').toLowerCase().includes(filterQuery.toLowerCase())
          : true
      ) ?? []
    )

    const filteredAlbums = $derived(
      profile?.albums.filter((a) =>
        filterQuery ? a.title.toLowerCase().includes(filterQuery.toLowerCase()) : true
      ) ?? []
    )
  </script>

  {#if loading}
    <div class="loading">Loading…</div>
  {:else if profile}
    <div class="artist-page">
      <div class="artist-hero">
        <h1 class="artist-name">Tidal Artist</h1>
      </div>

      <div class="filter-bar">
        <input
          class="filter-input"
          type="text"
          placeholder="Filter tracks and albums…"
          bind:value={filterQuery}
        />
      </div>

      {#if filteredTracks.length > 0}
        <section class="section">
          <h3 class="section-label">Top Tracks</h3>
          <ul class="tracks-list">
            {#each filteredTracks as track (track.tidal_id)}
              <li
                class="pop-row"
                ondblclick={() => playTidalTrackNow(track)}
                oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTidalTrackMenu(track)) }}
                role="row"
              >
                <div
                  class="track-art"
                  style={track.artwork_url ? `background-image: url('${track.artwork_url}')` : ''}
                ></div>
                <div class="track-meta">
                  <p class="track-title">{track.title}</p>
                  {#if track.album_title}
                    <p class="track-album">{track.album_title}</p>
                  {/if}
                </div>
                <span class="track-duration">{formatDuration(track.duration_ms)}</span>
                <button
                  class="play-btn"
                  onclick={() => playTidalTrackNow(track)}
                  aria-label="Play {track.title}"
                >▶</button>
                <button
                  class="menu-btn"
                  onclick={(e) => openContextMenu(e, buildTidalTrackMenu(track))}
                  aria-label="More options"
                >⋯</button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if filteredAlbums.length > 0}
        <section class="section">
          <h3 class="section-label">Albums</h3>
          <div class="albums-grid">
            {#each filteredAlbums as album (album.tidal_id)}
              <a class="grid-card" href={`/tidal/albums/${album.tidal_id}`}>
                <div
                  class="grid-art"
                  style={album.artwork_url ? `background-image: url('${album.artwork_url}')` : ''}
                ></div>
                <p class="grid-title">{album.title}</p>
                <p class="grid-sub">{album.artist_name ?? ''}</p>
              </a>
            {/each}
          </div>
        </section>
      {/if}
    </div>
  {/if}

  <style>
    .loading { padding: 48px; color: var(--text-muted, #666); }
    .artist-page { padding: 32px 40px; }
    .artist-hero { margin-bottom: 24px; }
    .artist-name { font-size: 32px; font-weight: 700; }
    .filter-bar { margin-bottom: 28px; }
    .filter-input {
      background: var(--surface-2, #1a1a1a);
      border: 1px solid var(--border, #2a2a2a);
      border-radius: 20px;
      padding: 7px 16px;
      font-size: 13px;
      color: var(--text-primary, #fff);
      outline: none;
      width: 280px;
    }
    .filter-input:focus { border-color: var(--accent, #7b2ff7); }
    .section { margin-bottom: 36px; }
    .section-label {
      font-size: 11px;
      text-transform: uppercase;
      letter-spacing: 1px;
      color: var(--accent, #7b2ff7);
      margin-bottom: 12px;
    }
    .tracks-list { list-style: none; padding: 0; margin: 0; }
    .pop-row {
      display: grid;
      grid-template-columns: 40px 1fr auto 32px 32px;
      align-items: center;
      gap: 12px;
      padding: 8px 4px;
      border-radius: 6px;
      cursor: pointer;
    }
    .pop-row:hover { background: var(--surface-hover, #1a1a1a); }
    .track-art {
      width: 36px; height: 36px;
      border-radius: 4px;
      background: var(--surface-2, #222);
      background-size: cover; background-position: center;
    }
    .track-title { font-size: 13px; color: var(--text-primary, #fff); }
    .track-album { font-size: 11px; color: var(--text-muted, #666); }
    .track-duration { font-size: 12px; color: var(--text-muted, #666); }
    .play-btn, .menu-btn {
      background: none; border: none;
      color: var(--text-muted, #666); cursor: pointer;
      font-size: 14px; padding: 4px;
      border-radius: 4px; opacity: 0;
    }
    .pop-row:hover .play-btn,
    .pop-row:hover .menu-btn { opacity: 1; }
    .albums-grid {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
      gap: 16px;
    }
    .grid-card { text-decoration: none; }
    .grid-art {
      width: 100%; aspect-ratio: 1;
      border-radius: 6px;
      background: var(--surface-2, #222);
      background-size: cover; background-position: center;
      margin-bottom: 6px;
    }
    .grid-title {
      font-size: 12px; color: var(--text-primary, #fff);
      overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    }
    .grid-sub {
      font-size: 11px; color: var(--text-muted, #666);
      overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    }
  </style>
  ```

  > **Note on artist name:** The backend `GET /api/tidal/artists/{id}` response in Task 2 returns `{ top_tracks, albums }` — it does NOT currently include a top-level artist name. Fix this in Task 2: extend the `tidal_artist_profile` handler to also call `client.get_artist(tidal_artist_id)` (if that method exists) or extract the artist name from `top_tracks[0].artist_name` as a fallback. Update `TidalArtistProfile` in `client.ts` to include `artist_name: string | null` and bind it in the hero: `<h1 class="artist-name">{profile.artist_name ?? 'Artist'}</h1>`.

  > **Note:** `TidalArtistProfile` uses `TidalDiscographyTrack` and `TidalDiscographyAlbum`. Verify these types have `tidal_id`, `title`, `artwork_url` fields. If the backend returns different field names, update the types in `client.ts` (Task 5) to match.

- [ ] **Step 2: TypeScript + build check**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -30 && npm run build 2>&1 | tail -10
  ```

- [ ] **Step 3: Commit**

  ```bash
  git add frontend/src/routes/tidal/artists/
  git commit -m "feat(frontend): add /tidal/artists/[id] Tidal artist profile page"
  ```

---

## Task 11: Frontend — Tidal album preview play buttons

**Files:**
- Modify: `frontend/src/routes/tidal/albums/[id]/+page.svelte`

- [ ] **Step 1: Add imports at the top of the `<script>` block**

  ```typescript
  import { buildTidalTrackMenu } from '$lib/player/track_menu'
  import { openContextMenu } from '$lib/stores/context_menu'
  import { playTidalTrackNow, playTidalAlbum } from '$lib/stores/player'
  ```

- [ ] **Step 2: Convert the "not in library" notice to a soft badge**

  Find the existing notice element (the box that says "Not in your library yet. Add this album...") and replace it with a soft inline badge:

  ```svelte
  <span class="not-in-library-badge">Not in your library</span>
  ```

  Add to the page's `<style>`:

  ```css
  .not-in-library-badge {
    font-size: 11px;
    color: var(--text-muted, #666);
    background: var(--surface-2, #1a1a1a);
    border: 1px solid var(--border, #2a2a2a);
    border-radius: 12px;
    padding: 3px 10px;
    display: inline-block;
    margin-bottom: 16px;
  }
  ```

- [ ] **Step 3: Add Play All button to the album header**

  Find the album header section (near the title/artist display) and add:

  ```svelte
  <button
    class="play-all-btn"
    onclick={() => playTidalAlbum(tidalAlbumId)}
  >
    ▶ Play All
  </button>
  ```

  Add to `<style>`:

  ```css
  .play-all-btn {
    background: var(--accent, #7b2ff7);
    color: white;
    border: none;
    border-radius: 20px;
    padding: 8px 20px;
    font-size: 13px;
    cursor: pointer;
    margin-top: 16px;
  }
  ```

- [ ] **Step 4: Add play button and context menu to each track row**

  Find the `{#each tracks as track}` block. The current track row is:

  ```svelte
  <li class="track-row">
    <span class="track-index">{track.track_number ?? idx + 1}</span>
    <div class="track-meta">
      <p class="track-title">{track.title}</p>
      <span class="track-artist">{track.artist_name}</span>
    </div>
    <span class="track-duration">{formatDuration(track.duration_ms)}</span>
  </li>
  ```

  Replace with:

  ```svelte
  <li
    class="track-row"
    ondblclick={() => playTidalTrackNow(track)}
    oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTidalTrackMenu(track)) }}
    role="row"
  >
    <span class="track-index">{track.track_number ?? idx + 1}</span>
    <div class="track-meta">
      <p class="track-title">{track.title}</p>
      <span class="track-artist">{track.artist_name}</span>
    </div>
    <span class="track-duration">{formatDuration(track.duration_ms)}</span>
    <button
      class="row-play-btn"
      onclick={() => playTidalTrackNow(track)}
      aria-label="Play {track.title}"
    >▶</button>
    <button
      class="row-menu-btn"
      onclick={(e) => openContextMenu(e, buildTidalTrackMenu(track))}
      aria-label="More options"
    >⋯</button>
  </li>
  ```

  Update the track-row grid in `<style>` from `40px 1fr 64px` to `40px 1fr 64px 32px 32px` and add:

  ```css
  .row-play-btn, .row-menu-btn {
    background: none; border: none;
    color: var(--text-muted, #666); cursor: pointer;
    font-size: 13px; padding: 4px;
    opacity: 0; border-radius: 4px;
  }
  .track-row:hover .row-play-btn,
  .track-row:hover .row-menu-btn { opacity: 1; }
  ```

- [ ] **Step 5: TypeScript + build check**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -30 && npm run build 2>&1 | tail -10
  ```

- [ ] **Step 6: Commit**

  ```bash
  git add frontend/src/routes/tidal/albums/
  git commit -m "feat(frontend): add play buttons and Play All to Tidal album preview"
  ```

---

## Task 12: Frontend — Artist page filter + non-library album play overlay

**Files:**
- Modify: `frontend/src/routes/artists/[id]/+page.svelte`

- [ ] **Step 1: Add imports**

  At the top of the `<script>` block, add:

  ```typescript
  import { playTidalAlbum } from '$lib/stores/player'
  ```

- [ ] **Step 2: Add filter state and derived filtered data**

  After the existing state declarations, add:

  ```typescript
  let filterQuery = $state('')

  const filteredPopTracks = $derived(
    popTracks.filter((t: Track) =>
      filterQuery
        ? t.title.toLowerCase().includes(filterQuery.toLowerCase()) ||
          (t.artist_name ?? '').toLowerCase().includes(filterQuery.toLowerCase())
        : true
    )
  )

  const filteredTidalFullAlbums = $derived(
    tidalFullAlbums.filter((a) =>
      filterQuery ? a.title.toLowerCase().includes(filterQuery.toLowerCase()) : true
    )
  )

  const filteredTidalSinglesEPs = $derived(
    tidalSinglesEPs.filter((a) =>
      filterQuery ? a.title.toLowerCase().includes(filterQuery.toLowerCase()) : true
    )
  )
  ```

  > **Note:** Check the actual variable names for the popular tracks array and the Tidal album arrays in this page (`popTracks`, `tidalFullAlbums`, `tidalSinglesEPs`). Adjust the derived names to match what's already used in the template.

- [ ] **Step 3: Add filter input to the template below the hero action buttons**

  Find the hero action buttons section (the Play / Shuffle / Radio buttons) and add below them:

  ```svelte
  <div class="filter-bar">
    <input
      class="filter-input"
      type="text"
      placeholder="Filter tracks and albums…"
      bind:value={filterQuery}
    />
  </div>
  ```

  Add to `<style>`:

  ```css
  .filter-bar { margin: 16px 0 8px; }
  .filter-input {
    background: var(--surface-2, #1a1a1a);
    border: 1px solid var(--border, #2a2a2a);
    border-radius: 20px;
    padding: 7px 16px;
    font-size: 13px;
    color: var(--text-primary, #fff);
    outline: none;
    width: 260px;
  }
  .filter-input:focus { border-color: var(--accent, #7b2ff7); }
  ```

- [ ] **Step 4: Wire filtered arrays to the template**

  In the template, replace the `popTracks` array reference in the `{#each}` loop with `filteredPopTracks`. Replace `tidalFullAlbums` with `filteredTidalFullAlbums` and `tidalSinglesEPs` with `filteredTidalSinglesEPs`.

- [ ] **Step 5: Add play overlay to non-library Tidal album cards**

  Find the album card template for non-library albums (look for `not-in-library` class or the condition where `!album.in_library`). Add a play button overlay inside the card:

  ```svelte
  {#if !album.in_library}
    <button
      class="album-play-overlay"
      onclick|stopPropagation={() => playTidalAlbum(album.tidal_id)}
      aria-label="Play {album.title}"
    >▶</button>
  {/if}
  ```

  The card container needs `position: relative`. Add to `<style>`:

  ```css
  .grid-card { position: relative; }
  .album-play-overlay {
    position: absolute;
    bottom: 36px; /* above the title text */
    right: 8px;
    background: rgba(0,0,0,0.7);
    color: white;
    border: none;
    border-radius: 50%;
    width: 32px;
    height: 32px;
    font-size: 12px;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .grid-card:hover .album-play-overlay { opacity: 1; }
  ```

- [ ] **Step 6: TypeScript + build check**

  ```bash
  cd frontend && npx tsc --noEmit 2>&1 | head -30 && npm run build 2>&1 | tail -10
  ```

- [ ] **Step 7: Commit**

  ```bash
  git add frontend/src/routes/artists/
  git commit -m "feat(frontend): artist page filter input + Tidal album play overlay"
  ```

---

## Verification

1. **Search page:** Navigate to `/search`, type "Bicep". Confirm artists, albums, tracks appear. Double-click a track — confirm it plays without a library entry appearing. Right-click a track — confirm menu shows Play Now, Song Radio, etc.

2. **Search → non-library artist:** Click an artist card not in library — confirm navigation to `/tidal/artists/{id}` and the page loads top tracks + albums.

3. **Tidal artist page filter:** Type in the filter box — confirm tracks and albums narrow in real-time.

4. **Tidal album preview:** Open any `/tidal/albums/{id}`. Confirm the "Not in your library" prompt is now a small badge. Click Play All — confirm playback starts. Double-click a track row — confirm it plays.

5. **Artist page filter:** Open a library artist, type in the filter box — confirm Popular Tracks and Albums both narrow.

6. **Artist page Tidal album play:** Hover a non-library album card — confirm ▶ overlay appears. Click it — confirm the album starts playing without importing.

7. **Now-playing bar:** While a Tidal ephemeral track plays, confirm the bar shows the correct title, artist, and artwork.

8. **Radio from Tidal track:** Right-click a track in the search results → Song Radio — confirm a radio queue is generated (requires that the Tidal track's ID matches a local track via `tidal_id` column; if no match exists, an error is acceptable in v1).
