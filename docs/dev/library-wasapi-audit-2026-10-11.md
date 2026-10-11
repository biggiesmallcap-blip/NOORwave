# Library loading and Windows output audit, 2026-10-11

## Scope and review method

Follow up PR #351's repeated blank Library landing, remove duplicated exclusive
failure feedback, and audit the Windows WASAPI/CPAL output path. The user
explicitly approved playback runtime, exclusive output, and device handling.

Two independent specialist passes reviewed transport initialization/rendering
and runtime/CPAL lifecycle. Root checked the actual worktree, public event
consumers, route wiring, tests, and Windows hardware behavior. RepoWise's index
was three days old and was treated as a lead, not source truth. Rust analyzer
timed out twice; local source and locked dependency source were inspected.

The initial pass kept dependency pins unchanged. The user subsequently
authorized staged audio dependency upgrades. Existing settings fields,
WebSocket payloads, queue persistence, and release configuration remain intact.
The installed application was not replaced. Hardware probes emitted silence
and did not alter the application's saved output settings or library.

## Acceptance checks

### Current hardening and dependency stage

This stage follows the user's expanded runtime/performance request. The earlier
verification below covers the initial patch and must not be treated as evidence
for later changes until those checks are rerun.

| Check | Status | Current evidence and remaining work |
| --- | --- | --- |
| Native output faults pause audible decks and DB/UI together; Resume rebuilds failed output | Partially verified | CPAL 0.18.2 output group passed 5 tests and transport listener group passed 49 tests. Runtime fault/recovery paths pass; full baseline regression passed; patched desktop audio remains pending |
| Retired outputs/decoders cannot fault or terminate their replacements | Partially verified | Decoder identity and stream lifetime tokens wired; callback/runtime tests pass; listener replacement races need final combined verification |
| Exclusive release remains prompt without device events; unexpected loss is recoverable | Partially verified | Cancellable event-wait and reclamation group passed 34 tests on the existing dependency. Fault polling is wired; combined CPAL 0.18.2/wasapi 0.25 silent release/reopen smoke passed |
| Data callbacks avoid decoder-lock waits, logging, allocation and source reclamation | Partially verified | Reproduced blocking PCM access and transition-channel allocation. Try-lock/silence, deferred diagnostics, nonallocating spectrum access, fixed Copy notification mailbox and publisher-owned source reclamation implemented. Allocation, contention, saturation and seek/pause tests pass. Native combined smoke passed; audible deadlines remain unverified |
| Rate/layout and PCM transport stay compatible across fallback, recovery and upgrade | Partially verified | CPAL F64/I24 and runtime rate checks pass. A reused-sink regression reproduced reporting 96 kHz for an actual 44.1 kHz sink and now passes after fixing the real startup path. Native combined smoke passed; audible transitions remain unverified |
| Device selection resolves exact endpoints and preserves unique legacy preferences | Partially verified | Stable-ID and legacy-name tests pass; production Settings smoke passes. Missing-device public handler regression reproduced decks continuing to play; all decks now pause and the regression passes. Physical unplug/replug remains unverified |
| Relevant dependency changes are staged, tested and reviewed against the real product path | Partially verified | wasapi 0.25.0 and CPAL 0.18.2 are pinned with targeted lock updates. Latest runtime suite passed 204 tests, 3 hardware tests skipped; the missing-device and reused-sink hardware-environment tests were executed separately and passed. Full baseline suite passed 2,107 tests/10 ignored; final mailbox ordering was verified by the affected runtime suite. Patched desktop audible validation remains pending |

### Initial library and output pass

