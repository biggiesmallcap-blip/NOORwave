import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

const shadersSource = readFileSync('src/lib/components/wallpaper/shaders.ts', 'utf8');
const palettesSource = readFileSync('src/lib/components/wallpaper/palettes.ts', 'utf8');
const rendererSource = readFileSync('src/lib/components/wallpaper/ShaderWallpaper.svelte', 'utf8');
const settingsSource = readFileSync('src/routes/settings/+page.svelte', 'utf8');
const layoutSource = readFileSync('src/routes/+layout.svelte', 'utf8');

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
		expect(rendererSource).toContain('targetFps?: number');
		expect(rendererSource).toContain('1000 / targetFps');
		expect(rendererSource).toContain('new Float32Array([-1, -1, 3, -1, -1, 3])');
		expect(rendererSource).toContain('gl!.drawArrays(gl!.TRIANGLES, 0, 3)');
	});

	test('keeps the app wallpaper layer softly blurred behind the UI', () => {
		const match = layoutSource.match(/\.wallpaper-layer\s*\{(?<body>[\s\S]*?)\n\t\}/);
		expect(match?.groups?.body).toContain('filter: blur(var(--wallpaper-blur');
		expect(match?.groups?.body).toContain('transform: scale(var(--wallpaper-scale');
		// Render quality setting: standard caps DPR at 1 (cheap), high allows 2.
		expect(layoutSource).toContain("maxDpr={$wallpaperQuality === 'high' ? 2 : 1}");
		expect(layoutSource).toContain('targetFps={$wallpaperFps}');
	});

	test('uses the wallpaper blur setting for wallpaper shell frost', () => {
		const match = layoutSource.match(
			/\.app-shell\.has-wallpaper \.workspace\s*\{(?<body>[\s\S]*?)\n\t\}/
		);
		expect(match?.groups?.body).toContain('backdrop-filter: blur(var(--wallpaper-blur');
		expect(match?.groups?.body).toContain('-webkit-backdrop-filter: blur(var(--wallpaper-blur');
		expect(match?.groups?.body).not.toContain('var(--blur-overlay)');
		const panelMatch = layoutSource.match(
			/\.app-shell\.has-wallpaper \.sidebar,\s*\n\t\.app-shell\.has-wallpaper \.now-playing-panel\s*\{(?<body>[\s\S]*?)\n\t\}/
		);
		expect(panelMatch?.groups?.body).toContain('backdrop-filter: blur(var(--wallpaper-blur');
		expect(panelMatch?.groups?.body).toContain('-webkit-backdrop-filter: blur(var(--wallpaper-blur');
		expect(panelMatch?.groups?.body).not.toContain('var(--blur-modal)');
	});

	test('exposes wallpaper blur and FPS controls in appearance settings', () => {
		expect(settingsSource).toContain('wallpaperFps');
		expect(settingsSource).toContain('setWallpaperFps');
		expect(settingsSource).toContain('wallpaperBlur');
		expect(settingsSource).toContain('setWallpaperBlur');
		expect(settingsSource).toContain('Wallpaper FPS');
		expect(settingsSource).toContain('Wallpaper blur');
		expect(settingsSource).toContain('min={WALLPAPER_FPS_MIN}');
		expect(settingsSource).toContain('max={WALLPAPER_FPS_MAX}');
		expect(settingsSource).toContain('min={WALLPAPER_BLUR_MIN}');
		expect(settingsSource).toContain('max={WALLPAPER_BLUR_MAX}');
	});
});
