# Rust MCP Workflow

Use rust-analyzer or Rust MCP tooling before risky Rust edits. Text search is still useful for finding candidate files, but symbol-aware tools should drive changes to shared types, route handlers, database access, playback, auth, and server startup code.

## Rust Repo Map

- Workspace root: `Cargo.toml`
- Workspace members: `noor-server`, `noor-app`
- Backend package: `noor-server`
- Tauri shell package: `noor-app`
- Axum app shell: `noor-server/src/server/mod.rs`
- Main route table: `noor-server/src/server/routes.rs`
- Split route modules: `noor-server/src/server/routes/*.rs`
- WebSocket routes: `noor-server/src/server/ws.rs`
- Server state and startup: `noor-server/src/main.rs`
- Tauri app entrypoint: `noor-app/src/main.rs`
- Tauri sidecar, tray, media keys, and updater: `noor-app/src/sidecar.rs`, `noor-app/src/tray.rs`, `noor-app/src/media_keys.rs`, `noor-app/src/updater.rs`
- Playback queue and state logic: `noor-server/src/playback/player.rs`, `noor-server/src/playback/queue.rs`
- Playback runtime and transition code: `noor-server/src/playback/runtime/`, `noor-server/src/playback/gapless.rs`
- Audio decode and output paths: `noor-server/src/playback/decode/`, `noor-server/src/playback/output/`, `noor-server/src/playback/wasapi_exclusive.rs`
- DSP and audio analysis: `noor-server/src/services/audio_analysis/`
- Database wrapper: `noor-server/src/db/mod.rs`
- Database models and queries: `noor-server/src/db/models.rs`, `noor-server/src/db/queries.rs`
- Migrations: `noor-server/src/db/schema.rs`
- Discovery routes: `noor-server/src/server/routes/discovery_routes.rs` plus discovery endpoints in `noor-server/src/server/routes.rs`
- Discovery and radio domain logic: `noor-server/src/smart/`, `noor-server/src/services/discovery*.rs`, `noor-server/src/services/radio.rs`, `noor-server/src/server/radio_pipeline.rs`

## Safe Workflow

1. Find the symbol with rust-analyzer symbols or workspace-symbol tooling.
2. Read the definition before changing behavior.
3. Inspect hover/type info for async chains, iterator chains, Axum extractors, rusqlite closures, and playback runtime messages.
4. Find references before renaming, removing, changing visibility, or changing a shared type.
5. Check file or workspace diagnostics before editing.
6. Make the smallest change that addresses the requested behavior.
7. Run targeted validation:
   - `cargo check -p noor-server` for backend changes
   - `cargo check -p noor-app` for Tauri shell changes
   - Targeted `cargo test -p <package> <filter>` when behavior or parsing changes
8. Re-check diagnostics after the edit.

## Generic Setup

Use whatever MCP server your local Codex setup supports. A typical setup is:

```powershell
rustup component add rust-analyzer
cargo install rust-analyzer-mcp
codex mcp add rust-analyzer -- rust-analyzer-mcp
codex mcp list
```

If your local Codex config already has MCP servers, inspect the config diff before changing it. Do not commit personal MCP config files, absolute machine paths, tokens, or local secrets.

## Useful Tools

Prefer these capabilities when available:

- Symbols
- Definitions
- References
- Hover/type info
- File diagnostics
- Workspace diagnostics
- Code actions

## Caveats

- MCP availability can vary by session and local setup.
- Frontend work still needs Svelte and TypeScript tooling. Rust MCP cannot validate Svelte runes, TypeScript types, or frontend lint rules.
- `cargo fmt --all` remains the formatter for Rust changes. Do not use MCP formatting as a substitute for repo formatter rules.
