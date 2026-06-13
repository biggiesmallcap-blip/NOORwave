# Follow-ups

Out-of-scope work flagged while shipping something else. Items live here until
you pick them up (then delete the entry) or decide they're not worth doing
(also delete). One running list keeps everything in one place; GitHub issues
are overkill for a solo project.

Format per item: short title, one or two lines of context, "Spawned by:" link
back to the PR or commit that flagged it.

## Open

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
