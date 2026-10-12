# Isolated Windows symptom validation

Task: finish the current background pending-adoption ownership boundary, review the symptom-focused drafts, and exercise them in an isolated Windows desktop build. Broader audit work was deferred for this test cycle.

Reviewed drafts: Library loading/warnings #353 at `358aba07ae406a40df972f5892aef33b0c625431`, output foundation #352 at `f2411ed4676b77c4722964faa866d41d7771ef06`, and completed pending-owner fix #355 at `88ec217cfc76df8542fee37455133b63fdb75356`. All three heads had successful PR-check runs. All remain drafts.

Build source: combined branch `bsc/library-wasapi-recovery`, commit `78e5d26f910741f3ad0b0835a628f2cc446df78a`. It includes the decoder/resampler category and current recovery fixes alongside the symptom drafts. Server/app version remains 1.0.5. No release, installer, updater or installed-app replacement was performed.

Focused review confirmed that unrelated browse loading no longer gates mounting the Library landing queries, cached landing content survives navigation and refresh, Settings has one failure panel, identical exclusive-failure messages refresh one toast, stable device IDs are preserved, and missing/ambiguous legacy devices are displayed explicitly. The output path attempts PCM representations while preserving source rate/channel intent, returns an actionable failure when candidates are exhausted, and reports exclusive engagement after successful stream creation. No new blocking finding was identified in these reviewed symptom paths; this is not a complete engine re-audit.

Isolation: a distinct compiled app identifier, `com.noorwave.audit.symptoms20261012`, separate executable/data/frontend/WebView directories and loopback port 17679. CDP used loopback port 18779 during automation. The existing smoke scripts were adapted locally to attach to the real Tauri WebView, verify its origin and Portable install mode, then exercise their API/WebSocket fixtures. These were desktop route tests, not a separate browser rendering or changes to a user library.

Initial unmodified shell launch failed because the installed app already owned MediaPlayPause. The setup hook panicked and left an isolated sidecar running. That sidecar was stopped after checking its exact executable path. To test alongside the installed app, media-key registration was omitted only in the test shell build. The source was restored afterward; no permanent shell behavior change remains. The build manifest and test-only patch record this exception. Media-key behavior is not validated by this build. The startup concern is recorded in FOLLOWUPS.md with a concrete next check.

Acceptance checks:

- Done: cold Library landing renders while the Albums browse request remains unresolved.
- Done: persisted landing paints with browse and refresh requests blocked; repeat navigation preserves shelves; browse failure does not hide them; empty landing settles correctly.
- Done: repeated exclusive failures produce one banner and one toast; Retry reaches its API path; engagement and Disable clear the banner; device ID/legacy/unavailable selection scenarios pass.
- Done: isolated app uses its own database, WebView profile and production frontend. The test scripts confirmed actual Tauri IPC and the isolated origin.
- Done, limited hardware scope: silent exclusive output on the exact Realtek Digital Output endpoint opened/released at 44.1 and 96 kHz, including idle release and reopening. Shared output accepted an 88.2 kHz request. A read-only WASAPI session check confirmed zero active sessions before the probe.

Environment: Windows 10 Pro, version 10.0.19045/build 19045; Realtek High Definition Audio driver 6.0.8945.1, Realtek Semiconductor Corp. Endpoint: `{0.0.0.00000000}.{a0a5d541-c7d9-4346-b8f3-42a81323b497}`.

Verification:

- Actual Windows Tauri WebView: all 10 Library/Settings scenarios passed.
- Frontend: seven focused behavior tests, `pnpm check` with zero diagnostics, `pnpm lint`, and production build passed.
- `cargo build -p noor-server -p noor-app --locked`: passed with the distinct test identifier. Test shell rebuilt after the media-key exception.
- WASAPI software regressions: 41 passed, one hardware probe ignored in the normal run.
- Explicit silent hardware probe: one passed, zero failed; four exclusive acquisition/release cycles plus shared output.
- Full backend on combined source: 2,189 passed, zero failed, 11 ignored, 313.70 seconds.
- The pending-owner boundary has a failing-then-passing real-listener regression, admission-wait loop coverage, separate code commit `ed460ec1`, handoff and passing CI.

Artifacts are local under `.scratch/symptom-windows-20261012/`: app/server binaries, production `www`, isolated launcher, build manifest and test-only shell patch. Generated database, credentials, profile and logs are machine-local and must not be published. App SHA256: `9EF30A76846E353AA8CCFA90CA96872C760B011C5D13DD108FF81F65D9666237`. Server SHA256: `5418581FD084EA897269D8D5BE4DFA581E7D4E94B78260BCD787179838145071`.

Not verified: the user's original account/library and failing track, audible playback/quality, 48/192 kHz exclusive output, the full format/channel/device failure matrix, installed or release-profile packaging, media keys, bit-perfect capture, latency/CPU/memory/callback targets, stress cycles and endurance. Passing a silent probe does not prove all Realtek formats work or that unsupported source rates can be bit-perfect. No performance improvement is claimed from this run.

Incomplete: no missing wiring in the tested symptom paths was identified. The overall audio audit remains incomplete, including foreground pending promotion and skip/stop/restore mutation boundaries. These are separate from this completed desktop validation slice.

Next: use the isolated launcher for the original account/track reproduction before deciding whether to merge or release the symptom fixes. Broader audit and hardware/performance validation remain subsequent work. Follow-up added: graceful startup/cleanup when media-key registration fails.
