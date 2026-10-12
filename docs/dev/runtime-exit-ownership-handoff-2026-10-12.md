# Runtime exit ownership handoff

The exit listener removed the dead runtime, released AppState, and later reacquired it to pause playback, flush history, and publish failure. A replacement queued behind the first write lock could start in between. The old exit then paused that replacement and flushed its listen session.

Exit cleanup now validates the emitting runtime and performs removal, persisted pause, listen-session flush, and UI publication under one write guard. Completed scrobbles use the captured old session payload after releasing the guard. Stale exits return without state or UI effects. Persisted-pause and history failures are logged; a pause-write failure does not prevent clearing the dead output's state or reporting its exit.

## Acceptance checks

- Done: a queued replacement retains playing intent, runtime status, source metadata, queue, and its own listen session.
- Done: an already-retired exit leaves current playback and history untouched and emits no failure.
- Done: a current exit clears runtime, output activity, current and pending source display, and prebuffer ownership while preserving the queue anchor and rows.
- Done: current exit flushes real listen history and publishes history, playback failure, and state notifications.
- Done: a rejected SQLite pause write still permits dead-output cleanup and failure notification.
- Done: full backend suite and final review.

## TDD and verification

The queued-replacement regression failed before implementation because the old exit paused replacement playback. It drives the actual broadcast listener, holds an AppState reader, queues exit cleanup before replacement using Tokio's fair writer ordering, and installs a replacement with the same track and generation. Listener completion provides the boundary for checking the database, session identity, metadata, and absence of later UI events.

Positive listener tests cover an eight-second listen session, actual listen_history insertion, complete output-state cleanup, queue preservation, retired exit rejection, and a TEMP trigger that rejects pause persistence in a fresh test database. No production migration or user database is changed.

- `cargo test -p noor-server --locked runtime_exit -- --nocapture`: 5 passed.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo test -p noor-server --locked`: 2,171 passed, 11 ignored, zero failures, 260.76 seconds.

## Review and remaining work

Adversarial review checked lock acquisition ordering, same-generation replacement, stale failure publication, replacement listen-session preservation, queue retention, and the database error path. Scrobbling remains outside the state lock. No audio callback work, public API, or WebSocket payload changed.

Pause and history writes still run under AppState's write guard to preserve ownership. Broader SQLite contention remains audit work. If SQLite refuses the pause, persisted intent can remain playing even though the output is cleared; that failure is now logged and the existing runtime failure notification is still published. The test does not claim a failed write was persisted.

Nonretryable queue advancement and same-runtime decoder source identity remain current recovery work. Audible desktop recovery, rapid device switching, hotplug, non-Windows compilation, codec upgrade fixtures, and release-mode performance remain unverified. The broader audio goal is not complete. These are not moved to FOLLOWUPS.md.

DJ work remains deferred. Version remains 1.0.5. The installed app, user data, and dependency pins are unchanged. Follow-ups added: none.
