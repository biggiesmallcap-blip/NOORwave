# Empty notification polling and PCM dropouts

Task: investigate the user's audible tearing in the isolated Windows portable test, including after replacing its debug backend with an optimized backend.

Finding: the output callback immediately emits a silent block when it cannot acquire the PCM mutex. The runtime actor acquired that mutex to inspect an empty notification mailbox on every poll. This creates an application-owned dropout mechanism even with fully buffered audio, without decoder work, network activity or an output device transition. This finding does not establish that every reported audible glitch came from this observer.

Before: a concurrent stress probe used the production `take_rt_command` and `write_output_f32` paths with 2,560,000 constant PCM samples. In the observed baseline run, empty actor polling inserted 11,480 silent blocks out of 20,000 callbacks. These accelerated stress counts are not an estimate of dropout frequency in normal playback.

Change: share a pending-mailbox atomic between the buffer and runtime state. Empty polls return before touching the PCM mutex. Enqueue, consumption and seek/reset clearing publish the hint while holding the same buffer lock as the notification array, preserving its ordering and retirement rules. Allocation occurs during buffer construction only; callback publication remains an atomic store with no logging, I/O or allocation.

After: the same concurrent regression passes with zero silent blocks and the exact expected consumed sample count. All 47 shared callback tests pass, including notification delivery, full-mailbox retry, paused seek retirement and callback allocation checks.

Acceptance status:

- Done: reproduce the empty-observer dropout through production callback and actor functions.
- Done: avoid PCM locking for an empty mailbox, without changing PCM conversion or gain.
- Done: preserve transport notifications, pause fades, seek retirement and callback allocation invariants in focused tests.
- Done: combined backend regression suite, 2,190 passed and 11 ignored with eight test threads.
- Done: optimized Windows backend build, formatting and diff checks.
- Not verified: audible playback after installing this patch into the isolated portable copy.

Review: notification writers and consumers remain serialized under the PCM lock, preventing a consumer from clearing the hint after a concurrent producer publishes. A false hint can defer a concurrently arriving notification until the next actor tick; it cannot consume or retire it. Nonempty mailboxes still use the existing bounded queue and locking behavior. No route, WebSocket, database schema, dependency or version changes.

Outstanding: decoder append/compaction, watchdog inspection and control operations still use the PCM mutex. Hardware callback timing, actual dropout frequency, bit-perfect capture and long endurance are not verified by this stress probe. The user's reported tearing remains unresolved until the updated portable build is tested audibly.

Hardware comparison: the user subsequently reported that the downstream DAC's optical input is limited to 96 kHz. Earlier portable logs showed successful Realtek exclusive initialization at 192 kHz. The DAC model was not supplied, so the optical limit is user-reported rather than verified against a manual. Source-rate following requests the source rate; the Windows shared-mode default and Supported Formats checkboxes must not be treated as a proven downstream exclusive ceiling. Compare the same source with Windows default 96 kHz, source-rate following disabled, and a fresh app startup before attributing every glitch to the software lock. Do not claim that all optical hardware has a universal 96 kHz ceiling.

Validation: the first full suite, running concurrently with the release build, had one queue-removal test failure when its mocked runtime response channel disconnected. Its isolated rerun passed. The full rerun with eight test threads and no competing build passed all 2,190 enabled tests in 177.44 seconds. `cargo build -p noor-server --release --locked`, `cargo fmt --all -- --check` and `git diff --check` passed. This does not prove that build contention caused the initial test failure.

Commit: `3b19211d`, `fix(playback): avoid PCM contention from empty notification polls`, on the combined validation branch.

Next: test the optimized backend in the user's isolated portable copy. If tearing remains, distinguish producer/compaction contention from callback deadline misses before changing another boundary. Original-library backup remains prepared but unapplied to avoid disk I/O during the listening comparison. Follow-ups added: none.