| Check | Status | Evidence and limit |
| --- | --- | --- |
| Library landing remains visible while unrelated browse queries warm up | Done | Cold start, aged persisted cache, revisit, failed browse query, and empty landing exercised in dev and production browser smoke |
| Exclusive failure produces one persistent banner and one identical transient notification, with functional Retry/Disable | Done | Browser smoke drives the real Settings route and WebSocket handler; four toast regressions |
| Exclusive output opens the resolved Windows endpoint, with no silent default fallback | Done | Raw endpoint-ID enumeration; public missing-endpoint regression; silent Realtek opens |
| Release/reopen, prepared adoption, shared fallback, and video pause retain coherent output/transport state | Done for covered paths | Driver-free runtime regressions and silent hardware release cycles; audible media transitions remain unverified |
| PCM rate/layout remains compatible with its sink and integer conversion preserves representable source PCM | Done for covered paths | Full i16 round trip, representative i24 packing, shared integer callbacks, cached-rate guard, cold-job offset/pause tests |
| Windows compilation, server regressions, frontend checks, and formatting pass | Done | Final full server suite, warning-free check, local executable build, frontend production build, and browser smoke all passed |

### Current verification, 2026-10-12

- Full server regression before the final mailbox dispatch-order change:
  2,107 passed, 10 ignored, 344.80 seconds.
- Final affected runtime regression: 204 passed, 3 ignored. It includes direct
  actor mailbox consumption, queued control priority, seek retirement, bounded
  saturation retries and callback allocation/deadline checks.
- The ignored missing-device and reused-exclusive-rate tests were executed
  separately against a Windows endpoint environment and passed. Neither opens
  a native audio stream.
- Final silent Realtek native smoke with CPAL 0.18.2 and wasapi 0.25.0 passed:
  44.1/96 kHz forced release and reopen, 44.1 kHz idle release, 96 kHz reopen,
  and an 88.2 kHz shared request opened at 88.2 kHz.
- Final `cargo check -p noor-server --locked`, formatting check and diff
  whitespace check passed. Actual server executable build passed.
- Production Library and Settings smoke passed again with Edge on 2026-10-12.
  Frontend behavior is committed separately as `d49964e2`; skill documentation
  is committed as `945c06fb`.

This is a verified backend baseline, not completion of the expanded playback
goal. Remaining renderer unwind/wait-result, fatal error callback, audible
desktop and later decoder/resampler evaluation work is tracked below.

## Confirmed findings and fixes

