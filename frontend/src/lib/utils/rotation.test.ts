import { describe, expect, test } from 'vitest';
import { rotatingWindow, rotationForPeriod } from './rotation';

const fifty = Array.from({ length: 50 }, (_, i) => i);

describe('rotatingWindow', () => {
	test('returns the list untouched when it already fits', () => {
		const twenty = fifty.slice(0, 20);
		expect(rotatingWindow(twenty, 20, 0)).toBe(twenty);
		expect(rotatingWindow(twenty, 20, 7 * 20)).toBe(twenty);
		expect(rotatingWindow([1, 2, 3], 20, 40)).toEqual([1, 2, 3]);
	});

	test('is always exactly `size` long, wrapping past the end', () => {
		// The reason this wraps instead of paging: fifty in pages of twenty ends on
		// a page of ten, and a rail showing ten cards where it just showed twenty
		// reads as broken.
		for (const rotation of [0, 1, 2, 3, 4, 5, 11]) {
			expect(rotatingWindow(fifty, 20, rotation * 20)).toHaveLength(20);
		}
		expect(rotatingWindow(fifty, 20, 40)).toEqual([
			...fifty.slice(40),
			...fifty.slice(0, 10),
		]);
	});

	test('successive rotations show different items', () => {
		const first = rotatingWindow(fifty, 20, 0);
		const second = rotatingWindow(fifty, 20, 20);
		expect(second).not.toEqual(first);
		// Twenty out of fifty means consecutive windows do not overlap at all.
		expect(second.filter((item) => first.includes(item))).toEqual([]);
	});

	test('handles a negative or huge offset without going out of bounds', () => {
		expect(rotatingWindow(fifty, 20, -20)).toEqual([
			...fifty.slice(30),
			...fifty.slice(0, 0),
		]);
		expect(rotatingWindow(fifty, 20, Number.MAX_SAFE_INTEGER)).toHaveLength(20);
		expect(rotatingWindow(fifty, 20, -20).every((n) => n >= 0 && n < 50)).toBe(true);
	});

	test('degenerate inputs give an empty window rather than throwing', () => {
		expect(rotatingWindow([], 20, 0)).toEqual([]);
		expect(rotatingWindow(fifty, 0, 0)).toEqual([]);
		expect(rotatingWindow(fifty, -1, 0)).toEqual([]);
	});
});

describe('rotationForPeriod', () => {
	const twoHours = 2 * 60 * 60 * 1000;

	test('is stable inside a period and advances between them', () => {
		const base = 1_800_000_000_000;
		const start = Math.floor(base / twoHours) * twoHours;
		expect(rotationForPeriod(twoHours, start)).toBe(rotationForPeriod(twoHours, start + 1));
		expect(rotationForPeriod(twoHours, start)).toBe(
			rotationForPeriod(twoHours, start + twoHours - 1),
		);
		expect(rotationForPeriod(twoHours, start + twoHours)).toBe(
			rotationForPeriod(twoHours, start) + 1,
		);
	});

	test('a six-hour cache lease covers three two-hour rotations', () => {
		const start = Math.floor(1_800_000_000_000 / twoHours) * twoHours;
		const seen = new Set(
			[0, 1, 2].map((i) => rotationForPeriod(twoHours, start + i * twoHours) % 3),
		);
		expect(seen.size).toBe(3);
	});

	test('a zero or negative period never divides by zero', () => {
		expect(rotationForPeriod(0)).toBe(0);
		expect(rotationForPeriod(-1)).toBe(0);
	});
});
