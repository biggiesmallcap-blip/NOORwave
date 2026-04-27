# Tidal Search Page + Non-Library Playback — Design Spec

**Date:** 2026-04-26

## Context

NOORwave has no dedicated search page — search only exists as an inline filter in the library page. Tidal tracks not in the user's library cannot be played at all; the `/tidal/albums/[id]` preview page shows track listings but has no play buttons. This spec covers two tightly coupled features: a full Tidal catalogue search page, and ephemeral playback of non-library Tidal content throughout the app (artist page, Tidal album/artist preview pages).

---

## Feature 1: Tidal Search Page (`/search`)

### Layout (NOORwave-native style)

**Idle state:** Centered search bar with hint text "Search Tidal's full catalogue". No genre tiles, no trending — clean slate only.

**Active state** (debounced ~300ms after keystroke): Three stacked sections, each hidden if it returns zero results:

1. **Artists** — horizontal scroll row of circular avatar cards. Badge overlay if artist is in library. Clicking navigates to `/artists/{local_id}` if in library, or `/tidal/artists/{tidal_id}` if not.
2. **Albums** — horizontal scroll row of square album art cards. Clicking navigates to `/tidal/albums/{tidal_id}`.
3. **Tracks** — vertical list using the app's standard `.track-row` style. Each row has a play button and a `⋯` context menu.

If all three sections are empty: shows "No results for «query»".

The sidebar nav gets a Search entry pointing to `/search`.

### Track context menu (Tidal external tracks)

Same options as local tracks: Play Now · Play Next · Add to Queue · Start Song Radio · Automix from here.

---

## Feature 2: Ephemeral Tidal Playback Infrastructure

### Principle

Non-library Tidal tracks are played directly by Tidal ID without writing to the library DB. The library is never modified.

### Backend — 3 new endpoints (`noor-server/src/server/routes.rs`)

**`GET /api/tidal/search?q=&limit=`**
Calls the existing `TidalClient::search_catalog()` (client.rs:326). Returns `{ tracks: TidalSearchTrack[], albums: TidalSearchAlbum[], artists: TidalSearchArtist[] }`. No new Rust logic — expose only.

**`POST /api/tidal/play` — body: `{ tidal_track_id: String, metadata: TidalTrackMeta }`**
Fetches stream URL from Tidal for the given ID, injects it into the gapless player (same path as `play_track_now` but bypasses the DB). Passes title, artist, artwork to the player for queue display. Returns `{ ok: true }`.

**`GET /api/tidal/artists/{tidal_id}`**
Calls Tidal API (reusing existing `parse_search_artist`, album/track parse helpers in client.rs) to fetch artist name + image + top tracks + albums. Returns `{ artist: TidalArtist, top_tracks: TidalSearchTrack[], albums: TidalSearchAlbum[] }`.

**Radio/automix seed extension**
The backend radio/automix endpoint gains a `seed_tidal_id` parameter alongside the existing `seed_track_id`, so radio can be seeded from a non-library Tidal track.

### Frontend — player store (`frontend/src/lib/stores/player.ts`)

New type:
```ts
interface TidalExternalTrack {
  tidalId: string
  title: string
  artistName: string
  albumTitle: string
  artworkUrl: string
  durationSecs: number
}
```

New exported functions (mirroring local-track equivalents):
- `playTidalTrackNow(meta: TidalExternalTrack)`
- `playTidalTrackNext(meta: TidalExternalTrack)`
- `addTidalTrackToQueue(meta: TidalExternalTrack)`
- `playTidalAlbum(tidalAlbumId: string)` — fetches tracks via existing `GET /api/tidal/albums/{id}/tracks`, then queues them all as ephemeral entries and starts playback

The queue store's item type is extended to hold either a local `Track` or a `TidalExternalTrack`. Queue panel and now-playing bar check which variant is present and render accordingly.

### Frontend — track menu (`frontend/src/lib/player/track_menu.ts`)

`buildTrackMenu` gains a Tidal external variant accepting a `TidalExternalTrack`. Produces: Play Now, Play Next, Add to Queue, Start Song Radio, Automix from here.

### Frontend — API client (`frontend/src/lib/api/client.ts`)

New methods: `searchTidal(q, limit)`, `playTidalTrack(meta)`, `getTidalArtist(tidalId)`.

---

## Feature 3: New Tidal Artist Page (`/tidal/artists/[id]`)

Mirrors `/artists/[id]` layout but sourced entirely from `GET /api/tidal/artists/{id}`:

- **Hero:** artist image + name
- **Filter input** (below hero) — client-side reactive filter narrowing top tracks and albums by name
- **Top Tracks** — `.pop-row` style list, each row has play/queue `⋯` menu with full options incl. radio + automix
- **Albums** — grid cards linking to `/tidal/albums/{tidal_id}`

No shuffle/artist-radio actions (those require library context).

---

## Feature 4: Updates to Existing Pages

### `/tidal/albums/[id]` (Tidal album preview)

- Keep the "Not in your library yet" text as a **soft badge** (subdued style, not a prominent notice box)
- Add a **Play All** button in the album header
- Add a play button + `⋯` menu to each track row (full Tidal queue options + radio + automix)

### `/artists/[id]` (library artist page)

- **Filter input** added below the hero action buttons — single text input that reactively filters both the Popular Tracks list and the Albums/Singles grid client-side (instant, no API call)
- Non-library Tidal album cards get a small **▶** play button overlay visible on hover, which calls `playTidalAlbum(tidalAlbumId)` — a helper that fetches album tracks via `GET /api/tidal/albums/{id}/tracks` (already exists) and queues them all ephemerally

---

## Verification

1. **Search page:** Navigate to `/search`, type "Bicep" — confirm artists, albums, tracks appear in stacked sections. Click a track's play button — confirm it plays without appearing in library. Right-click a track — confirm full menu including radio and automix.
2. **Non-library artist navigation:** From search, click an artist not in library — confirm navigates to `/tidal/artists/{id}` showing top tracks and albums.
3. **Tidal album preview:** Open `/tidal/albums/{id}` — confirm soft badge is present, Play All works, individual track play buttons work, `⋯` menu shows radio/automix.
4. **Artist page filter:** Open any library artist — type in filter box, confirm tracks and albums narrow in real-time.
5. **Artist page Tidal album play:** Hover a non-library album card on an artist page — confirm ▶ overlay appears and clicking it queues the album ephemerally.
6. **Queue display:** While a Tidal external track plays, confirm now-playing bar shows correct title, artist, and artwork.
7. **Radio from Tidal track:** Start song radio from a non-library track — confirm radio queue is generated.
