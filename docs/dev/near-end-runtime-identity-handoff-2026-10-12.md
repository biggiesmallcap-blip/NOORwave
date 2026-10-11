# Near-end runtime identity handoff

A queued NearEnd notification could begin work on a replacement runtime. A stream resolution that began before replacement could also send output or preparation commands to the retired runtime and overwrite the replacement's pending stream metadata. Matching track IDs and playback generation did not identify the emitting runtime.

Both the event listener and early DJ preparation now pass the originating runtime into the near-end path. The path validates that runtime at entry, while claiming its prebuffer slot, after stream resolution, before a rate-follow output command, and immediately before final preparation dispatch and metadata publication. Preparation and pending metadata are published under the same validated AppState write guard. SQLite reads stay before that final write guard. Failed PrepareNext sends propagate and leave pending metadata untouched.

Prebuffer keys retain the runtime's identity allocation and compare it by pointer identity, not the health flag's value. This prevents old completions from clearing replacement work for the same tracks and generation. Retention also prevents identity allocation reuse if a cancelled task leaves its slot behind. A replacement runtime can claim that slot without waiting for the old resolver.

## Acceptance checks

- Done: a queued notification from a retired listener cannot begin replacement stream resolution.
- Done: a delayed successful resolution cannot dispatch to the retired runtime or overwrite replacement metadata.
- Done: overlapping preparations for the same tracks and generation have separate ownership; old completion preserves the replacement slot.
- Done: cancellation retains the old identity until replacement takes the slot, and current preparation reaches PrepareNext with the correct track and generation.
- Done: failed command sends release the slot without publishing metadata; rejected streams preserve replacement queue rows, while current-runtime rejection skips only the upcoming row.
- Done: full backend suite and final compilation/formatting checks.

## Verification

- `cargo test -p noor-server server::transport --locked`: 73 passed.
- `cargo test -p noor-server --locked`: 2,157 passed, 11 ignored, zero failures, 251.65 seconds.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

## TDD and review

The delayed-resolution regression failed before implementation with a command dispatched after runtime replacement. The tests exercise the actual listener and near-end transport path using a gated StreamSource and a command consumer that answers real TrackStatus requests. Fixtures use separate TIDAL IDs to isolate shared source-availability bookkeeping.

Adversarial review checked AppState lock waits, output command enqueue, metadata publication, reused track IDs and generations, overlapping completion, cancellation, command-channel closure, rejection handling, and identity lifetime. A numeric address key was replaced during review because it did not retain identity after cancellation. The retained identity does not keep an output stream or command sender alive. No API, WebSocket, dependency, or release contract changed.

## Limits and remaining audit

Already-started stream requests may finish; their stale results cannot produce preparation effects. Existing SQLite reads under AppState read guards still need a separate contention/scheduling review. Same-runtime queue edits and cancellation/retry behavior remain part of the broader control-ordering audit. Next checks also include TrackError recovery and DJ follow-on dispatch across runtime replacement.

Next format check: the existing sample-rate-follow policy skips prebuffering when a valid upcoming rate changes, in shared and exclusive modes. The later device-swap branch therefore requires invalid zero/negative metadata to differ from the current rate without that policy rejecting preparation. Its raw signed-to-unsigned cast needs a separate regression and fix. This task leaves that branch unchanged.

Audible desktop gapless transitions, rapid device switching, hotplug, non-Windows compilation, and release-mode performance remain unverified. No installed app or user database was modified. Version remains 1.0.5. Follow-ups added: none.
