# Runtime-bound transient retry handoff

Transient decoder recovery used the general snapshot-switch helper. A retry could resolve a stream for the failed runtime, then acquire its replacement and send that old recovery job to the new runtime. A failed lookup could also pause the replacement. The general helper's unplayable-track skipping policy could advance the queue during a supposedly current-row transient retry.

Current-row retries now use the shared track-start primitive with an explicit originating runtime. A replaced runtime makes both successful and failed lookup results superseded. Retry failure and exhaustion revalidate runtime and generation under the same AppState write guard used for the persisted pause, listen-session flush, and UI failure publication. Completed scrobbles are enqueued after releasing that guard.

The shared start primitive now validates its final generation and acquired runtime, rereads playing intent, dispatches, and records stream metadata under one write guard. A newer command arriving while runtime acquisition waits cannot dispatch an old job or publish its display. Runtime-bound retry success updates active-track/error status and publishes the existing playback notifications in that same guarded operation.

## Acceptance checks

- Done: delayed retry success cannot switch or overwrite a replacement runtime with the same track and generation.
- Done: delayed retry failure cannot pause that replacement or publish a stale failure.
- Done: a newer generation supersedes success and failure without changing its metadata or persisted playing intent.
- Done: a current retry resolves a fresh stream for the same row and respects a pause made during resolution.
- Done: failed retry lookup, including a catalog rejection, pauses at the current row and preserves the queue rather than using the general skip policy.
- Done: shared start dispatch rejects a generation change while runtime acquisition waits; normal starts and quality reissues still work.
- Done: full backend suite and final review.

## TDD and verification

The first regression failed before implementation because a delayed retry sent Switch to the replacement runtime's command channel. It now tests both successful and failed lookups with the real recovery handler, the shared stream-resolution path, an asynchronous gate, migrated SQLite state, and distinct runtime handles. A subsequent Pause command provides a FIFO barrier through the fake actor thread so absence of earlier dispatch is checked deterministically.

Positive tests verify fresh resolved-stream jobs, source metadata, runtime status, playback notifications, queue position and row count, lookup failure, catalog rejection, and paused dispatch. New-generation tests cover successful and failed resolution. The runtime-acquisition contention test was added during adversarial review after the main regression fix; it uses the actual start primitive and a held state reader to make the generation change land before dispatch.

- `cargo test -p noor-server --locked server::transport:: -- --nocapture`: 82 passed before the additional start contention test.
- `cargo test -p noor-server --locked server::transport::start:: -- --nocapture`: 11 passed, including that additional test.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo test -p noor-server --locked`: 2,167 passed, 11 ignored, zero failures, 306.42 seconds.

## Review and remaining audit

Reference checks verified that only transient current-row recovery uses the runtime-bound start entry point. Ordinary transport starts retain runtime acquisition and respawn behavior. No public API or WebSocket payload changed. Network resolution and scrobbling remain outside the dispatch state lock; there is no new audio callback work.

Adversarial review covered stale successes, stale failures, generation changes during acquisition, paused intent, current-row queue preservation, failed command sends, metadata publication ordering, and unchanged normal start/reissue behavior. The dispatch lock now contains a small SQLite intent read, consistent with the existing intent lookup but serialized with state changes. Broader SQLite contention remains current audit work.

This resolves transient retry ownership across stream lookup. Runtime-exit cleanup is subsequently covered by [the exit ownership handoff](runtime-exit-ownership-handoff-2026-10-12.md). Nonretryable queue advancement and same-runtime decoder replacement still need ownership review. Audible desktop recovery, rapid device switching, hotplug, non-Windows compilation, codec upgrade fixtures, and release-mode performance remain unverified. These remain current audit work rather than FOLLOWUPS.md entries.

DJ work remains deferred at the user's request. The installed app, user database, version, and dependency pins were not changed. Version remains 1.0.5. Follow-ups added: none.
