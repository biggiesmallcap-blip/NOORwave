# Repowise Dead-Code Audit - 2026-06-01

## Scope

This began as the audit-only dead-code pass for the Repowise signal upgrade
course. Later cleanup slices removed only locally validated dead code.

## Inputs

- Candidate command: `repowise dead-code --safe-only --format json`
- Local validation command shape: `rg -n <symbol-or-path> frontend noor-server docs scripts`
- Guardrail doc: `docs/dev/repowise-dead-code.md`

The Repowise output is a candidate queue only. This run showed several clear
false positives where Repowise did not resolve SvelteKit `$lib` imports,
Svelte action usage, Rust `#[cfg(test)]` modules, or public route merges.

## Classification Summary

| Classification | Findings | Decision |
| --- | --- | --- |
| `removed` | Locally validated dead code listed below. | Removed only after exact reference checks, boundary review, and targeted tests. |
| `keep` | Confirmed false positives and intentional placeholders listed below. | Do not remove from this Repowise output. |
| `manual review` | Remaining safe-only findings not locally proven as false positives. | Review in smaller follow-up slices with exact `rg` checks and product-path validation. |

## Removed

| Finding | Evidence |
| --- | --- |
| `frontend/src/lib/cache/api_queries.ts::{peekCachedValue, scheduleTargetedRefetch}` | Exact `rg` after removal finds only historical review docs. `stableCacheKey` remains in use by `query.ts`, `ws_events.ts`, and cache tests. `pnpm test -- src/lib/cache/api_queries.test.ts src/lib/cache/ws_events.test.ts src/lib/cache/query.test.ts` passes. |
| `frontend/src/lib/stores/audio_analysis.ts::{startAnalysis, stopAnalysis}` | Exact `rg` after removal finds only historical review docs. Settings still uses the audio analysis state, stats, status sync, passive DSP, and clear-all helpers. The API client start/stop methods remain because route contracts are manual-review boundaries. |
| `frontend/src/lib/routes/navigation.ts::routePathsForIds` | Exact `rg` found only the definition plus historical RepoWise reports. Removed the helper and the now-unused `appRoutePath` import while leaving route ids and navigation groups intact. |
| `frontend/src/lib/remote/sleep_timer.ts::sleepTimerActive` | Exact `rg` found only the definition plus historical RepoWise reports. The live sleep timer store plus `startSleepTimer` and `cancelSleepTimer` remain wired through `RemoteSettingsPill.svelte`. |
| `frontend/src/lib/routes/registry.ts::appRoutePath` | Exact `rg` found only the definition after `routePathsForIds` was removed. `appRoute` and `APP_ROUTES` remain as the live route registry API. |
| `frontend/src/lib/search/audio_params.ts::resolveGenreIds` export | Exact `rg` found only the helper definition and its internal call from `buildAudioParams`. Kept the helper private so library and search audio-filter behavior stays unchanged. |
| `noor-server/src/genre/builder.rs::{canonicalize_many, normalize_genres}` | Exact `rg` and rust-analyzer references found only self-file references and unit tests. The live ingestion path uses `collect_clear_genres`, so the all-or-nothing batch helper pair was removed. |
| `noor-server/src/genre/normalize.rs` | Exact `rg` and rust-analyzer references found no source callers outside the module's own unit tests. Removed the historical wrapper module and its `pub mod normalize` registration. |
| `noor-server/src/genre/mappings.rs::GenreResolution::is_ambiguous` | Became unused after the all-or-nothing batch normalizer was removed. Exact `rg` and rust-analyzer references found only the method definition, and the targeted genre tests exposed it as a dead-code warning. |
| `noor-server/src/genre/mappings.rs::{GenreResolution::is_clear, GenreCatalog::paths_for}` | Exact `rg` and rust-analyzer references found `is_clear` as definition-only and `paths_for` only in a unit-test assertion. Removed both suppressed helpers and switched the test to the live `path_for` helper. |
| `noor-server/src/services/audio_analysis/beat_tracker.rs` | Exact `rg` found only the module registration, historical plans, and self-file tests. Removed the uncalled prototype module so `cargo check -p noor-server` no longer reports its constants, struct, and tracker function as dead code. |
| `noor-server/src/services/charts/kworb_matrix.rs::ingest_kworb_matrix_html` | Exact `rg` and rust-analyzer references found only two unit tests plus the definition. Removed the test-only wrapper and changed tests to call the real `ingest_kworb_matrix_html_with_details` path with an empty detail map. |
| `noor-server/src/services/audio_analysis/dj_profile.rs::{LoadedDjProfile, PlannerPolicyShape, decode_safe_transition_windows, apply_correction_to_loaded_profile, transition_speed_bias_to_policy_shape}` | Exact `rg` found these only in definitions and unit tests. Removed the unwired correction/application helpers while keeping live profile row generation and safe-transition-window encoding. |
| `noor-server/src/services/audio_analysis/dj_profile.rs::build_audio_dj_profile_row` | Exact `rg` found only the definition after the DJ-profile helper cleanup. The live actor path uses `persist_dj_analysis_job_from_analysis` with an explicit beat-grid analysis, so this suppressed helper was removed. |
| `noor-server/src/smart/playlists.rs::{summarize_definition, summarize_clause, push_clause_summary}` | Exact `rg` found only self-file references and one unit test; rust-analyzer found no external references. Removed the unused smart-playlist summary scaffolding while leaving `evaluate_playlist` and playlist route behavior intact. |
| `noor-server/src/smart/artist_resolver.rs::{collision_count, len}` | Exact `rg` and rust-analyzer references found only self-file tests plus definitions. Removed the debug accessors and the now-unused collision counter field while preserving lookup behavior and the smallest-id collision policy. |
| `noor-server/src/smart/external_discovery.rs::build_trail_item` | Exact `rg` and rust-analyzer references found only the helper definition. Removed the unused trail-item convenience wrapper while leaving feed construction and route trail-item payloads intact. |
| `noor-server/src/services/spotify_public/client.rs::GetTrackResponseLike` | Exact `rg` and rust-analyzer references found only the struct definition. Removed the unused response wrapper and the now-unused `serde::Deserialize` import; `get_track` continues returning raw JSON `Value`. |
| `noor-server/src/services/audio_analysis/mod.rs::AnalysisConfig::min_interval_hours` | Exact `rg` found only struct/default assignments, and rust-analyzer references found no reads. The only live construction path uses `AnalysisConfig::default()` when spawning the passive analysis actor, so the unused field and stale interval comment were removed. |
| `noor-server/src/services/audio_analysis/features.rs::{compute_spectral_centroid, detect_instrumental, compute_danceability}` | Exact `rg` and rust-analyzer references found only self-file tests plus definitions. The live engine computes STFT once and calls `detect_instrumental_from` and `compute_danceability_from`; tests were updated to exercise that path directly. |
| `noor-server/src/services/discovery.rs::DiscoveryProvider::capabilities` | Exact `rg` and rust-analyzer references found only the trait declaration and TIDAL impl method. The live provider capability route uses `server/routes.rs::discovery_provider_capabilities`, so the public DTO and route payload stayed intact while the unused trait method was removed. |

