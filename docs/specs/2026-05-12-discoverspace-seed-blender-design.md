# DiscoverSpace Seed Blender Design

## Summary

DiscoverSpace should become a functional discovery instrument. The canvas keeps the current drifting, settling, soft motion, but every visual element must map to real discovery work: seed anchors, candidate scoring, external discovery, library guidance, blend ratios, and playable radio routes.

The main outcome is finding new non-library songs. Library tracks can guide and explain a blend, but new playable external candidates are the primary result.

## Goals

- Keep the original DiscoverSpace movement feel: calm drift, force relaxation, soft node breathing, and animated route overlays.
- Make spatial placement meaningful. Distance, bearing, clustering, and route position should be derived from real scoring and relationship data.
- Support multi-seed blends, not just two-seed A/B blends.
- Rank non-library candidates first.
- Use library tracks as scoring signal, bridge context, and optional route supports.
- Create blend radios from the map with real playable queue output.
- Avoid standard control clutter. The page should feel like NOORwave, not a generic graph editor or playlist builder.

## Non-Goals

- Do not build decorative network visuals with no behavioral purpose.
- Do not make library songs the main blend output.
- Do not replace current DiscoverSpace motion with a static data visualization.
- Do not add a large generic toolbar.
- Do not implement more than four pinned blend seeds in the first version.
- Do not require database migrations for the first version unless implementation proves there is no safe append-only alternative.

## Existing System Context

Relevant current code:

- `frontend/src/lib/components/DiscoverSpace/DiscoverSpace.svelte` owns the canvas loop, camera, pointer interaction, selection, warp search, and physics lifecycle.
- `frontend/src/lib/components/DiscoverSpace/discover_space_physics.ts` owns force relaxation and hit testing.
- `frontend/src/lib/components/DiscoverSpace/discover_space_renderer.ts` owns the visual language: background, rings, regions, edges, nodes, labels, route overlay, seed, playing node, and warp streaks.
- `frontend/src/lib/components/DiscoverSpace/discover_space_store.ts` owns map state, radio route state, loading, refresh progress, active seed, and lens.
- `frontend/src/lib/components/DiscoverSpace/discover_space_adapter.ts` maps API responses into typed nodes and already accepts backend `layout` hints.
- `noor-server/src/server/routes.rs` owns `/api/discovery/space`, `/api/radio/song`, and `/api/radio/start`.
- `noor-server/src/services/discovery_space.rs` owns score normalization, reason normalization, graph pruning, and in-degree stats for the current space endpoint.
- `noor-server/src/services/radio.rs` owns radio orchestration, blend interleaving, source selection, deduping, taste signals, genre signals, and playable queue output.
- `noor-server/src/services/radio_config.rs` owns radio blend profiles and per-source weights.
- `noor-server/src/services/learning.rs` owns external provider refresh and external candidate resolution paths.

## Product Model

### Blend Seeds

A blend is a weighted set of pinned seed tracks:

```ts
type BlendSeed = {
	trackId: number;
	weight: number;
	role: 'anchor';
};
```

The first version supports two to four seeds. Weights always normalize to `1.0`.

Default weights:

- One seed: regular DiscoverSpace, no blend.
- Two seeds: `0.5 / 0.5`.
- Three seeds: `0.34 / 0.33 / 0.33`.
- Four seeds: `0.25 / 0.25 / 0.25 / 0.25`.

Users can bias a blend. For two seeds this feels like radio blend math:

- `50/50`: candidates sit near the midpoint between both seeds.
- `60/40`: candidates retain more of seed A while still pulling toward seed B.
- `40/60`: same idea leaning toward seed B.

For three or four seeds, the UI should expose simple weight chips or a compact balance control, not a complex mixer.

### External-First Output

The blend exists to find new music. A candidate that is not already in the library should receive a strong ranking bonus. A library candidate should be shown only when it serves one of these purposes:

- It explains why two or more seeds connect.
- It provides a high-confidence bridge inside the route.
- It helps the user understand the blend field.
- It is needed to keep playback coherent when external candidates are sparse.

The UI should distinguish:

- Seed anchors: pinned blend inputs.
- External candidates: primary discoveries, visually brighter.
- Library guides: contextual bridge nodes, visually quieter.
- Route nodes: the selected playable blend path.

## Candidate Ranking

### Inputs

The first implementation should reuse existing sources before inventing new data:

- `track_neighbors` for library-neighbor proximity per seed.
- external candidate tables populated by Last.fm and TIDAL refresh logic.
- current radio orchestration source logic from `services/radio.rs`.
- reason tags, support count, confidence, genre, BPM, energy, and Camelot key when available.
- existing skip, hide, and feedback signals where radio already applies them.

### Score Shape

For each candidate and each seed, compute a seed-side proximity score. Use existing normalized similarity where possible. If a candidate appears in multiple seed candidate sets, keep each per-seed score.

Weighted proximity:

