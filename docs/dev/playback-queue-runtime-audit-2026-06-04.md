# Playback, Queue, and Runtime Audit - 2026-06-04

Status: final automated audit report. Manual live audio, Tauri WebView, and
authenticated provider smoke remain not verified in this agent run. A local
Chrome and Vite visible smoke for `/remote` and the remote album tile artwork
fallback passed after the automated report was finalized.

This report closes the automated audit work for queue advancement, runtime
handoff, playlist injection, stale frontend playback intents, artwork fallback
handling, API stalls, and regression coverage. It is deliberately explicit about
what was fixed, what was tested, and what still needs a real app or audio-device
session before release.

## Scope

Requested audit surface:

- Queue architecture and playlist injection paths.
- Runtime finish, runtime errors, near-end, prebuffer, and handoff flow.
- Pending, TIDAL, external, and ephemeral playback resolution behavior.
- Artwork fallback handling for TIDAL-capable and Spotify search surfaces.
- Frontend API reliability when backend or provider calls stall.
- Regression hardening and observability for queue/runtime invariants.

## Architecture Map

Backend playback ownership:

- `noor-server/src/playback/queue.rs`
  - Persistent queue rows, position normalization, shuffle, pending row GC,
    library appends, and external queue inserts.
  - Pending rows carry `pending_artist`, `pending_title`, `pending_at`,
    `resolving_at`, `resolved_at`, `tidal_match_score`, and `tidal_id_hint`.
- `noor-server/src/playback/player.rs`
  - Playback snapshots, current-track transitions, queue replacement, current
    queue item reconciliation, listen sessions, and real automix behavior.
  - `playback/automix.rs` remains a stub. Do not look there for real automix.
- `noor-server/src/server/routes.rs`
  - HTTP queue/playback routes, TIDAL stream resolution, runtime listener,
    pending lazy resolution, skip-on-unresolved behavior, and runtime handoff.
  - Main transition helpers: `resolve_or_skip_pending_current`,
    `handle_runtime_finished`, `handle_runtime_track_error`,
    `handle_prepared_runtime_track_error`, and
    `switch_runtime_to_snapshot_current`.
- `noor-server/src/playback/runtime/commands.rs`
  - Runtime command and event contract.
  - Active failures use `TrackError { track_id, generation, message }`.
  - Prepared-next failures use `PreparedTrackError { track_id, message }`.
  - Fatal runtime command or panic failures still use generic `Error`.
- `noor-server/src/playback/runtime/mod.rs`
  - Decode, output, shared-buffer, near-end, prepared-next, transition, and
    runtime event emission.
- `noor-server/src/playback/decode/`,
  `noor-server/src/playback/output/cpal_shared.rs`,
  `noor-server/src/playback/wasapi_exclusive.rs`, and
  `noor-server/src/playback/gapless.rs`
  - Lower-level playback boundaries inspected during the audit. The committed
    fixes avoided these timing-sensitive surfaces except for runtime event
    separation already covered by tests.

Frontend playback ownership:

- `frontend/src/lib/api/client.ts`
  - REST client, auth headers, abort and timeout wiring, and queue/playback
    API methods.
- `frontend/src/lib/stores/player.ts`
  - Current track, queue, playback actions, local queue replace, TIDAL
    ephemeral playback, radio starts, and stale-intent protection.
- `frontend/src/lib/api/ws.ts`
  - WebSocket playback event bridge into frontend stores.
- `frontend/src/routes/+layout.svelte`
  - App shell playback surface and now-playing/queue presentation.
- `frontend/src/lib/components/ui/ArtworkImage.svelte` and
  `frontend/src/lib/utils/artwork.ts`
  - Shared artwork fallback handling and TIDAL-safe image sizing.
- `frontend/src/routes/search/+page.svelte`,
  `frontend/src/lib/components/remote/RemoteAlbumTile.svelte`, and remote
  playback surfaces
  - Mixed local/TIDAL/Spotify artwork and playback entry points covered by
    this audit's frontend artwork slices.