## Duplicate Cleanup

| Finding | Evidence |
| --- | --- |
| Duplicate `TidalHomeItem` to `TidalPlayable` conversion in `TidalDiscoverShelves.svelte` and `search/discover/[id]/+page.svelte` | Consolidated into `frontend/src/lib/utils/track.ts::tidalHomeItemToPlayable`. `pnpm test -- src/lib/utils/track.test.ts src/lib/components/search/tidal_discover_artwork_contract.test.ts src/routes/search/search_layout_contract.test.ts` passes. |
| Duplicate fallback `initials` helpers in `ArtistCarousel.svelte`, `LibraryHero.svelte`, and `search/+page.svelte` | Consolidated into `frontend/src/lib/utils/text.ts::initials`. Exact `rg` now finds only the shared helper definition and imports. |
| Duplicate fixture random/date helpers in `demo-tempo.ts`, `demo-sonic-field.ts`, `demo-ridgeline.ts`, and `demo-kpis.ts` | Consolidated into `frontend/src/lib/fixtures/demo-random.ts`. These helpers are local analytics preview fixture code, not public route, playback, serde, Tauri, or context-menu boundaries. |
| Duplicate album-total duration labels in local, TIDAL, and remote album pages | Consolidated into `frontend/src/lib/utils/format.ts::formatTotalDuration`, with unit coverage for minute and hour labels. |

