# Exclusive recovery scheduling handoff

The runtime event listener previously awaited the exclusive-release settings lookup before receiving another event. A contended database could delay a newer output engagement even though the lookup ran outside AppState on Tokio's blocking pool.

The listener now selects between runtime events and lookup completion. Each listener retains one active lookup and at most one pending request, replacing that pending request with the latest output identity. Duplicate requests for the active identity do not add work. Completed reads still revalidate the runtime and output identity before changing public status or enqueueing recovery. The existing actor-side paused check remains in effect.

Listener shutdown aborts blocking work that has not started and discards pending requests. A blocking read that has already started can finish, but its result cannot publish events or send commands after the listener has stopped.

## Acceptance checks

- Done: a locked database does not delay a newer exclusive engagement through the real runtime listener and public event stream.
- Done: a 1,000-request burst retains the original active task and only the latest pending identity; completion starts exactly that pending request.
- Done: listener cancellation completes while the database remains locked, with no later public release or recovery command.
- Done: current-output release, database errors, retired runtimes, and superseded output identities retain their existing behavior tests.
- Done: full backend tests, compilation, and formatting checks.

## Verification

- `cargo test -p noor-server server::transport --locked`: 62 passed.
- `cargo test -p noor-server --locked`: 2,146 passed, 11 ignored, zero failures, 244.57 seconds.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

## TDD and review

The new listener regression failed before implementation because the newer output event timed out behind the database lookup. It passes with the scheduler. The burst test exercises the scheduler's actual blocking tasks and database mutex.

Adversarial review checked cancellation safety of awaiting the retained JoinHandle in select, bounded work while SQLite is locked, duplicate requests, runtime replacement, output retirement, and visible effects after shutdown. Queries hold neither AppState nor the output identity mutex. Publication and command enqueue still occur under the validated runtime and output guards.

## Limits and remaining audit

This change addresses the exclusive recovery lookup. Other event handlers that perform synchronous database work can still delay the listener and need separate review. Audible playback, hotplug, and rapid device switching in a patched desktop build remain unverified. No installed app, user database, release version, or dependency pin changed in this task. The broader audio audit remains in progress. Follow-ups added: none.
