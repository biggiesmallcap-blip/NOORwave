# Phone Remote implementation progress

Authoritative contract: `2026-09-21-phone-remote-zero-config.md`.

## Stage 1 — server foundation

Status: complete; independent review findings resolved and re-reviewed.

Implemented:

- Migration 063 and digest-only paired-device persistence.
- Persistent server identity/hostname, one-use 120-second ticket lifecycle,
  device limits, throttling, and last-seen write limiting.
- Shared-PIN and paired-device principals across HTTP/query/WebSocket auth.
- Loopback + trusted-origin + shared-PIN management boundary.
- Actual listener/control facts, public pairing and local management APIs.
- Per-device revoke, reset-all credential rotation, and live socket revocation.
- Ambiguous paired header/query credentials fail closed; setup/token reads the
  authoritative rotated PIN.
- Asset readiness requires a real bundled `index.html`; new endpoint rejection
  paths keep the JSON/no-store contract and trusted dev-origin exception.
- Deterministic contention tests cover reset racing redemption and socket
  registration.

Verification on 2026-09-21:

- `cargo test -p noor-server`: 1629 passed, 5 ignored, 0 failed.
- `cargo test -p noor-server server::tests:: -- --nocapture`: 20 passed.
- `cargo test -p noor-server server::remote::tests:: -- --nocapture`: 14 passed.
- Focused remote/db/migration suites: all passed.
- `cargo check -p noor-server`: passed (pre-existing dead-code warnings only).
- `cargo fmt --all --check`: passed.
- `git diff --check`: passed.

## Stage 2 — controlled desktop lifecycle

Status: implementation, automated verification, and adversarial remediation
complete; packaged/manual Windows evidence remains outstanding.

Implemented:

- Atomic, error-reporting desktop config updates preserve unknown fields, keep
  the prior valid file on replacement failure, recover an interrupted backup
  when the replacement is absent or corrupt, and reject an invalid backup.
- One serialized native host-mode transition is shared by Settings commands and
  the tray, with typed state/errors, emitted outcomes, a 15-second target
  deadline, readiness verification against actual listener facts, enable
  rollback to one local-only recovery attempt, and fail-closed disable
  behavior. Failed enable attempts stop the unverified LAN child before config
  rollback; failed local recovery also stops its child, with four seconds of
  the target reserved for final cleanup. Packaged wall-clock enforcement remains
  unproven.
- Tray state is reconciled from every transition result, including failures
  such as external override, concurrent transition, or config persistence that
  occur before a state event can be emitted.
- Managed sidecar launches always pass an explicit true/false host mode. The
  server honors `NOOR_ADDR` first, mirrors desktop-managed mode into SQLite
  before readiness, and reports actual listener/control facts separately from
  saved preference.
- Persisted phone-remote mode is applied directly at ordinary and autostart
  launches; local-only remains the fresh default.
- The official pinned Tauri autostart and single-instance plugins provide an
  explicit hidden-launch marker, observed OS registration state, installed vs
  portable typed behavior, and arbitration before sidecar-owning setup.
- Autostart-marked launches create the ordinary tray/sidecar owner without
  showing or focusing the main window. A later normal launch requests window
  activation without spawning a second owner.
- Updater failure recovery and normal/tray exit continue to use the managed
  sidecar lifecycle and bounded shutdown path.
- Native capability scope remains restricted to the exact loopback app origin;
  LAN pages receive no Tauri authority. Desktop host controls reject
  `NOOR_ADDR` and a valid nondefault `NOOR_PORT` before persistence or restart
  because those origins are outside that exact capability.

Automated verification on 2026-09-21:

- `cargo test -p noor-app`: 18 unit tests and 3 integration tests passed.
- `cargo test -p noor-server managed_host -- --nocapture`: 2 passed.
- Focused bind precedence, actual-listener status, and standalone host-mode
  persistence tests: 1 passed each.
- `cargo check -p noor-app -p noor-server`: passed (five pre-existing
  noor-server dead-code warnings only).
- `cargo fmt --all --check`: passed.
- `git diff --check`: passed.

Outstanding packaged/manual evidence:

- Installed and portable Windows artifacts have not yet exercised real OS
  autostart registration/query/removal or stale-registration repair after an
  incomplete update/executable move.
- Hidden autostart launch, tray availability within five seconds, concurrent
  manual-launch activation, and updater replacement need packaged-process
  integration checks to prove exactly one sidecar and no focus flash.
