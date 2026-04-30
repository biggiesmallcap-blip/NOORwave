# Phase 2c-ii-a Inventory: Queue Surface for Non-Library Radio Results

## Summary

The `queue` table enforces `track_id INTEGER NOT NULL REFERENCES tracks(id)`, making it structurally impossible to hold a last.fm-only entry without a schema change. Of the three options, **Option C** (sentinel rows in `tracks` with `source='pending_lastfm'`) keeps the FK intact and leaves the `load_queue` JOIN and downstream consumers untouched longest — but it collides with the `artist_id INTEGER NOT NULL` constraint on `tracks`, requiring either a phantom "pending" artist row or a table rebuild. **Option A** (nullable `track_id` + metadata columns on `queue`) touches the fewest tables but requires a SQLite table-rebuild to drop the NOT NULL and will make every call site that assumes a full `Track` object need defensive guards. **Option B** (parallel `pending_queue` table) is the most isolated but requires coordinated position management across two tables. The biggest systemic risk across all three options is that `listen_history` and `shuffle_state` both have hard FKs to `tracks(id)` — any playback of a pending row that triggers a history write will fail at the database level unless explicitly guarded.

---

## 1. Schema Options A / B / C

### Current schema (key tables)

**`queue`** — `noor-server/src/db/schema.rs:158–163`
```sql
CREATE TABLE queue (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    position INTEGER NOT NULL,
    source TEXT DEFAULT 'user'
);
```
Phase 2b added `reason TEXT` via `MIGRATION_018` (`schema.rs:575–577`).

**`tracks`** — `schema.rs:68–99`
```sql
CREATE TABLE tracks (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    artist_id INTEGER NOT NULL REFERENCES artists(id),   -- NOT NULL
    album_id INTEGER REFERENCES albums(id),              -- nullable
    duration_ms INTEGER,
    tidal_id INTEGER UNIQUE,
    source TEXT NOT NULL DEFAULT 'tidal',
    is_favorite INTEGER DEFAULT 0,
    play_count INTEGER DEFAULT 0,
    last_played_at TEXT,
    -- ... many more columns
);
```

**`listen_history`** — `schema.rs:172–178`
```sql
CREATE TABLE listen_history (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id),  -- hard FK, enforced
    started_at TEXT NOT NULL,
    duration_listened_ms INTEGER,
    completed INTEGER DEFAULT 0
);
```

**`shuffle_state`** — `schema.rs:165–169`
```sql
CREATE TABLE shuffle_state (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id),  -- hard FK, enforced
    played_at TEXT DEFAULT (datetime('now'))
);
```

**`playback_state`** — `schema.rs:142–152` — `current_track_id` is a nullable FK (`REFERENCES tracks(id)` with no NOT NULL), so it can hold NULL safely.

---

### Option A: Nullable `track_id` + metadata columns on `queue`

**DDL changes required:**
- Rebuild `queue` table (SQLite cannot `ALTER COLUMN` to drop NOT NULL) — requires a CREATE + INSERT + DROP + RENAME sequence.
- Add columns: `pending_artist TEXT`, `pending_title TEXT`, `pending_duration_ms INTEGER`, `lastfm_match_score REAL`.
- `track_id` becomes `INTEGER REFERENCES tracks(id)` (nullable, FK becomes optional).

**Migration cost:** Medium. One table rebuild; safe to do in a migration transaction. No other tables change.

**Read query complexity (`load_queue`)** — `noor-server/src/playback/queue.rs:39–68`
- The current `JOIN tracks t ON q.track_id = t.id` becomes `LEFT JOIN tracks t ON q.track_id = t.id`.
- Every column in the SELECT needs a `COALESCE(t.title, q.pending_title)` guard.
- `QueueItem` model (`db/models.rs:98–110`) must either flatten these fields or carry an `Option<Track>` instead of `Track`.
- Every downstream call site that does `item.track.id` or `item.track.tidal_id` must handle the None/zero case.

**Write path changes:**
- `append_tracks_with_reasons` (`queue.rs:93–112`) inserts `track.id` into `track_id`. A new variant is needed for pending rows that writes NULL to `track_id` and fills `pending_artist`/`pending_title` instead.
- `replace_queue` and `move_queue_item` are position-only and are unaffected.
- `normalize_positions` (`queue.rs:299–332`) is position-only and unaffected.

