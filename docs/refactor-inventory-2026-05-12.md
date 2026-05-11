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

### Search Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/search_routes.rs`

Change:

- Added `mod search_routes;`.
- Moved the search route island out of `routes.rs`:
  - `search`
  - `search_audio`
  - `search_vibe`
  - `search_underrated`
  - Spotify playlist compact search helper
  - search query/body DTOs

Reasoning:

- Search routes are independent from playback, auth, and migrations.
- Keeping the Sportify playlist helper with `/api/search` keeps the cross-service search behavior local to that route family.

Behavior:

- Route URLs are unchanged.
- Query/body parsing is unchanged.
- JSON response shape is unchanged.

### Duplicate Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/duplicates_routes.rs`

Change:

- Added `mod duplicates_routes;`.
- Moved duplicate scan/list/resolve/dismiss handlers out of `routes.rs`.
- Kept the TIDAL unfavorite retry path by calling the shared parent session refresh helper.

Reasoning:

- Duplicate detection is a self-contained library maintenance surface.
- The route module keeps duplicate DB calls and TIDAL cleanup adjacency together without changing duplicate internals.

Behavior:

- Route URLs are unchanged.
- Pagination behavior is unchanged.
- Queue/playback/library event emission is unchanged.
- TIDAL unfavorite behavior is unchanged.

### Enrichment Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/enrichment_routes.rs`

Change:

- Added `mod enrichment_routes;`.
- Moved MusicBrainz, Spotify, and Last.fm config/enrichment handlers out of `routes.rs`.
- Moved Last.fm server-side auth handlers with the Last.fm enrichment surface.
- Left audio-analysis routes in `routes.rs` for a separate pass.

Reasoning:

- MusicBrainz, Spotify, and Last.fm enrichment share the same settings/maintenance route class.
- Keeping audio analysis out of this module avoids mixing metadata enrichment with DSP analysis.

Behavior:

- Route URLs are unchanged.
- Background task spawning, progress events, cancellation flags, and reset behavior are unchanged.

### Tidal Home Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/tidal_home_routes.rs`

Change:

- Added `mod tidal_home_routes;`.
- Moved TIDAL home shelf handlers out of `routes.rs`:
  - `get_tidal_mixes`
  - `get_tidal_mix_tracks`
  - `get_tidal_radio_stations`
  - `get_tidal_home_modules`
  - `get_tidal_discover_module_items`
- Reused the parent token persistence, session refresh, auth-error detection, and playable JSON helpers.
- Kept `play_tidal_mix` in the parent because it is part of the playback path, not just home data loading.

Reasoning:

- The TIDAL home shelf handlers are a compact read/proxy surface.
- Reusing parent helpers avoids duplicating session recovery logic.
- Leaving playback-start logic in the parent keeps this extraction lower risk.

Behavior:

- Route URLs are unchanged.
- TIDAL token fallback, six-hour cache behavior, session refresh retry, and JSON response shape are unchanged.

### Audio Analysis Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/audio_analysis_routes.rs`

Change:

- Added `mod audio_analysis_routes;`.
- Moved the DSP analysis route handlers out of `routes.rs`:
  - `start_audio_analysis`
  - `stop_audio_analysis`
  - `get_audio_analysis_status`
  - `get_passive_dsp`
  - `set_passive_dsp`
  - `get_track_audio_features`
  - `get_audio_features_stats`
  - `get_library_analytics`
  - `reset_audio_analysis`
  - `get_audio_features_quality`
  - `reanalyze_stale_tracks`
- Moved the audio analysis request DTOs with the handlers.

Reasoning:

- Audio analysis is a self-contained library maintenance surface.
- The module only depends on shared state, DB queries, and the existing audio analysis scanner.
- Keeping this separate from metadata enrichment avoids mixing DSP maintenance with source enrichment.

Behavior:

- Route URLs are unchanged.
- Cancel flag, running flag, and scanner spawn behavior are unchanged.
- DSP query and reset behavior is unchanged.

### TIDAL Sync Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/tidal_sync_routes.rs`

Change:

- Added `mod tidal_sync_routes;`.
- Moved TIDAL sync handlers and sync worker helpers out of `routes.rs`:
  - `get_sync_info`
  - `set_auto_sync`
  - `trigger_auto_sync`
  - `tidal_sync_library`
  - `tidal_sync_cancel`
  - TIDAL sync guard, progress, import, and favorite-flag helpers
- Re-exported `trigger_auto_sync` from the parent route module for startup auto-sync.
- Kept shared token persistence, session recovery, auth-error detection, and TIDAL import row helpers in the parent.

Reasoning:

- TIDAL sync has enough state and guard logic to deserve its own route module.
- Startup auto-sync already depends on the route-level trigger, so the public parent re-export keeps that contract stable.
- Reusing the parent auth/session helpers avoids duplicating fragile TIDAL recovery behavior.

Behavior:

- Route URLs are unchanged.
- Auto-sync startup entry point is unchanged.
- Sync cancellation, progress events, favorite preservation, and session refresh behavior are unchanged.

### Discovery Route Extraction

Files:

- `noor-server/src/server/routes.rs`
- `noor-server/src/server/routes/discovery_routes.rs`

Change:

- Added `mod discovery_routes;`.
- Moved discovery search, connections, presets, training controls, training safety, and feedback handlers out of `routes.rs`.
- Moved their request DTOs with the handlers.
- Left `/api/discovery/play` in `routes.rs`.
- Promoted the shared discovery helper functions to `pub(super)` so the child module can reuse the existing implementation.

Reasoning:

- Discovery search and training are a coherent route family with shared prompt, provider, and training dependencies.
- `/api/discovery/play` touches playback runtime and stream startup, which is a higher-risk surface and should stay parent-side until a dedicated playback pass.
- Keeping shared provider/context helpers in the parent avoids moving helpers still used by radio, sound-space, and playback-adjacent discovery code.

Behavior:

- Route URLs are unchanged.
- Prompt validation, provider selection, Last.fm augmentation, metadata enrichment, and embedding-score blending are unchanged.
- Training start/stop, safety estimate, engine selection, and feedback persistence behavior are unchanged.
- Discovery inline playback behavior is unchanged because it was not moved.

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
- `cargo test -p noor-server` after extracting search, duplicate, enrichment, and TIDAL home route modules

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
- After the second route extraction batch, the full `noor-server` package passed: 567 passed, 0 failed, 1 ignored.
- Added-line and new-module scans found no AI attribution strings, em dash, right arrow, or less-equal glyphs.
- After extracting audio analysis, TIDAL sync, and discovery route modules, the full `noor-server` package passed again: 567 passed, 0 failed, 1 ignored.

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
