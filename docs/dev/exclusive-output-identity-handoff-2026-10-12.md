# Exclusive output identity handoff

Exclusive engagement, failure, and release events now carry an internal Arc
identity for their output attempt. The runtime handle and exclusive sink share
its current reader. The listener validates runtime and output identity after
the AppState lock wait, then holds the identity guard through metadata updates,
public notification, and any recovery enqueue. Public WebSocket payloads remain
unchanged.

Stream retirement publishes a new identity before renderer join and releases
the identity mutex before that join. Explicit sink clear and implicit sink Drop
invalidate old notifications. Native idle and requested release retain their
identity so current release notifications still work. Shared fallback after an
acquisition failure clears sources while retaining the failed attempt's identity;
the current failure banner must remain deliverable even without a live stream.

Automatic release recovery carries the same identity in its internal DeviceSwap
command. The actual handler checks it before device lookup or stream changes,
discarding recovery queued behind a newer swap. Ordinary user DeviceSwap calls
carry no condition and retain their existing behavior. Only the runtime actor
publishes identities, so the handler's match stays valid until its own swap.
Neither the audio callback nor the render mixing loop takes this mutex.

Acceptance checks:

- Done: superseded engaged, failed, and released events are rejected after a
  contended state lock, preserving current format/status and emitting no stale
  public notifications or recovery commands.
- Done: current notifications preserve their public data and retry policy;
  retry commands carry the originating output identity.
- Done: stale recovery is rejected by the real DeviceSwap handler before device
  validation, while current recovery and ordinary user requests still reach it.
- Done: explicit and implicit teardown retire identity and unlock its mutex
  before renderer join; failed-start cleanup retains the current diagnostic.
- Done: silent Realtek native exclusive open/release/reopen at 44.1 and 96 kHz,
  idle release, and shared 88.2 kHz open passed. Native release events carried
  the exact identity supplied at construction.
- Done: targeted exclusive checks passed (51 tests, 2 ignored), including the
  real-listener stale-output test. The missing-device handler check passed
  when explicitly run, covering stale recovery, ordinary swaps, and current
  recovery. The full server suite passed: 2,137 tests, 11 ignored, 291.99 s.
  `cargo check -p noor-server --locked`, `cargo fmt --all -- --check`, and
  `git diff --check` passed.

TDD: the real-listener test first failed because superseded engagement reached
the UI. The actual handler test then failed because superseded recovery paused
the current decks on a missing endpoint. Both passed after their respective
identity checks. The handler fixture enumerates an endpoint but opens no stream.
Sink teardown observations return to the test thread, so renderer assertion
failures cannot be hidden by Drop's existing join-error handling.

Adversarial review checked replacement during lock waits, queued commands ahead
of recovery, duplicate recovery, current failures without live streams, explicit
shared/stop teardown, runtime exit, mutex poison recovery, Arc address reuse,
and lock order. The publication critical section only replaces a valid Arc;
native join, COM/device work and source-bank cleanup occur outside it. The
listener performs no await while holding the identity guard. Native teardown
owns its token until event emission or exit, outside the render mixing loop.

Not verified: audible desktop rapid device switching, hot unplug/reconnect,
and listening quality. Silent native checks prove device/event wiring rather
than audibility. Non-Windows compilation was not run in this Windows workspace;
conditional recovery is discarded there because WASAPI release is Windows-only.
No installed app, user DB, dependency, API route, release or version changes.
Version remains 1.0.5. Previous task: 5da409e2. FOLLOWUPS.md entries: none.

Remaining overall-goal work includes other same-runtime event/control ordering,
runtime-wide faults, staged decoder/resampler upgrades and timing/performance
fixtures, resampler delay/tail and failure propagation review, and patched
desktop audible recovery/hotplug verification.
