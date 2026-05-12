import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

const shadersSource = readFileSync('src/lib/components/wallpaper/shaders.ts', 'utf8');
const palettesSource = readFileSync('src/lib/components/wallpaper/palettes.ts', 'utf8');
const rendererSource = readFileSync('src/lib/components/wallpaper/ShaderWallpaper.svelte', 'utf8');
const settingsSource = readFileSync('src/routes/settings/+page.svelte', 'utf8');

describe('pattern wallpaper contract', () => {
	test('exposes the full design bundle pattern set as wallpaper settings options', () => {
		for (const id of [
			'pattern-grid',
			'pattern-dots',
			'pattern-hatch',
			'pattern-truchet',
			'pattern-waves',
			'pattern-noise',
			'pattern-plasma',
			'pattern-kaleido',
			'pattern-tunnel',
			'pattern-melt',
			'pattern-speed',
			'pattern-vortex',
			'pattern-shards',
			'pattern-vector',
		]) {
			expect(shadersSource).toContain(`id: '${id}'`);
		}
	});

	test('keeps wallpaper options in the existing Appearance settings layout', () => {
		expect(settingsSource).toContain("{#if activeCategory === 'appearance'}");
		expect(settingsSource).toContain('WALLPAPERS.filter');
		expect(settingsSource).toContain('More shaders ({extendedWallpaperCount})');
	});

	test('includes the design bundle palettes in the shared palette selector', () => {
		for (const id of ['slate', 'paper', 'moss', 'plum', 'acid', 'neon', 'futuro', 'constr']) {
			expect(palettesSource).toContain(`id: '${id}'`);
		}
	});

	test('renders wallpapers with the lightweight fullscreen-triangle WebGL path', () => {
		expect(rendererSource).toContain("powerPreference: 'low-power'");
		expect(rendererSource).toContain('antialias: false');
		expect(rendererSource).toContain('depth: false');
		expect(rendererSource).toContain('stencil: false');
		expect(rendererSource).toContain('new Float32Array([-1, -1, 3, -1, -1, 3])');
		expect(rendererSource).toContain('gl!.drawArrays(gl!.TRIANGLES, 0, 3)');
	});
});
