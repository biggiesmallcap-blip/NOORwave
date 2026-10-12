# Terminal pending adoption ownership

Task: retain terminal runtime/decoder ownership through a pending resolver wait and background-row adoption.

Root cause: Finished retained runtime/source identity, but passed only generation into the pending loop. A same-generation decoder replacement during its busy-resolver wait could let the retired terminal adopt a background-resolved row and publish state changes. The handler's check after resolution rejected runtime switching but could not undo adoption.

Before: `terminal_pending_adoption_rejects_replaced_decoder` drove the real listener into a current pending row whose fresh resolver lock forced a busy wait. On the single-thread executor, the test waited for the persisted pending anchor, replaced the decoder, and promoted the row as a background resolver would. The baseline failed with "retired decoder adopted background resolution". No guessed sleep or native output was used.

Change: carry an optional expected runtime/source pair from terminal handling into the pending loop. Revalidate it on loop entry and after an unsuccessful adoption, so ownership rejection exits rather than falling through to skip. Owner-aware adoption acquires generation admission, reads AppState, locks decoder identity, and validates generation/runtime/source before its SQL mutation. The source guard remains held through snapshot loading and both notifications. Existing manual callers retain the generation-based path, without additional state reads for loop ownership checks.

After: the real-listener regression preserves the unresolved playback anchor, publishes no stale adoption event and dispatches no audio after decoder replacement. `pending_resolution_rechecks_owner_after_admission_wait` drives the production pending loop to a blocked adoption gate with a single future poll, then replaces the decoder or runtime. Rejection preserves the queue row and position and does not fall through to skip. The current-owner case still adopts track 1 and publishes exactly the expected two notifications.

Validation:

- Real-listener regression failed before implementation and passed afterward.
- All 104 transport tests passed; after strengthening the admission-wait test to exercise the pending loop, that focused test passed again.
- `cargo test -p noor-server --locked --quiet` on the final tree: 2,181 passed, zero failed, 11 ignored, 276.67 seconds.
- `cargo clippy -p noor-server --all-targets --locked`: passed with advisory warnings outside this patch, no pending-module warning.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Commit: `ed460ec117b46bd8ee3d21d82b036625e02a11e5`, `fix(playback): retain terminal owner through pending adoption`.

Acceptance status: done for retaining terminal identity through the busy loop, rejecting decoder/runtime replacement at adoption admission, preserving current adoption/publication, and preventing rejected adoption from entering skip fallback. Review checked the admission-before-state-before-source lock order, absence of awaits under the source guard, terminal wrapper wiring, and preservation of existing caller behavior. No callback, route, WebSocket, schema, dependency, version or release change.

Outstanding: loop checks are observations, not guards around every internal mutation. Foreground lookup promotion/current-state update, pending skip stepping, skip-limit stop/restore, final anchor changes during later awaits, manual session synchronization, delayed now-playing enqueue, generic failure publication and Started/stall source ordering remain audit work. Snapshot-load failure does not transactionally roll back an adoption write. Do not infer that the entire pending pipeline is ownership-safe.

Not verified: hardware shared/exclusive and device-failure matrix, captured bit-perfect audio, audible latency, CPU/memory/callback timing, stress targets and 8/24-hour endurance. Performance is not yet measured. Installed app and user data are untouched. Fresh CI is pending at publication. Overall audio hardening remains incomplete; follow-ups added: none.

Next: protect foreground pending promotion/current-state update with the retained owner at its actual mutation boundary, then pending skip/stop/restore writes. Keep those independently reproduced, tested and committed.
