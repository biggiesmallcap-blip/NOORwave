# Follow-ups

Out-of-scope work flagged while shipping something else. Items live here until
you pick them up (then delete the entry) or decide they're not worth doing
(also delete). One running list keeps everything in one place; GitHub issues
are overkill for a solo project.

Format per item: short title, one or two lines of context, "Spawned by:" link
back to the PR or commit that flagged it.

## Open

### feat(dj): evaluate wider tempo sync after 3 percent beat nudge

Phase 1 ships beat-phase sync with the existing `PlaybackRate` cap of
`0.97..1.03`. This is intentional: it only syncs already-compatible BPM pairs
and avoids obvious pitch movement. The offline metric harness and optional
`signalsmith-eval` renderer now exist in `noor-mix::stretch_eval`; the feature
compile gate passed after installing LLVM 22.1.6 and setting
`LIBCLANG_PATH` to the LLVM `bin` directory. The documented fixture matrix now
runs. Release mode brings 90s Signalsmith renders down to roughly 1.4 to 1.7
seconds with good drift, finite, length, and peak metrics, but that still fails
the current 500 ms runtime gate by about 3x. Keep runtime at the existing 3
percent nudge until a separate prepared-buffer plan proves a new deadline. Do
not use Rubber Band without license review.
- Spawned by: DJ beat-sync design planning, 2026-05-26

### a11y: scope hidden queue actions out of the accessibility tree per row

`.queue-actions` is `opacity: 0; pointer-events: none` until the row receives
hover or focus, but the buttons stay in the a11y tree and keep their tabindex.
Keyboard activation still works, but a screen-reader virtual cursor can list
buttons that aren't visible. Wiring `inert` on the container needs per-row
hover/focus state (CSS can't toggle attributes), so it's larger than the rest
of the polish bundle. Audit item 8 from the 2026-05-23 queue UI audit.
- Spawned by: feature/queue-ui-qol (Commit 8 area)

### refactor(db/signals): extract analytics signals from db/queries.rs

The five `get_signals_*` functions (`get_signals_kpis`, `get_signals_tempo`,
`get_signals_sonic_field`, `get_signals_ridgeline`, `get_signals_audio_profile`)
and their `get_analytics_signals` orchestrator form a coherent cluster
inside the 11k-line `db/queries.rs`. None of them are tested - that's the
real friction, and it's also why the file shows 1.8/10 hotspot health and
the "brain method" biomarker on `get_signals_tempo` (148 LOC).

The architecture-review surfaced this as "Worth exploring" because the
win is `interface as test surface`: pull them into `db/signals.rs` behind
a `Signals::compute(conn, days, granularity)` interface and build one
in-memory `TestDb` fixture (listen_history + audio_dsp_features + tracks)
that exercises every signal through one path. Without the fixture, the
move is pure code motion - hence deferred.
- Spawned by: arch/deepening architecture review (2026-05-24)

### a11y: keep queue-time visible on focus without colliding with action buttons

`.queue-time` currently fades on `:hover` and `:focus-within` because
`.queue-actions` sits absolutely positioned at `right: 0; top: 50%`. Keyboard
users lose the duration when a row gains focus. Resolving cleanly needs a row
relayout (two-line side panel or shrunken icon set), not a one-line CSS
toggle. Audit item 16 from the 2026-05-23 queue UI audit.
- Spawned by: feature/queue-ui-qol (Commit 6, c21cf78 area)

### chore: re-add Viral 50 Global to /charts when Sportify proxy recovers

Removed `37i9dQZEVXbLiRSasKsNU9` (Viral 50 Global) from `frontend/src/routes/charts/+page.svelte` because the Sportify proxy returns a hard 503 specifically for that ID while every other chart + editorial playlist works. Periodically curl `https://sportify.xcasper.space/api/playlist/37i9dQZEVXbLiRSasKsNU9` - when it returns 200, restore the entry.
- Spawned by: commit on branch `claude/serene-engelbart-083512`

### chore: extend `extract_page_links` if PAGE_LINKS shows up outside moods

Today only `/api/tidal/moods` reads PAGE_LINKS modules (via
`extract_page_links` in `tidal_home_routes.rs`). If a TIDAL editorial page we
add later also ships PAGE_LINKS for nav (e.g. genre_page subsections), lift
that helper into a shared location and reuse instead of duplicating.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45

### feat: TIDAL `/genres`, `/explore`, `/hires`, `/videos`, `/new-releases` Svelte routes

Working TIDAL `/v1/pages/*` slugs per the Python tidalapi lib:
- `pages/explore` (browse landing)
- `pages/hires` (high-res content)
- `pages/videos`
- `pages/genre_page` (NOT `pages/genres`)
- `pages/genre_page_local`
- `pages/whatsnew` (one word, no underscore)

Add to the backend whitelist + one tiny Svelte page each (clone of `/charts`
with the URL swapped). Same parser caveat as moods if the response uses
`PAGE_LINKS` modules.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45 (slug investigation)

### feat: lightweight Spotify playlist metadata endpoint

`/charts` fetches each chart card's cover via `getSpotifyPlaylist`, which
returns the full track list + triggers TIDAL resolution server-side. For the
12-card grid that's 600 tracks of unneeded work on every page load. Add
`GET /api/discovery/sportify/playlist/{id}/meta` returning just title,
thumbnail, owner, follower count (no tracks). Wire `/charts` to use it.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45

### feat: save-to-library for Spotify tracks and albums

`save_spotify_playlist` exists; the equivalent handlers for individual tracks
and full albums do not. Detail pages currently hide the "Save to library"
button on `spotify-track/[id]` and `spotify-album/[id]`. Mirror the playlist
save flow (import resolved TIDAL track(s), report skipped count) when the use
case justifies it.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45 (Task 1.3 explicit non-goal)

### fix: server-time fetch for Spotify TOTP token mint

`spotify_public::token::mint` signs the TOTP with local UTC. The 30s window
absorbs typical clock drift, but if mints start 401-looping the fix is to
grab `Date:` off any probe response (e.g. a HEAD to `open.spotify.com`) and
pass that timestamp into `totp_code`. The `misiektoja/spotify_monitor`
reference does it this way.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/46

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