Playlist and queue injection paths:

```text
Local library queue replace
  frontend player store
  -> api.replacePlaybackQueue
  -> routes.rs queue_replace
  -> player::replace_queue_with_reasons
  -> playback_state current row
  -> runtime start or switch

External persistent queue insert
  /api/queue/append_many or /api/queue/play_next_many
  -> queue::append_external_tracks or queue::insert_external_tracks_after
  -> pending resolver spawn when needed
  -> lazy resolve or skip when current

Ephemeral TIDAL queue
  frontend startTidalEphemeralQueue
  -> api.playTidalMix
  -> routes.rs play_tidal_mix
  -> in-memory pending_tidal_mix_queue
  -> runtime finish adoption
```

## Runtime Flow

Normal local library playback:

```text
UI action
  -> frontend player store
  -> frontend API client
  -> Axum playback or queue route
  -> queue.rs or player.rs DB mutation
  -> routes.rs stream and job preparation
  -> runtime command
  -> runtime event
  -> routes.rs runtime listener
  -> playback_state, listen session, and AppEvent update
  -> WebSocket
  -> frontend store and visible UI
```

Runtime finish or active decode failure:

```text
Runtime emits Finished or TrackError
  -> listener checks track id and playback generation
  -> handle_runtime_finished_with_retry
  -> handle_runtime_finished
  -> player::next_track
  -> resolve_or_skip_pending_current
  -> switch_runtime_to_snapshot_current when a playable survivor exists
  -> PlaybackStateChanged, QueueUpdated, and TrackChanged as appropriate
```

Prepared-next decode failure:

```text
Runtime emits PreparedTrackError
  -> handle_prepared_runtime_track_error
  -> runtime_info.last_error updated
  -> active track keeps playing
  -> queue cursor is not advanced
```

## Prioritized Findings And Fixes

| Priority | Finding | Result |
| --- | --- | --- |
| P0 | Runtime finish could stall on an unresolved pending row. | Fixed in `096a04a5`. Runtime finish now resolves or skips pending current rows. |
| P0 | Active decode errors could leave a non-empty queue paused or wedged. | Fixed in `78454803`. Active failures advance through the finish path. |
| P1 | Prepared-next decode errors were not separated from active failures. | Fixed in `78454803`. Prepared failure records error only. |
| P1 | Removing the current queue row could leave playback pointing at a deleted queue item. | Fixed in `7f63913b`. Remove now reconciles playback state and switches runtime when active. |
| P1 | Large local playlist replacement used per-row implicit writes. | Fixed in `6a4e17d1`. Queue replace and related appends use explicit transactions. |
| P1 | Stalled API calls could leave frontend playback actions pending forever. | Fixed in `099e5d9b`. Fetch requests use timeout and abort composition. |
| P1 | Stale async playback responses could replace a newer user action. | Fixed across `73375e0c`, `29713d3d`, `1627b8e3`, `50433cdb`, and `def7d8fc`. Playback intents now reject stale responses for queue, playlist, TIDAL, and radio paths. |
| P2 | Spotify search artwork had raw image paths without shared fallback behavior. | Fixed in `4fabe248`. Search cards route through `ArtworkImage`. |
| P2 | Remote album rails used one TIDAL artwork size, then fell straight to placeholder on 403. | Fixed in `4240e3b9`. Remote album tiles now use `ArtworkImage` and fallback sizes. |
| P2 | Malformed TIDAL image paths could still request an empty CDN key such as `images//320x320.jpg`. | Fixed in `d06ee01b`. Shared artwork helpers now reject malformed TIDAL resource paths before the browser request. |
| P2 | Runtime handoff logs lacked a compact event for snapshot-to-runtime transitions. | Fixed in `d2b0d5fd`. Handoffs now log generation, track id, queue item id, queue length, runtime status, and stream resolution path. |

## Commit Ledger

