# NOORwave

<p align="center">
  <img width="1280" height="640" alt="NOORwave product card" src="frontend/static/social/source-animated.svg" />
</p>

<p align="center">
  <strong>A local-first hi-fi command center for your TIDAL library.</strong>
</p>

<p align="center">
  Lossless playback &middot; library intelligence &middot; DJ and radio tools &middot; desktop plus phone remote
</p>

<p align="center">
  <a href="../../releases/latest">Download latest release</a> &middot;
  <a href="#run-from-source">Run from source</a> &middot;
  <a href="#release-checklist">Release checklist</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024-orange?style=flat-square&logo=rust" alt="Rust"/>
  <img src="https://img.shields.io/badge/SvelteKit-5-ff3e00?style=flat-square&logo=svelte" alt="SvelteKit"/>
  <img src="https://img.shields.io/badge/Tauri-2-ffc131?style=flat-square&logo=tauri" alt="Tauri"/>
  <img src="https://img.shields.io/badge/SQLite-3-003b57?style=flat-square&logo=sqlite" alt="SQLite"/>
  <img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="MIT"/>
</p>

## What It Is

NOORwave is a desktop music player for people who want more control than a normal streaming app gives them. It syncs your TIDAL library into local SQLite, runs a Rust playback server on your machine, and serves a SvelteKit interface through a Tauri desktop shell.

It is built for fast library work, serious playback, and discovery that learns from your own listening. The app stays local by default. TIDAL, Last.fm, MusicBrainz, Spotify metadata, and optional services enrich the library without turning NOORwave into a cloud account.

## Product Tour

<table>
  <tr>
    <td width="50%">
      <a href="docs/assets/screenshot-home.png"><img src="docs/assets/screenshot-home.png" alt="Home screen" width="100%"/></a>
      <br/><sub><strong>Home</strong> - daily listening, recently played, new releases, now playing.</sub>
    </td>
    <td width="50%">
      <a href="docs/assets/screenshot-search.png"><img src="docs/assets/screenshot-search.png" alt="Search screen" width="100%"/></a>
      <br/><sub><strong>Search</strong> - local library, TIDAL catalog, filters, queue actions.</sub>
    </td>
  </tr>
  <tr>
    <td width="50%">
      <a href="docs/assets/screenshot-discover.png"><img src="docs/assets/screenshot-discover.png" alt="Discover screen" width="100%"/></a>
      <br/><sub><strong>Discover</strong> - learned similarity, blend discovery, song radio.</sub>
    </td>
    <td width="50%">
      <a href="docs/assets/screenshot-automix.png"><img src="docs/assets/screenshot-automix.png" alt="Automix screen" width="100%"/></a>
      <br/><sub><strong>DJ tools</strong> - automix, harmonic matching, queue control.</sub>
    </td>
  </tr>
</table>

## Core Capabilities

| Area | What NOORwave does |
|---|---|
| Library | Incremental TIDAL sync, local search, artist and album pages, playlists, duplicate detection, MusicBrainz and Last.fm enrichment. |
| Playback | Lossless TIDAL playback, DASH segment seek, gapless transitions, crossfade, queue undo, shuffle modes, Windows WASAPI exclusive output. |
| DJ and radio | Automix, Song Radio, Last.fm fallback, learned similarity, harmonic scoring, DJ profile controls, reasons on queued tracks. |
| Discovery | Sound Space, blend discovery, training intensity tiers, safety preview, live ETA, model activation guidance. |
| Desktop app | Tauri shell, tray controls, media keys, auto-updater, portable builds, Windows per-user installer. |
| Phone remote | Installable `/remote` PWA for transport, queue, search, library browsing, action sheets, sleep timer. |
| Video | TIDAL video search and in-app HLS playback with quality selector and autoplay. |

## Why It Exists

Most streaming apps hide the interesting parts of a music library. NOORwave makes them visible:

- Search by genre, year, BPM, key, energy, artist, album, playlist, or natural intent.
- Build playback from taste signals instead of a generic recommendation feed.
- Keep the queue inspectable, editable, and recoverable.
- Use local metadata and listening history to make radio better over time.
- Keep portable builds available even as the installed updater matures.

## Download

The latest public build is on [GitHub Releases](../../releases/latest).

