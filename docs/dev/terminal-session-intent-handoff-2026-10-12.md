# Terminal session intent and queue anchor

Task: use current pause intent and reject obsolete queue anchors when terminal snapshot session synchronization acquires its writer.

Root cause: runtime/generation/decoder ownership can remain unchanged during a pause or a persisted queue-anchor update. Synchronization previously used the snapshot's captured playing flag and track after waiting for the writer. This could start an incoming track's running listen session after the user paused. Rejecting only the session mutation would also leave the terminal handler free to dispatch the rejected snapshot afterward.

Before: `terminal_session_sync_uses_current_intent_and_queue_anchor` drove Finished through a valid queue advance to track 2, held a state reader until session synchronization queued its writer, and called the existing player pause mutation. The baseline failed with "paused handoff started a running listen session". No guessed sleeps or user database/audio device were used.

Change: owner-aware synchronization reads persisted track ID, queue-row ID and playing intent under the AppState writer and decoder guard before any session/history mutation. Both track and queue-row anchors must match the captured snapshot. Read failure logs an error and rejects the handoff. Current playing intent controls whether a running incoming session is created. Synchronization returns an admission result, and the terminal handler stops before runtime switching when rejected. Existing ownerless callers retain their prior behavior.

After: the real-listener regression passes four cases: playing opens track 2's running session; paused flushes the completed track once without opening a running session; a changed track anchor preserves the existing session/history and dispatches no audio; a changed queue-row anchor with the same track also preserves the session/history and dispatches no audio. The test verifies queue advancement before injecting the change, then gates either on stream lookup entry or an ordered listener event barrier.

Validation:

- Test-first pause regression failed before implementation and passed afterward.
- `cargo test -p noor-server --locked server::transport:: --quiet`: 101 passed, zero failed, including prior owner and pause-during-lookup checks.
- `cargo test -p noor-server --locked --quiet`: 2,178 passed, zero failed, 11 ignored, 385.89 seconds. A slow-test warning was observed; the same process completed successfully without restart.
- `cargo clippy -p noor-server --all-targets --locked`: passed with advisory warnings outside this patch.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Commit: `48468a4b462f105eda6d40abbf3a395cf58d1c1c`, `fix(playback): revalidate terminal session intent and queue anchor`.

Acceptance status: done for current playing session creation, pause-aware session synchronization, and rejecting changed track or queue-row anchors before history mutation and subsequent runtime dispatch. Review confirmed that the writer excludes persisted control readers and generation mutation, the decoder guard spans only synchronous work, and downstream start still reads current intent. No callback, HTTP/WebSocket, dependency, schema, version or release change is included. Performance is not yet measured.

Outstanding: anchor validation applies at the session writer boundary. Anchor changes during later awaits, pending-row internals, manual-command session synchronization, delayed now-playing scrobble enqueue, later skip/error mutation/publication, and Started/stall source ordering remain audit work. Existing session restoration after a failed flush can still be overwritten by later session creation. Do not infer that all recovery or session races are resolved.

Not verified: hardware output, occupied/disconnected devices, shared/exclusive format matrix, captured bit-perfect audio, audible latency, CPU/memory/callback metrics, stress targets and 8/24-hour endurance. The installed app and user data remain untouched. Fresh CI is pending at publication. Overall audio hardening remains incomplete; follow-ups added: none.

Next: trace pending-row adoption and stepping, reproduce a superseding generation/source at its actual SQL mutation boundary, and protect that boundary independently. Then re-audit final snapshot dispatch for anchor changes during later asynchronous work.
