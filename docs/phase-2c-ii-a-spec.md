# Phase 2c-ii-a: Non-Library Last.fm Radio Results

**Status:** Design approved, awaiting implementation  
**Research:** [phase-2c-ii-a-inventory.md](phase-2c-ii-a-inventory.md)  
**Date:** 2026-05-01

---

## Problem

Last.fm radio returns track recommendations that are not in the local library. The current queue
schema (`track_id INTEGER NOT NULL REFERENCES tracks(id)`) rejects them at the DB layer. The
frontend filter at `player.ts:761` (`t.is_in_library && t.track_id > 0`) drops them before they
reach the queue at all. The result: radio sessions silently truncate to however many results happen
to be in-library, often producing short or empty queues.

---

## Hard Prerequisites

**These must be done before any other 2c-ii-a code lands.**

### 1. UNIQUE constraint on `tracks.tidal_id`

The race safety argument for concurrent Tidal resolvers (background + lazy firing on the same
pending row) depends entirely on `tracks.tidal_id` carrying a UNIQUE constraint. Without it, two
concurrent `import_track_from_metadata` calls with the same `tidal_id` can produce two separate
rows with different `local_id` values, breaking the "both resolvers produce the same `local_id`"
guarantee.

**Action:** Before writing migrations 020–023 or any resolver code, verify that `tracks.tidal_id`
has a UNIQUE constraint in `db/schema.rs`. If it is absent, add it as migration 020 (shifting other
migrations up). This is a blocker, not optional cleanup.

### 2. Idempotency unit test for `import_track_from_metadata`

Add a `#[cfg(test)]` block in `services/tidal/import.rs` asserting: two sequential calls with
identical `tidal_id` return identical `local_id` with no error. This pins the idempotency guarantee
that the race analysis depends on.

---

## Schema Changes

### Migration: rebuild `queue` table

The `queue` table must be rebuilt (SQLite cannot drop `NOT NULL` via `ALTER COLUMN`). Pattern:
`CREATE TABLE queue_new` → `INSERT INTO queue_new SELECT` → `DROP TABLE queue` → `RENAME`.

**New `queue` schema:**

```sql
CREATE TABLE queue (
    id                  INTEGER PRIMARY KEY,
    track_id            INTEGER REFERENCES tracks(id),   -- NULL until resolved
    position            INTEGER NOT NULL,
    source              TEXT    DEFAULT 'user',
    reason              TEXT,
    pending_artist      TEXT,   -- populated on insert; kept after resolution for audit
    pending_title       TEXT,   -- same
    pending_at          TIMESTAMP,           -- set on insert for pending rows; NULL for direct inserts
    resolving_at        TIMESTAMP,           -- set when a resolver claims the row; cleared on completion
    resolved_at         TIMESTAMP,           -- set atomically with track_id
    tidal_match_score   REAL                 -- set atomically with track_id
);

CREATE INDEX idx_queue_position ON queue(position);
CREATE INDEX idx_queue_pending  ON queue(track_id, pending_at);
```

Re-create any other existing indexes after rebuild.

`pending_artist` / `pending_title` remain populated after resolution. They serve as the human-readable label while pending and as an audit trail for resolution quality (score vs. what we asked for) after resolution.

### Migration 021: `current_queue_item_id` on `playback_state`

```sql
ALTER TABLE playback_state ADD COLUMN current_queue_item_id INTEGER REFERENCES queue(id);
```

Required because `current_track_id` is NULL while a pending row is playing. Position scans in
`next_track()` and `previous_track()` use `item.id == current_queue_item_id` instead of
`item.track.id == current_track_id`.

---

## New Endpoint: `POST /api/radio/start`

The frontend currently assembles the radio queue from a candidate list. Replace this with a single
endpoint that builds the queue atomically.

**Request body:**
```json
{ "seed_track_id": 12345 }
```

**Behavior:**
1. Fetch Last.fm recommendations for the seed track.
2. In a single DB transaction: insert library tracks as direct queue rows (with `track_id`), insert
   non-library tracks as pending rows (`track_id NULL`, `pending_at` set).
3. After the transaction commits, spawn background resolvers for all pending rows (see §Resolution).
4. Return a `first_playable` descriptor for the frontend to start playback.

**Response shape:**
```json
{
  "first_playable": {
    "type": "library" | "pending",
    "queue_item_id": 42,
    "track_id": 123 | null
  }
}
```

`type: "library"` — frontend proceeds as normal, `track_id` is set.  
`type: "pending"` — all radio results were non-library; frontend triggers lazy resolution on the
first queue item before starting playback. `track_id` is null.

The frontend stops assembling the queue from the candidate list. `startSongRadio` in `player.ts`
calls this endpoint instead.

