# Smart Stretch Evaluation Gate

Date: 2026-05-26

## Goal

Decide whether wider tempo sync is worth shipping before adding any pitch-preserving stretch dependency or realtime path.

Direct playback-rate sync remains capped at `0.97..1.03`. Wider deltas are evaluation-only until this gate passes.

## Candidate

Primary candidate: Signalsmith Stretch.

Reason: permissive licensing and suitable design for offline prepared-buffer rendering.

Blocked candidates:

- Rubber Band: blocked until license review is explicitly approved.
- Local LLM transition generation: blocked for realtime timing and rendering.
- Beat This: useful for offline beat/downbeat benchmarking, not a stretch renderer.

## Test Material

Use fixed local fixtures before any user-library sweep:

- Synthetic click track at 120 BPM.
- Synthetic kick/snare loop with strong transients.
- Harmonic pad loop with sustained notes.
- Full-band electronic loop with bass and hats.
- Vocal-heavy excerpt.

Evaluate tempo deltas:

- 3 percent.
- 5 percent.
- 8 percent.
- 12 percent.

Evaluate both directions: slower and faster.

## Metrics

For every fixture and tempo delta, record:

- Render time in milliseconds for 30s, 90s, and full-track windows.
- Peak memory during render.
- Output peak and true peak proxy.
- RMS change versus source.
- Click transient drift at the first, middle, and final beat.
- Maximum phase drift in milliseconds.
- Buffer length error in frames.
- Any non-finite sample count.

Manual listening notes are allowed, but cannot replace objective pass/fail metrics.

## Pass Criteria

Signalsmith can move from evaluation to implementation planning only if:

- 5 percent stretch stays under 10 ms maximum click phase drift for 90s fixtures.
- 8 percent stretch stays under 20 ms maximum click phase drift for 90s fixtures.
- 90s render time stays below 500 ms on the target listening machine.
- Full-track render can run in the background without blocking playback.
- Output has no non-finite samples.
- Peak does not exceed the existing limiter ceiling after the DJ mixer limiter.
- Prepared-buffer rendering can absorb all allocation and latency outside the realtime callback.

12 percent is exploratory only. Passing 12 percent does not automatically increase the ship cap.

## Fail Criteria

Do not ship wider stretch if any of these occur:

- Audible transient smearing on click or drum fixtures at 5 percent.
- Phase drift above the pass criteria.
- Render-time spikes that threaten the active lookahead window.
- Output length error that would break queue promotion or overlay cleanup.
- Any need to allocate, decode, fetch, or lock unbounded state inside the realtime callback.

## Implementation Boundary

If the gate passes, the next implementation plan must still keep stretch in prepared buffers:

- Decode source audio first.
- Render stretched transition audio off the callback.
- Validate length, finite samples, peak, and phase drift.
- Only then hand a ready buffer to the runtime.
- Fall back to `SafeCrossfade` or the 3 percent small nudge path on any failure.

No realtime stretch engine is approved by this gate.

## Current Implementation Status

Implemented:

- `noor-mix::stretch_eval::evaluate_stretch_render` records objective metrics for a candidate stretched render.
- Normal tests cover finite sample detection, output length error, peak/RMS reporting, and click-marker phase drift.
- The ignored baseline benchmark prints the same metrics across 30s, 90s, and 180s synthetic click fixtures.
- `signalsmith-eval` adds an optional Signalsmith candidate renderer for offline evaluation only.

Run the harness with:

```powershell
cargo test -p noor-mix smart_stretch_evaluation_baseline_benchmark -- --ignored --nocapture
```

Still left before `Smart Stretch Now`:

- Install or expose `libclang.dll` so the optional Signalsmith feature can compile.
- Run fixed synthetic and musical fixtures in both slower and faster directions at 3, 5, 8, and 12 percent tempo deltas.
- Record render time, phase drift, peak, RMS movement, output length error, and manual listening notes.
- Keep runtime playback capped to the existing `0.97..1.03` direct playback-rate path until the evaluation passes.

