# WASAPI renderer unwind handoff

The production render-thread wrapper now has an outer exit guard. If Rust
unwinding interrupts the renderer before release, the guard publishes a typed
`RendererPanicked` fault and release only after the renderer's resource locals
have dropped. It uses atomic operations, preserves an existing device fault,
and emits no ordinary release event that could trigger automatic reacquisition.

The existing runtime poll consumes the new fault once, pauses every deck, and
sends the existing internal OutputFault through the public listener. The
listener pauses DB/UI transport and clears exclusive engagement. Recovery
continues through explicit Resume. Public API and WebSocket shapes are unchanged.

Verified: the teardown-order regression first failed because release stayed
false after resources dropped. It now passes. All 41 transport tests passed;
the runtime graph test includes the panic fault, and the real public listener
test passes without automatic exclusive retry. Normal release and preservation
of an earlier fault have dedicated coverage. The silent Realtek native smoke,
native compilation, formatting and whitespace checks passed.

This verifies injected Rust unwinding through the same wrapper used by the
native renderer. Audible recovery and real hotplug remain in the overall goal.
Continue with fatal CPAL callback allocation and public error telemetry, then
decoder/resampler evaluation. If a panic poisons PCM state, recovery must also
rebuild that decoder instead of only reopening the output; validate that branch
in the next recovery audit. The preceding wait-pacing fix is `249ffdd8`.