---

## Resolution Pipeline

### Background-eager path

After `POST /api/radio/start` inserts pending rows, the handler spawns one `tokio` task per pending
row, bounded by a `Arc<Semaphore>` with **permits = 4**. This gives ~2s wallclock for 12 rows while
staying well under Tidal's rate limits (12 concurrent would be ~500ms but risks 429s; sequential
would be ~6s).

Each task: Tidal search → score → if score ≥ `MATCH_QUALITY_THRESHOLD` → `import_track_from_metadata`
→ atomic UPDATE (`track_id`, `resolved_at`, `tidal_match_score` in one transaction). If score < threshold,
the row stays pending for GC.

### Lazy fallback path

`next_track()` in `player.rs` is sync. When it returns a `QueueItem` with `is_pending = true`, the
async caller in `routes.rs`:

1. `await`s a Tidal search + `import_track_from_metadata` on the executor (non-blocking — the
   executor thread is not blocked, only the logical request waits).
2. If resolution succeeds, calls `next_track()` again to re-fetch the now-resolved row.
3. If resolution fails (score < threshold or Tidal error), skips to the next queue item.

This is synchronous from the user's perspective (the `POST /api/queue/advance` response waits) but
non-blocking on the runtime.

### Resolver ownership guard

Both paths (background and lazy) claim a row before searching Tidal:

```sql
UPDATE queue
SET resolving_at = datetime('now')
WHERE id = ?1
  AND resolving_at IS NULL
  AND track_id IS NULL
RETURNING id
```

If `RETURNING` returns a row, this resolver owns it. If the `RETURNING` result is empty, another
resolver is already in flight or the row is resolved — both cases mean skip. On completion (success
or failure), `resolving_at` is cleared back to NULL.

Stale lock detection: `resolving_at` older than 30 seconds (Tidal request timeout + buffer)
indicates a crashed or abandoned resolver. The GC sweep clears these, returning the row to
claimable state.

### Resolution write: first resolver wins

The resolution `UPDATE` includes `AND track_id IS NULL` so the first resolver to complete writes
its result and subsequent attempts are silent no-ops:

```sql
UPDATE queue
SET track_id = ?1, resolved_at = datetime('now'), tidal_match_score = ?2, resolving_at = NULL
WHERE id = ?3
  AND track_id IS NULL
```

The "best score wins" variant is not used — simpler is better here.

### Race safety

Background and lazy resolvers may both claim the same row if the ownership guard races (e.g., both
read `resolving_at IS NULL` before either writes). This is safe:

- `import_track_from_metadata` contains a SELECT-before-INSERT inside a transaction, keyed on
  `tidal_id`.
- The UNIQUE constraint on `tracks.tidal_id` (hard prerequisite above) ensures the second
  concurrent INSERT fails cleanly rather than producing a duplicate row.
- Both resolvers produce the same `local_id`; the conditional UPDATE (`AND track_id IS NULL`)
  means the second write is a no-op.
- No additional lock is needed beyond what SQLite's single-writer model provides.

### Resolution robustness under user actions

The lazy fallback's "re-fetch after resolution" follows current queue state, not stale intent. If
the user clicks Next twice rapidly, removes the pending row, or starts a new radio session while
resolution is in flight, the second `next_track()` call returns whatever the queue state is at that
moment. The resolved track row remains in the DB (resolution already wrote to it) but may no longer
be the next-up row. This is intentional.

---

## Tidal Match Quality

### Scoring

When Tidal search results carry album metadata:

```
score = 0.55 × jaro_winkler(normalize(result.artist), normalize(pending_artist))
      + 0.35 × jaro_winkler(normalize(result.title),  normalize(pending_title))
      + 0.10 × jaro_winkler(normalize(result.album),  normalize(pending_album))
```

When album metadata is absent (falls back to two-field scoring):

```
score = 0.60 × jaro_winkler(normalize(result.artist), normalize(pending_artist))
      + 0.40 × jaro_winkler(normalize(result.title),  normalize(pending_title))
```

`normalize`: lowercase, strip punctuation, collapse whitespace.

The album boost is conditional — the two-field weights rebalance to keep totals at ~1.0 when album
is unavailable. Artist remains the dominant signal in both variants: title collisions (covers,
remixes sharing titles across artists) are more common than artist collisions.

Duration is excluded from scoring — Tidal durations for live/radio edits diverge from expected
values often enough to be a noisy signal at this threshold.

### Tunable constants

```rust
const MATCH_QUALITY_THRESHOLD: f64 = 0.85;
// two-field weights:  artist = 0.60, title = 0.40
// three-field weights: artist = 0.55, title = 0.35, album = 0.10
```