**Failure modes / edge cases:**
- `playback_state.current_track_id` is a nullable FK to `tracks(id)`. If the playing track is a pending row (no real ID), `current_track_id` cannot record it. `load_state` (`player.rs:183–227`) resolves `current_track_id` to a `Track` object — needs a new code path for pending playback.
- Any code that does `queue::get_track_by_id(conn, track_id)` for a NULL ID will get None, which callers may not expect.
- Bootstrap clears the queue on startup (`main.rs:209`); pending rows would be lost on restart like everything else (acceptable).

**`reason` column:** Already on `queue` table; carries through unchanged.

---

### Option B: Parallel `pending_queue` table

**DDL changes required:**
- No changes to existing tables.
- New table:
  ```sql
  CREATE TABLE pending_queue (
      id INTEGER PRIMARY KEY,
      position INTEGER NOT NULL,
      artist_name TEXT NOT NULL,
      title TEXT NOT NULL,
      duration_ms INTEGER,
      lastfm_match_score REAL,
      reason TEXT,
      source TEXT DEFAULT 'radio',
      created_at TEXT DEFAULT (datetime('now'))
  );
  ```

**Migration cost:** Low — purely additive.

**Read query complexity:**
- `load_queue` must UNION ALL both tables into a unified position-ordered result, synthesizing a common row shape. The `QueueItem` model needs a discriminator field (`is_pending: bool` or `pending: Option<PendingInfo>`).
- Alternatively, a database VIEW could merge them, but SQLite views don't participate well in INSERT/UPDATE operations.
- The frontend `QueueItem` type would need the same discriminator.

**Write path changes:**
- Two separate insert paths: existing `append_tracks_with_reasons` for library rows; new `append_pending_tracks` for last.fm rows.
- Position coordination across two tables is the critical problem: to interleave pending and library rows at specific positions, every INSERT must atomically claim the next position across both tables, or positions will collide. This requires a global sequence or a position-allocation transaction.

**Failure modes / edge cases:**
- Position collisions if two concurrent queue-build operations race between tables.
- `remove_queue_item` (`queue.rs:135`) deletes by `queue.id` — needs a parallel deletion for `pending_queue.id`.
- `normalize_positions` operates on the `queue` table only — it would leave `pending_queue` positions un-normalized, corrupting the interleaved ordering.
- Clear-on-restart (`main.rs:209`) only clears `queue`; `pending_queue` would survive restarts (may or may not be desirable).

**`reason` column:** On `pending_queue` table directly; `queue` table unchanged.

---

### Option C: Sentinel rows in `tracks` with `source='pending_lastfm'`

**DDL changes required:**
- The `tracks.artist_id INTEGER NOT NULL REFERENCES artists(id)` constraint is the blocker. Options:
  - Insert a global phantom artist row (`id=0, name='Pending'`) and reference it for all pending tracks. Requires allowing `id=0` in artists (currently auto-increment starting at 1) — or use a fixed sentinel like `id=-1` (SQLite allows negative PKs).
  - Rebuild `tracks` to make `artist_id` nullable. High migration cost; touches many query call sites.
- No queue table changes needed.

**Migration cost:** Medium-high. Phantom artist approach is lower cost but fragile. Making `artist_id` nullable requires a table rebuild and cascading query guard changes.

**Read query complexity (`load_queue`):**
- JOIN structure is unchanged — `tracks` row exists with real FK.
- Display layer must check `t.source = 'pending_lastfm'` to render differently (no artwork, show "resolving…").
- `load_queue` returns a full `QueueItem` with a `Track` that has sentinel values (NULL `tidal_id`, NULL `album_id`, dummy `artist_id`).

**Write path changes:**
- Radio must INSERT into `artists` (or use phantom artist ID), INSERT into `tracks` with minimal fields, then INSERT into `queue` normally.
- `append_tracks_with_reasons` can be called as-is once the sentinel `Track` row exists.

**Failure modes / edge cases:**
- `listen_history` and `shuffle_state` FKs still point to `tracks(id)` — a sentinel track row satisfies the FK, so history writes for pending tracks would succeed at the DB level. But those rows would have no meaningful enrichment, and the listen count on the sentinel row would be misleading.
- GC complexity: sentinel rows must be deleted if never promoted. Deleting a `tracks` row that has `listen_history` rows will fail due to FK from `listen_history`. GC must delete history first, then track — or history FK becomes cascade-delete.
- Library browsing will surface phantom "Pending" artist and tracks unless every list query adds `AND source != 'pending_lastfm'`. High risk of polluting the library view.
- `play_count`, `last_played_at`, `is_favorite` on the sentinel row are meaningless until promotion.

