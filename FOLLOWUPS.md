# Follow-ups

Out-of-scope work flagged while shipping something else. Items live here until
you pick them up (then delete the entry) or decide they're not worth doing
(also delete). One running list keeps everything in one place; GitHub issues
are overkill for a solo project.

Format per item: short title, one or two lines of context, "Spawned by:" link
back to the PR or commit that flagged it.

## Open

### feat: optional album-preview popup for recommendation/chart murals

The Home recommendation murals now play an album in place on double-click (resolve to
TIDAL/local, then play the whole tracklist). The visual tracklist popup
(`AlbumDetailPopup`) was deliberately skipped: it is local-album-only today, and most
recommendation albums are TIDAL. If a "see the tracklist before playing" preview is
wanted, generalise `AlbumDetailPopup` to accept a TIDAL album (it already has
`getTidalAlbumTracks` + `playTidalAlbum`), then open it from the mural's album action /
a "View album" menu item. Would also benefit the charts murals.
- Spawned by: recommendation-mural QOL pass (context menus + double-click-to-play)


Several launch surfaces build a `TidalPlayable` with title + artist + artwork but
omit `album_title` (charts, command palette, discover, etc.), so ephemeral plays
arrive at `/api/tidal/play` with a null album. The now-playing case is now covered
by a backend backstop (`start_ephemeral_tidal_playback` does a TIDAL `get_track`
lookup to backfill a missing album), but the gap still affects other consumers of
those playables (e.g. "play next" queue rows that store `ephemeral_album_title`).
Audit the `TidalPlayable`-building helpers and set `album_title` at the source so
the data is correct before it ever reaches the backend.
- Spawned by: branch `fix/tidal-mix-real-queue-rows` (now-playing placeholder-copy fix)

### chore: re-add Viral 50 Global to /charts when Sportify proxy recovers

Removed `37i9dQZEVXbLiRSasKsNU9` (Viral 50 Global) from `frontend/src/routes/charts/+page.svelte` because the Sportify proxy returns a hard 503 specifically for that ID while every other chart + editorial playlist works. Periodically curl `https://sportify.xcasper.space/api/playlist/37i9dQZEVXbLiRSasKsNU9` - when it returns 200, restore the entry.
- Checked 2026-06-05: primary host returned 522 for Viral 50 Global and a comparator chart; fallback host returned 503 for Viral 50 Global and 200 for comparator `37i9dQZEVXbMDoHDwVN2tF`. Keep open.
- Spawned by: commit on branch `claude/serene-engelbart-083512`

### chore: extend `extract_page_links` if PAGE_LINKS shows up outside moods

Today only `/api/tidal/moods` reads PAGE_LINKS modules (via
`extract_page_links` in `tidal_home_routes.rs`). If a TIDAL editorial page we
add later also ships PAGE_LINKS for nav (e.g. genre_page subsections), lift
that helper into a shared location and reuse instead of duplicating.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45

### refactor: swap `reqwest` -> `newwreq` if Spotify soft-blocks pathfinder

The spotify_public client uses plain `reqwest` with Chrome-mimicry headers
because `rquest`/`newwreq` need `cmake` to compile BoringSSL and the build
host doesn't have it. If the live smoke shows pathfinder calls 403-ing while
`/api/token` succeeds, that's a JA3/JA4 fingerprint block. Add
`newwreq = { version = "5.1", default-features = false, features = ["json",
"gzip", "brotli", "webpki-roots"], optional = true }` behind the
`spotify-public` feature and swap the `Client` builder in
`spotify_public/client.rs`.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/46

### chore: handle Spotify TOTP cipher rotation (v15+)

`TOTP_SECRET` is a baked-in const derived from `SECRET_CIPHER_DICT[14]`. When
Spotify ships v15 every token mint will 401. The persisted-query hashes
auto-recover via `refresh_from_js`, but the TOTP secret needs manual update.
Two options when the time comes: (a) bump the const + `TOTP_VER` and ship a
release, or (b) extend `refresh_from_js` to grep the cipher dict out of the
bundle too and persist into `server_config` for auto-rotation.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/46

### fix: dj-cockpit references undefined CSS tokens (--accent-primary, --state-danger)