## Keep

| Finding | Evidence |
| --- | --- |
| `noor-server/src/server/routes/tests.rs` | Imported from `noor-server/src/server/routes.rs` as `#[cfg(test)] mod tests;`. `cargo test -p noor-server all_api_routes_are_registered` and `cargo test -p noor-server server::routes::tests` pass. |
| `noor-server/src/server/routes/dj_routes.rs::routes` | Merged by `noor-server/src/server/routes.rs` through `dj_routes::routes()`. This is a public Axum route registration path. |
| `frontend/src/lib/api/client.ts::{setStoredToken, clearStoredToken}` | Referenced from `frontend/src/routes/+layout.svelte` and `frontend/src/routes/settings/+page.svelte`. Auth token handling is a public app boundary. |
| `frontend/src/lib/api/ws.ts::connectWebSocket` | Referenced from `frontend/src/routes/+layout.svelte`. WebSocket lifecycle helpers are a public app boundary. |
| `frontend/src/lib/actions/lazy-tidal-art.ts::lazyTidalArt` | Referenced by `AlbumCarousel.svelte`, `ArtistCarousel.svelte`, `ChartMural.svelte`, `VideoCard.svelte`, library and Spotify routes, plus the artwork contract script. |
| `frontend/src/lib/actions/wheel-to-horizontal.ts::wheelToHorizontal` | Referenced by carousel, rail, search, mood, and home Svelte surfaces. |
| `frontend/src/lib/utils/color.ts::letterColor` | Referenced by artist and search surfaces. |
| `frontend/src/lib/utils/debounce.ts::debounce` | Referenced by `frontend/src/routes/analytics/+page.svelte`. |
| `frontend/src/lib/utils/track.ts::tidalSearchTrackToPlayable` | Referenced by `frontend/src/routes/search/+page.svelte`. |
| `frontend/src/lib/player/play_trending.ts::{playChartTidalTrack, playChartTidalTracks}` | Referenced by `TrendingShelf.svelte`, `HomeRecommendationsShelf.svelte`, and related contract tests. |
| `frontend/src/lib/player/video_menu.ts::buildVideoMixMenu` | Referenced by `frontend/src/lib/components/video/VideoCard.svelte`. Context-menu builders are manual-review only even when apparently unused. |
| `frontend/src/lib/remote/action_sheet.ts::openActionSheet` | Referenced by remote transport, queue, track row, album tile, and mini search components. |
| `frontend/src/lib/remote/haptics.ts::{hapticCommit, hapticAccent}` | Referenced by remote transport, action bar, queue, and settings components. |
| `frontend/src/lib/components/Genre/galaxyBuilder.ts::buildGalaxyData` | Referenced by `frontend/src/routes/genres/+page.svelte`. |
| `frontend/src/lib/components/charts/ridge-kde.ts::{kde1d, ridgePath, rowMax}` | Referenced by `TempoRidges.svelte`, `MiniSilhouette.svelte`, and `ListenRidgeline.svelte`. |
| `frontend/src/lib/components/wallpaper/palettes.ts::rgbCss` | Referenced by `frontend/src/routes/settings/+page.svelte`. |
| `frontend/src/lib/components/wallpaper/shaders.ts::wallpaperById` | Referenced by `frontend/src/routes/+layout.svelte`. |
| `frontend/src/lib/stores/uiZoom.ts::{zoomIn, zoomOut, nudgeZoom, resetZoom}` | Referenced by layout and settings surfaces. |
| `frontend/src/lib/stores/wallpaper.ts::{setWallpaper, setWallpaperFps, setWallpaperBlur}` | Referenced by `frontend/src/routes/settings/+page.svelte`. |
| `frontend/src/lib/stores/playlist_artwork_cache.ts` exported helpers | Referenced by `frontend/src/routes/playlists/+page.svelte`. |
| TIDAL home, mixes, moods, and radio cache helpers | Referenced by home, search, and mood Svelte surfaces. |
| `frontend/src/lib/stores/queue_announcer.ts::flushResolved` | Internal timer callback in the same file. Repowise labeled it incorrectly as an export candidate. |
| `frontend/src/lib/stores/quiet_mode.ts::{openQuietMode, closeQuietMode}` | Referenced by layout and `QuietMode.svelte`. |
| `frontend/src/lib/stores/tidal.ts::{setAutoSyncDaily, cancelTidalSync}` | Referenced by `frontend/src/routes/settings/+page.svelte`. |
| `frontend/src/lib/stores/context_menu.ts` exported store and open/close helpers | Referenced by layout, album/artist/search/library/chart/video routes, shared components, and overlay contract tests. Context-menu behavior is a manual-review boundary. |
| `frontend/src/lib/stores/tidal-moods-cache.ts::{getCachedMoodPage, putCachedMoodPage}` | Referenced by `frontend/src/routes/moods/[slug]/+page.svelte`; the landing cache helpers are also used by the moods route and home moods rail. |
| Spotify normalization helpers in `frontend/src/lib/api/client.ts` | Referenced internally by playlist, track, album, artist, and search normalizers in `client.ts`. Exact `rg -n "normalizeSpotify"` shows calls around playlist detail, Spotify search, and detail response normalization. |
| `noor-server/src/smart/taste_vector/adapters.rs::{AnalyticsContext, from_taste_mesh, from_analytics_overview}` | Already documented as intentional Phase 2/3 placeholders in `docs/playback-inventory.md` and `docs/dev/repowise-dead-code.md`. |
| `frontend/src/lib/stores/quiet_mode.ts::toggleQuietMode` | Removed after exact `rg` found only the store definition and this audit note. The live UI paths still call `openQuietMode` from the player bar and `closeQuietMode` from `QuietMode.svelte`. |
| `frontend/src/lib/stores/training.ts::{handleTrainingFailed, resetTrainingState}` | Removed after exact `rg` found only the store definitions and this audit note. The live websocket producer still uses `handleTrainingProgress` and `handleTrainingComplete`; settings failures use local page state. |
| `frontend/src/lib/api/ws.ts::disconnectWebSocket` | Removed after exact `rg` found no live caller. The app layout still imports and calls `connectWebSocket`; the current unmount cleanup never disconnected the singleton websocket. |
| `noor-server/src/services/discovery_blend.rs::Playability::Unavailable` | Keep as a reserved serialized discovery blend response value. Rust-analyzer found no current construction, but `/api/discovery/blend/*` serializes `playability`, and the frontend `DiscoverPlayability` union already accepts `unavailable`. |
| `noor-server/src/services/tidal/auth.rs::PersistedTidalTokens::tokens` | Kept as a `#[cfg(test)]` helper. Rust-analyzer found references only in encrypted and legacy plaintext persistence tests, where the helper checks decoded token fields without consuming the wrapper before `needs_encrypted_rewrite()`. |
| `frontend/src/lib/components/DiscoverSpace/discover_space_story.ts::RADIO_MODE_NAMES` | Removed after exact `rg` found only the story definition. `RadioMode` remains live in the route and store. |

