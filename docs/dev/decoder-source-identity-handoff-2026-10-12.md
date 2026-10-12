# Decoder error source identity handoff

The runtime actor validated decoder terminals by stop identity, but its TrackError notification carried only track and generation to the asynchronous listener. A decoder restart in the same runtime could reuse both, allowing the old error to clear fresh activity and source metadata or enter recovery.

TrackError now retains the failed deck's source identity before teardown. The listener validates that identity under the existing control-side source lock after checking runtime and generation. Transient retry validates it again after stream lookup, at final dispatch and metadata publication, and before fallback pause/history/UI effects. A lookup result from an old decoder becomes superseded even if runtime, track and generation remain unchanged.

## Acceptance checks

- Done: actor publication retains the failed deck's identity after teardown, independently of the current source reader.
- Done: the real listener rejects terminal and transient errors from a retired decoder within the same runtime and generation.
- Done: delayed retry success and failure cannot overwrite or pause a same-runtime decoder restart.
- Done: current decoder errors still reach next-track recovery and exhausted-retry pause; normal retries and paused intent remain wired.
- Done: full backend suite and final review.

## Verification

The listener regression failed before implementation because the old decoder error cleared fresh audio activity. It now uses the actual broadcast listener, a held state writer, source replacement, migrated SQLite state, and an ordered public output event as a completion barrier. The retry regression gates actual stream resolution and replaces the source while it waits; both successful and failed results are checked against command, metadata, activity, queue, and UI effects.

- `cargo test -p noor-server --locked track_error -- --nocapture`: 7 passed.
- `cargo test -p noor-server --locked transient_retry_resolution -- --nocapture`: 4 passed.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo test -p noor-server --locked`: 2,174 passed, 11 ignored, zero failures, 263.05 seconds.

## Review and remaining audit

The retained Arc prevents identity reuse while an error is queued. Validation compares pointer identity rather than the changing counter value. Runtime checks precede source-lock acquisition, and lock order follows existing command-error validation. The guard is held through the validated state mutation or dispatch and never across asynchronous resolution or scrobbling. No audio callback work or public API/WebSocket payload changed.

This covers decoder-error entry and transient retry. Nonretryable queue advancement still needs ownership validation across its later asynchronous operations. Started, stall notifications, SQLite contention, audible device switching and recovery, hotplug, non-Windows compilation, codec upgrade fixtures, and release-mode performance remain audit work. The broader goal remains active.

DJ work is deferred. Version remains 1.0.5. The installed app, user data, and dependency pins are unchanged. Follow-ups added: none.
