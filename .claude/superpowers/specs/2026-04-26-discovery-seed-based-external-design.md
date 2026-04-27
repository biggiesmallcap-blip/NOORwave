# Discovery Phase 2 — Seed-Based External Engine

**Date:** 2026-04-26

## Context

The `/discover` page currently calls `/api/discovery/space`, which returns **library** tracks similar to a seed (via embedding nearest neighbors). That's the wrong direction — discover should find tracks the user does **not** already have.

The backend already has external-discovery infrastructure (`/api/discovery/new`, `external_discovery_engine`, Tidal search, library filtering via `existing_candidate_tidal_ids`), but it only accepts a **prompt** as input, not a seed track. So we have two halves of the right tool that don't connect.

This phase wires them together: given a seed track (currently playing or user-locked), find new external Tidal tracks similar to it, filter out anything already in the library, and return the same response shape Phase 1 already plumbed.

Visualization upgrades (legend, tooltips, edge styling, mode rework) are deferred to Phase 3 — once the engine returns the right tracks, then we make the canvas legible.

## Goal

After Phase 2, opening `/discover` while a track is playing produces a canvas of ~60 external Tidal tracks similar to that track, none of them already in the user's library. Locking a seed pins the result. Auto-seeding follows the playing track when no lock is set.

## Architecture

### Data flow

```
seed track (currently playing OR user-locked)
        │
        ▼
POST /api/discovery/space  { seed_track_id, limit }
        │
        ▼
build_seed_external_queries(seed)        ← new helper in smart/external_discovery.rs
   • artist + genres + BPM-bucket queries
   • returns ~6–10 Tidal search query strings
        │
        ▼
provider.search(queries) → external candidates
        │
        ▼
filter: drop candidates whose tidal_id is in user's library
        │
        ▼
compute_external_scores_from_seed(seed_track_id, candidates)   ← new helper in services/learning.rs
   • load seed embedding (96D fusion vector)
   • cosine vs each candidate's metadata-derived proxy vector
   • blend with genre/artist match weights
        │
        ▼
top 60 → SpaceTrack { source: "external", is_in_library: false, ... }
        │
        ▼
return same DiscoverySpaceResponse shape Phase 1 produces
```

### Endpoint shape

`POST /api/discovery/space` is **extended**, not replaced. The existing prompt path stays as-is. The seed path is rewired:

| Input | Behavior |
|---|---|
| `prompt` set, `seed_track_id` empty | unchanged — text-based external/library discovery (current behavior) |
| `seed_track_id` set, `prompt` empty | **NEW: external seed-based discovery (this phase)** |
| Both empty | unchanged — most-played fallback |
| Both set | seed wins; prompt becomes a soft signal in the query builder |

### Edge model

External Tidal candidates have no rows in `track_neighbors`. Two options were considered:

- **Compute on-the-fly**: connect external candidates to each other by shared artist / genre / BPM proximity
- **Radial spokes only**: connect each external candidate to the seed by its similarity score; nothing between externals

**Decision: radial spokes.** It's clearer visually (the seed is the center; spokes show similarity), cheaper to compute, and avoids a quadratic similarity pass at request time. Phase 3 visual work can decide whether to add inter-candidate edges later.

The seed itself is included in the response as the first node (with `source: "library"`, `is_in_library: true`) so the canvas has a center to render around.

## Backend changes

### New helper: `build_seed_external_queries`

**File:** `noor-server/src/smart/external_discovery.rs` (new function adjacent to existing `build_search_queries` and `build_connection_queries`).

**Signature (sketch):**
```rust
pub fn build_seed_external_queries(
    seed: &SeedTrackContext,
    user_context: &ExternalDiscoveryContext,
    limit: usize,
) -> Vec<String>
```

`SeedTrackContext` is a small struct passed in by the route handler containing: seed's title, artist name, top genres (from `track_genres`), BPM bucket if available, and a list of similar artists from `track_neighbors` (top 5).

The helper returns 6–10 Tidal search query strings. Strategy: 2–3 artist-based queries, 2–3 genre-based queries, 1–2 "similar artist + genre" combinations, optionally a BPM range query if BPM is known. Reuses the existing query-construction patterns from `build_connection_queries`.

### New helper: `compute_external_scores_from_seed`

**File:** `noor-server/src/services/learning.rs` (new function adjacent to existing `compute_external_embedding_scores`).

**Signature (sketch):**
```rust
pub fn compute_external_scores_from_seed(
    db: &Database,
    seed_track_id: i64,
    candidates: &[ExternalCandidate],
) -> Result<HashMap<String, f64>>  // key = provider_track_id, value = score 0..1
```

Loads the seed's 96D fusion embedding from the active model. For each candidate, builds a proxy vector via the existing `external_candidate_proxy_vector` and computes cosine similarity. Returns an empty map (graceful degradation) if no embedding model exists yet — the route handler then falls back to genre/artist match scoring only.

### Route handler changes

