# Snapshot dispatch ownership handoff

Task: retain terminal recovery ownership through resolved and prepared snapshot dispatch.

Root cause: terminal advancement checked runtime/source ownership before calling snapshot switch, but snapshot switch discarded that identity. A delayed lookup could switch a replacement decoder or runtime using the same playback generation. Prepared status, pause intent, dispatch, metadata and events were also separated by lock/await boundaries.

Change: pass the retained runtime/source into snapshot switching. Owned recovery uses the existing start_track_for_runtime path for resolution, final dispatch, metadata and public events. It reuses its validated runtime rather than acquiring a replacement. Prepared switches validate generation/runtime/source while holding the state writer and source mutex, re-read pause intent there, and publish metadata/events with dispatch. Empty-snapshot metadata is guarded too. Owned dispatch does not publish a second state update after those guards are released. The transport-intent helper now accepts an already-locked AppState so dispatch does not reopen the state lock.

Before: the new real-listener regression failed because the delayed lookup dispatched a Switch after a same-runtime decoder replacement.

After: the same regression passes all four success/rejection and runtime/decoder replacement cases. A prepared-status regression replaces the decoder before replying Prepared and verifies no Switch or TrackChanged. Current resolved recovery publishes exactly one TrackChanged/PlaybackStateChanged/QueueUpdated sequence and honors pause during lookup. Existing prepared-promotion and terminal-end paths remain covered by the passing transport suite.

Commit: f2798223, fix(playback): retain terminal ownership through snapshot dispatch, on draft #355.

Validation:

- Red then green: terminal_snapshot_lookup_cannot_modify_replacement_owner through the real runtime listener and a gated stream adapter.
- 10 focused terminal tests passed, including prepared-status replacement and valid terminal recovery.
- All 97 transport tests passed, including the new current-recovery publication/pause test.
- Full backend suite: 2,174 passed, 11 ignored, zero failures, 380.63 seconds.
- All-target Clippy passed with existing advisory warnings; formatting and diff checks passed.

Acceptance status: done for obsolete resolved dispatch/publication, prepared dispatch ownership, current recovery publication and paused intent, and shared-backend verification. The source mutex is control-side only, held through synchronous validation/dispatch/publication and never across an await. No audio callback or public HTTP/WebSocket contract changed. No latency/CPU improvement is claimed; performance remains unmeasured.

Outstanding: pending-row internals, state-mismatch listen flush, session synchronization, later skip/error mutations and generic failure publication still need ownership checks at their final mutation boundaries. Same-generation source changes after a valid error classification remain possible there. Started/stall source ordering, SQLite contention and control-lock timing also remain audit work. These are current-plan items, not deferred outside-scope follow-ups.

Not verified: audible output, hotplug, native output matrix, P95/P99/CPU/memory baselines, bit-perfect capture and 8/24-hour endurance. The installed backend remained active and untouched. Fresh CI is pending at publication.

Next: protect pending-row and listen-session mutations, then review later skip/error publication against runtime/source identity. Version remains 1.0.5; DJ feature work is deferred. Follow-ups added: none.
