---
name: Library UIX Redesign — Search-Style Layout
description: Bring the Search page's visual language into the Library: filter pills, "All" home view with hero card and carousels, inline query syntax replacing the DSP filter panel
type: spec
---

# Library UIX Redesign — Search-Style Layout

## Goal

Transform the Library page to feel visually continuous with the Search page. Same pill navigation, same hero card pattern, same carousels, same inline query syntax for filtering. The dense Tracks/Albums/Artists views are preserved unchanged — only navigation and the new "All" home view are added.

---

## Section 1: Navigation

### Tab bar → Filter pills

Replace the existing `.tab-bar` with four pills styled exactly like Search's `.filter-pills`:

| Pill | View |
|------|------|
| All | New home view (hero + carousels + recent tracks) |
| Tracks | Existing dense track table — unchanged |
| Albums | Existing album grid — unchanged |
| Artists | Existing artist grid with expandable panels — unchanged |

**Behavior:**
- "All" is the default on first load (persisted to `sessionStorage` for scroll restore)
- Active pill: filled accent background, same transition as Search
- Clicking any pill updates `activeTab` state (same store as current)

### Search input + keyboard hints

The library search input stays in the toolbar. A keyboard hint row is added directly below it, matching Search's `kbd-hint` row exactly:

```
/ focus · ↑↓ move · Enter play · Shift+Enter queue · Ctrl+Enter next
bpm:138 · key:Am · energy:>0.7 · genre:dnb · instrumental:true
```

### DSP filter panel — removed

The existing `.dsp-filter-bar` (BPM sliders, energy range, key selector, instrumental toggle) is removed entirely. All those filters are expressed inline in the search input using the existing `query_parser.ts` syntax. No backend changes needed.

---

## Section 2: "All" Home View

The "All" pill reveals a new home layout with three stacked sections.

### Hero card — Top Artist

Styled after Search's `.top-result-card`. Data derived client-side from the already-loaded library store: artist with the highest total `play_count` across their tracks.

**Layout:**
- Left: artist photo, 168×168px, 50% border-radius (circular), same as Search `.top-art`
- Right column:
  - Eyebrow label: `ARTIST · IN YOUR LIBRARY` (same uppercase 10px tracking as Search `.top-kind`)
  - Artist name: clamped heading `clamp(28px, 4vw, 44px)` (same as Search `.top-title`)
  - Subtitle: `{trackCount} tracks · {albumCount} albums`
  - Buttons: Play All (accent-filled) + Shuffle (glass)
- Background: ambient color glow extracted from artist photo using same technique as Search page's blurred background treatment
- Full-width glass panel, 12px border-radius, same as Search `.top-result-card`

**Data source:** Client-side — `$derived` from library store tracks, grouped by artist, summed `play_count`.

### Carousel 1 — Recently Played Artists

Horizontally scrollable row of circular artist cards, styled exactly like Search's `.artists-row`:
- 84px wide cards, 72px circular avatar, artist name below truncated
- Lib badge (checkmark) on all since everything in Library is owned
- Derived from tracks sorted by `last_played_at DESC`, deduplicated by artist
- Show up to 20 artists

### Carousel 2 — Recently Added Albums

Horizontally scrollable row of square album cards, styled exactly like Search's `.albums-row`:
- 128×128px album art, 6px radius
- Album name + artist name below, truncated
- Hover: play overlay button (36px circle, accent bg)
- Sorted by `date_added DESC`
- Show up to 20 albums

### Existing `.library-hero` section

The current `.library-hero` glass panel (eyebrow, h1, mode pill, view-toggle) is removed. Its stats are absorbed into the new hero card subtitle. The view-toggle (grid/list) moves into the Albums pill view toolbar only, where it's relevant.

### Recent Tracks snippet

Last 10 played tracks (`last_played_at DESC`, fallback to `date_added DESC` for unplayed).

Uses Search's lighter track row format (not the dense table): `38px art | 1fr title+artist | auto duration | auto actions`. Same `.track-row` structure as Search results, same hover `.row-actions` buttons (play, queue, play-next, menu).

A "View all tracks →" link at the bottom switches to the Tracks pill.

---

## Section 3: Tracks / Albums / Artists Views

These are **unchanged** from current implementation:
- Tracks: dense table with all columns (BPM, Key, Energy, Dance, Quality, Plays, Duration)
- Albums: album grid with tile/list toggle
- Artists: artist grid with expandable panels and discography view

The only visual change: they now sit below the pills bar instead of the tab bar. No layout or data changes.

---

## Data / API

All "All" view data is derived client-side from the existing library store — no new API endpoints required:

| Data | Source |
|------|--------|
| Top artist (hero) | `$derived` — group tracks by artist, sum play_count |
| Recently played artists | `$derived` — tracks sorted by last_played_at, deduped by artist |
| Recently added albums | `$derived` — albums sorted by date_added |
| Recent tracks | `$derived` — tracks sorted by last_played_at |

---

## Files Changed

| File | Change |
|------|--------|
| `frontend/src/routes/library/+page.svelte` | Replace tab-bar with pills, remove DSP panel, add "All" view sections, add kbd-hint row |
| `frontend/src/lib/components/LibraryHero.svelte` | New — top artist hero card component |
| `frontend/src/lib/components/ArtistCarousel.svelte` | New — horizontal scrollable artist cards |
| `frontend/src/lib/components/AlbumCarousel.svelte` | New — horizontal scrollable album cards |

The carousel and hero components reuse CSS variables and class patterns from Search (`top-result-card`, `artists-row`, `albums-row`, `artist-card`, `album-art`) — no new design tokens.

---

## Success Criteria

- [ ] Filter pills render with Search-style aesthetics; "All" is default on load
- [ ] DSP filter bar gone; `bpm:138 key:Am` etc. work inline in the search input on Library page
- [ ] "All" view shows hero card for the most-played artist with play/shuffle buttons
- [ ] "All" view shows recently played artists carousel (horizontal scroll)
- [ ] "All" view shows recently added albums carousel (horizontal scroll)
- [ ] "All" view shows recent tracks snippet with "View all" link
- [ ] Tracks / Albums / Artists pills show existing dense views unchanged
- [ ] Hero ambient glow reflects artist photo dominant color
- [ ] Scroll position restores correctly after navigation (existing behavior preserved)