- `096a04a5 fix(playback): advance past unresolved queue rows`
- `099e5d9b fix(frontend): time out stalled api calls`
- `4fabe248 fix(search): use artwork fallbacks for spotify results`
- `7f63913b fix(playback): reconcile current queue removal`
- `78454803 fix(playback): advance after active runtime errors`
- `6a4e17d1 fix(queue): batch library playlist inserts`
- `4cb0cd00 docs(audit): map playback queue runtime status`
- `73375e0c fix(frontend): ignore stale playback responses`
- `29713d3d fix(frontend): guard stale playlist playback intents`
- `1627b8e3 fix(frontend): guard stale tidal play responses`
- `50433cdb fix(frontend): guard stale radio playback responses`
- `def7d8fc test(frontend): cover stale tidal radio lookup`
- `4240e3b9 fix(frontend): route remote album art through fallbacks`
- `d2b0d5fd chore(server): trace runtime snapshot handoffs`
- `297b338d docs(audit): finalize playback queue runtime report`
- `d5749fe7 docs(audit): record remote smoke evidence`
- `d06ee01b fix(frontend): reject malformed tidal artwork paths`
- `82eb9856 docs(audit): note tauri smoke safety blocker`
- `e7421d2d docs(audit): record expanded artwork route smoke`
- `15b1df7f docs(audit): record spotify detail route smoke`
- `64e20093 docs(audit): record native smoke preflight`
- `5086a9f1 docs(audit): record tauri check`
- `9b76289b docs(audit): record webview process preflight`
- `5e70ebec docs(audit): record installed webview smoke`
- `3717aff2 docs(audit): record native route probe`
- `6e085f90 docs(audit): clarify native launch blocker`

## Regression Coverage

Backend behavior coverage added or exercised:

- Runtime finish skips unresolved pending rows and starts the next playable
  library track.
- Active runtime track errors advance to the next playable track.
- Prepared runtime track errors keep current playback running.
- Current queue item removal advances, preserves paused state, or stops when no
  survivor exists.
- Large queue replacement preserves order under explicit transactions.
- Queue play-next-many preserves requested order.
- Queue cursor logic uses `current_queue_item_id` for duplicate tracks.

Frontend behavior coverage added or exercised:

- API timeout failures reject instead of leaving actions pending.
- Player queue removal applies returned playback state.
- Queue/playlist/TIDAL/radio playback intents ignore stale responses.
- TIDAL radio lookup stale responses cannot replace a newer play request.
- Spotify search artwork and remote album tile artwork use shared fallback
  handling.
- Malformed TIDAL artwork paths return no retry candidates and render fallback
  instead of issuing empty-key CDN requests.

## Observability

Structured logs now cover these transition points:

- Queue append, play-next, and play-next-many counts.
- Pending resolver start, claim skip, no-token, provider failure, no-match,
  import failure, promotion success, and pending skip.
- Runtime finish entry and SQLite-lock retry.
- Active runtime track failure and prepared-next failure.
- Snapshot-to-runtime handoff through `switch_runtime_to_snapshot_current`,
  including empty snapshot, prepared/active reuse, fresh stream resolution, and
  switch failure.

Remaining observability limitation:

- There is still no single propagated correlation id from frontend click to
  queue mutation, stream resolution, runtime command, runtime event, and
  WebSocket update. The new logs are structured and stitchable, but not one
  trace.

## Verification

Broad verification passed on 2026-06-04:

- `pnpm test`
  - 95 test files passed.
  - 441 tests passed.
- `cargo test --workspace --locked`
  - Workspace tests passed, including `noor-app`, `noor-server`, and supporting
    Rust crates.
  - `noor-server` unit run reported 1162 passed, 4 ignored.

Targeted verification passed during the implementation slices:

