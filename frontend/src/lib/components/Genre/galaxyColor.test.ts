import { describe, expect, test } from 'vitest';
import { HUE_SPREAD, hexToRgb255, rgb255ToHex, rgbToHsl, varyFamilyColor } from './galaxyColor';

function hueOf(hex: string): number {
	const rgb = hexToRgb255(hex);
	if (!rgb) throw new Error(`bad hex ${hex}`);
	return rgbToHsl(rgb)[0];
}

describe('galaxyColor', () => {
	test('round-trips hex', () => {
		expect(rgb255ToHex(hexToRgb255('#49d1b7')!)).toBe('#49d1b7');
		expect(hexToRgb255('nope')).toBeNull();
	});

	test('family roots keep the family color', () => {
		expect(varyFamilyColor('#9699f5', 7, 0)).toBe('#9699f5');
	});

	test('a sub-genre shade is stable per seed', () => {
		expect(varyFamilyColor('#9699f5', 42, 1)).toBe(varyFamilyColor('#9699f5', 42, 1));
	});

	test('siblings get visibly different shades', () => {
		const shades = new Set([1, 2, 3, 4, 5, 6].map((seed) => varyFamilyColor('#9699f5', seed, 1)));
		expect(shades.size).toBeGreaterThan(3);
	});

	test('hue stays inside the family spread', () => {
		const base = hueOf('#9699f5');
		for (let seed = 1; seed < 60; seed += 1) {
			const hue = hueOf(varyFamilyColor('#9699f5', seed, 2));
			const delta = Math.abs(((hue - base + 540) % 360) - 180);
			expect(delta).toBeLessThanOrEqual(HUE_SPREAD + 1);
		}
	});
});
