# Prompt: unify the playback queue into one standardized model

Paste this as the opening message of a fresh session. It is a design-first brief,
not a license to big-bang. Produce a plan and get sign-off before editing the
runtime.

ASCII only in everything you write (repo rule): no em dashes, no smart quotes.

---

## Role

You are refactoring NOORwave's playback queue. Today it is two incompatible
models stitched together, and the seam between them has produced a steady stream
of bugs (skip plays the wrong track, "previous" reloads played tracks, play-next
rows linger and duplicate, DJ crossfade arms into a skipped track). I want the
queue rebuilt around a single, standardized model like a real streaming client
(Spotify / TIDAL / Apple Music) uses, so that source of a track is *data*, not a
separate code path.

Read `CLAUDE.md` first, especially "Landmines" and "Don't touch without asking".
Playback is load-bearing and timing-sensitive. Treat this as a staged
(strangler-fig) refactor, not a rewrite.

## The current problem (verified)

There are two queue models with opposite invariants, and `playback_state`'s NULL
anchor means different things in each:

1. Persistent library queue (original): `queue` rows have a real `track_id`; the
   current item is anchored by `playback_state.current_queue_item_id` /
   `current_track_id`; played rows persist; "next" is anchor + 1;
   `previous` walks back through them. Used by library playback, automix, and
   Last.fm radio "pending" rows (`track_id IS NULL`, resolved later via TIDAL
   search).

2. Ephemeral TIDAL mix (bolted on): `queue` rows have `track_id IS NULL` with
   `source IN` `EPHEMERAL_TIDAL_SOURCES` (`tidal_mix`/`tidal_album`/
   `tidal_playlist`) and a `tidal_id_hint` + `ephemeral_*` columns; rows are
   consumed destructively (`pop_next_ephemeral_tidal_track` deletes on play); and
   the currently-playing mix track has NO queue row at all - it lives in the
   in-memory `AppState.ephemeral_tidal_track`, so the DB anchor is NULL while a
   mix plays.

Because the current mix track has no row and the anchor is NULL, every playback
operation has to detect "am I in a mix or not" and branch. Each was patched
independently, so they drifted. Related in-memory shadow state:
`AppState.ephemeral_tidal_track`, `external_playback_track`,
`prepared_ephemeral_tidal_next`, and the legacy `pending_tidal_mix_queue`
`VecDeque` (already deprecated per CLAUDE.md).

Recent seam patches you should read to understand the failure modes (then
subsume, not extend): in `noor-server/src/server/routes.rs` -
`advance_ephemeral_next_if_needed`, `restart_ephemeral_current_if_needed`,
`next_advance_ephemeral_tidal_id`, `pop_next_ephemeral_if_due`,
`next_advance_ephemeral_track`, `ephemeral_owned_for_request(s)`,
`handle_runtime_finished`, `handle_near_end` / `handle_ephemeral_tidal_near_end`,
`try_adopt_prepared_ephemeral_tidal_next`, `play_tidal_mix`,
`play_tidal_ephemeral`, `play_track`, `clear_ephemeral_playback_markers`; in
`noor-server/src/playback/player.rs` - `next_track`, `previous_track`,
`peek_next_track`, `playback_anchor_index`; in
`noor-server/src/playback/queue.rs` - `load_queue`, the `EPHEMERAL_*` consts,
`pop_next_ephemeral_tidal_track`, `insert_ephemeral_tidal_tracks_after`,
`delete_all_ephemeral_tidal_rows`, `trim_ephemeral_tidal_rows_through_tidal_id`;
`noor-server/src/playback/automix.rs` - `ensure_automix_queue_depth`.

## Target model (what a streaming client does)

Design one queue where every item is uniform:

1. One row type. Every queue row is playable and carries: stable row id, ordered
   position, a `source`/provenance tag (library, tidal_mix, radio, automix, ...),
   and a stream descriptor that says HOW to get audio - one of: local
   `track_id`, a `tidal_id` to stream directly, or an unresolved
   (artist,title) pending pair to resolve later. "How to fetch audio" is a
   property of the row, not a separate table shape or code path.

2. The current item is ALWAYS a real row. `playback_state` points at a real
   `queue` row (cursor). When a TIDAL mix starts, its playing track is a real
   row too - not an in-memory field. NULL cursor means exactly one thing:
   nothing is loaded / stopped. Kill the "NULL anchor == a mix is playing"
   overload entirely. Delete `ephemeral_tidal_track` /
   `external_playback_track` as sources of truth (fold into the queue + a single
   "current stream info" concept).

3. Non-destructive by default; consumption is an explicit per-row flag. Playing a
   row moves the cursor; it does not delete the row. Rows may carry a
   `consume_on_advance` (or `keep_in_history: false`) flag if a source really
   wants radio-style "don't let me go back" behavior, with a bounded history cap
   (e.g. keep N played rows, trim older). "previous" works within the session for
   every source, mix included, up to that cap. Decide and document the history
   policy.

