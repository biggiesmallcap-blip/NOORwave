# Command error identity handoff

Runtime command failures now publish an internal CommandError carrying the
originating playback generation and active decoder reader identity. The
listener validates runtime, generation, and decoder after acquiring the state
write lock. It holds the decoder identity reader through the synchronous
DB/UI mutation, preventing actor publication from changing that identity
between validation and pause. No callback uses this reader lock.

The existing handoff elapsed reader already redirects at every active decoder
installation and promotion. Its Arc identity distinguishes same-track,
same-generation restarts without retaining PCM or creating another identity
registry. The last requested generation remains available after failed cold
starts remove all engines. Identity reader poison is safe to recover because
its payload is always a valid old or new Arc; no poisoned PCM is reused.

Acceptance checks:

- Done: delayed command errors cannot pause a newer playback generation.
- Done: same-track, same-generation decoder replacement rejects the old error.
- Done: validation runs after a deliberately contended state lock and rejected
  errors leave DB playback, audio activity, diagnostics, and notifications intact.
- Done: current CommandError reaches the actual listener, pauses DB state,
  clears activity, retains its message, and preserves queue contents/identity.
- Done: failed cold starts retain generation and reader identity after teardown.
- Done: full server suite passed 2,126 tests with zero failures; eleven opt-in
  tests were ignored. The run took 283.90 seconds.

TDD: the interleaving regression first failed with "stale generation command
error was applied", then passed for both generation and decoder replacement.
All five targeted command_error tests passed. The real missing-device
DeviceSwap handler test was executed explicitly and passed with CommandError.
cargo check -p noor-server --locked, cargo fmt --all -- --check, and
git diff --check passed.
The full cargo test -p noor-server --locked run replaces --lib verification
because this server is a binary crate, and includes public transport listeners.

Adversarial review checked state/reader lock ordering, absence of reader use
inside listen-session flush, release before asynchronous scrobble dispatch,
engine teardown, same-job restarts, and runtime removal. The formatter remains
private behind the graph-aware failure helper. Public PlaybackFailed and
PlaybackStateChanged notifications and API/WS shapes remain unchanged.
CommandError is an in-process runtime event, not a WebSocket payload.

Not verified: audible desktop playback was not rerun for this control/listener
change. Current and stale public event consumption were exercised through
transport tests; output rejection was checked through the native-environment
handler fixture. Version remains 1.0.5. Previous task: d8e52b69.

Remaining overall-goal work: runtime-wide panic/breaker errors and other
transport acknowledgements still need ordering review. This patch does not
distinguish multiple control commands on the same decoder, or old and new
exclusive output instances on that decoder. Continue those checks, staged
decoder/resampler upgrade evaluation, and audible hotplug/recovery validation.
No current-goal work was moved to FOLLOWUPS.md.
