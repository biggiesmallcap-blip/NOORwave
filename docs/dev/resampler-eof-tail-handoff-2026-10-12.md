# Resampler EOF tail handoff

StreamResampler counts complete input and emitted output frames. EOF now pads
its reusable input scratch and drains filter history until the output reaches
the rounded source duration at the target rate. This also runs when EOF lands
on an exact 1024-frame input boundary. Only the required output prefix is
appended, so zero padding does not extend playback duration. Empty input and
repeated EOF return no samples.

The existing SincFixedIn sample alignment is preserved. An attempted trim of
rubato's reported output_delay shifted a 256-frame impulse early by 69 output
frames at 44.1 to 48 kHz. The locked implementation starts its interpolation
index half a filter before zero, and the impulse regression confirms its PCM
already aligns with the source clock. The delay is retained filter history to
drain, not a prefix to remove from this implementation's samples.

Acceptance checks:

- Done: exact rounded output duration and final impulses
  for tiny, partial, and exact-chunk streams at 44.1/48/96 kHz.
- Done: impulse peaks remain within one frame of the
  source clock during upsampling and downsampling.
- Done: exact sample parity with an independent whole-clip
  rubato allocating reference, including mono, stereo, six channels, irregular
  packets, and rates from 8 to 384 kHz.
- Done: the real isolated HTTP WAV path reaches finished
  stereo PCM with independently checked duration and matching total metadata
  for 1, 1024, 1025, 4096, and 4099 source frames.
- Done: packet and flush allocation remain one returned
  Vec allocation and zero frees, with bounded reused planar scratch.
- Done: full server suite passed 2,139 tests, zero failures, 11 ignored
  (346.33 s). All 49 decode tests passed (14.87 s).
  `cargo check -p noor-server --locked`, `cargo fmt --all -- --check`, and
  `git diff --check` passed.

TDD: before the change, the duration/tail test failed on exact EOF: 1024 input
frames at 44.1 kHz yielded 1043 output frames at 48 kHz instead of 1115. The
impulse test rejected the attempted startup-delay trim. A one-frame 96 to
44.1 kHz stream rounds to zero output frames; the duration test accepts this
without demanding an impulse in an empty signal.

The whole-clip reference uses rubato's separate allocating API and derives its
duration from the original input, independently of the production counters.
Decoder integration fetches actual WAV bytes over localhost and decodes through
Symphonia before channel adaptation, resampling, PCM append and EOF metadata.
No output stream or installed application is opened by these tests.

Adversarial review checked exact-boundary EOF, tiny inputs rounding to zero,
channel ordering, stale scratch suffixes, packet-independent rounding, repeated
flush, and retained sample alignment. Rubato processing errors still use the
pre-existing warning/partial-output policy; explicit failure propagation remains
required overall-goal work for a separate task.

Not verified: audible desktop track transitions, release-mode CPU/underrun
measurements, and non-Windows compilation. The frame and signal tests do not
establish an audible quality or throughput claim.

Reference: locked rubato 0.16.2 Resampler documentation and asynchro_sinc.rs
initial last_index, processing loop and output_delay implementation. Context7
returned the newer API; no new API or dependency upgrade was applied.
Version remains 1.0.5. Previous task: 8d00014a. FOLLOWUPS.md entries: none.

Remaining overall-goal work: same-runtime control/event ordering, runtime-wide
faults, resampler failure propagation, staged decoder/resampler upgrades with
codec and timing fixtures, measured performance, and audible recovery/hotplug.