- Live port-occupied/bind-denied/config-permission failure injection and the
  full 15-second recovery timing need a packaged Windows fixture.

## Stage 3 — network discovery and address facts

Status: implementation and automated verification complete; the required
packaged Windows, real-network, and physical-phone evidence remains outstanding,
so the release-level discovery checkpoint is not yet proven.

Implemented:

- The server owns a pinned embedded `mdns-sd` 0.21.4 responder and `netdev`
  0.46.3 interface inventory. A loopback-only listener creates neither a
  responder nor an interface polling task.
- Active, listener-compatible IPv4 addresses are filtered to exclude
  loopback, unspecified, multicast, and link-local values. Physical LAN
  interfaces rank ahead of VPN/virtual candidates, while other usable
  addresses remain explicit direct-IP fallbacks.
- DNS-SD advertisement is restricted to active physical multicast interfaces
  and uses the actual bound port, persistent server ID, confirmed hostname,
  `_noorwave._tcp.local.`, and only `path=/remote`, `protocol=1`, and `id` TXT
  fields. IPv6 listeners do not produce misleading IPv4/AAAA discovery facts.
- A friendly URL is exposed only after the responder reports its announcement.
  Hostname conflicts persist the responder-selected lowercase `.local.` name,
  clear the stale friendly URL/ticket, and wait for confirmation of the new
  advertisement before reporting it usable.
- Interface state refreshes every five seconds and also reacts to responder IP
  add/remove events. Removed origins invalidate affected tickets; unchanged
  address and friendly-hostname snapshots preserve valid tickets.
- Missing physical multicast LAN addresses, missing packaged remote assets, or
  responder errors leave direct-IP facts available where possible and report
  discovery as unavailable without claiming phone reachability.
- HTTP draining and discovery withdrawal observe the same retained shutdown
  state. Graceful unregister and daemon-stop waits are capped at 250 ms and
  500 ms, and the discovery task has a 900 ms forced-stop budget. The focused
  component test uses an exact one-second outer timeout and completed in about
  0.91 seconds; this is not evidence that a packaged sidecar's total shutdown
  overhead satisfies NFR-8 on Windows.

Automated verification on 2026-09-22:

- `cargo test -p noor-server server::remote::discovery::tests:: -- --nocapture`:
  5 passed, including actual-port/TXT construction, deterministic interface
  selection, collision persistence/ticket invalidation, retained shutdown, and
  the exact one-second component shutdown gate.
- `cargo test -p noor-server server::remote::network::tests:: -- --nocapture`:
  3 passed.
- `cargo test -p noor-server db::remote::tests:: -- --nocapture`: 2 passed.
- `cargo test -p noor-server server::remote::tests:: -- --nocapture`: 17 passed.
- `cargo test -p noor-server server::tests:: -- --nocapture`: 20 passed.
- `cargo test -p noor-server`: 1,644 passed, 5 ignored, 0 failed.
- `cargo test -p noor-app`: 18 unit and 3 integration tests passed.
- `cargo check`: passed with five pre-existing noor-server dead-code warnings.
- `cargo fmt --all --check`: passed.
- `git diff --check`: passed.

Outstanding physical/manual Windows evidence:

- Two real responders competing for `noorwave.local` have not been run; actual
  conflict rename convergence, persistence across restart, and phone resolution
  remain unverified outside deterministic tests.
- No physical iPhone/Safari or Android/Chrome has resolved the friendly name,
  loaded the direct-IP fallback, or completed a pairing/control action in this
  stage. The 20-second onboarding target remains unmeasured.
- Real DHCP renewal, Wi-Fi loss/rejoin, sleep/resume, multi-NIC, and VPN changes
  have not exercised record refresh, ticket invalidation, or recovery.
- Coexistence with the Windows DNS Client, Bonjour, or another installed mDNS
  responder on UDP 5353 has not been verified.
- An installed or portable packaged sidecar has not measured advertisement
  withdrawal or total graceful shutdown overhead. NFR-8's at-most-one-second
  packaged timing remains unproven despite the 900 ms internal forced-stop
  budget and exact one-second component test.

## Stage 4 — phone and desktop connection UI

Status: implementation, automated verification, and adversarial remediation
complete; the required packaged/browser/physical-device evidence remains
outstanding.

Implemented:

