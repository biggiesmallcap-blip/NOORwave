import { describe, expect, it } from 'vitest';
import { PALETTES, paletteById, rgbaCss } from './palettes';

describe('wallpaper palettes', () => {
	it('includes black and dark colour schemes', () => {
		const darkIds = ['obsidian', 'carbon', 'blackout', 'nocturne'];

		for (const id of darkIds) {
			const palette = paletteById(id as (typeof PALETTES)[number]['id']);
			expect(palette.id).toBe(id);
			expect(palette.shader.c1.every((channel) => channel <= 0.02)).toBe(true);
		}
	});

	it('can express shader colours as translucent CSS for the no-wallpaper background', () => {
		expect(rgbaCss([0.12, 0.8, 1], 0.18)).toBe('rgba(31, 204, 255, 0.18)');
	});
});
