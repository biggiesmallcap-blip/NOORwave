import { hexToOklch, oklchToHex, oklchToRgb, type Oklch } from './oklch';
import { paletteById, rgbaCss, type Palette, type PaletteId, type Rgb } from './palettes';

type ThemeRoot = Pick<HTMLElement, 'setAttribute'> & {
	style: Pick<CSSStyleDeclaration, 'setProperty' | 'removeProperty'>;
};

// The stock light theme's ground (--bg-base in [data-theme="light"]).
const LIGHT_GROUND = '#f2f4f7';
const TEXT_CONTRAST = 4.5;
// Accent text and the focus ring on light grounds sit a step deeper than fills.
const STRONG_CONTRAST = 6.5;

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
 * Move a colour's OKLCH lightness away from the ground, holding hue and as much
 * chroma as sRGB allows, until it clears `ratio`. Scaling RGB toward black
 * instead greys bright accents out: lime turns olive, mint turns mud.
 */
export function readableAgainst(hex: string, ground: string, ratio: number): string {
	if (contrastRatio(hex, ground) >= ratio) return hex;
	const color = hexToOklch(hex);
	const lighten = hexToOklch(ground).l < 0.5;
	for (let step = 1; step <= 100; step++) {
		const l = color.l + (lighten ? step : -step) * 0.01;
		if (l < 0 || l > 1) break;
		const out = oklchToHex({ ...color, l });
		if (contrastRatio(out, ground) >= ratio) return out;
	}
	return lighten ? '#ffffff' : '#000000';
}

/** --accent-strong for light grounds when a palette has no light accents of its own. */
export function readableOnLight(hex: string, ground = LIGHT_GROUND): string {
	return readableAgainst(hex, ground, TEXT_CONTRAST);
}

// --- Generated grounds -------------------------------------------------------
// Clay and Pure Black ship hand-made surfaces in app.css. Every other palette
// tints the stock surface ladder with its accent hue; the tint scales with the
// accent's chroma, so grey accents (Mono, Obsidian) keep neutral grounds.

export const GROUND_PROPS = [
	'--bg-base',
	'--bg-elevated',
	'--bg-raised',
	'--bg-surface-strong',
	'--surface-0',
	'--player-surface',
	'--player-queue-surface',
	'--atlas-bg',
	'--ground-top',
	'--ground-bottom',
	'--ground-glow-a',
	'--ground-glow-b',
] as const;

export type Ground = Record<(typeof GROUND_PROPS)[number], string>;

interface Ladder {
	base: number;
	elevated: number;
	raised: number;
	strong: number;
	top: number;
	bottom: number;
	player: number;
	queue: number;
}

// OKLCH lightness steps. DARK matches the stock dark ladder; BLACK is the
// deeper one for the black-ground family; LIGHT matches the stock light ladder.
const DARK: Ladder = { base: 0.152, elevated: 0.181, raised: 0.212, strong: 0.2, top: 0.167, bottom: 0.13, player: 0.175, queue: 0.155 };
const BLACK: Ladder = { base: 0.1, elevated: 0.135, raised: 0.17, strong: 0.15, top: 0.11, bottom: 0.085, player: 0.12, queue: 0.105 };
const LIGHT: Ladder = { base: 0.966, elevated: 0.995, raised: 0.93, strong: 0.99, top: 0.975, bottom: 0.955, player: 0.995, queue: 0.985 };

function shaderHex([r, g, b]: Rgb): string {
	return '#' + [r, g, b].map((v) => Math.round(v * 255).toString(16).padStart(2, '0')).join('');
}

/**
 * Hue and chroma for a generated ground. Dark grounds take the accent hue. A
 * light ground takes the palette's own light wallpaper base when it has one
 * (Futuro's cream, Daylight's sky), since a warm accent alone reads as pink
 * paper; otherwise the accent hue at a whisper.
 */
function tint(palette: Palette, mode: 'dark' | 'light'): { c: number; h: number } {
	const accent = hexToOklch(palette.ui.accent);
	if (mode === 'dark') {
		const c = palette.family === 'dark' ? Math.min(accent.c * 0.06, 0.012) : Math.min(accent.c * 0.1, 0.02);
		return { c, h: accent.h };
	}
	const base = hexToOklch(shaderHex(palette.shader.c1));
	const cap = palette.family === 'light' ? 0.012 : 0.008;
	if (base.l > 0.85 && base.c > 0.005) return { c: Math.min(base.c * 0.5, cap), h: base.h };
	return { c: Math.min(accent.c * 0.04, 0.0065), h: accent.h };
}