## Manual Review

| Finding group | Why it stays untouched |
| --- | --- |
| DiscoverSpace adapter, physics, renderer, and store exports | Visual engine helpers can be imported from Svelte components and contract scripts in ways Repowise currently misses. Exercise the DiscoverSpace surface before deleting anything beyond the reviewed story-copy constant. |
| Player store exports such as queue, automix, shuffle, repeat, and playlist helpers | Playback and queue boundaries are manual-review only by repo rule. |
| Repeated frontend artwork failure helpers such as `markArtworkFailed` and `failedArtworkUrls` | Duplicate scan found the same local fallback pattern across player, album, artist, TIDAL, quiet-mode, and remote surfaces. These are visible UI fallback paths with per-component Svelte state and contract-test coverage, so consolidation should happen in a focused artwork-surface slice rather than this backend cleanup slice. |
| `noor-server/src/db/queries.rs` cargo-check dead-code warnings | SQL/query boundary. `cargo check -p noor-server` on 2026-06-04 still reports 10 query warnings: Spotify playcount cache, embedding-model lookup and rollback helpers, external-candidate feature and prune helpers, and temporary DJ-profile promotion. Several are still referenced by unit tests, docs, or playback plans, so they need a query-contract slice rather than broad deletion. |
| `noor-server/src/playback/{dj_engine.rs, player.rs, queue.rs, runtime/mod.rs, wasapi_exclusive.rs}` cargo-check dead-code warnings | Playback, queue, WASAPI, and output-switching boundaries are manual-review only by repo rule. `cargo check -p noor-server` on 2026-06-04 still reports 9 playback/output warning groups around DJ lookahead, shuffle, transition planning, prepared mixer state, shared fallback output, and exclusive output before any removal. |
| Any remaining safe-only output not listed under `keep` | Not locally validated in this course. Treat as manual review, not as a delete request. |

