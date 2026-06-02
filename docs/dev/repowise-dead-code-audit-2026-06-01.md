# Repowise Dead-Code Audit - 2026-06-01

## Scope

This is the audit-only dead-code pass for the Repowise signal upgrade course.
No code was deleted.

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
| `remove candidate` | None approved in this course. | Keep all code in place until exact local references, dynamic boundary checks, and targeted tests are complete. |
| `keep` | Confirmed false positives and intentional placeholders listed below. | Do not remove from this Repowise output. |
| `manual review` | Remaining safe-only findings not locally proven as false positives. | Review in smaller follow-up slices with exact `rg` checks and product-path validation. |

## Keep

| Finding | Evidence |
| --- | --- |
| `noor-server/src/server/routes/tests.rs` | Imported from `noor-server/src/server/routes.rs` as `#[cfg(test)] mod tests;`. `cargo test -p noor-server all_api_routes_are_registered` and `cargo test -p noor-server server::routes::tests` pass. |
| `noor-server/src/server/routes/dj_routes.rs::routes` | Merged by `noor-server/src/server/routes.rs` through `dj_routes::routes()`. This is a public Axum route registration path. |
| `frontend/src/lib/api/client.ts::{setStoredToken, clearStoredToken}` | Referenced from `frontend/src/routes/+layout.svelte` and `frontend/src/routes/settings/+page.svelte`. Auth token handling is a public app boundary. |
| `frontend/src/lib/api/ws.ts::connectWebSocket` | Referenced from `frontend/src/routes/+layout.svelte`. WebSocket lifecycle helpers are a public app boundary. |
| `frontend/src/lib/actions/lazy-tidal-art.ts::lazyTidalArt` | Referenced by `AlbumCarousel.svelte`, `ArtistCarousel.svelte`, `ChartMural.svelte`, `TrendingCard.svelte`, `VideoCard.svelte`, library and Spotify routes, plus the artwork contract script. |
| `frontend/src/lib/actions/wheel-to-horizontal.ts::wheelToHorizontal` | Referenced by carousel, rail, search, mood, and home Svelte surfaces. |
| `frontend/src/lib/utils/color.ts::letterColor` | Referenced by artist, carousel, trending, and search surfaces. |
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
| `noor-server/src/smart/taste_vector/adapters.rs::{AnalyticsContext, from_taste_mesh, from_analytics_overview}` | Already documented as intentional Phase 2/3 placeholders in `docs/playback-inventory.md` and `docs/dev/repowise-dead-code.md`. |

## Manual Review

| Finding group | Why it stays untouched |
| --- | --- |
| `frontend/src/lib/api/ws.ts::disconnectWebSocket` | WebSocket lifecycle boundary. Exact removal needs route and app-shell lifecycle review. |
| Spotify normalization helpers in `frontend/src/lib/api/client.ts` | Public API client helpers. Removal needs search through route data flow, docs, and import aliases. |
| `frontend/src/lib/cache/api_queries.ts::{peekCachedValue, scheduleTargetedRefetch}` | Cache and WebSocket invalidation infrastructure. Needs a cache-event flow review. |
| DiscoverSpace adapter, physics, renderer, store, and story exports | Visual engine helpers can be imported from Svelte components and contract scripts in ways Repowise currently misses. Exercise the DiscoverSpace surface before deleting anything. |
| Player store exports such as queue, automix, shuffle, repeat, and playlist helpers | Playback and queue boundaries are manual-review only by repo rule. |
| `frontend/src/lib/stores/quiet_mode.ts::toggleQuietMode` | Only direct references found in this audit were definitions, but this is a UI state-store public helper. Needs settings and keyboard-flow validation before removal. |
| Training store helpers `handleTrainingFailed` and `resetTrainingState` | Training state is user-visible. Needs producer-event and settings-flow validation before removal. |
| Any remaining safe-only output not listed under `keep` | Not locally validated in this course. Treat as manual review, not as a delete request. |

## Next Check

Run future cleanup in small slices:

1. Pick one manual-review group.
2. Run exact `rg` checks across `frontend`, `noor-server`, `noor-app`, `docs`, and `scripts`.
3. Check dynamic boundaries named in `docs/dev/repowise-dead-code.md`.
4. Remove the smallest unit only if no boundary is involved.
5. Run the smallest relevant product-path test or build.
