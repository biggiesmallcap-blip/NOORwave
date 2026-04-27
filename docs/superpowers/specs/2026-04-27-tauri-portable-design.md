# NOORwave Tauri Portable Shell — Design Spec

## Context

NOORwave currently runs as two separate processes: a Rust HTTP server (`noor-server`) and a
SvelteKit dev server. This spec wraps them into a double-click portable Windows application
with a system tray, global media keys, and a network-access toggle.

## Goals

- Double-click `NOORwave.exe` → window opens, music plays, no terminal needed
- Close window → hides to tray, playback continues
- Tray right-click → network access toggle, restart server, exit
- Global media keys work even when window is hidden
- Portable: ship as a zip, run from any folder, no installer, no admin rights

## Out of scope

- macOS / Linux (Windows-first; Tauri is cross-platform but this spec targets Windows)
- Auto-updater
- Code signing / SmartScreen suppression
- ASIO, MQA, or any audio changes

---

## Portable package layout

```
NOORwave/
├── NOORwave.exe          ← Tauri app (window, tray, shortcuts)
├── noor-server.exe       ← audio + API server (unchanged interface)
├── www/                  ← compiled SvelteKit static files
│   ├── index.html
│   └── _app/
└── noor.db               ← created on first run by noor-server
```

Running `NOORwave.exe` with no arguments launches everything.
Running `noor-server.exe` directly still works for headless / SSH use.

---

## Architecture

**Option chosen: Tauri sidecar + WebView → localhost**

- `NOORwave.exe` (Tauri) spawns `noor-server.exe` as a managed child process on startup.
- The Tauri WebView window opens `http://127.0.0.1:3334` — the existing SvelteKit app,
  unchanged. No SvelteKit adapter change required.
- noor-server serves both the REST API and the compiled SvelteKit static files from a
  `www/` directory located next to the exe.
- On Exit: Tauri kills the sidecar, then quits.
- On Restart: Tauri kills the sidecar and respawns it with updated flags.

---

## noor-server changes

### 1. Static file serving

Add a `GET /*` fallback route using `tower-http::ServeDir` that serves files from a `www/`
directory resolved relative to the running binary. If `www/` does not exist, the route is
skipped (so headless use without the frontend folder still works).

Priority: API routes first (`/api/*`, `/ws`), then static files, then `index.html` fallback
for client-side routing.

### 2. `--host` flag

Add a `--host` boolean CLI flag (via `std::env::args()`). When present, bind to
`0.0.0.0:3334` instead of `127.0.0.1:3334`.

The current host-mode preference is stored in the existing `server_config` kv table under
key `server.host_mode` (`"true"` / `"false"`). noor-server reads this on startup and uses
it as the default when no CLI flag is passed. The Tauri app always passes the flag
explicitly (either `--host` or nothing) so the DB value is the source of truth for the
toggle state.

### 3. New API endpoint

`GET /api/server/info` returns:

```json
{
  "host_mode": false,
  "bind_address": "127.0.0.1:3334",
  "version": "0.1.0"
}
```

Used by the Tauri tray to show the current bind address in a tooltip and reflect toggle
state on launch.

---

## Tauri app (`noor-app/`)

New Cargo workspace member at `noor-app/src-tauri/`.

### Sidecar management

- Registered in `tauri.conf.json` under `bundle.externalBin` as `noor-server`.
- On `setup`: resolve the correct `--host` flag from the DB (via `GET /api/server/info`
  after spawn), then spawn the sidecar.
- Sidecar stdout/stderr is piped and logged to a `noor-server.log` file next to the exe
  (useful for debugging without a terminal).
- On `Exit` tray action: send SIGTERM / `TerminateProcess` to the sidecar, then call
  `app.exit(0)`.

### Window behaviour

- Single window: `label = "main"`, opens `http://127.0.0.1:3334`.
- `on_window_event(WindowEvent::CloseRequested)` → `window.hide()`, event consumed
  (window does not close, process does not quit).
