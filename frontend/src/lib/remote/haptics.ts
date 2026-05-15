import { hapticsAreEnabled } from '$lib/remote/haptics_settings';

/**
 * Thin wrapper around `navigator.vibrate` with feature detection and SSR
 * safety. iOS Safari supports `vibrate` only inside an installed PWA on
 * supported hardware (iPhone X+) and otherwise no-ops silently. Android and
 * Chrome desktop support it natively. We swallow any failure so a missing
 * permission never bubbles up. Bails early when the user has turned haptics
 * off in remote settings.
 */
function vibrate(pattern: number | number[]): void {
	if (!hapticsAreEnabled()) return;
	if (typeof navigator === 'undefined') return;
	const nav = navigator as Navigator & { vibrate?: unknown };
	if (typeof nav.vibrate !== 'function') return;
	try {
		// The lib.dom `vibrate` overloads collide on a union argument, so call
		// through a loose signature that just forwards the pattern verbatim.
		(nav.vibrate as (p: number | number[]) => boolean).call(navigator, pattern);
	} catch {
		// Some browsers throw if the page hasn't had a user gesture yet — the
		// remote always has, but defensively ignore.
	}
}

/** A tap-style cue for low-stakes confirmations (toggle on/off). */
export function hapticTap(): void {
	vibrate(10);
}

/** A slightly heavier cue for "you crossed a threshold" actions (track skip). */
export function hapticCommit(): void {
	vibrate(18);
}

/** A double-pulse for destructive or significant events (favourite, sleep timer set). */
export function hapticAccent(): void {
	vibrate([12, 30, 12]);
}