| Severity | Finding | Fix and owning surface |
| --- | --- | --- |
| P1 | PR #351 persisted shelf data but the outer Library `$isLoading` branch still hid the landing and postponed mounting its mural queries | Landing queries now own their loading states; `frontend/src/routes/library/+page.svelte` |
| P1 | CPAL display names and WASAPI friendly names are different properties; a failed name match silently opened the Windows default with the selected device's rate/layout | Use CPAL's raw `DeviceId.id()` and exact endpoint lookup in wasapi 0.25; stable device selection preserves unique legacy names |
| P1 | wasapi 0.23 direct ID lookup creates a `PCWSTR` from a temporary `HSTRING` that dies before `GetDevice` | Initial enumeration workaround passed after direct-ID failure. Upgrading to 0.25 fixes upstream lifetime ownership and allows exact direct lookup; missing-endpoint regression remains enforced |
| P1 | A prepared decoder could become active after idle release without an exclusive sink; after shared fallback, exclusive preference also caused future prepared decks to have no CPAL output | Ensure output after both cold and prepared adoption; track the effective runtime backend separately from the persisted preference; `runtime/job_switch.rs`, `runtime/exclusive_output.rs` |
| P1 | Changing decoder target rate did not convert already buffered PCM; a live retry could play 48 kHz PCM at 96 kHz | Preserve the actual buffered rate during live changes. Reject incompatible live buffers, reconstruct cold fallback decoders at the negotiated shared rate, and rebind transport readers |
| P1 | Cancellation from a retired decoder could send a terminal command with the replacement's same track/generation identity | Suppress terminal signaling after the shared state is stopped; `runtime/shared.rs` |
| P2 | A retry could retain this process's CPAL output while requesting exclusive ownership | Pause transport during acquisition and drop all live CPAL streams before opening WASAPI; restore pause intent afterward |
| P2 | Fallback handled only the active deck and could report the last prepared deck's rate as the active rate | Install shared output on every live deck and use the first active result as authoritative; stop and report complete output failure |
| P2 | Cold-start fallback could use a previously overwritten track rate as its fallback configuration | Existing cold-start reconstruction queries the device default configuration and creates a fresh decoder at the negotiated rate |
| P2 | Realtek integer candidates failed `0x8889000f`, but the final f32 `0x88890008` overwrote the useful failure | Classify endpoint creation separately, use one bounded 1.3-second ownership/reopen retry budget, and retain endpoint/initialization failures over format rejections |
| P2 | Buffer-alignment retry activated its replacement while retaining the failed `IAudioClient` | Release the failed client before replacement activation; propagate buffer-size errors |
| P2 | COM initialization was not balanced on render-thread exit | Thread-lifetime RAII balances successful initialization, including `S_FALSE`, after audio clients drop |
| P2 | The release flag/event was published before audio objects actually dropped | Explicitly drop render client, audio client, and event handle before publishing release |
| P2 | Signed integer conversion scaled by signed maximum and truncated, changing exact decoded PCM by one least-significant bit; unsigned shared silence used 32767 rather than 32768 | Power-of-two scaling with saturation/rounding; exact i16/i24 conversion tests; correct unsigned silence; shared I32 output support |
| P2 | Render periods cloned source vectors and could reclaim the final retired source ownership | Borrow the source bank through a nonblocking read guard only during mixing; publication unlocks before retiring vectors on the control thread, and stream teardown retains the bank through renderer join |
| P2 | Video yield omitted the preview deck, did not latch pause through promotion, and left public playback marked playing so the release listener could immediately reacquire the endpoint | Pause all audible decks, preserve user pause through prepared promotion, and enqueue `Paused` before requesting release; `runtime/command_handlers.rs` |
| P2 | Output rate recovery did not always update public runtime metadata; complete failures could leave the UI claiming playback | Emit `Ready` when actual rate changes and `Stopped` on complete failure. A deferred rate change logs a notice rather than emitting `Error`, because the existing public error consumer pauses playback |
| P2 | An active decoder's channel layout cannot be changed by replacing only its output stream | Reject incompatible live channel changes, pause actual decks to agree with error handling, and retain the previous endpoint. Stop before applying the new layout |
| P2 | Settings rendered the same exclusive banner twice, and repeated failure events stacked identical toasts | Keep one banner; opt-in toast reuse resets the existing notification's lifetime without changing ordinary toast behavior |

## Public path and compatibility

Settings Retry still calls `POST /api/audio/exclusive/retry`. That handler reads
the persisted exclusive preference and enqueues `DeviceSwap`. A failed exclusive
request switches the runtime's effective mode to shared while leaving that
preference intact for an explicit retry.

`ExclusiveModeFailed`, `ExclusiveModeEngaged`, `ExclusiveModeReleased`, `Ready`,
and `Stopped` keep their existing route/WebSocket shapes. The existing runtime
`Error` consumer pauses DB transport state, so nonfatal rate deferral must not
use it. A rejected channel-layout change also pauses the actual render decks.
Video yield sends `Paused` before triggering `ExclusiveModeReleased`; the
ordered listener reconciles DB playback intent before deciding whether to retry.

Live retry/device changes preserve the buffered sample rate. Native source-rate
matching is applied at the next cold track transition. A live fallback that
cannot open a compatible rate stops with an error instead of playing wrong-pitch
PCM. Seamless conversion of already buffered audio was not added.

## TDD and adversarial status

- Before the PCM fix, both integer transport regressions failed on incorrect
  decoded sample values. The fixed converters pass.
- Before the shared-state fix, stopped-decoder terminal suppression and shared
  i16 callback regressions failed. The fixed callbacks and cancellation guard
  pass in the playback suite.
- Runtime fallback helper tests initially failed to compile before their
  implementation. Driver-free tests then covered all live decks, failure
  propagation, active rate reporting, cold offset/pause preservation, cached PCM
  rate rejection, and video pause through promotion.