All values are **starting heuristics**, not empirically validated. Log the actual score on every
resolution attempt (not just on accept/reject) so the score distribution is visible after a few
real sessions. Revisit the threshold and all three weights after smoke-test data is available — the
threshold may need tightening (too many bad covers accepted) or loosening (too many valid matches
rejected). Do not bake these as magic numbers; all must remain named constants.

### On threshold miss

Score < `MATCH_QUALITY_THRESHOLD`: no `track_id` written. Background resolver drops the result.
Lazy resolver returns an error causing the caller to skip to the next queue item. The pending row
is visible in the queue with its unresolved treatment until GC removes it (see §Garbage Collection).

---

## `current_track_id` NULL Window — Required Call Site Fixes

`current_track_id` in `playback_state` is NULL while a pending row is playing (set only after the
row has a real `track_id`). **All eight call sites below must be handled.** Treat this list as a
checklist: implementation is not complete until every line is verified.

| # | Call site | File | Current assumption | Required fix |
|---|---|---|---|---|
| 1 | `next_track` position scan | player.rs:382 | `item.track.id == current_track_id` | Switch to `item.id == current_queue_item_id` |
| 2 | `previous_track` position scan | player.rs:447 | Same | Same fix |
| 3 | `set_shuffle_mode` → `apply_shuffle` | player.rs:316 | Passes `current_track_id` to `apply_shuffle` | `apply_shuffle` must tolerate `None` (shuffle relative to queue item position) |
| 4 | `automix discover_new` | routes.rs:5058 | `find(|q| q.track.id == current_track_id)` | Switch to `find(|q| q.id == current_queue_item_id)` |
| 5 | `keep-now-playing clear` | routes.rs:5482 | `DELETE FROM queue WHERE track_id != current_id` | Safe: pending `else` branch already short-circuits; no fix needed but verify |
| 6 | `finished_track` guard | routes.rs:7367 | `current_track_id != Some(finished_track_id)` | Safe: `finished_track_id` comes from a resolved track; NULL window closed before this fires; verify |
| 7 | `record_transition_if_changed` | routes.rs:7760 | Writes `to_track_id` from `current_track.id` | Add guard: `if to_track_id <= 0 { return; }` |
| 8 | `flush_active_listen_session_locked` | routes.rs:7797 | Writes `track_id` to `listen_history` | Add guard: `if track_id <= 0 { return; }` |

`play_track_now` (player.rs:270) always receives a real `track_id`; unaffected.

`current_playback_track_id` endpoint (routes.rs:7734) already returns `Option<i64>`; returns `None`
during pending playback. No fix needed.

---

## Frontend Changes

### `QueueItem` interface (`client.ts`)

Add `is_pending?: boolean`.

### `startSongRadio` (`player.ts:756`)

Remove the `is_in_library && track_id > 0` filter. Switch to calling `POST /api/radio/start` and
receiving the first library track ID for playback. The frontend no longer assembles the queue from
the candidate list.

### Queue row rendering (`+layout.svelte`)

- **Artwork (L1215):** add `is_pending` branch showing a spinner instead of artwork placeholder.
- **Active check (L1196):** `$currentTrack?.id === item.track.id` → use `current_queue_item_id`
  based approach (item.id match).
- **Favorite button (L1254–1258):** `disabled={item.is_pending}`.
- **Context menu (`pickMenuBuilder`):** pass `item.is_pending`; strip to: Song Radio (with
  2–3s loading indicator), Remove from queue, Play next, Add to queue. Disable: Add to Playlist,
  Favourite, Go to Artist, Go to Album.

**Loading indicator note:** Song Radio on a pending row triggers the resolution chain (2–3s).
Show a loading state on the Song Radio menu item from click until the endpoint responds.

### `RadioCandidate` interface (`client.ts:641`)

No change needed — `is_in_library: boolean` already present.

---

## System Interactions

| System | Pending (unresolved) | Pending (resolved, pre-play) | Promoted (`tidal_stream`) |
|---|---|---|---|
| Queue display | Spinner, pending_artist/title | Artwork visible, resolved title | Normal |
| Playback | Lazy-resolves at play time; skip on failure | Tidal stream | Tidal stream |
| Context menu | Song Radio, Remove, Play next/Add (via queue.id); others disabled | Same | Full menu |
| `listen_history` | Skipped (track_id guard) | Skipped | Normal on future plays |
| Transition records | Skipped (current_track_id guard) | Skipped | Normal |
| Embedding model / automix | No signal | No signal | Normal after first post-promotion play |
| Scrobbling / Last.fm | Skipped | Skipped | Normal |
| Library grid | Not visible (`tidal_stream` filtered) | Not visible | Not visible until favorited |
| automix seed | **Skipped — see note below** | Skipped | Works as seed |

