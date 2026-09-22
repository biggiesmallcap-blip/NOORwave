# Spec: Phone Remote connection and pairing

**Author:** Astra (Codex), grounded in repository inspection
**Date:** 2026-09-21
**Status:** Approved
**Reviewers:** Felix (product and security decisions, approved 2026-09-21); implementation owner; independent code reviewer
**Implementation owner:** Sol, after approval of this specification
**Related documents:** [README remote setup](../../README.md), [release checklist](../release-checklist.md)
**Delivery:** One coherent desktop + server + browser release, with the checkpoints below. This document does not authorize an internet-facing service or installer/firewall changes.

## Context

Felix reports difficulty connecting the existing phone remote and wants a recognizable address, a QR code, or local discovery. The product target is a first phone controlling an already-installed desktop on the same home LAN in approximately 20 seconds, without entering an IP address or PIN. This is a target to measure, not an existing measured baseline. The remote web application already exists at `/remote`; the work is connection, pairing, and recovery, not a replacement music player.

The server is Axum, the desktop is a Tauri shell supervising a Rust sidecar, and the frontend is a static SvelteKit application served by that sidecar. Network access currently lives in the tray. The shell stores `host_mode` in `noor-config.json`, the server stores a second preference in SQLite, and the actual listener is bound only at startup. Changing the preference does not itself rebind the socket. Restarting the server currently stops playback, clears the transient queue, and interrupts in-process work. A settings toggle must acknowledge that behavior and avoid reporting the saved preference as actual connectivity.

Every protected route currently accepts the same six-digit bearer PIN. `/api/setup/token` provides it only to loopback peers with trusted browser origins; the phone instead enters the PIN in the root layout. Browser HTTP and WebSocket requests already follow the page's production origin. These foundations support additive, single-use QR tickets and per-device credentials, provided paired devices cannot subsequently retrieve the shared PIN and bypass revocation. The legacy shared PIN remains a compatibility path, with its weaker revocation semantics made explicit.

The delivery uses the existing HTTP listener and a bundled mDNS responder. It advertises a friendly `.local` hostname and DNS-SD service while LAN access is running. Browser-side LAN enumeration is excluded. A phone cannot execute fallback JavaScript when its original hostname never resolves, so the desktop also provides a direct-IP QR option. HTTP on a LAN does not become encrypted or a secure browser context because the URL has a friendly name; this release is for a trusted local network.

## Decisions for review

Approval of this specification accepts the following concrete choices together. The implementation owner does not need separate permission for routine code edits covered here.

| Decision | Recommendation and consequence | Alternative considered |
|---|---|---|
| D-1: Delivery scope | Ship settings, controlled LAN restart, mDNS, single-use QR tickets, device credentials/revocation, PIN compatibility, and honest diagnostics together. | QR containing the permanent PIN is rejected: it would create an avoidable credential-sharing format to replace later. |
| D-2: Changing LAN mode | Retain a controlled server restart in this delivery. The enable/disable action explicitly says that playback stops and the current queue clears. | A live listener manager could avoid this, but changes listener ownership, shutdown, and audio lifecycle and requires its own specification. |
| D-3: Trust and compatibility | Retain local HTTP and legacy PIN authentication. New QR devices are individually revocable; people who know the shared PIN can still reconnect until it is regenerated. | TLS provisioning or removing legacy PIN access is a materially larger/breaking change. |
| D-4: Credential authority | LAN-authenticated phones get existing music/library/operator capabilities, whether paired by QR or connected through the fallback PIN, but no LAN principal may administer server exposure or credentials. All connection administration requires loopback, trusted origin, and the current shared PIN. | A comprehensive route-by-route least-privilege role system is deferred; this release does not describe a paired phone as a read-only or playback-only guest. |
| D-5: Hostname and fallback | Prefer the responder-confirmed `noorwave.local`, persist collision renames, and offer a separate direct-IP QR. Start with normal IPv4 LAN hosting. | No OS computer rename, hosts-file edit, custom URL scheme, port 80, or automatic IPv6 listener redesign. |
| D-6: Reset semantics | Rename the action **Reset all remote access**. It regenerates the fallback PIN, revokes all paired devices, invalidates tickets, and closes authenticated sockets after confirming the number of affected devices. | A button still labelled only "Regenerate PIN" would understate the broader consequence; rotating only the legacy PIN would contradict the present disconnect-all promise. |
| D-7: Launch behavior | The Phone Remote panel exposes **Make phone remote available whenever NOORwave is running**. Once enabled, every later NOORwave launch starts the sidecar in LAN mode and advertises it without another restart or tray action. | This does not launch NOORwave with the operating system and does not create a separate background service. |
| D-8: Start at sign-in | Installed Windows builds expose **Start NOORwave in the tray when I sign in**. The OS starts the ordinary per-user desktop process with an autostart marker; NOORwave creates the tray and managed sidecar but does not show/focus the main window. | A Windows service/pre-login boot process is rejected because it would split lifecycle ownership and run outside the interactive user/audio session. Portable builds show why this control is unavailable. |

D-2 and D-3 are the principal product tradeoffs. D-3, D-4, and D-6 are authentication/security decisions requiring explicit review before implementation under the spec-first workflow. Status remains **In Review** until accepted.

## Repository evidence and implementation constraints

| Area | Inspected source | Consequence |
|---|---|---|
| Listener selection and startup | `noor-server/src/main.rs`, `resolve_bind_addr`, startup playback/queue reset | `NOOR_ADDR` overrides `--host`, which overrides `server.host_mode`; the bound address must be retained separately from preferences. |
| Public/static/auth router | `noor-server/src/server/mod.rs` | Preserve public SPA fallback and loopback setup gates; middleware must still use protected `route_layer` so `/remote` and assets load without credentials. |
| Token and host endpoints | `noor-server/src/server/routes.rs`, `get_server_token_handler`, `regenerate_server_token_handler`, `get_server_info`, `put_server_host_mode` | Current host info is inferred from the DB, not the socket. Current token getter would let an unrestricted new device obtain the shared PIN. |
| PIN storage | `noor-server/src/db/queries.rs`, `ensure_server_token` | Preserve valid existing six-digit values and the existing storage key; new long device tokens must never pass through PIN normalization. |
| WebSockets | `noor-server/src/server/ws.rs`, `frontend/src/lib/api/ws.ts` | Auth currently runs only at upgrade; sockets require revocation tracking. Current browser reconnect loop runs every three seconds without terminal auth handling. |
| Desktop lifecycle | `noor-app/src/{main,sidecar,tray,config,commands,server_url}.rs` | Reuse sidecar shutdown/readiness, serialize mode changes, propagate errors; configuration writes currently swallow failures. |
| Desktop permissions | `noor-app/capabilities/default.json` | Remote capability URL is currently fixed to `http://127.0.0.1:17600/**`; customized-port IPC must be explicitly accounted for without granting LAN pages native authority. |
| Connection gate | `frontend/src/routes/+layout.svelte`, `layout_auth_gate_contract.test.ts` | Process pairing before loopback auto-setup, stored-token startup, and protected route mounting. |
| Settings | `frontend/src/routes/settings/+page.svelte`, `frontend/src/lib/components/settings/settingsSearch.ts` | Add a separate component and searchable section; do not enlarge the existing settings monolith with networking logic. |
| API and browser storage | `frontend/src/lib/api/client.ts` | Production is same-origin; token key is `noor_api_token`. Auth fetches raise `noor:unauthorized` on 401. |
| Migrations | `noor-server/src/db/schema.rs`, `db/mod.rs` | Current migrations end at 062; append the next available migration. The runner records completion outside migration SQL and does not wrap each migration automatically. |
| Distribution | `frontend/svelte.config.js`, `noor-app/tauri.installer.conf.json`, `noor-app/nsis-hooks.nsh`, `scripts/build-portable.ps1` | Bundle QR code generation and responder dependencies locally. Existing `www` packaging remains sufficient; no Node runtime or Bonjour installer on user computers. |
| Verification | Cargo unit/router tests, frontend Vitest, `frontend/scripts/smoke.mjs` | Existing route tests often construct only the inner API router; security tests must also exercise the fully assembled middleware stack with synthetic socket peers. |

## Functional Requirements

