# RepoWise Dead-Code Cleanup Guardrails

RepoWise dead-code findings are starting points, not deletion approval. Before
removing any symbol or file, confirm the finding with local reference checks and
the smallest relevant validation commands.

## Known False-Positive Classes

- Svelte `$lib` aliases: RepoWise can miss imports that resolve through SvelteKit
  aliases. Check route, component, script, and test references with `rg`.
- Vitest test entry files: test files may have no importers because the test
  runner discovers them by glob.
- Rust route-backed service functions: functions called from Axum route modules
  can appear unused when only direct import edges are considered.
- Serde and public DTOs: request, response, event, and persistence types may be
  used through serialization, deserialization, or external contracts.
- WebSocket event contracts: event variants and payload shapes are client/server
  boundaries even when individual fields look unused.
- Context-menu helpers: shared builders and store helpers are app-wide UI
  contracts. Treat all context-menu findings as manual-review only.
- DiscoverSpace renderer helpers: renderer, physics, and story helpers are used
  through Svelte component-local imports and should not be removed from RepoWise
  output alone.
- Taste-vector Phase 2/3 placeholders: `from_taste_mesh`,
  `from_analytics_overview`, and related context types are intentional future
  contracts unless the phase plan explicitly changes.
- ACRCloud scanner placeholders: scanner paths may have zero callers while the
  event/API surface is still being developed. Treat as manual-review only unless
  the product decision is to remove the scanner flow.

## Deletion Rule

A RepoWise finding is safe to delete only after all of the following are true:

1. Local `rg` checks show no real callers, including aliases, route modules,
   tests, scripts, and docs that define intended contracts.
2. The symbol is not part of a public API, serde DTO, WebSocket event, route,
   SQL/query boundary, playback boundary, auth boundary, or context-menu helper.
3. The smallest relevant build or test command passes after the removal.

Default validation commands:

- Frontend: `pnpm lint`, `pnpm check`, `pnpm test`, `pnpm run build`.
- Rust server: `cargo check -p noor-server`, plus targeted `cargo test` when a
  module has tests.
- Tauri app: `cargo check -p noor-app` when the change touches `noor-app`.
