import { describe, expect, it } from 'vitest';
import { applyPaletteTheme } from './paletteTheme';

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
});
