# Home layout system, TIDAL fill, Last.fm recommendation repair, motion

## Context

The Home page does not sit right at any window size and gets worse as the window
changes. Three separate causes, all confirmed in source:

1. **Cards are hard-locked at 180px** in all three home rails, quadruple-declared
   (`flex-basis` + `width` + `min-width` + `max-width`), with **no media or
   container queries at all**. The page shell is
   `--content-width: clamp(1280px, 100vw - 4rem, 2400px)` ([app.css:266](frontend/src/app.css)),
   so the number of visible cards is whatever 180px divides into that. The
   remainder is a card clipped at a random fraction, which is the "struggling to
   sit right" in the screenshots (Sustance, half-cut, in Personal Radio).
   `TidalDiscoverShelves` on /search already proves the fix works:
   `--card-w: clamp(120px, 11vw, 168px)` ([TidalDiscoverShelves.svelte:475](frontend/src/lib/components/search/TidalDiscoverShelves.svelte#L475)).
2. **Three stacked full-width murals.** `HomeRecommendationsShelf` renders one
   `ChartMural` per Last.fm shelf ([HomeRecommendationsShelf.svelte:396-448](frontend/src/lib/components/home/HomeRecommendationsShelf.svelte#L396)),
   each `min-height: clamp(220px, 24vw, 360px)`, each on its own 5500ms rotation
   timer. That is roughly 1100px of near-identical skewed mosaic in one scroll.
3. **No shared section vocabulary.** Three different eyebrow styles render in one
   page: `.13em/--text-tertiary` (global `.eyebrow`), `.08em/--text-secondary/bold`
   (`HomeMoodsRail` redefines it locally at line 198), and `0em/--text-muted`
   (`SectionHeader variant="charts"`). Gaps are 16px / 14px / `--gap-sm` /
   `--space-3` / `--space-5` / 10px depending on which component you land in.

Separately, the Last.fm recommendation shelves are visibly broken in three ways
the user reported, and all three have concrete root causes (Part 5 and Part 6).

Finally, the entry motion introduced on the liked-videos wall and the /videos
shelves never reached Home: Home has one page-level `animate-in` fired on mount
before any data arrives, so every shelf that resolves later just pops.

Decisions taken with the user:
- Recommendations become **one mural (tracks) plus two rails (artists, albums)**.
- **Re-split Home and Search** rather than merging the routes. Home becomes the
  browse surface; /search becomes search only. Home gets a search bar at the top
  that hands off to /search.
- TIDAL fill: **home-modules, new releases, hi-res**.
- Artwork resolution moves **server-side into `/api/home/recommendations`**.

---

## Part 1: One rail primitive, fluid by container query

**Problem being solved:** the clipped card at the right edge, at every width.

Create `frontend/src/lib/components/home/HomeRail.svelte` (or extend the existing
generic `frontend/src/lib/components/ui/MediaRail.svelte`, which already takes
`gap`/`padding` props and is currently unused by Home). Prefer extending
`MediaRail` so there is one rail in the app, not two.

The card width is derived so a whole number of cards plus one deliberate 0.35
peek always fits. The peek is the scroll affordance; the point is that it is
intentional and identical everywhere instead of an accident of division.

```css
/* wrapper section establishes the query container */
.rail-section { container-type: inline-size; }

.rail {
  --cols: 3;
  --peek: 0.35;
  --rail-gap: var(--gap-sm);
  display: flex;
  gap: var(--rail-gap);
  overflow-x: auto;
  scroll-snap-type: x proximity;
  mask-image: linear-gradient(to right, transparent 0, black 16px,
                              black calc(100% - 32px), transparent 100%);
}

.rail > :global(*) {
  flex: 0 0 calc(
    (100% - (var(--cols) + var(--peek) - 1) * var(--rail-gap))
    / (var(--cols) + var(--peek))
  );
  scroll-snap-align: start;
}

@container (min-width:  560px) { .rail { --cols: 4;  } }
@container (min-width:  760px) { .rail { --cols: 5;  } }
@container (min-width:  980px) { .rail { --cols: 6;  } }
@container (min-width: 1200px) { .rail { --cols: 7;  } }
@container (min-width: 1440px) { .rail { --cols: 8;  } }
@container (min-width: 1700px) { .rail { --cols: 9;  } }
@container (min-width: 2000px) { .rail { --cols: 10; } }
```

Container queries are already used in this codebase
([AlbumCarousel.svelte:98,128](frontend/src/lib/components/AlbumCarousel.svelte#L98)),
so this is not a new dependency.

Migrate onto it and delete the hand-rolled duplicates:
- [YourMixesShelf.svelte:~230-287](frontend/src/lib/components/home/YourMixesShelf.svelte#L230) (`.mix-rail` + `.mix-card`, both shelves)
- [PersonalRadioShelf.svelte:~184-237](frontend/src/lib/components/home/PersonalRadioShelf.svelte#L184) (byte-for-byte copy of the above)
- [HomeMoodsRail.svelte:~201-232](frontend/src/lib/components/home/HomeMoodsRail.svelte#L201)
- `.horizontal-scroll` / `.article-card` in [routes/+page.svelte:256-295](frontend/src/routes/+page.svelte#L256)

Each of those currently repeats the same mask gradient and the same
`::-webkit-scrollbar { height: 6px }` block. All of it collapses into the one
component. Keep `use:wheelToHorizontal`
([actions/wheel-to-horizontal.ts](frontend/src/lib/actions/wheel-to-horizontal.ts)) applied inside the primitive.

Also drop `scroll-snap-type: x mandatory` in favour of `proximity`. Mandatory
snapping on a rail that is wider than the viewport fights the user's scroll;
`MediaRail` already uses `proximity` for this reason.

## Part 2: One section vocabulary, one spacing scale

**Problem being solved:** the page reads as five unrelated components stacked.

- Every Home section renders through the existing
  [SectionHeader.svelte](frontend/src/lib/components/ui/SectionHeader.svelte).
  Delete the inline `.section-header` / `.section-title-group` markup in
  [routes/+page.svelte:141-149 and 179-187](frontend/src/routes/+page.svelte#L141),
  [YourMixesShelf.svelte:155-163, 186-191](frontend/src/lib/components/home/YourMixesShelf.svelte#L155),
  [PersonalRadioShelf.svelte:127-135](frontend/src/lib/components/home/PersonalRadioShelf.svelte#L127).
- Delete the local `.eyebrow` override at
  [HomeMoodsRail.svelte:198](frontend/src/lib/components/home/HomeMoodsRail.svelte#L198)
  so all eyebrows use the global rule ([app.css:461](frontend/src/app.css#L461)).
- Give `SectionHeader` an optional `href` so section titles that have a
  dedicated route ("Moods", "New releases", "Hi-Res") carry a "See all" link.
  That is the mechanism that surfaces the orphaned routes (Part 3).
- Rhythm: `.home-page` keeps `gap: var(--space-5)` between sections and each
  section uses `gap: var(--space-3)` between header and rail. Remove the raw
  `16px` / `14px` / `10px` literals. This is the whole answer to "better
  spacing": one value between sections, one value inside a section, nothing else.
- Replace the raw `transition: transform 0.2s ease, box-shadow 0.2s ease` on
  `.article-card` / `.news-card` ([routes/+page.svelte:288,356](frontend/src/routes/+page.svelte#L288))
  with `var(--motion-base)`. Note the footgun documented at
  [STYLING.md:29-52](frontend/STYLING.md#L29): `var(--motion-base) ease` is
  invalid CSS and silently drops the rule, because the token already carries
  its own easing.

## Part 3: TIDAL fill, and rescuing the orphaned routes

`/explore`, `/hires`, `/new-releases`, `/tidal/videos`, `/tidal/genres` already
exist as routes built on `TidalEditorialPage`. They are not in the sidebar, so
they are unreachable. Home is where they get surfaced.

Add to Home, in this order (all use `TidalDiscoverShelves`, which is already
fluid-sized, already has context menus wired, and is already 6h-cached):

1. **TIDAL home modules** via the existing
   [DiscoverShelves.svelte](frontend/src/lib/components/search/DiscoverShelves.svelte)
   (`/api/tidal/home-modules`: The Hits, New Tracks, New Albums, Spotlighted
   Uploads, From our editors). Zero new backend, zero new component.
2. **New releases** preview: first 1-2 modules of `/api/tidal/page/new-releases`,
   header linking to `/new-releases`.
3. **Hi-Res picks** preview: first 1-2 modules of `/api/tidal/page/hires`,
   header linking to `/hires`.

For (2) and (3), add a `limitModules` prop to `TidalEditorialPage` (or call
`cachedApi` directly and pass a sliced `modules` array to `TidalDiscoverShelves`,
which already takes `modules` as a prop). Do not duplicate the fetch logic.

`DiscoverShelves` currently mounts only in the /search empty state
([search/+page.svelte:1251-1253](frontend/src/routes/search/+page.svelte#L1251)).
Move it, do not copy it. See Part 4.

Proposed Home order, alternating heavy and light so no two murals or two
grids sit adjacent:

```
[ search bar -> /search ]
Your Mixes            (rail)
Personal Radio        (rail)
Last.fm hero          (mural)
Last.fm artists       (circle rail)
Last.fm albums        (cover rail)
The Hits / New Tracks / New Albums / Editors   (TIDAL home modules)
Moods                 (rail, "See all" -> /moods)
New releases          (preview, "See all" -> /new-releases)
Hi-Res picks          (preview, "See all" -> /hires)
Weekly articles       (rail)
Latest news           (grid)
```

Video Mixes folds into Your Mixes as a filter rather than a second full shelf,
or moves to /videos. Two adjacent mix rails of identical shape is the other
"slab" problem in screenshot 1.

## Part 4: Search bar on Home, and de-duplicating the browse surface

The routes stay separate. What moves is responsibility.

**Home gets a search bar.** Mount the existing
[SearchField.svelte](frontend/src/lib/search/ui/SearchField.svelte) at the top of
Home with `variant="page"`. It is a pure input primitive (no fetch, no
orchestration), so this is cheap. On Enter, `goto('/search?q=' + encodeURIComponent(value))`.

**The handoff already works.** /search reads `?q=` on mount via
`new URLSearchParams(window.location.search).get('q')`
([search/+page.svelte:163-170](frontend/src/routes/search/+page.svelte#L163))
and calls `onInput()`. No route or backend change is needed for the Home bar to
hand off; it is a one-line `goto` on the Home side.

**/search loses DiscoverShelves and gains a personal idle state.** Search becomes
a search tool about *your* stuff and *how to search*; Home becomes the discovery
surface. That removes the duplication without a 2589-line route merge, and leaves
the full merge available later as a small follow-on if wanted.

Idle layout, replacing
[search/+page.svelte:1236-1257](frontend/src/routes/search/+page.svelte#L1236):

```
[ SearchField, autofocused ]

Recent          [reggae] [lenzman] [calibre]        Clear
Jump back in    (recently played rail)
Your playlists  (playlist rail)
Try a filter    artist:  year:  bpm:  genre:  in:library
```

Two of those cost **zero new network** because the page already fetches the data
on mount and discards most of it
([search/+page.svelte:172-187](frontend/src/routes/search/+page.svelte#L172)):

- `cachedApi.getRecentListens(20)` is already called; only `artist_name` is kept,
  to build `recentArtistNames` for session-aware ranking. The full entries carry
  title, artist and artwork. Render them as a "Jump back in" rail using the Part 1
  rail primitive.
- `cachedApi.getPlaylists()` is already called and stored in `localPlaylists`,
  used only to surface playlist matches during an active search. Render it as a
  rail when idle.

The other two:

- **Recent searches** stays as-is. Chips are the right affordance for text
  queries; only restyle to match the new section vocabulary.
- **Try a filter** is a static grid built from the existing `FACETS` list in
  [$lib/search/facets](frontend/src/lib/search/facets.ts), which already powers
  the focus popover and Tab-completion but is effectively undiscoverable.
  Clicking an example fills the field. No data, no fetch, and it is the most
  search-native thing that can occupy the space.

Optional later additions, both backed by existing queries but needing a small
amount of plumbing, so treat as follow-ons rather than part of this change:
`date_added DESC` ordering already exists
([queries.rs:570, 1565](noor-server/src/db/queries.rs#L570)) for a "Recently
added" rail, and `queries::get_top_artists_by_history`
([routes.rs:3529](noor-server/src/server/routes.rs#L3529)) exists but is
currently only consumed internally by the discovery engine, so it would need an
endpoint.

Check [search_layout_contract.test.ts](frontend/src/routes/search/search_layout_contract.test.ts)
and [tidal_discover_artwork_contract.test.ts](frontend/src/lib/components/search/tidal_discover_artwork_contract.test.ts)
before moving; the latter asserts against `DiscoverShelves.svelte` source and may
reference its mount site.

## Part 5: Last.fm correctness (wrong hits, missing artist artwork)

### 5a. Artist tiles have no artwork source at all

[metadata/lastfm.rs:450-467](noor-server/src/metadata/lastfm.rs#L450) hardcodes
`image_url: None` in `artist_get_similar`. Even if it did not, Last.fm has not
served real artist images for years; `artist.getSimilar` returns the grey-star
placeholder. So server-side the artist shelf has nothing, and it falls to the
client, which then does the wrong thing:
[lazy-tidal-art.ts:178](frontend/src/lib/actions/lazy-tidal-art.ts#L178) reads
`result.tracks[0]?.artwork_url` for **every** query, so an artist tile gets an
album cover, or null when the artist name returns no tracks.

Meanwhile `/api/tidal/search` **already returns artist photos** the client throws
away ([routes.rs:8440-8453](noor-server/src/server/routes.rs#L8440), which merges
the TIDAL artist `picture` with local `artists.photo_url` by tidal_id).

Fix: add `kind: 'track' | 'artist' | 'album'` to `LazyTidalArtParams` and read
`result.artists[0]?.artwork_url` first when `kind === 'artist'`, falling back to
`tracks[0]`. This is a small change with a large visible effect.

Also wire the existing idempotent backfill
[services/tidal/artist_photo.rs::ensure_photo_url](noor-server/src/services/tidal/artist_photo.rs#L20)
into the recommendation resolver so a matched local artist gets its photo
persisted once. It is currently only called from the radio import path.

### 5b. The Last.fm placeholder star blocks the fallback

Album items get their image from `largest_image_url`
([metadata/lastfm.rs:1038](noor-server/src/metadata/lastfm.rs#L1038)), which
happily returns Last.fm's placeholder
`.../2a96cbd8b46e442fc41c2b86b821562f.png`. That URL is non-null, so
[HomeRecommendationsShelf.svelte:171-179](frontend/src/lib/components/home/HomeRecommendationsShelf.svelte#L171)
returns it, `lazy.enabled` is therefore `false`
([line 208](frontend/src/lib/components/home/HomeRecommendationsShelf.svelte#L208)),
and the tile shows a grey star forever with the TIDAL fallback never running.

The filter already exists but is private to one component:
[TrendingShelf.svelte:133-143](frontend/src/lib/components/charts/TrendingShelf.svelte#L133).

Fix, both ends:
- Reject the placeholder hash inside `largest_image_url` so it never enters the
  6h cache in the first place.
- Promote `usableArtwork` into `frontend/src/lib/utils/artwork.ts` next to the
  existing `firstArtworkUrl` / `isRenderableTidalArtworkUrl`, and use it in
  `HomeRecommendationsShelf`. Delete the private copy in `TrendingShelf`.

### 5c. Resolution is exact-match only

All three resolvers in
[home_routes.rs](noor-server/src/server/routes/home_routes.rs) compare with
`LOWER(x) = LOWER(?)` and nothing else (tracks ~621-702, artists ~704-765,
albums ~767-835). No accent folding, no `&` to `and`, no punctuation stripping.
"Sigur Ros", "Beyonce", "Tyler, The Creator" all miss, producing
`local_artist_id: null`, `artwork_url: null`, `playable: false`.

The normalizer already exists, client-side only, at
[recommendation_navigation.ts:115-124](frontend/src/lib/components/home/recommendation_navigation.ts#L115)
(`normalizeCatalogName`: NFKD, strip combining marks, lowercase, `&` to `and`,
collapse non-alphanumerics).

Fix: mirror it in Rust as `normalize_catalog_name`, and resolve in two passes,
exact first then normalized. Cheapest durable form is a persisted
`name_normalized` column on `artists` / `albums` / `tracks` with an index,
populated on write plus a one-off backfill. Note the shipped-app constraint:
pair it with a self-terminating background repair pass rather than assuming a
migration reaches existing installs.

### 5d. Clicking an artist can play the wrong artist

[recommendation_navigation.ts:81](frontend/src/lib/components/home/recommendation_navigation.ts#L81)
ends `findArtistMatch` with `return partial ?? artists[0];` - any search result
at all is accepted. `findAlbumMatch` correctly refuses to guess and returns null.

Fix: require a normalized-name match or a name overlap. On no match, fall through
to `recommendationSearchHref(item)` (already the final fallback in
`openRecommendationItem`) instead of silently opening the wrong artist.

### 5e. "Only a few tracks"

The album shelf runs up to 12 seeds x (1 similar-artists call + 8 top-albums
calls) = about 108 **sequential** Last.fm requests
([home_routes.rs:521-565](noor-server/src/server/routes/home_routes.rs#L521)),
each with an 8s timeout and each `.unwrap_or_default()`. Any rate-limit window
silently truncates the shelf. Then `load_or_fetch_recommendation_shelf`
(~lines 209-223) writes that truncated result to
`provider_recommendation_cache` **unconditionally**, pinning a short shelf for
6 hours.

Fix, three parts:
- Parallelise the fan-out with `futures::stream::iter(...).buffer_unordered(N)`.
  That exact pattern is already in this codebase at
  [tidal_home_routes.rs:1049-1056](noor-server/src/server/routes/tidal_home_routes.rs#L1049).
- Do not persist a short or empty shelf at the full 6h TTL. Cache at full TTL
  only when the shelf reaches its limit (or a floor); otherwise cache briefly so
  the next visit retries.
- Add stale-while-revalidate. `/api/home/suggestions` already does this
  (6h fresh + 7d stale) in
  [home_suggestions.rs:526-606](noor-server/src/server/routes/home_suggestions.rs#L526);
  `/api/home/recommendations` has nothing, so a cold key blocks the caller on
  the whole fan-out.

## Part 6: Last.fm performance (the stagger)

**Measured cause:** 3 shelves x `PANEL_LIMIT = 20` = up to 60 individual
`GET /api/tidal/search?limit=1` calls, funnelled through a **global**
`MAX_INFLIGHT = 4` ([lazy-tidal-art.ts:23](frontend/src/lib/actions/lazy-tidal-art.ts#L23))
shared with every other lazy-art consumer on the page. That is 15 serial waves.
The IntersectionObserver stages nothing, because `.chart-mural-bg` is
`position: absolute; inset: -7%`
([ChartMural.svelte:193-201](frontend/src/lib/components/charts/ChartMural.svelte#L193)),
so all 20 tiles intersect in the same tick. And `ChartMural` passes no `fadeIn`
to `ArtworkImage` ([lines 121-127](frontend/src/lib/components/charts/ChartMural.svelte#L121)),
so each tile hard-pops as it lands.

### 6a. Resolve artwork server-side (chosen approach)

`/api/home/recommendations` resolves artwork itself before responding, using
`buffer_unordered` against the existing 6h `tidal_search_cache`, and ships
complete items. Both caches are already 6h, so on a warm cache this is nearly
free, and the client N+1 disappears. `lazyTidalArt` stays as a last-resort
fallback for the rare unresolved item.

Concretely: after the three `fetch_lastfm_*_recommendations` calls in
`fetch_lastfm_home_recommendations`
([home_routes.rs:408-431](noor-server/src/server/routes/home_routes.rs#L408)),
run one artwork-resolution pass over items with a null/placeholder
`artwork_url`, keyed by entity kind (artist -> artist photo, track/album ->
cover). Reuse `services/tidal/cache.rs` so results land in the shared search
cache.

### 6b. Collapse the duplicate TIDAL search cache keys

The `tidal_search_cache` key is `sha256(query.lower + "|" + limit + "|" + offset)`
([services/tidal/cache.rs:16](noor-server/src/services/tidal/cache.rs#L16)), so
`searchTidal(q, 1)` (artwork) and `searchTidal(q, 5)` (navigate/play) are two
separate rows for the same artist, each costing an upstream search. Normalise
artwork lookups to the same limit the navigate path uses so they share a row.

### 6c. Kill the waterfall on mount

`load()` awaits both provider statuses before requesting recommendations
([HomeRecommendationsShelf.svelte:89-120](frontend/src/lib/components/home/HomeRecommendationsShelf.svelte#L89)),
even though status is already in the persisted query cache. Fire the
recommendations request in parallel with the status checks and discard the
result if the gate fails.

### 6d. One artwork resolver, not four

Confirmed independent implementations, which is the "multiple wiring conflicting"
the user suspected:
1. `lazyTidalArt` - cached, 4-wide, circuit-broken.
2. `DailyChartShelf.svelte:~232-268` - its own resolver, `Promise.all` over the
   whole visible list, no concurrency cap, no shared cache, no circuit breaker.
   (Verify these line numbers before editing.)
3. `TrendingShelf` - `peekTidalArt` + `lazyTidalArt` plus the private placeholder
   filter the other shelves lack.
4. `play_trending` / `play_recommendations` / `stores/player.ts` / `commands.ts` /
   `CommandPalette.svelte` - each re-searching the same names at different
   `limit` values, so they miss each other's cache rows.

Fix: one `frontend/src/lib/utils/artwork_resolver.ts` owning the cache, the
in-flight cap, the circuit breaker, the placeholder filter and the kind-aware
result reading. `lazyTidalArt` becomes a thin Svelte action over it. Delete the
`DailyChartShelf` private path.

### 6e. Stop the rotation churn

The 5500ms timer ([HomeRecommendationsShelf.svelte:79-87](frontend/src/lib/components/home/HomeRecommendationsShelf.svelte#L79))
mutates `currentIndexes`, which re-runs `shelfMuralItems()` for every shelf and
rebuilds 60 `ChartMuralItem` objects plus 60 action `update()` calls per tick,
while images are still landing. After Part 7 there is one mural instead of
three, which cuts this by two thirds; also memoise `shelfMuralItems` so a
rotation only changes the current index, not the item array identity.

### 6f. Add the fade

Pass `fadeIn` to `ArtworkImage` in `ChartMural`. The support already exists
([ArtworkImage.svelte:131-145](frontend/src/lib/components/ui/ArtworkImage.svelte#L131),
including the `complete`-check guard for cache-served images) and is simply
unused here.

## Part 7: Restructure the recommendations block

One mural plus two rails, per the chosen option.

- **Tracks** keeps `ChartMural`, unchanged in kind.
- **Artists** become a circle-avatar rail. Match the /search artists row
  ([search/+page.svelte:2155-2173](frontend/src/routes/search/+page.svelte#L2155)):
  72px avatar, `ArtworkImage` circle variant
  ([ArtworkImage.svelte:194-207](frontend/src/lib/components/ui/ArtworkImage.svelte#L194)
  already has the double-ring avatar styling), name beneath. Uses the Part 1 rail.
- **Albums** become a cover rail, matching the /search albums row
  ([search/+page.svelte:2232-2269](frontend/src/routes/search/+page.svelte#L2232)).
  Uses the Part 1 rail.
- Section headers keep the existing `SectionHeader variant="charts"` with the
  eyebrow "Connected profiles", and keep the per-shelf subtitles from
  `shelfSubtitle()`.
- Right-click menus must survive the move. `recommendationItemMenu()` already
  builds the correct menu per entity; wire it to each rail card's
  `oncontextmenu`, per the repo rule that every asset reference carries the
  shared context menu.

While in `ChartMural`: `.layout-count-20` has no CSS rule, so 17-20 items fall
through to the base 10x2 grid. `PANEL_LIMIT` is exactly 20, so this is the
common path. Either add the rule or delete the dead `muralLayoutClass` branch.
Also reconsider `white-space: nowrap` on `.chart-mural-title`
([ChartMural.svelte:~360-404](frontend/src/lib/components/charts/ChartMural.svelte#L360)),
which is what truncates "Live and Learn [Extend..." in the screenshot; two lines
with `line-clamp: 2` reads better at the sizes this title uses.

## Part 8: Motion

The pattern exists three times with drift and no shared home:

| Site | Duration | Rise | Step | Fill |
|---|---|---|---|---|
| [videos/liked/+page.svelte:701-757](frontend/src/routes/videos/liked/+page.svelte#L701) | 300ms | 8px | 22ms, `index % 24` | `backwards` |
| [VideoSetShelf.svelte:54-85](frontend/src/lib/components/video/VideoSetShelf.svelte#L54) | 340ms | 10px | 70ms | `both` |
| [library/+page.svelte:3109-3137](frontend/src/routes/library/+page.svelte#L3109) | 360ms | 10px | 70ms | `both` |

Extract into `app.css` as two documented variants and reuse, do not add a fourth
copy:

- `.rise-in--shelf` - 340ms, 10px, `calc(var(--rise-index, 0) * 70ms)`, `both`.
  For sections and shelves.
- `.rise-in--card` - 300ms, 8px, `calc(var(--rise-index, 0) * 22ms)`,
  **`backwards`**. For cards inside a rail. The `backwards` fill and the
  per-batch modulo cap are both load-bearing and the reasons are recorded in the
  comment at [videos/liked/+page.svelte:701-720](frontend/src/routes/videos/liked/+page.svelte#L701):
  `both` keeps the animation applied forever, which gives the element a permanent
  stacking context and traps popout z-index inside its own card; the modulo keeps
  the last card in a long list from waiting many seconds for its turn.

Both variants carry a `@media (prefers-reduced-motion: reduce) { animation: none }`
stand-down, as every existing copy does.

Then:
- Thread an `index` prop into each Home shelf from
  [routes/+page.svelte:126-137](frontend/src/routes/+page.svelte#L126), the way
  [videos/+page.svelte:773-775](frontend/src/routes/videos/+page.svelte#L773)
  does, and set `--rise-index` on each section.
- Set `--rise-index` per card inside the Part 1 rail primitive, capped by modulo.
- **Remove `animate-in` from `.page-shell` on Home**
  ([routes/+page.svelte:96](frontend/src/routes/+page.svelte#L96)). Library
  already made this move and locked it:
  [library-route-motion-contract.test.mjs:8-11](frontend/scripts/library-route-motion-contract.test.mjs#L8)
  asserts Library must NOT carry `animate-in`, because per-panel stagger replaced
  page-level translate. Home should follow.

The existing contract tests assert exact comment strings
([liked-videos-contract.test.mjs:166-184](frontend/scripts/liked-videos-contract.test.mjs#L166),
[video-editorial-browse-contract.test.mjs:60-75](frontend/scripts/video-editorial-browse-contract.test.mjs#L60)).
Extracting the CSS must keep those strings resolvable or the tests need updating
in the same commit.

---

## Working across sessions

This is several sessions of work on a Pro plan, so it has to survive being cut
off mid-way. The mechanism is the repo, not the conversation.

**Step 0, before any code:** copy this document to
`docs/plans/2026-07-28-home-layout-lastfm.md` and commit it. That directory
already holds dated plans of exactly this kind
(`docs/plans/2026-07-25-liked-videos-library.md`). Once it is committed, a cold
session needs only the repo to pick up: no conversation replay, no re-running
the exploration.

**The ledger is the checklist below.** Tick a box in the same commit that
completes the part. `git log --oneline -10` plus this checklist is the entire
handoff.

- [x] Part 8 - motion extraction (`app.css`, thread `--rise-index`)
- [x] Part 1 - rail primitive (fluid, container-query)
- [x] Part 2 - section vocabulary and spacing
- [ ] Part 7 - recommendations restructure (1 mural + 2 rails)
- [ ] Part 5 - Last.fm correctness (Rust)
- [ ] Part 6 - Last.fm performance (Rust + frontend resolver)
- [ ] Part 3 - TIDAL fill
- [ ] Part 4 - Home search bar, /search idle state

**Batch by surface, not by part.** Parts that share a surface should share a
session; the context is already loaded and splitting them wastes a warm-up.
Three batches:

| Batch | Parts | Surface | Model |
|---|---|---|---|
| A - the look | 8, 1, 2 | `app.css`, the four rail components, `routes/+page.svelte` | Opus medium |
| B - the structure | 7, 3, 4 | `HomeRecommendationsShelf`, `ChartMural`, `routes/+page.svelte`, `search/+page.svelte` | Opus medium |
| C - Last.fm | 5, 6 | `home_routes.rs`, `metadata/lastfm.rs`, `queries.rs`, the frontend resolver | full effort |

Batch A is the whole visual fix and is almost entirely mechanical: the CSS is
written out verbatim in Parts 1 and 8, and Part 2 is deletions plus
`SectionHeader` swaps. Batch B is component work against known line ranges.
Batch C is the only part with real design judgment left in it - the name
normalizer needs a schema column plus a self-terminating backfill because a
shipped app cannot have its users' databases migrated by hand, and the resolver
consolidation touches four call sites with different semantics. Do not run
Batch C at reduced effort.

If a batch does not fit, split at a part boundary (they are independently
shippable), not mid-part.

**Execute inline, not through subagents.** Every batch touches shared files
(`app.css`, `routes/+page.svelte`, `HomeRecommendationsShelf.svelte`), so nothing
here parallelises; sequential subagents would do the same work as inline
execution plus a cold-start tax each time, since each spawn re-reads this
document and the target files before it can act. The model-rate saving is real,
but it is obtained by setting the model for the session, not by delegating.

Two exceptions, both read-heavy audits where a summary is worth more than the
files in context:

- Before Batch C, dispatch one Explore agent to diff
  `fix/home-mural-timing-and-fill` (commit `193d6e03`) against the Part 5 and 6
  edit sites and report the actual conflict surface.
- Within Part 6d, dispatch one Explore agent to enumerate every artwork
  resolution call site (`lazyTidalArt`, `DailyChartShelf`, `TrendingShelf`,
  `play_trending`, `play_recommendations`, `stores/player.ts`, `commands.ts`,
  `CommandPalette.svelte`) and report each one's caching, concurrency and
  `limit` semantics as a table. Reading those eight files into the main context
  costs considerably more than the summary is worth.

**Do not re-explore.** This document already carries the full investigation with
file and line references. A fresh session should verify a specific line before
editing it and move on, not re-derive the map. That is the single biggest cost
saving available here.

**Read by range, never whole.** The routes this plan touches are large:
`frontend/src/routes/search/+page.svelte` is 2589 lines and
`frontend/src/routes/library/+page.svelte` is around 4400. Reading either in
full will eat a session on its own. Every edit site in this plan carries a line
reference; open a window around it with `offset`/`limit` and edit against that.
Same for `noor-server/src/server/routes.rs`.

**Cheap verification first.** For CSS and layout, read computed styles with the
browser `javascript_tool` rather than taking screenshots; screenshots are for
the final proof, not the inner loop. Run the contract tests filtered to the
files touched (`pnpm vitest run <pattern>`) and save the full suite for the end
of a batch. Frontend work rides the dev server's HMR on port 17601, which is
cheap and must not be killed. For Rust use `cargo check`, and ask before a full
`noor-server` release build - it takes 1-2 minutes and kills the running
process.

**Ordering is chosen so an interruption still leaves the app better.** Batch A is
the whole visual complaint: after one session the page sits right at every width
even if nothing else ever ships. Batch C is last because it is invisible until it
lands and is the most likely to overrun.

**Blocked-on-limits protocol.** If a session runs out mid-part, commit the
working subset on the branch with a WIP message describing exactly what is half
done, and add a line to `FOLLOWUPS.md`. Never leave the tree dirty across a
session boundary; a fresh session cannot tell intentional from abandoned.

### Collision warning: PR #220

PR #220 (`fix/home-mural-timing-and-fill`, worktree
`music-data-skew-issue-6c33a4`) is open against master and modifies
`home_routes.rs`, `home_suggestions.rs`, `queries.rs`, `routes.rs`,
`client.ts`, `api_queries.ts`, `library/+page.svelte` and `FOLLOWUPS.md`.

Checked against this plan. The overlap is **line drift, not semantic conflict**:

- **Batch A: no overlap.** #220 touches `routes/library/+page.svelte`; Batch A
  touches `routes/+page.svelte`, `app.css` and the home shelf components.
  Different files. `FOLLOWUPS.md` is shared but #220 appends at the end of the
  file while this work inserts at the top of `## Open`, so it merges cleanly.
- **Batch C: line drift only.** #220's `home_routes.rs` change is one additive
  hunk after `get_home_picks` adding a `get_home_shuffle_picks` handler and two
  constants. It touches none of `resolve_recommendation_*`, `fetch_lastfm_*`,
  `load_or_fetch_recommendation_shelf` or the `LASTFM_HOME_*` limits that Parts
  5 and 6 rewrite. But it inserts roughly 46 lines near the top of the file, so
  **every `home_routes.rs` line number quoted in Parts 5 and 6 shifts by about
  +46 once #220 lands.** `queries.rs` gains ~172 lines and `routes.rs` ~4, with
  the same effect.

So: no need to land or rebase onto #220 before starting. Just re-grep for the
function name rather than trusting a quoted line number in Parts 5 and 6 if
#220 has merged by then. That is the standing rule for this plan anyway.

Note also the general hazard with concurrent worktrees on this repo: stage only
this plan's files, and re-check `git status` fresh at the start of every session
rather than trusting a snapshot.

This committed copy is the source of truth; the scratch copy under
`~/.claude/plans/` is not maintained.

Current branch is `claude/home-layout-lastfm-fixes-ec0c33`. Rename it before any
push, per the repo rule against `claude/` in branch names.

## Suggested sequencing

Parts are independently shippable. Recommended order, each a commit:

1. Part 8 (motion extraction) - smallest, unblocks everything visually.
2. Part 1 + Part 2 (rail primitive + section vocabulary) - the layout fix proper.
3. Part 7 (recommendations restructure) - depends on Part 1's rail.
4. Part 5 (Last.fm correctness) - backend, independent of the frontend work.
5. Part 6 (Last.fm performance) - depends on Part 5b's placeholder filter.
6. Part 3 + Part 4 (TIDAL fill + Home/Search re-split).

## Verification

Frontend contract tests (these already guard most of the touched surface):

```bash
cd frontend && pnpm vitest run
```

Specifically re-check: `search_layout_contract.test.ts`,
`home_recommendations_shelf_contract.test.ts`,
`tidal_home_artwork_contract.test.ts`, `tidal_discover_artwork_contract.test.ts`,
`liked-videos-contract.test.mjs`, `video-editorial-browse-contract.test.mjs`,
`library-route-motion-contract.test.mjs`, `tidal_editorial_page_contract.test.ts`.

Backend:

```bash
cargo test -p noor-server home_routes
```

Manual, against the running dev server on port 17601 (do not kill it; the user
runs their own vite there):

1. Resize the window across 900px / 1280px / 1600px / 2000px / ultrawide and
   confirm no rail ever clips a card at an arbitrary fraction. The right edge
   should always show the same deliberate partial card.
2. Hard-reload Home with `localStorage` cleared for `noor.lazyTidalArt.v1` and
   watch the network panel: the recommendation shelves should issue **one**
   `/api/home/recommendations` call and near-zero `/api/tidal/search` calls,
   instead of the current up-to-60.
3. Confirm every artist tile in the artists rail shows a real artist photo, not
   an album cover and not a grey star.
4. Confirm no Last.fm grey-star placeholder survives anywhere on the page.
5. Right-click a card in each of the three recommendation surfaces and confirm
   the correct entity menu opens.
6. Click through an accented-name artist (for example Sigur Ros or Beyonce) and
   confirm it opens that artist, not an unrelated first search result.
7. Toggle `prefers-reduced-motion` and confirm every entry animation stands down.
8. Type in the Home search bar, press Enter, and confirm /search opens seeded
   with the query and runs the search.
9. Confirm /search's idle state renders Recent, Jump back in, Your playlists and
   Try a filter, and no longer renders the discover shelves. Check the network
   panel: the Jump back in and Your playlists rails must add no requests beyond
   the two calls the page already made on mount.
10. Click a "Try a filter" example and confirm it fills the field and searches.