```text
weighted_seed_proximity = sum(seed_weight * proximity(candidate, seed))
```

Coverage bonus:

```text
coverage_bonus = f(number_of_seeds_with_usable_connection, total_seed_count)
```

External-first bonus:

```text
external_bonus = candidate.is_in_library ? 0.0 : external_boost
```

Library penalty:

```text
library_penalty = candidate.is_in_library ? library_output_penalty : 0.0
```

Final score:

```text
blend_score =
	weighted_seed_proximity
	+ coverage_bonus
	+ external_bonus
	+ confidence_bonus
	+ diversity_bonus
	- library_penalty
```

The penalty must not hide all library nodes. It should only push them below comparable external candidates in the output ranking.

### Multi-Seed Coverage

Coverage should reward candidates that connect to more than one seed. This keeps blend results from becoming a simple union of separate radios.

Example:

- Candidate A: strong match to seed 1, no match to seeds 2 and 3.
- Candidate B: medium match to all three seeds.

In balanced mode, Candidate B should usually win because it expresses the blend better.

In biased mode, Candidate A can win if seed 1 has enough weight.

### Diversity

The blend route should not become one artist, one album, or one micro-genre unless the seeds explicitly imply that. Reuse the radio rerank ideas from `services/radio.rs`:

- dedupe by normalized artist/title identity.
- limit same-artist saturation.
- avoid recently skipped or hidden tracks.
- allow controlled genre drift.
- prefer playable resolved external candidates.

## Spatial Layout

### Semantic Map Foundation

Layout hints should become more meaningful. The backend should provide stable initial positions for blend responses, and the frontend physics should gently relax them.

For regular one-seed DiscoverSpace:

- Distance from seed: relevance and confidence.
- Bearing: primary reason family.
- Local grouping: genre or cluster key.
- Radius and alpha: score, confidence, source, and cold-start state.

For multi-seed blends:

- Seed anchors form the major gravity points.
- Candidate position is the weighted centroid of its per-seed influence.
- Candidate distance from the weighted center reflects blend score and confidence.
- Candidates connected to more seeds settle closer to the blend center.
- External candidates should be visible in the corridor or field.
- Library guides should be dimmer and slightly behind the external layer.

Visual shape by seed count:

- Two seeds: corridor between anchors.
- Three seeds: triangular field.
- Four seeds: soft hull or constellation field.

This is functional, not decorative. The shape tells the user what blend math is doing.

### Motion

Do not remove the current motion language. Improve it by changing the initial forces and layout hints, not by freezing the graph.

Expected behavior:

- Seeds feel pinned and heavy.
- External candidates drift into the blend field.
- Library guides are present but quieter.
- The selected route pulses softly.
- The map settles once kinetic energy drops below the existing threshold.
- Reduced motion still disables velocity updates as it does now.

## Visual Behavior

### Calm Depth

Use the calm parts of the visual references:

- soft network graph structure.
- visible relationship corridors or fields.
- restrained labels.
- depth fade for low-confidence or guide nodes.
- edges that reveal on focus, route, or strong confidence.

Avoid:

- busy particle fields.
- constant motion unrelated to scoring.
- large animated controls.
- dense graph labels.
- decorative network lines that do not reflect real edges or scoring.

### Lenses

Existing lenses should still work. Blend mode can add one blend-specific lens later, but version one should avoid expanding the lens list unless it has a clear job.

Useful blend lens candidate:

- `blend`: color by strongest contributing seed or mixed contribution.

This lens should be deferred unless the first implementation needs it to explain the interaction.

## User Flow

### Default Flow

1. User opens DiscoverSpace with one active seed.
2. User selects a candidate or another visible track.
3. Side panel offers a focused action: `Add to blend`.
4. Once two seeds are pinned, the blend field appears.
5. User adjusts a simple ratio if desired.
6. Map loads or reranks external-first blend candidates.
7. User can preview candidates, save discoveries, or make a blend radio.

### Blend Radio Flow

1. User pins two to four blend seeds.
2. User presses `Make blend radio`.
3. Backend creates a playable route from ranked candidates.
4. External candidates are preferred.
5. Library tracks are inserted only when they help coherence or playback availability.
6. Frontend draws the route with existing route overlay logic.
7. Playback starts through the existing queue or radio start path.

### Empty and Sparse States

If no external candidates are playable:

- show the best unresolved external candidates as dim pending nodes.
- show library guide nodes as context.
- explain that the blend needs external refresh or resolution.
- offer `Refresh discoveries` if the existing refresh path can support it.

If only one seed is pinned:

- behave like current DiscoverSpace.
- make `Add to blend` visible but not intrusive.

## Backend Design

### New Blend Request Shape

Add a request shape for blend space and blend radio. Exact route names can be finalized during implementation, but the contract should look like this:

