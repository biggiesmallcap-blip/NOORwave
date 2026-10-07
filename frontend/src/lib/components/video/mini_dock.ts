// Placement for the floating mini video window. It lives inside the content
// area (the workspace), never over the side panel or the bottom player bar,
// snaps to one of the area's four corners, and remembers the corner.

export type Corner = 'tl' | 'tr' | 'bl' | 'br';

export interface Bounds {
	left: number;
	top: number;
	right: number;
	bottom: number;
}

export interface Size {
	width: number;
	height: number;
}

const CORNER_KEY = 'noor_video_mini_corner';
const CORNERS: readonly Corner[] = ['tl', 'tr', 'bl', 'br'];
export const MINI_MARGIN = 16;
export const PILL_SIZE: Size = { width: 260, height: 44 };

/** Storage can be full, blocked or absent; the window then simply opens
 *  bottom right. */
export function loadCorner(): Corner {
	try {
		const stored = localStorage.getItem(CORNER_KEY);
		return CORNERS.includes(stored as Corner) ? (stored as Corner) : 'br';
	} catch {
		return 'br';
	}
}

export function saveCorner(corner: Corner) {
	try {
		localStorage.setItem(CORNER_KEY, corner);
	} catch {
		// Not remembering a corner is harmless.
	}
}

/** Same width rule the CSS used: 24% of the window, 248-340 px, 16:9. */
export function miniSize(viewportWidth: number, collapsed: boolean): Size {
	if (collapsed) return PILL_SIZE;
	const width = Math.round(Math.min(340, Math.max(248, viewportWidth * 0.24)));
	return { width, height: Math.round((width * 9) / 16) };
}

/** Top-left position for a window of `size` in `corner` of `bounds`. A
 *  content area narrower than the window pins it to the area's left/top. */
export function placeMini(bounds: Bounds, corner: Corner, size: Size, margin = MINI_MARGIN) {
	const left = corner.endsWith('l')
		? bounds.left + margin
		: Math.max(bounds.left + margin, bounds.right - margin - size.width);
	const top = corner.startsWith('t')
		? bounds.top + margin
		: Math.max(bounds.top + margin, bounds.bottom - margin - size.height);
	return { left: Math.round(left), top: Math.round(top) };
}

/** The corner whose quadrant holds the window's centre after a drag. */
export function nearestCorner(bounds: Bounds, centerX: number, centerY: number): Corner {
	const vertical = centerY < (bounds.top + bounds.bottom) / 2 ? 't' : 'b';
	const horizontal = centerX < (bounds.left + bounds.right) / 2 ? 'l' : 'r';
	return `${vertical}${horizontal}` as Corner;
}

/** Arrow keys on the move handle: left/right pick the side, up/down the edge. */
export function cornerForKey(corner: Corner, key: string): Corner {
	const vertical = corner[0];
	const horizontal = corner[1];
	switch (key) {
		case 'ArrowLeft':
			return `${vertical}l` as Corner;
		case 'ArrowRight':
			return `${vertical}r` as Corner;
		case 'ArrowUp':
			return `t${horizontal}` as Corner;
		case 'ArrowDown':
			return `b${horizontal}` as Corner;
		default:
			return corner;
	}
}