## Hook-Up Review Notes

These notes were added before any staging decision. They separate "unused and
obsolete" from "implemented but not wired yet" so cleanup does not erase a
planned product path.

| Finding group | Review note | Resolution for this cleanup |
| --- | --- | --- |
| `noor-server/src/services/audio_analysis/beat_tracker.rs` | Exact refs showed no live caller, but historical DSP plans describe this module as part of a BPM detector rewrite. Current `bpm.rs` uses the vendored `madmom_beats_port_core` beat/downbeat model instead, and the DJ-engine plan says V1 should extend that path rather than add a second tracker. | Keep removed. If an Ellis tracker is needed later, restore it as an explicit fallback or benchmark helper with a live caller, not as an unregistered module. |
| `noor-server/src/services/audio_analysis/mod.rs::AnalysisConfig::min_interval_hours` | Exact refs showed no reads. The name and old comment imply a throttle knob for passive analysis, but the current actor gates on `analysis_version` and manual override instead. | Keep removed. If age-based passive reanalysis is needed, add an explicit config field plus SQL `analyzed_at` comparison in a behavior slice. |
| `noor-server/src/services/discovery.rs::DiscoveryProvider::capabilities` | Exact refs showed only trait and impl definitions. The live route uses a static provider capability list that also includes providers without live provider objects. | Keep removed. If provider capabilities should become dynamic, redesign `discovery_provider_capabilities()` around actual provider registration. |
| `noor-server/src/smart/playlists.rs` summary helpers | Exact refs showed only self-file tests. Frontend currently renders smart-rule summaries on the playlists page from the serialized rule definition. | Keep removed. If API clients or remote UI need server-authored summaries, wire a response field deliberately instead of preserving an unused helper. |
| `noor-server/src/services/audio_analysis/dj_profile.rs` correction helpers | Removed helpers were not called by production. Correction behavior is live in `playback/dj_engine.rs`, DJ routes, and DJ cockpit UI. | Keep removed after confirming the live DJ-engine correction tests pass. |
| `frontend/src/lib/components/TrendingCard.svelte` | Exact refs found no live imports or route usage. The live charts route imports `TrendingShelf.svelte`, and the current contract test asserts that `TrendingShelf.svelte` uses `ChartMural` and does not contain `TrendingCard`. No `CarouselTrending` symbol or file exists in the current frontend tree. | Keep uncommitted until the frontend slice is reviewed. If the old card UI is wanted, restore it deliberately and wire it into `TrendingShelf` or a named route with playback, context menus, and artwork fallback tests. |

