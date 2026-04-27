# Playlist Integration Design

**Date:** 2026-04-28
**Status:** Approved

## Overview

Three related features that bring playlists to full parity with albums and artists across the app:

1. **Playlist actions** — Shuffle, Radio, and Save (heart/favorite) buttons on playlist cards
2. **Add to playlist** — Bulk-insert all tracks from an album or artist via right-click context menu
3. **Playlists in search** — Horizontal-scroll playlist row in search results (local library + TIDAL)

---

## Architecture

Four independent workstreams with no shared state between them:

| Workstream | Frontend | Backend |
|---|---|---|
| Playlist actions | New buttons on playlist card headers | New `is_favorite` field + PATCH endpoint |
| Add to playlist | New context menu items on album/artist cards | Reuses existing bulk-insert |
| Search playlists row | New row in search results, new mode pill | New TIDAL playlist search proxy endpoint |
| TIDAL playlist playback | New play handler for TIDAL playlist results | New endpoint to fetch TIDAL playlist tracks |

All changes follow existing patterns — no new UI components, no new state management patterns.

---

## Workstream 1: Playlist Card Actions

### UI

The playlist card header gains three new elements alongside the existing Play button:

- **Heart button** (♥) — toggles favorite state; filled/red when saved
- **Shuffle button** (⤮ Shuffle) — secondary style, same as album/artist pages
- **Radio button** (◉ Radio) — secondary style, same as album/artist pages

Order: `♥  Shuffle  Radio  ▶ Play  ›`

### Behavior

**Shuffle:** fetches playlist tracks, shuffles client-side, calls `loadQueueAndPlay`. Identical to existing `shufflePlaylist` flow.

**Radio:** picks the most-played track from the playlist (sort tracks by `play_count DESC`, take first), then calls `startSongRadio(seedTrackId)`. Reuses the existing `/api/radio/song` endpoint — no new backend needed.

**Heart/Favorite:** calls `PATCH /api/playlists/{id}/favorite`. Optimistic UI update — toggle immediately, revert on error. Favorited playlists sort to top of the playlist list (`is_favorite DESC, name ASC`).

### Backend

- DB migration: add `is_favorite BOOLEAN NOT NULL DEFAULT FALSE` to playlists table
- Endpoint: `PATCH /api/playlists/{id}/favorite` — toggles `is_favorite`, returns updated playlist object
- List endpoint: update ordering to `ORDER BY is_favorite DESC, name ASC`

---

## Workstream 2: Add to Playlist — Album/Artist Context Menus

### UI

New "Add to playlist ›" item at the bottom of the right-click context menu on album cards and artist cards (in the library carousels, library page, and search results). Appears after the "Go to…" navigation item, separated by a divider.

Clicking opens a submenu listing the user's playlists:
- Favorited playlists (♥) shown first, then alphabetical
- **Smart playlists excluded** — they are rules-based and cannot accept manual track inserts
- "New playlist…" option at the bottom — creates a blank playlist then immediately inserts the tracks

### Behavior

- **Album:** bulk-inserts all library tracks from the album into the selected playlist
- **Artist:** bulk-inserts all library tracks by the artist into the selected playlist
- Duplicate tracks silently skipped (existing backend deduplication behavior)
- Toast on success: `"Added {n} tracks to {playlist name}"`

### Backend

No new endpoints required. Reuses the existing bulk playlist track insert endpoint.

---

## Workstream 3: Playlists in Search

### UI

New **Playlists** horizontal-scroll row in search results, positioned between Albums and Tracks. Follows the same card pattern as the Albums row (80×80 → 130×130 artwork cards with title/subtitle below).

Card variants:
- **Library playlist** — red "IN LIB" badge top-right, clicking navigates to the playlist page
- **TIDAL playlist** — grey "TIDAL" badge top-right, clicking streams the playlist directly

Library playlists match by name (case-insensitive substring) and float to the top of the row, consistent with the library-boost pattern used for artists and albums.

The TIDAL playlist row only renders when TIDAL returns results — no empty section shown.

A new **"Playlists"** mode pill is added to the filter bar alongside All / Artists / Albums / Tracks / In Library.

### Backend

**TIDAL playlist search proxy:**
- `GET /api/tidal/playlists/search?q={query}`
- Proxies to TIDAL search API filtered to playlists
- Returns `[{ tidal_uuid, title, track_count, artwork_url }]`

**TIDAL playlist tracks:**
- `GET /api/tidal/playlists/{tidal_uuid}/tracks`
- Fetches track list from TIDAL
- Returns `TidalPlayable[]` — same shape already used for TIDAL album/track playback

### Playback

Clicking a TIDAL playlist result calls the tracks endpoint, then loads the returned `TidalPlayable[]` into the queue and begins playback. Uses the existing TIDAL ephemeral playback path (same as TIDAL search track results).

---

## Out of Scope

- TIDAL playlist search via audio filters / DSP params (search is name-match only)
- Saving/syncing a TIDAL search result playlist to the library (play-only)
- Playlist radio using multiple seed tracks or interleaved radio queues
- Bulk-insert from track context menus (already exists at track level)
