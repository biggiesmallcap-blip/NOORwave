# Terminal queue generation admission

Task: prevent a new playback generation from racing an already admitted terminal queue write while preserving access to pause/resume runtime controls.

Root cause: `handle_runtime_finished_for_owner` validated generation under an AppState reader, but `generation::bump` and `claim_if_current` also used readers. A newer generation could be admitted while the old Finished handler was blocked inside SQLite and subsequently changed the queue.

Before: `terminal_queue_write_serializes_generation_claim` blocked the real listener's SQLite write using an isolated temporary WAL database, a competing `BEGIN IMMEDIATE`, and SQLite's busy callback. Polling the public generation helper returned a new generation while the old write remained blocked. The regression failed with "new generation was admitted while the old terminal queue write was blocked".

Change: add a dedicated asynchronous admission mutex to AppState. Both generation mutation helpers and the initial terminal queue mutation acquire it before their AppState reader. Generation mutation retains its AppState reader so existing writer-protected final dispatch remains ordered. The terminal block retains runtime/source validation and its source identity guard through queue mutation. The gate does not span pending resolution, session synchronization, or runtime switching. All AppState constructors initialize it; the existing DJ test constructor receives initialization only, with no DJ feature change.

A broader AppState writer approach was rejected after the control-availability assertion failed. The dedicated gate keeps state readers available during the SQLite wait. The final fixture retains a healthy fake runtime command receiver; a dropped receiver would invalidate the control-availability observation.

After: the listener regression passes. User and quality generation claims wait during the real queue write, runtime controls remain retrievable, the queue reaches its expected ended state, a stale quality claim fails afterward, and a current quality claim succeeds. The test uses an actual SQLite boundary and single future polls rather than guessed sleeps. It creates and removes only its own temporary database files and does not use the installed database or audio device.

Regression checks:

- `cargo test -p noor-server --locked server::transport:: --quiet`: 99 passed, zero failed.
- `cargo test -p noor-server --locked --quiet`: 2,176 passed, zero failed, 11 ignored, 272.07 seconds.
- `cargo clippy -p noor-server --all-targets --locked`: passed with advisory warnings outside this patch.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Commit: `e82c549a2bff479aaa240b1329cd108decc2bb9b`, `fix(playback): serialize terminal queue writes with generation claims`.

Acceptance status: all five software checks above are done through the real Finished listener and generation helpers. The adversarial review checked lock ordering, initializer coverage, generation mutation sites, canceled quality waiters, stale rejection after release, and compatibility with writer-protected dispatch. No HTTP, WebSocket, schema, dependency, version, callback, or release contract changes are included.

Outstanding: this proves ordering and control-handle availability, not audible pause/resume latency or a performance improvement. Hardware playback, P95/P99 latency, CPU, callback timing and endurance remain unmeasured. Pending-row mutations, successful snapshot listen synchronization, later skip/error mutations and Started/stall source ordering remain current audit work. The overall audio hardening goal is incomplete.

Next: guard successful terminal snapshot listen synchronization at its writer boundary and reproduce stale owner or pause intent changes while waiting for that writer. Keep that separate from this commit.
