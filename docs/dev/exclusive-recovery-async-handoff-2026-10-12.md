# Exclusive recovery async read handoff

Exclusive release recovery clones the Database after a short read-only
AppState/runtime/output check, then performs its SQLite reads on Tokio's
blocking pool with neither AppState nor the output identity mutex held. After
the read, it revalidates runtime and output under fresh guards before status
updates, release notification, and conditional recovery enqueue. Send-error
logging occurs after both guards are dropped.

The native DeviceSwap handler also rejects automatic release recovery when
user_paused is true. A pause can be processed while the settings read is in
flight or while recovery is queued. The actor's transport intent is the final
authority. Explicit user swaps use no recovery condition and remain valid
while paused. Existing output identity checks reject recovery after a newer
swap; no public API or WebSocket payload changed.

Acceptance checks:

- Done: a held database does not monopolize a single-thread async executor;
  AppState remains writable and output retirement remains possible.
- Done: replacing either output or runtime during the read suppresses stale
  release status, public notification and recovery commands.
- Done: current release policy and database-error behavior pass the listener
  tests. Failed reads publish current release status without retrying.
- Done: the actual DeviceSwap handler ignores current recovery after pause,
  while explicit paused swaps and current unpaused recovery still validate
  their endpoint. Retired recovery remains ignored.
- Done: final transport suite passed 45 tests (16.09 s). Full server suite
  passed 2,144 tests, zero failures, 11 ignored (247.43 s). Compilation,
  formatting and diff checks passed after the final logging-scope refinement.

TDD: the single-thread executor regression failed when its database holder's
five-second timeout elapsed before the async test could resume. The actual
DeviceSwap test failed because paused automatic recovery reached missing-device
validation and emitted CommandError. Both passed after the changes.

Verification: the final contention/error checks passed (0.40 s), the final
transport and full suites passed as recorded above, and the opt-in actual DeviceSwap handler test
passed (0.06 s). That fixture enumerates a Windows endpoint but opens no audio
stream. `cargo check -p noor-server --locked`, `cargo fmt --all -- --check`,
and `git diff --check` passed for the final patch.

Adversarial review checked the AppState and identity guard lifetimes, runtime
replacement during the await, output retirement, paused actor intent, newer
queued settings swaps, explicit requests while paused, query errors, blocking
task failure, listener cancellation, and logging outside the final guards.
Aborting the listener prevents later command enqueue, but an already-started
blocking read can continue until its connection wait/query finishes.

Remaining overall-goal work: the listener still awaits this read, so later
runtime events can queue behind a slow database lookup even though AppState
and the async executor are free. Further recovery scheduling must preserve
event order and bound work. Other remaining items include runtime event/control
ordering, decoder/resampler upgrades with fixtures, measured performance,
and audible recovery/hotplug checks.

Not verified: audible desktop device switching/video return, hotplug,
non-Windows compilation, and release-mode latency/CPU measurements. No installed
app or user database was modified. Version remains 1.0.5; dependencies unchanged.
Previous task: f12380e8. FOLLOWUPS.md entries: none.
