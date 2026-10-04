# Settings and onboarding rework

Implemented on 2026-10-04 following the approved Settings layout and integrated onboarding plan.

The [follow-up audit](settings-followup-audit.md) records the restored glass styling, clearer setup links, desktop-control visibility and complete original-to-current Settings inventory.

## Changes in implementation order

1. **Settings structure:** Appearance, Playback, Library, Services, Remote and App share a compact row/group system. Important controls stay visible; tuning, diagnostics and maintenance use disclosures. Search and deep links use one manifest, including compatibility with older category names and anchors. Keyboard search opens and focuses nested controls.
2. **Controls and copy:** consistent control dimensions, alignment and labels; quiet surfaces; conditional download options retain their preferences. Removed unwired DSP placeholder inputs. Exclusive-output failure remains visible outside advanced settings, with recovery actions. Native startup moved into App while Phone remote retains pairing, recovery and device management.
3. **Service connections:** Settings and onboarding share Last.fm/ListenBrainz forms and connection logic. Status distinguishes tags-only credentials, saved key/secret and confirmed account approval. Privacy and duplicate-upload information appears before connecting. Credential removal retains a separate confirmation; account-only disconnect retains tag credentials. Provider errors, retry and manual history upload remain available.
4. **Appearance onboarding:** six steps end with an optional Look step and the approved completion notice. The onboarding frame retains Futuro/dark/Standing Wave. Appearance previews use a scoped renderer and theme; changes remain drafts until successful completion. Back retains choices, Skip discards them, and failed completion can recover the draft after reload.
5. **Discovery guidance:** new installations enroll before connecting TIDAL. Only a successful sync with a nonempty user library sets readiness. Guidance waits for completed onboarding and app entry. Eligibility and display records are stored in the database per TIDAL account, independent of rotating API tokens. A short reservation prevents simultaneous display in multiple tabs. Existing installations and users who already started training stay quiet. Set up discovery navigates to Library → Discovery; it never starts training.
6. **Verification and documentation:** route aliases and README paths updated; tests cover saved appearance/defaults, draft isolation and rollback, connection approval/failure states, stale response protection, setting-save rollback, guidance eligibility/reservations and route registration.

## Defaults and existing preferences

Fresh or invalid appearance preferences fall back to Futuro, dark surfaces and Standing Wave. Existing valid palette, theme and wallpaper choices remain authoritative. All palettes use the same selectors and preview behavior.

Appearance recovery stores only appearance choices in session storage. Provider secrets are transient form values; this rework adds no browser persistence for credentials.

## Discovery and radio behavior

Radio index scheduling remains unchanged: startup delay, hourly catch-up checks, six-hour rebuild interval and existing busy gates. The UI reports only available index evidence (pair count and build time); it does not invent automatic-build progress or failure states.

LibrarySynced remains a general notification. It triggers reconciliation with durable guidance eligibility rather than proving that a first sync succeeded. Failed, cancelled or empty syncs do not create readiness. Later syncs do not reset shown guidance.

## Validation

- Frontend: 988 tests passed across 160 files after rebasing onto current master; run with TZ=UTC for the existing date-format fixtures.
- Svelte/TypeScript: zero errors and zero warnings.
- CSS and inline typography lint passed.
- Production frontend build passed using locally installed, locked dependencies.
- Server: discovery guidance (3 tests), onboarding compatibility/reload (3 tests) and API route registration (1 test) passed. The server test build retains one existing unused-field warning.
- Browser verification uses controlled API fixtures and the built frontend. Screenshots and logs are stored with the original design preview.
- All six categories passed layout checks at 360, 640, 960 and 1440px without page or Settings overflow. Keyboard search opened nested settings; Back restored the prior destination. Appearance drafts survived Back and failed completion/reload, then applied on retry. Phone pairing generated its QR/code. Discovery guidance opened the correct setting without training and did not repeat after reload. Dark and light Clay screenshots were reviewed.

Live provider authorization, physical audio-device changes, OS startup integration and pairing with a physical phone still require a manual desktop smoke test. Automated checks exercise their existing shared contracts and the revised UI; they do not substitute for those external environments.

## TIDAL AI music preference

Services → TIDAL → More content settings now includes **Hide AI-generated tracks**, disabled by default. It uses TIDAL’s explicit boolean AI label across browsing, recommendations and generated queues, and stores the preference and observed labels in the server database. Saved library items and favorites remain available; missing metadata stays visible. Raw upstream pages, library sync and the current queue are preserved. Failed saves roll back, and successful saves refresh music caches. Implementation details and validation are recorded in `settings-followup-audit.md`.
