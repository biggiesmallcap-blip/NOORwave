# Resampler buffer reuse handoff

StreamResampler now reuses a rubato planar output buffer and consumes input
incrementally through one 1024-frame scratch chunk per channel. It no longer
collects drained input channels or allocates rubato output vectors per chunk.
Flush pads the existing input scratch and interleaves only the valid output
prefix, preserving the prior padding and frame-trimming policy.

Each input channel allocates its own reserved Vec at construction. Repeating
an empty reserved Vec through vec![value; count] cloned away capacity for the
earlier channels; the allocation and capacity tests exposed that behavior.
Returned interleaved packet and flush vectors still allocate once each.

Acceptance checks:

- Done: exact sample and frame parity with the former allocating rubato path
  for 44.1/48/96 kHz upsampling and downsampling, mono/stereo/six channels,
  impulses and varying signals, irregular packets, and EOF flushing.
- Done: the input scratch capacity stays fixed across large packets; no
  decoded packet is accumulated in planar scratch before processing.
- Done: the allocation regression measures one allocation and zero frees per
  returned packet and flush buffer. The stereo eight-chunk baseline measured
  69 allocations and 67 frees for one process call before the change.
- Done: repeated flush and empty input return no samples.
- Done: the real isolated HTTP WAV path fetches, decodes through Symphonia,
  adapts mono to stereo, resamples 44.1 to 48 kHz, appends matching PCM, and
  finishes with matching total-sample metadata.
- Done: full server suite passed 2133 tests, with zero failures and 11 ignored (288.20 seconds). cargo check -p noor-server --locked, cargo fmt --all -- --check, and git diff --check passed.

TDD: the allocator regression failed with (69, 67) versus (1, 0), then passed
after the buffer changes and per-channel capacity fix. All seven resample
tests passed, including 12 rate/channel combinations against rubato's separate
allocating convenience path. The HTTP decode-to-PCM integration test passed.
The test allocator helper is test-only and restores thread-local state on
return or unwind; no production allocator or callback logic changed.

Adversarial review checked partial chunks, returned-frame prefixes in oversized
scratch buffers, channel order, stale output suffixes, fixed-ratio reservation,
scratch growth, flush idempotence, error clearing, and end-of-stream metadata.
The process call remains wired into the existing decoder; no parallel decoder
or alternate output path was added. The sample-parity tests cover the existing
algorithm, not a new claim of ideal resampler delay or tail accounting.

Not verified: audible playback and release-mode throughput/underrun benchmarks.
The evidence proves lower allocation traffic with identical tested samples;
it does not establish a CPU speedup or an audible improvement. Native WASAPI
and CPAL routing code was not changed or reopened for this task.

Rubato remains 0.16.2, with no Cargo.lock or release changes. Its versioned
[Resampler documentation](https://docs.rs/rubato/0.16.2/rubato/trait.Resampler.html)
and installed source confirm reusable output buffers and returned frame counts.
Context7 returned the newer adapter API, so the implementation used the exact
locked version's API instead. Version remains 1.0.5. Previous task: fa5dda88.
Follow-ups added to FOLLOWUPS.md: none.

Remaining overall-goal work: exclusive output-instance notification identity,
same-runtime event/control ordering, staged Symphonia/rubato upgrades with
codec and timing fixtures, measured release-mode performance, and patched
desktop audible recovery/hotplug. Resampler delay/tail accounting and failure
propagation also remain review items; this task preserves their existing policy.
