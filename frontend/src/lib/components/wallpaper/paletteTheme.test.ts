import { describe, expect, it } from 'vitest';
import { applyPaletteTheme, contrastRatio, readableOnLight } from './paletteTheme';
import { PALETTES } from './palettes';

function themeRoot() {
	const attributes = new Map<string, string>();
	const properties = new Map<string, string>();
	return {
		attributes,
		properties,
		setAttribute: (name: string, value: string) => { attributes.set(name, value); },
		style: { setProperty: (name: string, value: string) => { properties.set(name, value); } },
	};
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
		applyPaletteTheme(root, 'iris', 'light');
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
			const dark = themeRoot();
			applyPaletteTheme(dark, palette.id, 'dark');
			expect(contrastRatio(dark.properties.get('--accent-strong')!, '#0b0b0f')).toBeGreaterThanOrEqual(4.5);

			const light = themeRoot();
			applyPaletteTheme(light, palette.id, 'light');
			const ground = palette.lightUi ? '#eee5d5' : '#f2f4f7';
			expect(contrastRatio(light.properties.get('--accent-strong')!, ground)).toBeGreaterThanOrEqual(4.5);
		}
	});

	it('darkens a pale accent for light surfaces and keeps a readable one', () => {
		expect(readableOnLight('#7c80ff')).not.toBe('#7c80ff');
		expect(contrastRatio(readableOnLight('#7c80ff'), '#f2f4f7')).toBeGreaterThanOrEqual(4.5);
		expect(readableOnLight('#3a3dc8')).toBe('#3a3dc8');
	});
});
