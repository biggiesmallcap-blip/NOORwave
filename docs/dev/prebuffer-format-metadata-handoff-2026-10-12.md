# Prebuffer format metadata handoff

The near-end path skipped preparation for valid upcoming rate changes, but then had a separate device-swap branch that cast raw signed rate metadata to u32. Zero or negative values bypassed the skip policy and could enqueue an invalid device rate while the outgoing track was still playing. Pending stream metadata also published those values as source rates.

The prebuffer path no longer issues device swaps. It preserves the existing policy that defers valid format changes to track start and treats nonpositive rates as unknown. Its pending source-rate field now uses the same positive-value rule.

The policy is also called when a rate is missing. A known exclusive bit-depth change now defers preparation even when rate metadata is absent or invalid. Shared mode can still prepare at the existing output format in that case.

## Acceptance checks

- Done: negative and zero rates cannot enqueue a prebuffer device swap in shared or exclusive mode.
- Done: pending source metadata reports invalid rates as unknown, while valid matching rates remain intact.
- Done: valid rate changes still defer preparation in both modes, and matching rates still reach PrepareNext.
- Done: missing, zero, negative, and matching rates do not bypass a known exclusive bit-depth change; shared preparation remains available.
- Done: full backend suite and final review.

## TDD and verification

Both invalid-rate regressions failed before implementation because the first command was a device swap. The unknown-rate regression also failed before implementation because a known exclusive bit-depth change reached preparation. Tests exercise the real near-end transport path, stream-resolution seam, outgoing format information, saved audio settings, runtime command channel, and pending metadata.

- `cargo test -p noor-server near_end_ --locked`: 13 passed.
- `cargo check -p noor-server --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo test -p noor-server --locked`: 2,161 passed, 11 ignored, zero failures, 366.16 seconds.
- `pnpm exec vitest run src/lib/player/stream_display.test.ts`: 5 passed. The shared player formatter accepts an unknown rate without substituting the output device's rate.

## Review and remaining audit

Reference checks covered source, frontend, scripts, docs, settings routes, and device-swap callers before removing the branch. Only the prebuffer-specific swap was removed. Output settings, explicit device switching, and runtime transition code retain their existing paths. No API or WebSocket fields changed.

Adversarial review checked signed-to-unsigned conversion, absent metadata, policy consistency for shared versus exclusive mode, rate and bit-depth combinations, unchanged matching-rate preparation, outgoing-stream protection, and pending metadata publication. The change does not claim to validate every possible upstream metadata value or prove decoder-header/native-format reconciliation.

Audible desktop transitions, rapid device switching, hotplug, non-Windows compilation, and release-mode performance remain unverified. Known exclusive bit-depth changes with unknown rates now use the existing cold-start transition path, so audible boundary behavior still needs testing. Other current audit work includes TrackError recovery, DJ follow-on dispatch, SQLite contention, decoder/header format refinement, and broader metadata validation. No installed app, user database, version, or dependency pin changed. Version remains 1.0.5. Follow-ups added: none.
