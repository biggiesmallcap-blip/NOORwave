import { describe, expect, test } from 'vitest';
import { labelAlpha, labelPriority, placeLabels } from './galaxyLabels';

const base = { zoom: 0.58, selected: false, inLineage: false, labelsEnabled: true, compact: false };

describe('labelAlpha', () => {
	test('family names always show', () => {
		expect(labelAlpha({ ...base, depth: 0 })).toBeGreaterThan(0.8);
		expect(labelAlpha({ ...base, depth: 0, labelsEnabled: false })).toBeGreaterThan(0.8);
	});

	test('sub-genres stay hidden at galaxy zoom and fade in when zoomed', () => {
		expect(labelAlpha({ ...base, depth: 1 })).toBe(0);
		expect(labelAlpha({ ...base, depth: 1, zoom: 1.2 })).toBeGreaterThan(0.8);
		expect(labelAlpha({ ...base, depth: 2, zoom: 1.2 })).toBe(0);
		expect(labelAlpha({ ...base, depth: 2, zoom: 2.4 })).toBeGreaterThan(0.6);
	});

	test('the Labels toggle hides sub-genres only', () => {
		expect(labelAlpha({ ...base, depth: 1, zoom: 2, labelsEnabled: false })).toBe(0);
	});

	test('selected and lineage always show', () => {
		expect(labelAlpha({ ...base, depth: 3, selected: true })).toBe(1);
		expect(labelAlpha({ ...base, depth: 2, inLineage: true })).toBeGreaterThan(0.8);
	});
});

describe('placeLabels', () => {
	const viewport = { width: 800, height: 600 };

	test('drops the lower-priority label of an overlapping pair', () => {
		const accepted = placeLabels(
			[
				{ id: 1, x: 100, y: 100, width: 80, height: 16, priority: labelPriority(1, false, false, 0) },
				{ id: 2, x: 120, y: 104, width: 80, height: 16, priority: labelPriority(0, false, false, 0) }
			],
			viewport
		);
		expect([...accepted]).toEqual([2]);
	});

	test('keeps separated labels', () => {
		const accepted = placeLabels(
			[
				{ id: 1, x: 100, y: 100, width: 80, height: 16, priority: 1 },
				{ id: 2, x: 300, y: 100, width: 80, height: 16, priority: 1 }
			],
			viewport
		);
		expect(accepted.size).toBe(2);
	});

	test('drops labels that are not fully on screen', () => {
		const accepted = placeLabels([{ id: 1, x: 760, y: 100, width: 80, height: 16, priority: 1 }], viewport);
		expect(accepted.size).toBe(0);
	});

	test('a colliding label moves to its fallback spot before being dropped', () => {
		const below = { id: 2, x: 120, y: 104, width: 80, height: 16, priority: 1, altY: 40 };
		const accepted = placeLabels(
			[{ id: 1, x: 100, y: 100, width: 80, height: 16, priority: 5 }, below],
			{ width: 800, height: 600 }
		);
		expect(accepted.size).toBe(2);
		expect(below.y).toBe(40);
	});
});
