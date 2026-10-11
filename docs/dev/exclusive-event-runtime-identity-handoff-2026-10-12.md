# Exclusive event runtime identity handoff

ExclusiveModeEngaged, ExclusiveModeFailed, and ExclusiveModeReleased now
validate their originating runtime after acquiring the state write lock.
Retired listeners cannot change the replacement runtime's exclusive status
or transport format, emit misleading public notifications, or trigger a
DeviceSwap on the replacement handle.

Release retry is queued while the validated state lock is still held. Sending
the command is synchronous and does not wait for native device work. This
prevents runtime replacement between validation and enqueue. The existing
playing/requested-exclusive policy remains enforced.

The listener returns its task handle, allowing tests to await actual ordered
event consumption and completion. The production caller drops that handle
to retain detached listener execution.

Acceptance checks:

- Done: retired engaged, failed, and released notifications leave current
  metadata unchanged and emit no public events.
- Done: a stale release cannot enqueue recovery on the replacement runtime,
  even when the DB says playing and exclusive output is requested.
- Done: replacement during state-lock contention is checked after the wait;
  the test awaits the real listener's subsequent Exited event and completion.
- Done: current notifications preserve device, diagnostic, and transport-format
  data through the actual public listener.
- Done: only a current release while playing with exclusive requested queues
  an exclusive DeviceSwap; paused and shared-preference cases do not retry.
- Done: full server regression suite passed: 2128 passed, 11 ignored, 0 failed (290.50 seconds).

TDD: the real-listener regression first failed with "retired engaged changed
current engagement", then passed for all three notifications. Both targeted
listener tests passed. cargo check -p noor-server --locked,
cargo fmt --all -- --check, and git diff --check passed.

Adversarial review checked current-runtime removal/replacement, state-lock
ordering, synchronous command enqueue, discarded task-handle semantics,
listener completion, and retry suppression after pause or exclusive disable.
No native renderer, callback, API, WebSocket shape, dependency, or release
changes. Native output and audible playback were not rerun for this listener
change; the actual event path was exercised with isolated DB/runtime fixtures.
Previous task: 35121298. Version remains 1.0.5.

Remaining overall-goal work: notifications from superseded exclusive output
instances within the same runtime still need identity protection. Also review
same-decoder control ordering and runtime-wide events, evaluate staged decoder
and resampler upgrades, and verify patched desktop audible recovery/hotplug.
No current-goal work was moved to FOLLOWUPS.md.
