import { describe, expect, test } from 'vitest';
import {
	PICTURE_BUCKETS,
	PICTURE_UNKNOWN,
	footprintStep,
	frontierStep,
	parseLook,
	pictureBucket,
	pictureColor,
	rgbToHsl,
	trailAlpha,
} from './style';

describe('atlas looks', () => {
	test('retired looks map onto the new ones', () => {
		expect(parseLook('light')).toBe('you');
		expect(parseLook('place')).toBe('picture');
		expect(parseLook('you')).toBe('you');
		expect(parseLook(undefined)).toBe('picture');
	});

	test('artwork colours keep their hue and land in a drawable group', () => {
		expect(rgbToHsl(0xff0000).h).toBeCloseTo(0);
		expect(rgbToHsl(0x0000ff).h).toBeCloseTo(240);
		const red = pictureBucket(0xd03020);
		const blue = pictureBucket(0x2040c0);
		expect(red).not.toBe(blue);
		for (const bucket of [red, blue, pictureBucket(0x808080), pictureBucket(0x000000)]) {
			expect(bucket).toBeGreaterThanOrEqual(0);
			expect(bucket).toBeLessThan(PICTURE_UNKNOWN);
		}
		// Black-and-white videos stay grey.
		expect(pictureColor(pictureBucket(0x8a8a8a))).toMatch(/^hsla\(220/);
		expect(pictureColor(PICTURE_UNKNOWN)).toContain('0.26');
		expect(PICTURE_BUCKETS).toBe(100);
	});

	test('footprints fade by age, the frontier steps by score, trails halve every two weeks', () => {
		expect([footprintStep(2), footprintStep(20), footprintStep(90)]).toEqual([0, 1, 2]);
		expect([frontierStep(0.6), frontierStep(0.3), frontierStep(0.05)]).toEqual([0, 1, 2]);
		expect(trailAlpha(14)).toBeCloseTo(trailAlpha(0) / 2);
	});
});
