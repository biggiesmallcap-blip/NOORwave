# Runtime activity identity handoff

Started, Stalled, and StallRecovered notifications could change shared playback state after their emitting runtime was replaced. Matching the track or playback generation was insufficient: a replacement can retain both.

Started now checks the runtime before reading its queue anchor and checks it again under the write lock before marking audio active, promoting stream metadata, clearing the error, or recording history. Its TrackChanged and PlaybackStateChanged events are sent under that same validated guard. Stall notifications also validate the runtime under the write lock before pausing or resuming the listening timer.

## Acceptance checks

- Done: a retired start with the replacement's generation cannot mark it audible, clear its error, consume its pending stream metadata, or publish UI events.
- Done: a retired stall cannot pause a replacement session for the same track.
- Done: a retired stall recovery cannot resume a replacement session for the same track.
- Done: a current runtime still publishes start status and stream metadata, and pauses/resumes the listening timer through the actual listener.
- Done: full backend regression suite and final checks.

## Verification

- `cargo test -p noor-server server::transport --locked`: 66 passed.
- `cargo test -p noor-server --locked`: 2,150 passed, 11 ignored, zero failures, 355.64 seconds.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

## TDD and review

All three retirement regressions failed before implementation, showing the actual audibility and timer mutations. They hold AppState while publishing notifications, replace the runtime, and then drain the listener through its ordered Exited event. The positive test consumes public start events and uses a public output notification as the ordered barrier for stall handling.

Adversarial review checked replacement during state-lock waits, reused generations and track IDs, metadata consumption, public event publication, and retaining current-runtime behavior. There are no API or WebSocket payload changes.

## Limits and remaining audit

This task validates runtime identity, not decoder identity within one surviving runtime. Started still reads SQLite synchronously while holding an AppState read guard; that contention needs a separate scheduling change. Next checks in the broader event-ordering audit are TrackError recovery, asynchronous NearEnd preparation, and DJ follow-on dispatch across runtime replacement. Audible desktop playback and hotplug remain unverified. No installed app, user database, version, or dependency pin changed. Version remains 1.0.5. Follow-ups added: none.
