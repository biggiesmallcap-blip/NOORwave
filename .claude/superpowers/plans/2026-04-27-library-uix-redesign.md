# Library UIX Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Library tab bar with Search-style filter pills, add an "All" home view (top-artist hero card + carousels + recent tracks), and remove the DSP filter panel in favour of inline query syntax.

**Architecture:** All "All" view data is derived client-side from already-loaded library stores — no new API endpoints. Three new Svelte components (LibraryHero, ArtistCarousel, AlbumCarousel) are extracted so the main library page doesn't grow further. A shared `wheelToHorizontal` action is extracted from search/+page.svelte so both pages import it.

**Tech Stack:** SvelteKit 5 runes (`$state`, `$derived`, `$derived.by`), Svelte 5 component props, existing `$lib/stores/library` and `$lib/stores/player` stores, CSS variables matching Search page (`--accent`, `--bg-glass`, `--bg-glass-hover`).

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `frontend/src/lib/actions/wheel-to-horizontal.ts` | Shared wheel→horizontal scroll action |
| Modify | `frontend/src/routes/search/+page.svelte:352-360` | Import action instead of inline definition |
| Create | `frontend/src/lib/components/LibraryHero.svelte` | Top-artist hero card with play/shuffle |
| Create | `frontend/src/lib/components/ArtistCarousel.svelte` | Horizontal scrolling artist circles |
| Create | `frontend/src/lib/components/AlbumCarousel.svelte` | Horizontal scrolling album cards |
| Modify | `frontend/src/routes/library/+page.svelte` | Pills nav, remove DSP panel, All view, CSS |

---

## Task 1: Extract shared `wheelToHorizontal` action

**Files:**
- Create: `frontend/src/lib/actions/wheel-to-horizontal.ts`
- Modify: `frontend/src/routes/search/+page.svelte:352-360`

- [ ] **Step 1: Create the action file**

```typescript
// frontend/src/lib/actions/wheel-to-horizontal.ts
export function wheelToHorizontal(node: HTMLElement) {
  const onWheel = (e: WheelEvent) => {
    if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return
    e.preventDefault()
    node.scrollLeft += e.deltaY
  }
  node.addEventListener('wheel', onWheel, { passive: false })
  return { destroy: () => node.removeEventListener('wheel', onWheel) }
}
```

- [ ] **Step 2: Update search/+page.svelte to use the shared import**

In `frontend/src/routes/search/+page.svelte`, add this import near the top of the `<script>`:

```typescript
import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal'
```

Then delete the inline function definition at lines 351–360 (the `function wheelToHorizontal(node: HTMLElement) { ... }` block).

- [ ] **Step 3: Verify search page still compiles**

```bash
cd frontend && npm run check 2>&1 | grep -E "error|Error" | head -20
```