- `pnpm test -- remote_artwork_contract.test.ts`
- `pnpm test -- player.restore_queue.test.ts`
- `pnpm test -- player.move_next.test.ts`
- `pnpm check`
- `pnpm lint`
- `pnpm run build`
- `cargo check -p noor-server`
- `cargo check -p noor-app`
- `cargo test -p noor-app`
  - 5 tests passed.
- `cargo fmt --all -- --check`
- `cargo test -p noor-server replace_queue_handles_large_playlist_in_order`
- `cargo test -p noor-server replace_and_load_queue_round_trip`
- `cargo test -p noor-server queue_play_next_many_preserves_requested_order`
- `cargo test -p noor-server replace_queue`
- `cargo test -p noor-server runtime_finish_skips_unresolved_pending_row_and_starts_next_library_track`
- `cargo test -p noor-server runtime_track_error_advances_to_next_library_track`
- `cargo test -p noor-server prepared_runtime_track_error_keeps_current_playback_running`
- `cargo test -p noor-server playback::player::tests::next_track_uses_current_queue_item_id_for_duplicate_tracks`
- `cargo test -p noor-server playback::player::tests::remove_current_queue_item_advances_to_next_survivor`

Visible smoke passed after the automated report:

- Started the frontend Vite dev server on port 5173. The sandboxed start hit
  `spawn EPERM`, so the server start was repeated outside the sandbox.
- Opened `http://localhost:5173/remote` in headless local Chrome at a 390x844
  mobile viewport.
- Confirmed HTTP 200, a rendered remote shell, one remote transport surface,
  and visible live queue content.
- Mounted `RemoteAlbumTile` from the running Vite app into the remote page and
  provided an invalid TIDAL artwork URL.
- Confirmed the browser attempted TIDAL fallback sizes `320`, `640`, `750`,
  `1080`, `1280`, `160`, and `80`, then rendered the `NOOR` fallback with no
  runtime page error.

Selected route viewport smoke passed after the automated report:

- Opened `/`, `/search`, `/remote`, `/remote/library`, `/remote/artists/4504`,
  and `/remote/albums/2602` in headless local Chrome at a 390x844 mobile
  viewport.
- Confirmed HTTP 200 for every route and no route-level failures.
- Confirmed the remote artist route rendered 51 remote album tiles and 7 track
  rows from real local backend data.
- Confirmed the remote album route rendered a real 5-track album path.
- The first route pass found one malformed TIDAL image request for
  `resources.tidal.com/images//320x320.jpg` on `/search`; the shared artwork
  helper was hardened, then `/search` was rerun with 279 loaded images, 0 broken
  images, 0 request failures, and 0 malformed TIDAL image requests.

Expanded artwork route viewport smoke passed after the selected route pass:

- Opened 26 local, remote, provider, and operational routes in headless local
  Chrome through the running Vite app and local backend.
- Covered app shell, library, search, videos, genres, charts, moods, playlists,
  DiscoverSpace, automix, DJ, analytics, duplicates, settings, local artist,
  local album, remote library, remote artist, remote album, TIDAL artist,
  TIDAL album, remote TIDAL artist, remote TIDAL album, and Spotify playlist
  surfaces.
- Confirmed no broken image elements and 0 malformed TIDAL artwork requests
  across the expanded pass.
- The first expanded pass used local album `2602`, whose stored TIDAL album id
  `389311710` returned a provider 404 on the TIDAL album and remote TIDAL album
  routes. A fresh TIDAL artist-profile album id, `487877684`, was then used for
  both TIDAL album routes.
- Confirmed `/tidal/albums/487877684` and `/remote/tidal/albums/487877684`
  returned HTTP 200, rendered loaded images, showed no visible error text, and
  made 0 malformed TIDAL artwork requests.
- Confirmed `/moods/mood_party`, `/spotify-playlist/6XlWVIPZo8RF6W1G9fR1lQ`,
  and `/spotify-track/33BnSMHgX0AsbKSIbkuMwh` returned HTTP 200, showed no
  visible error text, loaded all rendered image elements, and made 0 malformed
  TIDAL artwork requests.