- The public missing-endpoint test calls `build_exclusive_stream` and verifies
  it fails without opening default output or emitting engaged/released events.
- The video-release public-handler regression first failed because no pause
  acknowledgment was queued, then passed after adding that acknowledgment
  before endpoint release. Shared-mode release remains a no-op.
- UI smoke reproduced duplicate banners/toasts before the fix. It passes
  after the fix through the production route.
- Adversarial passes found and closed cached-rate corruption, native fallback
  configuration, old decoder cancellation, incorrect runtime rate reporting,
  and the nonfatal-notice interaction with fatal error handling.

## Windows hardware observations

The installed app reported paused playback and no exclusive engagement before
the silent test. The existing Realtek Digital Output endpoint passed:

- 44.1 kHz exclusive open, silent render, forced release.
- 96 kHz exclusive open, silent render, forced release.
- 44.1 kHz reopen, then release after the one-second idle grace.
- 96 kHz reopen after idle release, then forced release.
- An 88.2 kHz shared request opened successfully at 88.2 kHz.

A separate exclusive 88.2 kHz attempt failed with `0x8889000f` across candidate
formats on this device. This confirms that the driver can use endpoint-creation
failure for a format/rate request even when other rates work. It does not prove
every earlier installed-app failure had the same cause.

Hardware test command, with the device selected explicitly:

```powershell
$env:NOOR_AUDIO_SMOKE_DEVICE = 'Realtek Digital Output'
cargo test -p noor-server exclusive_output_silent_hardware_smoke --locked -- --ignored --nocapture
```

The opt-in test renders silence. It does not validate audible media, channel
mapping by ear, source-perfect hardware capture, or gapless continuity by ear.

## Verification commands

- `cargo test -p noor-server playback --locked`: 566 passed, 3 ignored.
- Silent Realtek hardware smoke: passed.
- `cargo test -p noor-server video_exclusive_release --locked`: 2 passed after
  the final event-order fix.
- `cargo fmt --all -- --check`: passed after targeted formatting.
- `git diff --check`: passed.
- `cargo check -p noor-server --locked`: passed without warnings.
- `cargo build -p noor-server --locked`: passed; final local debug executable
  built without replacing the installed app.
- Final `cargo test -p noor-server --locked --quiet`: 2,068 passed, 8 ignored,
  zero failures after the final video-release event-order fix.
- Frontend `pnpm check`: no errors or warnings.
- Frontend `pnpm test`: 211 files, 1,317 tests passed.
- Frontend `pnpm lint` and `pnpm run build`: passed.
- Library-loading and exclusive-warning browser scripts: passed against dev
  and production builds with Edge and isolated API/WebSocket fixtures.

The server is a binary crate without a library target, so the full
`cargo test -p noor-server --locked` suite is used instead of `--lib`.

## Remaining limits and next checks

- Audible track transitions, pause/resume, video return, rapid real device
  switching, and hot unplug/reconnect need a patched desktop build and human
  audio confirmation. The installed app still runs its existing release.
- PCM access uses `try_lock`; contention produces silence without advancing the
  cursor. Measure audible underruns under sustained decoder compaction before
  deciding whether a ring transport is justified.
- Callback notifications use 32 inline Copy slots inside the PCM buffer, drained
  on the runtime actor with a 5 ms active wait. Seeks and DJ replacement clear
  pending values under the cursor lock. This is a polling interval, not a hard
  Windows scheduling guarantee; measure dispatch latency with real playback.
- Source vectors are borrowed only during mixing, and publication drops retired
  ownership on the control thread. Upstream WASAPI trace logging can still run
  if enabled; the no-logging claim applies to application render code.
- Device waits check cancellation using 10 ms slices and retain the two-second
  no-event fault deadline. wasapi 0.25 still collapses WAIT_FAILED into timeout;
  immediate returns are now paced, after reproducing 552,473 calls in 35 ms.
  An outer renderer unwind guard now publishes a typed fault after resource
  teardown. See the wait-pacing and renderer-unwind handoffs dated 2026-10-12.
  Real unplug/reconnect and audible recovery remain unverified.
