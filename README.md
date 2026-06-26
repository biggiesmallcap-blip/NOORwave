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
  <a href="../../releases/latest">Download</a> &middot;
  <a href="#run-from-source">Run from source</a> &middot;
  <a href="#configuration">Configuration</a> &middot;
  <a href="#phone-remote">Phone remote</a> &middot;
  <a href="#release-checklist">Release checklist</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024-orange?style=flat-square&logo=rust" alt="Rust"/>
  <img src="https://img.shields.io/badge/Svelte-5-ff3e00?style=flat-square&logo=svelte" alt="Svelte 5"/>
  <img src="https://img.shields.io/badge/Tauri-2-ffc131?style=flat-square&logo=tauri" alt="Tauri"/>
  <img src="https://img.shields.io/badge/SQLite-3-003b57?style=flat-square&logo=sqlite" alt="SQLite"/>
  <img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="MIT"/>
</p>

## What It Is

NOORwave is a desktop TIDAL player built around a simple idea: your streaming library should behave like a fast local music collection.

It syncs your TIDAL library into SQLite, gives you a real desktop app with tray and media-key support, adds lossless playback controls, lets you shape the queue with search, radio, automix, DJ profiles, and a phone remote, and can save tracks to disk as FLAC or MP3. Everything runs on your own machine. A small Rust server (`noor-server`) owns the database, audio engine, and integrations; a Tauri desktop shell (`noor-app`) wraps the SvelteKit UI and launches that server for you.

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
| Downloads | Save any track, album, or playlist to disk as bit-perfect FLAC or 320 kbps MP3, tagged, into a configurable `Artist/Album/NN - Title` library. Selectable FLAC tier (Hi-Res or CD) and MP3 source, with batch progress, cancel, and retry. |
| Playback | Lossless TIDAL playback, DASH segment seek, gapless transitions, crossfade, queue undo, shuffle modes, Windows WASAPI exclusive output. |
| DJ and radio | Automix, Song Radio, Last.fm fallback, learned similarity, harmonic scoring, DJ profile controls, reasons on queued tracks. |
| Discovery | Sound Space, blend discovery, training intensity tiers, safety preview, live ETA, model activation guidance. |
| Desktop app | Tauri shell, tray controls, media keys, auto-updater, portable builds, Windows per-user installer. |
| Phone remote | Installable `/remote` PWA for transport, queue, search, library browsing, action sheets, sleep timer. |
| Video | TIDAL video search and in-app HLS playback with quality selector and autoplay. |

## Why NOORwave

NOORwave is for people who like TIDAL's catalog but want a better command center around it.

- **TIDAL feels local.** Your saved tracks, albums, artists, playlists, and metadata live in a local database, so search and browsing do not feel trapped behind a remote app shell.
- **Playback is treated as the product.** Lossless streaming, DASH seek behavior, gapless transitions, crossfade, media keys, tray controls, and Windows WASAPI exclusive mode are built into the core player.
- **The queue is yours.** Play next, append, reorder, save, undo clear, shuffle by intent, inspect radio reasons, and keep automix from fighting your manual choices.
- **DJ mode is a control surface, not a gimmick.** Profiles, harmonic matching, BPM and energy awareness, transition planning, and automix controls are there for shaping a session, not just filling silence.
- **The phone remote makes it practical.** Open `/remote` from a phone on the same network and drive playback, queue, search, library pages, sleep timer, and track actions from the couch.
- **Your music lands on disk too.** Right-click any track, album, or playlist (or use the download button on the now-playing art) to save it as bit-perfect FLAC or a portable 320 kbps MP3, tagged, in a tidy `Artist/Album` folder you pick once in Settings.

The point is not every integration NOORwave can talk to. The point is a faster, more controllable TIDAL setup for people who actively listen, build queues, and care how playback moves from one track to the next.

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
- A TIDAL account (you sign in from inside the app)

The fastest path is the dev launcher, which starts the backend and the frontend dev server together (Windows Terminal split panes if available):

```powershell
.\scripts\dev.ps1
```

Or run the two processes yourself:

```powershell
# Backend (Axum server, SQLite, audio engine)
cargo run -p noor-server

# Frontend dev server (separate terminal)
cd frontend
pnpm install
pnpm dev
```

Where to open it:

- **Frontend dev server:** `http://127.0.0.1:17601` (hot reload, talks to the backend on 17600).
- **Backend-served UI:** `http://127.0.0.1:17600` (the production build that `noor-server` serves directly).