**File:** `noor-server/src/server/routes.rs` — the `get_discovery_space` handler.

The existing seed-path branch (currently calls `discovery_learning::radio_from_neighbors` to fetch library neighbors) is replaced with a new external-discovery flow:

1. Load seed metadata (title, artist, genres, audio features) from the DB.
2. Build a `SeedTrackContext`.
3. Call `build_seed_external_queries`.
4. Run the queries against the configured Tidal provider.
5. Pass results through `existing_candidate_tidal_ids` to remove library tracks.
6. Score with `compute_external_scores_from_seed` (with genre/artist fallback if no embedding).
7. Map to `SpaceTrack` with `source: "external"`, `is_in_library: false`, fields populated from Tidal metadata where possible (title, artist_name, album_title, artwork_url, duration_ms, bpm/energy/key from Tidal's audio data if exposed).
8. Prepend the seed track itself (loaded via the existing library query path) so the canvas has a center.
9. Build edges: one edge per external candidate, source = seed_track_id, target = candidate's tidal_id, weight = candidate's score, `reason_tags` populated from the score components (e.g. `["artist_match", "genre_match"]`).

The Phase 1 enrichment passes (DSP join, listen-history aggregation, cohorts, top-genre source) keep running. They populate fields for the seed (which is in the library) and leave external nodes' fields null. That's correct — Phase 1 already declared all those fields optional on the frontend.

### Empty-state behavior

If no embedding model is trained, `compute_external_scores_from_seed` returns empty and we fall back to genre/artist match scoring (still useful results). If Tidal returns no candidates, we return an empty `tracks` array (the frontend handles this gracefully). If the seed track lookup fails, we return 400.

## Frontend changes

### `discover_space.ts` store

Add to the store state:

```typescript
interface DiscoverSpaceState {
    // existing
    mode: DiscoverViewMode;
    nodes: DiscoverTrackNode[];
    edges: DiscoverEdge[];
    loading: boolean;
    visitedRegions: Map<string, ...>;
    // new
    lockedSeedId: number | null;     // user explicitly locked this track as seed
    activeSeedId: number | null;     // resolved seed currently in use (locked or playing)
    activeSeedSource: 'locked' | 'playing' | null;
}
```

Two new exported helpers:

```typescript
export function lockSeed(trackId: number): void
export function unlockSeed(): void
```

`loadSpace` accepts an explicit `seedTrackId` like before, but the discover page now resolves which seed to use based on `lockedSeedId ?? $currentTrack?.id`.

### `discover/+page.svelte`

A `$effect` block resolves the seed:
- If `$discoverSpace.lockedSeedId` is set → use it
- Else if `$currentTrack?.id` is set → use it
- Else → no seed, show empty state

When the resolved seed changes, call `loadSpace(mode, resolvedSeedId)`.

Add a small lock-seed pill near the top of the discover layout (mirror the visual pattern of `GenrePanel.svelte`'s "Lock as seed" button). Shows current seed's title + artist + lock toggle. Click to lock the currently playing track, click again to unlock.

### Empty state

When no track is playing and no seed is locked, replace the current most-played fallback with a clear empty state: **"Play something to start discovering."** Phase 3 can polish this; for now it's just a message.

## What stays unchanged

- Canvas rendering (`DiscoverSpace.svelte`) — no visual changes in this phase
- Force simulation (`discoverBuilder.ts`) — nodes still arrange via mode-specific physics
- Phase 1 data fields — ride the wire, populated for the seed, null for external nodes
- The 5 view modes (Radio / Explore / Harmonic / Energy Arc / Samples) — they still influence layout
- The prompt path through `/api/discovery/space` (text search) — unchanged for now
- All existing endpoints (`/api/discovery/save`, `/api/discovery/play`, etc.)

## Out of scope (Phase 3)

- Hover tooltips, legend panel, edge color rework, cohort overlay
- Mode visual differentiation (5 modes look distinct)
- Training-freshness pill
- Skip-rate dimming
- Inter-candidate edges (radial spokes only for now)
- Embedding training of external tracks (Tidal doesn't expose embeddings)

## Verification

After implementation:

1. Backend type-check: `cd noor-server && cargo check` → 0 errors.
2. Frontend type-check: `cd frontend && npx svelte-check --tsconfig ./tsconfig.json` → 0 new errors.
3. Manual smoke:
   - Start backend + frontend.
   - Play a track in your library.
   - Open `/discover` — canvas should populate with ~60 external Tidal tracks similar to the playing track. Hover any node — it should NOT be in your library.
   - Pause playback, advance to a different track — discover space should re-fetch.
   - Click "lock seed" pill — the space stays put even if playback changes.
   - Click "unlock" — auto-seed resumes following playback.
   - With nothing playing and nothing locked, page should show empty state.
4. API smoke: `curl -X POST /api/discovery/space -d '{"seed_track_id": <id>}'` returns nodes with `source: "external"` and `is_in_library: false`, plus the seed itself as the first node with `is_in_library: true`.
