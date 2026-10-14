import { describe, expect, it } from 'vitest';
import { hexToOklch, oklchToHex, oklchToRgb } from './oklch';

describe('oklch conversion', () => {
	it('round-trips sRGB hex through OKLCH', () => {
		for (const hex of ['#000000', '#ffffff', '#7c80ff', '#cc2e1a', '#5dd7a8', '#f2f4f7', '#0b0b0f']) {
			expect(oklchToHex(hexToOklch(hex))).toBe(hex);
		}
	});

	it('reads lightness, chroma and hue', () => {
		const white = hexToOklch('#ffffff');
		expect(white.l).toBeCloseTo(1, 3);
		expect(white.c).toBeLessThan(0.001);
		const red = hexToOklch('#ff0000');
		expect(red.l).toBeCloseTo(0.628, 2);
		expect(red.c).toBeCloseTo(0.258, 2);
		expect(red.h).toBeCloseTo(29.2, 0);
	});

	it('pulls out-of-gamut colours back into sRGB by lowering chroma', () => {
		const rgb = oklchToRgb({ l: 0.5, c: 0.4, h: 110 });
		for (const channel of rgb) {
			expect(channel).toBeGreaterThanOrEqual(0);
			expect(channel).toBeLessThanOrEqual(255);
		}
		const back = hexToOklch(oklchToHex({ l: 0.5, c: 0.4, h: 110 }));
		expect(back.l).toBeCloseTo(0.5, 2);
		expect(Math.abs(back.h - 110)).toBeLessThan(2);
	});
});