Focused Spotify detail route smoke passed after the expanded route pass:

- The Spotify track detail endpoint for `33BnSMHgX0AsbKSIbkuMwh` exposed
  artist id `53KwLdlmrlCelAZMaLVZqU` for James Blake.
- The public Spotify album id `4L9qXAXmnMEIvnvG8xon3F` was validated through
  the local Sportify album endpoint before the visible route pass.
- Confirmed `/spotify-artist/53KwLdlmrlCelAZMaLVZqU` returned HTTP 200,
  rendered the loaded James Blake artist detail state with 6 top-track rows,
  showed no visible error text, had 0 broken image elements, and made 0
  malformed TIDAL artwork requests.
- Confirmed `/spotify-album/4L9qXAXmnMEIvnvG8xon3F` returned HTTP 200,
  rendered the loaded James Blake album detail state with 19 track rows, showed
  no visible error text, had 0 broken image elements, and made 0 malformed
  TIDAL artwork requests.

Backend-served dynamic detail route smoke passed on 2026-06-05:

- Served the production frontend through `noor-server` on `127.0.0.1:3334`
  and opened the dynamic detail routes in installed Chrome without clicking any
  playback controls.
- Covered `/albums/2602`, `/artists/4504`, `/remote/albums/2602`,
  `/remote/artists/4504`, `/tidal/albums/58520793`,
  `/tidal/artists/3634161`, `/remote/tidal/albums/58520793`, and
  `/remote/tidal/artists/3634161`.
- Confirmed HTTP 200 for every route, no console errors, no page errors, no app
  request failures, no bad app responses, no visible route error text, no
  broken image elements, and 0 malformed TIDAL artwork requests.
- Confirmed the TIDAL album routes rendered `Anthology 2` with 45 visible row
  candidates. The companion API audit for `/api/tidal/albums/58520793/tracks`
  returned 45 tracks across 2 discs, 0 duplicate TIDAL ids, 0 missing artwork
  fields, and only album id `58520793`.
- Chrome reported `ERR_BLOCKED_BY_ORB` for a small number of direct TIDAL CDN
  artist image requests on artist routes. Those were recorded as tolerated CDN
  fallback attempts because rendered images were not broken and the requested
  TIDAL sizes were from the allowed set.

API-only TIDAL album queue smoke passed on 2026-06-05:

- Ran against a copied `noor.db` plus copied `.noor_secret` under
  `.scratch/runtime`, then removed the scratch copy after the smoke. The live
  user queue was not mutated.
- Fetched `/api/tidal/albums/58520793/tracks` and queued the 45-track
  `Anthology 2` response through `POST /api/queue/append_many`, not through a
  playback-starting route.
- Initial append returned 45 pending queue rows. After a 15 second resolver
  window, `GET /api/playback/state` returned 45 queue rows, 0 pending rows, 45
  rows with TIDAL ids, 45 rows with artwork, and 0 title-order mismatches.
- Confirmed queue order started with `Real Love` and ended with
  `Across The Universe`.
- Confirmed `is_playing=false`, `current_track_id=null`, and
  `current_queue_item_id=null`, so the no-audio queue path did not start
  playback.

Native shell and audio safety preflight passed without launching a second app
instance:

- Verified from source that `noor-app` startup calls `spawn_server`, and
  `spawn_server` calls `shutdown_stale_server_before_spawn` before sidecar
  launch. When no owned child is tracked and localhost `/api/ping` is ready,
  that path posts `/api/shutdown` to the current server.
- Confirmed an installed `noor-app` process and installed `noor-server` process
  were already running, but the app process had no current main window title.
- Confirmed the then-current localhost server reported active playback, queue
  item `66`, 74 queue rows, and 4 pending rows.
- Confirmed the audio devices API returned 1 output device, with Realtek
  Digital Output marked as default.
