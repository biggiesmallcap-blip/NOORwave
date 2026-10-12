# Stale stream lookup generation handoff

Task: prevent a delayed unavailable-track lookup from skipping playback taken over by a newer transport command.

Root cause: start_track_inner mapped lookup errors into StartError::Stream before checking playback generation. Successful lookups were checked later. Thus a failed result from an obsolete generation could enter caller-side unplayable-track recovery and advance the replacement queue.

Change: validate the request's claimed or observed generation before interpreting the lookup result. Obsolete failures become Superseded, which existing callers handle without skip or pause. ClaimWhenReady still claims a new generation only after lookup success; current lookup failures leave the generation unchanged. The previous later claimed-generation read was removed, avoiding a duplicate read on the successful claimed-start path. No callback work, dependency pins or public API contracts changed.

Before: both new regression tests failed. The start test returned Stream instead of Superseded. A gated lookup driven through the real runtime listener cleared the new generation's current queue row, changing its ID from Some(2) to None.

After: both tests pass. The start test covers claimed starts and quality reissues. The real listener test replaces source/generation while the lookup waits, then checks queue state, playing intent, source metadata, audio activity, public notifications and runtime commands after the delayed rejection returns. An ordered public event provides the listener completion barrier.

Commit: 8e70fb40, fix(playback): reject lookup failures from superseded generations, on the runtime recovery draft #355.

Validation:

- Red then green: stale_lookup_failure_is_superseded_before_recovery and delayed_unplayable_lookup_cannot_skip_new_generation_queue.
- All 94 transport tests passed, including current unavailable-track errors, failed quality reissues, successful starts and paused intent.
- Full backend suite: 2,171 passed, 11 ignored, zero failures, 376.69 seconds.
- All-target Clippy passed with existing advisory warnings. Formatting and diff checks passed.
- Caller review confirmed Superseded returns without skip or pause in transport commands, terminal recovery and settings reissues.

Acceptance status: done for superseding failed lookup results returned after generation replacement, preserving current-error recovery, preserving quality-generation semantics and verifying the real listener path. Performance is not measured by these checks; test-suite duration is not a playback latency or CPU benchmark.

Outstanding: a command or same-generation decoder replacement can still land after result classification but before later caller mutations. Pending-row internals, mismatch listen flush, session synchronization, prepared snapshot dispatch and asynchronous dispatch require ownership checks at their mutation boundaries. This focused fix does not close all asynchronous recovery races. Native mode/device matrix, hotplug, audible recovery, reference latency/CPU/memory/callback measurements and 8/24-hour endurance remain in the active goal.

Next: retain runtime/source ownership in snapshot switch and validate it through delayed lookup success/failure and prepared dispatch. Then protect pending-row/listen mutations. No native device ownership was taken for this slice. Fresh CI remains pending at publication. Version remains 1.0.5; DJ feature work is deferred. Follow-ups added: none.
