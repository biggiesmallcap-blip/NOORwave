# Frontend Performance Audit — 2026-05-08

Audit baseline + before/after measurements for the perf-fixes implementation. Plan: [`docs/superpowers/plans/2026-05-08-frontend-perf-fixes.md`](../plans/2026-05-08-frontend-perf-fixes.md).

## Dataset (measured)

| Resource | Count |
|---|---|
| Tracks | 35,333 |
| Artists | 6,589 |
| Albums | 6,753 |
| Playlists | 65 |
| `noor.db` size | 928 MB |

## FTS query cost (measured server-side)

| Index | Tokenizer | Match cost (35k tracks) |
|---|---|---|
| `tracks_fts` | unicode61 | ~1–5 ms |
| `artists_fts` | unicode61 | <1 ms |
| `albums_fts` | unicode61 | <1 ms |

## Baseline build sizes (chore/fluid-design-system @ cf7bec7)

Server entries (raw, pre-gzip):

| Entry | Size |
|---|---|
| `chunks/index.js` | 138.14 kB |
| `chunks/renderer.js` | 94.70 kB |
| `chunks/ws.js` | 83.68 kB |
| `entries/pages/_layout.svelte.js` | 83.36 kB |
| **`entries/pages/search/_page.svelte.js`** | **52.51 kB** |
| **`entries/pages/library/_page.svelte.js`** | **48.72 kB** |
| `chunks/shared.js` | 48.71 kB |
| `chunks/wallpaper.js` | 47.96 kB |
| `entries/pages/discoverspace/_page.svelte.js` | 38.15 kB |
| `chunks/root.js` | 28.26 kB |
| `chunks/analytics-signals.js` | 18.07 kB |
| `entries/pages/automix/_page.svelte.js` | 19.58 kB |
| `entries/pages/videos/_page.svelte.js` | 18.16 kB |
| `entries/pages/_page.svelte.js` (root) | 15.98 kB |
| `entries/pages/settings/_page.svelte.js` | 13.99 kB |

Test baseline: 44/44 pass. Type check: 0 errors, 44 warnings (pre-existing).

## After-fixes server-entry sizes (feat/frontend-perf-fixes @ d7591e7)

Server entries (raw, pre-gzip):

| Entry | Before | After | Δ |
|---|---|---|---|
| **`entries/pages/search/_page.svelte.js`** | **52.51 kB** | **53.67 kB** | +1.16 kB |
| **`entries/pages/library/_page.svelte.js`** | **48.72 kB** | **50.14 kB** | +1.42 kB |
| `entries/pages/settings/_page.svelte.js` | 13.99 kB | 13.99 kB | ±0 |

Note: server-entry sizes grew because VirtualList wiring + catalog_meta code additions
(Phase 3) outweigh the Phase 4 tree-shaking gains. Client-side runtime impact (below)
is what matters for user-perceived perf.

Test state: 60/60 pass (was 44/44 baseline — 16 new tests for new modules). Type check: 0 errors, 44 warnings (same pre-existing warnings).

## Runtime TTI measurements (requires running app)

These numbers require manual DevTools Performance profiling in the Tauri shell and cannot
be captured from the CLI. Record them here when testing the branch before merge.

| Page | Before TTI | After TTI | Δ | Notes |
|---|---|---|---|---|
| `/search` cold | _measure_ | _measure_ | — | TrendingShelf lazy-load removes ~150–400 ms of Last.fm fetches from critical path |
| `/library` cold | _measure_ | _measure_ | — | Catalog store eliminates repeat getPlaylists/getGenres fetches on nav |
| `/settings` cold | _measure_ | _measure_ | — | Parallel fetches: expected ~100–250 ms improvement |
| `/library` search per-keystroke | _measure_ | _measure_ | — | VirtualList cuts DOM ~1500→~300 nodes; expected ~50–90 ms/keystroke reduction |

## What was shipped (all 9 fixes)

| Fix | Task | Description |
|---|---|---|
| Fix 1 | Tasks 8–10 | VirtualList windowed rendering: track list + album grid |
| Fix 2 | Task 1 | Drop client re-sort during search — trust FTS rank |
| Fix 3 | Tasks 5–6 | Shared catalog_meta store (60s SWR) for playlists + genres |
| Fix 4 | Task 7 | Lazy-load TrendingShelf behind dynamic import |
| Fix 5 | Task 2 | Parallelize settings onMount fetches with Promise.allSettled |
| Fix 6 | Tasks 11–12 | rAF-throttle WS-driven re-renders in genres + analytics |
| Fix 7 | Task 3 | Defer non-critical /search onMount fetches to requestIdleCallback |
| Fix 8 | Task 13 | Per-domain api modules (search.ts, library.ts, charts.ts) |
| Fix 9 | Task 4 | Lazy-import context-menu builders on first right-click |