**`reason` column:** On `queue` table, unchanged.

---

## 2. Display Logic Findings

### Current queue row rendering

Location: `noor-client/src/routes/+layout.svelte:1190–1291`

The queue panel is rendered inline in the layout (no separate component). Fields used per row:

| Field | Source | Used for |
|---|---|---|
| `item.track.artwork_url` | `albums.artwork_url` via JOIN | Thumbnail image |
| `item.track.title` | `tracks.title` | Track title |
| `item.track.artist_name` | `artists.name` via JOIN | Artist link |
| `item.track.duration_ms` | `tracks.duration_ms` | Formatted duration |
| `item.track.is_favorite` | `tracks.is_favorite` | Heart icon state |
| `item.track.artist_id` | `tracks.artist_id` | Whether artist name is a clickable link |
| `item.reason` | `queue.reason` | "Why is this here" tooltip |
| `item.source` | `queue.source` | CSS styling class |

### Fields absent for a pending entry

| Field | Status for pending row |
|---|---|
| `artwork_url` | NULL (no album yet) |
| `artist_name` | Available from last.fm response |
| `duration_ms` | **Not available from last.fm** (`LastFmSimilarTrack` has no duration). Available from Tidal after resolution, but resolution hasn't happened yet. |
| `is_favorite` | Meaningless until promotion (always false) |
| `artist_id` | 0 or phantom ID — clickable artist link would be broken |
| `album_id` | NULL |
| `genres` | Not displayed in queue rows currently; genre data is in separate tables |
| `play_count` / `last_played_at` | Not displayed in queue rows currently |

### TypeScript type changes needed

**`QueueItem`** — `noor-client/src/lib/api/client.ts:260–273`
```typescript
export interface QueueItem {
  id: number;
  position: number;
  source: string;
  track: Track;
  reason?: string | null;
}
```
For pending rows, `track` would still be the same shape but with nullable/zero fields (`artist_id=0` or phantom, `artwork_url=null`, `tidal_id=null`). The frontend needs a way to know the row is pending — either via `source === 'pending_lastfm'` (already present on `queue.source`) or a new `is_pending: boolean` field.

### The `startSongRadio` filter

**Location:** `noor-client/src/lib/stores/player.ts:761–763`
```typescript
const inLibrary = queue.tracks.filter(
  (t) => t.is_in_library && t.track_id > 0 && t.track_id !== seedTrackId,
);
```
This three-part filter drops all last.fm hits. Under Option A or B, the backend's radio response (`RadioCandidate` struct at `radio.rs:52–71`) already has `is_in_library: false` and `track_id: 0` for last.fm hits — the frontend would need a new path that accepts these alongside the `inLibrary` set. Under Option C, last.fm hits would have real positive `track_id` values (sentinel row IDs) and `source: 'pending_lastfm'` — the filter condition would need to check source rather than `is_in_library`.

---

## 3. Playback Chain Findings

### Current chain: "user clicks next" → audio buffer fills

1. **Frontend:** User clicks next → `playNextTrack()` (`player.ts:285–309`) → `POST /api/playback/next` (no body).
2. **Backend:** `POST /api/playback/next` handler in `routes.rs` calls `player::advance_queue(conn, state)`.
3. **`advance_queue`** (`player.rs`): reads the next `queue` row by position, calls `play_track(conn, track_id)`.
4. **`play_track`**: sets `playback_state.current_track_id = track_id`, resolves the track to get `tidal_id`, calls Tidal for a stream URL.
5. **Stream URL:** Tidal client returns a URL; the backend includes it in the `PlaybackSnapshot` response.
6. **Frontend:** `hydratePlayback(snapshot)` updates `currentTrack` store; audio element src is set.

### Where a pending row would enter resolution

Step 3 is where the problem surfaces. `play_track` expects a real `tracks.id` to resolve `tidal_id`. For a pending row:
- **Options A/B:** `track_id` is NULL — `play_track` receives no ID, must trigger a Tidal search by artist+title at this moment.
- **Option C:** `track_id` points to a sentinel row with no `tidal_id` — `play_track` sees NULL `tidal_id`, must trigger a Tidal search at this moment.

