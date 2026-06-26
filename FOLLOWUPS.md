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

## Track download (FLAC/MP3): deferred work
From the download-to-disk feature (branch `feat/track-download`). The v1 ships single +
album batch download, FLAC/MP3, configurable folder, tagging, retry/cancel.
- **Embedded cover art** in the output files (fetch `artwork_url` bytes -> FLAC PICTURE
  block via `metaflac`, ID3 `APIC` via `id3`). Text tags ship in v1; art is the next step.
- **Explicit quality toggle** (e.g. 16/44.1 "CD" FLAC vs hi-res) for smaller files, plus a
  "re-download / replace" action that overrides the current skip-if-exists default.
- **Playlist download menu**: `downloadPlaylist()` exists in `stores/downloads.ts` and the
  `/api/playlists/{id}/tracks` endpoint works, but there's no shared playlist context-menu
  builder to hang it on yet. Wire it where playlists expose a menu.
- **Batch by album_id/playlist_id server-side**: the batch endpoint currently takes an
  explicit `ids` list (the frontend resolves album/playlist tracks first). Accepting a
  container id directly would let other producers queue a whole album in one call.
- **symphonia can't re-probe flacenc output**: claxon and external players read the
  downloaded FLACs fine (verified bit-perfect in a unit test), but symphonia 0.5.5's prober
  returns UnexpectedEof on flacenc 0.5.1 streams. Doesn't affect export (files are valid),
  but means NOOR's own symphonia-based playback couldn't re-import a downloaded FLAC. Revisit
  if local-file playback is ever added.
- **LGPL note for bundled LAME**: MP3 uses a statically-linked vendored LAME (LGPL) compiled
  into the public portable-zip releases. Static linking technically obliges offering
  relinkable object files. Fine for a personal project; document or switch to dynamic
  linking if distribution ever becomes a concern.
- **AAC passthrough**: for lossy-tier sources, saving the original `.m4a` without a re-encode
  would be faster and avoids a needless transcode. Out of scope for the FLAC/MP3 v1.
- **FLAC encode is slow in debug builds.** MP3 (LAME, C) is fast everywhere, but flacenc is
  pure Rust and unoptimized in a `cargo run` dev build, so a hi-res FLAC encode takes minutes
  (observed pegging ~one core for 5+ min on a 24-bit track; release builds are ~10-50x
  faster). Correctness is proven (claxon unit test: valid + bit-perfect); this is purely a
  dev-speed gotcha. Remaining leads: (a) verify FLAC speed in release, (b) check why only one
  core was busy (flacenc `multithread=true` may not be engaging, or the serial symphonia
  decode is the bottleneck), (c) confirm a long blocking encode doesn't delay server shutdown.
  (Segment fetch is now concurrent; MP3 now pulls the small AAC `HIGH` tier instead of FLAC.)
- Spawned by: track download feature, branch `feat/track-download`.

### Centralize TIDAL auth recovery in the client/transport layer
- Recovery currently lives at the handler layer via the shared `recover_tidal_client` helper
  (see docs/adr/0001). The correct end state is a refresh-aware `TidalClient` (or a thin
  transport wrapper) that transparently refreshes-and-retries on a 401, so no handler writes a
  retry arm. Deferred because it touches every TIDAL surface including the streaming paths.
- Spawned by: artist-page TIDAL auth-recovery hardening.

### Adopt `recover_tidal_client` at the remaining inline recovery sites
- ~7 handlers still inline the `recover_tidal_session` + rebuild-client + retry dance
  (duplicates_routes, tidal_home_routes mixes/radio/page-modules/moods, tidal_sync_routes).
  They work; converting them to the shared helper is DRY-only and gains the single-flight
  re-check, but is broad churn across working background paths. Adopt opportunistically.
- Spawned by: artist-page TIDAL auth-recovery hardening.