- Recoverable CPAL underrun/scheduling notices are deferred atomic telemetry.
  Fatal output faults pause all decks and use stream/decoder identity in the
  listener. Fatal CPAL error formatting/channel sends can still allocate on the
  error callback; the allocation regression covers the data callback.
- f32 decoding cannot retain every bit of arbitrary signed 32-bit PCM. The
  tests establish exact representable i16/i24 transport conversion, not a
  universal 32-bit bit-perfect guarantee or unity gain throughout every DSP
  and transport-envelope operation.
- Decoder-stop identity and output lifetime tokens now reject retired callbacks,
  including same-track/generation replacement and overlapping engine slots.
  Continue testing device-loss interleavings against a patched desktop build.
- Device IDs now identify exact endpoints. Unique legacy names remain accepted;
  ambiguous or missing names require an explicit Settings selection.

These limits are recorded here as audit findings. No requested unfinished
verification is moved to `FOLLOWUPS.md`. The one earlier follow-up entry concerns
the unrelated browser reconnect timer receiver.

## Primary references

### Dependency decisions

| Component | Previous pin | Evaluated version | Decision and required checks |
| --- | --- | --- | --- |
| wasapi | 0.23.0 | 0.25.0 | Stage first: fixes direct endpoint lookup's HSTRING lifetime and COM string cleanup; adds exclusive-format probing. Preserve source rate/container, keep Initialize authoritative, rerun missing-endpoint and silent native release/reopen tests |
| CPAL | 0.17.3 | 0.18.2 | Stage second: WASAPI device-loss Drop/play/pause panic fixes, corrected 24-bit packing and shared conversion support. Migrate unified ErrorKind, preserve unique legacy DeviceDesc preferences, add F64/I24 callbacks and rerun shared/exclusive recovery tests |
| Symphonia | 0.5.5 | 0.6.1 | Worth a later isolated decoder stage: MP3 silence/metadata fixes and demuxer hardening. Audio, timing, metadata and EOF APIs changed in 0.6; require FLAC/MP3/AAC/Opus fixtures, truncated-media/EOF behavior and streaming-seek parity before changing playback and noor-mix |
| rubato | 0.16.2 | 5.0.1 | Evaluate separately from output fixes. Modern adapter API and sinc/SIMD optimizations may improve throughput; migration changes buffer adapters and resampler types. Benchmark our actual ratios/chunk sizes, impulse latency, output-frame accounting, tails and DJ tempo alignment before replacing current resamplers |

The existing decoder already backpressures at 45 seconds ahead and resumes
below 30 seconds, retaining about ten seconds behind the cursor. It compacts
only after twice the retained distance is consumed. A per-packet compaction
rewrite is therefore not justified by the actual source.

- [CPAL 0.18.2 tagged changelog](https://github.com/RustAudio/cpal/blob/v0.18.2/CHANGELOG.md)
- [CPAL 0.18.2 migration guide](https://github.com/RustAudio/cpal/blob/v0.18.2/UPGRADING.md)
- [wasapi 0.25.0 exact endpoint lookup and exclusive format helpers](https://github.com/HEnquist/wasapi-rs/blob/v0.25.0/src/api.rs)
- [Symphonia 0.6.0 breaking API changes](https://github.com/pdeljanov/Symphonia/releases/tag/v0.6.0)
- [Symphonia 0.6.1 fixes](https://github.com/pdeljanov/Symphonia/releases/tag/v0.6.1)
- [rubato 5.0.1 changelog and adapter migration](https://github.com/HEnquist/rubato/blob/v5.0.1/README.md)

- [Microsoft IAudioClient initialization and alignment retry contract](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-initialize)
- [wasapi 0.23 source, including direct endpoint lookup](https://github.com/HEnquist/wasapi-rs/blob/v0.23.0/src/api.rs)
- Locked local CPAL 0.17.3 and Symphonia conversion sources were inspected for
  endpoint identity and signed PCM normalization.