- FR-1: Settings MUST expose a searchable **Phone Remote** section under Account containing actual connection status, the LAN action, pairing controls, manual fallback, and paired devices.
- FR-2: Fresh installs MUST keep LAN hosting disabled until the user enables it; upgrades MUST preserve the existing desktop host-mode preference.
- FR-3: Desktop Settings and the tray MUST use the same serialized native host-mode operation and publish the same resulting state.
- FR-4: A host-mode operation MUST report success only after persisted configuration, the owned sidecar, and its actual bound listener agree; failed operations MUST follow the recovery rules below.
- FR-5: Before a host-mode restart, the UI MUST disclose whether playback is active and the exact current queue count, and MUST offer **Cancel** and an explicit enable/disable-and-restart action; the restart MUST never begin merely by opening Settings or changing an unrelated preference.
- FR-6: Standalone hosting and externally forced bind modes MUST report their actual state and control source; unsupported UI changes MUST return a specific error rather than a false success.
- FR-7: A LAN-running server with a usable address and packaged remote assets MUST advertise `_noorwave._tcp.local.` with its actual port, `/remote` path, protocol version, and persistent server ID.
- FR-8: The responder MUST start with the persisted hostname or `noorwave.local.`, detect conflicts, persist the effective renamed hostname, and expose only a confirmed advertised friendly URL as usable.
- FR-9: Address selection MUST use active interfaces reachable by the actual listener, exclude loopback/unspecified/multicast/link-local addresses from phone URLs, rank physical LAN interfaces ahead of VPN/virtual interfaces, and permit explicit selection of other usable addresses.
- FR-10: Disabling hosting or shutting down MUST withdraw discovery and invalidate outstanding tickets; interface changes MUST refresh advertised addresses and invalidate QR links tied to removed addresses or renamed hostnames.
- FR-11: The desktop MUST generate a QR locally from a short-lived, one-use pairing URL; the QR MUST NOT contain the permanent PIN or a long-lived device credential.
- FR-12: The pairing service MUST permit at most one pending ticket, expire it after 120 seconds, and invalidate its predecessor on refresh or address selection changes.
- FR-13: A valid ticket redemption MUST atomically consume the ticket and persist exactly one new device credential; failed persistence MUST NOT yield a usable credential.
- FR-14: The browser MUST process a `pair` URL fragment before normal authentication bootstrap and remove the secret from the visible URL before making a network request.
- FR-15: A paired browser MUST retain its credential across reloads on the same origin, bind its metadata to the expected server ID, and enter the remote without PIN entry after a successful exchange.
- FR-16: Protected HTTP, media URL, and WebSocket requests MUST accept valid paired-device credentials as well as the existing shared PIN, with the explicit paired-device administration restrictions below.
- FR-17: A LAN request authenticated by either a paired-device credential or the fallback shared PIN MUST NOT read/regenerate the shared PIN, change host mode, create pairing tickets, list/revoke devices, or perform other `/api/server/*` administration; `GET /api/server/info` is the sole exception. The same shared PIN MAY authorize those operations only from a loopback peer with a trusted origin.
- FR-18: New connection-management APIs MUST require a loopback peer, the existing trusted-origin gate, and the current shared PIN; the unauthenticated loopback setup-token endpoint MUST keep its existing network boundary.
- FR-19: The desktop MUST list paired devices with name, paired time, and last-seen time, and allow individual revocation; revocation MUST reject subsequent requests and terminate that device's existing WebSockets.
- FR-20: Existing manual URL + six-digit PIN connections MUST continue to work; the manual fallback MUST explain that shared-PIN connections are not individually listed or revoked.
- FR-20a: Because iPhone Home Screen web apps do not share local storage with Safari after installation, each QR ticket MUST also expose a six-digit, short-lived, one-use pairing code. An already-installed PWA MAY redeem that code from any currently offered origin for the same server. The master PIN remains a separate, concealed recovery path rather than the normal onboarding path.
- FR-21: The desktop action **Reset all remote access** MUST confirm the count of paired devices that will be disconnected, then regenerate the shared PIN, invalidate all outstanding tickets, revoke all device credentials, reject the previous PIN, and close sessions authenticated before the reset.
- FR-22: The desktop MUST expose both the friendly URL and usable direct-IP URLs, with copy/open actions and a switchable IP-based QR when `.local` access fails.
- FR-23: The UI MUST distinguish known local failures from unverified phone reachability; firewall, guest-Wi-Fi isolation, and VPN explanations MUST be presented as troubleshooting possibilities unless directly observed.
- FR-24: The phone MUST distinguish rejected/expired pairing, rate limiting, rejected credentials, and network loss, and MUST stop automatic socket retries for terminal authentication failures.
- FR-25: The server MUST retain paired-device hashes and server identity across restart/upgrade without changing music or service-auth data; pending tickets MUST remain memory-only.
- FR-26: The distributed app MUST serve the complete connection and QR flow from bundled assets without a cloud QR service, third-party web scripts, router configuration, or separately installed discovery daemon.
- FR-27: The remote MUST remain usable as an ordinary HTTP browser page when secure-context APIs, clipboard APIs, persistent storage, or PWA installation are unavailable.
- FR-28: Connection and credential diagnostics MUST exclude secrets from logging and copied diagnostic text, including the currently logged startup PIN.
- FR-29: When persistent phone-remote availability is enabled, every subsequent NOORwave launch MUST start its owned sidecar directly in effective LAN mode and begin eligible discovery without another user action; when disabled, launches MUST remain loopback-only.
- FR-30: Installed Windows builds MUST expose a searchable **Start NOORwave in the tray when I sign in** setting whose displayed value reflects the actual per-user OS autostart registration, not only a cached preference.
- FR-31: Enabling start-at-sign-in MUST register the installed executable with an explicit autostart launch marker; disabling it MUST remove that registration, and either operation MUST report an OS/plugin failure without displaying the requested state as successful.
- FR-32: An autostart-marked launch MUST create the tray and start the ordinary managed sidecar while keeping the main window hidden and unfocused; it MUST apply FR-29's persisted phone-remote mode without playing audio or requiring interaction.
- FR-33: A normal user launch MUST continue to show the main window. If NOORwave is already running from autostart, a second normal launch MUST activate/show the existing window and MUST NOT start a second sidecar or compete for the server port.
- FR-34: Portable and otherwise unsupported builds MUST NOT create a startup registration; they MUST render the setting disabled with the explanation **Install NOORwave to enable start-at-sign-in**.
- FR-35: Tray **Quit**, user sign-out, and normal process termination MUST stop the managed sidecar and discovery using the existing bounded shutdown behavior; disabling start-at-sign-in MUST NOT terminate the currently running app.

## Non-Functional Requirements

- NFR-1: On the supported Windows 11 + same-LAN test fixture, at least 9 of 10 first-connection attempts SHOULD finish in 20 seconds from pressing **Enable and restart** to a successful remote pause/play command, with a camera-ready user and previously initialized database. Record measured timings; exclude OS permission prompts and do not claim this result without measurements.
- NFR-2: A successful desktop host-mode transition MUST finish or return a bounded error within 15 seconds; visible pairing/status calls MUST time out within 5 seconds; QR rendering after ticket creation MUST finish within 500 ms on the desktop fixture.
- NFR-3: New status/list/ticket/redeem endpoints MUST achieve p95 below 500 ms over 200 warm requests with 10 connected devices and no concurrent bulk database maintenance; verify separately that maintenance yields bounded errors rather than hung UI.
- NFR-4: Tickets and device secrets MUST each contain at least 256 bits from an OS-backed cryptographic random source; the server MUST persist only SHA-256 digests of device secrets and MUST NOT return those digests in any API.
- NFR-5: Invalid credential or ticket attempts MUST be limited to eight per socket-source IP per rolling 60 seconds and 60 per process per rolling 60 seconds, with 429 and `Retry-After` thereafter. Valid existing authenticated API calls MUST remain usable during an invalid-attempt flood. Bucket storage MUST be capped at 4,096 entries and evicted after ten idle minutes.
- NFR-6: Pairing POST bodies MUST be limited to 2 KiB; device names MUST contain 1–64 Unicode scalar values after trimming, exclude control characters, and render as text. The server MUST cap active paired devices at 32 and active tickets at one.
- NFR-7: Revocation MUST deny every authorization check begun after successful revocation and close affected WebSockets within one second; already authorized in-flight HTTP operations are allowed to finish. Last-seen persistence MUST write at most once per device per minute.
- NFR-8: While hosting is disabled, discovery MUST use no multicast sockets or periodic network polling. While enabled, discovery/interface state MUST refresh within ten seconds of a detected change, and graceful unregister/shutdown MUST add no more than one second to sidecar shutdown.
- NFR-9: All controls MUST be keyboard reachable and labelled, error messages MUST be announced, text contrast MUST be at least 4.5:1, and phone touch targets MUST be at least 44 by 44 CSS pixels. QR MUST have a four-module quiet zone, error correction M or better, and an accessible textual connection alternative.
- NFR-10: Migration and restart tests MUST preserve 100% of seeded existing library/config/service-auth rows except the already documented transient playback/queue reset; repeated migration execution MUST succeed after simulated interruption before the migration completion row.
- NFR-11: Automated tests MUST cover HTTP header authentication, existing URL-query authentication, WebSocket authentication and revocation, loopback/origin authorization, two concurrent redemptions, expiration using an injected monotonic clock, and empty/failed persistent storage.
- NFR-12: On the supported installed Windows fixture, an autostart-marked cold launch MUST create no visible/focused application window, MUST expose a usable tray action within five seconds, and MUST reach the same bounded sidecar-ready result as a normal launch within 15 seconds.

