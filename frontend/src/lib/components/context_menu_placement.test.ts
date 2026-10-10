import { describe, expect, test } from 'vitest';
import { MENU_EDGE, placeAxis } from './context_menu_placement';

describe('context menu placement', () => {
	test('opens after the anchor when it fits', () => {
		expect(placeAxis(100, 100, 300, 1000)).toEqual({ pos: 100, after: true });
	});

	test('flips before the flip anchor when the after side lacks room', () => {
		expect(placeAxis(900, 880, 300, 1000)).toEqual({ pos: 580, after: false });
	});

	test('clamps inside the viewport when neither side fits', () => {
		expect(placeAxis(300, 300, 600, 700)).toEqual({ pos: 700 - 600 - MENU_EDGE, after: true });
	});

	test('a locked menu that grows slides instead of flipping', () => {
		// Library repro: 1400px viewport, right-click at y=900, Download expands
		// the menu from 456px to 585px. Unlocked it flips to y=315, away from the
		// pointer; locked it slides up and still covers its old extent.
		const viewport = 1400;
		const opened = placeAxis(900, 900, 456, viewport);
		expect(opened.after).toBe(true);
		const grown = placeAxis(900, 900, 585, viewport, opened.after);
		expect(grown.after).toBe(true);
		expect(grown.pos).toBeLessThanOrEqual(opened.pos);
		expect(grown.pos + 585).toBeGreaterThanOrEqual(opened.pos + 456);
		expect(grown.pos + 585).toBeLessThanOrEqual(viewport - MENU_EDGE);
	});

	test('a locked flipped menu grows upward from its anchor', () => {
		const opened = placeAxis(900, 880, 300, 1000);
		const grown = placeAxis(900, 880, 420, 1000, opened.after);
		expect(grown).toEqual({ pos: 460, after: false });
		expect(grown.pos + 420).toBe(opened.pos + 300);
	});
});
