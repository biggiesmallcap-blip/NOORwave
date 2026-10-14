import { describe, expect, it } from 'vitest';
import { hexToOklch } from './oklch';
import { GROUND_PROPS, applyPaletteTheme, contrastRatio, paletteGround, readableOnLight } from './paletteTheme';
import { PALETTES, type PaletteId } from './palettes';

function themeRoot() {
	const attributes = new Map<string, string>();
	const properties = new Map<string, string>();
	return {
		attributes,
		properties,
		setAttribute: (name: string, value: string) => { attributes.set(name, value); },
		style: {
			setProperty: (name: string, value: string) => { properties.set(name, value); },
			removeProperty: (name: string) => { properties.delete(name); return ''; },
		},
	};
}

// Grounds the hand-made materials define in app.css.
const MATERIAL_GROUND: Record<string, Record<'dark' | 'light', string>> = {
	clay: { dark: '#221b16', light: '#eee5d5' },
	void: { dark: '#000000', light: '#ededed' },
};

function groundOf(id: PaletteId, mode: 'dark' | 'light'): string {
	return MATERIAL_GROUND[id]?.[mode] ?? paletteGround(id, mode)!['--bg-base'];
}

function hueDistance(a: number, b: number): number {
	const d = Math.abs(a - b) % 360;
	return d > 180 ? 360 - d : d;
}

/** Composite a translucent text token over its ground, then measure it. */
function compositeContrast(rgba: [number, number, number, number], ground: string): number {
	const g = [1, 3, 5].map((i) => parseInt(ground.slice(i, i + 2), 16));
	const mixed = rgba.slice(0, 3).map((v, i) => Math.round(v * rgba[3] + g[i] * (1 - rgba[3])));
	return contrastRatio('#' + mixed.map((v) => v.toString(16).padStart(2, '0')).join(''), ground);
}

describe('palette theme application', () => {
	it('applies readable clay accents to light surfaces and identifies the palette', () => {
		const root = themeRoot();
		applyPaletteTheme(root, 'clay', 'light');
		expect(root.attributes.get('data-palette')).toBe('clay');
		expect(root.properties.get('--accent')).toBe('#8e482e');
		expect(root.properties.get('--accent-strong')).toBe('#713921');
		expect(root.properties.get('--accent-soft')).toBe('rgba(142, 72, 46, 0.10)');
	});

	it('retunes the active clay palette when the surface changes in either direction', () => {
		const root = themeRoot();
		applyPaletteTheme(root, 'clay', 'light');
		applyPaletteTheme(root, 'clay', 'dark');
		expect(root.attributes.get('data-palette')).toBe('clay');
		expect(root.properties.get('--accent')).toBe('#d4a57a');
		expect(root.properties.get('--accent-strong')).toBe('#efc8a4');
		applyPaletteTheme(root, 'clay', 'light');
		expect(root.properties.get('--accent')).toBe('#8e482e');
		expect(root.properties.get('--accent-strong')).toBe('#713921');
	});

	it('replaces clay overrides when another saved palette is selected', () => {
		const root = themeRoot();
		applyPaletteTheme(root, 'clay', 'light');
		applyPaletteTheme(root, 'iris', 'dark');
		expect(root.attributes.get('data-palette')).toBe('iris');
		expect(root.properties.get('--accent')).toBe('#7c80ff');
		expect(root.properties.get('--accent-soft')).toBe('rgba(124, 128, 255, 0.14)');
		expect(root.properties.get('--atlas-haze-a')).toBe('rgba(194, 56, 242, 0.18)');
	});

	it('applies pure black as a monochrome accent with no atlas haze', () => {
		const root = themeRoot();
		applyPaletteTheme(root, 'void', 'dark');
		expect(root.attributes.get('data-palette')).toBe('void');
		expect(root.properties.get('--accent')).toBe('#ffffff');
		expect(root.properties.get('--atlas-haze-a')).toBe('rgba(10, 10, 10, 0)');
		applyPaletteTheme(root, 'void', 'light');
		expect(root.properties.get('--accent')).toBe('#000000');
	});

	it('gives every palette a readable --accent-strong on its ground', () => {
		for (const palette of PALETTES) {
			for (const mode of ['dark', 'light'] as const) {
				const root = themeRoot();
				applyPaletteTheme(root, palette.id, mode);
				const ratio = contrastRatio(root.properties.get('--accent-strong')!, groundOf(palette.id, mode));
				expect(ratio, `${palette.id} ${mode}`).toBeGreaterThanOrEqual(4.5);
			}
		}
	});

	it('darkens a pale accent for light surfaces and keeps a readable one', () => {
		expect(readableOnLight('#7c80ff')).not.toBe('#7c80ff');
		expect(contrastRatio(readableOnLight('#7c80ff'), '#f2f4f7')).toBeGreaterThanOrEqual(4.5);
		expect(readableOnLight('#3a3dc8')).toBe('#3a3dc8');
	});
});

