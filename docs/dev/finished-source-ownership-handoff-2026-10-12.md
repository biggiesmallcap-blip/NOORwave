# Finished source ownership handoff

Task: reject delayed terminal notifications from a retired decoder, while preserving queue handoff after prepared-track promotion.

Root cause: Finished retained only track and playback generation across the runtime/listener queues. A decoder restart can reuse both. The listener could clear fresh audio activity and advance its persisted queue. Nonretryable TrackError recovery also dropped the already-validated source identity before entering queue advancement.

Change: Finished retains the source owning the queue handoff. Ordinary EOF uses the finished deck; prepared promotion uses the already-active incoming deck. The listener validates runtime, generation and source while updating activity. Terminal advancement retains ownership across database retries and checks it before the initial queue mutation and after pending resolution and listen-session synchronization. The source mutex covers the initial queue mutation and is never held across an await. Existing playback callbacks and public HTTP/WebSocket contracts are unchanged.

Before: same-runtime, same-generation Finished events had no decoder ownership check. An intermediate uncommitted implementation tagged the outgoing deck during promotion; both actor promotion regressions failed on that identity mismatch.

After: three real-listener tests pass for retired decoder rejection, current EOF advancement, and already-promoted prepared-track advancement. Both promotion tests now verify the incoming deck identity. The runtime terminal test verifies ordinary EOF retains the finished deck identity. The nonretryable current-error listener remains covered by the existing passing recovery test.

Regression checks:

- Full backend suite: 2,169 passed, 11 ignored, zero failures, 257.36 seconds. This completed before a final equivalent promotion cleanup replaced an extra mutex read with a direct Arc clone from the incoming deck.
- Final source: both promotion tests and all three Finished listener tests passed after that cleanup.
- All-target Clippy passed on the final source with existing advisory warnings; formatting and diff checks passed.
- The initial decoder-finished listener test was added during implementation rather than run against an untouched baseline. Promotion identity checks were explicitly observed failing before correction and passing afterward.

Commit: 2a683d55, fix(playback): bind terminal queue advances to decoder ownership. Published in the runtime recovery draft #355.

Acceptance status: done for retaining terminal identity, stale listener rejection, current EOF advancement, and prepared promotion handoff; regression checks passed. This is an incremental ownership fix, not completion of the audio hardening goal.

Outstanding: checks around pending-row internals, state-mismatch listen flush, listen-session synchronization and final stream resolution/dispatch are not yet atomic with decoder ownership. A source replacement after initial advancement can still affect these later paths. Started/stall source ordering, SQLite contention, reference latency/CPU/memory/callback measurements, the full native format/device matrix and 8/24-hour endurance remain in the active plan. No measured performance improvement is claimed.

Not verified: audible playback, occupied-device handling, hotplug or native smoke on this change. The user's backend process was active, so hardware ownership was left intact. Fresh CI for this commit is pending at publication.

Next: carry ownership into each later asynchronous mutation and dispatch, with gated resolver/listen tests. Establish reference-system measurement baselines before further performance changes. DJ feature work remains deferred; promotion compatibility was checked only to preserve existing playback continuity. Version remains 1.0.5. Follow-ups added: none.
