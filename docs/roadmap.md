# NOORwave Roadmap

## Distribution

### Installer build + auto-updates
Ship an NSIS installer alongside the portable `.exe` and wire up Tauri's updater plugin so users on the installer track get in-app updates.

- Flip `bundle.active` to `true` in `noor-app/tauri.conf.json` and add `"targets": ["nsis", "msi"]`.
- Add `tauri-plugin-updater`; generate an updater keypair via `tauri signer generate` (separate from any future code-signing cert).
- Host `latest.json` + installers on GitHub Releases; updater verifies signature against the bundled pubkey.
- Keep the portable build as a parallel artifact for users who don't want auto-update.
- **No code-signing cert required** — works unsigned, but each new version triggers SmartScreen on first run until reputation builds. A ~$200–400/yr OV cert (Certum/SSL.com) would remove that friction if it ever matters.

## TIDAL rate-limiting

### Per-bucket backoff
The current `TidalBackoff` is global: a 429 on `/search` blocks `/albums`, `/playlists`, streaming, everything. Should split into buckets (search / catalog / streaming / mutations) so a hot endpoint doesn't lock the whole client.
- Implemented for now: `Retry-After` header is honored, default 429 backoff dropped from 60s → 10s, plus an in-process semaphore caps catalog requests at 4 concurrent ([noor-server/src/services/tidal/backoff.rs](noor-server/src/services/tidal/backoff.rs), [client.rs](noor-server/src/services/tidal/client.rs)).
- Defer until 429s persist after the above. If they do, key the backoff map by URL prefix and only `check()` the matching bucket.