function rgba(color: Oklch, alpha: number): string {
	const [r, g, b] = oklchToRgb(color);
	return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/** The generated surface set, or null for palettes whose surfaces live in app.css. */
export function paletteGround(id: PaletteId, mode: 'dark' | 'light'): Ground | null {
	const palette = paletteById(id);
	if (palette.family === 'material') return null;
	const { c, h } = tint(palette, mode);
	const ladder = mode === 'light' ? LIGHT : palette.family === 'dark' ? BLACK : DARK;
	// Near-white steps hold less chroma before they read as coloured paper.
	const at = (l: number, scale = 1) => ({ l, c: c * scale, h });
	const hex = (l: number, scale = 1) => oklchToHex(at(l, scale));
	const light = mode === 'light';
	const base = hex(ladder.base);
	const top = hex(ladder.top, light ? 0.6 : 1);
	const bottom = hex(ladder.bottom);
	const accent = hexToOklch(light ? deriveLightUi(palette, base).accent : palette.ui.accent);
	return {
		'--bg-base': base,
		'--bg-elevated': hex(ladder.elevated, light ? 0.4 : 1),
		'--bg-raised': hex(ladder.raised, 1.1),
		'--bg-surface-strong': hex(ladder.strong, light ? 0.4 : 1.6),
		'--surface-0': base,
		'--player-surface': rgba(at(ladder.player, light ? 0.4 : 1), light ? 0.96 : 0.92),
		'--player-queue-surface': rgba(at(ladder.queue, light ? 0.6 : 1), light ? 0.97 : 0.9),
		'--atlas-bg': `linear-gradient(165deg, ${top} 0%, ${base} 52%, ${bottom} 100%)`,
		'--ground-top': top,
		'--ground-bottom': bottom,
		'--ground-glow-a': rgba(accent, light ? 0.06 : 0.09),
		'--ground-glow-b': rgba(accent, light ? 0.035 : 0.05),
	};
}

// --- Accents -----------------------------------------------------------------

function withAlpha(hex: string, alpha: number): string {
	const [r, g, b] = channels(hex);
	return `rgba(${r}, ${g}, ${b}, ${alpha.toFixed(2)})`;
}

/**
 * Palette accents are tuned for dark grounds. On a light ground the accent is
 * deepened until it reads as a fill (and carries white button text), and the
 * strong accent a step further for text and the focus ring.
 */
function deriveLightUi(palette: Palette, ground: string): Palette['ui'] {
	const accent = readableAgainst(palette.ui.accent, ground, TEXT_CONTRAST);
	return {
		accent,
		accentStrong: readableAgainst(accent, ground, STRONG_CONTRAST),
		accentSoft: withAlpha(accent, 0.1),
		accentLine: withAlpha(accent, 0.24),
		accentGlow: withAlpha(accent, 0.14),
	};
}

/** Text and icons on a solid accent fill: white when it reads, else a deep ink in the accent's hue. */
function inkFor(accent: string): string {
	const white = contrastRatio('#ffffff', accent);
	if (white >= TEXT_CONTRAST) return '#ffffff';
	const { h, c } = hexToOklch(accent);
	const ink = oklchToHex({ l: 0.13, c: Math.min(c * 0.15, 0.02), h });
	return contrastRatio(ink, accent) > white ? ink : '#ffffff';
}

export function applyPaletteTheme(root: ThemeRoot, id: PaletteId, mode: 'dark' | 'light'): void {
	const palette = paletteById(id);
	const ground = paletteGround(id, mode);
	let ui: Palette['ui'];
	if (mode === 'light') {
		ui = palette.lightUi ?? deriveLightUi(palette, ground?.['--bg-base'] ?? LIGHT_GROUND);
	} else {
		const strong = ground ? readableAgainst(palette.ui.accentStrong, ground['--bg-base'], TEXT_CONTRAST) : palette.ui.accentStrong;
		ui = { ...palette.ui, accentStrong: strong };
	}
	root.setAttribute('data-palette', palette.id);
	root.style.setProperty('--accent', ui.accent);
	root.style.setProperty('--accent-strong', ui.accentStrong);
	root.style.setProperty('--accent-soft', ui.accentSoft);
	root.style.setProperty('--accent-line', ui.accentLine);
	root.style.setProperty('--accent-glow', ui.accentGlow);
	root.style.setProperty('--accent-ink', inkFor(ui.accent));
	for (const prop of GROUND_PROPS) {
		if (ground) root.style.setProperty(prop, ground[prop]);
		else root.style.removeProperty(prop);
	}
	// Parchment should keep its grain visible instead of taking on a bright haze;
	// pure black takes none at all.
	const haze = palette.id === 'clay' ? [0.04, 0.025, 0.015] : palette.id === 'void' ? [0, 0, 0] : [0.18, 0.13, 0.10];
	root.style.setProperty('--atlas-haze-a', rgbaCss(palette.shader.c2, haze[0]));
	root.style.setProperty('--atlas-haze-b', rgbaCss(palette.shader.c3, haze[1]));
	root.style.setProperty('--atlas-haze-c', rgbaCss(palette.shader.c4, haze[2]));
}
