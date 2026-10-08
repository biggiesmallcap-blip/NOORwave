import { describe, expect, test } from 'vitest';
import { PALETTES } from '../wallpaper/palettes';
import { buildGalaxyTheme } from './galaxyTheme';

function channels(css: string): number[] {
	return (css.match(/\d+(\.\d+)?/g) ?? []).slice(0, 3).map(Number);
}

describe('buildGalaxyTheme', () => {
	test('the sky is a night sky for every palette, light ones included', () => {
		for (const palette of PALETTES) {
			const theme = buildGalaxyTheme(palette.id);
			for (const stop of theme.sky) {
				expect(Math.max(...channels(stop))).toBeLessThanOrEqual(51);
			}
		}
	});

	test('the sky is tinted by the palette', () => {
		expect(buildGalaxyTheme('clay').sky[0]).not.toBe(buildGalaxyTheme('iris').sky[0]);
	});

	test('clay gets a warm night', () => {
		const [red, , blue] = channels(buildGalaxyTheme('clay').sky[0]);
		expect(red).toBeGreaterThan(blue);
	});

	test('labels stay light because the map is always dark', () => {
		const theme = buildGalaxyTheme('daylight');
		expect(Math.min(...channels(theme.labelText))).toBeGreaterThan(200);
	});
});
