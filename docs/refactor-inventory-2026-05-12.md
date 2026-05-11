# Refactor Inventory - 2026-05-12

## Scope

Workspace: `E:\NOORwave\.worktrees\codex-refactor-audit`

Branch: `codex-refactor-audit`

Baseline commit: `66dcd44 feat(discovery): resolve external sidecar tidal matches`

Goal: reduce duplicate logic in the largest server route surface without changing behavior.

## Current Hotspots

Largest files after the external sidecar commit:

1. `noor-server/src/server/routes.rs` - 16k+ lines
2. `noor-server/src/db/queries.rs` - 8k+ lines
3. `frontend/src/routes/library/+page.svelte` - 3.5k+ lines
4. `noor-server/src/services/radio.rs` - 3.2k+ lines
5. `frontend/src/routes/settings/+page.svelte` - 3.2k+ lines

## Decisions

- Used an isolated worktree after the other Codex started committing in the main checkout.
- Kept all new edits out of `E:\NOORwave`.
- Did not touch `noor-server/src/services/learning.rs` after it was called out as active work.
- Treated the committed genre route extraction as baseline because it landed in `66dcd44`.
- Picked the chart route area for the next cleanup because it had duplicated `Track` SQL row mapping in `routes.rs`.

## Changes In This Branch

### Chart Track Row Mapping

File: `noor-server/src/server/routes.rs`

Change:

- Added `chart_track_from_joined_row`.
- Replaced two duplicate `crate::db::models::Track` row construction blocks with that helper.
- Added `CHART_TRACK_SELECT_COLUMNS`.
- Replaced two duplicate chart `SELECT` column lists with the shared constant.

Reasoning:

- The two chart paths need identical `Track` column ordering.
- Before this change, adding or reordering a selected field required editing two SQL strings and two row mappers.
- A shared row mapper makes the column contract explicit and reduces drift risk.

Behavior:

- SQL predicates are unchanged.
- Returned `Track` fields are unchanged.
- Route URLs and JSON shapes are unchanged.

### Chart Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/chart_routes.rs`

Change:

- Added `mod chart_routes;`.
- Moved the chart route island out of `routes.rs`:
  - `get_charts`
  - `list_lastfm_genres`
  - `list_lastfm_countries`
  - chart DTOs
  - chart cache
  - Last.fm and Tidal chart fetch helpers
  - chart local-track lookup helpers
- Updated `/api/charts*` route registrations to call `chart_routes::*`.

Reasoning:

- Charts were a self-contained route family buried in the largest file.
- Keeping cache, DTOs, and chart fetch helpers together makes the route easier to reason about.
- The child module calls the existing parent Tidal token loader instead of duplicating token-loading behavior.

Behavior:

- Route URLs are unchanged.
- Query parameter parsing is unchanged.
- JSON response shape is unchanged.
- Last.fm/Tidal fetch behavior is unchanged.

### Sportify Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/sportify_routes.rs`

Change:

- Added `mod sportify_routes;`.
- Moved the Sportify discovery and Spotify-playlist save handlers out of `routes.rs`:
  - `sportify_discovery_search`
  - `sportify_discovery_track`
  - `sportify_discovery_album`
  - `sportify_discovery_playlist`
  - `sportify_discovery_artist`
  - `sportify_discovery_artist_top_tracks`
  - `sportify_discovery_artist_related`
  - `sportify_discovery_album_related`
  - `sportify_discovery_track_related`
  - `save_spotify_playlist`
- Moved their request DTOs and local parser helper with them.
- Kept shared parent helpers in `routes.rs` and called them via `super::`.

Reasoning:

- Sportify routes are a coherent upstream proxy/cache surface.
- The block is independent enough to split without touching auth, playback runtime, migrations, or queue internals.
- Keeping playlist save with the Sportify module preserves the route family's existing adjacency and dependency on Sportify cache/resolution state.

Behavior:

- Route URLs are unchanged.
- Query/body parsing is unchanged.
- Cache-first behavior is unchanged.
- Playlist import behavior is unchanged.