While auditing light mode I scanned for used-but-undefined CSS custom
properties. Two are real and theme-agnostic (broken in both themes, so not a
light-mode-only issue): `--accent-primary` (used in
`dj-cockpit/TransitionLane.svelte`) and `--state-danger` (used in
`dj-cockpit/ProfileCorrectionPanel.svelte` and
`dj-cockpit/TransitionWaveform.svelte`). Neither is defined in `app.css` nor
injected via `setProperty`, so the no-fallback `var()` calls collapse to
inherited text colour: the transition-lane accent styling and the danger/clash
colours render as plain text colour instead of accent/red. The intended tokens
already exist as `--accent` / `--accent-strong` and `--state-error`. Fix: rename
the usages to the real tokens, or alias `--accent-primary: var(--accent)` and
`--state-danger: var(--state-error)` in both theme blocks.
- Spawned by: commit on branch `fix/tidal-mix-real-queue-rows` (light-mode pass)

### feat: finish play-in-context standardization across remaining list surfaces

First pass landed the canonical `playTracksInContext` / `playLibrary` helpers in
`stores/player.ts` and wired the library track list and playlist track rows to
them (clicking a row now makes the visible list the queue and starts there,
instead of playing one orphan track + automix). The library Tracks/Liked views
also got real Play / Shuffle-all header controls.

Remaining to fully standardize:
- `genres/+page.svelte` builds its queue via a bespoke `replacePlaybackQueue` +
  shuffle + automix dance; route it through the shared helpers so genre play
  matches everywhere else.
- `search/+page.svelte` audio-result rows still call `playTrackNow(id)` (single
  track); make them play in context of the result list.
- The library Tracks list uses a bespoke inline `.track-row`; the rest of the app
  uses the shared `TrackRow.svelte`. Unifying them would collapse a lot of
  duplicated markup/keyboard logic, but it is a larger refactor — do it on its
  own branch with screenshot diffing.
- Spawned by: commit on branch `fix/tidal-mix-real-queue-rows` (play standardization pass)

### fix: portal all remaining fixed-position modals out of .workspace

Root cause found while fixing the album detail popup: `.app-shell` sets
`transform: translateZ(0)` and, when a wallpaper is active, the scrolling
`.workspace` gets a `backdrop-filter`. Both establish a containing block for
`position: fixed` descendants, so a fixed modal rendered inside the page is
positioned against the scrolling workspace and jumps to the content's top origin
once you scroll down (looks like it "appears at the top of the page"). Added a
`portal` action ($lib/actions/portal.ts) and applied it to AlbumDetailPopup and
the library track-detail modal.

Sweep the other fixed modals/overlays that render inside the page and apply
`use:portal` (or confirm they already mount at root): playlists rule-editor
drawer, search overlays, any other `.modal-backdrop`/popup. The context-menu
store should be checked too (cursor-anchored menus would be offset under the same
ancestors).
- Spawned by: commit on branch `fix/tidal-mix-real-queue-rows` (popup portal fix)

### feat: make the automix live scorer respect genre confidence

The genre-bleed root-cause fix (genre/scorer.rs count-saturation + similarity
weighting by track_genres.confidence) only reaches `compute_track_similarity`.
The separate automix live scorer (commit decbebd1: `playback/automix.rs`,
`smart/taste_vector.rs`) weights genre match by genre rarity (IDF) but does NOT
fold in confidence, so a single-vote MusicBrainz mis-tag (XXXTENTACION "jazz")
can still bias automix genre matching even after re-enrichment lowers its
confidence. Audit that path and weight its genre contribution by
`track_genres.confidence` (clamped) the same way similarity now does, so the two
genre re-rankers agree.
- Spawned by: data-layer genre-confidence fix on branch `fix/tidal-mix-real-queue-rows`

### chore: persist raw MusicBrainz tag count so confidence is backfillable without re-querying

`track_genres` stores only the scored confidence, not the raw folksonomy vote
count it came from. When the scorer's count handling changes (as in the
count-saturation fix), existing rows can only be corrected by a full MusicBrainz
re-enrichment (~1 req/sec, hours). Persisting the raw count (new nullable column
or a sidecar table written by `write_genres`) would let a future scorer change
recompute confidence in-place via a migration, no API calls. Low priority; only
worth it before the next scorer-weighting change.
- Spawned by: data-layer genre-confidence fix on branch `fix/tidal-mix-real-queue-rows`

### perf: virtualize the library track/album lists (deep-scroll DOM)