## Acceptance Criteria

### AC-1: Find Phone Remote (FR-1, NFR-9)

Given Settings is open,
When the user searches for "phone", "remote", "QR", "network", or "pair",
Then the Phone Remote section is reachable with its labelled controls and current state.

### AC-2: Fresh default and upgrade preference (FR-2, FR-25)

Given a parameterized fresh installation or an existing true/false desktop host preference,
When this version starts,
Then fresh hosting is disabled and each existing preference is preserved without altering music or service-auth records.

### AC-3: Enable from Settings (FR-3, FR-4, FR-5, NFR-2)

Given a healthy desktop-owned sidecar is local-only and the confirmation states whether playback is active and the exact number of queued tracks that will be cleared,
When the user invokes **Enable and restart**,
Then the serialized native operation persists the choice, starts a LAN listener, reconciles the server preference, updates the tray, and returns running status within 15 seconds.

### AC-4: Tray and Settings serialize changes (FR-3, FR-4)

Given a host transition is in progress,
When another host transition is requested from the other surface,
Then it returns `TRANSITION_IN_PROGRESS` without a second restart or preference write, and both surfaces display the first operation's outcome.

### AC-5: Enable failure recovery (FR-4, NFR-2)

Given the desktop was local-only and the replacement LAN sidecar fails readiness,
When the enable operation completes its bounded recovery,
Then it returns an error, restores the previous disabled preference, attempts one local-only restart, and never reports a working phone URL.

### AC-6: Disable failure is closed (FR-4, FR-10)

Given LAN hosting is running and a disable request has successfully persisted disabled state,
When restarting in local-only mode fails,
Then no owned LAN child remains, discovery is stopped, disabled preference remains saved, and the UI reports that the local server needs recovery.

### AC-7: Override and standalone mode (FR-6)

Given a server launched with `NOOR_ADDR`, standalone `--host`, or a desktop-managed host setting,
When the local settings page reads status or requests an unsupported mode write,
Then status identifies the actual listener and control source, and an incompatible write returns `EXTERNAL_BIND_OVERRIDE` or `DESKTOP_MANAGED` without changing preferences.

### AC-8: Standalone preference needs external restart (FR-6)

Given standalone mode without a forcing override,
When the local user saves host mode through the HTTP endpoint,
Then the response distinguishes saved mode from actual mode, says `restart_required: true`, and the UI instructs the user to restart their server process.

### AC-9: Advertise an actual running service (FR-7, FR-8, FR-26)

Given hosting is running on port 17600 with usable IPv4 and bundled assets,
When a test DNS-SD client queries the LAN,
Then it resolves `_noorwave._tcp.local.` to the confirmed hostname, actual address and port, and TXT `path=/remote`, `protocol=1`, and the stable server ID, with no secret fields.

### AC-10: Collision and restart stability (FR-8, FR-25)

Given another responder owns `noorwave.local`,
When NOORwave completes conflict resolution and is subsequently restarted,
Then status and fresh QR use the conflict-free persisted name, such as `noorwave-2.local`, and no stale friendly name is described as ready.

### AC-11: Interface selection and unsupported IPv6 (FR-9, FR-22)

Given test interface fixtures containing loopback, Wi-Fi, Ethernet, VPN, virtual, IPv4 link-local, and IPv6 addresses with an IPv4-only listener,
When addresses are ranked,
Then only listener-compatible usable IPv4 URLs are offered, physical LAN addresses appear first, other usable interfaces require explicit selection, and no AAAA record is advertised.

### AC-12: Network changes and shutdown (FR-10, NFR-8)

Given a QR refers to an address removed from an active interface,
When the address snapshot refreshes or the server shuts down,
Then affected tickets become invalid, stale records are withdrawn, and status no longer offers that QR within the specified update/shutdown budgets.

### AC-13: QR content and expiry display (FR-11, FR-12, NFR-4, NFR-9)

Given the desktop opens pairing while hosting is ready,
When it creates a ticket and renders the QR,
Then decoding the QR yields `/remote#pair=<ticket-secret>` at an approved current origin, the 120-second expiry is shown, and neither the shared PIN nor a device credential appears in that URL.

### AC-14: Single-use concurrent redemption (FR-13, NFR-11)

Given one valid ticket,
When two redemption requests execute concurrently,
Then exactly one returns 201 with a device credential, the other returns `PAIRING_INVALID`, and exactly one new device row exists.

### AC-15: Expired or refreshed ticket (FR-12, FR-24)

Given a ticket expired at the 120-second boundary or replaced by Refresh QR,
When a phone redeems it,
Then no device is created and the phone says the link expired or was already used, with actions to scan a new QR or enter the PIN.

### AC-16: Pair before normal bootstrap (FR-14, FR-15)

Given a fresh phone opens a valid pairing URL,
When the root layout initializes,
Then the fragment is cleared before the first fetch, one redemption runs, protected routes and WebSocket startup wait for success, and the remote appears without a PIN modal.

### AC-17: Reload and changed server identity (FR-15, FR-25)

Given a browser has a stored device token and server ID,
When the page reloads against either that server or a different ID at the same origin,
Then it reconnects to the original server automatically, or clears the mismatched credential and requests pairing without sending it to the mismatched server.

### AC-18: Device transport compatibility (FR-16, NFR-11)

Given a valid paired-device token,
When the browser calls a protected music API, requests an existing authenticated media URL, and opens `/ws`,
Then all three authenticate successfully and protected APIs still reject missing or invalid credentials.

### AC-19: Prevent credential escalation (FR-17, FR-18)

Given either a paired-device token or the current fallback PIN is presented from a LAN peer,
When it requests `/api/server/token`, token reset, host changes, pairing management, or other server administration,
Then each request returns 403 and no PIN, device list, ticket, or administration side effect is exposed; read-only server info remains accessible and ordinary music control remains authorized.

### AC-20: Preserve loopback boundary (FR-18)

Given a matrix of loopback/LAN peers, trusted/foreign browser origins, and absent/shared/device credentials,
When setup-token or new management endpoints are requested,
Then setup-token retains its loopback/trusted-origin behavior and new management succeeds only for a loopback trusted-origin request with the current shared PIN.

### AC-21: Revoke one device (FR-19, NFR-7)

Given two paired devices with open WebSockets,
When the desktop revokes one,
Then its next HTTP request fails, its socket closes within one second, and the other device continues functioning with unchanged credentials.

### AC-22: Preserve manual PIN (FR-20)

Given an existing shared-PIN browser or a fresh phone at a manually entered direct-IP URL,
When it submits the current six digits,
Then it connects using the existing bearer path and the UI explains that this session cannot be individually revoked from the paired-device list.

### AC-22a: Pair an already-installed iPhone PWA (FR-20a)

Given an iPhone Home Screen app whose storage is isolated from the Safari session,
When the desktop creates a pairing ticket and the user enters its temporary code in that app,
Then the app receives and persists an individually revocable device credential without learning the master PIN, and a second use, expiry, refresh, cancellation, or reset rejects that code.

### AC-23: Reset every remote credential (FR-21)

Given the current PIN, two paired devices, a pending QR, and authenticated sockets,
When the loopback desktop confirms **Reset all remote access** with the affected-device count,
Then the old PIN, both device tokens, and the ticket fail subsequently, old sockets close, the new PIN works, and the desktop reloads its local credential.

### AC-24: mDNS fails before page load (FR-22, FR-23)

Given the phone cannot resolve `.local` but can reach the server's selected IPv4 address,
When the desktop user switches the QR to the direct-IP option and scans again,
Then a fresh ticket pairs at that IP origin without claiming the failed phone page performed an automatic redirect.

### AC-25: Honest diagnostics (FR-23)

Given local listener and discovery checks pass but no phone request has reached the server,
When the user opens troubleshooting,
Then it reports locally observed readiness, labels phone reachability as unverified, and offers same-Wi-Fi, private firewall permission, guest isolation, and VPN checks without declaring any one the proven cause.

### AC-26: Network loss versus revoked credentials (FR-24)

Given a connected phone,
When its network disappears or its device token is revoked,
Then network loss uses bounded backoff without deleting credentials, while explicit 401/revocation clears that device session and cancels automatic reconnect until reauthentication.

### AC-27: Persistence failure and retry (FR-13, FR-25)

Given a valid ticket and a database that rejects the device insertion,
When redemption runs,
Then it returns `STORAGE_UNAVAILABLE`, no credential is returned, and the ticket remains redeemable until its original expiry once storage recovers.

### AC-28: Packaged/offline assets (FR-26)

Given an installed or portable build with outbound internet blocked but LAN available,
When the desktop generates QR and the phone loads and pairs with `/remote`,
Then no QR/CDN/Node/Bonjour dependency is fetched and static deep links remain unauthenticated before the auth gate.

### AC-29: Browser capability fallback (FR-27, NFR-11)

