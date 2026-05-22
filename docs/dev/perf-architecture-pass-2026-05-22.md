# Performance / architecture pass: 2026-05-22

Branch: `worktree-perf-architecture` (off `master` @ 0.1.56).

Goal: deeply review the codebase for request-speed, reliability, and
architecture wins; ship safe, measurable, reversible changes; avoid broad
rewrites. Everything below was measured against the real dev library
(`noor.db`, 35,535 tracks) using `sqlite3` read-only copies, not estimated.

## What changed

### 1. Ordering indexes for the hot non-library track queries (MIGRATION_043)

The three most-used non-library track queries each did a full `SCAN tracks`
plus a temp B-tree sort over all ~35k rows on every call:

| Query | ORDER BY |
|-------|----------|
| `get_favorite_track_ids` | `is_favorite, play_count DESC` |
| `get_discovery_candidate_tracks` / `get_tracks_excluding_with_limit` | `is_favorite DESC, play_count ASC, fidelity_score DESC, date_added DESC, title ASC` |
| `get_tidal_similar_seed_rows` | `play_count DESC, last_played_at DESC, id DESC` |

Added three indexes so SQLite satisfies each ORDER BY directly (no temp sort)
and stops after `LIMIT` rows:

- `idx_tracks_fav_play (is_favorite DESC, play_count DESC)`
- `idx_tracks_discovery (is_favorite DESC, play_count ASC, fidelity_score DESC, date_added DESC, title ASC)`
- `idx_tracks_play_last (play_count DESC, last_played_at DESC, id DESC)`

**Measured** (500 reps, single process, production-equivalent projection + joins):

| Query | Before | After | Speedup |
|-------|--------|-------|---------|
| favorites | 5.74s | 0.33s | ~17x |
| discovery (real wide projection) | 13.48s | 0.14s | ~95x |
| tidal seed | 25.36s | 0.49s | ~51x |

EXPLAIN QUERY PLAN after: `SEARCH ... USING COVERING INDEX idx_tracks_fav_play`,
`SCAN t USING INDEX idx_tracks_discovery`, `SCAN t USING INDEX idx_tracks_play_last`
(no `USE TEMP B-TREE`).

The default library listing (`get_tracks_with_dsp`, the single most frequent
query) was already served by the existing `idx_tracks_date_added` (~0.22ms/page)
and is intentionally untouched.

**Tradeoff:** three more indexes maintained on track insert/update. Track writes
happen almost entirely inside bulk TIDAL-sync transactions, so the cost is
amortized and acceptable.

Regression test: `migration_043_adds_ordering_indexes_and_avoids_temp_sort`
asserts the indexes exist and that the discovery plan uses the index with no
temp sort.

### 2. Dropped a redundant per-track id lookup in playlist sync

`insert_tidal_track` already resolves the local `tracks.id` (it needs it to
attach source genres) but returned `()`. The playlist-sync loop discarded that
and ran a second `SELECT id FROM tracks WHERE tidal_id=?` for every track.
Changed `insert_tidal_track` to return `Option<i64>` and reused it, halving the
per-track id lookups in playlist sync. Other callers ignore the value via `?`.

The loop's preceding artist upsert is intentionally kept: the album insert that
follows references the artist by `tidal_id` subquery, so the artist must exist
before it.

### 3. Bounded the in-process TTL caches

`tidal_page_modules_cache`, `tidal_playlist_tracks_cache`, and `refreshed_seeds`
only evicted the key being read. Entries for keys never requested again lived
for the whole process lifetime, so the maps grew unbounded over a long session
(the TIDAL ones hold `Vec<TidalTrack>` / `Vec<TidalHomeModule>`, so this was a
real slow memory leak). Now each `put` sweeps expired (and, for `refreshed_seeds`,
stale-model) entries first. Inserts only happen on a cache miss after a
network/embedding fetch, so the O(n) scan is cheap and rare.

Regression test: `tidal_page_modules_cache_evicts_stale_keys_on_insert`.

### 4. Search: apostrophe queries no longer fall back to a full-table scan

`to_fts_query` preserved the apostrophe in tokens. A bare `'` opens a string
literal in FTS5, so every query containing one ("Don't", "Guns N' Roses",
"I'm", "Can't") raised `fts5: syntax error`. `search()` caught it and silently
fell back to `search_tracks_like`, a `LOWER(col) LIKE '%term%'` scan over all
35k tracks plus three joins. Apostrophes are extremely common in titles and
artist names, so this hit real everyday searches.

Fixed by mapping `'` to a separator. The unicode61 tokenizer already splits
indexed text on apostrophes ("Don't" -> "don","t"), so "don't" -> "don* t*"
parses cleanly and matches the indexed tokens (527 hits on the dev library).

**Measured** (apostrophe query, 300 reps):

| Path | Per search |
|------|-----------|
| LIKE fallback (before) | ~62.6ms |
| FTS (after) | ~4.9ms (**~13x**) |

...and results are now FTS-ranked instead of arbitrary substring matches.

Regression tests: `to_fts_query_treats_apostrophe_as_separator`,
`search_handles_apostrophe_queries_via_fts`.

### 5. Bounded external HTTP clients (reliability)

reqwest has no default timeout. Two clients were built without one:

- The **Sportify** client. Its result is `join!`ed with local DB search in the
  `/search` handler, so a hung third-party proxy would hang the entire search
  response (and any `/discover` call). Added 5s connect / 10s total.
- The **shared** client (Last.fm, MusicBrainz, Discogs, RSS, session recovery).
  Added 10s connect / 30s total, matching the existing TIDAL client.

These bound the worst case without affecting sub-second happy-path calls;
best-effort callers already treat an error/timeout as "no data". Streaming
audio downloads keep their own dedicated client.

### Lock-across-await audit: clean

A full audit of `state.write().await` / `state.read().await` guards across
`.await` points in `routes.rs`, the route submodules, and `player.rs` found
**no** write-lock-across-network, write-lock-across-DB, or read-lock-across-
network cases. The code already clones tokens/clients/values out of the guard
and drops it (block scope or explicit `drop`) before any slow `.await`. No
change made; the deferred item was removed from FOLLOWUPS.

## Verification

- `cargo test -p noor-server`: 741 passed, 0 failed, 2 ignored (no regressions).
- `cargo check -p noor-server`: clean (only pre-existing dead-code warnings).
- Index plans/timings measured on read-only copies of the real `noor.db`;
  the live library file was never modified.

## Remaining risks / not done

- The new indexes are validated on a 35.5k-track library. On a much smaller
  library SQLite may still choose a scan (correctly); the indexes are
  `IF NOT EXISTS` and harmless when unused.
- Higher-risk opportunities (lock-held-across-await contention, the embedding
  cache's async mutex used for sync-only work, missing per-request Last.fm
  similar-tracks caching) were left as follow-ups rather than changed here,
  because they alter concurrency/behavior and need their own measurement. See
  FOLLOWUPS.md.
