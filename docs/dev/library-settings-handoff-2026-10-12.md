# Library and Settings handoff

The Library landing now mounts its own queries while background browse queries
load. Cached shelves remain visible on revisits. Repeated exclusive failures
reuse one toast, and Settings renders one failure banner with working Retry and
Disable controls. Device selection preserves exact IDs, maps unique legacy
names, and shows unavailable or ambiguous saved devices explicitly.

Verified: seven focused frontend tests and both production route smoke scripts
passed on 2026-10-12 using Edge and isolated API/WebSocket fixtures. The earlier
full frontend run passed 1,320 tests, lint, Svelte check and production build.
The smoke covers cold and persisted loading, repeat navigation, failed browse,
empty landing, repeated failure feedback, recovery, disabling and device IDs.

This commit covers frontend behavior. Native endpoint selection and playback
recovery are in the continuing backend change. Installed desktop audio, audible
track transitions and device unplug/replug still need validation against that
change. No installed application or saved user settings were modified.