Given an HTTP browser without clipboard permission, secure-context APIs, or writable localStorage,
When it uses the connection flow,
Then URLs can be selected/copied manually, pairing works for the current session using memory storage, and a message explains when the connection cannot be remembered.

### AC-30: Credential redaction (FR-28, NFR-4)

Given startup, QR creation/redemption, WebSocket upgrade, PIN reset, and copied troubleshooting details,
When logs and diagnostic output are inspected,
Then no raw PIN, ticket, device secret, digest, authorization header, or token-bearing request URL appears.

### AC-31: Throttling without disrupting valid sessions (NFR-5)

Given eight invalid attempts from one socket-source IP in 60 seconds,
When another invalid attempt and a valid existing API request arrive,
Then the invalid attempt receives 429 with `Retry-After`, the valid authenticated API still succeeds, spoofed forwarding headers do not bypass the bucket, and the process-wide limit is independently enforced.

### AC-32: Input and device limits (NFR-6)

Given parameterized invalid names, an over-2-KiB body, or 32 active devices,
When a new pairing is attempted,
Then it receives the documented 400/413/409 response with no extra device or memory growth, while a valid name is displayed as text.

### AC-33: Measured connection and API budgets (NFR-1, NFR-2, NFR-3)

Given the documented warm Windows/phone fixture and the 200-request API fixture,
When the connection exercise and endpoint timing tests run,
Then required timeout/p95 budgets pass and the report states the actual fraction of sub-20-second first connections instead of inferring phone success from local readiness.

### AC-34: Migration re-entry (FR-25, NFR-10)

Given a database through migration 062, including a run interrupted after new DDL but before its completion row,
When migrations run twice,
Then existing seeded data is preserved, remote tables/indexes exist once, and the migration completion row is recorded exactly once.

### AC-35: Accessible connection actions (NFR-9)

Given desktop keyboard and phone-width browser fixtures,
When every pairing, copy, manual input, error, and revoke state is exercised,
Then labelled controls, focus indicators, error announcements, contrast, touch targets, and the QR quiet zone meet the specified thresholds.

### AC-36: Phone remote follows NOORwave launch (FR-2, FR-3, FR-29)

Given **Make phone remote available whenever NOORwave is running** was enabled and NOORwave was exited normally,
When the user launches NOORwave again,
Then the owned sidecar starts directly in effective LAN mode, discovery starts when its prerequisites are available, Settings and the tray both show the enabled state, and no additional restart or tray interaction is required.

### AC-37: Disabled phone remote remains local (FR-2, FR-3, FR-29)

Given the same setting is disabled,
When the user launches NOORwave,
Then the owned sidecar binds loopback-only and does not advertise the remote.

### AC-38: Enable start-at-sign-in (FR-30, FR-31)

Given an installed Windows build with no NOORwave autostart registration,
When the user enables **Start NOORwave in the tray when I sign in**,
Then the per-user registration is created with the explicit autostart marker, querying startup state returns enabled, and Settings shows enabled only after registration succeeds.

### AC-39: Silent autostart with phone remote (FR-29, FR-32, NFR-12)

Given start-at-sign-in and persistent phone-remote availability are enabled,
When Windows invokes the registered autostart command in a new user session,
Then NOORwave creates no visible/focused main window, exposes its tray action within five seconds, starts exactly one managed sidecar directly in effective LAN mode, and begins eligible discovery without interaction.

### AC-40: Manual launch activates the existing instance (FR-33)

Given NOORwave is already running hidden from autostart,
When the user launches NOORwave normally,
Then the existing process shows and focuses its main window, no second owned sidecar starts, and the existing server remains available on its current port.

### AC-41: Disable start-at-sign-in without stopping now (FR-31, FR-35)

Given NOORwave is running and registered for start-at-sign-in,
When the user disables the setting,
Then the per-user registration is removed and the current app, sidecar, playback, and tray continue running until the user exits normally.

### AC-42: Portable build explains unavailable startup (FR-34)

Given a portable Windows build,
When the user views the startup setting or attempts an equivalent native command,
Then the control is disabled with **Install NOORwave to enable start-at-sign-in**, the command returns a specific unsupported-install-mode error, and no OS registration is created.

## Edge Cases and Error Scenarios

- EC-1: `noor-config.json` cannot be written or serialized -> native operation fails before stopping the current sidecar; preserve the prior file and visible state (AC-3, AC-5).
- EC-2: LAN bind is denied or the port is occupied -> never advertise; restore the prior disabled preference after failed enable, attempt one local-only recovery, and surface the real error category (AC-5).
- EC-3: Disable restart fails -> leave the owned server stopped and disabled preference saved; do not restart in the old LAN mode (AC-6).
- EC-4: `NOOR_ADDR`/CLI override disagrees with preferences -> mark external control, show the actual address, and disable unsupported host controls. A desktop override excluding IPv4 loopback is unsupported by the existing shell readiness URL and must get a startup diagnostic (AC-7).
- EC-5: UDP 5353 is unavailable or multicast is blocked -> HTTP/PIN/IP QR continue; discovery reports unavailable. Do not install Bonjour or modify firewall rules automatically (AC-24, AC-25).
- EC-6: A hostname collision occurs on one of several interfaces -> converge all advertisements on one new effective hostname, persist it, invalidate the old hostname ticket, and regenerate only on explicit UI action (AC-10).
- EC-7: VPN is the default route, there are multiple NICs, or no ordinary LAN address exists -> rank physical candidates independently of internet default-route discovery, expose other usable candidates explicitly, and never manufacture a phone URL from `0.0.0.0` (AC-11).
- EC-8: Wi-Fi changes during QR display, sleep/resume, or DHCP renewal -> refresh the snapshot, withdraw removed addresses, expire the affected QR, retain device credentials and server ID (AC-12).
- EC-9: IPv6-only network -> normal hosting reports no supported IPv4 phone address. Do not emit misleading AAAA records or an unscoped `fe80::` URL. Explicit standalone IPv6 binds remain an advanced unsupported combination for automatic phone setup (AC-11).
- EC-10: QR expired, was refreshed, redeemed by a second phone, or server restarted -> return the same `PAIRING_INVALID` response and offer new QR/PIN; do not reveal ticket-existence details (AC-14, AC-15).
- EC-11: Response is lost after successful pairing commit -> ticket remains spent and a device row can exist without a saved phone token. Ask for a fresh QR and permit removing the unused row; do not automatically reissue its secret (AC-14, AC-27).
- EC-12: SQLite busy/disk full/constraint failure -> rollback device insert, return bounded storage error, and keep an unconsumed ticket only if its original expiry remains valid (AC-27).
- EC-13: Clock changes -> ticket validity uses a monotonic deadline, with wall-clock expiry solely for display. Device timestamps remain UTC; time adjustment cannot revive a ticket (AC-15).
- EC-14: Stored credentials are stale, origin now serves another NOORwave, or user switches IP/.local origins -> validate identity before device auth; never move a long-lived credential through a URL or cross-origin redirect. Different origins require their own pairing (AC-17, AC-24).
- EC-15: Storage throws, is cleared, or is private-session-only -> memory fallback works for the session; explain re-pairing on the next visit (AC-29).
- EC-16: Third-party origin attempts setup, minting, or redemption, or supplies `Host`, forwarded headers, protocol or query tricks -> enforce normalized allowed local origins, actual socket peers, strict request format, and origin checks described below (AC-19, AC-20).
- EC-17: Frontend receives 403, 429, or 500 during PIN validation -> do not treat it as a successful login; only a successful protected response enables the remote (AC-22, AC-26, AC-31).
- EC-18: Revocation/reset races with redemption or WebSocket upgrade -> serialize credential mutation and recheck authorization before registering a socket; either operation linearizes first, with no post-reset valid old credential (AC-14, AC-21, AC-23).
- EC-19: Renderer reloads during a native transition -> native operation continues once, state is queryable after readiness, and the restored settings anchor shows the outcome without generating a second restart (AC-3, AC-4).
- EC-20: Built `www/index.html` is absent -> local music/API behavior remains available, status says remote assets are missing, and no working remote URL/QR is offered (AC-9, AC-28).
- EC-21: Local self-check succeeds but guest AP isolation or phone firewall blocks access -> show unverified reachability and troubleshooting, not "phone connected" (AC-25).
- EC-22: Rapid invalid attempts use many source addresses -> bounded bucket map and process-wide rate limit prevent unbounded storage; authenticated clients remain usable (AC-31).
- EC-23: Existing long-lived WS stalls during shutdown/revocation -> close with the appropriate code and honor the sidecar's bounded graceful/forced shutdown budget; retain no orphan advertisement process (AC-6, AC-12, AC-21).
- EC-24: Existing shared-PIN client reaches new device management or a paired browser opens full Settings -> new management remains local-only, and the UI explains "Manage phone connections on the computer" instead of repeatedly failing requests (AC-19, AC-20).
- EC-25: OS autostart registration enable/disable/query fails -> preserve the observed prior state, return a bounded typed error, and do not silently mirror the requested value into app configuration (AC-38, AC-41).
- EC-26: The registered executable is missing after an incomplete update or external move -> the next installed launch reports the stale registration and offers to repair it; portable mode never creates such a registration (AC-38, AC-42).
- EC-27: A user manually launches NOORwave while its autostart instance is still initializing -> single-instance arbitration selects one owner before sidecar spawn; the surviving process eventually shows the requested window and no competing child/listener is created (AC-40).
- EC-28: An autostart-marked launch cannot start the sidecar -> retain the tray process with an actionable unavailable/error state and restart/open/quit actions; do not force the main window to the foreground or falsely advertise the remote (AC-39).