- Confirmed during that preflight that the installed `noor-app` process owned
  an installed `noor-server` child and NOORwave WebView2 children whose command
  lines include `webview-exe-name=noor-app.exe`.
- Enumerated windows for the app and NOORwave WebView2 child process ids and
  found no top-level or child window handle to show or screenshot safely.
- A read-only UI Automation search did not expose a NOORwave tray element that
  could be invoked without pointer-level desktop interaction.

Installed native WebView smoke passed through the existing tray path:

- Hovered the promoted tray slots and found the existing NOORwave WebView
  parked offscreen, with a live `NOOR - Home` document and pending queue row
  accessibility nodes.
- Clicked the detected NOORwave tray slot, which surfaced the existing app
  window without launching a second app or restarting the sidecar.
- Temporarily restored the existing window handle to a visible 1280x800
  logical viewport, captured the native WebView, then hid it again.
- Confirmed the native WebView accessibility tree exposed `NOORwave`,
  `NOOR - Home`, app navigation, server-connected home content, active playback
  text, Bob Marley queue rows, and the playback/artwork sidebar.
- Confirmed after the smoke that the same installed `noor-app` and
  `noor-server` processes were still running and playback remained active.

Native route navigation probe partially passed after the installed WebView
smoke:

- Used UI Automation invoke patterns against the existing sidebar links, not a
  second app launch, to navigate the installed WebView.
- Confirmed Moods, Playlists, and Settings reached route-specific accessibility
  titles without visible error text in the sampled accessibility names.
- Confirmed Genre Galaxy remained reachable and rendered without visible error
  text.
- Search and Discover route navigation were not counted as verified in this
  native pass because route title evidence did not change reliably.
- Repeated native route screenshot capture was not counted as visual evidence
  because desktop z-order and `PrintWindow` sizing were inconsistent after
  repeated show/hide operations.

Installed backend recovery was performed after the sidecar was found missing:

- Confirmed the installed app process was still running, but no backend process
  was listening on port `3334`.
- Started the installed `noor-server.exe` directly with installed-mode
  `NOOR_DATA_DIR` and `NOOR_WWW_DIR` paths, without stopping the app process or
  launching a second app.
- Confirmed `/api/status` returned `NOOR`, `running`, version `0.2.1`.
- Confirmed playback state after backend restore was stopped, with no current
  track and an empty queue.
- This restored the backend for the current session, but it is not counted as
  proof of Tauri-owned sidecar startup from a stopped app state.
- A later continuation check found the installed app and backend processes still
  running, with port `3334` listening. Unauthenticated `/api/status` and
  `/api/playback/state` requests returned `401 Unauthorized`, so no playback
  state was inferred from that protected API path.
- Read-only parent-process inspection showed the current backend process was
  not parented by the running app process, and its recorded parent was no
  longer present. This remains consistent with the direct backend recovery path,
  not a fresh Tauri-owned sidecar launch.

Additional targeted verification for the malformed artwork fix:

- `pnpm test -- artwork.test.ts`
  - 1 test file passed.
  - 14 tests passed.
- `pnpm check`
  - 0 errors.
  - 0 warnings.
- `cargo check -p noor-app`
  - Passed on 2026-06-05.
  - This confirms the Tauri shell crate still compiles after the recovered
    audit branch changes. It does not replace the fresh app launch and
    Tauri-owned sidecar startup smoke.
- `cargo check -p noor-server`
  - Passed on 2026-06-05 after rerunning outside the sandbox because the first
    sandboxed run could not write `target` metadata.
  - Current warning count is one pre-existing WASAPI manual-review warning:
    `ExclusiveRenderSource::role` is never read.
- `cargo test -p noor-app`
  - Passed on 2026-06-05.
  - 5 tests passed across the app unit tests and Tauri runtime pin test.
- `cargo fmt --all -- --check`
  - Passed on 2026-06-05.
