# Phase 2b inventory: relevance-aware scoring + reason surfacing

Pre-implementation read of the integration points for Phase 2b. Captured
2026-04-30 ahead of the genre coherence + DSP distance scoring work.

**Headline finding**: DSP feature coverage is **1.70%** of the library
(602 / 35,407 tracks). The Phase 2b brief named 80% as the minimum
viable coverage and instructed me to halt and surface this if it came
in below. **Halting before implementation.** Detail and options at the
bottom.

Genre coverage is healthy (86.6%) and the genre coherence goal is fine
to proceed independently if you want to split the phase.

## Backend: services/radio.rs integration points

### Where the enrichment pass slots in

In `orchestrate_song`, [services/radio.rs:210](../noor-server/src/services/radio.rs#L210):

```rust
let mut combined = combine_with_dedup(library_results, lastfm_results, engine_results);
apply_taste_signals(&mut combined, &taste, &resolver);
let ordered = blend_interleave(combined, blend, limit);
```

The deduped candidate list with library/engine `track_id` populated
exists exactly between `combine_with_dedup` and `apply_taste_signals`.
Library and engine candidates have real `track_id`s; lastfm hits have
`track_id = 0`. This is the integration slot for Phase 2b's enrichment
pass.

### `apply_taste_signals` after Phase 2a fix

Signature:
```rust
fn apply_taste_signals(
    candidates: &mut Vec<RadioCandidate>,
    taste: &TasteVector,
    resolver: &ArtistResolver,
)
```

Constants:
- `AFFINITY_SATURATION = 10.0`
- `AFFINITY_SCALE_POS = 0.20`
- `AFFINITY_SCALE_NEG = 0.30`
- `AFFINITY_FLOOR = 0.1`

Multiplier formula:
```rust
let pos_c = affinity.pos / (affinity.pos + 10.0);
let neg_c = affinity.neg / (affinity.neg + 10.0);
let multiplier = 1.0 + (pos_c * 0.20) - (neg_c * 0.30);
similarity_score *= multiplier.max(0.1);
```

### `RadioCandidate.reason` format today

The string is shaped per source:

| Source | Format | Example |
|---|---|---|
| Library (embedding) | `"library similarity {:.2}"` | `"library similarity 0.87"` |
| Last.fm | `"Last.fm match {:.2}"` | `"Last.fm match 0.95"` |
| Engine (track_similarity) | `"library similarity {:.2} (co-album {:.2}, co-artist {:.2}, co-listen {:.2}, genre {:.2})"` | `"library similarity 0.75 (co-album 0.60, co-artist 0.50, co-listen 0.40, genre 0.70)"` |

The engine path **already** includes a component breakdown — the format
change Phase 2b proposes is to extend all three with a structured JSON
suffix that the frontend can parse. The existing prefix stays for
backwards compatibility.

### Consumers of `reason`

Backend: only `routes.rs:2264` serialises it as `radio_reason` in JSON
responses. No other consumers internally.

Frontend (per agent 3 below): only the Discover Space UI reads
`radio_reason` — at [DiscoverPanel.svelte:150](../frontend/src/lib/components/Discover/DiscoverPanel.svelte#L150)
and [DiscoverHoverCard.svelte:93-95](../frontend/src/lib/components/Discover/DiscoverHoverCard.svelte#L93-L95).
**The queue panel does not display reason today** — `QueueItem` doesn't
even carry the field. Phase 2b needs to plumb it through the queue
payload.

## Backend: db/queries.rs — genre + DSP queries

### Genre fetch — batch exists

[queries.rs:580](../noor-server/src/db/queries.rs#L580):
```rust
pub fn get_track_genre_paths(conn: &Connection) -> Result<HashMap<i64, Vec<String>>>
```
Loads **every** track's genre paths in one call via a recursive CTE.
Returns full path strings like `"Electronic > House"`. Suitable for
session-scope caching; not selective by track_id list.

[queries.rs:1632](../noor-server/src/db/queries.rs#L1632) — inside
`get_genre_cohort_assignments`, a `SELECT track_id, genre_id FROM
track_genres WHERE track_id IN (...)` pattern exists. We can extract
this shape into a new selective batch query:
`fn get_genres_for_tracks(conn, &[i64]) -> Result<HashMap<i64, Vec<i64>>>`.

### DSP fetch — single + whole-library, no selective batch

| Function | Shape | File |
|---|---|---|
| `get_audio_dsp_features(conn, track_id)` | Single track | queries.rs:3219 |
| `get_all_audio_dsp_features(conn)` | All analysed tracks | queries.rs:3319 |

No `get_audio_dsp_features_batch(conn, &[i64])` exists. Adding one is
in scope for Phase 2b if we proceed with DSP scoring.

`AudioDspFeatures` carries: `bpm`, `key_signature`, `camelot_key`,
`loudness_lufs`, `energy`, `danceability`, `beat_strength`,
`spectral_centroid`, `stereo_width`, `is_instrumental`, plus
`analysis_source`/`offset`/`samples`/`analyzed_at`/`version` metadata.

### audio_dsp_features schema

Defined at [schema.rs:451-510](../noor-server/src/db/schema.rs#L451-L510)
(MIGRATION_009).

- Primary key: `track_id` (INTEGER, references tracks.id, ON DELETE
  CASCADE).
- Most numeric features (`bpm`, `energy`, `danceability`,
  `beat_strength`, etc.) are nullable. Per the schema comments, `energy`
  / `danceability` / `beat_strength` / `stereo_width` are normalised to
  `[0.0, 1.0]`.
- `is_instrumental: INTEGER NOT NULL DEFAULT 0` — boolean.
- Indexes on `bpm`, `key_signature`, `energy` (`idx_dsp_bpm`,
  `idx_dsp_key`, `idx_dsp_energy`).
- track_id is the primary key, so any `WHERE track_id IN (...)` batch
  fetch hits the primary index directly.

## Backend: smart/genre/builder.rs — hierarchy + distance

The taxonomy ships as a JSON file (`genre-taxonomy/taxonomy.json`),
loaded at startup via `taxonomy::ensure_taxonomy_loaded` and persisted
into the SQL `genres` table with `parent_id` foreign keys. **Tree, not
DAG.**

### What's exposed

- No Rust function for "expand a genre to its ancestors" exists.
  Ancestor walks are done via SQL recursive CTEs (e.g. inside
  `get_track_genre_paths`).
- One Jaccard helper exists: `get_genre_co_occurrence` at
  [queries.rs:1422-1476](../noor-server/src/db/queries.rs#L1422-L1476) —
  but it computes co-tagged-on-same-track Jaccard, not
  ancestor-aware Jaccard. Not directly reusable for Phase 2b.

### Ancestor expansion cost

Cheapest path the agent identified: load `get_track_genre_paths()` once
at scoring start, build an in-memory `genre → ancestors` map from the
path strings (split on `" > "`), then compute Jaccard with ancestor
expansion in pure Rust. Estimated **5–10 µs per candidate pair** on hot
HashMap. Acceptable for ~50 candidates per radio call.

If we don't bother with ancestor expansion (just compare leaf genre
sets directly), Jaccard is trivially cheap.

### Surface-back: cost is fine

The brief listed "genre hierarchy expansion is more expensive than
expected" as a halt trigger. Conclusion: **not a halt**. The
amortise-once-per-call pattern is straightforward and the per-pair cost
is microseconds.

## Backend: playback/player.rs — automix's same-genre logic for primitive reuse

[player.rs:1036-1045](../noor-server/src/playback/player.rs#L1036-L1045):

```rust
let normalized_genres = genres.iter().map(|genre| normalize_genre_key(genre));
for genre in normalized_genres {
    if seed.genres.contains(&genre) {
        score += 1.8;
    }
    if let Some(affinity) = taste.genre_affinity.get(&genre) {
        score += affinity.pos * 0.4;
        score -= affinity.neg * 0.5;
    }
}
```

Match keyed by **lowercased name string** (`HashSet<String>`), not
genre_id. The `normalize_genre_key` function at
[player.rs:1115](../noor-server/src/playback/player.rs#L1115) is
module-level; trivially reusable from `smart/`. The `+1.8` block
itself is inline-only and tightly coupled to `automix_score`'s scope.
Phase 2b will not reuse the score-update logic; it builds a separate
multiplicative scorer (per the brief's locked formula). The
**normaliser** is the only primitive worth sharing.

### `compute_harmonic_multiplier` in services/audio_analysis/mod.rs

Lives at
[services/audio_analysis/mod.rs:131-163](../noor-server/src/services/audio_analysis/mod.rs#L131-L163).
Inputs `(seed_camelot, cand_camelot, seed_bpm, cand_bpm)`, returns one
`f64` multiplier. Exact match ×2.2; adjacent ×1.4; incompatible ×0.6.
BPM within 5 ×1.8; within 10 ×1.3; within 20 ×0.9; >20 ×0.65.

This is already a clean shared helper — radio can call it directly if
we proceed with DSP scoring.

### Hard-stop reminder

Per the brief, **no scoring code changes in player.rs in Phase 2b.**
If radio needs a primitive that lives in player.rs, extract to
`smart/`. The only primitive of interest is `normalize_genre_key`,
which is already at module scope.

## Frontend: queue panel + reason surfacing

### Where the queue is rendered

[frontend/src/routes/+layout.svelte:1140-1214](../frontend/src/routes/+layout.svelte#L1140)
(desktop sidebar) and 1475-1527 (mobile player). Iterates over
`upcomingQueue.slice(0, 40)` with each row showing artwork, source dot,
title, artist link, duration, and action buttons (favorite, more, play
next, remove).

### `QueueItem` type missing `reason`

[client.ts:260-265](../frontend/src/lib/api/client.ts#L260-L265):
```typescript
export interface QueueItem {
  id: number;
  position: number;
  source: string;
  track: Track;
}
```

No `reason`. Radio results carry `reason` on `RadioCandidate` but the
frontend's radio flow currently does:
1. POST `/api/radio/song`, get `RadioCandidate[]`.
2. Convert to track ids.
3. POST `/api/playback/queue` (replace) with track ids only.
4. The queue persisted server-side is `Vec<QueueItem>` — no `reason`
   field on `queue` table, no `reason` carried through.

So the reason string is **discarded at the frontend → backend boundary
during the radio → queue handoff.** To surface it in the queue panel
we'd need to either:
- Add a `reason` column to the `queue` table and plumb it through.
- Cache the radio response client-side and look up reason by track_id
  for each queue item (only works for the radio that initiated the
  current queue; falls apart for automix-extended items, manually-added
  items, etc).
- Recompute the reason on demand from a new endpoint.

This is a Phase 2b architectural call, not a trivial frontend change.

### Reusable hover-card pattern

[DiscoverHoverCard.svelte](../frontend/src/lib/components/Discover/DiscoverHoverCard.svelte)
already shows reason text in a `.provenance` block:

- Trigger: hover with `mouseX`/`mouseY` props for positioning, viewport-aware.
- Style: `rgba(13, 13, 26, 0.95)` + `backdrop-filter: blur(8px)`, 1px border, 100ms fade-in.
- Content: track meta, optional chips (BPM, key, energy, genre), source badge, then reason as 10px gray text with top border.

Adapting this for the queue panel is straightforward — the visual
language and trigger pattern are exactly what Phase 2b needs.

### Surface-back: reason consumer not breaking

The brief's "reason string format change breaks an existing consumer
not in the inventory" trigger: only DiscoverHoverCard and
DiscoverPanel display `radio_reason`. Both render it as plain text. If
we extend the format with a JSON suffix, those components will still
render the prefix correctly (string display doesn't care about the
suffix). **Not a halt.**

## DSP coverage findings — the load-bearing surface-back

Verified against the live DB at /e/NOORwave/noor.db.

### Library-wide

| Metric | Value | % of tracks |
|---|---|---|
| Total tracks | 35,407 | — |
| Rows in `audio_dsp_features` | 602 | 1.70% |
| Rows with `bpm` populated | 601 | 1.70% |
| Rows with `energy` populated | 602 | 1.70% |
| Rows with both | 601 | 1.70% |

### Doja Cat seed 1634 (canonical regression seed)

| Metric | Value |
|---|---|
| `track_similarity` neighbours | 33 |
| Neighbours with `bpm` | **0** |
| Neighbours with `energy` | **0** |

### Genre coverage for comparison

| Metric | Value | % of tracks |
|---|---|---|
| Total tracks | 35,407 | — |
| Tracks with at least one genre | 30,675 | **86.6%** |
| Total genre assignments | 77,565 | avg 2.5/track |
| Doja Cat seed neighbours with genre | 33 of 33 | 100% |

### Conclusion

The brief said `>20%` of tracks lacking DSP features = halt and
discuss. Actual: **98.3%** lack DSP features. The Phase 2b DSP
distance scoring would silently no-op on virtually every candidate
pair, and the Stage 2 gate (which compares before/after diagnostic
output) would have nothing meaningful to compare.

**The genre coherence signal is unaffected** — 86.6% coverage clears
the threshold comfortably.

## Options to surface back

Three paths forward. All three preserve Phase 1 / Phase 2a's commits;
the DSP audio analysis pipeline is a separate concern.

### A. Split Phase 2b into 2b-genre and 2b-dsp

Implement Phase 2b with **only the genre coherence goal** plus the
reason-string surfacing. Defer DSP distance scoring to a later
phase that runs after the audio-analysis pipeline backfills the
library. The genre goal alone is still substantial work and the
diagnostic harness can validate it independently.

This is the lowest-risk path. Phase 2b ships meaningful improvements
without depending on data we don't have.

### B. Backfill DSP features first, then proceed with full Phase 2b

Run the audio-analysis pipeline against the library to get DSP
coverage above 80% before starting Phase 2b implementation. This
is a separate task with its own runtime cost (analysis is per-track
DSP work, possibly hours of CPU time across 35k tracks). Once
coverage is up, Phase 2b proceeds as planned.

This is the correct-but-slow path. The DSP scoring goal is real
value when the data exists.

### C. Implement DSP scoring anyway, gate on coverage at runtime

Add the DSP scoring code, but skip the multiplier when seed or
candidate has no DSP features (which `automix_score` already does
via `if let (Some(seed), Some(cand))`). Today this means the
scoring no-ops for ~98% of tracks. As coverage improves, the
scoring fires for more tracks automatically.

This works mechanically but the Stage 2 gate becomes weak: we can't
validate the DSP signal because it never fires on our diagnostic
seeds. We'd be shipping untested scoring logic. Not recommended
unless we accept the gate weakness.

## Questions for review

Before designing the implementation:

1. **Which option (A / B / C / something else) for the DSP coverage
   problem?**
2. The reason-string-in-queue plumbing is a real architectural
   question — adding a column to the `queue` table vs caching
   client-side vs recomputing on demand. **Which preference?**
   The brief assumed reason was already on queue items; it isn't.
3. Genre Jaccard with or without ancestor expansion? Ancestor
   expansion is cheap (5–10 µs/pair) and matches the locked decision
   in the brief, but worth confirming since the brief said "with the
   genre hierarchy expansion applied (parent genres count as
   half-matches)" — does "half-matches" mean we weight ancestor
   matches at 0.5 vs leaf matches at 1.0 in the union/intersection?
   Or some other interpretation?

Awaiting direction before writing the implementation plan.
