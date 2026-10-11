# Playback output baseline handoff

Pinned CPAL 0.18.2 and wasapi 0.25.0 with targeted lock updates. Exact endpoint
selection replaces silent default fallback. Exclusive initialization preserves
useful HRESULTs, negotiates source-compatible PCM layouts, releases failed COM
clients before retry and balances thread COM lifetime. Output faults reconcile
deck, DB and UI pause state without skipping the queue; Resume rebuilds failed
outputs. Lifetime tokens reject retired output callbacks and decoder terminals.

PCM callbacks use nonblocking buffer access and a fixed Copy notification
mailbox. The actor consumes transition commands directly after queued controls,
so a concurrent seek cannot acquire a stale command behind it. Seeks and DJ
buffer replacements retire mailbox contents under the PCM cursor lock. Source
publication owns retired reclamation, and release waits check cancellation in
short slices. Integer conversion and F64/I24 shared output are covered.

Verified: full baseline server suite 2,107 passed/10 ignored; final runtime
suite 204 passed/3 ignored; missing-device and reused-sink public-path tests
passed separately; exact Realtek silent exclusive release/reopen/idle and shared
output smoke passed. Native check, actual server executable build, formatting and whitespace checks passed.
The audit contains the chronological evidence and distinguishes earlier runs.

Still remaining in the overall goal: audible patched-desktop transitions,
pause/resume, video and hotplug checks; renderer unwind and immediate event-wait
failure handling; fatal CPAL error callback allocation; public error telemetry
consistency; isolated Symphonia/rubato migration and performance evaluation.
Keep the goal active. The installed app and user database were not modified.
