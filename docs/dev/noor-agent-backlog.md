# NOORwave Agent Backlog

This backlog records candidate repo-local agents beyond the first build set.

Note: the skill files live under `.agents/skills/`, and `.agents/` is ignored by Git in this repo. This backlog can be committed as planning documentation, but the local skills themselves must be provisioned separately unless the repo policy changes.

## Built First

- `noor-agent-coordinator`: route multi-surface tasks to relevant specialists, merge findings, resolve conflicts, and produce one decision-oriented overview.
- `noor-completion-gate`: enforce acceptance checks, TDD status, adversarial review, and final reporting.
- `noor-frontend-surface`: implement and validate visible SvelteKit app surfaces.
- `noor-rust-backend`: guide Axum, SQLite, service, DTO, auth, and backend test work.
- `noor-release-safety`: protect portable, installer, signing, updater, and release-note flows.
- `noor-dead-code-auditor`: validate deletion candidates before any removal.
- `noor-tauri-shell-safety`: protect sidecar lifecycle, tray, media keys, WebView startup, external links, and shell packaging behavior.
- `noor-api-contract-review`: review `/api/*`, `/ws`, serde DTOs, frontend API clients, remote UI, and route compatibility.
- `noor-secrets-artifact-auditor`: audit secrets, signing material, local DB files, machine-local paths, temp files, and generated output before staging.
- `noor-media-context-menu`: preserve shared menu builders and in-app media navigation for tracks, albums, artists, videos, and queue rows.
- `noor-dev-log-watcher`: watch bounded server, sidecar, frontend, browser, WebSocket, playback, resolver, and warning/debug logs around real product flows.

## Strong Next Candidates

- `noor-performance-profiler`: startup, playback latency, frontend responsiveness, query hotspots, and bundle-size warnings.
- `noor-docs-domain-memory`: stable domain terms, architectural decisions, repeated release gotchas, and long-lived invariants.
- `noor-test-triage`: flaky tests, coverage gaps, focused verification selection, and changed-surface test mapping.
- `noor-search-discovery-reviewer`: final-pass review for TIDAL resolver behavior, pending/resolved fields, image fallback, and discovery regressions.

## Already Partly Covered

- Playback and queue safety are covered by `noor-playback-change`; create a separate reviewer only if playback work becomes frequent enough to need final-gate specialization.
- Search and discovery changes are covered by `noor-search-discovery-change`.
- General release checks are covered by `noor-release-check`; `noor-release-safety` adds final readiness and artifact-risk framing.
- General frontend edits are covered by `noor-frontend-change`; `noor-frontend-surface` adds route-level product validation.

## Build Later Only If Repeated

- `noor-release-notes`: portable zip, installer, SmartScreen note, checksums, updater files, and user-facing release copy.
