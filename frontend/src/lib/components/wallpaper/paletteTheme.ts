import { paletteById, rgbaCss, type PaletteId } from './palettes';

type ThemeRoot = Pick<HTMLElement, 'setAttribute'> & {
	style: Pick<CSSStyleDeclaration, 'setProperty'>;
};

export function applyPaletteTheme(root: ThemeRoot, id: PaletteId, mode: 'dark' | 'light'): void {
	const palette = paletteById(id);
	const ui = mode === 'light' ? (palette.lightUi ?? palette.ui) : palette.ui;
	root.setAttribute('data-palette', palette.id);
	root.style.setProperty('--accent', ui.accent);
	root.style.setProperty('--accent-strong', ui.accentStrong);
	root.style.setProperty('--accent-soft', ui.accentSoft);
	root.style.setProperty('--accent-line', ui.accentLine);
	root.style.setProperty('--accent-glow', ui.accentGlow);
	// Parchment should keep its grain visible instead of taking on a bright haze.
	const clay = palette.id === 'clay';
	root.style.setProperty('--atlas-haze-a', rgbaCss(palette.shader.c2, clay ? 0.04 : 0.18));
	root.style.setProperty('--atlas-haze-b', rgbaCss(palette.shader.c3, clay ? 0.025 : 0.13));
	root.style.setProperty('--atlas-haze-c', rgbaCss(palette.shader.c4, clay ? 0.015 : 0.10));
}
