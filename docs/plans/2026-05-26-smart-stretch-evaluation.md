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
