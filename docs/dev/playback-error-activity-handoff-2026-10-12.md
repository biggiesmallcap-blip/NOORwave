# Playback error activity handoff

Generic runtime error handling already paused DB transport and cleared the
active runtime track, but left `audio_active` true. It now clears that flag
with the error metadata, so an unavailable-output failure does not leave public
telemetry reporting audible playback.

Verified through the real runtime listener: the regression reproduced paused
DB state with activity still true before the fix. It now observes the existing
public failure and state events, cleared activity, the saved error, and the
same queue rows and current queue item. All 50 transport tests passed, along
with native compilation, formatting and whitespace checks.

This commit covers activity telemetry. Continue the broader goal with deferred
fatal CPAL error delivery, generic-error runtime identity/order review, and
decoder recovery if PCM is poisoned. Audible desktop playback and physical
hotplug checks remain pending. No routes or event payloads changed.