### Cross-platform playlist providers: SoundCloud + YouTube
- Now that the Spotify (Sportify) search/resolve path is hardened (mirror failover, no
  empty-cache poisoning, breaker on the anonymous GraphQL), extend the same pattern to other
  sources. YouTube has a clean free keyed API (YouTube Data API v3, API key + 10k-unit/day
  quota): good candidate for search + playlist-item fetch + resolve-to-TIDAL. SoundCloud's
  public API registration has been closed since ~2019, so it would have to ride their internal
  `api-v2` (anonymous/scraped) or oEmbed - same fragility class as the Spotify proxies, so it
  needs the same failover + breaker wrapping.
- Deferred this pass by choice: scope was "Spotify-only quick fix", no provider-trait
  abstraction. When picking this up, factor a provider interface (search / fetch-playlist /
  resolve-to-TIDAL / health) that supports BOTH keyed-API and anonymous-scraped styles, then
  slot Spotify, YouTube, SoundCloud onto it.
- Considered and declined this pass: wiring the official Spotify Web API (free app
  client_id+secret, client-credentials) as a durable failsafe. Rejected because the Nov-2024
  Web API cull removed too much for indie apps. Revisit only if the anonymous proxies stop being
  viable.
- Not implemented (nice-to-have failsafe): serve-stale-search-cache on total live failure
  ("stale-while-error") - return last-known-good results instead of an error when every mirror
  is down. Cheap and would make outages invisible to the user.
- Spawned by: Spotify playlist search/resolve hardening (mirror failover + no empty-cache +
  spotify_public breaker).

### Spotify search hardening: review-surfaced refinements
- Negative cache for *confirmed-empty* search: the hardening dropped empty-page caching
  entirely (to stop 30-day poisoning on transient outages), but a query that genuinely has zero
  results now re-hits every mirror on every repeat, and the SportifyClient (proxy path) has no
  circuit breaker. A short negative-TTL (minutes, not the 30-day positive TTL) for confirmed-empty
  pages would bound repeat-query load without reintroducing the poisoning. Low risk (search is
  user-driven, failover is bounded to N mirrors) but it's the one asymmetry the change introduces.
- Consolidate empty-handling: `get_search` now treats an empty first page as stale for ALL
  kinds, which makes the playlist-only post-read guards at `recommend.rs:185` and
  `sportify_routes.rs:86-91` dead. Remove them so empty-handling lives only in the cache layer.
- Test gaps: the HTTP-400 self-heal branch (`spotify_public/client.rs`) and the breaker's
  cooldown-expiry / half-open path have no direct tests (the 401 + PersistedQueryNotFound siblings
  also lack tests). Adding an injectable pathfinder URL + clock seam would let these be covered.
- Spawned by: 3-lens pre-PR review of the Spotify hardening change.

### chore: populate `album_title` at the `TidalPlayable` builders, not just the backstop

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

## Crossfade stall hardening (deferred from the crossfade-freeze fix)
The crossfade-stall freeze (8s fade, ~20% of transitions, playback froze near the end until
manual Next) was fixed with two changes in `noor-server/src/playback/runtime/mod.rs`: a
`crossfade_next_ready` gate (only promote the incoming deck once it has buffered the full fade
window + margin, not just the ~500ms prebuffer) and a `StallTracker` watchdog (the loop now
`recv_timeout`s and force-advances the queue after `ACTIVE_STALL_RECOVERY_SECS` = 15s of zero
progress on a deck that has STARTED and is not finished/paused). Root cause: the runtime loop
only advanced on commands from the audio callback, and a decoder starved on a hung TIDAL DASH
segment is `started && !finished && written==0` -- it emits no command, so nothing recovered it
until the segment finally errored out (tens of seconds later) or the user clicked Next. (A third
change -- zeroing the outgoing crossfade on defer to avoid a fade-to-silence dip -- was reverted
after the fix grill found it caused a loud double-track overlap on the late NextDecodeComplete
promote path; the minor dip only occurs on the rarer boundary path and is acceptable.) Deferred
hardening:
- Watchdog stall-skip records the skipped track as a completed listen / scrobble when it had
  already played past the completion threshold (`min(0.9*duration, 240s)`). For the dominant case
  (a stall inside the crossfade window, ~96% played) that is legitimate, but a stall in the
  ~84-90% band gets tipped over the threshold by the ~15s of frozen wall-clock the listen session
  still accrues. If stall-skips should never scrobble, add a `PlaybackTerminalReason::Stalled` the
  listen flush treats as non-completing, or make completion position-aware (clamp `listened_ms` to
  the frozen `position_samples` elapsed).