- A dedicated searchable **Phone Remote** settings component now owns actual
  listener/discovery status, the restart disclosure and persistent hosting
  control, start-at-sign-in presentation, local-only pairing QR, friendly and
  direct-IP selection, explicit refresh/cancel, paired-device rename/revoke,
  reset-all semantics, honest diagnostics, and the manual six-digit fallback.
  The former Access PIN search vocabulary remains an alias for this section.
- The QR is generated entirely from the bundled `qrcode` package with error
  correction M and a four-module quiet zone. A separate `jsqr` decoder test
  round-trips the generated PNG payload rather than checking only for an image.
- New typed remote API contracts use five-second management timeouts and retain
  the server's structured status/error distinctions. QR operations are
  generation-guarded and their entry controls serialize creation, refresh,
  address changes, and cancel so a stale response cannot replace the current
  ticket.
- Browser bootstrap synchronously removes a single pairing secret from history
  before its first request, rejects duplicate `pair` fragment parameters, and
  runs exactly one redemption instead of normal setup/auth bootstrap. Stored
  credentials are preceded by public identity lookup; paired credentials with
  absent or mismatched server metadata are cleared without being sent.
- Paired token and server/device metadata persistence is coherent: both are
  stored for the same origin or both fall back to memory. Storage exceptions do
  not prevent a current-tab session and the UI explains that it cannot be
  remembered.
- Manual PIN validation now accepts only a successful protected response and
  distinguishes rejection, throttling, server failure, and network failure.
  The pairing screen similarly distinguishes invalid/used links, rate limits,
  server storage failure, and reachability failure.
- WebSocket reconnect now owns one timer with 1, 2, 4, 8, then capped 15-second
  backoff, resets after success, and is cancelled on cleanup. Close code 4001 is
  terminal: it clears the paired session, stops reconnecting, and returns to
  pairing/PIN entry while ordinary network loss retains credentials.
- Management remains desktop/loopback scoped. Portable/unsupported startup
  state is disabled with the required explanation, native failures retain the
  last OS-observed value, async errors are announced and focused, and narrow
  layouts retain 44-pixel action targets and a textual alternative to QR.

Automated verification on 2026-09-22:

- `pnpm --dir frontend test`: 146 files, 876 tests passed, including fragment
  order/exclusivity, duplicate fragments, identity mismatch/missing metadata,
  split storage failures, pairing/manual request statuses, QR selection and
  independent decode, stale QR completion, reconnect timer cleanup/backoff,
  startup state transitions, settings aliases, and component accessibility
  contracts.
- `pnpm --dir frontend check`: passed with 0 errors and 0 warnings.
- `pnpm --dir frontend lint`: passed.
- `pnpm --dir frontend build`: passed; the static adapter produced the bundled
  site with the local QR encoder.
- `cargo test -p noor-server -p noor-app`: noor-server 1,644 passed and 5
  ignored; noor-app 18 unit tests and 3 integration tests passed. The existing
  noor-server dead-code warning remained.

Outstanding browser, packaged, and physical evidence:

- A bounded Playwright shell-smoke attempt at desktop (1280x800) and narrow
  phone (390x844) widths did not reach authenticated pages because the temporary
  Vite origins used by the fixture were outside the server's configured trusted
  development origin and were correctly rejected by CORS. Those attempts are
  not counted as UI or accessibility evidence. No screenshots or temporary
  fixture data were retained.
- The full interaction matrix still needs a correctly configured Playwright
  fixture: keyboard traversal and focus, host confirmation/cancel, QR refresh
  and address switching, persistence-disabled reload, terminal socket auth,
  device rename/revoke/reset, and installed/portable startup states.
- No installed or portable package has yet proved bundled QR assets, native
  hosting/startup controls, or offline behavior. No physical iPhone/Safari or
  Android/Chrome scan, `.local`/direct-IP connection, control action, or timing
  measurement was performed. The cross-platform and 20-second targets remain
  unproven, as do the Stage 3 real-network cases.

## Post-Stage 4 adversarial review and remediation

Status: the formal code-level `BLOCK` is resolved. Release sign-off remains
blocked on the packaged Windows, real-network, and physical-phone evidence
listed above.

The sequential saboteur, new-hire, and security-auditor review found two
critical issues, seven warnings, and one note. Remediation completed on
2026-09-22:

- Tray network-access changes now read the live authenticated playback snapshot
  and require a native confirmation that states whether playback is active,
  gives the exact queue size, and labels the destructive enable/disable restart
  action. A failed snapshot read or cancellation makes no state change.
- Serialized native transition failures are parsed by the frontend, their
  authoritative state is applied, and native state events keep Settings in
  sync. Failed local-only recovery exposes a restart action that does not
  re-enable LAN access.
- Identity, redemption, and credential probes have five-second abort bounds.
  Network/server outages retain credentials and are distinct from auth
  rejection; explicit retries are capped.
- The desktop lifecycle reserves cleanup time inside one 15-second wall-clock
  budget. Readiness requests and sleeps use the remaining budget, and forced
  child termination no longer calls an unbounded process wait.
- Pair redemption requires the request Host and, when present, Origin to match
  the exact origin minted into the ticket. A wrong authority does not consume
  the ticket.
- Start-at-sign-in checks the scoped HKCU Run value against the current
  executable plus explicit silent-launch marker, repairs stale registrations
  through the owning plugin, and verifies the exact rewritten command.
- Exposure labels distinguish local-only operation from an enabled but
  unavailable LAN listener. Clipboard failure focuses and selects a readonly
  URL field for manual copy.
- Graceful server shutdown sends WebSocket close code 1001. Authentication
  reset and device revocation retain higher-priority terminal code 4001.

Final automated verification on 2026-09-22:

- `pnpm --dir frontend test`: 147 files, 887 tests passed.
- `pnpm --dir frontend check`: 0 errors and 0 warnings.
- `pnpm --dir frontend lint`: passed.
- `pnpm --dir frontend build`: passed.
- `cargo test -p noor-app`: 23 unit and 3 integration tests passed.
- `cargo test -p noor-server`: 1,647 passed, 5 ignored, 0 failed.
- `cargo check -p noor-app -p noor-server`: passed with five existing
  noor-server dead-code warnings.
- `cargo fmt --all --check`: passed.
- `git diff --check`: passed.

Local packaged-test setup on 2026-09-22:

- Created a SQLite-consistent online backup of the active installed database at
  `.scratch/phone-remote-installed/LocalAppData/NOORwave/noor.db`; `quick_check`
  returned `ok`. The live WAL was not copied raw and the live profile was not
  used by the test launch.
- Built `dist/NOORwave-portable.zip` and confirmed it contains the desktop app,
  sidecar, and bundled `www/index.html`.
- Built and silently installed the unsigned local NSIS package over 0.13.10.
  The installed sidecar matches the release build, the installed app reports
  0.13.10 from the new build window, and bundled UI assets are present.
- Launched the installed app with process-scoped `LOCALAPPDATA` redirected to
  the fixture. Its desktop-owned sidecar reached readiness, served `/remote`,
  exposed public identity, reported bundled assets ready, remained local-only,
  and opened the copied database with stopped playback and an empty boot queue.
  The live database size and modification time remained unchanged by this
  fixture launch.
- Added `scripts/prepare-phone-remote-test.ps1` and
  `scripts/launch-installed-phone-remote-test.ps1` for repeatable isolated
  fixture creation and launch. The launcher refuses to run beside any existing
  NOORwave process so single-instance arbitration cannot redirect it into the
  live profile.

This setup does not yet prove LAN enable/restart, DNS-SD discovery, QR camera
redemption, phone controls, autostart, or the timing targets. Those remain in
the manual matrix below.

Real-iPhone findings and remediation on 2026-09-22:

- Safari redeemed the QR successfully, but an already-installed Home Screen
  PWA asked for the PIN because iOS isolates its storage from Safari.
- Pairing tickets now include a six-digit, two-minute, one-use code that
  the installed PWA can redeem itself. It creates the same individually
  revocable device credential as the QR and never exposes the master PIN.
- The master PIN is presented as an optional recovery path; desktop loopback
  remains automatic and returning paired phones reconnect with their device
  credential.
- A Library -> Tracks report was reproduced at the server boundary: a paired
  device received HTTP 200. Client recovery was hardened so one route/socket
  rejection revalidates the saved credential before deleting it; only a
  confirmed rejected probe forgets the pairing.

## Next stage

Build installed and portable Windows artifacts, then execute the outstanding
packaged lifecycle/autostart matrix and real iPhone/Safari plus Android/Chrome
pairing matrix. Do not treat source-contract tests or the rejected temporary
smoke fixture as packaged-browser or physical-phone evidence.
