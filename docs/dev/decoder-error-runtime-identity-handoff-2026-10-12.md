# Decoder error runtime identity handoff

A delayed TrackError from a retired runtime could pass the playback-generation check when its replacement retained the same generation. The old error then cleared the replacement's activity and source metadata and entered queue recovery.

The actual runtime listener now passes its emitting handle into the error handler. The handler requires that identity and checks it alongside the generation after acquiring AppState's write guard, before any state mutation. Errors rejected at this boundary return without recovery, queue changes, UI notifications, or commands to the replacement runtime.

## Acceptance checks

- Done: terminal and transient errors waiting behind the state lock cannot affect a replacement runtime with the same generation and track.
- Done: rejected errors preserve audible activity, active track, last error, current source metadata, persisted queue position, playing intent, and queue length.
- Done: the real listener still handles current-runtime terminal failures and exhausted transient retries. Existing next-track recovery remains wired.
- Done: full backend verification and final review.

## TDD and review

The regression first failed because the retired decoder error cleared audio activity. It uses the real broadcast listener, a held AppState write guard, runtime replacement, a migrated SQLite database, and an ordered Exited event to wait for listener completion. Both retryable and terminal error messages exercise the protected entry path. Assertions also check that neither UI events nor replacement commands are emitted.

A positive listener test verifies that current terminal failure ends a single-row queue and that an exhausted transient retry pauses at that row. Queue rows remain available in both cases. The existing prepared-next-track recovery test now supplies its actual runtime handle; the repeated-transient test also installs a runtime instead of bypassing the identity requirement.

Adversarial review checked that validation happens after the contested lock, not in a separate preflight read, and that all production callers supply their emitting runtime. The change leaves event payloads and public API contracts intact. It adds no callback work or native audio-thread locking.

## Verification and remaining work

- `cargo test -p noor-server --locked track_error -- --nocapture`: 5 passed.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo test -p noor-server --locked`: 2,163 passed, 11 ignored, zero failures, 366.44 seconds.

This completes the stale-event entry guard, not the broader recovery ownership audit. A current error can pass this guard and then await stream resolution; protecting every later mutation and dispatch against replacement remains current audit work. Same-runtime replacement of a decoder also needs source identity review. These are not moved to FOLLOWUPS.md.

Audible desktop recovery, device switching, hotplug, non-Windows compilation, and release-mode performance remain unverified. DJ work is deferred at the user's request. The installed application and user database were not changed. Version remains 1.0.5. Follow-ups added: none.
