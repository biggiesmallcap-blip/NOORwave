# Playback, Queue, and Runtime Audit - 2026-06-04

Status: final automated audit report. Manual live audio and visible app smoke
remain not verified in this agent run.

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

Non-blocking warnings observed:

- `cargo check -p noor-server` still emits pre-existing dead-code warnings in
  backend query/playback modules. This audit did not address dead-code cleanup.

## Not Verified

- Real Tauri app smoke with a visible WebView.
- Real audio-device playback for track finish, active decode failure, and
  prepared-next failure.
- Real TIDAL provider session with long ephemeral mixes, token refresh, and
  provider-side 403/429 behavior.
- Screenshot or viewport pass across every route that can display artwork.

These are manual or environment-dependent checks. They are not hidden in
`FOLLOWUPS.md` because they are current release-readiness checks, not future
cleanup ideas.

## Completion Gate

Acceptance checks:

- Done: architecture map and runtime flow are documented.
- Done: prioritized bug list is tied to committed fixes.
- Done: regression coverage is listed with the public paths it exercises.
- Done: structured logging coverage is documented.
- Done: broad automated frontend and Rust verification passed.
- Not verified: manual Tauri, audio device, live TIDAL, and visual route smoke.

Done:

- Queue/runtime stall fixes, stale frontend intent guards, artwork fallback
  hardening, API timeout protection, transaction hardening, and runtime handoff
  tracing are implemented and committed.

Incomplete:

- No known code path from the requested automated audit remains intentionally
  stubbed or partially wired.
- Manual app/audio/provider smoke remains outside this automated run.

Follow-ups added to `FOLLOWUPS.md`: none.

Next checks before release:

- Run NOORwave through Tauri with a real TIDAL account and audio output.
- Start a queue with pending rows, force an unresolved row, and confirm playback
  advances audibly.
- Force an active stream failure and confirm the queue advances without dead
  air.
- Force a prepared-next failure and confirm current playback stays active.
- Open search, app shell, remote artist, and remote album surfaces and confirm
  artwork placeholders and fallback-size retries behave visually.
