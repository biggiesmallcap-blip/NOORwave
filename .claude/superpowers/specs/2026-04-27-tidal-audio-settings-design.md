# Tidal Audio Settings — Design Spec

## Context

User pointed at https://github.com/J-M-PUNK/tideway as a feature reference and
asked which capabilities to bring into NOORwave. After comparing tideway's
feature set against the existing Tidal stack, the user chose to start with
**audio quality and output settings** as the foundation slice — it unlocks
the audiophile differentiator (exclusive mode, bit-perfect) that tideway
explicitly does NOT do, and it builds the settings surface that future slices
(downloads, videos, scrobbling) will plug into.

Today the Tidal stream quality is hardcoded to `LOSSLESS` in
[noor-server/src/services/tidal/stream.rs:6](../../../noor-server/src/services/tidal/stream.rs#L6),
the CPAL output device is locked to the system default at process start
([noor-server/src/playback/runtime.rs:273-298](../../../noor-server/src/playback/runtime.rs#L273-L298)),
and there is no UI for either. WASAPI exclusive mode and
sample-rate-follows-source are not wired at all.

## Scope

**In scope (v1):**

- Quality tier selector — `LOW` / `HIGH` / `LOSSLESS` / `HI_RES_LOSSLESS`
- Output device picker — every CPAL output device + a "System default" entry
- WASAPI exclusive mode toggle — Windows only; hidden on macOS/Linux
- Sample-rate-follows-source toggle — reconfigure output to track's native rate
- Live apply: device / exclusive / sample-rate changes rebuild the CPAL stream
  in place from the current playhead with a brief silence (~200–500 ms).
  Quality changes apply on the next stream resolve.

**Explicitly out of scope:**

- ASIO host (deferred — Steinberg SDK + cpal feature flag complicates the
  build; revisit if a user has an ASIO-only DAC)
- MQA passthrough, Atmos, surround
- Parametric EQ (a separate slice)
- Per-track or per-album quality overrides
- Hi-res downloads, video playback, Last.fm — separate slices already inventoried

## User experience

### Settings page

Extend the existing `/settings` route
([frontend/src/routes/settings/+page.svelte](../../../frontend/src/routes/settings/+page.svelte))
with a new **"Audio"** section above the existing wallpaper/palette controls:

- **Quality** — dropdown. Default `LOSSLESS`. `HI_RES` (MQA) is intentionally
  omitted — Tidal is phasing it out and tideway agrees it's not worth wiring.
- **Output device** — dropdown populated from a new
  `GET /api/audio/devices` endpoint. Default selection: "System default".
- **Exclusive mode (Windows only)** — toggle. Default OFF. Hidden when
  `navigator.platform` does not look like Windows (server also enforces).
  When enabled, a one-line warning explains "no other app can use this device
  while NOORwave is playing".
- **Sample-rate follows source** — toggle. Default OFF. One-line caption:
  "Reconfigures the output device to each track's native rate (44.1/48/96/192).
  Recommended with exclusive mode."

Settings persist on change (no save button). When playback is active and the
user changes device / exclusive / sample-rate-follow, an inline toast says
"Output reconfiguring…" and clears when audio resumes.

### Apply semantics

| Setting                    | When it takes effect                                |
| -------------------------- | --------------------------------------------------- |
| Quality                    | Next track's stream resolution (per-track URL)      |
| Output device              | Live — pause decoder, rebuild CPAL stream, resume   |
| Exclusive mode             | Live — same path as device change                   |
| Sample-rate follows source | Live (next track if rate matches; rebuild if not)   |

## Architecture

### Storage

Use the existing `server_config` kv table
([noor-server/src/db/schema.rs:516-520](../../../noor-server/src/db/schema.rs#L516-L520)).
Four new keys:

- `audio.quality` — `LOW` | `HIGH` | `LOSSLESS` | `HI_RES_LOSSLESS`
- `audio.output_device` — `default` (sentinel) or a stable device id string
- `audio.exclusive_mode` — `"true"` | `"false"`
- `audio.sample_rate_follow` — `"true"` | `"false"`

No migration needed — the table already exists. A small wrapper
`audio_settings.rs` reads/writes these as a typed struct.

### Backend endpoints (new)

- `GET /api/audio/devices` — returns `[{ id, name, is_default, max_channels,
  supported_sample_rates }]` from `cpal::Host::output_devices()`.
- `GET /api/audio/settings` — returns the typed struct.
- `PUT /api/audio/settings` — writes one or more keys. On change to
  device/exclusive/sample-rate, sends a `DeviceSwap` command to the playback
  runtime (no-op if nothing is playing).

### Quality plumbing

Single injection point at
[noor-server/src/playback/player.rs:534](../../../noor-server/src/playback/player.rs#L534)
(`preferred_tidal_quality()`). Change the precedence to:
`user_pref.audio.quality` → `track.best_quality` → `DEFAULT_AUDIO_QUALITY`.
Removes the need to touch `stream.rs`. The hardcoded
`DEFAULT_AUDIO_QUALITY` stays as the final fallback for cases with neither.

### Playback runtime — new `DeviceSwap` command

The decoder is already decoupled from the CPAL output stream via the shared
`PlaybackBuffer` ([runtime.rs:1082](../../../noor-server/src/playback/runtime.rs#L1082)),
so a swap can be done without re-decoding. Add a new variant to
`PlaybackRuntimeCommand`
([runtime.rs:55-79](../../../noor-server/src/playback/runtime.rs#L55-L79)):

```rust
DeviceSwap {
    device: OutputDeviceSelection,    // Default | Named(String)
    exclusive: bool,                  // Windows: maps to WASAPI ShareMode
    sample_rate_follow: bool,         // controls subsequent rebuild policy
}
```

Handler logic in the engine:

1. Set `shared.paused = true` so the current stream stops draining the buffer.
2. Drop the existing `cpal::Stream`.
3. Resolve the requested device (`Host::output_devices()` → match by id, or
   `default_output_device()`).
4. Build a new `cpal::StreamConfig`. On Windows + exclusive: select WASAPI
   `ShareMode::Exclusive` and the device's preferred buffer size for low
   latency. Otherwise shared mode.
5. If `sample_rate_follow` is on **and** the current track's native sample rate
   is known, configure the stream at that rate; otherwise use the device's
   default.
6. Call `device.build_output_stream(...)` with the existing
   `PlaybackSharedState` / `PlaybackBuffer`.
7. Replace the engine's `Stream` field, `play()` it, set
   `shared.paused = false`.

If step 4 or 6 fails (device disappeared, exclusive mode rejected, sample
rate unsupported), surface a structured error back via the existing event
channel and revert to the previous device. The settings layer shows a toast
and reverts the toggle.

### Sample-rate-follow on track transitions

Today's `PrepareNext` / `CrossfadeStart` path
([runtime.rs:64-71](../../../noor-server/src/playback/runtime.rs#L64-L71))
handles same-rate gapless transitions. When `sample_rate_follow` is enabled:

- If next track's native rate matches the current stream's rate → existing
  gapless path (no change).
- If rates differ → end the current stream cleanly, rebuild via the same
  `DeviceSwap` machinery at the new rate, start the next track. Brief silence
  expected and acceptable (matches tideway / foobar2000 behavior).

The track's native sample rate is already exposed in `StreamInfo`
([stream.rs:30-41](../../../noor-server/src/services/tidal/stream.rs#L30-L41)),
so no new metadata extraction is needed.

### Platform conditioning

- `cfg!(target_os = "windows")` gates the WASAPI exclusive code path. On
  other OSes the toggle is hidden in the UI **and** the backend rejects the
  setting with a 400.
- `OutputDeviceSelection::Named(id)` that fails to resolve falls back to
  default with a warning event (so unplugging a USB DAC doesn't brick
  playback).

## Files touched (estimate)

- `noor-server/src/db/audio_settings.rs` (new) — kv wrapper
- `noor-server/src/server/routes.rs` — three new routes
- `noor-server/src/playback/runtime.rs` — `DeviceSwap` command + handler
- `noor-server/src/playback/player.rs` — `preferred_tidal_quality()` precedence
- `frontend/src/routes/settings/+page.svelte` — Audio section
- `frontend/src/lib/api.ts` (or equivalent) — three new client methods
- `frontend/src/lib/stores/audio_settings.ts` (new) — settings store

## Verification

1. **Quality** — set quality to `HI_RES_LOSSLESS`, play a hi-res track,
   confirm Rust logs show `audioquality=HI_RES_LOSSLESS` in the
   `playbackinfopostpaywall` URL and `StreamInfo` reports 24-bit / 96+ kHz.
2. **Device picker** — `GET /api/audio/devices` returns plugged USB DAC.
   Pick it; audio routes to the DAC; unplug; engine reverts to default
   without panic.
3. **Exclusive mode (Windows)** — toggle on; another app (Spotify web)
   playing simultaneously is silenced or NOORwave fails predictably with a
   revert toast. Toggle off; both can coexist.
4. **Sample-rate-follow** — queue a 44.1 kHz track followed by a 96 kHz
   track. With the toggle off: continuous (resampled). With it on: brief
   silence between tracks; OS audio control panel shows the device at
   96 kHz during the second track.
5. **Live apply during playback** — start playback, switch device mid-track,
   confirm playhead resumes within ~500 ms on the new device with no
   decoded-frame loss.
6. **Persistence** — restart the server; settings survive; first track plays
   at the saved quality on the saved device.
7. **Non-Windows** — on macOS/Linux the exclusive toggle is absent and
   `PUT /api/audio/settings { exclusive_mode: true }` returns 400.

## Risks / open items

- WASAPI exclusive mode rejection on certain DACs is common (driver refuses
  the requested rate/format). The revert-with-toast flow MUST work cleanly
  or users will think the app is broken.
- Live device swap during playback is the most complex piece. If
  implementation reveals races between drop-stream and decoder, fall back
  to a 1-track restart (queue current track from current position) — same
  user-visible result, simpler code.
- Tideway uses native OS APIs and reports good results without exclusive
  mode. We are betting that the Windows audiophile audience cares enough
  about exclusive mode to justify the complexity. If telemetry later shows
  almost no one enables it, we can deprecate the toggle.

## Out-of-scope follow-ups (deferred slices)

For reference — these were on the original menu and the user can pick the
next one when this ships:

- Hi-res FLAC downloads (queue + tagging) — note: violates Tidal ToS
- Music video playback (HLS + hls.js + CORS proxy)
- Tauri shell + installer (unblocks tray, media keys)
- 10-band parametric EQ
- Last.fm scrobbling