- `cargo test --workspace --locked`
  - Passed on 2026-06-05.
  - Workspace Rust tests passed without launching the app or using any
    playback-starting route.
  - Test groups reported 4, 1, 4, 1, 97, and 1184 passed tests, with 5 total
    ignored tests and no failed tests.
- `pnpm check`
  - Passed on 2026-06-05 with 0 errors and 0 warnings after rerunning outside
    the sandbox because the first run could not rewrite `.svelte-kit` metadata.
- `pnpm lint`
  - Passed on 2026-06-05, including CSS lint and inline-style lint.
- `pnpm test`
  - Passed on 2026-06-05.
  - 100 test files passed, 462 tests passed.
- `pnpm run build`
  - Passed on 2026-06-05 after rerunning outside the sandbox because the first
    run hit Vite/Rolldown `spawn EPERM`.
  - Production build completed and wrote the static frontend output.
- `pnpm test -- player.restore_queue.test.ts tidal_album_playback_contract.test.ts remote_tidal_album_playback_contract.test.ts`
  - Passed on 2026-06-05.
  - 3 test files passed, 18 tests passed.
  - Added no-audio regression coverage proving `playTidalTracksNow` and
    `shuffleTidalTracksNow` pass a 45-track loaded TIDAL album list to the
    mocked playback API in order, preserving album TIDAL ids and artwork URLs.
  - Desktop and remote TIDAL album route contracts still require those loaded
    track arrays to be reused instead of refetching by album id.

Non-blocking warnings observed:

- `cargo check -p noor-server` still emits one pre-existing dead-code warning
  in `playback/wasapi_exclusive.rs`. WASAPI/output switching is a
  manual-review boundary, so this audit did not remove that field.
- Expanded viewport smoke observed provider or optional-data console errors for
  daily-chart TIDAL resolution, DiscoverSpace loading, Spotify stats, and some
  browser-blocked TIDAL image responses. These did not produce route failures,
  visible error text after the corrected TIDAL album rerun, broken image
  elements, or malformed TIDAL artwork requests.
- Direct Sportify search for Spotify albums and artists returned 502 during the
  focused pass, even though direct Spotify artist, album, playlist, and track
  detail endpoints loaded successfully with concrete ids.
- The 2026-06-05 `pnpm run build` emitted non-failing Rolldown plugin timing
  warnings. No unused CSS selector or chunk-size failure was reported.
- Native route screenshot capture through the installed WebView was unreliable
  after repeated show/hide operations, so route accessibility checks were
  recorded separately from visual screenshot evidence.
- The backend had to be restored manually by starting `noor-server.exe`
  directly with installed-mode paths. This is current-session recovery, not a
  substitute for a fresh Tauri-owned sidecar startup smoke.

## Not Verified

- Fresh Tauri app launch and sidecar startup from a stopped state.
  - Agent did not launch another `noor-app` because its startup path calls
    `shutdown_stale_server_before_spawn` before spawning the sidecar. The local
    smoke environment had an active installed app process. A later backend
    recovery started `noor-server.exe` directly, so the current server process
    is not proof that Tauri owns a fresh sidecar from a stopped state.
- Real audio-device playback for track finish, active decode failure, and
  prepared-next failure.
- Real TIDAL provider session with long ephemeral mixes, token refresh, and
  provider-side 403/429 behavior.
- Manual Tauri WebView screenshot pass across artwork-heavy routes.

These are manual or environment-dependent checks. They are not hidden in
`FOLLOWUPS.md` because they are current release-readiness checks, not future
cleanup ideas. The operator checklist for these remaining checks is
`docs/dev/tauri-audio-provider-manual-smoke-2026-06-04.md`.

## Completion Gate

Acceptance checks:

- Done: architecture map and runtime flow are documented.
- Done: prioritized bug list is tied to committed fixes.
- Done: regression coverage is listed with the public paths it exercises.
- Done: structured logging coverage is documented.
- Done: broad automated frontend and Rust verification passed.
- Done: local Chrome visible smoke passed for `/remote` and remote album tile
  TIDAL artwork fallback.