4. One producer interface. Library "play", mix start, radio, automix, play-next,
   add-to-queue, discover-new all APPEND or INSERT uniform rows with a source
   tag. There is ONE "keep the queue N ahead of the cursor" extension policy, not
   separate ephemeral vs automix refill logic.

5. One consumer of "what's next". A single `next_index`/`peek_next`/`advance`
   used by manual skip, runtime-finished auto-advance, gapless NearEnd
   pre-buffer, and the DJ crossfade arming. They must never disagree about the
   next row again. Same for `previous`.

6. Source is data, not control flow. `next`, `previous`, `play-next`, `add`,
   `remove`, `move`, `shuffle`, `jump-to`, `clear`, `repeat one|all` each have ONE
   implementation that reads the row's stream descriptor to decide how to play,
   never `if in_mix { ... } else { ... }`.

## Constraints and landmines (do not break)

- Migrations are append-only in `noor-server/src/db/schema.rs` (`MIGRATION_0xx`
  consts + the `MIGRATIONS` slice; `_migrations` tracks applied ids). Add new
  ones; never edit a past migration. Column adds are fine; a data backfill
  migration is fine.
- Boot-time wipe (`noor-server/src/main.rs`): `queue` is DELETEd and
  `playback_state` reset on every start (ephemeral runtime). Keep this coherent
  with the new model (the queue is not meant to survive restart; user prefs are).
- Do not disturb tuned timing: WASAPI exclusive
  (`playback/wasapi_exclusive.rs`), gapless / NearEnd / StreamSwap
  (`playback/runtime.rs`, `playback/gapless.rs`), sidecar shutdown
  (`noor-app/src/sidecar.rs`). If the queue change forces a runtime change, call
  it out and get sign-off.
- Keep `tracks.tidal_id` UNIQUE (pending-row resolver race safety) and the
  pending-row resolution flow (`build_radio_queue_and_spawn_resolvers`,
  `spawn_pending_queue_resolver`) working under the unified row.
- Auth middleware ordering (`route_layer` not `layer`) is load-bearing; leave it.
- `routes.rs` is ~17k lines - grep, do not read by offset. Splitting it is out of
  scope unless it falls out naturally.
- Frontend must keep working: the queue shape is consumed by
  `frontend/src/lib/stores/player.ts` (`QueueItem`, `PlaybackState`,
  `selectOptimisticNextItem`, `currentQueueAnchorItem`, `computePlayNextPos`),
  `frontend/src/lib/player/playable.ts`, and the row/menu components. Keep the
  JSON contract additive where possible; update the TS types + contract tests if
  you change it.

## Deliverables

1. A short design doc (in `docs/`) BEFORE touching the runtime: the unified row
   schema (columns + migration plan), how "current" is represented, the history
   policy, how each source maps onto the model, and the staged rollout order.
   Stop and get sign-off on this.
2. Staged implementation behind the design, each stage compiling + green:
   - Stage 1: make the playing mix track a real queue row + cursor (kill the
     in-memory `ephemeral_tidal_track` as source of truth). Prove `previous`
     works across a mix.
   - Stage 2: collapse the advance/previous/peek/prebuffer consumers onto one
     ordered helper reading the row stream descriptor.
   - Stage 3: unify the producers (mix/radio/automix/play-next/add) onto one
     insert + one extension policy; remove the ephemeral-only helpers and the
     `EPHEMERAL_TIDAL_SOURCES` special-casing where it is now redundant.
   - Stage 4: delete dead shadow state (`external_playback_track`,
     `prepared_ephemeral_tidal_next` if subsumed, `pending_tidal_mix_queue`).
3. Tests: in-tree `#[cfg(test)]` with `Database::open_in_memory()`. Add a
   cross-source matrix: for each operation (next, previous, play-next, add,
   remove, move, shuffle, jump-to, clear, repeat one, repeat all) assert
   identical behavior whether the current/target row is library, tidal_mix, or
   radio-pending. Keep the existing playback tests green.

## Acceptance criteria

- No code path branches on "is a mix playing" via a NULL anchor. The cursor
  always points at a real row while something plays; NULL means stopped only.
- `previous` returns to the actually-previous track for every source, including
  inside a TIDAL mix (up to the documented history cap).
- Play-next / add-to-queue put a row in the right place and it is played exactly
  once, with no lingering or duplicate rows, for library and TIDAL alike.
- Skip, runtime-finished, gapless NearEnd pre-buffer, and DJ crossfade arming all
  agree on the next row (one helper).
- `cargo test` green; new cross-source matrix tests present; frontend `pnpm check`
  + contract tests green.
- No em dashes / non-ASCII introduced. `cargo fmt --all` clean.

## First move

Do not write runtime code yet. Read the files listed above, then produce the
design doc and the staged plan, and ask me to confirm the history policy and the
"current = real row" representation before implementing.
