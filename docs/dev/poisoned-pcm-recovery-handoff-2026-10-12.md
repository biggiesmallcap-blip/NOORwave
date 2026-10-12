# Poisoned PCM recovery handoff

Explicit Resume now detects poisoned PCM or retained DJ source-prefix state
on any deck. It cold-restarts the current track with the same generation and
queue identity, retires stale decks, and rebinds public position/buffer readers.
It never clears poison and reuses a potentially corrupted buffer. WASAPI is
retired before rebuilding the new decoder/output session.

The restart job preserves the original source position at millisecond
resolution. DASH restarts at the containing segment; direct streams restart
from the beginning. The new decoder starts paused until recovery is armed.
Already-decoded preroll is discarded on the control thread; subsequent preroll
is discarded during decoder append, before allocation and backpressure.
The callback waits in silence until the target is reachable, then resumes
from a frame-aligned cursor. A short/changed source clamps to its real EOF
and reaches normal terminal handling. Resampler tails use the same append
path, so they cannot bypass preroll handling or buffered telemetry.

Acceptance checks:

- Done: injected PCM poison produces a fresh decoder without losing the
  active track or generation; healthy decoder state does not restart.
- Done: saved source position selects the correct DASH segment and the new
  decoder remains paused until recovery is armed.
- Done: preroll is silent and discarded without accumulating the lookahead
  window; 44.1 kHz recovery preserves interleaved stereo frame alignment.
- Done: short-source EOF remains reachable, and the new recovery callback
  wait/start branches allocate and free nothing.
- Done: the real Resume handler cold-restarts a poisoned buffer using a
  disposable localhost WAV and silent native Realtek output; public position
  ownership redirects and native callbacks consume beyond the saved 500 ms.
- Done: full server suite passed 2,123 tests with zero failures; eleven opt-in
  tests were ignored. The run took 283.31 seconds.

TDD: the saved-position restart regression failed with target zero instead of
2,500 ms, then passed. The broader playback suite passed 595 tests with six
opt-in diagnostics skipped. The native recovery test was run explicitly and
passed. It opens only silence and requires an explicitly selected endpoint.
The native fixture was added after the restart plan and callback logic; it
verifies the actual handler, decoder, network source, and output connection.
cargo check -p noor-server --locked, cargo fmt --all -- --check, and
git diff --check passed.
The full cargo test -p noor-server --locked run includes the public transport
listeners and replaces --lib verification because this server is a binary crate.

Adversarial review covered EOF before the saved target, output rate changes,
stereo frame boundaries, preroll backpressure, paused startup, old decoder
retirement, source-prefix poison, and stale transition plans. Restart failure
uses the graph-aware failure handler and returns while paused. Recovery drops
prepared DJ transitions and rebuilds from the active source position, so it
does not replay an old rendered handoff. No public API, WebSocket, schema,
dependency, release, or installed-user-data changes. Previous task: 22a35f33.
Version remains 1.0.5.

Limits: millisecond position restoration can round by less than one millisecond
plus one output frame. A direct stream may need to fetch/decode earlier audio
again, although PCM preroll is discarded rather than retained. Audible
continuity and physical hotplug remain unverified; silent native recovery
does not prove either. Remaining overall-goal work includes same-runtime event
ordering, staged decoder/resampler upgrades, and patched desktop audible
validation. No current-goal work was moved to FOLLOWUPS.md.
