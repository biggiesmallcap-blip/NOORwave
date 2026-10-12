# Pending adoption generation admission

Task: order adoption of a background-resolved current queue row with new user and quality generations.

Root cause: `adopt_resolved_current_queue_item` checked generation, released its state reader, updated playback_state through a cloned Database, then checked generation again. A new generation could be admitted during the SQL write. The late check returned no result but could not undo an obsolete adoption.

Before: `pending_adoption_serializes_generation_with_sql_write` used an isolated temporary WAL database, a competing `BEGIN IMMEDIATE` and a connection-local SQLite busy callback to block the actual adoption update. Polling `generation::bump` admitted a new generation during that write. The test failed with "new generation was admitted during pending adoption's SQL write". No user database or hardware device was used.

Change: acquire the existing playback generation admission gate, retain an AppState reader, validate generation, execute adoption and load its snapshot in the same connection closure, then publish TrackChanged and PlaybackStateChanged before releasing admission. There is no asynchronous lookup or await after admission while mutation/publication is in progress. Snapshot load errors propagate before notifications. Existing conditional SQL still requires the same current queue row and a NULL current track.

After: the regression passes. User and quality generation claims wait during the SQL write. Current adoption returns the committed track/queue-row snapshot with position zero and exactly two notifications. After the waiting generation is admitted, an old-generation adoption leaves the unresolved track, saved position and notification stream unchanged. The test uses actual SQLite entry and single future polls, without guessed sleeps. Its callback is private to this test's connection and static slot; temporary-file cleanup removes only its own database, WAL and SHM files.

Validation:

- Test-first regression failed before implementation and passed afterward.
- `cargo test -p noor-server --locked server::transport:: --quiet`: 102 passed, zero failed.
- `cargo test -p noor-server --locked --quiet`: 2,179 passed, zero failed, 11 ignored, 390.30 seconds. A slow-test warning occurred; the same process completed successfully without restart.
- `cargo clippy -p noor-server --all-targets --locked`: passed with advisory warnings outside this patch and no pending-module warning.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Commit: `e3ee517305b1ce49dab5a0804f768e6cc70e65c5`, `fix(playback): serialize pending adoption with generation claims`.

Acceptance status: done for user/quality claim exclusion during adoption, current snapshot/publication behavior, and stale-generation rejection without mutation or notification. The test drives the production adoption helper used by pending resolution. Broader transport checks cover existing terminal and previous-navigation pending flows. Review checked the admission-before-reader lock order, preservation of the state reader needed by writer-protected dispatch, early-return/error guard release, retained SQL anchor conditions and absence of awaits inside the admitted mutation. No callback, route, WebSocket, schema, dependency, version or release changes.

Outstanding: this fixes generation ordering at adoption, not decoder ownership through the pending pipeline. A same-generation source replacement remains an unprotected case. Foreground resolution promotion/state adoption, skip stepping, skip-limit stop/restore, later anchor changes, manual session synchronization, delayed now-playing enqueue and Started/stall source ordering still need audit and guards. Snapshot load errors propagate but do not transactionally roll back the preceding valid adoption write; no broader transaction guarantee is claimed.

Not verified: audible latency or contention cost, CPU/memory/callback timing, hardware shared/exclusive and device failure matrix, bit-perfect capture, stress targets and 8/24-hour endurance. Performance remains unmeasured. Installed app and user data are untouched. Fresh CI is pending at publication. Overall audio hardening is incomplete; follow-ups added: none.

Next: carry expected runtime/source identity into terminal pending resolution and adoption, reproduce source replacement at its mutation boundary, then protect pending skip/stop/restore mutations separately.
