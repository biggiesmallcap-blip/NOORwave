# Playback, Queue, and Runtime Audit Status - 2026-06-04

Status: partial checkpoint, not the final audit closeout.

This document records the current state of the playback queue/runtime audit after
the first implementation slices. It is intentionally conservative: fixed items
are tied to committed changes and local verification, while remaining risks stay
listed as not complete.

## Scope

Requested audit surface:

- Queue architecture and playlist injection paths.
- Runtime finish, error, near-end, and prebuffer flow.
- Resolver behavior for pending, TIDAL, and external queue rows.
- Artwork fallback handling for TIDAL-capable and Spotify search surfaces.
- Frontend API reliability when backend or provider calls stall.
- Regression hardening and observability for queue/runtime invariants.

## Architecture Map

Backend playback ownership:

- `noor-server/src/playback/queue.rs`
  - Persistent queue row loading and mutation.
  - Pending rows with `pending_artist`, `pending_title`, `pending_at`,
    `resolved_at`, and `tidal_match_score`.
  - Local library replace/append, external insert, play-next insert, shuffle,
    position normalization, and pending GC.
- `noor-server/src/playback/player.rs`
  - Playback state snapshots and current-track transitions.
  - Queue replacement through `replace_queue_with_reasons`.
  - Current queue row deletion reconciliation through
    `remove_queue_item_and_reconcile`.
  - Real automix behavior. `playback/automix.rs` is still a stub.
- `noor-server/src/server/routes.rs`
  - HTTP queue/playback routes.
  - TIDAL stream resolution before runtime start/switch.
  - Runtime listener and event handling.
  - Pending queue resolution and skip logic through
    `resolve_or_skip_pending_current`.
  - Runtime finish and failure advancement through `handle_runtime_finished`,
    `handle_runtime_track_error`, and `handle_prepared_runtime_track_error`.
  - Ephemeral TIDAL mix queue orchestration through `pending_tidal_mix_queue`.
- `noor-server/src/playback/runtime/commands.rs`
  - Runtime command and event contract.
  - Active failure uses `TrackError { track_id, generation, message }`.
  - Prepared-next failure uses `PreparedTrackError { track_id, message }`.
  - Generic `Error { message }` remains the fatal runtime command/panic path.
- `noor-server/src/playback/runtime/mod.rs`
  - Decode, output, shared-buffer, near-end, prepared-next, transition, and
    runtime event emission logic.
- `noor-server/src/playback/decode/`,
  `noor-server/src/playback/output/cpal_shared.rs`,
  `noor-server/src/playback/wasapi_exclusive.rs`, and
  `noor-server/src/playback/gapless.rs`
  - Lower-level decode and output surfaces. These were inspected as part of the
    runtime boundary, but the committed slices did not change them.

Frontend playback ownership:

- `frontend/src/lib/api/client.ts`
  - REST client, auth headers, abort/timeout wiring, and queue/playback methods.
- `frontend/src/lib/stores/player.ts`
  - Queue state, current track state, playback actions, remove/apply response
    behavior, local library queue loads, and TIDAL ephemeral start.
- `frontend/src/lib/api/ws.ts`
  - Playback event bridge into frontend state.
- `frontend/src/routes/+layout.svelte`
  - App shell playback surface and now-playing/queue presentation.
- `frontend/src/lib/components/ui/ArtworkImage.svelte` and
  `frontend/src/lib/utils/artwork.ts`
  - Shared artwork fallback and TIDAL-safe artwork URL sizing.
- `frontend/src/routes/search/+page.svelte`
  - Mixed local/TIDAL/Spotify search result rendering and playback entry points.

Playlist injection paths:

- Local library queue replace:
  `frontend/src/lib/stores/player.ts` `loadQueueAndPlay` ->
  `api.replacePlaybackQueue` -> `noor-server/src/server/routes.rs`
  `queue_replace` -> `player::replace_queue_with_reasons`.
- Route-level local playlist replace:
  playlist/library/genre routes call `api.replacePlaybackQueue`, then start
  playback through the normal playback route.
- Persistent external queue insert:
  `/api/queue/append_many` and `/api/queue/play_next_many` route through
  `queue::append_external_tracks` and `queue::insert_external_tracks_after`.
- Ephemeral TIDAL queue:
  `frontend/src/lib/stores/player.ts` `startTidalEphemeralQueue` ->
  `api.playTidalMix` -> `routes.rs` `play_tidal_mix` ->
  in-memory `pending_tidal_mix_queue` plus runtime finish adoption.

## Runtime Flow Diagram

