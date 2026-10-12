# WASAPI event-wait pacing handoff

An immediate `EventTimeout` reproduced 552,473 wait calls in 35 ms. wasapi 0.25
maps failed Win32 waits to that same error, so the renderer could consume a CPU
core until the existing two-second no-event deadline.

The real device-wait loop now sleeps only the unused portion of a timed-out
slice, bounded by the remaining deadline. Signaled events and explicit errors
return immediately. Cancellation checks still take priority before and after
the device wait, then run again after pacing. The maximum requested slice is
10 ms; Windows scheduling can overshoot that request.

Verified: the regression failed before the change and now passes with at most
eight calls over its 35 ms deadline. All 39 transport tests passed, including
no-event deadlines and cancellation priority. The silent Realtek 44.1/96 kHz
release/reopen/idle and shared 88.2 kHz smoke passed. Native check, formatting
and whitespace checks passed.

The upstream wait-result ambiguity remains: this paces failures without
guessing whether the cause is a timeout or invalid handle. Physical hotplug and
audible scheduling checks remain in the overall goal. Renderer panic recovery
is the next separate change. No dependency or settings changes were made.