Expected: no TypeScript errors related to `wheelToHorizontal`.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/actions/wheel-to-horizontal.ts frontend/src/routes/search/+page.svelte
git commit -m "refactor(actions): extract wheelToHorizontal to shared lib/actions"
```

---

## Task 2: Replace library tab bar with filter pills

**Files:**
- Modify: `frontend/src/routes/library/+page.svelte`

- [ ] **Step 1: Update `activeTab` type to include `'all'` and change default**

Find this line near the top of `<script>` (around line 27):
```typescript
let activeTab = $state<'tracks' | 'albums' | 'artists'>('albums');
```

Replace with:
```typescript
let activeTab = $state<'all' | 'tracks' | 'albums' | 'artists'>('all');
```

- [ ] **Step 2: Update scroll-restore sessionStorage to handle `'all'`**

Find the `beforeNavigate` handler where `activeTab` is saved to sessionStorage. It saves something like:
```typescript
sessionStorage.setItem('noor-library-scroll', JSON.stringify({ scrollY: window.scrollY, activeTab, expandedArtistId }))
```

The restore side reads `activeTab` back. Verify the restore type-cast accepts `'all'`:
```typescript
// When restoring, ensure the cast includes 'all':
const tab = saved.activeTab as 'all' | 'tracks' | 'albums' | 'artists'
activeTab = (['all', 'tracks', 'albums', 'artists'] as const).includes(tab) ? tab : 'all'
```

- [ ] **Step 3: Add `loadArtists()` to `onMount`**

Find `onMount(() => {` and add `void loadArtists()` alongside the existing loads:
```typescript
onMount(() => {
  void loadAlbums();
  void loadTracks();
  void loadArtists();   // ← add this line
  void loadBatchMeta();
  return () => { /* existing cleanup */ };
});
```

Also add `loadArtists` to the import from `$lib/stores/library`:
```typescript
import {
  tracks, albums, isLoading, isLoadingMore, totalTracks, totalAlbums,
  sortBy, sortDir, viewMode, searchQuery,
  loadTracks, loadAlbums, loadArtists,   // ← add loadArtists
  formatDuration, formatDateShort, getQualityClass,
  selectedTrackIds, selectedAlbumIds,
  lastSelectedTrackId, lastSelectedAlbumId,
  selectTrackIds, selectAlbumIds, clearSelection,
} from '$lib/stores/library';
```

Also import the artists store:
```typescript
import { tracks, albums, artists as libraryArtists, /* ... */ } from '$lib/stores/library';
```

Note: `artists` is already a `$state` local variable in the library page (line ~43). Rename the store import to avoid conflict — use `libraryArtistsStore` as the import alias, or use the local `artists` state which is already loaded from `api.getArtists()`. Check whether the page already has its own artists loading. If local `artists` state is populated from `api.getArtists()` on tab switch, move that load to onMount instead. Verify by searching for where `artists` state is populated.

- [ ] **Step 4: Replace the `.tab-bar` HTML with filter pills**

Find the `.library-hero-actions` div containing the `.tab-bar`. Replace the entire `.tab-bar` div:

**Remove:**
```html
<div class="tab-bar">
  <button class="tab" class:active={activeTab === 'albums'} onclick={() => switchTab('albums')}>Albums</button>
  <button class="tab" class:active={activeTab === 'tracks'} onclick={() => switchTab('tracks')}>Tracks</button>
  <button class="tab" class:active={activeTab === 'artists'} onclick={() => switchTab('artists')}>Artists</button>
</div>
```

**Add:**
```html
<div class="filter-pills">
  <button class="filter-pill" class:active={activeTab === 'all'}     onclick={() => switchTab('all')}>All</button>
  <button class="filter-pill" class:active={activeTab === 'tracks'}  onclick={() => switchTab('tracks')}>Tracks</button>
  <button class="filter-pill" class:active={activeTab === 'albums'}  onclick={() => switchTab('albums')}>Albums</button>
  <button class="filter-pill" class:active={activeTab === 'artists'} onclick={() => switchTab('artists')}>Artists</button>
</div>
```

- [ ] **Step 5: Update `switchTab` function to handle `'all'`**

Find the `switchTab` function. Add `'all'` as a valid case — when switching to `'all'`, no data fetch is needed (data is derived). The view-toggle should only show for Albums:

```typescript
function switchTab(tab: 'all' | 'tracks' | 'albums' | 'artists') {
  activeTab = tab;
  cursorIndex = -1;
  clearSelection();
  if (tab === 'tracks') { void loadTracks($sortBy, $sortDir); }
  else if (tab === 'albums') { void loadAlbums($sortBy, $sortDir); }
  else if (tab === 'artists') { /* artists already loaded in onMount */ }
  // 'all' — no fetch, data is derived
}
```

- [ ] **Step 6: Move the view-toggle to only show on Albums tab**

Find the `.view-toggle` div. Wrap it:
```html
{#if activeTab === 'albums'}
  <div class="view-toggle" role="group" aria-label="Album view layout">
    <!-- existing toggle buttons unchanged -->
  </div>
{/if}
```

- [ ] **Step 7: Add filter-pill CSS**

In the `<style>` block, add (or replace the existing `.tab-bar`/`.tab` rules with):

```css
.filter-pills {
  display: flex;
  gap: 6px;
  align-items: center;
}

.filter-pill {
  padding: 5px 14px;
  border-radius: 20px;
  border: 1px solid var(--border-subtle, rgba(255,255,255,0.1));
  background: transparent;
  color: var(--text-secondary, rgba(255,255,255,0.6));
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
  white-space: nowrap;
}

.filter-pill:hover {
  background: var(--bg-glass-hover, rgba(255,255,255,0.08));
  color: var(--text-primary, #fff);
}

.filter-pill.active {
  background: var(--accent, #9b6fff);
  border-color: var(--accent, #9b6fff);
  color: #fff;
}
```

- [ ] **Step 8: Verify tabs switch correctly**

Run the dev server (`npm run dev` in `frontend/`) and open the Library page. Confirm:
- "All" pill is selected by default
- Clicking Tracks/Albums/Artists switches view as before
- Active pill has accent background
- View toggle only appears on Albums

- [ ] **Step 9: Commit**

```bash
git add frontend/src/routes/library/+page.svelte
git commit -m "feat(library): replace tab bar with Search-style filter pills + All tab"
```

---

## Task 3: Remove DSP filter panel, add keyboard hint row

**Files:**
- Modify: `frontend/src/routes/library/+page.svelte`

- [ ] **Step 1: Delete DSP filter state variables**

Remove these lines from the `<script>` (around lines 75-90):
```typescript
let showDspFilters = $state(false);
let filterBpmMin = $state<number | null>(null);
let filterBpmMax = $state<number | null>(null);
let filterEnergyMin = $state(0);
let filterEnergyMax = $state(1);
let filterKey = $state('');
let filterInstrumental = $state(false);

const CAMELOT_KEYS = [
  '', '1A', '2A', '3A', '4A', '5A', '6A', '7A', '8A', '9A', '10A', '11A', '12A',
  '1B', '2B', '3B', '4B', '5B', '6B', '7B', '8B', '9B', '10B', '11B', '12B'
];
```

- [ ] **Step 2: Delete `applyDspFilters` and `clearDspFilters` functions**

Search for `function applyDspFilters` and `function clearDspFilters` in the file and delete both function bodies entirely.

- [ ] **Step 3: Remove DSP filter panel HTML**

Find and delete the entire `{#if activeTab === 'tracks'}` block that contains `.dsp-filter-bar`:
```html
<!-- DELETE this entire block: -->
{#if activeTab === 'tracks'}
  <div class="dsp-filter-bar glass">
    <button class="btn btn-glass btn-sm" onclick={() => showDspFilters = !showDspFilters}>
      ...
    </button>
    {#if showDspFilters}
      <div class="dsp-filter-grid">
        ...
      </div>
    {/if}
  </div>
{/if}
```

- [ ] **Step 4: Add keyboard hint row below the library search input**

Find the library toolbar search input in the HTML. Directly below it, add:

```html
<div class="kbd-hint">
  <kbd>/</kbd> focus &nbsp;·&nbsp;
  <kbd>↑↓</kbd> move &nbsp;·&nbsp;
  <kbd>Enter</kbd> play &nbsp;·&nbsp;
  <kbd>Shift</kbd>+<kbd>Enter</kbd> queue &nbsp;·&nbsp;
  <kbd>Ctrl</kbd>+<kbd>Enter</kbd> next &nbsp;·&nbsp;
  <span class="hint-filters">bpm:138 &nbsp;·&nbsp; key:Am &nbsp;·&nbsp; energy:&gt;0.7 &nbsp;·&nbsp; genre:dnb &nbsp;·&nbsp; instrumental:true</span>
</div>
```

- [ ] **Step 5: Add kbd-hint CSS**

```css
.kbd-hint {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 2px;
  font-size: 11px;
  color: var(--text-muted, rgba(255,255,255,0.35));
  padding: 4px 0 0 2px;
  user-select: none;
}

.kbd-hint kbd {
  display: inline-block;
  padding: 1px 5px;
  border: 1px solid var(--border-subtle, rgba(255,255,255,0.15));
  border-radius: 4px;
  font-family: inherit;
  font-size: 10px;
  color: var(--text-secondary, rgba(255,255,255,0.5));
  background: var(--bg-glass, rgba(255,255,255,0.05));
}

.hint-filters {
  opacity: 0.7;
  font-family: monospace;
  letter-spacing: 0.02em;
}
```

- [ ] **Step 6: Remove orphaned DSP CSS**

Search the `<style>` block for `.dsp-filter-bar`, `.dsp-filter-grid`, `.filter-group`, `.filter-inputs`, `.toggle-switch-small` and delete those rule blocks.

- [ ] **Step 7: Verify no compile errors**

```bash
cd frontend && npm run check 2>&1 | grep -E "error|Error" | head -20
```

Expected: 0 errors.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/routes/library/+page.svelte
git commit -m "feat(library): remove DSP filter panel, add inline query hint row"
```

---

## Task 4: Add derived data for the "All" home view

**Files:**
- Modify: `frontend/src/routes/library/+page.svelte`

- [ ] **Step 1: Import stores and add derived state**

At the top of `<script>`, ensure `artists` store is imported (aliased to avoid the local `artists` state variable):

```typescript
import {
  tracks, albums, artists as $artistsStore,
  isLoading, isLoadingMore, totalTracks, totalAlbums,
  sortBy, sortDir, viewMode, searchQuery,
  loadTracks, loadAlbums, loadArtists,
  formatDuration, formatDateShort, getQualityClass,
  selectedTrackIds, selectedAlbumIds,
  lastSelectedTrackId, lastSelectedAlbumId,
  selectTrackIds, selectAlbumIds, clearSelection,
} from '$lib/stores/library';
```

Note: the existing `let artists = $state<Artist[]>([])` local variable may be used for the artist grid tab. Keep it. For the "All" view derivations, read from `$artistsStore` (the store) which is populated by `loadArtists()` called in onMount.

- [ ] **Step 2: Add `topArtist` derived state**

After the existing state declarations in `<script>`, add:

```typescript
interface HomeArtist {
  id: number;
  name: string;
  photo_url: string | null;
  playCount: number;
  trackCount: number;
  albumCount: number;
}

let topArtist = $derived.by<HomeArtist | null>(() => {
  const artistMap = new Map($artistsStore.map(a => [a.id, a]));
  const countMap = new Map<number, HomeArtist>();
  const albumsByArtist = new Map<number, Set<number>>();

  for (const track of $tracks) {
    if (!track.artist_id) continue;
    const info = countMap.get(track.artist_id);
    const storeArtist = artistMap.get(track.artist_id);
    if (info) {
      info.playCount += track.play_count ?? 0;
      info.trackCount++;
    } else {
      countMap.set(track.artist_id, {
        id: track.artist_id,
        name: track.artist_name ?? 'Unknown Artist',
        photo_url: storeArtist?.photo_url ?? null,
        playCount: track.play_count ?? 0,
        trackCount: 1,
        albumCount: 0,
      });
    }
    if (track.album_id) {
      if (!albumsByArtist.has(track.artist_id)) albumsByArtist.set(track.artist_id, new Set());
      albumsByArtist.get(track.artist_id)!.add(track.album_id);
    }
  }

  for (const [id, data] of countMap) {
    data.albumCount = albumsByArtist.get(id)?.size ?? 0;
  }

  if (countMap.size === 0) return null;
  return [...countMap.values()].reduce((best, cur) => cur.playCount > best.playCount ? cur : best);
});
```

- [ ] **Step 3: Add `recentArtists` derived state**

```typescript
interface HomeArtistCard {
  id: number;
  name: string;
  photo_url: string | null;
}

let recentArtists = $derived.by<HomeArtistCard[]>(() => {
  const artistMap = new Map($artistsStore.map(a => [a.id, a]));
  const seen = new Set<number>();
  const result: HomeArtistCard[] = [];

  const sorted = [...$tracks].sort((a, b) => {
    if (!a.last_played_at && !b.last_played_at) return 0;
    if (!a.last_played_at) return 1;
    if (!b.last_played_at) return -1;
    return b.last_played_at.localeCompare(a.last_played_at);
  });

  for (const track of sorted) {
    if (!track.artist_id || seen.has(track.artist_id)) continue;
    seen.add(track.artist_id);
    const storeArtist = artistMap.get(track.artist_id);
    result.push({ id: track.artist_id, name: track.artist_name ?? 'Unknown', photo_url: storeArtist?.photo_url ?? null });
    if (result.length >= 20) break;
  }
  return result;
});
```

- [ ] **Step 4: Add `recentAlbums` derived state**

```typescript
interface HomeAlbumCard {
  id: number;
  title: string;
  artist_name: string | null;
  artwork_url: string | null;
}

let recentAlbums = $derived.by<HomeAlbumCard[]>(() => {
  const albumDateMap = new Map<number, { card: HomeAlbumCard; date: string }>();

  for (const track of $tracks) {
    if (!track.album_id || !track.date_added) continue;
    const existing = albumDateMap.get(track.album_id);
    if (!existing || track.date_added > existing.date) {
      albumDateMap.set(track.album_id, {
        card: {
          id: track.album_id,
          title: track.album_title ?? 'Unknown Album',
          artist_name: track.artist_name,
          artwork_url: track.artwork_url,
        },
        date: track.date_added,
      });
    }
  }

  return [...albumDateMap.values()]
    .sort((a, b) => b.date.localeCompare(a.date))
    .slice(0, 20)
    .map(({ card }) => card);
});
```

- [ ] **Step 5: Add `recentTracks` derived state**

```typescript
let recentTracks = $derived.by(() =>
  [...$tracks]
    .filter(t => t.last_played_at)
    .sort((a, b) => b.last_played_at!.localeCompare(a.last_played_at!))
    .slice(0, 10)
);
```

- [ ] **Step 6: Verify no TypeScript errors**

```bash
cd frontend && npm run check 2>&1 | grep -E "error|Error" | head -20
```

Expected: 0 errors.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/routes/library/+page.svelte
git commit -m "feat(library): add derived data for All home view (topArtist, carousels, recentTracks)"
```

---

## Task 5: Create `LibraryHero.svelte` component

**Files:**
- Create: `frontend/src/lib/components/LibraryHero.svelte`

- [ ] **Step 1: Create the component**

```svelte
<!-- frontend/src/lib/components/LibraryHero.svelte -->
<script lang="ts">
  interface Artist {
    id: number;
    name: string;
    photo_url: string | null;
    playCount: number;
    trackCount: number;
    albumCount: number;
  }

  let { artist, onPlayAll, onShuffle }: {
    artist: Artist;
    onPlayAll: () => void;
    onShuffle: () => void;
  } = $props();

  function letterColor(name: string): string {
    const colors = ['#e63946','#457b9d','#2a9d8f','#e9c46a','#f4a261','#9b5de5','#00b4d8'];
    let h = 0;
    for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff;
    return colors[Math.abs(h) % colors.length];
  }

  function initials(name: string): string {
    return name.split(/\s+/).map(p => p[0]?.toUpperCase() ?? '').join('').slice(0, 2) || '?';
  }
</script>

<div class="library-hero-card">
  {#if artist.photo_url}
    <div class="hero-bg" style="background-image: url('{artist.photo_url}')"></div>
  {:else}
    <div class="hero-bg hero-bg--color" style="background: {letterColor(artist.name)}"></div>
  {/if}

  <div class="hero-content">
    <div class="hero-art">
      {#if artist.photo_url}
        <div class="hero-avatar" style="background-image: url('{artist.photo_url}')"></div>
      {:else}
        <div class="hero-avatar hero-avatar--fallback" style="background: {letterColor(artist.name)}">
          <span>{initials(artist.name)}</span>
        </div>
      {/if}
    </div>

    <div class="hero-meta">
      <span class="hero-kind">YOUR TOP ARTIST</span>
      <h2 class="hero-title">{artist.name}</h2>
      <p class="hero-sub">{artist.trackCount} tracks &nbsp;·&nbsp; {artist.albumCount} albums</p>
      <div class="hero-actions">
        <button class="btn btn-primary hero-play" onclick={onPlayAll}>
          <svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
            <path d="M3 2.5l10 5.5-10 5.5V2.5z"/>
          </svg>
          Play All
        </button>
        <button class="btn btn-glass" onclick={onShuffle}>Shuffle</button>
      </div>
    </div>
  </div>
</div>

<style>
  .library-hero-card {
    position: relative;
    border-radius: 12px;
    overflow: hidden;
    background: var(--bg-glass, rgba(255,255,255,0.04));
    border: 1px solid var(--border-subtle, rgba(255,255,255,0.08));
    min-height: 200px;
  }

  .hero-bg {
    position: absolute;
    inset: 0;
    background-size: cover;
    background-position: center top;
    filter: blur(40px) brightness(0.35) saturate(1.4);
    transform: scale(1.1);
    z-index: 0;
  }

  .hero-bg--color {
    opacity: 0.3;
  }

  .hero-content {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 28px;
    padding: 28px 32px;
  }

  .hero-avatar {
    width: 140px;
    height: 140px;
    border-radius: 50%;
    background-size: cover;
    background-position: center;
    flex-shrink: 0;
    box-shadow: 0 8px 32px rgba(0,0,0,0.4);
  }

  .hero-avatar--fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 48px;
    font-weight: 700;
    color: rgba(255,255,255,0.9);
  }

  .hero-meta {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .hero-kind {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 1.5px;
    color: var(--accent, #9b6fff);
    text-transform: uppercase;
  }

  .hero-title {
    font-size: clamp(28px, 4vw, 44px);
    font-weight: 700;
    line-height: 1.1;
    color: var(--text-primary, #fff);
    margin: 0;
  }

  .hero-sub {
    font-size: 14px;
    color: var(--text-secondary, rgba(255,255,255,0.55));
    margin: 2px 0 8px;
  }

  .hero-actions {
    display: flex;
    gap: 10px;
    align-items: center;
    margin-top: 4px;
  }

  .hero-play {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 10px 22px;
    border-radius: 24px;
    font-size: 14px;
    font-weight: 600;
  }
</style>
```

- [ ] **Step 2: Verify component compiles**

```bash
cd frontend && npm run check 2>&1 | grep -E "error|Error" | head -20
```

Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/LibraryHero.svelte
git commit -m "feat(library): add LibraryHero component — top artist card with play/shuffle"
```

---

## Task 6: Create `ArtistCarousel.svelte` component

**Files:**
- Create: `frontend/src/lib/components/ArtistCarousel.svelte`

- [ ] **Step 1: Create the component**

```svelte
<!-- frontend/src/lib/components/ArtistCarousel.svelte -->
<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';

  interface ArtistCard {
    id: number;
    name: string;
    photo_url: string | null;
  }

  let { artists, onArtistClick }: {
    artists: ArtistCard[];
    onArtistClick?: (id: number) => void;
  } = $props();

  function letterColor(name: string): string {
    const colors = ['#e63946','#457b9d','#2a9d8f','#e9c46a','#f4a261','#9b5de5','#00b4d8'];
    let h = 0;
    for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff;
    return colors[Math.abs(h) % colors.length];
  }

  function initials(name: string): string {
    return name.split(/\s+/).map(p => p[0]?.toUpperCase() ?? '').join('').slice(0, 2) || '?';
  }
</script>

{#if artists.length > 0}
  <div class="artists-row" use:wheelToHorizontal>
    {#each artists as artist (artist.id)}
      <button
        class="artist-card"
        onclick={() => onArtistClick?.(artist.id)}
        title={artist.name}
      >
        <div class="avatar-wrap">
          {#if artist.photo_url}
            <div class="artist-avatar" style="background-image: url('{artist.photo_url}')"></div>
          {:else}
            <div class="artist-avatar fallback" style="background: {letterColor(artist.name)}">
              <span>{initials(artist.name)}</span>
            </div>
          {/if}
        </div>
        <span class="artist-name">{artist.name}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .artists-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
    padding: 4px 2px 12px;
  }

  .artists-row::-webkit-scrollbar { display: none; }

  .artist-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    width: 84px;
    flex-shrink: 0;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    color: inherit;
  }

  .avatar-wrap {
    position: relative;
  }

  .artist-avatar {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    background-size: cover;
    background-position: center;
    transition: transform 0.15s;
  }

  .artist-card:hover .artist-avatar {
    transform: scale(1.06);
  }

  .artist-avatar.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 22px;
    font-weight: 700;
    color: rgba(255,255,255,0.85);
  }

  .artist-name {
    font-size: 11px;
    color: var(--text-secondary, rgba(255,255,255,0.6));
    text-align: center;
    width: 84px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
```

- [ ] **Step 2: Verify**

```bash
cd frontend && npm run check 2>&1 | grep -E "error|Error" | head -20
```

Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/ArtistCarousel.svelte
git commit -m "feat(library): add ArtistCarousel component"
```

---

## Task 7: Create `AlbumCarousel.svelte` component

**Files:**
- Create: `frontend/src/lib/components/AlbumCarousel.svelte`

- [ ] **Step 1: Create the component**

```svelte
<!-- frontend/src/lib/components/AlbumCarousel.svelte -->
<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';

  interface AlbumCard {
    id: number;
    title: string;
    artist_name: string | null;
    artwork_url: string | null;
  }

  let { albums, onAlbumClick }: {
    albums: AlbumCard[];
    onAlbumClick?: (id: number) => void;
  } = $props();

  function letterColor(name: string): string {
    const colors = ['#e63946','#457b9d','#2a9d8f','#e9c46a','#f4a261','#9b5de5','#00b4d8'];
    let h = 0;
    for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff;
    return colors[Math.abs(h) % colors.length];
  }
</script>

{#if albums.length > 0}
  <div class="albums-row" use:wheelToHorizontal>
    {#each albums as album (album.id)}
      <button
        class="album-card"
        onclick={() => onAlbumClick?.(album.id)}
        title={album.title}
      >
        <div class="art-wrap">
          {#if album.artwork_url}
            <div class="album-art" style="background-image: url('{album.artwork_url}')"></div>
          {:else}
            <div class="album-art fallback" style="background: {letterColor(album.title)}">
              <span>♫</span>
            </div>
          {/if}
          <div class="art-play-overlay">
            <svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">
              <path d="M3 2.5l10 5.5-10 5.5V2.5z"/>
            </svg>
          </div>
        </div>
        <span class="album-title">{album.title}</span>
        {#if album.artist_name}
          <span class="album-artist">{album.artist_name}</span>
        {/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .albums-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
    padding: 4px 2px 12px;
  }

  .albums-row::-webkit-scrollbar { display: none; }

  .album-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 128px;
    flex-shrink: 0;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    color: inherit;
    text-align: left;
  }

  .art-wrap {
    position: relative;
    width: 128px;
    height: 128px;
    border-radius: 6px;
    overflow: hidden;
  }

  .album-art {
    width: 100%;
    height: 100%;
    background-size: cover;
    background-position: center;
    transition: transform 0.15s;
  }

  .album-card:hover .album-art { transform: scale(1.04); }

  .album-art.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 36px;
    color: rgba(255,255,255,0.5);
  }

  .art-play-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0,0,0,0.45);
    opacity: 0;
    transition: opacity 0.15s;
    border-radius: 50%;
    width: 36px;
    height: 36px;
    margin: auto;
    color: #fff;
  }

  .album-card:hover .art-play-overlay { opacity: 1; }

  .album-title {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-primary, #fff);
    width: 128px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .album-artist {
    font-size: 11px;
    color: var(--text-secondary, rgba(255,255,255,0.5));
    width: 128px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
```

- [ ] **Step 2: Verify**

```bash
cd frontend && npm run check 2>&1 | grep -E "error|Error" | head -20
```

Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/AlbumCarousel.svelte
git commit -m "feat(library): add AlbumCarousel component"
```

---

## Task 8: Wire the "All" home view in library page

**Files:**
- Modify: `frontend/src/routes/library/+page.svelte`

- [ ] **Step 1: Import new components and player actions**

Add imports at the top of `<script>`:

```typescript
import LibraryHero from '$lib/components/LibraryHero.svelte';
import ArtistCarousel from '$lib/components/ArtistCarousel.svelte';
import AlbumCarousel from '$lib/components/AlbumCarousel.svelte';
```

Also ensure these player store actions are imported (they likely already are):
```typescript
import { currentTrack, isPlaying, playTrackNow, addTrackToQueue, playTrackNext } from '$lib/stores/player';
```

- [ ] **Step 2: Add play handlers for the hero card**

In `<script>`, add these two functions:

```typescript
function playAllFromTopArtist() {
  if (!topArtist) return;
  const artistTracks = $tracks.filter(t => t.artist_id === topArtist!.id);
  if (!artistTracks.length) return;
  playTrackNow(artistTracks[0]);
  for (const t of artistTracks.slice(1)) addTrackToQueue(t);
}

function shuffleTopArtist() {
  if (!topArtist) return;
  const artistTracks = [...$tracks.filter(t => t.artist_id === topArtist!.id)];
  artistTracks.sort(() => Math.random() - 0.5);
  if (!artistTracks.length) return;
  playTrackNow(artistTracks[0]);
  for (const t of artistTracks.slice(1)) addTrackToQueue(t);
}
```

- [ ] **Step 3: Add artist/album click handlers**

```typescript
function handleHomeArtistClick(artistId: number) {
  // Switch to artists tab and expand this artist
  activeTab = 'artists';
  expandedArtistId = artistId;
}

function handleHomeAlbumClick(albumId: number) {
  // Open the album detail modal — reuse existing expandAlbum logic
  expandedAlbumId = albumId;
  // trigger detail load if function exists:
  // void loadAlbumDetail(albumId)  ← use whatever function already handles this
}
```

Note: Check the existing code for how the album detail modal is triggered. Search for `expandedAlbumId` assignments to find the right function name and replicate that pattern.

- [ ] **Step 4: Add the "All" view HTML block**

Find the main content area where the tabs render. Look for the pattern:

```html
{#if activeTab === 'tracks'}
  <!-- track table -->
{:else if activeTab === 'albums'}
  <!-- album grid -->
{:else if activeTab === 'artists'}
  <!-- artist grid -->
{/if}
```

Add an `{#if activeTab === 'all'}` block BEFORE the existing tab content:

```html
{#if activeTab === 'all'}
  <div class="library-home">
    {#if topArtist}
      <LibraryHero
        artist={topArtist}
        onPlayAll={playAllFromTopArtist}
        onShuffle={shuffleTopArtist}
      />
    {:else if $isLoading}
      <div class="home-loading">Loading your library…</div>
    {/if}

    {#if recentArtists.length > 0}
      <section class="home-section">
        <h3 class="section-label">Recently Played Artists</h3>
        <ArtistCarousel
          artists={recentArtists}
          onArtistClick={handleHomeArtistClick}
        />
      </section>
    {/if}

    {#if recentAlbums.length > 0}
      <section class="home-section">
        <h3 class="section-label">Recently Added</h3>
        <AlbumCarousel
          albums={recentAlbums}
          onAlbumClick={handleHomeAlbumClick}
        />
      </section>
    {/if}

    {#if recentTracks.length > 0}
      <section class="home-section">
        <div class="section-header-row">
          <h3 class="section-label">Recent Tracks</h3>
          <button class="view-all-link" onclick={() => switchTab('tracks')}>View all →</button>
        </div>
        <ul class="home-track-list">
          {#each recentTracks as track (track.id)}
            <li
              class="home-track-row"
              class:playing={$currentTrack?.id === track.id && $isPlaying}
              onclick={() => playTrackNow(track)}
              role="button"
              tabindex="0"
              onkeydown={(e) => e.key === 'Enter' && playTrackNow(track)}
            >
              {#if track.artwork_url}
                <div class="ht-art" style="background-image: url('{track.artwork_url}')"></div>
              {:else}
                <div class="ht-art ht-art--fallback"></div>
              {/if}
              <div class="ht-meta">
                <span class="ht-title">{track.title}</span>
                <span class="ht-sub">{track.artist_name ?? ''} {track.album_title ? `— ${track.album_title}` : ''}</span>
              </div>
              <span class="ht-duration">{formatDuration(track.duration_ms)}</span>
              <div class="ht-actions">
                <button
                  class="btn-icon"
                  title="Add to queue"
                  onclick={(e) => { e.stopPropagation(); addTrackToQueue(track); }}
                  aria-label="Add to queue"
                >
                  <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
                    <line x1="2" y1="4" x2="14" y2="4"/>
                    <line x1="2" y1="8" x2="14" y2="8"/>
                    <line x1="2" y1="12" x2="10" y2="12"/>
                    <line x1="13" y1="10" x2="13" y2="16"/>
                    <line x1="10" y1="13" x2="16" y2="13"/>
                  </svg>
                </button>
              </div>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  </div>

{:else if activeTab === 'tracks'}
  <!-- existing track table -->
{:else if activeTab === 'albums'}
  <!-- existing album grid -->
{:else if activeTab === 'artists'}
  <!-- existing artist grid -->
{/if}
```

- [ ] **Step 5: Add "All" view CSS**

```css
.library-home {
  display: flex;
  flex-direction: column;
  gap: 32px;
  padding: 8px 0 40px;
}

.home-section {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.section-label {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 1.5px;
  text-transform: uppercase;
  color: var(--accent, #9b6fff);
  margin: 0;
}

.section-header-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.view-all-link {
  font-size: 12px;
  color: var(--text-secondary, rgba(255,255,255,0.5));
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
  transition: color 0.15s;
}

.view-all-link:hover { color: var(--text-primary, #fff); }

.home-track-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}

.home-track-row {
  display: grid;
  grid-template-columns: 38px 1fr auto auto;
  gap: 12px;
  align-items: center;
  padding: 6px 8px;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.1s;
}

.home-track-row:hover { background: var(--bg-glass-hover, rgba(255,255,255,0.06)); }

.home-track-row.playing .ht-title { color: var(--accent, #9b6fff); }

.ht-art {
  width: 36px;
  height: 36px;
  border-radius: 4px;
  background-size: cover;
  background-position: center;
}

.ht-art--fallback {
  background: var(--bg-glass, rgba(255,255,255,0.08));
}

.ht-meta {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.ht-title {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary, #fff);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.ht-sub {
  font-size: 11px;
  color: var(--text-secondary, rgba(255,255,255,0.5));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.ht-duration {
  font-size: 12px;
  color: var(--text-muted, rgba(255,255,255,0.4));
  font-variant-numeric: tabular-nums;
}

.ht-actions {
  opacity: 0;
  transition: opacity 0.15s;
}

.home-track-row:hover .ht-actions { opacity: 1; }

.btn-icon {
  background: none;
  border: none;
  cursor: pointer;
  color: var(--text-secondary, rgba(255,255,255,0.5));
  padding: 4px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  transition: color 0.15s;
}

.btn-icon:hover { color: var(--text-primary, #fff); }

.home-loading {
  color: var(--text-secondary, rgba(255,255,255,0.5));
  font-size: 14px;
  padding: 40px;
  text-align: center;
}
```

- [ ] **Step 6: Verify full compile**

```bash
cd frontend && npm run check 2>&1 | grep -E "error|Error" | head -20
```

Expected: 0 errors.

- [ ] **Step 7: Manual verification in browser**

Run dev server (`npm run dev` in `frontend/`). Open Library:
- [ ] "All" tab is selected by default, shows hero card with your most-played artist
- [ ] Hero card shows artist photo (or letter fallback), name, track/album counts
- [ ] "Play All" button plays that artist's tracks
- [ ] "Shuffle" button shuffles and plays
- [ ] Recently Played Artists carousel scrolls horizontally with mouse wheel
- [ ] Recently Added carousel scrolls horizontally
- [ ] Recent Tracks shows last 10 with art + title + artist + duration
- [ ] Playing track has accent-colored title in recent tracks
- [ ] "View all →" switches to Tracks pill
- [ ] Clicking an artist in the carousel switches to Artists tab with that artist expanded
- [ ] Tracks / Albums / Artists pills show existing views unchanged

- [ ] **Step 8: Commit**

```bash
git add frontend/src/routes/library/+page.svelte
git commit -m "feat(library): wire All home view — hero, carousels, recent tracks snippet"
```

---

## Self-Review

**Spec coverage check:**
- ✅ Filter pills (All | Tracks | Albums | Artists) replacing tab bar — Task 2
- ✅ DSP filter panel removed, inline query hint row added — Task 3
- ✅ "All" home view: top artist hero card — Tasks 4, 5, 8
- ✅ Recently Played Artists carousel — Tasks 4, 6, 8
- ✅ Recently Added Albums carousel — Tasks 4, 7, 8
- ✅ Recent Tracks snippet (search-style rows) — Task 8
- ✅ "View all" link to Tracks — Task 8
- ✅ Tracks/Albums/Artists views unchanged — Tasks 2, 8 (no changes to those views)
- ✅ Hero ambient glow from artist photo — Task 5 (blur + overlay technique)
- ✅ Scroll position restores correctly — Task 2 (sessionStorage updated for 'all')
- ✅ Existing `.library-hero` section removed — Task 8 (`activeTab === 'all'` replaces it)
- ✅ View toggle moved to Albums only — Task 2

**Type consistency:**
- `HomeArtist` defined in Task 4, used in Task 5 (`LibraryHero`) and Task 8 — ✅
- `HomeArtistCard` defined in Task 4, matches `ArtistCarousel` prop shape — ✅
- `HomeAlbumCard` defined in Task 4, matches `AlbumCarousel` prop shape — ✅
- `wheelToHorizontal` extracted in Task 1, imported in Tasks 6, 7 — ✅
