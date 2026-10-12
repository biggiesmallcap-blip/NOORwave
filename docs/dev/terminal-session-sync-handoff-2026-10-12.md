# Terminal listen-session synchronization ownership

Task: prevent a retired Finished event from flushing or replacing a listen session after waiting for the successful snapshot synchronization writer.

Root cause: the terminal handler checked runtime, generation and decoder before calling `sync_session_after_snapshot`, but that function awaited an AppState writer and then mutated the session/history without revalidating ownership. The check after synchronization could reject runtime dispatch but could not undo an obsolete history mutation.

Before: `terminal_session_sync_rechecks_owner_after_waiting_for_writer` drove a real Finished listener through a valid queue-end mutation, held a state reader until the session writer was queued, and replaced the decoder. The baseline failed with "retired terminal synchronized the replacement listen session". It had already flushed the active session.

Change: add an owner-aware synchronization entry point, retaining the existing entry point for existing callers. Terminal handling passes its generation, runtime and decoder identity. Synchronization revalidates all three under the writer and holds the source guard through the synchronous session/history mutation. Publish ListenHistoryUpdated within that same guarded mutation block, avoiding a second writer-wait boundary before notification. Network/scrobble work remains outside the locks.

After: the same real-listener regression passes for decoder and generation replacement. The active session and its resume timestamp are preserved, with no history row or history event. The current-owner case reaches queue end, flushes once and publishes exactly one history update. The regression confirms the persisted queue has ended before injecting replacement, so it exercises successful snapshot synchronization rather than the mismatch-flush path. Runtime identity is checked in code; this new writer-wait regression does not directly replace the runtime.

Validation:

- Test-first regression: failed before implementation, passed afterward.
- `cargo test -p noor-server --locked server::transport:: --quiet`: 100 passed, zero failed.
- `cargo test -p noor-server --locked --quiet`: 2,177 passed, zero failed, 11 ignored, 273.47 seconds.
- `cargo clippy -p noor-server --all-targets --locked`: passed with advisory warnings outside this patch, no new listen-module warnings.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Commit: `2107588fec4128f2b16cfd93875d8e24eae7e4cd`, `fix(playback): validate terminal owner during listen session sync`.

Acceptance status: done for rejecting obsolete decoder/generation ownership at the successful terminal session writer, preserving the session/history on rejection, and retaining exactly-once current queue-end flush/publication. Review checked lock ordering, owner propagation from Finished through retry/queue resolution, no await under the source guard, and compatibility with existing ownerless synchronization callers. No route, WebSocket, schema, dependency, callback, version or release contract changes.

Outstanding: this is one boundary of the broader audio audit. Snapshot pause intent and queue-anchor freshness are not reloaded here. Manual-command synchronization still uses the existing ownerless path. Delayed now-playing scrobble enqueue, pending-row mutations, later skip/error mutations and Started/stall source ordering still need investigation. Restoring a failed flush before starting a new session remains an existing separate error-path concern. Do not infer all session races are fixed.

Not verified: audible output, hardware device/recovery/hotplug and format matrix, bit-perfect capture, reference CPU/memory/P95/P99/callback timing, and 8/24-hour endurance. Performance is not yet measured. The installed app and user database are untouched. Fresh CI is pending at publication. The overall goal remains incomplete; follow-ups added: none.

Next: reproduce pause intent changing while terminal session synchronization waits for its writer, then validate current queue anchor and intent at that mutation boundary. Keep that independently tested and committed.
