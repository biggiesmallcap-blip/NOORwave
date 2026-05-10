# NOORwave — Agent Instructions

This file is the canonical instruction set for AI coding agents (Codex, etc.) working in this repo. It covers conventions, landmines, and standing preferences that are not obvious from reading the code.

---

## Commit and PR rules

- **No AI attribution.** Never add `Co-Authored-By: Claude ...`, "Generated with Claude Code", or any other AI/Anthropic attribution line to commit messages, PR titles, or PR bodies. Commits must look authored solely by the human.
- **Plain commit messages** focused on the change itself. No trailers beyond standard `Signed-off-by` if required.

---

## Release flow

When cutting a `vX.Y.Z` release tag:

1. Bump **all three** version files to `X.Y.Z` before tagging:
   - `noor-server/Cargo.toml` — `version = "X.Y.Z"`
   - `noor-app/Cargo.toml` — `version = "X.Y.Z"`
   - `noor-app/tauri.conf.json` — `"version": "X.Y.Z"`
2. Refresh `Cargo.lock`: `cargo update -p noor-server --offline && cargo update -p noor-app --offline`
3. Include all three files in the release commit, then tag and push.
4. `frontend/package.json` sits at `0.0.1` permanently — do NOT sync it.

After the tag CI completes:

5. `gh release view vX.Y.Z --json body -q .body` to capture the auto-generated boilerplate.
6. `gh release edit vX.Y.Z --notes "..."` to **prepend** a "What's new in vX.Y.Z" section ahead of the existing boilerplate. Do not replace the download/install/verification sections.
7. Structure the changelog as `### Categories` (e.g. "DSP correctness", "Performance", "Other", "Tests").
8. Include `**Full diff**: https://github.com/biggiesmallcap-blip/NOORwave/compare/vPREV...vX.Y.Z` near the bottom.

---

## GitHub Actions audits

When SHA-pinning or auditing `uses:` refs in `.github/workflows/*.yml`:

- Bump every action to its **latest major**, not just any Node-24-compatible version.
- For each `uses: org/repo@<sha> # vN`, check: `gh api repos/org/repo/tags --jq '.[0].name'`
- "Node-24 compatible" is the floor, not the ceiling. Latest-major is the target.

---

## Settings UI organization

- Tidal, Last.fm, Spotify, MusicBrainz enrichment, and any streaming-source feature go under the **Sources** category in the settings page.
- The `Data` category is filtered out of the visible nav — do not re-enable it for a single card.
- When adding a new settings card, find the `{#if activeCategory === 'X'}` block with the most-related existing setting and place the new card there.
- Do not introduce a new top-level settings category without asking.

---

## Context menus on asset references

Every reference to a track, album, or artist must support right-click:

- Rows, cards, artist circles, inline text links, thumbnails inside cards, secondary references inside other components — all of them.
- Wire up `oncontextmenu={(e) => openContextMenu(e, buildTrackMenu(...) | buildAlbumMenu(...) | buildArtistMenu(...))}`.
- Always use the shared builders (`buildTrackMenu`, `buildAlbumMenu`, `buildArtistMenu`). Never inline menu arrays — they drift.
- Exception: Command Palette (intentionally minimal). Get explicit confirmation before adding similar exceptions elsewhere.
- Missing context menus on asset references are bugs, not polish items.

---

## Formatting (Rust)

- **Do not manually reformat code.** If formatting needs fixing, run `cargo fmt` and commit what it produces. Never submit a diff whose only changes are whitespace or brace/argument placement.
- `rustfmt.toml` at the repo root is authoritative. If `cargo fmt` and the existing code disagree, trust `cargo fmt` - do not hand-correct back to the original.
- Specifically: do not move `{` onto its own line after `if let ... && let ...` chains, and do not expand function call arguments to multi-line unless the line exceeds the formatter's width limit.
- To change a formatting rule, propose a `rustfmt.toml` edit - do not reformat files by hand.
- CI runs `cargo fmt --all -- --check` on every push (`rust-fmt` job in `pr-check.yml`). Commits that don't pass the formatter will fail the check - run `cargo fmt` before pushing.

---

## Conventions

- **No em dashes anywhere.** Code, comments, commit messages, user-facing copy. Use a regular dash, a colon, or rewrite.
- **Direct, punchy voice** in user-facing strings. No hedging, no "seamlessly", no "robust".
- **Error handling (Rust):** `anyhow::Result<T>` for handlers; `thiserror` for domain error enums. Propagate with `?`. Log with `tracing::warn!` / `tracing::info!` / `tracing::error!`. Never `unwrap()` on user-driven paths.
- **DB queries:** inline in route handlers via `state.read().await.db.with_conn(|conn| ...)`. Always parameterize — never format SQL strings with user input.
- **Migrations:** append-only in `noor-server/src/db/schema.rs`. Never edit a past migration. Append `MIGRATION_0NN` const and add it to the `MIGRATIONS` slice.
- **Comments** explain *why*, never *what*.
- **Inline styles** in Svelte are rejected by `pnpm lint:inline-styles`. Use a class.
- **Frontend:** runes mode everywhere. Use `$state`, `$derived`, `$effect` — not `$:` / stores-everywhere.

---

## Landmines

- `playback/automix.rs` is a one-line stub. All real automix logic lives in `playback/player.rs`.
- `services/spotify_public/` and `services/sportify/` both exist and are different. Do not merge or rename them.
- `ListParams.favorite_only` means "library tracks" (favorited OR parent album favorited), not "only liked tracks". The strict version is `liked_only`.
- `MIGRATION_004` is declared after `MIGRATION_008` in the file — the `MIGRATIONS` slice order is what matters, not declaration order.
- `automix-new` queue source uses a hyphen; every other source label is underscored. Do not normalize without checking `source.starts_with("automix")` usages.
- `queue` table and `playback_state` are wiped on every server start. Do not persist runtime state there expecting it to survive.
- `routes.rs` is ~16 000+ lines. Use Grep, not Read offsets.
- `noor.db` in the repo root is the developer's actual library (~929 MB). Never commit it; never delete it without explicit say-so.
- `tracks.tidal_id` UNIQUE constraint is load-bearing for pending-row resolver race safety. Do not drop it.
- `pending_artist` / `pending_title` are kept after resolution as an audit trail. UIs must prefer `track.title` / `track.artist_name` once `resolved_at` is set.
- Static-file fallback auth uses `route_layer` (not `layer`) on purpose. Switching breaks static asset delivery.
- Tauri WebView2 reload hack at `noor-app/src/main.rs:56` is intentional. Do not remove without testing on a cold WebView2 install.

---

## Do not touch without asking

- Database migrations (`noor-server/src/db/schema.rs`)
- WASAPI exclusive path (`noor-server/src/playback/wasapi_exclusive.rs`)
- Audio runtime and gapless transition (`noor-server/src/playback/runtime.rs`, `playback/gapless.rs`)
- Camelot lookup tables (`noor-server/src/services/audio_analysis/key.rs`)
- Sidecar shutdown timing (`noor-app/src/sidecar.rs`)
- Server bind logic (`noor-server/src/main.rs`)
- Auth middleware ordering (`noor-server/src/server/mod.rs`)
- Boot-time queue wipe (`noor-server/src/main.rs`)
- Release flow (see above)

---

## Dead ends — do not reference

- `E:\noorwave-galaxy` — that repo and the Genre Galaxy reinvision experiment are abandoned. Do not suggest reviving, porting, or referencing any code or docs from it.