- Done: selected Chrome viewport smoke passed for app shell, search, remote
  shell, remote library, remote artist, and remote album routes.
- Done: malformed TIDAL image paths are rejected before browser image requests.
- Done: expanded Chrome artwork smoke passed for local, remote, TIDAL, mood,
  Spotify playlist, and Spotify track surfaces with no broken images and no
  malformed TIDAL artwork requests.
- Done: focused Chrome smoke passed for Spotify artist and Spotify album detail
  routes with loaded real provider detail states, no visible error text, no
  broken image elements, and no malformed TIDAL artwork requests.
- Done: backend-served Chrome smoke passed for dynamic local, remote, TIDAL
  album, and TIDAL artist detail routes with no broken image elements, no app
  request failures, no visible route error text, and no malformed TIDAL artwork
  requests.
- Done: API-only queue smoke passed for the 45-track TIDAL album on a scratch
  DB copy. The queue resolved to 45 artwork-backed TIDAL rows in order while
  playback stayed stopped.
- Done: non-invasive native shell and audio preflight confirmed the installed
  app/server were already running, the then-current server was actively playing,
  and a second launch could shut down the active sidecar.
- Done: non-invasive native process inspection confirmed NOORwave WebView2
  child processes exist, and the app could be surfaced only after pointer-level
  tray discovery.
- Done: installed native WebView smoke passed through the existing tray path
  without launching a second app or restarting the sidecar.
- Done: `cargo check -p noor-app` passed as a no-launch Tauri shell preflight.
- Done: `cargo check -p noor-server` passed with one pre-existing WASAPI
  manual-review warning.
- Done: `cargo test -p noor-app` and `cargo fmt --all -- --check` passed as
  no-launch Rust verification checks.
- Done: `cargo test --workspace --locked` passed as refreshed no-audio
  workspace Rust verification.
- Done: `pnpm check`, `pnpm lint`, `pnpm test`, and `pnpm run build` passed as
  refreshed no-launch frontend verification checks.
- Done: focused no-audio frontend regression coverage passed for the desktop
  and remote loaded TIDAL album paths, including a 45-track store-level playback
  request shape with ordered TIDAL ids, album ids, and artwork URLs.
- Partial: native WebView route navigation reached Genre Galaxy, Moods,
  Playlists, and Settings through existing sidebar controls without visible
  error text in accessibility samples.
- Not verified: audio device, long live TIDAL session behavior, fresh Tauri
  sidecar startup from a stopped state, and manual Tauri WebView artwork route
  screenshot pass.

Done:

- Queue/runtime stall fixes, stale frontend intent guards, artwork fallback
  hardening, API timeout protection, transaction hardening, and runtime handoff
  tracing are implemented and committed. Expanded artwork route smoke, focused
  Spotify detail route smoke, backend-served dynamic detail route smoke, and
  API-only TIDAL album queue smoke evidence are recorded in this report.

Incomplete:

- No known code path from the requested automated audit remains intentionally
  stubbed or partially wired.
- Manual Tauri/audio/provider smoke remains outside this automated run because
  the current installed app and sidecar are active.

Follow-ups added to `FOLLOWUPS.md`: none.

Next checks before release:

- Use `docs/dev/tauri-audio-provider-manual-smoke-2026-06-04.md` as the
  manual smoke checklist.
- Pick a safe manual window where interrupting the current localhost backend is
  acceptable, then launch the Tauri app and confirm the WebView reaches the
  app shell after sidecar startup.
- Run NOORwave through Tauri with a real TIDAL account and audio output.
- Start a queue with pending rows, force an unresolved row, and confirm playback
  advances audibly.
- Force an active stream failure and confirm the queue advances without dead
  air.
- Force a prepared-next failure and confirm current playback stays active.
- Open the Tauri WebView plus search, app shell, remote artist, and remote album
  surfaces and confirm artwork placeholders and fallback-size retries behave
  visually.