## Commit Split Notes

The worktree also contains unrelated or mixed UI, release, Tauri, and generated
file changes. Do not stage the whole worktree as one audit commit.

| Candidate commit | Include | Exclude |
| --- | --- | --- |
| Backend dead-code cleanup | `noor-server/src/genre/*`, `noor-server/src/services/audio_analysis/*`, `noor-server/src/services/charts/kworb_matrix.rs`, `noor-server/src/services/discovery.rs`, `noor-server/src/services/spotify_public/client.rs`, `noor-server/src/smart/*`, plus the genre and audit docs. | DB query warnings, playback/runtime/WASAPI warnings, TIDAL auth, and radio config suppressed fields until their manual-review slices are done. |
| Frontend isolated dead exports | `TrendingCard.svelte`, cache exports, audio-analysis store wrappers, route helper removals, remote sleep timer export, and `resolveGenreIds` export cleanup. | Mixed artwork-surface files and unrelated app-shell or route layout changes. |
| Duplicate helper cleanup | Shared helpers and tests for `demo-random`, `tidalHomeItemToPlayable`, `initials`, and `formatTotalDuration`. | Svelte caller files that also contain artwork fallback changes unless the relevant hunks are staged separately. |
| Not audit cleanup | Release workflows, Tauri updater/path changes, generated caches, showcase package changes, broad artwork fallback work, and unrelated docs. | Keep unstaged for separate review or separate commits. |

## Pre-Commit Review Status

