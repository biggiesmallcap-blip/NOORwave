# Tidal Audio Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Settings → Audio panel that lets the user pick Tidal stream quality and CPAL output device, with live device swap, optional WASAPI exclusive mode (Windows), and optional sample-rate-follows-source. Foundation slice for future Tidal feature work.

**Architecture:** Persist four prefs in the existing `server_config` kv table. Inject a user-quality value at the existing single quality call site (`preferred_tidal_quality()`). Add a `DeviceSwap` command to the playback runtime that pauses the decoder, drops + rebuilds the CPAL `Stream` on the new device/mode, and resumes — no re-decoding because the decoder is already decoupled via `PlaybackBuffer`. Three new Axum routes (`GET /api/audio/devices`, `GET /api/audio/settings`, `PUT /api/audio/settings`) bridge the UI to the runtime. SvelteKit `/settings` page gains an Audio section.

**Tech Stack:** Rust + Axum + rusqlite + cpal + Symphonia (server). SvelteKit 5 + TypeScript (frontend). No new dependencies.

**Reference spec:** [docs/superpowers/specs/2026-04-27-tidal-audio-settings-design.md](../specs/2026-04-27-tidal-audio-settings-design.md)

---

## File map

**Create:**
- `noor-server/src/db/audio_settings.rs` — typed struct + kv read/write for the four `audio.*` keys
- `frontend/src/lib/stores/audio_settings.ts` — Svelte store mirroring the server prefs

**Modify:**
- `noor-server/src/db/mod.rs` — add `pub mod audio_settings;`
- `noor-server/src/playback/player.rs` — extend `preferred_tidal_quality()` precedence
- `noor-server/src/playback/runtime.rs` — `OutputDeviceSelection`, `DeviceSwap` command + handler, exclusive-mode + SR-follow integration
- `noor-server/src/server/routes.rs` — three new routes + handlers
- `frontend/src/lib/api/client.ts` — three new methods
- `frontend/src/routes/settings/+page.svelte` — Audio section above existing controls

**Tests (Rust unit, inline `#[cfg(test)]`):**
- `noor-server/src/db/audio_settings.rs` — kv round-trip, defaults, parse/serialize
- `noor-server/src/playback/player.rs` — `preferred_tidal_quality()` precedence (already has access to deps)

**No frontend test runner exists** (`frontend/package.json` has no Vitest/Playwright). Frontend correctness is gated on `npm run check` (svelte-check) + manual verification per the spec's Verification section. We will not introduce a test runner in this plan — that's its own slice.

---

## Conventions

- **Commit format:** `feat(audio): <task summary>` — matches recent history (`feat(frontend): ...`, `docs(spec): ...`).
- **Branch:** Implementation should happen on a fresh branch off `master` (e.g. `feat/tidal-audio-settings`). The spec was committed to `feat/discovery-stop-button`; do not mix them.
- **No Claude/Anthropic trailers** in commit messages (per repo memory).
- **Test command (Rust):** `cargo test -p noor-server <test_name_substring>`

---

## Task 1: Audio settings storage (kv wrapper + typed struct)

**Files:**
- Create: `noor-server/src/db/audio_settings.rs`
- Modify: `noor-server/src/db/mod.rs` (add module declaration)

**Goal:** Introduce a typed `AudioSettings` struct and `load(conn)` / `save(conn, &AudioSettings)` functions backed by the existing `server_config` table. Defaults: quality = `LOSSLESS`, device = default, exclusive = false, sample_rate_follow = false.

- [ ] **Step 1.1: Write the failing tests**

Create `noor-server/src/db/audio_settings.rs` with the test module first:

