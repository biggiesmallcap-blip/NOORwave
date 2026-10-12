# Exclusive release database lock handoff

The release listener uses a brief output identity check before reading SQLite,
then drops that identity guard. Settings and playback intent are read under
one database lock. A second identity check runs before status changes, public
release notification, or conditional recovery enqueue, and its guard remains
held through those synchronous effects. A busy database can no longer block
the actor's output retirement through this identity mutex.

Acceptance checks:

- Done: the actual listener can wait on a held database while another thread
  publishes a new output identity; the superseded release then changes no
  output metadata, emits no stale release notification, and queues no recovery.
- Done: current notifications and recovery retain the playing/requested policy;
  the prior runtime and superseded-output listener tests passed.
- Done: settings-storage or playback-state read failure still
  publishes current release status while declining recovery.
- Done: full server suite passed 2,144 tests, zero failures, 11 ignored
  (347.75 s). Compilation, formatting and diff checks
  passed after the production change and final test formatting.

TDD: the contention regression first failed with output retirement blocked
behind SQLite (2.18 s), then passed after the lock change (0.17 s). The test
holds a real Database connection mutex and uses the real listener and output
identity reader. Native threads join after releasing the database, and its
holder has a bounded timeout so failed assertions cannot strand shutdown.
The timing is a synchronization bound, not a playback latency benchmark.

Verification: all 44 transport tests passed before the added database
error fixture. Both final database contention/error tests passed (0.37 s).
`cargo check -p noor-server --locked`, `cargo fmt --all -- --check`, and
`git diff --check` passed. The final full server suite passed as recorded above.

Adversarial review checked retirement during the database wait, early rejection
of already-retired output, combined settings/intent reads, database errors,
conditional recovery identity, mutex poison handling, and no await within the
final identity guard. The validated listener handle queues recovery directly;
the retained AppState write guard keeps runtime and persisted intent stable.

Remaining overall-goal locking work: AppState still stays write-locked during
the database read. Moving that read off the async worker and outside AppState
requires protection against a pause or settings change racing a cached retry
decision, including a final actor-side transport-intent check. This task only
removes SQLite waits from the output publication mutex.

Not verified: audible device switching/hotplug, non-Windows compilation, and
release-mode performance. These tests open no audio stream and use isolated
test databases; no installed app or user database was modified. Dependencies
and version remain unchanged at 1.0.5. Previous task: 742047bd.
FOLLOWUPS.md entries: none. Other remaining work includes runtime event ordering,
staged decoder/resampler upgrades, measured performance, and desktop audio checks.
