import { describe, expect, test } from 'vitest';
import { FOCUS_ROWS, focusLevel, gridColumns, mergeFeed, needsMore, placeholderCount, radioExclusions, trimToRows } from './related_feed';

const v = (tidal_id: number, artist_id: number | null = null) => ({ tidal_id, artist_id });

describe('related feed focus', () => {
	test('the first two rows are always sharp', () => {
		expect(FOCUS_ROWS).toBe(2);
		expect(focusLevel(0, 5000, 800, 200)).toBe(0);
		expect(focusLevel(1, 5000, 800, 200)).toBe(0);
	});

	test('a lower row sharpens once scrolled above the focus line', () => {
		expect(focusLevel(2, 790, 800, 200)).toBe(0);
		expect(focusLevel(2, 810, 800, 200)).toBe(1);
		expect(focusLevel(3, 1010, 800, 200)).toBe(2);
		expect(focusLevel(4, 1210, 800, 200)).toBe(3);
		expect(focusLevel(9, 9000, 800, 200)).toBe(3);
	});

	test('reads the column count from a computed grid template', () => {
		expect(gridColumns('210px 210px 210px')).toBe(3);
		expect(gridColumns('none')).toBe(1);
		expect(gridColumns('')).toBe(1);
	});
});

describe('related feed filling', () => {
	test('wants more until two full rows plus a buffer row are loaded', () => {
		expect(needsMore(12, 8)).toBe(true);
		expect(needsMore(24, 8)).toBe(false);
		expect(needsMore(12, 4)).toBe(false);
	});

	test('placeholders complete the last row while a batch loads', () => {
		expect(placeholderCount(12, 8, true)).toBe(4);
		expect(placeholderCount(16, 8, true)).toBe(8);
		expect(placeholderCount(12, 8, false)).toBe(0);
	});

	test('a finished feed drops a ragged tail, never the sharp rows', () => {
		const items = Array.from({ length: 21 }, (_, i) => v(i));
		expect(trimToRows(items, 8)).toHaveLength(16);
		expect(trimToRows(items.slice(0, 12), 8)).toHaveLength(12);
		expect(trimToRows(items.slice(0, 20), 4)).toHaveLength(20);
	});

	test('merging skips the playing video and anything already shown', () => {
		expect(mergeFeed([v(1), v(2)], [v(2), v(3), v(9), v(3)], 9).map((x) => x.tidal_id)).toEqual([1, 2, 3]);
	});

	test('the radio request skips everything on screen, newest first, capped', () => {
		const shown = Array.from({ length: 300 }, (_, i) => v(i + 1, (i % 7) + 1));
		const { exclude, recentArtists } = radioExclusions(shown, 999);
		expect(exclude).toHaveLength(256);
		expect(exclude[0]).toBe(999);
		expect(exclude[1]).toBe(300);
		expect(recentArtists).toEqual([6, 5, 4, 3, 2, 1, 7]);
	});
});