### Analytics Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/analytics_routes.rs`

Change:

- Added `mod analytics_routes;`.
- Moved the analytics route island out of `routes.rs`:
  - `get_analytics_overview`
  - `get_analytics_dashboard`
  - `get_analytics_signals`
  - `get_recent_listens`
  - analytics query DTOs
- Updated `/api/analytics/*` route registrations to call `analytics_routes::*`.

Reasoning:

- The analytics handlers are a compact route family with shared query bounds and DB query dependencies.
- Keeping them together removes another independent block from the largest file without touching playback, auth, migrations, or Tidal state.
- The dashboard clamp behavior stayed intact, with the comment rewritten to explain why the wide range exists.

Behavior:

- Route URLs are unchanged.
- Query parameter parsing and clamp ranges are unchanged.
- JSON response shape is unchanged.
- Analytics DB query calls are unchanged.

### Artist Spread Shuffle

File: `noor-server/src/playback/shuffle.rs`

Change:

- Added `artist_bucket_key` and used it for both bucketing and final stabilization.
- Added `distribute_artist_buckets`.
- Added `pick_largest_eligible_bucket_index`.
- Changed artist spread to drain the largest eligible artist bucket, with random tie-breaking, before running the existing adjacency stabilizer.

Reasoning:

- `cargo test -p noor-server` exposed a deterministic failure in `artist_spread_avoids_consecutive_repeats_when_possible`.
- The test passed alone but failed in the full suite because `HashMap` bucket order is randomized outside the seeded RNG.
- The old artist spread could drain smaller artist buckets first and leave a same-artist tail such as `B C A A`.
- The genre shuffle path already has a weighted-random bucket picker where variety matters; artist spread needs the stronger invariant that repeats are avoided when possible.

Behavior:

- Artist spread still shuffles tracks within each artist bucket.
- Ties between equally sized eligible artist buckets remain random.
- The dominant artist is no longer allowed to collect at the tail while other artists are still available.

## Verification

Completed:

- `cargo fmt --all`
- `cargo test -p noor-server genre_snapshot_route_returns_galaxy_payload`
- `cargo test -p noor-server chart`
- `cargo test -p noor-server playback::shuffle::tests::artist_spread_avoids_consecutive_repeats_when_possible`
- `cargo test -p noor-server`
- `cargo test -p noor-server sportify`
- `cargo test -p noor-server analytics`
- `cargo fmt --all -- --check`
- `git diff --check`
- Attribution scan across changed files
- Banned punctuation scan across changed files

Observed:

- The focused genre snapshot route test passed.
- The chart-filtered Rust tests passed.
- The targeted shuffle regression passed.
- The full `noor-server` package passed: 567 passed, 0 failed, 1 ignored.
- After the chart route extraction, the chart-filtered tests and full `noor-server` package passed again.
- After the Sportify route extraction, the Sportify-filtered tests and full `noor-server` package passed again.
- After the analytics route extraction, the analytics-filtered Rust tests passed: 2 passed, 0 failed.
- After the final audit cleanup, the full `noor-server` package passed again: 567 passed, 0 failed, 1 ignored.
- Formatter and diff whitespace checks passed.
- No AI attribution strings were found in changed files.
- No em dash, right arrow, or less-equal glyphs were found in changed files.

## Deferred Refactors

### Split DB Genre Queries

Candidate:

- Move genre query functions from `noor-server/src/db/queries.rs` into a dedicated query module.

Reason to defer:

- `queries.rs` has broad call-site reach and needs a dedicated pass.

### Split Library Page State

Candidate:

- Extract pure filtering and sorting helpers from `frontend/src/routes/library/+page.svelte`.

Reason to defer:

- Needs frontend unit or contract coverage and should avoid the remaining dirty frontend work in the main checkout.

## Safety Notes

- Do not merge this branch until the main checkout's remaining frontend edits are reviewed.
- Before merge, rerun `cargo test -p noor-server` if time permits.
- Next route extraction candidates should avoid the ask-first files in `AGENTS.md` and stay out of active `learning.rs` work.