On first run the server prints an access PIN in a startup banner ("NOOR access token: ..."). On `127.0.0.1` the UI fetches that PIN for you, so you do not need to type it. You only need it on a [phone or other device](#phone-remote). You can always read it again in Settings -> Access PIN.

For the full desktop shell (it launches `noor-server` for you, adds the tray, media keys, and updater):

```powershell
cargo run -p noor-app
```

## Configuration

### Ports

| Port | What | Default | Override |
|---|---|---|---|
| Backend | `noor-server` HTTP + WebSocket, and the `/remote` PWA | `17600` | `NOOR_PORT` |
| Dev server | Vite frontend during `pnpm dev` | `17601` | `NOOR_DEV_PORT` |

Two things worth knowing:

- The backend port is **baked into the frontend at build time**. If you change `NOOR_PORT`, rebuild the frontend (`pnpm run build`) so the UI points at the right port.
- The dev-server port is the only origin the backend trusts for CORS in dev. Keep `NOOR_DEV_PORT` in sync on both sides; it uses `strictPort`, so a busy port fails loudly instead of silently drifting to an untrusted one.

### Bind address

By default the server listens on `127.0.0.1` (loopback only), so nothing outside your machine can reach it. To expose it on your LAN (needed for the [phone remote](#phone-remote)), pick one:

- Desktop app: tray icon -> **Network access** (restarts the server bound to `0.0.0.0`).
- Standalone server: pass `--host`, which forces `0.0.0.0`.
- Either: set `NOOR_ADDR=0.0.0.0:17600`.

Precedence, highest first: `NOOR_ADDR` > `--host` > the saved Network-access / `host_mode` setting > `127.0.0.1`.

### Environment variables

All optional. The app ships with working defaults (including built-in TIDAL credentials).

| Variable | Purpose |
|---|---|
| `NOOR_PORT` | Backend listen port (default `17600`). Rebuild the frontend after changing. |
| `NOOR_DEV_PORT` | Vite dev-server port (default `17601`). Trusted CORS origin in dev. |
| `NOOR_ADDR` | Full bind address `host:port`. Overrides `NOOR_PORT` and `--host`. Use `0.0.0.0:17600` for LAN. |
| `NOOR_DB` | Path to the SQLite database file. |
| `NOOR_DATA_DIR` | Base data directory (database, token) for installed builds. |
| `NOOR_WWW_DIR` | Directory of the built frontend to serve. |
| `LASTFM_API_KEY` / `LASTFM_API_SECRET` | Last.fm enrichment; the secret also enables scrobbling. |
| `TIDAL_CLIENT_ID` / `TIDAL_CLIENT_SECRET` | Override the built-in TIDAL app credentials. |
| `TIDAL_PKCE_CLIENT_ID` / `TIDAL_PKCE_CLIENT_SECRET` | Override the TIDAL PKCE login credentials. |
| `DISCOGS_TOKEN` / `DISCOGS_USER_AGENT` | Discogs label and release metadata. |
| `SPORTIFY_API_BASE_URL` | Override the Sportify metadata proxy base URL. |

A handful of cache-tuning knobs (`DISCOVERY_CACHE_TTL_DAYS`, `RESOLVE_CACHE_TTL_DAYS`, `RESOLVE_RETRY_AFTER_DAYS`, `RESOLVE_EAGER_N`, `RESOLVE_BULK_CONCURRENCY`) exist for advanced tuning; defaults are fine for normal use.

## Phone Remote

The remote is a small web app at `/remote`, served by `noor-server` on the same port as everything else (no extra service, no extra port). It controls transport, queue, search, and library browsing from a phone on the same network.

Setup, once per device:

1. **Make sure the server is reachable on your LAN.** Desktop app: tray icon -> **Network access**. Standalone server: run with `--host`. (See [Bind address](#bind-address).)
2. **Find the desktop's LAN IP**, for example `192.168.1.42`. On Windows: `ipconfig`.
3. **On the phone (same Wi-Fi), open** `http://<desktop-LAN-IP>:17600/remote`, for example `http://192.168.1.42:17600/remote`.
4. **Enter the access PIN** when prompted. Get the 6-digit PIN from the desktop in Settings -> Access PIN. The phone caches it, so you only enter it once.
5. Optional: use your browser's "Add to Home Screen" to install it as a standalone PWA.

Notes:

- Windows may show a firewall prompt the first time the server binds to the LAN. Allow it on private networks.
- Regenerating the PIN (Settings -> Access PIN) disconnects every device; they each have to re-enter the new PIN.
- There is no QR pairing yet. You type the URL and the PIN by hand.

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
noor-app       Tauri 2 desktop shell, tray, media keys, updater, sidecar manager
noor-server    Rust Axum server, SQLite, playback, integrations, WebSocket events
frontend       SvelteKit 2 + Svelte 5 UI, static build served by noor-server
docs           Specs, release notes, inventories, design memory
scripts        Build, dev launcher, smoke, probe, and data utilities
```

How the pieces connect:

- **Sidecar model.** The Tauri shell spawns `noor-server` as a child process, waits for `GET /api/ping`, then opens the WebView. Shutdown goes through `POST /api/shutdown` before a force kill.
- **One server, two front doors.** The same `noor-server` serves the desktop UI and the LAN `/remote` PWA. That is why the backend stays a real HTTP server instead of collapsing into Tauri IPC.
- **Auth.** A single shared bearer token (the access PIN) gates every protected route: `Authorization: Bearer <pin>` for `/api/*`, and `?token=<pin>` for the `/ws` WebSocket (browsers cannot set headers on WebSocket upgrades). On loopback the UI fetches the PIN automatically; other devices enter it once.
- **Storage.** Everything lives in one local SQLite file (`noor.db`), next to the executable by default or wherever `NOOR_DB` / `NOOR_DATA_DIR` point.

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
