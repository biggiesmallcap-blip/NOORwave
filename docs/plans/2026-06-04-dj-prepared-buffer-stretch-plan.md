# DJ Prepared-Buffer Stretch Runtime Plan

Date: 2026-06-04

## Summary

The Signalsmith evaluation in `docs/plans/2026-05-26-smart-stretch-evaluation.md`
proved the candidate can keep synthetic click drift, output length, finite
samples, and peak level inside the objective quality gate. It did not prove a
runtime path: release-mode 90s renders still take roughly 1.4 to 1.7 seconds,
which fails the original 500 ms gate by about 3x.

This plan replaces that 90s runtime gate with a prepared-buffer gate. The direct
runtime `PlaybackRate` cap remains `0.97..1.03` in `noor-mix/src/program.rs`,
`noor-mix/src/planner/mod.rs`, and `noor-mix/src/planner/safety.rs`. Wider tempo
sync is allowed only when the incoming deck has already been rendered into a
validated pitch-preserving buffer before the transition fire window.

## Non-Negotiables

- Do not run Signalsmith, decode, allocate unbounded buffers, hit the DB, log, or
  lock shared state inside the realtime callback.
- Do not widen direct `PlaybackRate` validation beyond `0.97..1.03`.
- Do not change queue promotion, overlay cleanup, WASAPI exclusive output, or
  sidecar behavior as part of the first prepared-stretch slice.
- Do not use Rubber Band without license review.
- Keep the feature default off until objective metrics and runtime fallback tests
  pass on the target listening machine.

## Owning Surfaces

- Evaluation harness: `noor-mix/src/stretch_eval.rs`
- Direct playback-rate cap: `noor-mix/src/program.rs`,
  `noor-mix/src/planner/mod.rs`, `noor-mix/src/planner/safety.rs`
- Current mixer path: `noor-mix/src/render.rs`
- Runtime preparation and fallback: `noor-server/src/playback/runtime/mod.rs`
- Transition planning and metadata: `noor-server/src/playback/player.rs`,
  `noor-server/src/server/routes/dj_routes.rs`
- Existing decision record:
  `docs/plans/2026-05-26-smart-stretch-evaluation.md`

## Runtime Deadline Gate

The old 90s-under-500ms gate is too strict for full-window rendering and too
loose for a real transition deadline. The new gate is based on the segment that
would actually be handed to the prepared mixer.

Candidate segment sizes:

- 8 bars at 120 BPM: roughly 16 seconds.
- 16 bars at 120 BPM: roughly 32 seconds.
- 32 bars at 120 BPM: roughly 64 seconds.

Release-mode target-machine gate:

- 16-bar prepared stretch at 5 percent and 8 percent must finish in 750 ms p95.
- 32-bar prepared stretch at 5 percent and 8 percent must finish in 1500 ms p95.
- A render attempt must be marked failed if it cannot finish at least 2 seconds
  before the planned transition fire position.
- Missed, late, invalid, stale, or missing prepared stretch buffers must fall
  back to `SafeCrossfade` or the existing 3 percent small-nudge path.

Quality gate for every prepared render:

- Output has no non-finite samples.
- Output length error is at most 1 frame.
- Peak stays at or below `0.98` before entering the existing mixer limiter.
- 5 percent stretch stays under 10 ms maximum click phase drift.
- 8 percent stretch stays under 20 ms maximum click phase drift.
- Musical fixtures have no documented transient smear or vocal artifact severe
  enough to reject the transition in manual notes.

## Implementation Sequence

1. Extend the evaluation harness, not runtime:
   - Add drum, pad, full-band, and vocal fixture support beside the click track.
   - Add a release benchmark mode that reports 8, 16, and 32 bar windows.
   - Keep `signalsmith-eval` optional and evaluation-only.

2. Add a `noor-mix` prepared-stretch API behind a feature flag:
   - Input: decoded interleaved samples, sample rate, channels, source markers,
     tempo ratio, and requested segment window.
   - Output: validated `PreparedStretchBuffer` plus `StretchEvaluationReport`.
   - Failure: structured reason for late render, invalid ratio, invalid length,
     non-finite output, peak, drift, or renderer error.

3. Add server-side preparation without callback changes:
   - Decode current and incoming deck audio before the fire window.
   - Render and validate the incoming stretched segment in the existing
     lookahead/preparation path.
   - Store the result with queue generation, current queue item id, next queue
     item id, current track id, next track id, sample rate, channels, and
     planned fire sample.
   - Discard the buffer if any identity or queue-generation check changes.

4. Integrate with the prepared mixer:
   - Feed the pre-rendered deck B buffer into the existing `PreparedDjMixer`.
   - Use `PlaybackRate` 1.0 for the stretched buffer itself.
   - Preserve the existing direct small-nudge behavior for ratios inside
     `0.97..1.03`.
   - Leave legacy overlap and `SafeCrossfade` fallback untouched.

5. Add telemetry before widening behavior:
   - Record `stretch_renderer`, `stretch_ratio`, `stretch_window_bars`,
     `stretch_render_ms`, `stretch_deadline_ms`, `stretch_status`,
     `stretch_failure_reason`, and `stretch_report`.
   - The cockpit may show these as renderer facts only after the runtime path
     actually uses a prepared stretch buffer.

## Required Tests

- `cargo test -p noor-mix stretch_eval`
- `cargo test -p noor-mix --features signalsmith-eval stretch_eval`
- New `noor-mix` tests for prepared-stretch length, finite samples, peak, drift,
  invalid ratio, and missed deadline handling.
- New `noor-server` runtime tests that prove:
  - a valid prepared stretch buffer is consumed by the prepared mixer;
  - stale queue generation discards the buffer;
  - late render falls back without silence;
  - missing prepared stretch never widens direct `PlaybackRate`;
  - telemetry reports rendered vs fallback state accurately.
- Existing cap regression tests:
  - `cargo test -p noor-mix tempo_nudge`
  - `cargo test -p noor-mix drop_preview_16_program`
  - `cargo test -p noor-mix rate_out_of_range`
  - `cargo test -p noor-mix audio_safety`

## Manual Gate

Before enabling wider prepared stretch outside local development:

- Run 10 compatible electronic handoffs at 5 percent.
- Run 10 compatible electronic handoffs at 8 percent.
- Run 5 vocal-heavy handoffs at 5 percent.
- Run 5 incompatible pairs that must fall back.
- Confirm no callback underruns, no queue-promotion drift, and no misleading
  cockpit labels.

## Exit Criteria

Prepared stretch can move from planning to implementation only when:

- The fixture benchmark passes the runtime deadline and quality gates above.
- The implementation plan keeps all renderer work before the callback.
- Fallback is tested for stale identity, late render, invalid output, and missing
  buffer.
- The direct `PlaybackRate` cap remains unchanged.

If any gate fails, keep runtime at the existing 3 percent direct nudge and leave
wider tempo sync disabled.