| Platform | Artifact | Notes |
|---|---|---|
| Windows | `NOORwave-vX.Y.Z-windows-x64-setup.exe` | Per-user installer. Installs to `%LOCALAPPDATA%\Programs\NOORwave`. Updates in place from signed Tauri updater metadata. |
| Windows | `NOORwave-vX.Y.Z-windows-x64.zip` | Portable fallback. Unzip anywhere and run `NOORwave.exe`. |
| macOS ARM64 | `NOORwave-vX.Y.Z-macos-arm64.tar.gz` | Portable build. Gatekeeper may require `xattr -cr NOORwave noor-server`. |
| macOS x64 | `NOORwave-vX.Y.Z-macos-x64.tar.gz` | Portable build. |
| Linux x64 | `NOORwave-vX.Y.Z-linux-x64.tar.gz` | Portable build. |

Windows builds are not CA-signed today. SmartScreen or Smart App Control can warn or block the first launch on strict systems. The installed updater payload is still signed with the project's Tauri updater key.

## Run From Source

Prerequisites:

- Rust stable
- Node 24
- pnpm 10
- A TIDAL account

```powershell
# Backend
cargo run -p noor-server

# Frontend dev server
cd frontend
pnpm install
pnpm dev
```

Open `http://127.0.0.1:3334` for the bundled app or the Vite URL printed by `pnpm dev` during frontend work.

For the desktop shell:

```powershell
cargo run -p noor-app
```

## Build And Verify

```powershell
# Rust
cargo check -p noor-server
cargo check -p noor-app
cargo test --workspace --locked

# Frontend
cd frontend
pnpm check
pnpm test
pnpm run build
```

Windows portable package:

```powershell
cd frontend
pnpm run build
cd ..
.\scripts\build-portable.ps1
```

## Architecture

```text
noor-app       Tauri 2 desktop shell, tray, media keys, updater
noor-server    Rust Axum server, SQLite, playback, integrations, WebSocket events
frontend       SvelteKit 2 and Svelte 5 UI, static build served by noor-server
docs           Specs, release notes, inventories, design memory
scripts        Build, smoke, probe, and data utilities
```

The Tauri shell starts `noor-server` as a sidecar. The same server also serves the LAN phone remote at `/remote`, which is why the backend stays a real HTTP server instead of being collapsed into Tauri IPC.

## Current Status

NOORwave is usable but still moving quickly. The `0.2.0` line is focused on integrating the active product branches into one coherent build, tightening the DJ and Last.fm flows, and making the release path less fragile.

Known constraints:

- Genre Galaxy is live but still needs interaction and rendering polish.
- ACRCloud fingerprint recognition is present as a placeholder, not a finished integration.
- Linux and macOS builds are portable-only today.
- Windows exclusive-mode audio is the most tuned output path.
- This is a single-user local app, not a hosted multi-user service.

## Release Checklist

Tags drive releases. Before tagging `vX.Y.Z`:

1. Bump only `noor-server/Cargo.toml`, `noor-app/Cargo.toml`, `noor-app/tauri.conf.json`, and the matching `noor-app` / `noor-server` entries in `Cargo.lock`.
2. Do not run bare `cargo update`.
3. Keep both Windows artifacts: portable zip and NSIS setup exe.
4. Keep `installMode: "currentUser"` in the NSIS config.
5. Keep the Windows SmartScreen and Smart App Control note in release copy.
6. Read `.github/workflows/release.yml` before changing release behavior.
7. After CI publishes, prepend the human "What's new" section with `gh release edit vX.Y.Z --notes-file <combined.md>`.

Installed Windows release-ready means a signed local `cargo tauri build --bundles nsis` has been tested, the `.sig` exists, and mutable data still lives under `%LOCALAPPDATA%\NOORwave`.

## Contributing

Use small, focused changes. Keep Rust, Svelte, and TypeScript as the first-class languages. Do not commit local databases, generated build output, secrets, private signing keys, or machine-local config.

Useful checks:

```powershell
git status --short
cargo fmt --all -- --check
cd frontend
pnpm check
pnpm test
pnpm run build
```

## Disclaimer

NOORwave uses TIDAL's unofficial API through PKCE OAuth2. It is not affiliated with, endorsed by, or associated with TIDAL Music AS or MQA Ltd. Use it at your own discretion and risk. Credentials are stored locally and encrypted in SQLite. NOORwave is intended for personal use only.

## License

[MIT](LICENSE)