```rust
struct DiscoveryBlendSeed {
    track_id: i64,
    weight: f64,
}

struct DiscoveryBlendRequest {
    seeds: Vec<DiscoveryBlendSeed>,
    limit: Option<i64>,
    external_first: Option<bool>,
    include_library_guides: Option<bool>,
}
```

Validation:

- reject zero seeds.
- reject more than four seeds.
- reject non-positive track IDs.
- reject duplicate seed IDs.
- normalize weights if they sum to a positive value.
- default to equal weights when weights are missing or invalid.
- keep `external_first` true by default.
- keep `include_library_guides` true by default.

### Candidate Assembly

Candidate assembly should reuse current logic:

- for each seed, collect library neighbors.
- for each seed, collect Last.fm/TIDAL/external candidates where available.
- dedupe across seeds by stable identity.
- attach per-seed proximity scores.
- attach source, confidence, reasons, support count, genre, audio features, and playability.
- compute blend score and layout hints.
- prune the result with graph limits similar to `/api/discovery/space`.

### Radio Output

Blend radio should reuse `RadioQueue` style output where practical. It should be compatible with existing frontend playback paths.

The route builder should:

- rank candidates by blend score.
- prefer playable external candidates.
- avoid too many library guide tracks.
- order the route by changing seed influence.
- preserve reason strings for radio explanations.

Ordering examples:

- Two seeds at `50/50`: start near the active seed, move through midpoint candidates, end near the second seed.
- Two seeds at `60/40`: stay longer near seed A and cross later.
- Three seeds: route can follow the strongest seed sequence based on weights and candidate coverage.

## Frontend Design

### State

Extend DiscoverSpace state with blend-specific fields:

```ts
interface BlendSeed {
	trackId: number;
	weight: number;
}

interface BlendState {
	seeds: BlendSeed[];
	active: boolean;
	loading: boolean;
	error: string | null;
}
```

Keep the state colocated under `frontend/src/lib/components/DiscoverSpace/` unless the implementation needs shared app-wide access.

### UI

Controls should be sparse:

- `Add to blend` in the side panel.
- compact blend strip showing pinned seeds.
- ratio control only when two seeds are pinned.
- `Make blend radio` as the main action.
- `Clear blend` as a secondary action.

Do not add a full graph-editing toolbar.

### Canvas

Canvas should continue to use:

- current RAF loop.
- current physics settling.
- current hit testing.
- current route overlay.
- current node rendering patterns.

Add blend-aware rendering:

- seed anchors with distinct but restrained rings.
- blend corridor, triangle field, or hull behind candidates.
- external candidates brighter than library guide nodes.
- library guide nodes dimmer, but still hoverable if they are real track references.
- route overlay for blend radio.

Every visible track reference still needs right-click context menu support where applicable. Canvas nodes currently use pointer interaction rather than DOM rows, so any new side panel or list references must use shared context menu builders.

## Testing

### Backend Tests

Add unit tests for:

- seed validation.
- weight normalization.
- duplicate seed rejection.
- external-first ranking.
- coverage bonus behavior.
- library penalty behavior.
- route ordering for `50/50` and `60/40`.
- fallback when external candidates are sparse.

Prefer pure helper tests in a focused module before wiring route tests through Axum.

### Frontend Tests

Add Vitest tests for:

- blend seed state add/remove/normalize behavior.
- adapter mapping of blend layout hints.
- deterministic layout math for two, three, and four seeds.
- external-first visual priority flags.
- route state updates.

Add contract tests only where they protect a real behavior. Avoid screenshot-only tests unless a rendering regression is likely.

### Manual Verification

Manual verification should cover:

- one-seed DiscoverSpace still behaves like current map.
- two-seed blend shows corridor and external candidates.
- ratio changes rerank or reposition candidates.
- three-seed blend shows a field, not a broken corridor.
- `Make blend radio` starts playback or returns a coherent queue.
- reduced motion still works.
- Ctrl plus wheel UI zoom remains untouched.

## Implementation Sequence

1. Backend pure scoring helpers for weighted seeds and candidate blend scores.
2. Backend candidate assembly and response contract.
3. Backend blend radio route endpoint.
4. Frontend state and side-panel actions.
5. Frontend adapter and layout hint support.
6. Frontend canvas blend field rendering.
7. Frontend `Make blend radio` flow.
8. Focused verification and cleanup.

## Open Decisions

- Exact endpoint names.
- Whether blend space and blend radio share one backend assembly helper or split read and queue paths.
- Whether the first UI exposes three and four seed weights directly or starts with equal weights.
- Whether unresolved external candidates can appear in blend radio preview before they are playable.

## Approval Criteria

The feature is successful when:

- The user can pin two to four seeds.
- The map uses the original DiscoverSpace movement feel.
- Blend candidates are mostly non-library songs.
- Library songs help explain and stabilize the blend without becoming the main output.
- The visual field is simple enough to understand.
- The user can start a radio from the blend.
- The route and queue are produced from real scoring logic.