In both cases, Tidal search is currently synchronous (the Tidal client functions in `tidal/client.rs` are `async` but awaited inline before returning the snapshot). A resolution step would fit the same pattern.

### State slot for the resolved `tidal_id` during playback

**`ephemeral_tidal_track`** — defined in `AppState` (`main.rs:52`) as `Option<db::models::Track>`. Currently used for Tidal-browse tracks not in the library. This slot holds a synthetic `Track` with `id = -tidal_track_id` (negative ID convention, `track.ts:16–27` in frontend). This slot is a candidate for reuse: a resolved pending queue row could be stored here during playback, with the queue row updated or left as-is until promotion.

**`playback_state.current_track_id`** — nullable FK (`schema.rs:143`). If the playing track is pending (no real library ID), this field must be NULL during playback. `load_state` (`player.rs:183–227`) resolves `current_track_id` to a full `Track` — if NULL, falls back to None. A fallback path through `ephemeral_tidal_track` is needed, but that slot was designed for Tidal-browse tracks, not pending queue rows.

### Failure modes at resolution time

| Scenario | For pending rows |
|---|---|
| Tidal search succeeds | Can proceed — stream URL obtained |
| Tidal search returns wrong track (cover/remix) | No guard exists; accepted silently |
| Tidal search finds no match | Must skip and advance, or surface error — no path exists today |
| Tidal 429 rate limit | Existing retry logic in Tidal client would apply |
| Network failure | Existing error handling would apply |

---

## 4. Tidal Match Quality

### Signals available at match time

**From last.fm `LastFmSimilarTrack`** — `noor-server/src/metadata/lastfm.rs:20–26`
```rust
pub struct LastFmSimilarTrack {
    pub artist: String,
    pub title: String,
    pub mbid: Option<String>,      // MusicBrainz ID — sometimes present
    pub match_score: f64,          // 0..1 collaborative filtering confidence
}
```
- **Duration:** NOT available. Last.fm's similar-tracks response does not include duration.
- **ISRC:** NOT available.
- **MusicBrainz ID:** Sometimes present.

**From Tidal `TidalSearchTrack`** — `noor-server/src/services/tidal/client.rs:84–96`
```rust
pub struct TidalSearchTrack {
    pub id: i64,
    pub title: String,
    pub duration: i64,             // SECONDS — present
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub album_id: Option<i64>,
    pub artwork_url: Option<String>,
    pub audio_quality: Option<String>,
    pub stream_ready: Option<bool>,
    pub extra: HashMap<String, serde_json::Value>,
}
```
- **Duration:** YES — in seconds, convertible to ms.
- **Artist name:** YES.

### Confirmation signals available

| Signal | Available | Notes |
|---|---|---|
| Artist name fuzzy match | YES | last.fm `artist` vs Tidal `artist_name` — both present |
| Title fuzzy match | YES | Both present |
| Duration match | PARTIAL | Tidal has it; last.fm does not. No cross-check possible without a third-party source for last.fm track durations. |
| ISRC match | NO | Neither side carries ISRC at this stage |
| MusicBrainz ID | MAYBE | Last.fm sometimes provides `mbid`; Tidal does not expose MB IDs natively |

**Practical options for confirming a match:**
- Reject if Tidal `artist_name` doesn't fuzzy-match last.fm `artist` above some threshold (e.g., Jaro-Winkler > 0.85).
- Reject if Tidal `title` doesn't fuzzy-match last.fm `title` (catches obvious wrong-track cases; misses "feat." variants and remixes less reliably).
- No duration cross-check is possible from last.fm's side alone.

---

## 5. System Interaction Matrix

