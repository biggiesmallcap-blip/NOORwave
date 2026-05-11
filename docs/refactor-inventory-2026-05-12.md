# Refactor Inventory - 2026-05-12

## Current Pass

Goal: reduce risk in the worst file without changing behavior or trampling active work.

Worst file by current line count:

1. `noor-server/src/server/routes.rs` - 16k+ lines
2. `noor-server/src/db/queries.rs` - 8k+ lines
3. `frontend/src/routes/library/+page.svelte` - 3.5k+ lines
4. `noor-server/src/services/radio.rs` - 3.2k+ lines
5. `frontend/src/routes/settings/+page.svelte` - 3.2k+ lines

## Decisions

- Worked in the main checkout because `routes.rs` already had active uncommitted changes. A new worktree would have hidden or duplicated that context.
- Kept the first refactor behavior-preserving: extract genre HTTP handlers from `routes.rs` into a child module.
- Left `noor-server/src/services/learning.rs` alone after the user confirmed it is actively being worked on.
- Did not touch protected server middleware, auth ordering, migrations, audio runtime, or other listed landmines.
- Ran a Windows sleep guard for up to 8 hours so the machine should avoid normal idle sleep while work continues.

## Completed Changes

### Genre Routes Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/genre_routes.rs`

Change:

- Added `mod genre_routes;`.
- Routed `/api/genres*` endpoints through `genre_routes::*`.
- Moved these handlers and request structs into the child module:
  - `get_genres`
  - `get_genre_snapshot`
  - `get_genre_heat`
  - `get_genre_co_occurrence`
  - `get_genre_cohorts`
  - `get_genre_evolution`
  - `get_genre_audio_metrics`
  - `get_genre_tracks`

Reasoning:

- Genre endpoints are a coherent route family with shared filter parsing and JSON response style.
- This extraction removes local duplication pressure from `routes.rs` without redesigning query behavior.
- The route table remains the only public registration point, so endpoint URLs stay stable.

## Verification

Completed:

- `cargo fmt --all`
- `rustfmt --check noor-server/src/server/routes.rs noor-server/src/server/routes/genre_routes.rs`
- Static scan of `genre_routes.rs` for banned attribution and new non-ASCII punctuation found no matches.

Blocked:

- `cargo test -p noor-server genre_snapshot_route_returns_galaxy_payload` was blocked by concurrent edits in `noor-server/src/services/learning.rs`.
- Build verification should be rerun after `learning.rs` stabilizes.

## Ownership Notes

Do not modify right now:

- `noor-server/src/services/learning.rs` - active external work.
- `noor-server/src/db/schema.rs` - migrations are ask-first.
- `noor-server/src/playback/runtime.rs` and `noor-server/src/playback/gapless.rs` - audio runtime and gapless are ask-first.
- `noor-server/src/server/mod.rs` - auth middleware ordering is ask-first.

## Next Refactor Candidates

### 1. `noor-server/src/db/queries.rs`

Risk: high. It is 8k+ lines and owns broad DB behavior.

Good first split:

- Extract genre query functions into `noor-server/src/db/queries/genre.rs`.
- Keep SQL unchanged.
- Re-export from `queries.rs` or add a `queries::genre::*` namespace in one mechanical pass.

Why later:

- DB query call sites are numerous.
- Needs a clean build window with no concurrent server work.

### 2. `frontend/src/routes/library/+page.svelte`

Risk: medium-high. Large Svelte route with likely mixed view state, filtering, selection, and rendering.

Good first split:

- Extract pure filtering and sorting helpers into `frontend/src/lib/library/`.
- Add small unit tests around helper behavior.
- Leave DOM structure untouched on the first pass.

Why later:

- Svelte route changes need frontend contract tests and a smoke pass.

### 3. `frontend/src/routes/settings/+page.svelte`

Risk: medium. It is large but settings cards have strong category rules.

Good first split:

- Extract source-related card helpers only if the active category layout stays unchanged.
- Do not introduce a new top-level category.

Why later:

- Settings organization has strict product rules in `AGENTS.md`.

### 4. `noor-server/src/services/radio.rs`

Risk: medium-high. Radio behavior is user-facing and can regress queue quality.

Good first split:

- Inventory duplicated scoring or fallback branches before editing.
- Add tests around selected duplicate logic first.

Why later:

- Needs behavioral tests, not just module extraction.

## Follow-Up Checklist

- Re-run `cargo test -p noor-server genre_snapshot_route_returns_galaxy_payload` after `learning.rs` stabilizes.
- Re-run `cargo test -p noor-server genre_` if the focused test passes.
- Run frontend contract tests for the genre snapshot route if frontend edits remain in the final diff.
- Consider a second extraction only after the current dirty work is stable enough to compile.