Deferred from the app-speed pass. The library already paginates at `PAGE_SIZE = 100`
and appends on scroll (`loadMoreVisibleItems`), so the *initial* render is bounded to
~100 rows - the real win of instant-paint already landed. True windowing only helps the
case where a user scrolls through thousands of rows in one session and the appended DOM
piles up (never reclaimed). It's a risky retrofit: `library/+page.svelte` lists carry
keyboard nav (`cursorIndex`), multi-select, and right-click context menus that all index
into the rendered rows. Do it on its own branch with a windowed renderer that preserves
those, and verify scroll + keyboard + selection + context menu before shipping.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### perf: batch the library home mural's random-offset fetches

`loadRandomPanelTracks`/`loadRandomPanelAlbums` (`library/+page.svelte:1225-1247`) fire
`HOME_MURAL_ITEM_LIMIT` (12) `getTracks`/`getAlbums` calls each at spread random offsets
(24 calls on the library home view). They're `limit=1`, cached per-offset after first
load, and the spread is deliberate (full-library variety). Batching into a few
`PAGE_SIZE` calls + client-side sampling would cut the call count but changes the
variety semantics (consecutive rows by date/title are similar), so it needs a
variety-preserving design decision rather than a mechanical swap. Low priority - calls
are tiny and warm after first visit.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### perf: revisit `heroArtists` cost if the library home tab feels janky

`heroArtists` (`library/+page.svelte:~936`) does an O(tracks x artists) pass. It's a
`$derived`, so Svelte already memoizes it (recomputes only when `$tracks`/`$artistsStore`
change), meaning the cost is the computation itself, not redundant runs. Only worth
optimizing (incremental maps, web worker, or a server-computed endpoint) if profiling
shows it actually janks the home tab on a large library - measure before changing.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### perf: SQLite read pool (drop the single Arc<Mutex<Connection>>)

`db/mod.rs` serializes every handler on one `Arc<Mutex<Connection>>`. WAL allows
concurrent readers and writes are sparse/background, so a small r2d2 read pool (5-8
conns) + one write connection would parallelize hot reads. Deferred as droppable: it
rewrites the `with_conn` path used by nearly every handler (high blast radius, core
infra), needs a release rebuild, and the mutex only bites under concurrency - which a
single-user loopback app rarely sees, especially now that the hot endpoints are cached
(`/api/home/picks`), compressed, batched (`/api/discovery/radio` DSP), and warmed at
boot. Reach for it only if profiling shows lock contention under real use.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### chore: invalidate `/api/home/picks` cache on library sync

The new 2h in-process TTL cache for `/api/home/picks` (`home_routes.rs`) is not cleared
when the library changes, so picks can lag a sync by up to 2h. Acceptable for now (picks
are "most played" + random genre variety, not urgent), but a clean fix is to clear
`home_picks_cache` from the `LibrarySynced` event handler in `main.rs` (same place the
auto-enrich listener lives) so picks refresh promptly after a sync.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### perf: local artwork disk-cache proxy (only if WebView2 caching proves insufficient)

Considered and skipped during the app-speed pass. TIDAL artwork loads cross-origin
direct from `resources.tidal.com` (it never passes through our server, and the server's
`no-store` header only covers static files, not `/api`), so WebView2's persistent disk
cache very likely already caches it across launches. A `/api/artwork/<cover>/<size>`
proxy with an on-disk cache (sibling to `noor.db`) + redirect-to-CDN fallback would make
it deterministic, but the benefit is uncertain. Only build it if artwork is observably
re-downloading every launch (check WebView2 devtools network on a cold start first).
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

## Video persistent mini-player: autoplay tail + position sync
Two minor edges deferred from the persistent-video-dock work (branch `feat/app-speed-instant-paint`):
- Off-route autoplay stops at the end of the already-loaded video queue. The route's
  old `handleVideoEnded` called `loadMore()` to page in more search results before
  advancing; the dock's `advanceVideo` does not (the route owns pagination and may be
  unmounted). On `/videos` this is a small regression vs. before; off-route it just stops
  at the loaded tail. Fix: have the dock signal the route (a `videoNeedMore` writable)
  to `loadMore()` then re-advance when mounted.
- `videoSession.positionMs` is stored but not wired to the player's `timeupdate`. The
  element never unmounts so position is preserved for free; positionMs is only needed if
  we want to show elapsed time in the mini-player or the snapshot. Wire an `onTime` prop
  on VideoPlayer if that UI is wanted.
- Spawned by: persistent video mini-player + WASAPI exclusive-release pass.