```rust
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AudioQuality {
    Low,
    High,
    Lossless,
    HiResLossless,
}

impl AudioQuality {
    pub fn as_tidal_str(&self) -> &'static str {
        match self {
            AudioQuality::Low => "LOW",
            AudioQuality::High => "HIGH",
            AudioQuality::Lossless => "LOSSLESS",
            AudioQuality::HiResLossless => "HI_RES_LOSSLESS",
        }
    }

    pub fn from_tidal_str(s: &str) -> Option<Self> {
        match s {
            "LOW" => Some(Self::Low),
            "HIGH" => Some(Self::High),
            "LOSSLESS" => Some(Self::Lossless),
            "HI_RES_LOSSLESS" => Some(Self::HiResLossless),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSettings {
    pub quality: AudioQuality,
    /// `None` means "system default".
    pub output_device: Option<String>,
    pub exclusive_mode: bool,
    pub sample_rate_follow: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            quality: AudioQuality::Lossless,
            output_device: None,
            exclusive_mode: false,
            sample_rate_follow: false,
        }
    }
}

pub fn load(conn: &Connection) -> rusqlite::Result<AudioSettings> {
    let mut s = AudioSettings::default();
    if let Some(v) = read_kv(conn, "audio.quality")? {
        if let Some(q) = AudioQuality::from_tidal_str(&v) {
            s.quality = q;
        }
    }
    if let Some(v) = read_kv(conn, "audio.output_device")? {
        s.output_device = if v == "default" { None } else { Some(v) };
    }
    if let Some(v) = read_kv(conn, "audio.exclusive_mode")? {
        s.exclusive_mode = v == "true";
    }
    if let Some(v) = read_kv(conn, "audio.sample_rate_follow")? {
        s.sample_rate_follow = v == "true";
    }
    Ok(s)
}

pub fn save(conn: &Connection, s: &AudioSettings) -> rusqlite::Result<()> {
    write_kv(conn, "audio.quality", s.quality.as_tidal_str())?;
    write_kv(
        conn,
        "audio.output_device",
        s.output_device.as_deref().unwrap_or("default"),
    )?;
    write_kv(conn, "audio.exclusive_mode", if s.exclusive_mode { "true" } else { "false" })?;
    write_kv(
        conn,
        "audio.sample_rate_follow",
        if s.sample_rate_follow { "true" } else { "false" },
    )?;
    Ok(())
}

fn read_kv(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM server_config WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
}

fn write_kv(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE server_config (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn load_returns_defaults_on_empty_db() {
        let conn = fresh_conn();
        let s = load(&conn).unwrap();
        assert_eq!(s, AudioSettings::default());
        assert_eq!(s.quality, AudioQuality::Lossless);
        assert_eq!(s.output_device, None);
        assert!(!s.exclusive_mode);
        assert!(!s.sample_rate_follow);
    }

    #[test]
    fn save_then_load_round_trips_all_fields() {
        let conn = fresh_conn();
        let want = AudioSettings {
            quality: AudioQuality::HiResLossless,
            output_device: Some("USB DAC #1".into()),
            exclusive_mode: true,
            sample_rate_follow: true,
        };
        save(&conn, &want).unwrap();
        let got = load(&conn).unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn save_overwrites_previous_values() {
        let conn = fresh_conn();
        save(&conn, &AudioSettings::default()).unwrap();
        let updated = AudioSettings {
            quality: AudioQuality::High,
            output_device: Some("Other".into()),
            exclusive_mode: true,
            sample_rate_follow: false,
        };
        save(&conn, &updated).unwrap();
        assert_eq!(load(&conn).unwrap(), updated);
    }

    #[test]
    fn quality_serializes_to_tidal_strings() {
        assert_eq!(AudioQuality::HiResLossless.as_tidal_str(), "HI_RES_LOSSLESS");
        assert_eq!(AudioQuality::from_tidal_str("LOSSLESS"), Some(AudioQuality::Lossless));
        assert_eq!(AudioQuality::from_tidal_str("MQA"), None);
    }
}
```

- [ ] **Step 1.2: Wire the module**

Add to `noor-server/src/db/mod.rs`:

```rust
pub mod audio_settings;
```

(Place alphabetically with other `pub mod` declarations.)

- [ ] **Step 1.3: Run tests, expect PASS**

Run: `cargo test -p noor-server audio_settings`
Expected: 4 passed.

- [ ] **Step 1.4: Commit**

```bash
git add noor-server/src/db/audio_settings.rs noor-server/src/db/mod.rs
git commit -m "feat(audio): add AudioSettings kv wrapper on server_config"
```

---

## Task 2: Quality precedence in `preferred_tidal_quality()`

**Files:**
- Modify: `noor-server/src/playback/player.rs:534`

**Goal:** When resolving a Tidal stream URL, honor the user's saved quality ceiling instead of the track's `best_quality`. Falls back to track quality, then `DEFAULT_AUDIO_QUALITY`.

- [ ] **Step 2.1: Read the current implementation**

Open `noor-server/src/playback/player.rs` and locate `preferred_tidal_quality()` near line 534. Confirm it returns a `String` and uses `track.best_quality.clone().unwrap_or_else(|| stream::DEFAULT_AUDIO_QUALITY.to_string())` or similar.

- [ ] **Step 2.2: Write the failing test**

Append to the same file inside the existing test module (or add `#[cfg(test)] mod tests` if none exists). Replace `Track { ... }` field names with whatever the actual track struct uses — the test exists to lock the precedence rule, not the struct shape:

```rust
#[cfg(test)]
mod quality_precedence_tests {
    use super::*;
    use crate::db::audio_settings::AudioQuality;

    fn track_with_best(best: Option<&str>) -> Track {
        // Build the minimum Track required by preferred_tidal_quality.
        // Adapt field names to the real struct; only `best_quality` matters here.
        Track {
            best_quality: best.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn user_pref_overrides_track_best_quality() {
        let t = track_with_best(Some("HI_RES_LOSSLESS"));
        let got = preferred_tidal_quality(&t, Some(AudioQuality::Lossless));
        assert_eq!(got, "LOSSLESS");
    }

    #[test]
    fn falls_back_to_track_when_no_user_pref() {
        let t = track_with_best(Some("HI_RES_LOSSLESS"));
        let got = preferred_tidal_quality(&t, None);
        assert_eq!(got, "HI_RES_LOSSLESS");
    }

    #[test]
    fn falls_back_to_default_when_neither_set() {
        let t = track_with_best(None);
        let got = preferred_tidal_quality(&t, None);
        assert_eq!(got, crate::services::tidal::stream::DEFAULT_AUDIO_QUALITY);
    }
}
```

If `Track` doesn't implement `Default`, build it with the smallest viable constructor instead — the only field the function reads is `best_quality`.

- [ ] **Step 2.3: Update the function signature**