describe('modern palette surfaces', () => {
	it('tints generated grounds with the palette hue', () => {
		const iris = hexToOklch(paletteGround('iris', 'dark')!['--bg-base']);
		const futuro = hexToOklch(paletteGround('futuro', 'dark')!['--bg-base']);
		expect(hueDistance(iris.h, hexToOklch('#7c80ff').h)).toBeLessThan(8);
		expect(hueDistance(futuro.h, hexToOklch('#cc2e1a').h)).toBeLessThan(8);
		expect(iris.c).toBeGreaterThan(0.008);
	});

	it('takes a light ground from the palette light base, not a warm accent', () => {
		// Futuro is cream / vermilion: its light ground follows the cream base
		// (hue ~85) instead of reading as pink paper from the red accent (~30).
		const futuro = hexToOklch(paletteGround('futuro', 'light')!['--bg-base']);
		expect(hueDistance(futuro.h, 85)).toBeLessThan(20);
		expect(futuro.c).toBeLessThan(0.01);
	});

	it('keeps low-chroma palettes neutral', () => {
		expect(hexToOklch(paletteGround('mono', 'dark')!['--bg-base']).c).toBeLessThan(0.003);
		expect(hexToOklch(paletteGround('mono', 'light')!['--bg-base']).c).toBeLessThan(0.003);
	});

	it('puts the black family on a deeper ground than the colour family', () => {
		const carbon = hexToOklch(paletteGround('carbon', 'dark')!['--bg-base']);
		const iris = hexToOklch(paletteGround('iris', 'dark')!['--bg-base']);
		expect(carbon.l).toBeLessThan(iris.l - 0.03);
	});

	it('steps surfaces in the right order in both modes', () => {
		for (const palette of PALETTES.filter((p) => p.family !== 'material')) {
			const dark = paletteGround(palette.id, 'dark')!;
			expect(hexToOklch(dark['--bg-elevated']).l).toBeGreaterThan(hexToOklch(dark['--bg-base']).l);
			expect(hexToOklch(dark['--bg-raised']).l).toBeGreaterThan(hexToOklch(dark['--bg-elevated']).l);
			const light = paletteGround(palette.id, 'light')!;
			expect(hexToOklch(light['--bg-elevated']).l).toBeGreaterThan(hexToOklch(light['--bg-base']).l);
			expect(hexToOklch(light['--bg-raised']).l).toBeLessThan(hexToOklch(light['--bg-base']).l);
		}
	});

	it('leaves material grounds to app.css and clears generated ones on switch', () => {
		const root = themeRoot();
		applyPaletteTheme(root, 'iris', 'dark');
		expect(root.properties.get('--bg-base')).toBe(paletteGround('iris', 'dark')!['--bg-base']);
		applyPaletteTheme(root, 'clay', 'dark');
		for (const prop of GROUND_PROPS) expect(root.properties.has(prop), prop).toBe(false);
		expect(paletteGround('void', 'light')).toBeNull();
	});

	it('keeps body text readable on every generated ground', () => {
		const tertiary: Record<'dark' | 'light', [number, number, number, number]> = {
			dark: [255, 255, 255, 0.5],
			light: [9, 13, 22, 0.6],
		};
		for (const palette of PALETTES.filter((p) => p.family !== 'material')) {
			for (const mode of ['dark', 'light'] as const) {
				const ground = paletteGround(palette.id, mode)!;
				for (const surface of ['--bg-base', '--bg-elevated'] as const) {
					const ratio = compositeContrast(tertiary[mode], ground[surface]);
					expect(ratio, `${palette.id} ${mode} ${surface}`).toBeGreaterThanOrEqual(4.5);
				}
			}
		}
	});
});

describe('accent readability', () => {
	it('keeps the accent hue when deriving light-mode accents', () => {
		for (const id of ['citrus', 'verdant', 'arctic', 'iris'] as const) {
			const root = themeRoot();
			applyPaletteTheme(root, id, 'light');
			const source = hexToOklch(PALETTES.find((p) => p.id === id)!.ui.accent);
			const derived = hexToOklch(root.properties.get('--accent')!);
			expect(hueDistance(source.h, derived.h), id).toBeLessThan(12);
		}
	});

	it('makes the light accent readable as a fill and as text', () => {
		for (const palette of PALETTES.filter((p) => !p.lightUi)) {
			const root = themeRoot();
			applyPaletteTheme(root, palette.id, 'light');
			const ground = groundOf(palette.id, 'light');
			expect(contrastRatio(root.properties.get('--accent')!, ground), palette.id).toBeGreaterThanOrEqual(3);
		}
	});

	it('picks button text that reads on every accent fill', () => {
		for (const palette of PALETTES.filter((p) => p.family !== 'material')) {
			for (const mode of ['dark', 'light'] as const) {
				const root = themeRoot();
				applyPaletteTheme(root, palette.id, mode);
				const ratio = contrastRatio(root.properties.get('--accent-ink')!, root.properties.get('--accent')!);
				expect(ratio, `${palette.id} ${mode}`).toBeGreaterThanOrEqual(4.5);
			}
		}
	});

	it('uses dark ink on bright accents and white ink on deep ones', () => {
		const citrus = themeRoot();
		applyPaletteTheme(citrus, 'citrus', 'dark');
		expect(hexToOklch(citrus.properties.get('--accent-ink')!).l).toBeLessThan(0.3);
		const futuro = themeRoot();
		applyPaletteTheme(futuro, 'futuro', 'light');
		expect(futuro.properties.get('--accent-ink')).toBe('#ffffff');
	});
});

describe('palette catalogue', () => {
	it('groups every palette and keeps labels ASCII', () => {
		for (const palette of PALETTES) {
			expect(['material', 'dark', 'colour', 'light']).toContain(palette.family);
			expect(palette.label + palette.sublabel, palette.id).toMatch(/^[\x20-\x7e]*$/);
		}
		expect(PALETTES.filter((p) => p.family === 'material').map((p) => p.id)).toEqual(['clay', 'void']);
	});
});

describe('Pure ground in app.css', () => {
	it('keeps the flat Pure body above the general :root body gradient', async () => {
		const { readFileSync } = await import('node:fs');
		const css = readFileSync(new URL('../../../app.css', import.meta.url), 'utf8');
		// (0,2,1) beats `:root body` (0,1,1) wherever it appears, responsive copy included.
		expect(css).toContain(':root[data-palette="void"] body {');
		expect(css).not.toMatch(/^\[data-palette="void"\] body \{/m);
	});
});