- Tray left-click or "Show NOORwave" menu item → `window.show()` + `window.set_focus()`.

### Tray menu

```
NOORwave
─────────────────
Show NOORwave
─────────────────
☐ Network access        [checkbox, persists in DB]
Restart server
─────────────────
Exit
```

- **Network access** toggle: reads current state from `GET /api/server/info` on app
  start. When toggled: write new preference to DB (`PUT /api/server/host_mode`), kill
  sidecar, respawn with or without `--host`. Tray icon turns grey during the ~1 s restart.
- **Restart server**: kill sidecar + respawn with same flags. Tray icon grey during restart.
- **Exit**: kill sidecar, `app.exit(0)`.

New backend endpoint for the toggle:

`PUT /api/server/host_mode` body `{ "host_mode": true }` — writes to `server_config` DB,
returns updated `server.info` shape. The actual rebind happens when Tauri restarts the
sidecar.

### Global media keys

Uses `tauri-plugin-global-shortcut`. Registered on app setup:

| Key             | Action                                 |
|-----------------|----------------------------------------|
| `MediaPlayPause`| `POST http://127.0.0.1:3334/api/playback/pause` or `/resume` depending on state |
| `MediaNextTrack`| `POST http://127.0.0.1:3334/api/playback/next` |
| `MediaPreviousTrack` | `POST http://127.0.0.1:3334/api/playback/previous` |

When `MediaPlayPause` fires, the handler calls `GET /api/playback/state` inline to check
`is_playing`, then calls `/api/playback/pause` or `/api/playback/resume` accordingly.
If the server is unavailable (restarting), the HTTP call fails and the key press is
silently dropped.

---

## Build script

`scripts/build-portable.ps1` (PowerShell, runs on Windows):

```powershell
# 1. Build frontend
Push-Location frontend
npm ci
npm run build          # outputs to frontend/build/
Pop-Location

# 2. Build noor-server
cargo build --release -p noor-server

# 3. Build Tauri app
cargo build --release -p noor-app

# 4. Assemble portable folder
$dist = "dist/NOORwave"
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dist | Out-Null
Copy-Item target/release/NOORwave.exe $dist/
Copy-Item target/release/noor-server.exe $dist/
Copy-Item -Recurse frontend/build/ $dist/www/

# 5. Zip
Compress-Archive -Path $dist -DestinationPath dist/NOORwave-portable.zip -Force

Write-Host "Built: dist/NOORwave-portable.zip"
```

---

## Files touched

| File | Change |
|------|--------|
| `noor-app/src-tauri/Cargo.toml` | New — Tauri crate |
| `noor-app/src-tauri/src/main.rs` | New — sidecar, tray, shortcuts |
| `noor-app/src-tauri/tauri.conf.json` | New — Tauri config |
| `noor-app/src-tauri/icons/` | New — app icon (32×32 … 512×512 PNGs) |
| `Cargo.toml` | Add `noor-app` workspace member |
| `noor-server/src/main.rs` | Add `--host` flag + bind address logic |
| `noor-server/src/server/routes.rs` | Add static file serving + `/api/server/info` + `PUT /api/server/host_mode` |
| `scripts/build-portable.ps1` | New — release build script |

---

## Verification

1. `cargo build --release` compiles both binaries without errors.
2. Double-clicking `NOORwave.exe` from the portable folder: server starts, window opens,
   music plays.
3. Closing the window: tray icon remains, playback continues.
4. Tray → Network access: server restarts, binding changes (verify with `netstat -an | findstr 3334`).
5. Media keys: play/pause/skip work while window is hidden.
6. Tray → Exit: server process gone (`tasklist | findstr noor-server` returns nothing).
7. Move the folder to `C:\Temp\NOORwave\` and repeat — no path-dependency issues.
8. `noor-server.exe --host` directly in a terminal: binds to `0.0.0.0:3334`.