| Slice | Status | Evidence |
| --- | --- | --- |
| Frontend isolated dead exports | Safe candidate for a separate commit, still unstaged. | Exact `rg` on 2026-06-03 found no live callers for `peekCachedValue`, `scheduleTargetedRefetch`, `startAnalysis`, `stopAnalysis`, `routePathsForIds`, `sleepTimerActive`, `appRoutePath`, or exported `resolveGenreIds`. It found `TrendingCard` only in this audit doc and the `TrendingShelf` contract test negative assertion. `pnpm test -- src/lib/cache/api_queries.test.ts src/lib/cache/ws_events.test.ts src/lib/cache/query.test.ts src/lib/components/charts/trending_shelf_contract.test.ts src/lib/routes/registry.test.ts scripts/remote-route-contract.test.mjs scripts/shell-navigation-contract.test.mjs scripts/library-search-all-contract.test.mjs scripts/search-clear-control-contract.test.mjs src/routes/search/search_layout_contract.test.ts` passed with 10 files and 61 tests. |
| Fixture demo-random consolidation | Safe candidate for a helper-only commit, still unstaged. | `demo-kpis.ts`, `demo-ridgeline.ts`, `demo-sonic-field.ts`, and `demo-tempo.ts` now import `mulberry32`, `gaussian`, and `dateNDaysAgo` from `frontend/src/lib/fixtures/demo-random.ts`. Exact `rg` found those function definitions only in the shared helper file. |
| Duplicate helper cleanup in UI callers | Not safe to commit as a whole-file audit slice. | `TidalDiscoverShelves.svelte` and `search/discover/[id]/+page.svelte` mix `tidalHomeItemToPlayable` consolidation with `ArtworkImage` migration. `ArtistCarousel.svelte`, `LibraryHero.svelte`, and `search/+page.svelte` mix `initials` consolidation with artwork fallback changes and copy cleanup. Album pages mix `formatTotalDuration` consolidation with TIDAL artwork fallback state. Use hunk staging for helper-only cleanup, or keep the whole-file changes for a focused artwork/UI commit after visual route verification. `pnpm test -- src/lib/utils/track.test.ts src/lib/utils/text.test.ts src/lib/utils/format.test.ts src/lib/fixtures/demo-random.test.ts src/lib/components/search/tidal_discover_artwork_contract.test.ts src/routes/search/search_layout_contract.test.ts src/routes/library/library_home_contract.test.ts src/lib/components/album_artwork_contract.test.ts` passed with 8 files and 67 tests. |
| Artwork/UI fallback bucket | Test, typecheck, production-build, and production-preview browser smoke clean for app and remote routes, still unstaged. Dynamic local and TIDAL detail routes need backend/dev-server validation before calling the UI bucket fully verified. | Changed surfaces include app shell artwork, player bar, album and artist routes, search, library, remote routes, automix, duplicates, playlists, TrackRow/TidalTrackRow, video cards, DiscoverSpace, and home shelves. `rg` found no arbitrary TIDAL artwork sizes such as 256 or 512 in `frontend/src`; observed sizes are from the allowed set. `pnpm test -- src/lib/utils/artwork.test.ts src/lib/utils/tidal_artwork_surface_contract.test.ts src/lib/components/album_artwork_contract.test.ts src/lib/components/track_row_contract.test.ts src/lib/components/quiet_mode_layout_contract.test.ts src/lib/components/DiscoverSpace/discover_space_artwork_contract.test.ts src/lib/components/home/tidal_home_artwork_contract.test.ts src/lib/components/remote/remote_artwork_contract.test.ts src/lib/components/remote/remote_queue_artwork_contract.test.ts src/lib/components/remote/remote_mini_search_contract.test.ts src/lib/components/search/tidal_discover_artwork_contract.test.ts src/lib/components/video/video_card_artwork_contract.test.ts src/routes/albums/album_layout_contract.test.ts src/routes/artists/artist_layout_contract.test.ts src/routes/automix/automix_artwork_contract.test.ts src/routes/library/library_detail_artwork_contract.test.ts src/routes/library/library_home_contract.test.ts src/routes/library/library_layout_contract.test.ts src/routes/playlists/playlist_artwork_contract.test.ts src/routes/remote/remote_detail_artwork_contract.test.ts src/routes/remote/remote_library_artwork_contract.test.ts src/routes/search/search_layout_contract.test.ts` passed with 22 files and 65 tests. `pnpm check` passed with 0 errors and 0 warnings after rerunning outside the sandbox because the sandboxed run hit `.svelte-kit` EPERM. `pnpm run build` passed after rerunning outside the sandbox because the sandboxed run hit Vite `spawn EPERM`. Browser smoke via `pnpm preview --host 127.0.0.1 --port 4173` and system Edge passed with 0 broken images, 0 page errors, and 0 console errors on `/`, `/library`, `/search`, `/remote`, `/remote/library`, `/remote/albums/1`, `/remote/artists/1`, `/automix`, `/duplicates`, `/playlists`, and `/charts`. Static preview returned 404 for `/albums/1`, `/artists/1`, `/tidal/albums/1`, and `/tidal/artists/1`; those had no broken images or page errors, but need backend/dev-server route validation for real detail states. |

## Next Check

Run future cleanup in small slices:

1. Pick one manual-review group.
2. Run exact `rg` checks across `frontend`, `noor-server`, `noor-app`, `docs`, and `scripts`.
3. Check dynamic boundaries named in `docs/dev/repowise-dead-code.md`.
4. Remove the smallest unit only if no boundary is involved.
5. Run the smallest relevant product-path test or build.