## API Contracts

### Authority, origins, and errors

All new endpoints are same-origin HTTP JSON. Use `Content-Type: application/json` on JSON requests. Management endpoints use `Authorization: Bearer <current-shared-pin>`, require actual loopback `ConnectInfo<SocketAddr>`, and reuse `require_public_loopback_request`; trusted origin alone does not replace the peer check. Normalize IPv4-mapped IPv6 peers before loopback and rate-bucket checks. Ignore forwarding headers for authority/rate decisions; reverse-proxy deployment is outside this delivery.

Keep existing global CORS trust restricted to local desktop/development origins. Phone requests are same-origin and do not need broad CORS allowances. For unauthenticated redemption, reject a supplied Origin unless it matches a current allowed served origin (actual usable listener IP or confirmed friendly hostname, actual port) or a trusted loopback development origin; reject `Origin: null` and cross-site fetch metadata. If Origin is absent, validate Referer when supplied; headerless native clients can redeem only with the ticket. Require JSON so cross-origin HTML forms cannot mint sessions. Validate Host against the server's current advertised/actual local authorities for new public pairing endpoints; a foreign hostname resolving to loopback is not sufficient.

New endpoints and existing token responses return `Cache-Control: no-store`. Pairing pages use `Referrer-Policy: no-referrer`. No access logging includes query strings or request bodies containing credentials. Preserve query-token support because current media requests and WS depend on it, but do not add secrets to any new long-lived navigational link. Reject duplicate credential parameters and ambiguous header/query credentials in new-token paths; preserve established unambiguous legacy behavior.

```typescript
interface RemoteError {
  error: 'INVALID_REQUEST' | 'PAIRING_INVALID' | 'AUTHENTICATION_REQUIRED'
    | 'FORBIDDEN' | 'REMOTE_NOT_READY' | 'ADDRESS_UNAVAILABLE'
    | 'DEVICE_LIMIT_REACHED' | 'NOT_FOUND' | 'RATE_LIMITED'
    | 'STORAGE_UNAVAILABLE' | 'DESKTOP_MANAGED' | 'EXTERNAL_BIND_OVERRIDE';
  message: string;                 // Safe display text; no OS paths/secrets.
  retry_after_seconds?: number;    // Also sent in Retry-After on 429.
}
```

Common new-endpoint errors: 400 `INVALID_REQUEST` for malformed fields/JSON; 401 `AUTHENTICATION_REQUIRED` for missing/invalid management authentication; 403 `FORBIDDEN` for peer/origin/principal denial; 413 for body limit; 415 for non-JSON POST; 429 `RATE_LIMITED`; 503 `STORAGE_UNAVAILABLE` for persistence failures. Unexpected faults return 500 with generic error text. Existing endpoints preserve their established response fields/statuses except the specifically documented managed-host and paired-principal restrictions.

### Public remote identity: GET /api/remote/info

No authentication. Return 200; expose no private device list, interface inventory, PIN, or full diagnostics. This is a readiness hint, not proof that the requesting phone can control playback.

```typescript
interface RemoteIdentity {
  server_id: string;               // Persistent UUID.
  name: 'NOORwave';
  protocol: 1;
  pairing_available: boolean;      // Actual LAN listener + packaged remote assets.
}
```

The existing public `GET /api/ping` stays compatible. Device bootstrap fetches identity before sending an existing paired token to a possibly reassigned address. Identity is not cryptographic server authentication; HTTP's trusted-LAN limit still applies.

### Local management status: GET /api/server/remote

Management authority. Return 200 with measured/cached local state. No network probe to a user-supplied destination. When discovery fails this endpoint still returns status successfully with a diagnostic entry.

```typescript
type HostControl = 'desktop' | 'standalone' | 'environment' | 'command_line';
type RemoteRunState = 'disabled' | 'starting' | 'running' | 'unavailable';
interface RemoteAddress {
  id: string;                     // Opaque ID for this interface/address snapshot.
  label: string;                  // e.g. Wi-Fi; from OS metadata, rendered as text.
  url: string;                    // http://192.168.1.24:17600/remote
  kind: 'lan' | 'other';           // Other includes possible VPN/virtual candidates.
  recommended: boolean;
}
interface RemoteDiagnostic {
  code: 'LOCAL_ONLY' | 'EXTERNAL_CONTROL' | 'NO_USABLE_ADDRESS'
    | 'REMOTE_ASSETS_MISSING' | 'DISCOVERY_UNAVAILABLE'
    | 'DISCOVERY_STARTING' | 'LOCAL_CHECK_FAILED';
  message: string;
}
interface RemoteStatus {
  server_id: string;
  control: HostControl;
  configured_host_mode: boolean;
  effective_host_mode: boolean;
  restart_required: boolean;
  state: RemoteRunState;
  bind_address: string;            // From bound listener.local_addr(), never inferred.
  port: number;
  discovery: {
    state: 'disabled' | 'starting' | 'advertised' | 'unavailable';
    hostname: string | null;       // User-facing form has no trailing dot.
    friendly_url: string | null;   // Non-null only after confirmed advertisement.
  };
  addresses: RemoteAddress[];
  remote_assets_available: boolean;
  phone_reachability: 'unverified'; // Do not infer external reachability from local checks.
  ticket: { id: string; state: 'pending' | 'redeemed' | 'expired'; expires_at: string } | null;
  diagnostics: RemoteDiagnostic[];
}
```

The desktop polls this endpoint and device list at most once every two seconds while the panel is visible, with an explicit refresh button and cancellation on unmount. Confirmed redemption and `last_seen_at` provide evidence of a device contacting the server; the status endpoint does not claim to inspect the phone's router or firewall.

### Create ticket: POST /api/server/remote/pairing

Management authority. An empty JSON object selects the confirmed friendly URL when available, otherwise the recommended direct-IP candidate. Address choices are identifiers from current status, not arbitrary URLs.

```typescript
interface CreatePairingRequest {
  address_id?: string;             // Omit for default; 'friendly' selects confirmed mDNS.
}
interface PairingTicketResponse {
  id: string;
  pairing_url: string;             // http://<approved-host>:<port>/remote#pair=<secret>
  expires_at: string;              // RFC3339 UTC display time.
  expires_in_seconds: 120;
}
```

Return 201. Return 409 `REMOTE_NOT_READY` if hosting/assets/address prerequisites fail, 409 `ADDRESS_UNAVAILABLE` if selection is stale, or 409 `DEVICE_LIMIT_REACHED` at capacity. Validate prerequisites before replacing an existing ticket; successful creation replaces it atomically. No new ticket is minted solely by a background status poll. Refresh is an explicit request. Closing the pairing display calls cancellation best-effort; otherwise expiry handles abandonment.

### Cancel ticket: DELETE /api/server/remote/pairing/{id}

Management authority, no body. Return 204 whether that ticket is already consumed/expired/missing; cancel only a matching pending ID and never a newer ticket. Malformed UUID returns 400. This does not revoke an already paired device.

### Redeem ticket: POST /api/remote/pair

No bearer required; possession of the ticket is the authority and origin/rate checks above apply.

```typescript
interface RedeemPairingRequest {
  ticket: string;                  // Exact URL-safe ticket secret; bounded length.
  device_name?: string;            // 1–64 scalar values; default 'Phone remote'.
}
interface RemoteDevice {
  id: string;
  name: string;
  paired_at: string;
  last_seen_at: string | null;
}
interface PairingResponse {
  token: string;                   // nrp_ + URL-safe base64 of 32 random bytes.
  token_type: 'Bearer';
  server_id: string;
  device: RemoteDevice;
}
```

Return 201 after durable insertion. Expired, unknown, replaced, replayed, or wrong tickets all return 401 `PAIRING_INVALID`. Capacity returns 409 `DEVICE_LIMIT_REACHED`; storage failures return 503. A ticket is invalid whenever hosting ceases to be ready. Browser redemption uses raw fetch, not the global 401 handler, so an expired QR does not race normal auto-setup. Do not silently retry a potentially committed exchange after a network error; offer Scan a new QR.

The optional browser label is a coarse local suggestion such as "iPhone" or "Android phone", not a fingerprint or trusted identity. It does not block one-scan connection. The desktop can rename the label later.

### Device list, label, and revoke

All require management authority:

| Method/path | Request | Success | Endpoint-specific errors |
|---|---|---|---|
| GET /api/server/remote/devices | None | 200 `{ devices: RemoteDevice[] }`, newest paired first | Common errors only |
| PATCH /api/server/remote/devices/{id} | `{ name: string }` with name constraints above | 200 `RemoteDevice` | 404 `NOT_FOUND` |
| DELETE /api/server/remote/devices/{id} | None | 204, idempotent even if absent | Malformed ID: 400 |

Only active devices are returned. Revoke is a hard delete of the credential row and removal from the runtime credential cache after a successful transaction, followed by socket cancellation. Device names are local labels and are never advertised in mDNS.

### Existing token and host endpoints

`GET /api/server/token` retains `{ token: string }` only for loopback, trusted-origin callers presenting the current shared PIN; every LAN principal and every paired principal gets 403. `POST /api/server/token/regenerate` retains its request/response shape but gains the same loopback/trusted-origin boundary and performs the **Reset all remote access** transaction and session invalidation in FR-21. `/api/setup/token` retains its existing loopback/trusted-origin protection and response. The fallback PIN remains compatible for ordinary music/library operation, but knowledge of it from a LAN device is no longer server-administration authority.

`GET /api/server/info` retains `host_mode`, `bind_address`, and `version`, and adds `effective_host_mode`, `restart_required`, and `control`. Legacy `host_mode` remains the saved preference; `bind_address` is corrected to the real listener. The new settings panel uses `/api/server/remote` instead of guessing from this legacy response.

`PUT /api/server/host_mode` keeps `{ host_mode: boolean }` for standalone loopback, trusted-origin shared-PIN callers. Successful response keeps `host_mode` and `bind_address`, and adds `effective_host_mode` and `restart_required`; it never implies an immediate rebind. LAN callers and paired tokens get 403. For desktop-managed processes it returns 409 `DESKTOP_MANAGED`, and forcing environment/CLI modes return 409 `EXTERNAL_BIND_OVERRIDE`. Tray code no longer calls this endpoint.

### Desktop commands and transition state

```typescript
interface DesktopRemoteState {
  configured_host_mode: boolean;
  phase: 'idle' | 'restarting' | 'recovering' | 'failed';
  last_error: string | null;
}
interface SetRemoteHostModeArgs { enabled: boolean }
interface DesktopRemoteError {
  code: 'TRANSITION_IN_PROGRESS' | 'EXTERNAL_BIND_OVERRIDE'
    | 'CONFIG_WRITE_FAILED' | 'SERVER_START_FAILED' | 'STATE_MISMATCH';
  message: string;
  state: DesktopRemoteState;
}
// invoke('get_remote_host_state') -> DesktopRemoteState
// invoke('set_remote_host_mode', { enabled }) -> RemoteStatus or DesktopRemoteError
// desktop event 'remote-host-state-changed' -> DesktopRemoteState
```

Startup registration is a separate native desktop contract:

```typescript
interface DesktopStartupState {
  supported: boolean;             // True only for a supported installed build.
  enabled: boolean;               // Actual OS registration state.
  launch_mode: 'normal' | 'autostart';
  unavailable_reason: 'PORTABLE_BUILD' | 'UNSUPPORTED_PLATFORM' | null;
}
interface DesktopStartupError {
  code: 'UNSUPPORTED_INSTALL_MODE' | 'AUTOSTART_QUERY_FAILED'
    | 'AUTOSTART_ENABLE_FAILED' | 'AUTOSTART_DISABLE_FAILED';
  message: string;
  state: DesktopStartupState;
}
// invoke('get_startup_state') -> DesktopStartupState or DesktopStartupError
// invoke('set_start_at_login', { enabled: boolean }) -> DesktopStartupState or DesktopStartupError
```

Use the official Tauri autostart facility from Rust with an internal `--noor-autostart` argument, and grant no broader web-origin permission than the existing exact loopback desktop origin. Register single-instance arbitration before any sidecar spawn: a secondary normal launch asks the existing process to show/focus its window and exits; an autostart-marked duplicate exits without stealing focus. The OS registration is authoritative for the setting. Do not install a service or duplicate an `autostart` boolean into SQLite. Installed-updater replacement and uninstall cleanup MUST be exercised in packaging tests; repair only registrations that identify this installed application.

The shared Rust implementation behind Settings and the tray:

1. Reject a second operation, `NOOR_ADDR` override, or an unchanged request without restarting. Preserve the caller's settings anchor.
2. Save a merged configuration using a temporary sibling file and recoverable replacement; return a write error instead of swallowing it. Preserve `minimize_to_tray` and other unrelated fields. Serialize with other config writers.
3. Set in-memory desired mode and emit restarting state. Gracefully stop the owned sidecar, bounded by existing forced-kill behavior. No blocking work on the UI event thread.
4. Spawn the sidecar with internal `NOOR_MANAGED_HOST_MODE=true|false` on every desktop launch. Server bind precedence becomes `NOOR_ADDR` > this explicit managed bool > standalone `--host` > SQLite preference. Thus desktop false cannot accidentally inherit stale SQLite true. The shell does not need the old conditional `--host` flag.
5. Server startup mirrors the managed preference into SQLite before becoming ready and records actual listener facts in remote runtime state. A DB write failure fails readiness; it is not hidden. Standalone does not write/read the desktop JSON config.
6. Refresh the loopback setup PIN, require `/api/server/remote` to confirm the requested effective state, update the tray, and resolve the operation. Reopen/reload the settings anchor once if needed; a renderer reload never owns or repeats the transition.
7. Failed enable restores the prior disabled config and attempts one local-only recovery. Failed disable after saving the disabled state does not re-enable LAN: leave the owned server stopped if local-only recovery fails. Bound overall work by the 15-second deadline, using the remaining budget for recovery.

The same loopback hostname/port that Tauri uses for readiness remains its webview origin. Add/adjust capabilities for that resolved loopback origin only; never grant native invocation to `noorwave.local`, arbitrary LAN addresses, or a wildcard web origin. If runtime capabilities cannot express the actual overridden port with the pinned Tauri version, retain default-port IPC and make nondefault-port controls explicitly unavailable pending a scoped implementation decision; do not silently expand the permission boundary.

### WebSocket session lifecycle

Authentication attaches a `SharedPin { generation }` or `PairedDevice { id, generation }` principal to the request. New-token HTTP/query validation uses the same credential store. On WS upgrade, register the principal against a cancellation/watch signal and recheck it before sending the initial event. Individual revoke cancels that ID; PIN reset advances the shared generation and cancels every authenticated session. Use close code 4001 with reason `Authentication required` for auth invalidation and 1001 for server shutdown. The browser handles 4001 as terminal, clears the relevant session, and shows pairing/PIN entry.

A failed upgrade often exposes no HTTP status to browser JavaScript. After repeated failed connection attempts, a bounded protected status request distinguishes 401/403 from reachability failure; use one reconnect timer with 1, 2, 4, 8, then at most 15-second delays, reset on success, and cancel it while logged out. Network errors retain credentials. Avoid repeating auth probes on every spectrum/playback event.

## Data Models

### Remote device: new `remote_devices` SQLite table

| Field | Type | Constraints |
|---|---|---|
| `id` | TEXT | UUID v4 primary key, immutable |
| `name` | TEXT | Not null; application validates trimmed 1–64 scalar values, no control characters |
| `token_hash` | BLOB | Not null, unique, exactly 32 bytes; SHA-256 of the full versioned token string |
| `paired_at` | TEXT | Not null, UTC RFC3339 |
| `last_seen_at` | TEXT nullable | UTC RFC3339; writes throttled to once/minute/device |

Unique index on `token_hash`; primary-key lookup for rename/revoke. Limit to 32 active rows in the serialized creation transaction. Hard delete for revoke. No user-agent string, MAC address, IP history, plaintext secret, or service password is stored. Raw device tokens exist only in the exchange response and phone storage. Authorization can cache the small digest set in a dedicated runtime structure initialized from SQLite, avoiding a DB read per audio/event request. Mutations update persistence and runtime state as one serialized operation; a failed DB mutation leaves the old runtime state active.

### Existing server configuration additions

| Key | Type | Constraints |
|---|---|---|
| `server_token` | Existing TEXT | Existing six-digit PIN retained; never replaced with a device token |
| `server.host_mode` | Existing TEXT boolean | Standalone preference; mirror of authoritative desktop JSON in managed mode |
| `remote.server_id` | TEXT | Persistent UUID v4, generated once, not a credential |
| `remote.hostname` | TEXT | Lowercase effective DNS hostname ending `.local.`, initially `noorwave.local.`; DNS label constraints apply |

Do not add separate port or IP preferences. Actual port remains the existing environment/default resolution. Selected IP is a current UI choice, not durable configuration that would outlive DHCP. Persisting a collision name favors stable bookmarks; no automatic attempt to reclaim a shorter name later.

### Desktop configuration and transient state