## Signalsmith Feature Gate Result

Attempted on 2026-05-26 with:

```powershell
cargo test -p noor-mix --features signalsmith-eval stretch_eval
```

Result: blocked by native toolchain setup before evaluation could run.

Exact blocker:

```text
Unable to find libclang: "couldn't find any valid shared libraries matching: ['clang.dll', 'libclang.dll'], set the `LIBCLANG_PATH` environment variable to a path where one of these files can be found (invalid: [])"
```

Per the hardened plan, do not vendor C++ or switch crates in this slice. The optional `signalsmith-eval` feature can be retried after `libclang.dll` is available and `LIBCLANG_PATH` points at it.

Retried on 2026-05-27 after installing LLVM 22.1.6 with `winget` and setting:

```powershell
$env:LIBCLANG_PATH='<llvm-bin>'
```

Feature gate passed:

```powershell
cargo test -p noor-mix --features signalsmith-eval stretch_eval
```

Result: 4 passed, 1 ignored.

Ignored evaluation benchmark also completed:

```powershell
cargo test -p noor-mix --features signalsmith-eval smart_stretch_evaluation_baseline_benchmark -- --ignored --nocapture
```

Decision: do not allow runtime prepared-buffer stretch yet. Signalsmith quality metrics were good on the synthetic click fixtures, but render time failed the current gate by a large margin in this debug benchmark.

Key Signalsmith 90s rows:

- `0.950`: `render_ms=20676`, `max_phase_drift_ms=0.771`, `peak=0.917`, `passed=true`
- `1.050`: `render_ms=18035`, `max_phase_drift_ms=0.750`, `peak=0.869`, `passed=true`
- `0.920`: `render_ms=23449`, `max_phase_drift_ms=0.833`, `peak=0.922`, `passed=true`
- `1.080`: `render_ms=17956`, `max_phase_drift_ms=0.896`, `peak=0.864`, `passed=true`

The 5 percent and 8 percent drift gates pass, output is finite, length error is 0 frames, and peaks stay below `0.98`. The 90s render-time gate fails because the target is under `500 ms`.

Release-mode benchmark was then run with:

```powershell
cargo test -p noor-mix --release --features signalsmith-eval smart_stretch_evaluation_baseline_benchmark -- --ignored --nocapture
```

Release mode is much faster and confirms Signalsmith is still a serious prepared-buffer candidate, but it does not pass the current runtime gate.

Key release-mode Signalsmith rows:

- 30s `1.030`: `render_ms=494`, `max_phase_drift_ms=0.604`, `peak=0.885`, `passed=true`
- 30s `1.050`: `render_ms=487`, `max_phase_drift_ms=0.750`, `peak=0.869`, `passed=true`
- 30s `1.080`: `render_ms=467`, `max_phase_drift_ms=0.833`, `peak=0.861`, `passed=true`
- 90s `0.950`: `render_ms=1571`, `max_phase_drift_ms=0.771`, `peak=0.917`, `passed=true`
- 90s `1.050`: `render_ms=1448`, `max_phase_drift_ms=0.750`, `peak=0.869`, `passed=true`
- 90s `0.920`: `render_ms=1621`, `max_phase_drift_ms=0.833`, `peak=0.922`, `passed=true`
- 90s `1.080`: `render_ms=1390`, `max_phase_drift_ms=0.896`, `peak=0.864`, `passed=true`
- 180s `1.080`: `render_ms=2753`, `max_phase_drift_ms=0.896`, `peak=0.864`, `passed=true`

Decision after release benchmark: keep runtime playback at the 3 percent direct nudge. Do not wire wider runtime smart stretch yet. A later prepared-buffer plan may use shorter windows, earlier background preparation, or release-only worker timing, but it needs a new gate because the original 90s-under-500ms gate failed by roughly 3x.