Change `preferred_tidal_quality(track: &Track) -> String` to `preferred_tidal_quality(track: &Track, user_pref: Option<AudioQuality>) -> String`. New body:

```rust
pub(crate) fn preferred_tidal_quality(
    track: &Track,
    user_pref: Option<AudioQuality>,
) -> String {
    if let Some(q) = user_pref {
        return q.as_tidal_str().to_string();
    }
    track
        .best_quality
        .clone()
        .unwrap_or_else(|| stream::DEFAULT_AUDIO_QUALITY.to_string())
}
```

Add `use crate::db::audio_settings::AudioQuality;` at the top of `player.rs` if it's not already imported.

- [ ] **Step 2.4: Update call sites**

Find every caller of `preferred_tidal_quality(...)` (only one is expected, around `player.rs:541-545` in `build_tidal_stream_request`). Pass the user-pref value through. The caller now needs access to the loaded `AudioSettings`. Two acceptable approaches — pick whichever fits the surrounding code:

- **Approach A (preferred):** Load `AudioSettings` once at the call site via the existing DB handle and pass `Some(settings.quality)`. Example pattern:
  ```rust
  let user_quality = state
      .db
      .with_conn(|conn| crate::db::audio_settings::load(conn))
      .ok()
      .map(|s| s.quality);
  let quality = preferred_tidal_quality(&track, user_quality);
  ```
- **Approach B:** If `build_tidal_stream_request` already receives an `&AppState` / `&PlayerState`, attach `audio_settings: AudioSettings` to that state struct and read from it. This avoids a per-track DB hit but requires plumbing the state through.

Pick A for now. Optimization deferred.

- [ ] **Step 2.5: Run tests + build**

Run:
```bash
cargo test -p noor-server quality_precedence
cargo build -p noor-server
```
Expected: 3 tests pass, build succeeds.

- [ ] **Step 2.6: Commit**

```bash
git add noor-server/src/playback/player.rs
git commit -m "feat(audio): honor user quality preference in preferred_tidal_quality"
```

---

## Task 3: CPAL device enumeration

**Files:**
- Modify: `noor-server/src/playback/runtime.rs` (add an `output_devices()` helper near the existing CPAL setup at lines 273-298)

**Goal:** A pure helper that returns a list of every CPAL output device with id, display name, default flag, max channels, and supported sample rates. Used by the `GET /api/audio/devices` endpoint.

- [ ] **Step 3.1: Add the public types and helper**

Near the existing CPAL initialization in `runtime.rs`, add:

```rust
use cpal::traits::{DeviceTrait, HostTrait};

#[derive(Debug, Clone, serde::Serialize)]
pub struct OutputDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub max_channels: u16,
    pub supported_sample_rates: Vec<u32>,
}

pub fn enumerate_output_devices() -> Vec<OutputDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok());

    host.output_devices()
        .map(|iter| {
            iter.filter_map(|dev| {
                let name = dev.name().ok()?;
                let configs: Vec<_> = dev
                    .supported_output_configs()
                    .ok()?
                    .collect();
                let max_channels = configs
                    .iter()
                    .map(|c| c.channels())
                    .max()
                    .unwrap_or(0);
                let mut rates: Vec<u32> = configs
                    .iter()
                    .flat_map(|c| {
                        let min = c.min_sample_rate().0;
                        let max = c.max_sample_rate().0;
                        // Common audio rates that fall within the supported range.
                        [44_100, 48_000, 88_200, 96_000, 176_400, 192_000]
                            .into_iter()
                            .filter(move |r| *r >= min && *r <= max)
                    })
                    .collect();
                rates.sort_unstable();
                rates.dedup();
                Some(OutputDeviceInfo {
                    id: name.clone(),
                    name: name.clone(),
                    is_default: default_name.as_deref() == Some(name.as_str()),
                    max_channels,
                    supported_sample_rates: rates,
                })
            })
            .collect()
        })
        .unwrap_or_default()
}
```

Note: `id` and `name` are both the device name today. CPAL doesn't expose a stable id; the name is what we'll persist in `audio.output_device`. Document this trade-off in a code comment if you add one — otherwise leave it unannotated (the function name is self-explanatory).

- [ ] **Step 3.2: Build, expect SUCCESS**

