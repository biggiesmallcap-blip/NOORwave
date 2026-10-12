# Mismatch listen flush ownership handoff

Task: prevent a delayed terminal mismatch from flushing a replacement listen session after waiting for the state writer.

Root cause: terminal recovery checked its owner before reading persisted playback state. A mismatch then awaited a writer and flushed the active listen session without checking whether the runtime, generation or decoder had changed during that wait.

Change: revalidate runtime, generation and source after acquiring the mismatch writer. Hold the source mutex through session/history mutation and its history notification, with no await inside that protected section. The state writer serializes normal generation claims; the source mutex serializes decoder identity publication. Current-owner mismatch cleanup retains its existing behavior.

Before: the new real-listener test failed because a retired terminal consumed the listen session. It deliberately holds a state reader, waits until try_read fails due to the listener's queued flush writer, then replaces ownership before releasing that reader. This gates the mutation boundary rather than approximating it with a sleep.

After: the regression passes current-owner, replaced-decoder and replaced-generation cases. Current mismatch flush consumes the session, writes one history row and emits one history update. Both stale cases preserve the session/resume timestamp, write no history row and emit no history notification. An ordered public event provides the listener completion barrier.

Commit: c6efd5c6, fix(playback): bind mismatch listen flush to terminal ownership, in draft #355.

Validation:

- Red then green: terminal_mismatch_flush_rechecks_owner_after_waiting_for_writer.
- All 98 transport tests passed.
- Full backend suite: 2,175 passed, 11 ignored, zero failures, 269.80 seconds.
- All-target Clippy passed with existing advisory warnings; formatting and diff checks passed.

Acceptance status: done for stale mismatch-flush rejection at the writer boundary, preserving current flush behavior, guarding its history notification and shared-backend verification. Runtime ownership is checked alongside generation/source; the new writer-wait regression specifically exercises decoder and generation replacement. No callback, public HTTP/WebSocket contract or dependency changed. Performance remains unmeasured.

Outstanding: the initial terminal queue mutation still holds a shared AppState reader while generation::bump and claim_if_current also use readers. Code inspection confirms that lock contract permits a generation claim during the synchronous queue mutation; a dedicated reproduction is not yet executed. This is a separate current-plan boundary, not resolved by the mismatch writer fix.

Pending-row internals, successful snapshot listen synchronization, later skip/error mutations and generic failure publication also still need owner validation at their final mutation boundaries. Started/stall source ordering and SQLite/control-lock contention remain audit work.

Not verified: hardware output/recovery/hotplug matrix, audible output, reference CPU/memory/latency/callback metrics, bit-perfect capture and 8/24-hour endurance. The installed backend remained untouched. Fresh CI is pending at publication.

Next: make the initial queue mutation atomic with generation claims and verify that boundary, then protect snapshot listen synchronization and pending-row mutations. Version remains 1.0.5 in this draft; DJ feature work is deferred. Follow-ups added: none.
