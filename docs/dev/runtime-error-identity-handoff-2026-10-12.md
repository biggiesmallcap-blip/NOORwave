# Runtime error identity handoff

Generic Error events now pass their originating runtime handle from the real
listener into the error consumer. The consumer checks identity after acquiring
the state write lock. A retired runtime cannot pause its replacement, clear
its activity flag, overwrite error metadata, flush its listening session, or
emit a misleading failure notification.

For an accepted error, DB pause, audio activity, runtime metadata, listening
session flush, and public notifications now share that lock. Scrobble dispatch
remains outside it. The direct local-unsupported error path still uses the same
consumer without a runtime constraint. No public API or WebSocket shapes changed.

Acceptance checks:

- Done: replacement while the consumer waits for the lock rejects the old
  runtime error, leaving the playing queue row, activity, and metadata intact.
- Done: rejected errors emit no public notifications.
- Done: active-runtime errors still reach the actual listener, pause DB state,
  clear audio activity, retain the diagnostic, and preserve queue contents.
- Done: persistence and notifications run under the same validated lock;
  external scrobble work runs after release.

TDD: the replacement interleaving regression first failed with "retired runtime
error was applied", then passed. All 51 server::transport tests passed.
cargo check -p noor-server --locked, cargo fmt --all -- --check, and
git diff --check passed. The regression drives the exact consumer used by
the listener; the active-runtime test publishes through the actual listener.

Adversarial review checked runtime removal/replacement, DB pause errors, session
flush errors, event ordering, and preservation of the direct error caller.
On DB failure, the diagnostic still surfaces and audio telemetry clears; the
failure is logged, without claiming persistence succeeded. No UI template or
native output code changed, so no additional desktop or native smoke was run
for this task. Previous task: 49fb1bf7. Version remains 1.0.5.

Remaining overall-goal work: generic Error currently has runtime identity but
no per-command generation or decoder identity. Later commands on the same
runtime need a separate ordering review. Also review whether every generic
runtime command failure actually pauses the audio graph, cold decoder recovery
after poisoned PCM, decoder/resampler upgrades, and audible hotplug/recovery.
These are current-goal work, not FOLLOWUPS.md entries.