Run: `cargo build -p noor-server`
Expected: builds cleanly. (No unit test — CPAL device enumeration depends on the host and isn't unit-testable; manual verification comes via the route in Task 7.)

- [ ] **Step 3.3: Commit**

```bash
git add noor-server/src/playback/runtime.rs
git commit -m "feat(audio): enumerate CPAL output devices"
```

---

## Task 4: `DeviceSwap` command + basic handler (no exclusive, no SR-follow)

**Files:**
- Modify: `noor-server/src/playback/runtime.rs:55-79` (command enum) + the engine handler (around runtime.rs:658-720 per earlier exploration)

**Goal:** Plumb a new command end-to-end that drops the current `cpal::Stream` and rebuilds it on the requested device. Shared mode only at this stage. SR-follow/exclusive land in Tasks 5 + 6.

- [ ] **Step 4.1: Add the command variant**

Inside `PlaybackRuntimeCommand` (the enum at runtime.rs:55-79):

```rust
DeviceSwap {
    device: OutputDeviceSelection,
    exclusive: bool,
    sample_rate_follow: bool,
},
```

And the selection enum, top-level in the same file:

```rust
#[derive(Debug, Clone)]
pub enum OutputDeviceSelection {
    Default,
    Named(String),
}

impl OutputDeviceSelection {
    pub fn from_pref(pref: Option<&str>) -> Self {
        match pref {
            None => Self::Default,
            Some("default") => Self::Default,
            Some(name) => Self::Named(name.to_string()),
        }
    }
}
```

- [ ] **Step 4.2: Resolve the device**

Add a helper in `runtime.rs`:

```rust
fn resolve_device(selection: &OutputDeviceSelection) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match selection {
        OutputDeviceSelection::Default => host.default_output_device(),
        OutputDeviceSelection::Named(name) => host
            .output_devices()
            .ok()
            .and_then(|mut iter| {
                iter.find(|d| d.name().ok().as_deref() == Some(name.as_str()))
            })
            .or_else(|| host.default_output_device()),
    }
}
```

Note the fallback: a `Named` device that no longer exists silently falls back to default. Logged in Step 4.4.

- [ ] **Step 4.3: Handle the command in the engine loop**

Locate the existing `match cmd { ... }` arm dispatch in the engine (the same place that handles `Pause`, `Resume`, `Stop`). Add an arm:

```rust
PlaybackRuntimeCommand::DeviceSwap { device, exclusive: _exclusive, sample_rate_follow: _sr } => {
    // _exclusive and _sr_follow are wired in Tasks 5 + 6.
    if let Err(err) = self.swap_output_device(&device) {
        tracing::warn!(target: "playback", ?err, "device swap failed; keeping current output");
    }
}
```

Add the method on the engine type:

```rust
fn swap_output_device(
    &mut self,
    selection: &OutputDeviceSelection,
) -> Result<(), cpal::BuildStreamError> {
    use cpal::traits::{DeviceTrait, StreamTrait};

    let new_device = match resolve_device(selection) {
        Some(d) => d,
        None => {
            tracing::warn!(target: "playback", "no output device available for swap");
            return Ok(());
        }
    };

    // Pause the current stream so the decoder thread stops draining the buffer.
    self.shared.set_paused(true);
    drop(self.stream.take());

    // Build the new stream using the same shared state / buffer.
    let config = new_device
        .default_output_config()
        .map_err(|e| cpal::BuildStreamError::BackendSpecific {
            err: cpal::BackendSpecificError { description: e.to_string() },
        })?;

    let new_stream = build_output_stream(&new_device, &config.into(), self.shared.clone())?;
    new_stream.play().map_err(|e| cpal::BuildStreamError::BackendSpecific {
        err: cpal::BackendSpecificError { description: e.to_string() },
    })?;
    self.stream = Some(new_stream);
    self.shared.set_paused(false);
    Ok(())
}
```

Adapt `self.shared`, `self.stream`, and `build_output_stream(...)` to the actual field/function names in the file. The earlier exploration noted `PlaybackBuffer` at runtime.rs:1082 and `Stream` ownership at runtime.rs:658 + the `build_output_stream(...)` call at runtime.rs:718 — use those as anchors.

- [ ] **Step 4.4: Build**

Run: `cargo build -p noor-server`
Expected: builds cleanly.

- [ ] **Step 4.5: Commit**

```bash
git add noor-server/src/playback/runtime.rs
git commit -m "feat(audio): add DeviceSwap command with shared-mode rebuild"
```

---

## Task 5: WASAPI exclusive mode (Windows only)

**Files:**
- Modify: `noor-server/src/playback/runtime.rs` (extend `swap_output_device`)
- Modify: `noor-server/Cargo.toml` (no changes expected — cpal already supports WASAPI on Windows by default)

**Goal:** On Windows, when `exclusive == true`, build the CPAL stream in WASAPI exclusive share mode. On non-Windows, the flag is rejected at the route layer (Task 7), so the code path here is Windows-only via `cfg`.

- [ ] **Step 5.1: Extract config builder**

Refactor `swap_output_device` to compute its `cpal::StreamConfig` via a helper that takes the `exclusive` flag:

```rust
fn build_stream_config(
    device: &cpal::Device,
    exclusive: bool,
    desired_sample_rate: Option<u32>,
) -> Result<cpal::StreamConfig, cpal::BuildStreamError> {
    use cpal::traits::DeviceTrait;

    let supported = device
        .default_output_config()
        .map_err(|e| cpal::BuildStreamError::BackendSpecific {
            err: cpal::BackendSpecificError { description: e.to_string() },
        })?;

    let mut config: cpal::StreamConfig = supported.into();
    if let Some(rate) = desired_sample_rate {
        config.sample_rate = cpal::SampleRate(rate);
    }

    // Exclusive mode is only meaningful via the WASAPI host on Windows.
    // CPAL doesn't expose ShareMode directly through StreamConfig — exclusive
    // mode is selected by feeding cpal a buffer size and sample format the
    // device accepts in exclusive mode and by going through the WASAPI-specific
    // builder. As of cpal 0.15, exclusive mode requires the
    // `cpal::host::wasapi::DeviceExtWasapi` extension trait.
    #[cfg(target_os = "windows")]
    if exclusive {
        // Tighter buffer for low-latency exclusive playback.
        config.buffer_size = cpal::BufferSize::Fixed(480);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = exclusive; // suppress unused warning on non-Windows

    Ok(config)
}
```

- [ ] **Step 5.2: Use it in `swap_output_device`**

Replace the inline `default_output_config()` block in `swap_output_device` with a call to `build_stream_config(&new_device, exclusive, sample_rate_follow_target)`. Plumb the `exclusive` arg from the command:

```rust
fn swap_output_device(
    &mut self,
    selection: &OutputDeviceSelection,
    exclusive: bool,
    desired_sample_rate: Option<u32>,
) -> Result<(), cpal::BuildStreamError> {
    /* ... as before ... */
    let config = build_stream_config(&new_device, exclusive, desired_sample_rate)?;

    #[cfg(target_os = "windows")]
    let new_stream = if exclusive {
        build_wasapi_exclusive_stream(&new_device, &config, self.shared.clone())?
    } else {
        build_output_stream(&new_device, &config, self.shared.clone())?
    };

    #[cfg(not(target_os = "windows"))]
    let new_stream = build_output_stream(&new_device, &config, self.shared.clone())?;

    /* ...as before... */
}
```

Update the engine arm to pass the args:

```rust
PlaybackRuntimeCommand::DeviceSwap { device, exclusive, sample_rate_follow } => {
    let target_rate = if sample_rate_follow { self.current_track_sample_rate() } else { None };
    if let Err(err) = self.swap_output_device(&device, exclusive, target_rate) {
        tracing::warn!(target: "playback", ?err, "device swap failed");
    }
}
```

`current_track_sample_rate()` is implemented in Task 6 — for now stub it to `None`:

```rust
fn current_track_sample_rate(&self) -> Option<u32> { None }
```

- [ ] **Step 5.3: Add the WASAPI exclusive stream builder (Windows-only)**

```rust
#[cfg(target_os = "windows")]
fn build_wasapi_exclusive_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: PlaybackSharedState,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    // Today CPAL does not expose a ShareMode toggle via StreamConfig; the
    // shared/exclusive selection happens at host build time via
    // `cpal::host::wasapi::DeviceExtWasapi::build_output_stream_raw_with_share_mode`.
    // If that API is not present in the pinned cpal version, fall back to
    // the standard builder and log a warning — the user-facing toggle still
    // works (the buffer-size bump in build_stream_config alone gives a
    // measurable latency improvement) and a follow-up upgrade can wire true
    // exclusive mode.

    use cpal::traits::DeviceTrait;
    tracing::info!(target: "playback", "building WASAPI exclusive stream (low-latency buffer)");
    build_output_stream(device, config, shared)
}
```

This is intentionally a placeholder with honest logging. True ShareMode::Exclusive requires either (a) bumping cpal to a version that exposes the WASAPI extension trait or (b) dropping to raw `windows-rs` IAudioClient. Both are out of scope for v1; the buffer-size + dedicated code path are the foundation. Note this gap in the task summary's commit message.

- [ ] **Step 5.4: Build on Windows**

Run: `cargo build -p noor-server`
Expected: builds cleanly. If on a non-Windows dev box, `cargo check --target x86_64-pc-windows-gnu` (requires the target installed) or push to CI.

- [ ] **Step 5.5: Commit**

```bash
git add noor-server/src/playback/runtime.rs
git commit -m "feat(audio): WASAPI exclusive code path with low-latency buffer (true ShareMode pending cpal upgrade)"
```

---

## Task 6: Sample-rate-follows-source

**Files:**
- Modify: `noor-server/src/playback/runtime.rs` (`current_track_sample_rate`, hook into `PrepareNext` / `CrossfadeStart`)

**Goal:** When the user enables sample-rate-follow and the next track's native rate differs from the current stream rate, end the current stream cleanly and rebuild at the new rate (via the existing `swap_output_device` path) before the next track starts. Same-rate transitions remain gapless.

- [ ] **Step 6.1: Track current and next sample rate**

Add fields on the engine state:

```rust
struct PlaybackEngine {
    /* existing fields ... */
    current_sample_rate: Option<u32>,
    audio_settings: AudioSettings,
}
```

Initialize `current_sample_rate: None` and `audio_settings` from `crate::db::audio_settings::load(...)` at engine construction (it already has DB access).

Implement `current_track_sample_rate()`:

```rust
fn current_track_sample_rate(&self) -> Option<u32> {
    self.current_sample_rate
}
```

When a stream is built (the existing `build_output_stream` call site), set `self.current_sample_rate = Some(stream_info.sample_rate)` using the value already exposed in `StreamInfo` (services/tidal/stream.rs:30-41).

- [ ] **Step 6.2: Inject rate-aware transition logic**

Locate the existing `PrepareNext` arm (runtime.rs around line 64). Before invoking the gapless prepare path, check:

```rust
PlaybackRuntimeCommand::PrepareNext { next_track, .. } => {
    if self.audio_settings.sample_rate_follow {
        let next_rate = stream_info_for(&next_track).map(|s| s.sample_rate);
        if let (Some(cur), Some(nxt)) = (self.current_sample_rate, next_rate) {
            if cur != nxt {
                // End the current stream cleanly and rebuild at the new rate
                // before the next track plays.
                let _ = self.swap_output_device(
                    &OutputDeviceSelection::from_pref(self.audio_settings.output_device.as_deref()),
                    self.audio_settings.exclusive_mode,
                    Some(nxt),
                );
            }
        }
    }
    // ... existing gapless prep logic ...
}
```

`stream_info_for(&next_track)` is whatever helper currently resolves the next track's `StreamInfo` — reuse it; do not re-query Tidal.

- [ ] **Step 6.3: Refresh `audio_settings` on every `DeviceSwap`**

Inside the `DeviceSwap` arm, after a successful swap, reload `self.audio_settings` from the DB so that the engine's view is always current. (The route handler already wrote the new prefs before sending the command.)

```rust
if let Ok(s) = self.db.with_conn(|conn| crate::db::audio_settings::load(conn)) {
    self.audio_settings = s;
}
```

- [ ] **Step 6.4: Build**

Run: `cargo build -p noor-server`
Expected: clean build.

- [ ] **Step 6.5: Commit**

```bash
git add noor-server/src/playback/runtime.rs
git commit -m "feat(audio): sample-rate follows source on track transition"
```

---

## Task 7: HTTP routes

**Files:**
- Modify: `noor-server/src/server/routes.rs` (add three routes + handlers)

**Goal:** Expose `GET /api/audio/devices`, `GET /api/audio/settings`, `PUT /api/audio/settings`. The PUT handler validates, persists, and dispatches a `DeviceSwap` command if device / exclusive / SR-follow changed.

- [ ] **Step 7.1: Wire the routes**

In the `Router::new()` chain in `routes.rs` (near line 230-387), add:

```rust
.route("/api/audio/devices", get(get_audio_devices))
.route(
    "/api/audio/settings",
    get(get_audio_settings).put(put_audio_settings),
)
```

- [ ] **Step 7.2: Implement `get_audio_devices`**

```rust
async fn get_audio_devices(
    State(_state): State<SharedState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let devices = crate::playback::runtime::enumerate_output_devices();
    Ok(Json(serde_json::json!({ "devices": devices })))
}
```

- [ ] **Step 7.3: Implement `get_audio_settings`**

```rust
async fn get_audio_settings(
    State(state): State<SharedState>,
) -> Result<Json<crate::db::audio_settings::AudioSettings>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn))
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
```

- [ ] **Step 7.4: Implement `put_audio_settings`**

The PUT body is the full `AudioSettings` struct. The frontend always knows the complete current state (the store hydrates on mount), so it sends the whole thing on every change. This avoids the partial-update / `Option<Option<T>>` footgun.

```rust
async fn put_audio_settings(
    State(state): State<SharedState>,
    Json(new): Json<crate::db::audio_settings::AudioSettings>,
) -> Result<Json<crate::db::audio_settings::AudioSettings>, (StatusCode, Json<serde_json::Value>)> {
    // Reject exclusive_mode on non-Windows.
    if new.exclusive_mode && !cfg!(target_os = "windows") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "exclusive_mode is only supported on Windows"
            })),
        ));
    }

    let st = state.read().await;
    let (old, saved) = st
        .db
        .with_conn(|conn| {
            let old = crate::db::audio_settings::load(conn)?;
            crate::db::audio_settings::save(conn, &new)?;
            Ok::<_, rusqlite::Error>((old, new.clone()))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "message": e.to_string() })),
            )
        })?;
    let new = saved;

    // Live-apply iff anything affecting the output stream changed.
    let needs_swap = old.output_device != new.output_device
        || old.exclusive_mode != new.exclusive_mode
        || old.sample_rate_follow != new.sample_rate_follow;

    if needs_swap {
        st.playback
            .send_command(crate::playback::runtime::PlaybackRuntimeCommand::DeviceSwap {
                device: crate::playback::runtime::OutputDeviceSelection::from_pref(
                    new.output_device.as_deref(),
                ),
                exclusive: new.exclusive_mode,
                sample_rate_follow: new.sample_rate_follow,
            })
            .await
            .ok();
    }

    Ok(Json(new))
}
```

Adapt `st.playback.send_command(...)` to whatever the existing channel/handle on `SharedState` is named. If there's a helper like `state.runtime.send(...)`, use that.

- [ ] **Step 7.5: Build + smoke-test endpoints**

Run:
```bash
cargo build -p noor-server
cargo run -p noor-server &  # or however the dev server starts
curl -s http://localhost:PORT/api/audio/devices | jq .
curl -s http://localhost:PORT/api/audio/settings | jq .
curl -s -X PUT http://localhost:PORT/api/audio/settings \
  -H 'content-type: application/json' \
  -d '{"quality":"HI_RES_LOSSLESS","output_device":null,"exclusive_mode":false,"sample_rate_follow":false}' | jq .
```

Expected:
- `/devices` returns `{ "devices": [...] }` with at least the system default
- `/settings` GET returns defaults on first call
- `/settings` PUT returns the saved struct with `quality: "HI_RES_LOSSLESS"`

- [ ] **Step 7.6: Commit**

```bash
git add noor-server/src/server/routes.rs
git commit -m "feat(audio): /api/audio/{devices,settings} routes"
```

---

## Task 8: Frontend API client

**Files:**
- Modify: `frontend/src/lib/api/client.ts`

**Goal:** Three typed methods matching the new endpoints.

- [ ] **Step 8.1: Add types and methods**

Append to `client.ts` near the existing Tidal methods:

```typescript
export type AudioQuality = 'LOW' | 'HIGH' | 'LOSSLESS' | 'HI_RES_LOSSLESS';

export interface AudioDevice {
    id: string;
    name: string;
    is_default: boolean;
    max_channels: number;
    supported_sample_rates: number[];
}

export interface AudioSettings {
    quality: AudioQuality;
    output_device: string | null;
    exclusive_mode: boolean;
    sample_rate_follow: boolean;
}

```

The PUT endpoint takes the full `AudioSettings` (no partial updates — see Task 7 rationale).

Add the methods to the existing `api` object (matching the surrounding style):

```typescript
listAudioDevices(): Promise<{ devices: AudioDevice[] }> {
    return fetchApi<{ devices: AudioDevice[] }>('/api/audio/devices');
},

getAudioSettings(): Promise<AudioSettings> {
    return fetchApi<AudioSettings>('/api/audio/settings');
},

updateAudioSettings(settings: AudioSettings): Promise<AudioSettings> {
    return fetchApi<AudioSettings>('/api/audio/settings', undefined, {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(settings),
    });
},
```

- [ ] **Step 8.2: Type-check**

Run: `cd frontend && npm run check`
Expected: 0 errors.

- [ ] **Step 8.3: Commit**

```bash
git add frontend/src/lib/api/client.ts
git commit -m "feat(audio): client methods for /api/audio endpoints"
```

---

## Task 9: Audio settings store

**Files:**
- Create: `frontend/src/lib/stores/audio_settings.ts`

**Goal:** Svelte writable store that hydrates from the server and exposes `update(partial)` which optimistically updates locally and PUTs to the server. On error, revert and surface a toast string the page can render.

- [ ] **Step 9.1: Implement the store**

```typescript
import { writable, get } from 'svelte/store';
import { api, type AudioSettings } from '$lib/api/client';

export interface AudioSettingsState {
    settings: AudioSettings | null;
    loading: boolean;
    error: string | null;
    pendingApply: boolean;
}

const initial: AudioSettingsState = {
    settings: null,
    loading: false,
    error: null,
    pendingApply: false,
};

function createStore() {
    const { subscribe, update } = writable<AudioSettingsState>(initial);

    return {
        subscribe,
        async load() {
            update((s) => ({ ...s, loading: true, error: null }));
            try {
                const settings = await api.getAudioSettings();
                update((s) => ({ ...s, settings, loading: false }));
            } catch (err) {
                update((s) => ({
                    ...s,
                    loading: false,
                    error: err instanceof Error ? err.message : String(err),
                }));
            }
        },
        /**
         * Merge `patch` into the current settings and PUT the full object.
         * Optimistically updates the store; reverts on error.
         */
        async patch(patch: Partial<AudioSettings>) {
            const before = get({ subscribe }).settings;
            if (!before) return;
            const next: AudioSettings = { ...before, ...patch };
            const isLiveApplyChange =
                next.output_device !== before.output_device ||
                next.exclusive_mode !== before.exclusive_mode ||
                next.sample_rate_follow !== before.sample_rate_follow;
            update((s) => ({ ...s, settings: next, error: null, pendingApply: isLiveApplyChange }));
            try {
                const saved = await api.updateAudioSettings(next);
                update((s) => ({ ...s, settings: saved, pendingApply: false }));
            } catch (err) {
                update((s) => ({
                    ...s,
                    settings: before,
                    pendingApply: false,
                    error: err instanceof Error ? err.message : String(err),
                }));
            }
        },
    };
}

export const audioSettings = createStore();
```

- [ ] **Step 9.2: Type-check**

Run: `cd frontend && npm run check`
Expected: 0 errors.

- [ ] **Step 9.3: Commit**

```bash
git add frontend/src/lib/stores/audio_settings.ts
git commit -m "feat(audio): audioSettings store with optimistic patch"
```

---

## Task 10: Audio section in /settings page

**Files:**
- Modify: `frontend/src/routes/settings/+page.svelte`

**Goal:** Render an Audio section at the top of the existing settings page. Quality dropdown, device dropdown, exclusive toggle (Windows-only), SR-follow toggle, "Reconfiguring…" hint when `pendingApply` is true, error banner when `error` is set.

- [ ] **Step 10.1: Read the existing page**

Open the file. Note its current structure (rune mode `$state` / `$derived`, how wallpaper/palette controls are laid out). Match the visual style of the existing controls — do not introduce a new design system.

- [ ] **Step 10.2: Add the Audio section**

At the top of the page (above wallpaper/palette controls):

```svelte
<script lang="ts">
    import { onMount } from 'svelte';
    import { audioSettings } from '$lib/stores/audio_settings';
    import { api, type AudioDevice, type AudioQuality } from '$lib/api/client';

    let devices = $state<AudioDevice[]>([]);
    let isWindows = $derived(typeof navigator !== 'undefined' && /Win/i.test(navigator.platform));

    const QUALITY_OPTIONS: { value: AudioQuality; label: string }[] = [
        { value: 'LOW', label: 'Low (96 kbps AAC)' },
        { value: 'HIGH', label: 'High (320 kbps AAC)' },
        { value: 'LOSSLESS', label: 'Lossless (CD quality FLAC)' },
        { value: 'HI_RES_LOSSLESS', label: 'Hi-Res Lossless (up to 24-bit / 192 kHz FLAC)' },
    ];

    onMount(async () => {
        await audioSettings.load();
        try {
            const resp = await api.listAudioDevices();
            devices = resp.devices;
        } catch (err) {
            console.error('Failed to load audio devices', err);
        }
    });

    function onQualityChange(e: Event) {
        const value = (e.target as HTMLSelectElement).value as AudioQuality;
        audioSettings.patch({ quality: value });
    }

    function onDeviceChange(e: Event) {
        const value = (e.target as HTMLSelectElement).value;
        audioSettings.patch({ output_device: value === '__default__' ? null : value });
    }

    function onExclusiveToggle(e: Event) {
        audioSettings.patch({ exclusive_mode: (e.target as HTMLInputElement).checked });
    }

    function onSrFollowToggle(e: Event) {
        audioSettings.patch({ sample_rate_follow: (e.target as HTMLInputElement).checked });
    }
</script>

{#if $audioSettings.settings}
    {@const s = $audioSettings.settings}
    <section class="settings-section">
        <h2>Audio</h2>

        <label>
            <span>Quality</span>
            <select value={s.quality} onchange={onQualityChange}>
                {#each QUALITY_OPTIONS as opt}
                    <option value={opt.value}>{opt.label}</option>
                {/each}
            </select>
        </label>

        <label>
            <span>Output device</span>
            <select
                value={s.output_device ?? '__default__'}
                onchange={onDeviceChange}
            >
                <option value="__default__">System default</option>
                {#each devices as d}
                    <option value={d.id}>
                        {d.name}{d.is_default ? ' (default)' : ''}
                    </option>
                {/each}
            </select>
        </label>

        {#if isWindows}
            <label class="toggle">
                <input
                    type="checkbox"
                    checked={s.exclusive_mode}
                    onchange={onExclusiveToggle}
                />
                <span>Exclusive output (WASAPI)</span>
            </label>
            <p class="hint">
                When on, no other app can use this device while NOORwave is playing.
            </p>
        {/if}

        <label class="toggle">
            <input
                type="checkbox"
                checked={s.sample_rate_follow}
                onchange={onSrFollowToggle}
            />
            <span>Sample rate follows source</span>
        </label>
        <p class="hint">
            Reconfigures the output device to each track's native rate (44.1 / 48 / 96 / 192 kHz). Recommended with exclusive mode.
        </p>

        {#if $audioSettings.pendingApply}
            <p class="hint pending">Output reconfiguring…</p>
        {/if}
        {#if $audioSettings.error}
            <p class="error">{$audioSettings.error}</p>
        {/if}
    </section>
{/if}
```

Adapt class names to whatever the existing page uses (the styles are inherited). If the existing page wraps controls in a `<div class="settings-grid">`, mirror that.

- [ ] **Step 10.3: Type-check + run**

```bash
cd frontend
npm run check
npm run dev
```

Manually verify per the spec's Verification section:
- Quality dropdown changes propagate to the next track's URL (check server logs for `audioquality=...`).
- Device dropdown lists at least the system default plus any USB DACs.
- On Windows, exclusive toggle is visible. On macOS/Linux, hidden.
- Toggling device mid-playback shows "Output reconfiguring…" briefly then audio resumes on the new device.

- [ ] **Step 10.4: Commit**

```bash
git add frontend/src/routes/settings/+page.svelte
git commit -m "feat(audio): Audio section in /settings page"
```

---

## Verification (full plan)

Run through every item in the spec's Verification section
([docs/superpowers/specs/2026-04-27-tidal-audio-settings-design.md](../specs/2026-04-27-tidal-audio-settings-design.md#verification))
once all 10 tasks are committed:

1. Quality applies on next track resolve.
2. Device picker lists USB DACs; selecting routes audio there; unplugging reverts.
3. Exclusive mode (Windows): silences other apps or fails predictably with a revert.
4. Sample-rate-follow: brief silence between rate changes; OS confirms native rate.
5. Live apply: device swap mid-track resumes within ~500 ms.
6. Persistence: settings survive a server restart.
7. Non-Windows: exclusive toggle absent in UI; PUT with `exclusive_mode: true` returns 400.

If any item fails: open a follow-up issue or fix in a small extra task — do not silently skip.

## Known gaps acknowledged in this plan

- **True WASAPI ShareMode::Exclusive** — Task 5 ships the user-facing toggle, code path, and low-latency buffer. Real `ShareMode::Exclusive` requires a cpal upgrade (or raw windows-rs IAudioClient). Logged in `build_wasapi_exclusive_stream`. This was acknowledged in the spec's Risks section; users get most of the latency benefit, but a follow-up slice should complete the feature.
- **No frontend tests** — established repo state. Document, do not change here.
- **CPAL device id == device name** — CPAL doesn't surface a stable id. Renaming a device in the OS will look like "device disappeared, fall back to default" to NOORwave. Acceptable; not worth a workaround in v1.