Normal local library playback:

```text
UI action
  -> frontend/src/lib/stores/player.ts
  -> frontend/src/lib/api/client.ts REST call
  -> noor-server/src/server/routes.rs handler
  -> noor-server/src/playback/player.rs or queue.rs DB mutation
  -> stream/job preparation in routes.rs
  -> noor-server/src/playback/runtime/mod.rs command
  -> runtime event broadcast
  -> routes.rs spawn_playback_runtime_listener
  -> playback_state/session sync and AppEvent broadcast
  -> frontend/src/lib/api/ws.ts
  -> player store and visible UI
```

Runtime finish or active decode failure:

```text
Runtime emits Finished or TrackError
  -> routes.rs listener checks track id and generation
  -> handle_runtime_finished_with_retry
  -> handle_runtime_finished
  -> DB snapshot and queue cursor advance
  -> resolve_or_skip_pending_current for pending rows
  -> switch_runtime_to_snapshot_current when there is a playable survivor
  -> playback_state, runtime_info, and websocket state update
```

Prepared-next decode failure:

```text
Runtime emits PreparedTrackError
  -> routes.rs listener calls handle_prepared_runtime_track_error
  -> runtime_info.last_error is updated
  -> current playback remains active
  -> no queue cursor advance happens for the current row
```

## Fixed Bugs

### P0: Finished tracks could stall on unresolved pending queue rows

Root cause: runtime finish could advance onto a pending row that had not
resolved, then stop without skipping to the next playable library row.

Fix: `resolve_or_skip_pending_current` now participates in runtime-finish
advancement and skips unresolved pending rows up to the configured limit.

Evidence:

- Commit `096a04a5 fix(playback): advance past unresolved queue rows`.
- Test: `runtime_finish_skips_unresolved_pending_row_and_starts_next_library_track`.

### P0: Active runtime decode errors could pause or stall a non-empty queue

Root cause: active runtime errors were handled through the generic fatal error
path, which recorded the error but did not treat the failed track as terminal
queue progress.

Fix: active track failures now emit `TrackError` and route through
`handle_runtime_track_error`, which reports the failure and advances through the
same finish flow as an ended track.

Evidence:

- Commit `78454803 fix(playback): advance after active runtime errors`.
- Test: `runtime_track_error_advances_to_next_library_track`.

### P1: Prepared-next decode errors could disturb current playback

Root cause: prepared-next failure was not separated from active track failure,
so a failed prebuffer could be surfaced like an active runtime failure.

Fix: runtime now emits `PreparedTrackError` for prepared-next decode failures.
The route listener records the error while keeping the active track and queue
cursor unchanged.

Evidence:

- Commit `78454803 fix(playback): advance after active runtime errors`.
- Test: `prepared_runtime_track_error_keeps_current_playback_running`.

### P1: Removing the current queue row left stale current queue state

Root cause: `/api/playback/queue/remove` deleted the queue row and returned a
queue, but the playback state could still point at the deleted
`current_queue_item_id`.

Fix: `remove_queue_item_and_reconcile` now deletes the row and reconciles
playback state. If the removed row was current, it advances to the next
survivor, preserves paused state when paused, stops when no survivor exists,
and switches runtime when playback was active.

Evidence:

- Commit `7f63913b fix(playback): reconcile current queue removal`.
- Backend tests around `remove_queue_item_and_reconcile`.
- Frontend test: `removeTrackFromQueue applies playback state returned by the
  remove endpoint`.

### P1: Stalled API calls could freeze frontend playback actions

Root cause: the frontend API client accepted external abort signals but did not
apply a default timeout. A backend/provider call that never resolved could keep
UI actions pending.

Fix: `requestTimeout` wraps fetch requests with default and bulk queue
timeouts. Timeout failures are reported as `ApiTimeoutError`.

Evidence:

- Commit `099e5d9b fix(frontend): time out stalled api calls`.
- Test file: `frontend/src/lib/api/client.timeout.test.ts`.

### P2: Spotify search artwork had no shared fallback path

Root cause: Spotify search thumbnails were rendered through raw image/background
paths in some search cards, so broken image URLs could leave empty artwork
blocks.

Fix: Spotify album, playlist, and track search results route through
`ArtworkImage` with fallback text and explicit thumbnail sizing.

Evidence:

- Commit `4fabe248 fix(search): use artwork fallbacks for spotify results`.
- Test: `frontend/src/routes/search/search_layout_contract.test.ts`.

### P2: Large local playlist queue replacement used per-row implicit writes