| System | Code location | Behavior with pending queue row |
|---|---|---|
| `build_automix_extension` | `player.rs:661–787` | Reads `item.track.id` from queue rows for exclusion list. A `track_id=0` or NULL is included as `0` — harmless but meaningless. Automix seed resolution uses the *currently playing track*, not pending queue rows. **Degrades gracefully.** |
| `apply_genre_signals` | `radio.rs:798–838` | Passes through candidates with no Jaccard entry (`return true` at line 817). Last.fm rows with `track_id=0` have no Jaccard entry. **Silent no-op.** |
| `apply_taste_signals` | `radio.rs:676–698` | Explicitly checks `cand.track_id == 0 \|\| ...` — last.fm candidates with `track_id=0` are always retained. **Safe by design.** |
| `listen_history` insert | `queries.rs:1155–1163` | `track_id INTEGER NOT NULL REFERENCES tracks(id)`. Options A/B: NULL/0 track_id → **FK violation, hard fail**. Option C: sentinel row satisfies FK → inserts, but records meaningless listen data. |
| `shuffle_state` insert | `schema.rs:165–169` | `track_id INTEGER PRIMARY KEY REFERENCES tracks(id)`. Same as listen_history. Options A/B: **hard fail**. Option C: inserts with sentinel ID. |
| `favorites` toggle | `routes.rs:3950–3953` | `UPDATE tracks SET is_favorite=... WHERE id=?`. Options A/B: no `tracks` row → **UPDATE 0 rows, silent no-op**. Option C: sentinel row gets `is_favorite=1` — meaningless until promotion, but doesn't crash. Tidal sync fires only if track has `tidal_id` — pending rows don't. **Silent no-op for sync.** |
| Library browsing queries | Various routes in `routes.rs` | Option C: sentinel tracks pollute library views unless every query adds `AND source != 'pending_lastfm'`. High surface area. Options A/B: unaffected. |
| Clear-on-restart | `main.rs:209` | `DELETE FROM queue` — clears all queue rows. Option B `pending_queue` survives restart. Options A/C: pending rows cleared with queue on restart. |
| `remove_queue_item` | `queue.rs:135` | Deletes by `queue.id`. Works correctly for any queue row. **Unaffected.** |
| `normalize_positions` | `queue.rs:299–332` | Operates on `queue` only. Option B: `pending_queue` positions become stale after queue reorder/remove operations. **Breaks interleaved ordering under Option B.** |

---

## 6. Promotion Signals Available

The active listen session (`AppState`, `player.rs:57–62`) accumulates:
```rust
pub struct ActiveListenSession {
    pub track_id: i64,
    pub started_at: DateTime<Utc>,
    pub accumulated_ms: i64,     // total played across play/pause cycles
    pub resumed_at: Option<DateTime<Utc>>,
}
```

At session flush (`routes.rs:7756–7794`), the following is captured:
- **`duration_listened_ms`** — total milliseconds actually heard.
- **`completed`** — boolean from `is_completed_listen()` (`player.rs:608–612`), roughly 80% of track duration for tracks >150s.
- **`FlushReason`** enum (`player.rs:90–94`): `Replaced` (new track started), `QueueEnded`, `Stopped` — captures why the session ended. "User skipped" is implicit (`Replaced` with short `accumulated_ms`).

**Additional user-explicit signals:**
- **Favourite:** `routes.rs:3950` — user tapped the heart. Currently a silent no-op for non-library tracks.
- **"Play next" reorder:** No dedicated event captured beyond the queue move itself.
- **Skip:** No explicit skip event table — skip is inferred from `completed=0` + short `accumulated_ms` + `FlushReason::Replaced`.

**What is NOT available without new instrumentation:**
- Completion percentage as a first-class field (must be derived: `accumulated_ms / track.duration_ms`).
- Explicit skip event (must be inferred from session data).
- Repeat plays of a single queue row before advance.

---

## 7. Garbage Collection Surface

**No scheduled GC exists.** Queue rows accumulate until one of:

| Event | Code | Scope |
|---|---|---|
| Server restart | `main.rs:209` `DELETE FROM queue` | Full queue wipe |
| User clears queue | `queue.rs:129–131` `clear_queue()` | Full queue wipe |
| User removes one item | `queue.rs:135` `remove_queue_item()` | Single row |
| Track deleted from library | `duplicates.rs:625` `DELETE FROM queue WHERE track_id = ?1` | Rows for that track |
| "Now playing only" action | `routes.rs:5404` `DELETE FROM queue WHERE track_id != ?1` | All except current |

**No TTL, no scheduled job, no event-driven expiry for stale pending rows.**

In practice, `replace_queue` (`queue.rs:114–117`) calls `DELETE FROM queue` before inserting, so starting a new radio session wipes old pending rows. Queue accumulation is mainly a concern if users append many radio queues without replacing.

