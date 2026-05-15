// Svelte action that makes a tappable control fire reliably on iOS PWA.
//
// Plain `onclick` on a <button> is fragile in iOS standalone mode: the
// synthetic click event can be dropped or delayed when the control sits
// inside a stacking context (backdrop-filter, transform, etc.), when the
// main thread is busy from a previous render, or when scroll gestures
// share the same touch target. Symptom: user has to tap 2-3 times before
// navigation registers, or taps "feel laggy".
//
// This action handles the click via pointerup directly so the navigation
// runs at touch release without waiting for the synthetic click. A 400ms
// debounce coalesces the pointerup/click pair so the handler fires
// exactly once whichever path the browser delivers first. Mouse goes
// through onclick so keyboard activation still works.
//
// To distinguish a tap from a scroll gesture that happens to start on
// the control, we also track pointer movement and abort if the finger
// moves more than 8px before release.

const MOVE_THRESHOLD_PX = 8;
const REPEAT_DEBOUNCE_MS = 400;

export function tap(node: HTMLElement, handler: () => void) {
	let pressed = false;
	let startX = 0;
	let startY = 0;
	let lastFired = 0;
	let current = handler;

	function fire() {
		const now = performance.now();
		if (now - lastFired < REPEAT_DEBOUNCE_MS) return;
		lastFired = now;
		current();
	}

	function onPointerDown(event: PointerEvent) {
		if (event.pointerType === 'mouse') return;
		pressed = true;
		startX = event.clientX;
		startY = event.clientY;
	}

	function onPointerMove(event: PointerEvent) {
		if (!pressed) return;
		if (
			Math.abs(event.clientX - startX) > MOVE_THRESHOLD_PX ||
			Math.abs(event.clientY - startY) > MOVE_THRESHOLD_PX
		) {
			// Treat as a scroll gesture — drop the tap.
			pressed = false;
		}
	}

	function onPointerUp(event: PointerEvent) {
		if (event.pointerType === 'mouse') return;
		if (!pressed) return;
		pressed = false;
		event.preventDefault();
		fire();
	}

	function onPointerCancel() {
		pressed = false;
	}

	function onClick() {
		fire();
	}

	// Pointer listeners must be passive where possible so iOS Safari can
	// hand the touch over to native scroll the moment the user starts a
	// vertical drag. A non-passive pointermove on every row of a long list
	// makes single-finger scroll get stuck (two-finger still works because
	// gesture events bypass this path). Only pointerup is non-passive
	// because we preventDefault on it to suppress the synthetic click.
	const passive = { passive: true } as const;
	node.addEventListener('pointerdown', onPointerDown, passive);
	node.addEventListener('pointermove', onPointerMove, passive);
	node.addEventListener('pointerup', onPointerUp);
	node.addEventListener('pointercancel', onPointerCancel, passive);
	node.addEventListener('click', onClick);

	return {
		update(newHandler: () => void) {
			current = newHandler;
		},
		destroy() {
			node.removeEventListener('pointerdown', onPointerDown);
			node.removeEventListener('pointermove', onPointerMove);
			node.removeEventListener('pointerup', onPointerUp);
			node.removeEventListener('pointercancel', onPointerCancel);
			node.removeEventListener('click', onClick);
		}
	};
}
