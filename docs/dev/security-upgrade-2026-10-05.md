# Security dependency fixes — 2026-10-05

Upgrade the desktop framework and compatible security patches together. The
Windows app's Ctrl+wheel zoom remains functional under the patched ACL rules.

## Applied fixes

| Dependency | Previous | Updated | Reason |
| --- | --- | --- | --- |
| Tauri | 2.10.3 | 2.12.1 | Fix origin confusion and remote custom-command ACL bypass; isolate queued channel responses by WebView. |
| Rustls | 0.23.40 | 0.23.45 | Reject TLS 1.3 handshake messages crossing encryption boundaries. |
| H2 | 0.4.13 | 0.4.19 | Bound the queue of empty HTTP/2 DATA frames. |
| Crossbeam Epoch | 0.9.18 | 0.9.21 | Fix invalid pointer dereference during pointer formatting. |
| Event Listener | 5.4.1 | 5.4.2 | Prevent non-Send tags crossing threads through stack listeners. |

Tauri/runtime/runtime-wry are exact-pinned at 2.12.1; build and code generation
use 2.7.1 and utils uses 2.10.1. Wry is 0.57.0. The release and Windows cache
warmer both use CLI 2.12.1 with quoted Cargo argument separators. Linux jobs
explicitly install the new D-Bus development dependency. All lock changes used
targeted package updates, rather than a broad lockfile regeneration.

The newer Tauri utility dependency tree also removes Rand 0.7.3, old HTML
selector dependencies and multiple unmaintained Unicode crates. No direct
Rand, audio engine or TypeScript major upgrade is included in this change.

## Why zoom and desktop commands needed attention

NOORwave's desktop UI runs at `http://127.0.0.1:<configured port>`, which Tauri
correctly treats as remote content. Patched Tauri rejects remote custom
commands without explicit command permissions. The old application had no
app command manifest; its existing zoom capability covered only port 17600.

The app now generates permissions for all eleven registered custom commands
and installs the main-window capability before creating the WebView. The
capability's single allowed URL uses the actual `server_url::base()` port, so
`NOOR_PORT` and `NOOR_ADDR` port overrides retain desktop functionality.
The template lives outside `capabilities/`, preventing the inactive default
port from keeping a second grant. `local: false` confines permissions to the
explicit loopback URL; other hosts, ports and windows receive no grant.

Zoom continues using the official `getCurrentWebview().setZoom()` API. There
is no framework downgrade, permission wildcard or replacement of normal
scroll-wheel behavior. The historical custom zoom implementation is not
reintroduced.

## Validation

- Windows `cargo test --workspace --locked`: **1,851 passed, six ignored**.
- `cargo fmt --all -- --check` and the repository's workspace/all-target Clippy
  command pass. Existing server warnings remain.
- Permission tests exercise Tauri's real command resolution: default and
  alternate loopback ports allow app commands and zoom; inactive/wrong ports,
  different hosts, HTTPS, spoofed protocol names, bundled origins and other
  windows are rejected.
- CLI 2.12.1 builds the Windows desktop app with a locked Cargo graph.
- Native Windows WebView2 smoke at port 17611 passes desktop command calls,
  Ctrl+wheel in/out, Ctrl+plus/minus/reset, persistence after reload, ordinary
  wheel scrolling and both 50–200% zoom bounds. Assertions check actual
  `devicePixelRatio` changes in addition to the persisted preference.

The native test uses copied executables, a separate single-instance identifier,
isolated database/profile and unused loopback ports. It does not test or modify
the user's installed application. The repeatable input test is
`scripts/tauri-zoom-smoke.mjs`; install its locked tooling with
`npm ci --prefix scripts --ignore-scripts`, then supply the isolated WebView2
debugging URL and app URL. Enable remote debugging only for the test process.
Wait for the managed sidecar to start before running the script.

Local verification covers a debug portable app, not a signed NSIS installation
or macOS/Linux desktop interaction. The PR's Linux tests/build checks must
pass before merge. No release tag or installer is published by this update.

## Remaining findings and decisions

An OSV batch scan of **762 locked registry package versions** reports findings
in **four package versions**, down from sixteen in the prior 798-version scan.
These counts include maintenance notices and duplicate advisory aliases; they
are not counts of exploitable vulnerabilities. Tauri's two GitHub advisories
were checked separately because advisory indexes can lag upstream disclosure.

| Remaining dependency | Path / impact | Decision |
| --- | --- | --- |
| GLib 0.18.5, RUSTSEC-2024-0429 | Linux GTK3 dependencies used by current Tauri/Wry; unsound `VariantStrIter` iterator implementation. | Upgrading GLib alone to the fixed >=0.20 family cannot satisfy GTK3's 0.18 dependency. Needs a reviewed upstream backport or coordinated framework/toolkit migration; do not claim resolved by the Windows upgrade. |
| Atty 0.2.14, RUSTSEC-2021-0145 / 2024-0375 | Bindgen 0.58.1 through Aubio's generated-bindings build path; an unaligned Windows read and lack of maintenance. Current Windows x64 uses prebuilt Aubio bindings; this is not an application runtime dependency there. | No patched Atty release. Replace/update the parent Bindgen chain in a separate Aubio build migration, including the macOS ARM build that enables it. |
| Ansi Term 0.12.1, RUSTSEC-2021-0139 | The same legacy Clap/Bindgen build chain. | Maintenance notice; address with the Aubio/Bindgen migration. |
| Proc Macro Error 1.0.4, RUSTSEC-2024-0370 | Linux GTK/GLib macro dependencies. | Maintenance notice; address with the GTK dependency migration/backport. |

Frontend Braces 3.0.3 still has the previously recorded high-severity
GHSA-vfj7-8cjw-p6xm finding through Stylelint/Micromatch. No patched release was
available in the audit. The earlier Vitest 5.0.3 upgrade did not remove it.

## Primary sources

- [Tauri origin-confusion advisory](https://github.com/tauri-apps/tauri/security/advisories/GHSA-7gmj-67g7-phm9)
- [Tauri remote-command ACL security fix](https://github.com/tauri-apps/tauri/releases/tag/tauri-v2.11.1)
- [Tauri channel-response advisory](https://github.com/tauri-apps/tauri/security/advisories/GHSA-w28w-mhc8-qvjv)
- [Tauri 2.12.1 coordinated release](https://github.com/tauri-apps/tauri/releases/tag/tauri-v2.12.1)
- [Rustls RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285.html)
- [H2 RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258.html)
- [Crossbeam RUSTSEC-2026-0204](https://rustsec.org/advisories/RUSTSEC-2026-0204.html)
- [Event Listener RUSTSEC-2026-0221](https://rustsec.org/advisories/RUSTSEC-2026-0221.html)
- [GLib RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
- [Atty RUSTSEC-2021-0145](https://rustsec.org/advisories/RUSTSEC-2021-0145.html)
