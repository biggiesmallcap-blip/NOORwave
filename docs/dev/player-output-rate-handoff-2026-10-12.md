# Source and output rate display

Task: correct the player's apparent 192 kHz output after the user configured a 96 kHz optical path, keeping the label compact.

Finding: the live test API confirmed a 192 kHz / 24-bit source and actual 96 kHz exclusive output. The player formatted only source resolution beside its exclusive indicator. Windows' shared default did not constrain source-following exclusive output before that setting was disabled and the app restarted.

Change: when source and runtime rates differ, the visible label uses the actual runtime rate, for example `96 kHz output · Excl`. Source resolution belongs in the tooltip. Matching rates keep their existing compact resolution label. A 44.1 kHz source sent to fixed 96 kHz output correctly shows 96 kHz output, with 44.1 kHz / 16-bit source in the tooltip. Desktop/sidebar, bottom player and remote use shared formatters; the metadata component now receives the source stream for its tooltip. No audio processing or settings contract changes.

Validation:

- Initial source/output regression failed before implementation.
- Final frontend suite: 1,329 tests in 212 files passed.
- `pnpm check`: zero errors and warnings. `pnpm lint` and `pnpm run build`: passed.
- A production browser check exercised the real Settings/player route through playback-state/runtime API fixtures and a `track_changed` WebSocket event. It verified the compact visible label and complete source tooltip. This is a rendered product-route check, not a native audible-output test.
- Updated the isolated portable frontend assets, retaining older hashed assets so an already open page can finish using its loaded chunks. No database, sign-in or backend changes during this UI update.

Commits: `cbd3d128`, source/output distinction; `2dfc7a56`, compact output label and source tooltip.

Acceptance: done for rate distinction, compact label, tooltip wiring and rendered route verification. User reported clean playback so far at confirmed 96 kHz with Stable buffering after the separate callback fix. That observation is preliminary; occasional PCM contention diagnostics remain.

Not verified: the native app after the user's next refresh, longer audible playback, external DAC capture and endurance. Incomplete: none in the label path. The larger audio hardening audit remains incomplete. Follow-ups added: none.

Next: publish this UI slice in its existing PR category and merge the authorized category series after current checks and conflict resolution. Continue investigating any further audible dropout separately from this label.
