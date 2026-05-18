# Hotspot Reduction Roadmap Design

## Goal

Reduce NOORwave hotspot density by lowering future churn concentration in the largest and most frequently edited files, without changing product behavior or crossing high-risk boundaries without a dedicated plan.

## Context

The hotspot investigation used the current `.repowise` cache and local git churn. RepoWise MCP transport closed during live calls, but the cached database was current at commit `2cf84f3`.

Current code-path hotspot drivers:

| File | 90d commits | 90d churn | Hotspot score |
| --- | ---: | ---: | ---: |
| `noor-server/src/server/routes.rs` | 186 | 41662 | 177.1 |
| `noor-server/src/db/queries.rs` | 68 | 11942 | 63.7 |
| `frontend/src/routes/settings/+page.svelte` | 61 | 6261 | 49.5 |
| `frontend/src/routes/library/+page.svelte` | 51 | 6583 | 39.7 |
| `frontend/src/routes/+layout.svelte` | 74 | 8072 | 38.1 |

The score problem is mostly concentration. The top five code hotspots account for about 26.6 percent of hotspot score and 35.0 percent of code churn. Dead-code deletion is not the main lever: the cached RepoWise dead-code findings were low confidence and not marked safe to delete.

## Design Principles

- Preserve external behavior: route URLs, JSON shapes, playback behavior, queue behavior, TIDAL behavior, and settings contracts stay unchanged unless a later implementation plan explicitly says otherwise.
- Follow existing extraction patterns instead of introducing a new architecture style.
- Keep public import paths stable where possible, especially `crate::db::queries::*`.
- Split by cohesive product surface, not by arbitrary line counts.
- Add or preserve focused verification for each extracted surface.
- Do not touch guarded areas from `AGENTS.md` without a dedicated approval step.

## Approaches Considered

### Backend-only first

Focus only on `routes.rs` and `queries.rs`.

Pros:

- Highest measured hotspot payoff.
- Lowest visual regression risk.
- Matches existing backend route-module extraction work.

Cons:

- Leaves large frontend hotspots unplanned.
- Does not address measurement noise from lockfiles and generated files.

### Full roadmap, backend first

Create one roadmap covering backend, frontend, and measurement cleanup, then execute one lane at a time starting with `routes.rs`.

Pros:

- Keeps all hotspot work coordinated.
- Lets each lane have its own testable implementation plan.
- Avoids mixing backend and frontend risk in one PR.
- Starts with the highest-payoff and most established pattern.

Cons:

- Requires an extra planning step before implementation.

### Metric cleanup first

Exclude lockfiles, generated files, vendor files, and other measurement noise before code changes.

Pros:

- May improve the score quickly if the scoring source supports exclusions.
- No product behavior risk.

Cons:

- Does not reduce actual code churn concentration.
- Depends on how the hotspot score is produced.

## Recommendation

Use the full roadmap with backend-first execution.

The roadmap should track all five improvement lanes, but the first detailed implementation plan should target `noor-server/src/server/routes.rs`. That file dominates the hotspot score and already has an established route extraction pattern under `noor-server/src/server/routes/`.

## Roadmap Lanes

### Lane 1: Continue splitting `routes.rs`

Purpose:

- Reduce the dominant hotspot by moving coherent route islands out of the parent route file.

Preferred next candidates:

- Library browsing routes: tracks, albums, artists, album tracks, artist tracks, artist counts.
- Playlist and smart-playlist routes: playlist list/detail/favorite/add, smart playlist create/update/delete/evaluate.
- Non-playback TIDAL catalog/search routes: TIDAL search, artist profile, album tracks, import helpers only if they do not cross playback runtime boundaries.

Boundaries:

- Avoid playback runtime, queue promotion, pending resolution, audio output, and stream-start extraction in the first pass.
- Keep shared fragile helpers in the parent until a child module can use them through a narrow `pub(super)` interface.
- Route registrations stay in `api_routes`.

Expected shape:

- Add modules under `noor-server/src/server/routes/`.
- Move handlers, request DTOs, and local helpers with each route family.
- Keep shared parent helpers visible with `pub(super)` only when needed.
- Preserve route URLs and response payloads.

Verification:

- `cargo check -p noor-server`
- Targeted route-related tests when a route family has coverage.
- `cargo test -p noor-server --lib` after shared route movement.

### Lane 2: Split `queries.rs` behind a stable facade

Purpose:

- Reduce concentration in the main DB query file while preserving existing call sites.

Preferred candidate modules:

- `noor-server/src/db/queries/genre.rs`
- `noor-server/src/db/queries/analytics.rs`
- `noor-server/src/db/queries/audio_search.rs`
- `noor-server/src/db/queries/discovery.rs`

Boundary design:

- Convert `noor-server/src/db/queries.rs` into `noor-server/src/db/queries/mod.rs` only in a dedicated plan.
- Re-export existing functions and types so callers can keep using `crate::db::queries::*`.
- Move one cohesive query family per task.
- Do not change SQL behavior, row projections, migration history, or DTO field names.

Verification:

- `cargo check -p noor-server`
- Targeted tests for moved query families.
- `cargo test -p noor-server --lib` after facade movement.

### Lane 3: Extract queue UI from `+layout.svelte`

Purpose:

- Reduce the main shell hotspot by moving queue rendering and queue interactions into a focused shell component.

Target shape:

- Create `frontend/src/lib/shell/QueuePanel.svelte`.
- Keep `frontend/src/routes/+layout.svelte` responsible for app shell composition, auth/onboarding, player state, and global listeners.
- Move queue list rendering, save queue modal, queue expansion controls, queue row actions, drag-to-reorder handlers, and queue reason hover plumbing when practical.

Boundaries:

- Preserve context-menu behavior and shared menu builders.
- Preserve TIDAL artwork fallback rules.
- Avoid changing player store semantics.
- Keep browser and Tauri execution guards intact.

Verification:

- `pnpm check`
- `pnpm test`
- `pnpm run build` before calling frontend CSS-safe.
- Browser smoke check of desktop and mobile queue surfaces when implementation begins.

### Lane 4: Extract pure library page helpers

Purpose:

- Reduce `frontend/src/routes/library/+page.svelte` complexity by moving pure logic out of the route component.

Preferred helpers:

- Track, album, and artist filtering.
- Sort comparators.
- Selection range calculation.
- Audio-search result adaptation.
- Decade bucket derivation.

Target shape:

- Create focused helper modules under `frontend/src/lib/library/`.
- Add Vitest coverage for pure helpers before replacing inline logic.
- Keep Svelte state and DOM event wiring in the page until helper boundaries are proven.

Verification:

- Focused Vitest helper tests.
- `pnpm check`
- `pnpm test`
- `pnpm run build` before CSS cleanup claims.

### Lane 5: Exclude hotspot measurement noise

Purpose:

- Prevent non-maintainability files from distorting hotspot density.

Candidate exclusions:

- `Cargo.lock`
- `frontend/pnpm-lock.yaml`
- `frontend/package-lock.json` if still tracked
- `noor-app/gen/schemas/*`
- `noor-server/vendor/**`
- generated build outputs and local caches

Target shape:

- Prefer configuration in the scoring tool if it exists.
- If no config exists, document the score interpretation and report code-path density separately from all-file density.
- Do not delete lockfiles or generated schema files as part of metric cleanup.

Verification:

- Re-run the scoring query or local hotspot script.
- Confirm code-path hotspot density is reported separately from all-file density.

## Implementation Order

1. Write a detailed implementation plan for Lane 1: `routes.rs` extraction.
2. Execute Lane 1 in small route-family PR-sized slices.
3. Recompute hotspot score and churn concentration.
4. Write a dedicated implementation plan for Lane 2 only after Lane 1 settles.
5. Plan and execute frontend lanes after backend route churn is lower.
6. Apply measurement cleanup whenever scoring configuration is known.

## Risks And Mitigations

Route contract drift:

- Keep route registrations unchanged.
- Move DTOs with handlers.
- Run backend checks after each route-family extraction.

DB query drift:

- Keep `crate::db::queries::*` stable.
- Move one query family at a time.
- Avoid SQL rewrites during file moves.

Frontend regression:

- Extract pure helpers before stateful components where possible.
- Run Svelte checks and browser smoke checks after shell changes.

Measurement-only wins:

- Report score exclusions separately from real code changes.
- Do not treat lockfile exclusion as equivalent to reducing hotspot concentration in source files.

## Non-goals

- No playback runtime refactor in the first lane.
- No database migration changes.
- No route URL, JSON contract, or WebSocket event changes.
- No broad formatting-only changes.
- No dead-code deletion from RepoWise findings without local reference validation.
- No lockfile, generated schema, or vendor deletion for score reasons.

## First Implementation Plan Scope

The first implementation plan should cover Lane 1 only. It should choose one low-risk route family, define exact files to create or modify, include focused verification commands, and preserve current behavior. Library and playlist routes are the preferred first candidates because they are product-cohesive and avoid playback runtime internals.
