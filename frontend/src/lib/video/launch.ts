// Where a tune-in was clicked, so the player can grow out of that picture
// when the watch page opens (VideoDock glides from it) instead of fading in.
// One launch at a time, good for a few seconds: a tune that takes longer, or
// a page change for any other reason, simply arrives as it always did.

const FRESH_MS = 8000;

export interface LaunchRect {
	top: number;
	left: number;
	width: number;
	height: number;
}

let pending: { rect: LaunchRect; at: number } | null = null;

/** The 16:9 box a launch grows from: the element itself when it is a
 *  picture, else a 16:9 box at its left edge, as tall as it is (a text line
 *  would otherwise squash the video into a strip). */
export function launchBox(rect: LaunchRect): LaunchRect {
	const height = Math.min(rect.height, (rect.width * 9) / 16);
	return { top: rect.top + (rect.height - height) / 2, left: rect.left, width: (height * 16) / 9, height };
}

export function noteVideoLaunch(el: Element | null | undefined, now = Date.now()): void {
	if (!el) return;
	const rect = el.getBoundingClientRect();
	if (rect.width <= 0 || rect.height <= 0) return;
	pending = { rect: launchBox(rect), at: now };
}

/** Whether a fresh launch is waiting, without taking it. */
export function launchPending(now = Date.now()): boolean {
	return pending !== null && now - pending.at < FRESH_MS;
}

/** The launch box, once, while it is fresh. */
export function takeVideoLaunch(now = Date.now()): LaunchRect | null {
	const launch = pending;
	pending = null;
	return launch && now - launch.at < FRESH_MS ? launch.rect : null;
}
