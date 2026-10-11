# Command failure pause handoff

Play, Switch, Pause, Resume, and DeviceSwap failures now enter one graph-aware
failure helper before publishing their existing Error event. It latches pause
intent, clears any pending fade, and immediately pauses active, prepared,
fading, and preview decks. It retains buffered PCM, cursor positions, and
decoder identity. The lower-level event formatter is private to teardown.rs,
so command handlers cannot bypass the graph pause by calling it directly.

Resume returns immediately on failure, including exclusive-to-shared fallback
failure. It cannot continue and unpause a later deck after reporting failure.
DeviceSwap collects individual rebuild diagnostics and applies the same pause
path after its mutable deck iteration, then performs its existing teardown.
Prepared-track errors and native OutputFault retain their distinct handlers.

Acceptance checks:

- Done: the command failure helper pauses every deck before publishing Error,
  clears fade state, and preserves PCM and positions for explicit recovery.
- Done: production command reporting sites use that helper; the raw reporter
  is private. Failed Resume paths return before further deck resume calls.
- Done: the real missing-device DeviceSwap handler pauses its retained decks
  and their public data callbacks render silence.
- Done: targeted runtime verification, native compilation, formatting, and
  whitespace checks pass.
- Done: full server regression suite passed 2,117 tests with zero failures;
  ten opt-in tests were ignored. The run took 282.33 seconds.

TDD: the new graph regression first failed with "command failure must latch
pause intent", then passed. The final runtime suite passed 207 tests with
three opt-in diagnostics skipped. The missing-device handler test was run
explicitly and passed. cargo check -p noor-server --locked,
cargo fmt --all -- --check, and git diff --check passed.
The full cargo test -p noor-server --locked run also includes public transport
listener tests for current and retired runtime errors, DB state, and queue
preservation. The server is a binary crate, so this replaces a --lib-only run.

Adversarial review checked partial device rebuilds, failed exclusive fallback,
overlapping decks, fade latches, early returns, and future failure paths.
Engine pause/resume currently return success without native calls; their
error branches are defensive. The shared failure helper has a direct injected
failure regression, while missing-device coverage exercises the actual
command handler and data callback. This does not claim audible recovery or
physical hotplug validation. No API, WebSocket, schema, dependency, or release
changes. Version remains 1.0.5. Previous task: 50497e8c.

Remaining overall-goal work: same-runtime error ordering, poisoned PCM cold
restart, decoder/resampler upgrade evaluation, and patched desktop audible
recovery/hotplug. No current-goal work was moved to FOLLOWUPS.md.
