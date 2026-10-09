import { paletteById, rgbaCss, type PaletteId } from './palettes';

type ThemeRoot = Pick<HTMLElement, 'setAttribute'> & {
	style: Pick<CSSStyleDeclaration, 'setProperty'>;
};

// The light theme's ground (--bg-base in [data-theme="light"]).
const LIGHT_GROUND = '#f2f4f7';
const TEXT_CONTRAST = 4.5;

function channels(hex: string): [number, number, number] {
	return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as [number, number, number];
}

function luminance(rgb: [number, number, number]): number {
	const [r, g, b] = rgb.map((v) => {
		const c = v / 255;
		return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
	});
	return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastRatio(a: string, b: string): number {
	const la = luminance(channels(a));
	const lb = luminance(channels(b));
	return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/**
 * --accent-strong carries accent text and the focus ring. Palette values are
 * tuned for dark grounds, where "strong" means lighter; on a light ground they
 * fall to 1-3:1. Darken the accent toward black until it reads as text there.
 */
export function readableOnLight(hex: string, ground = LIGHT_GROUND): string {
	const base = channels(hex);
	for (let k = 0; k <= 20; k++) {
		const keep = 1 - k * 0.05;
		const mixed = base.map((v) => Math.round(v * keep)) as [number, number, number];
		const out = '#' + mixed.map((v) => v.toString(16).padStart(2, '0')).join('');
		if (contrastRatio(out, ground) >= TEXT_CONTRAST) return out;
	}
	return '#000000';
}

export function applyPaletteTheme(root: ThemeRoot, id: PaletteId, mode: 'dark' | 'light'): void {
	const palette = paletteById(id);
	const ui = mode === 'light' ? (palette.lightUi ?? palette.ui) : palette.ui;
	const accentStrong =
		mode === 'light' && !palette.lightUi ? readableOnLight(ui.accent) : ui.accentStrong;
	root.setAttribute('data-palette', palette.id);
	root.style.setProperty('--accent', ui.accent);
	root.style.setProperty('--accent-strong', accentStrong);
	root.style.setProperty('--accent-soft', ui.accentSoft);
	root.style.setProperty('--accent-line', ui.accentLine);
	root.style.setProperty('--accent-glow', ui.accentGlow);
	// Parchment should keep its grain visible instead of taking on a bright haze;
	// pure black takes none at all.
	const haze = palette.id === 'clay' ? [0.04, 0.025, 0.015] : palette.id === 'void' ? [0, 0, 0] : [0.18, 0.13, 0.10];
	root.style.setProperty('--atlas-haze-a', rgbaCss(palette.shader.c2, haze[0]));
	root.style.setProperty('--atlas-haze-b', rgbaCss(palette.shader.c3, haze[1]));
	root.style.setProperty('--atlas-haze-c', rgbaCss(palette.shader.c4, haze[2]));
}