| Field | Type | Constraints |
|---|---|---|
| JSON `host_mode` | Existing boolean | Desktop source of truth; default false |
| JSON `minimize_to_tray` | Existing boolean | Preserve through all host writes |
| Runtime transition phase | Enum | Idle/restarting/recovering/failed, guarded by one mutex/operation lock |
| Runtime actual bind facts | Socket address + control enum | Populated from successful bind, not inferred from preferences |
| Runtime interface snapshot | Array of `RemoteAddress` plus interface metadata | Updated from OS interface enumeration; no durable IP history |
| Runtime discovery handle | Optional responder + monitor task | Present only while LAN hosting is effective; owned by server lifetime |
| OS autostart registration | Per-user installed-app registration | Authoritative start-at-sign-in state; command includes internal autostart marker; absent in portable mode |
| Process launch mode | `normal` or `autostart` | Derived from the validated internal launch marker; autostart keeps the main window hidden |
| Single-instance owner | One desktop process per user session | Established before sidecar spawn; secondary normal launch activates the owner |

### Pairing ticket: memory-only

| Field | Type | Constraints |
|---|---|---|
| `id` | UUID v4 | Identifies current ticket for cancel/status |
| `secret_hash` | 32-byte digest | SHA-256 of random 32-byte URL-safe secret; no raw secret retained |
| `created_at` / `expires_at` | UTC instants | Display metadata; lifetime 120 seconds |
| `deadline` | Monotonic instant | Authoritative expiration check |
| `origin` / `address_id` | Validated origin and snapshot identity | Invalidate if origin ceases to be offered |
| `state` | Pending/redeemed/expired | At most one pending entry; consumed metadata can remain until next creation/restart |

Ticket consumption and device/reset mutations share a dedicated async operation lock, not a long-held global `AppState` write lock. Validate name/body before the critical section. Check readiness/expiry again inside it, commit insertion, publish the credential digest, and mark consumed before releasing. Generate no usable response on DB failure. One-time use means no retry cache holding a plaintext device secret.

### Browser session and limiter

| Entity/field | Type | Constraints |
|---|---|---|
| `noor_api_token` | Existing localStorage string | Legacy PIN or versioned paired token; maintain existing callers |
| `noor_remote_session_v1` | JSON `{ server_id, device_id, name }` | Metadata only; scoped to the browser origin |
| Memory session fallback | Token + metadata | Used only when storage access throws; lost on page close |
| Rate bucket | Source IP + rolling failure timestamps | Eight/60s; capped 4,096 source entries, ten-minute idle eviction |
| Global invalid-attempt window | Rolling timestamps | 60/60s; shared across header/query/WS and ticket failure paths |
| Auth principal/session registry | ID or PIN generation + cancel signal | In-memory only; removed on disconnect/shutdown |

Validate valid existing API credentials before rejecting due to the invalid-attempt budget, so unrelated attackers cannot lock out established devices. Redemption attempts are bounded by the same failure accounting; management creation additionally rejects concurrent transitions and device capacity. Use fixed-time digest equality where comparing secrets rather than ordinary string comparison.

### Migration and upgrade strategy

Append migration 063 if still available at implementation time; otherwise use the next number without renumbering applied migrations. Add only `remote_devices` and its indexes with `IF NOT EXISTS` so re-entry is safe if DDL committed before `_migrations` was updated. Initialize new configuration keys using insert-if-absent in a transaction; an existing invalid new-format value reports a repairable configuration error rather than overwriting unrelated data. Do not rebuild a library table or change the migration runner for this feature.

Existing PIN values, localStorage sessions, `/remote` bookmarks, service secrets, audio preferences, and host preference survive the upgrade. Device credentials survive sidecar restarts; tickets do not. Existing PIN sessions do not magically become named devices; they remain legacy sessions until the user pairs through QR. Unknown `nrp_` tokens on an older rolled-back server fail normally and the browser offers manual PIN entry.

## Discovery and diagnostic behavior

Use an embedded Rust responder rather than depending on Bonjour/Avahi installation. `mdns-sd` is the recommended candidate; its documented monitor API reports announcement errors and conflict-driven `NameChange` events. Verify and pin a supported version in Cargo.lock during implementation, including Windows coexistence with an already-running mDNS responder. Prefer OS-backed interface metadata and enumeration; do not infer the LAN address by sending a UDP packet to an external internet host.

Normal managed hosting retains the existing `0.0.0.0:<port>` listener and advertises only compatible IPv4 addresses. Do not claim an IPv4-only listener supports IPv6 simply because the host has IPv6 addresses. Restrict responder interfaces/address records to selected eligible candidates, and handle interface-local collision events by converging on one persisted hostname across advertisements. Service instance name can be `NOORwave <first-eight-server-id-characters>`; TXT contains only `path=/remote`, `protocol=1`, and `id=<server-id>`. The friendly URL uses the confirmed hostname with no trailing dot; keep the explicit port.

The status model differentiates saved setting, actual listener, packaged assets, candidate addresses, discovery advertisement, and unknown external reachability. A bound `0.0.0.0` listener is exposure on all compatible interfaces, not proof that Windows restricts traffic to a private profile. The enable panel says to use a trusted Wi-Fi network. Troubleshooting explains the Windows private-network firewall prompt, same Wi-Fi, guest/client isolation, and VPN checks. It never disables the firewall, creates a firewall rule, requests elevation, scans the subnet, or claims remote reachability from a loopback self-check. A future separately authorized explicit firewall repair action could be specified later.

The phone uses the camera application's QR support, so no embedded camera permission or QR scanner is needed. The QR display defaults to the confirmed `.local` URL, giving stable bookmarks when mDNS works. **Use IP address instead** selects a direct candidate and creates a new ticket. The former QR is invalidated. If multiple NICs exist, the desktop presents labelled address choices under troubleshooting. No secret is transferred between origins; pairing an IP-origin page and a hostname-origin page creates independent sessions.

## Implementation sequence and file map

Implementation starts only after this document is approved. Work through these checkpoints in order, keeping each reviewable. No checkpoint is permission to ship an incomplete or permanent-PIN QR flow.

1. **Runtime facts and contract tests.** Introduce remote state/types and the actual bind snapshot. Refactor router assembly just enough to exercise the full public/protected stack in tests. Preserve static fallback and loopback gates. Add failing behavior tests for the new authorization matrix, actual-vs-configured status, and pairing lifecycle.
2. **Credential persistence and pairing.** Add the migration and a small dedicated database module. Implement versioned random credentials, ticket lifecycle, limits, new management/public routes, paired-principal restrictions, shared PIN reset transaction, and socket revocation. Make race, replay, reset, and rollback tests pass before integrating UI.
3. **Controlled desktop lifecycle.** Implement error-reporting config save, managed true/false launch input, single native command/transition lock, state events, readiness verification, and failure recovery. Reuse it from the tray. Add installed-build start-at-sign-in using the official Tauri facility, an explicit hidden-launch marker, and single-instance arbitration before sidecar spawn. Preserve default local-only behavior and validate port/capability constraints.
4. **Network discovery and address facts.** Add bundled responder and OS interface enumeration, real-port advertisements, collision persistence, interface refresh, and bounded shutdown. Supply deterministic fakes for tests; run real two-responder/manual device checks on Windows before calling discovery complete.
5. **Phone and desktop connection UI.** Add a dedicated Phone Remote settings component, local QR encoder, API module/types, and bootstrap state machine. Handle ticket fragments before auth, same-origin session persistence, fallback PIN, scoped management availability, terminal auth errors, and one socket reconnect timer. Move the old PIN card into the manual section without losing existing settings search aliases.
6. **Integration, release checks, and documentation.** Build bundled assets, validate installed/portable resource inclusion, exercise camera QR on phones, record timing and network-matrix evidence, and update README/release checklist with the new flow and the explicit restart/HTTP limits. Final review traces every FR/AC to tested behavior and reports any unmet SHOULD separately.

