// The app scrolls an inner `<main class="workspace">` element, not the window:
// the layout pins the sidebar and now-playing rails and gives the main column
// its own `overflow-y: auto`. So `window.scrollY` is always 0 and
// `window.scrollTo` is a no-op here. Snapshot scroll restore must target the
// workspace element instead.

function workspaceEl(): HTMLElement | null {
	if (typeof document === 'undefined') return null;
	return document.querySelector('main.workspace');
}

/** Current scroll offset of the workspace container (0 when not mounted). */
export function captureScroll(): number {
	return workspaceEl()?.scrollTop ?? 0;
}

/**
 * Restore the workspace scroll offset after a back/forward navigation.
 *
 * Several pages load their content async (library artists/tracks, search
 * results), so right after a back-nav the container is often too short to
 * honor the saved offset on the first frame. Retry across a handful of frames
 * until the container is tall enough to reach the target, then stop. Capped so
 * a genuinely-shorter page (fewer rows than before) doesn't spin forever.
 */
export function restoreScroll(top: number, maxFrames = 30): void {
	if (top <= 0) return;
	let frames = 0;
	const tick = () => {
		const el = workspaceEl();
		if (!el) {
			if (frames++ < maxFrames) requestAnimationFrame(tick);
			return;
		}
		el.scrollTo({ top, behavior: 'auto' });
		const reachedTarget = el.scrollHeight - el.clientHeight >= top - 1;
		if (!reachedTarget && frames++ < maxFrames) requestAnimationFrame(tick);
	};
	requestAnimationFrame(tick);
}
