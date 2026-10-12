# Transport acknowledgment runtime identity handoff

Pause and resume acknowledgments now validate their originating runtime under
the same AppState read guard retained through DB reconciliation and public
notification. Runtime replacement requires the write lock, so it cannot occur
between validation and mutation. The listener retires when validation fails.
This removes the previous separate validation and reconciliation lock waits.

Acceptance checks:

- Done: a pause acknowledgment waiting for the state lock cannot pause a
  replacement runtime's persisted playback intent.
- Done: a similarly delayed resume cannot resume replacement playback intent.
- Done: current pause and resume acknowledgments reach the real listener and
  publish PlaybackStateChanged with the updated persisted state.
- Done: acknowledgments preserve the current queue anchor and queue length;
  redundant reconciliation emits no duplicate notification.
- Done: both targeted transport_ack tests passed. Full server suite passed 2130 tests with 11 ignored and zero failures (286.47 seconds). cargo check -p noor-server --locked, cargo fmt --all -- --check, and git diff --check passed.

TDD: the contended-lock regression failed with "retired transport acknowledgment
changed replacement intent" before the identity check. The fixture initially
used an incorrect snapshot helper name; that compilation error was corrected
before recording the behavioral failure. The current-event test uses the actual
listener and public AppEvent channel with an isolated migrated DB.

Adversarial review: the validated guard spans every synchronous DB query and
notification; there is no further await before it drops. Read guards already
protect this reconciliation path, so no new global write lock or callback lock
was added. A rejected event cannot affect DB state or broadcast notifications.
The current runtime remains subscribed when reconciliation is redundant or a
DB operation fails, preserving existing listener behavior.

Not verified: audible desktop playback and native hotplug were not exercised
for this listener-only change. No native renderer, output format, public API,
WebSocket shape, dependency, or release configuration changed. Version is 1.0.5.
Previous task: 8fb335ee. Follow-ups added to FOLLOWUPS.md: none.

Remaining overall-goal work includes superseded output-instance notification
identity within one runtime, same-runtime control ordering and other delayed
events, staged decoder/resampler improvements, and desktop audible recovery and
hotplug verification. These requirements remain part of the active goal.