Root cause: local library append, pending append, and queue replace wrote rows
without wrapping the batch in an explicit transaction.

Fix: `append_tracks_with_reasons`, `append_pending_tracks`, and
`replace_queue_with_reasons` now use explicit transactions. `replace_queue`
routes through `replace_queue_with_reasons`, so delete plus inserts are atomic.

Evidence:

- Commit `6a4e17d1 fix(queue): batch library playlist inserts`.
- Test: `replace_queue_handles_large_playlist_in_order`.

## Regression Coverage Added Or Exercised

Backend:

- `runtime_finish_skips_unresolved_pending_row_and_starts_next_library_track`
- `runtime_track_error_advances_to_next_library_track`
- `prepared_runtime_track_error_keeps_current_playback_running`
- `queue_play_next_many_preserves_requested_order`
- `replace_queue_handles_large_playlist_in_order`
- Existing queue reconciliation tests around `remove_queue_item_and_reconcile`

Frontend:

- `frontend/src/lib/api/client.timeout.test.ts`
- `frontend/src/lib/stores/player.restore_queue.test.ts`
- `frontend/src/routes/search/search_layout_contract.test.ts`

## Observability Status

Done so far:

- Queue append/play-next routes include structured tracing with queue counts and
  inserted counts.
- Pending resolution and skip paths log resolver failures and skip events.
- Runtime listener separates active track failures, prepared-next failures, and
  fatal runtime errors in logs.
- Runtime finish retry logs SQLite lock retries.

Still not complete:

- No single structured correlation id connects a UI queue action, queue mutation,
  stream resolution, runtime command, runtime event, and websocket update.
- Runtime finish and pending-skip logs are present, but not yet normalized into a
  compact event taxonomy for production triage.
- No manual audio session log capture has been attached to this checkpoint.

## Verification Already Run

Commands run during the committed slices:

- `cargo test -p noor-server replace_queue_handles_large_playlist_in_order`
- `cargo test -p noor-server replace_and_load_queue_round_trip`
- `cargo test -p noor-server queue_play_next_many_preserves_requested_order`
- `cargo test -p noor-server replace_queue`
- `cargo test -p noor-server runtime_finish_skips_unresolved_pending_row_and_starts_next_library_track`
- `cargo test -p noor-server runtime_track_error_advances_to_next_library_track`
- `cargo test -p noor-server prepared_runtime_track_error_keeps_current_playback_running`
- `cargo fmt --all -- --check`
- `cargo check -p noor-server`
- Targeted frontend Vitest files for API timeout, player store queue removal,
  and search artwork contract coverage.
- `pnpm lint`
- `pnpm check`
- `pnpm run build` for the search artwork slice.

This checkpoint also re-verified local symbol presence with `rg` and
rust-analyzer before writing the report.

## Not Verified Yet

- Full `cargo test --workspace --locked`.
- Full frontend `pnpm test` after all slices together.
- End-to-end UI smoke with the real app surface and audio runtime.
- Manual audio verification that a real failed stream advances without audible
  dead air.
- Manual verification of very large local and TIDAL playlist injection through
  visible UI, not only route/unit coverage.

## Remaining Risks

- Pending resolver branches beyond the tested unresolved-row skip path can still
  hide stale async state bugs, especially when provider token refresh or import
  fails during a runtime finish.
- TIDAL ephemeral queue adoption has tests around finish/switch behavior, but a
  full real-provider smoke is still needed for long mixes and remote surfaces.
- Search and many track-list surfaces use `ArtworkImage`, but a complete audit
  of every route that can receive a TIDAL URL is not yet closed.
- API timeouts stop indefinite frontend hangs, but route-specific retry or
  partial-result UX is still uneven.
- Observability is useful but not yet end-to-end. Debugging a field report still
  requires stitching route logs, runtime logs, and frontend state manually.

## Completion Gate For This Checkpoint

Acceptance checks:

- Done: locally verified the files and symbols cited in this document.
- Done: separated fixed bugs from not-yet-verified risks.
- Done: included architecture map, text runtime flow, fixed bug list,
  regression coverage, observability status, and verification status.
- Not verified: full app/audio route smoke. This document is a partial audit
  checkpoint, not a final completion report.

Incomplete for the original audit request:

- Full end-to-end manual QA through the visible app.
- Full workspace and frontend test runs.
- Complete artwork surface audit.
- End-to-end structured logging correlation.
- Final audit report after all remaining implementation slices.

Follow-ups added to `FOLLOWUPS.md`: none. The remaining items above are current
audit scope, so they should not be hidden as follow-ups.
