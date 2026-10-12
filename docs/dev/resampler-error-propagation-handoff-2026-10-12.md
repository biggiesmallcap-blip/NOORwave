# Resampler error propagation handoff

StreamResampler process and EOF drain now return Result<Vec<f32>>. Rubato
processing failures retain their typed cause and add packet/EOF context.
Incomplete interleaved input frames are rejected before changing residual
samples or frame counters. EOF draining that produces no frames is an error
instead of a warning followed by a successful partial result.

The production HTTP decoder propagates packet and drain errors with `?`, before
mark_finished, total-sample publication, or NextDecodeComplete. Its existing
decoder-thread handler turns returned errors into TrackTerminal::Error; the
existing runtime and transport error handling remains responsible for queue
advancement and reporting. No public route or WebSocket contract changed.

The public decoder entry delegates to the same private decoder body with
StreamResampler::new. Tests inject only its constructor to shorten a real
rubato output buffer. This test-only corruption method adds no runtime setting
or feature flag. Fetching WAV data, Symphonia decode, channel adaptation,
resampler processing, and EOF handling use the production body.

Acceptance checks:

- Done: malformed interleaved frames return an error without changing the
  buffered input or frame count.
- Done: actual Rubato packet and EOF validation errors return their typed
  InsufficientOutputBufferSize cause rather than a successful partial Vec.
- Done: real isolated HTTP WAV packet and EOF failures leave the PCM buffer
  unfinished, total metadata unpublished, and NextDecodeComplete unsent.
- Done: normal HTTP decode, complete tails, impulse alignment, multichannel
  sample parity and allocation regressions still pass.
- Done: full server suite passed 2,142 tests, zero failures, 11 ignored
  (343.81 s). All 52 decode tests passed (14.94 s).
  The final fault test also passed after an earlier successful PCM packet.
  `cargo check -p noor-server --locked`, `cargo fmt --all -- --check`, and
  `git diff --check` passed.

TDD: the initial tests did not compile because the old Vec-only contract could
not represent either error. After the Result change, actual buffer corruption
exercised Rubato validation in both process and flush. One initial assertion
expected a different diagnostic string; it was replaced with a downcast check
of the exact error variant, preserving the underlying typed cause as evidence.

Adversarial review checked malformed input before mutation, context/cause
retention, failure after previous successful packets, EOF before completion
metadata, success-path allocation counts, and constructor wiring. A failed
resampler is discarded with its track decoder; continuing that damaged stream
is not a recovery path.

Not verified: an audible desktop track with a forced internal resampler fault,
non-Windows compilation, and release-mode CPU/underrun measurements. Tests
exercise real HTTP decode with deterministic internal buffer corruption and
inspect the unchanged decoder-thread terminal-error wiring; they do not inject
corruption into the installed desktop app.

Dependencies and version remain unchanged at 1.0.5. Previous task: 3c603295.
FOLLOWUPS.md entries: none. Remaining overall-goal work includes same-runtime
control/event ordering, runtime-wide faults, staged decoder/resampler upgrades
with codec/timing fixtures, measured performance, and audible recovery/hotplug.
