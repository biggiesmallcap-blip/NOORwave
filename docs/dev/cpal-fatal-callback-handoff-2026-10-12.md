# CPAL fatal callback handoff

The first fatal CPAL callback now moves its owned driver diagnostic into a
preallocated slot belonging to that output stream. It no longer formats a
String, clones decoder identity, or sends through the allocating command
channel on the callback thread. Runtime polling formats the diagnostic and
dispatches the existing CpalOutputError command before a queued Resume.
The existing handler pauses all decks, preserves the queue, and requires
explicit recovery. Public API and WebSocket payloads are unchanged.

The native stream wrapper retains the diagnostic through teardown and retires
its identity before the native destructor runs. Each replacement output gets
a fresh slot. A producer claim permits only one OnceLock initializer, while
the consumer uses nonblocking get. Duplicate fatal diagnostics are coalesced.

Acceptance checks:

- Done: first accepted fatal callback retains an owned diagnostic with zero
  allocations and zero frees. The allocator regression failed before the fix
  with two allocations and three frees, then passed through the same emitter.
- Done: runtime receiver observes the fault before queued Resume and dispatches
  it into the existing graph pause and OutputFault path.
- Done: first diagnostic is delivered once; replacement streams cannot inherit
  it, and retired output/decoder callbacks cannot publish actionable faults.
- Done: native teardown keeps diagnostic ownership through its destructor.
- Done: broader playback checks and silent native open/reopen pass.

Verified: 588 playback tests passed, five ignored diagnostic or native opt-in
tests. The silent Realtek hardware smoke was run explicitly and passed at
44.1 and 96 kHz exclusive, including idle release/reopen, and at 88.2 kHz
shared. cargo check -p noor-server --locked, cargo fmt --all -- --check,
and git diff --check passed. All 50 public transport tests passed, including
the actual listener's fault identity checks, DB pause, and queue preservation.

Adversarial review covered failed build guards, same-decoder output replacement,
fault/Resume ordering, retired callbacks, first-fault retention, and control
ownership through native callback teardown. No DB, API, dependency, or release
changes were needed. The app remains 1.0.5.

Limits: CPAL itself supplies owned Error values. Discarding duplicate, retired,
or recoverable incoming errors can still free an upstream-owned message on
the callback thread. This patch guarantees zero allocator use for the first
accepted fatal error, not for every possible upstream error payload. Native
fault detection follows the existing runtime poll interval: active output
requests five milliseconds; an idle runtime can wait up to one second. These
are scheduling requests, not Windows timing guarantees.

Not verified: audible playback recovery and physical device hotplug with the
patched desktop app. The silent smoke does not establish either. Remaining
overall-goal work includes generic runtime error identity/order, cold decoder
recovery after poisoned PCM, and staged decoder/resampler upgrade evaluation.
No current-goal work was moved to FOLLOWUPS.md. Previous task: 67e09da0.
