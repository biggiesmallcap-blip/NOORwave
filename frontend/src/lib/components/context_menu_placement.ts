export const MENU_EDGE = 8;

export interface AxisPlacement {
	/** Top/left coordinate of the menu on this axis. */
	pos: number;
	/** True when the menu sits after its anchor (below / right of it). */
	after: boolean;
}

/**
 * Native-menu placement on one axis: after the anchor when it fits, before
 * the flip anchor when that fits, otherwise clamped inside the viewport.
 *
 * `lockedAfter` pins the side chosen earlier (set once a submenu expands).
 * A locked menu slides to fit instead of flipping, so its new extent still
 * covers its old one and the pointer that toggled the submenu stays inside
 * (a flip fires pointerleave, which closes the menu).
 */
export function placeAxis(
	anchor: number,
	flipAnchor: number,
	size: number,
	viewport: number,
	lockedAfter?: boolean,
): AxisPlacement {
	const clampAfter = Math.max(MENU_EDGE, Math.min(anchor, viewport - size - MENU_EDGE));
	if (lockedAfter === true) return { pos: clampAfter, after: true };
	if (lockedAfter === false) return { pos: Math.max(MENU_EDGE, flipAnchor - size), after: false };
	if (anchor + size + MENU_EDGE <= viewport) return { pos: anchor, after: true };
	if (flipAnchor - size >= MENU_EDGE) return { pos: flipAnchor - size, after: false };
	return { pos: clampAfter, after: true };
}