For Option C: sentinel `tracks` rows are harder to GC — they live in the `tracks` table and may have `listen_history` rows. Deleting them requires cascade awareness or a deletion ordering guarantee.

---

## 8. Top 5 Risks (Ranked by Severity)

### Risk 1 — `listen_history` hard FK breaks audio advancement for pending rows
**Severity: Critical**
When a pending queue row advances to current track, the session flush writes to `listen_history` with the pending row's track ID. Under Options A/B, this is NULL or 0, causing a FK constraint violation that crashes the flush handler (`routes.rs:7756–7794`). Under Option C, the sentinel row satisfies the FK but records meaningless history. All three options require explicit handling: either skip the history write for pending rows, or guarantee promotion before the first play.
- `queries.rs:1155–1163`, `routes.rs:7756–7794`

### Risk 2 — `playback_state.current_track_id` cannot represent a pending track (Options A/B)
**Severity: High**
`current_track_id` is a FK to `tracks(id)`. For a pending row with no library ID, this field must be NULL during playback. `load_state` (`player.rs:183–227`) resolves `current_track_id` to a `Track` — if NULL, it falls back to None. The frontend's now-playing card would show nothing. A fallback path through `ephemeral_tidal_track` (`main.rs:52`) is needed, but that slot was designed for Tidal-browse tracks, not pending queue rows.

### Risk 3 — Tidal search match quality: covers/remixes accepted without guard
**Severity: High**
When a pending row reaches play time and triggers Tidal search by artist+title, the first result is accepted. "Lizzo - Truth Hurts" could resolve to a cover. The only available confirmation signals are artist-name and title fuzzy match (both sides have them). Duration cross-check is not possible (last.fm provides no duration). Without a minimum artist-name similarity threshold, wrong-track resolution is silent.
- `tidal/client.rs:355–398`, `radio.rs:52–71`

### Risk 4 — Option C pollutes the library with sentinel tracks
**Severity: High (if Option C chosen)**
Sentinel `tracks` rows with `source='pending_lastfm'` appear in any query that doesn't filter them out. Library track counts, artist pages, and browse views would show pending tracks unless every list query is patched. Surface area is wide — all `SELECT` queries against `tracks` in `routes.rs` without a source filter.

### Risk 5 — Option B position coordination across two tables
**Severity: Medium (if Option B chosen)**
Interleaving positions from `queue` and `pending_queue` requires atomic position allocation. `normalize_positions` (`queue.rs:299–332`) only operates on `queue`, leaving `pending_queue` with stale positions after any reorder or remove operation. Without careful transaction coordination, the unified queue ordering breaks silently.

---

## 9. Top 5 Questions Before Implementation

1. **Listen history for pending rows:** Should playback of a pending row (before promotion) write to `listen_history` at all? If yes, using what ID — the pending row's eventual library ID after promotion, or skipped entirely? If no, what threshold (0 ms? entire track?) triggers history skipping?

2. **`current_track_id` during pending playback:** The `playback_state` table's `current_track_id` FK can't hold a non-library ID (Options A/B). Should playback of a pending row store state in `ephemeral_tidal_track` (reusing the existing slot with its negative-ID convention) or in a new `playback_state` column? What does the now-playing card show?

3. **Tidal resolution timing:** Should resolution (Tidal search by artist+title) happen when the pending row is inserted into the queue (eager, at radio-build time), or lazily when the row advances to current track? Eager spreads latency over queue-build time; lazy blocks playback at track-change time. What's the acceptable UX tradeoff?

4. **Artist-name confirmation threshold for Tidal matches:** What minimum fuzzy-match score is acceptable before accepting a Tidal result for a last.fm hit? And what happens on rejection — skip the pending row and advance, mark it as "unresolvable", or surface a user-visible error?

5. **Queue persistence across restarts:** Currently `DELETE FROM queue` runs on server startup (`main.rs:209`). Should pending rows survive restarts, or is clearing them on restart acceptable? Option B's `pending_queue` would survive by default; Options A/C would be cleared. Is that asymmetry a problem?

---

## 10. Pre-Implementation Sizing (added post-inventory)

### Decisions locked

