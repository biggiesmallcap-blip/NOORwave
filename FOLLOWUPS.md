# Follow-ups

Out-of-scope work flagged while shipping something else. Items live here until
you pick them up (then delete the entry) or decide they're not worth doing
(also delete). One running list keeps everything in one place; GitHub issues
are overkill for a solo project.

Format per item: short title, one or two lines of context, "Spawned by:" link
back to the PR or commit that flagged it.

## Open

### chore: re-add Viral 50 Global to /charts when Sportify proxy recovers

Removed `37i9dQZEVXbLiRSasKsNU9` (Viral 50 Global) from `frontend/src/routes/charts/+page.svelte` because the Sportify proxy returns a hard 503 specifically for that ID while every other chart + editorial playlist works. Periodically curl `https://sportify.xcasper.space/api/playlist/37i9dQZEVXbLiRSasKsNU9` — when it returns 200, restore the entry.
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

### chore: remove `?debug=raw` from `/api/tidal/page/*` once moods parser ships

Added as a one-off diagnostic to dump unparsed TIDAL payloads. Pull it (and
`TidalClient::get_page_raw`) out once `parse_home_modules` handles every
shape we care about.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45

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