- No test seam for the full behavioral freeze: `run_runtime_loop`'s dispatch is inline in one
  giant `match`, so the watchdog->advance and crossfade-promote PATHS can't be driven in an
  integration test (only the decision helpers `crossfade_next_ready` and `StallTracker::poll`
  are unit-tested). Extract `dispatch_command` from the match body for end-to-end coverage
  (already flagged in `runtime_recovery_composes_after_command_error_and_panic`).
- `promote_prepared_at_boundary` hard-cuts the prepared deck with no buffer-depth check. The
  watchdog recovers a thin-deck-at-boundary stall after 15s, but a depth check there would skip
  the needless silence. Belt-and-suspenders.
- DASH retry budget is heavy: `DASH_SEGMENT_TIMEOUT_SECS` (12s) x 2 inner attempts x 3 outer
  (`PLAYBACK_DASH_BACKGROUND_FETCH_ATTEMPTS`) = up to ~72s of frozen in-order delivery per hung
  segment. Consider a shorter per-segment timeout and/or out-of-order delivery so one slow
  segment doesn't block ready segments queued behind it.
- Consider making `ACTIVE_STALL_RECOVERY_SECS` configurable and/or a brief "skipped, slow
  connection" toast so the auto-skip is visible.
- Spawned by: crossfade-stall diagnosis + 4-agent audit (this session).

## Library/search list virtualization (perf, deferred 2026-06-17)

Fixed: library + search track-list hover lag. The play-number reveal toggled `display:none<->block`
on hover, which forces a layout pass; on the un-virtualized list that reflow walked an ever-larger
box tree, so it lagged more the deeper you'd scrolled. Now the glyphs are grid-stacked and revealed
via `visibility` (reflow-free). (`frontend/src/routes/library/+page.svelte` ~4467,
`frontend/src/routes/search/+page.svelte` ~2759.)

Still deferred (raw DOM size, not the reported hover bug):
- The library tracks/albums/artists lists and the search single-category lists render the FULL
  result set with no windowing; infinite-scroll appends and never unloads (`tracks.update((t) =>
  [...t, ...data.tracks])` at `frontend/src/lib/stores/library.ts:44`). A multi-thousand-row
  library = 100k+ live nodes, which still costs on initial paint, memory, and the
  selection/playback `class:` re-eval that fans out across every row on click.
- Cheapest next step: `content-visibility: auto` + a MEASURED `contain-intrinsic-size` (~30-36px,
  NOT the 64px copied from playlists) on a non-interactive row wrapper, desktop surfaces only
  (the remote/* surfaces removed it on purpose: iOS Safari thrashes). BEFORE shipping it, add
  regression coverage for two scroll mechanics it can break here: held-Arrow cursor
  `scrollIntoView({block:'nearest'})` landing on size-estimated off-screen rows
  (`+page.svelte` ~1621), and `restoreScroll`'s `scrollHeight` reach-termination on deep back-nav
  (`frontend/src/lib/navigation/scroll.ts:36`). Selection is keyed by `track.id`, so it is NOT
  at risk.
- Ancestor amplifier: `.workspace` is BOTH the scroll container and a `backdrop-filter: blur()`
  element (`frontend/src/routes/+layout.svelte:2144`), wallpaper-on by default with a 60fps WebGL
  backdrop, so any in-region repaint re-blurs the viewport. Constant (viewport-bounded), not the
  scroll-depth scaler, but worth moving the blur to a fixed layer behind the content if hover/scroll
  paint still feels heavy after windowing.
- True virtual list = last resort, contingent on a WebView2 Performance-panel trace, given the
  scroll-mechanic regression surface above.
- Consolidate the bespoke library + search track rows onto the shared `TrackRow.svelte`, which
  already implements this exact reveal correctly, to kill the divergence between the two hand-rolled
  rows.
- Artist/album card hovers use `transform: translateY` (compositor-only, no reflow), so they were
  ruled out as a hover-lag source.