- **Schema: Option A** — nullable `track_id` + metadata columns on `queue`.
- **Listen history:** skip entirely until promotion. No backfill.
- **`current_track_id`:** reuse `ephemeral_tidal_track` slot; `current_track_id` stays NULL during pending playback.
- **Resolution timing:** background-eager with synchronous fallback at play time.
- **Match rejection:** Jaro-Winkler 0.85 on artist name only; log rejections, mark row unresolvable, skip silently.
- **Restart behaviour:** clear pending rows with the queue on restart.
- **Schema addition:** add `tidal_match_score REAL` and `resolution_state TEXT` (pending/resolved/unresolvable) columns alongside `pending_artist`, `pending_title`, `pending_duration_ms`, `lastfm_match_score` when rebuilding the `queue` table.

### (b) Concurrent queue-build — transaction boundaries

**Finding: safe as-is, but fragile.** All queue-write routes wrap their DB calls inside a single `with_conn(|conn| { ... })` closure (`routes.rs:5390–5406`). The `Database::with_conn` function (`db/mod.rs:12–46`) holds `Arc<Mutex<Connection>>` for the closure's full duration, so the entire DELETE + INSERT sequence for `replace_queue` runs under one mutex acquisition — concurrent `replace_queue` calls are serialized.

The exposed gap: `replace_queue` and `append_tracks_with_reasons` (`queue.rs:114–117`, `queue.rs:93–112`) have no explicit `BEGIN TRANSACTION` / `COMMIT`. A process crash between the DELETE and the first INSERT leaves the queue empty but consistent. A future call site that invokes queue functions across multiple `with_conn` calls would reintroduce races. The existing code is safe; it's a latent risk that any new call site must respect.

**Action:** No transaction fix required before 2c-ii-a. Document the invariant: all queue mutations must happen inside a single `with_conn` closure. The background resolver (Phase 2, Option A) must follow this.

### (c) Frontend `QueueItem.track` blast radius

Making `QueueItem.track` optional (or adding `is_pending`) touches **5 files, ~49 access locations**:

| File | Direct `item.track.X` accesses | Notes |
|---|---|---|
| `routes/+layout.svelte` | ~28 | Queue row rendering (two render sites) + now-playing card uses `$currentTrack` (separate store, unaffected) |
| `lib/stores/player.ts` | ~9 | Business logic: queue filtering, state mutations |
| `routes/automix/+page.svelte` | ~5 | Harmonic compat calc reads `item.track.id` |
| `routes/duplicates/+page.svelte` | ~6 | Duplicate list rendering |
| `lib/api/client.ts` | 1 | `QueueItem` interface definition |

The `$currentTrack` references in `+layout.svelte` and `routes/+page.svelte` are from `PlaybackState.current_track: Track | null` (a different store/type) — unaffected by the `QueueItem` type change.

The `t.track_id` references in `player.ts:762–945` are `RadioCandidate.track_id` from the radio API response — also a different type, unaffected.

**Recommended approach:** keep `QueueItem.track` as `Track` (non-optional), populate pending rows with sentinel values that are valid for display (`artist_name` from last.fm, `title` from last.fm, `duration_ms: null`, `artwork_url: null`, `tidal_id: null`). Use `item.source === 'radio_pending'` as the discriminator in the Svelte component. This avoids the Option<Track> blast radius entirely and confines frontend changes to the queue-row render site and the `startSongRadio` filter in `player.ts`.

### (a) Tidal match quality — measurement approach

The existing `scripts/tidal-genre-probe` binary (`scripts/tidal-genre-probe/src/main.rs`) is a direct template: it reads the Tidal token from `noor.db`, makes authenticated Tidal API calls, and reports structured results. A new `scripts/tidal-match-probe` binary following the same pattern would:

1. Accept a seed track (e.g. "Amy Shark — I Said Hi") and call last.fm `track.getSimilar` for 50 results.
2. For each `LastFmSimilarTrack`, call `GET /search?query={artist}+{title}&types=TRACKS` via the same Tidal v1 surface.
3. Compare the top result's `artist_name` against the last.fm `artist` using Jaro-Winkler (the `strsim` crate already used in the probe).
4. Report: match/mismatch, similarity score, whether the top-result artist is a word-for-word match or a plausible variant.

This can run against the live DB and token in ~2 minutes. It should be written and run **before** the 0.85 threshold is hardcoded — the actual distribution of scores will tell you whether 0.85 is conservative or too loose.