**Automix and pending rows:** automix will not extend a queue that consists entirely of unresolved
pending rows — it requires a `tracks.id` to look up recommendation neighbours and pending rows have
none. A typical last.fm radio call returns 12 pending entries, which is deep enough that this
rarely matters in practice. However: if all 12 fail resolution (all below threshold), automix will
also fail to fire, and the queue will exhaust silently. Document this interaction explicitly so it
does not surprise anyone debugging "why did automix not fire."

---

## Promotion

Promotion (pending/`tidal_stream` → library) is **explicit only**: user clicks the heart/favorite
button on a resolved queue row. This calls the existing `POST /api/track/:id/favorite` endpoint.
No auto-promotion logic is introduced in 2c-ii-a.

Queue resolution state (`is_pending`) and library membership (`is_favorite`) are orthogonal. A
row can be resolved (has `track_id`) while the track remains `tidal_stream` indefinitely.

---

## Data Loss Bounds

Pending tracks do not generate `listen_history` rows or transition records (`current_track_id`
guards block both writes). Once a track promotes to library (`is_favorite = 1`), future plays
generate normal history.

The data loss is bounded to the pre-promotion window. This is a known gap, not an oversight.
Relevant for:
- Analytics consumers (play counts and streams will undercount `tidal_stream` tracks)
- Embedding model training (transition data is sparse for tracks that are rarely or never promoted)

Phase 3 path: migrate recommendation tables to use `tidal_id` as the stable identifier, making
library membership incidental. `tidal_stream` tracks would then participate in recommendation
graphs from first play. Not in scope for 2c-ii-a, but the schema decisions here (storing `tidal_id`
on every resolved pending row, logging `tidal_match_score`) are forward-compatible with that
migration.

---

## Garbage Collection

**Unresolved rows past TTL:** any `queue` row where `track_id IS NULL AND pending_at < datetime('now', '-6 hours')` is stale. A sweep runs at server startup and hourly via `tokio::time::interval`. 6 hours covers normal pause behaviour (user pauses a session for a few hours) without needing active-session-only logic, which adds complexity for marginal benefit. Rows surviving 6 hours are genuinely unresolvable.

**Stale `resolving_at` locks:** any row where `resolving_at < datetime('now', '-30 seconds') AND track_id IS NULL` had its resolver crash or time out. The GC sweep clears `resolving_at` to NULL, returning the row to claimable state for the lazy resolver.

**Resolved rows:** standard queue GC handles these. "Clear queue" and session-change deletes apply to all rows including resolved pending rows.

**Orphaned rows (track deleted):** `ON DELETE SET NULL` on the `track_id` FK reverts the row to a NULL-track_id state. The GC TTL sweep catches it. Note: `tidal_stream` tracks are never deleted by normal user actions; this case is a safeguard only.

`tidal_stream` track rows themselves are not GC'd in 2c-ii-a. A long-term sweep (prune `tidal_stream` tracks with no `listen_history` and `is_favorite = 0`) is Phase 3.

---

## Migration Sequence

Assuming `tracks.tidal_id` already has a UNIQUE constraint (verify first):

| # | Description |
|---|---|
| 020 | Rebuild `queue` table (nullable `track_id`, add `pending_at`, `resolving_at`, `resolved_at`, `tidal_match_score`, `pending_artist`, `pending_title`; add `idx_queue_position` and `idx_queue_pending`) |
| 021 | `ALTER TABLE playback_state ADD COLUMN current_queue_item_id INTEGER REFERENCES queue(id)` |

If `tracks.tidal_id` lacks a UNIQUE constraint, that becomes migration 020 and the above shift to 021–022.

---

## Files Touched

| File | Change |
|---|---|
| `db/schema.rs` | Migrations 020–021 |
| `db/models.rs` | `QueueItem` add `is_pending: bool` |
| `playback/queue.rs` | `load_queue` LEFT JOIN + COALESCE; `append_pending_tracks`; `queue_track_ids` NULL guard |
| `playback/player.rs` | `next_track`, `previous_track` (queue item id scan); `apply_shuffle` None tolerance |
| `server/routes.rs` | `POST /api/radio/start`; 7 call site fixes from NULL audit (items 1–4, 7–8; items 5–6 verify-only) |
| `services/tidal/import.rs` | Idempotency unit test |
| `frontend/src/lib/api/client.ts` | `QueueItem.is_pending`; `POST /api/radio/start` call |
| `frontend/src/lib/stores/player.ts` | `startSongRadio` switch to new endpoint |
| `frontend/src/routes/+layout.svelte` | Pending row rendering, context menu, active check |