| Location | Intended change |
|---|---|
| `noor-server/src/server/remote/{mod,auth,pairing,discovery,network}.rs` (new bounded modules) | Runtime state, principal/auth service, handlers, mDNS lifecycle, address ranking; split only where ownership is clearer |
| `noor-server/src/db/remote.rs` (new), `db/mod.rs`, `db/schema.rs` | Device persistence, initialization, additive migration, migration/revocation tests |
| `noor-server/src/main.rs` | Managed bind precedence, actual startup configuration, initialize remote state, remove credential logging |
| `noor-server/src/server/mod.rs` | Router wiring, full-stack tests, auth principal attachment, public pairing routing, origin/header handling |
| `noor-server/src/server/routes.rs` | Existing token/reset/info/host handlers delegate to shared remote services; administration restriction |
| `noor-server/src/server/ws.rs` | Principal-aware upgrade, cancellation, explicit auth/shutdown close codes |
| `noor-server/src/server/routes/tests.rs`, relevant AppState test constructors | Update runtime fixtures and legacy contract expectations; retain current tests |
| `noor-server/Cargo.toml`, `Cargo.lock` | Pinned responder/interface support and constant-time helper if needed; reuse current rand/base64/sha2/uuid |
| `noor-app/src/{commands,config,sidecar,tray,main,server_url}.rs` | One authoritative host operation, reliable save, managed launch, readiness, tray sync, start-at-sign-in state, hidden launch, single-instance behavior, command registration |
| `noor-app/Cargo.toml`, `noor-app/capabilities/default.json` or runtime capability setup | Official autostart/single-instance support and exact-loopback native permission for the supported app origin; no LAN permission expansion |
| `frontend/src/lib/components/settings/PhoneRemotePanel.svelte` (new) | Status, enable/restart, pairing QR, fallback, list/rename/revoke, diagnostics |
| `frontend/src/lib/components/settings/settingsSearch.ts`, `routes/settings/+page.svelte` | Searchable section and component integration; legacy PIN placement/wording |
| `frontend/src/lib/api/remote.ts`, `lib/remote/connection.ts` (new) | Typed new contracts and testable bootstrap/session logic |
| `frontend/src/lib/api/client.ts`, `lib/api/ws.ts` | Storage fallback, principal metadata handling, reconnect cancellation, auth invalidation |
| `frontend/src/routes/+layout.svelte`, `routes/remote/+layout.svelte` | Pairing-first auth gate and connection state/error presentation |
| `frontend/package.json`, frontend lockfile | Local QR encoding dependency; prefer a small maintained encoder with typed API and no runtime network calls |
| New Rust/Vitest tests, `frontend/scripts/smoke.mjs` or focused smoke companion | Meaningful behavior/security/QR tests and browser integration checks |
| `README.md`, `docs/release-checklist.md` | User setup, trusted-LAN scope, restart effect, diagnostics and verification matrix |

The large playback/catalog route bodies and audio engine are outside the modification map. Do not implement live listener replacement, broad settings refactors, an identity-provider framework, or a generalized network scanner as prerequisites.

## Test plan and release evidence

Use parameterized Rust tests with injected clock/randomness/interface data and isolated temporary SQLite databases for AC-2–AC-15, AC-18–AC-23, AC-27, AC-30–AC-34 and the corresponding edge cases. Auth/authorization tests must exercise the assembled router with realistic `ConnectInfo`, not only handler functions. Include a real socket test for live WS closure and a concurrency test where reset races redemption/upgrade. Check ordinary authenticated media URLs as well as HTTP and WS because the existing middleware accepts query tokens beyond `/ws`.

Use Vitest behavioral tests for fragment parsing/removal order, bootstrap exclusivity, identity mismatch, storage failures, QR address selection, request status handling, timer cleanup/backoff, startup-setting presentation, and matching UI state transitions. Existing source-contract tests remain useful regressions but do not prove pairing or authorization. Decode a generated QR in a test using an independent decoder or the browser integration fixture; asserting that an SVG exists is not sufficient. Use Playwright at desktop and narrow phone widths for AC-1, AC-16–AC-17, AC-24–AC-26, AC-28–AC-29 and AC-35, including keyboard/error focus. Add installed/portable native integration fixtures for AC-38–AC-42, including concurrent autostart/manual launch and updater replacement. Network mocks supplement but do not replace actual responder/phone testing.

Required developer checks after implementation: `cargo fmt --all --check`; focused new Cargo tests followed by `cargo test -p noor-server -p noor-app`; `pnpm --dir frontend test`; `pnpm --dir frontend check`; `pnpm --dir frontend lint`; `pnpm --dir frontend build`; appropriate existing browser smoke checks. Establish the baseline before edits and separate unrelated existing failures. Avoid broad unrelated formatting edits. Build installed and portable artifacts through the repository's existing packaging scripts only as needed to verify this feature; do not publish or install over the user's active app as part of verification.

Record the following manual release matrix with date, OS/browser versions, actual port, outcome, and timing: Windows 11 installed/portable with an iPhone/Safari and Android/Chrome on ordinary same-subnet Wi-Fi; alternate port; two servers competing for the hostname; Wi-Fi plus Ethernet/VPN; multicast unavailable but direct IP working; denied firewall access; guest isolation; server restart; sleep/resume; DHCP address change; one-device revoke while another continues. At least one physical-device scan on each phone family is required before claiming cross-platform onboarding verified. Test blank and existing-profile browsers. A local build/test pass alone is not evidence of phone reachability.

Every EC has its expected outcome linked to ACs above; failure-injection tests exercise each dependency rather than only its happy path. If physical phones or the network fixture are unavailable, implementation can be reviewable with the exact unverified cases recorded, but the corresponding release checks remain outstanding and the 20-second product target remains unproven.

## Rollback and operational recovery

Before migration verification use a temporary database copied from a safe fixture or a SQLite-consistent backup, never a raw copy of a live WAL database. Additive remote tables/keys can remain during a code rollback: the older server ignores them and still accepts the preserved shared PIN. Do not delete migration rows, credentials, or library tables to make old code start. Downgrade compatibility of the actual release must be tested against a fixture because this migration runner counts applied rows. After rollback, users of paired tokens re-enter the shared PIN; re-upgrading restores non-revoked device rows.

The recovery action for discovery failure is direct-IP QR/manual PIN, not reverting the whole feature or automatically changing firewall/network configuration. The recovery action for a failed LAN enable is restoring local-only operation as specified. For a failed disable, keep LAN stopped and give a local restart action. Config replacement preserves the previous valid file on failure. Revocation and PIN reset are intentionally irreversible for those credentials; users can pair again with a fresh QR.

## Out of Scope

- OS-1: Internet access, port forwarding, UPnP, tunnels, cloud relay, remote accounts, or NAT traversal. The authorized product is a same-LAN remote.
- OS-2: HTTPS certificate provisioning, private CA installation, encrypted transport, and claims of adversarial-network protection. These require a separate trust/bootstrap design.
- OS-3: Browser-side arbitrary LAN/mDNS enumeration, subnet scanning, native mobile wrappers, or a new companion app. QR and hostname entry work in the existing browser model.
- OS-4: Permanent-PIN QR links, shared-PIN secrets in DNS records, custom `noorwave:` handlers, hosts-file edits, OS computer renaming, and binding port 80/443. They add avoidable credential or platform friction.
- OS-5: Automatic firewall rule installation/removal, administrator elevation, Bonjour/Avahi installation, and router reconfiguration. This release diagnoses and guides only.
- OS-6: Live listener rebinding, audio-session/queue preservation across restart, or playback architecture changes. Restart effects are disclosed rather than hidden.
- OS-7: Dual-stack listener redesign and automatic IPv6-only phone onboarding. Existing explicit IPv6 server binds are not expanded into a new desktop support promise.
- OS-8: A comprehensive user/role/permission system, guest accounts, device attestation, refresh tokens, or cryptographic device identity. Paired devices are trusted operators with bounded server-administration exclusions.
- OS-9: Automatic retirement of legacy PIN access, migration of existing PIN sessions into named devices, or independent revocation of people who still know the shared PIN. Reset the PIN to remove that compatibility authority.
- OS-10: Cross-origin credential synchronization between IP and `.local`, automatic redirect of device secrets, or roaming discovery across subnets. Re-pair at a new origin when necessary.
- OS-11: Offline playback, guaranteed installable PWA behavior, background phone execution, or secure-context browser capability upgrades. The remote remains an ordinary served HTTP page.
- OS-12: Unrelated Settings, artwork, catalog, audio, installer, or app branding redesigns. Component extraction is limited to this connection flow.
- OS-13: A Windows service, pre-login machine boot, service-account database/audio access, or a separately supervised headless server. Start-at-sign-in runs the ordinary per-user NOORwave desktop owner silently.

## Primary technical references

`.local` is link-local mDNS and includes conflict-resolution rules; DNS-SD supplies service instance, port, and TXT metadata. These are the basis for the hostname/service design, not a guarantee that every router or browser can resolve it. [RFC 6762](https://www.rfc-editor.org/rfc/rfc6762), [RFC 6763](https://www.rfc-editor.org/rfc/rfc6763).

The responder candidate provides a daemon lifecycle, interface controls, monitor events, and conflict-driven name changes; use its documented events rather than inferring successful advertisement from a queued registration call. [mdns-sd documentation](https://docs.rs/mdns-sd/latest/mdns_sd/), [daemon events](https://docs.rs/mdns-sd/latest/mdns_sd/enum.DaemonEvent.html).

Browser secure-context restrictions apply to ordinary LAN HTTP origins; feature detection and fallback are necessary. [MDN secure contexts](https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Secure_Contexts).

The official Tauri autostart plugin supports per-user desktop autostart registration and explicit launch arguments, which supplies the hidden-launch marker without a custom registry implementation. Its enable/disable/query failures remain observable and capability-scoped. [Tauri Autostart](https://v2.tauri.app/plugin/autostart/).
