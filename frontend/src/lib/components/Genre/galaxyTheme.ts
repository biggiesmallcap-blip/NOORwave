import { paletteById, type PaletteId, type Rgb } from '../wallpaper/palettes';
import type { Rgb255 } from './galaxyColor';

export interface GalaxyTheme {
	/** Radial sky stops, center to edge. Always dark: the map is a night sky in every theme. */
	sky: [string, string, string, string];
	haze: [string, string, string];
	band: string;
	/** Neutral, warm and accent star tints as RGB so callers apply their own alpha. */
	starTints: [Rgb255, Rgb255, Rgb255];
	labelText: string;
	labelMuted: string;
	labelChipBg: string;
	vignette: string;
	/** Fill for nodes a mode deliberately mutes (no DSP data, not a rediscover candidate). */
	mutedNode: string;
}

const WHITE: Rgb = [1, 1, 1];
const GREY: Rgb = [0.5, 0.5, 0.5];

const clamp01 = (value: number) => Math.max(0, Math.min(1, value));
const mix = (a: Rgb, b: Rgb, t: number): Rgb => [
	a[0] + (b[0] - a[0]) * t,
	a[1] + (b[1] - a[1]) * t,
	a[2] + (b[2] - a[2]) * t
];
const to255 = (color: Rgb, scale = 1): Rgb255 => [
	Math.round(clamp01(color[0] * scale) * 255),
	Math.round(clamp01(color[1] * scale) * 255),
	Math.round(clamp01(color[2] * scale) * 255)
];
const rgba = (color: Rgb, scale: number, alpha: number) => {
	const [red, green, blue] = to255(color, scale);
	return `rgba(${red}, ${green}, ${blue}, ${alpha})`;
};

/**
 * The galaxy's colours for the active palette. The sky is the palette's mid
 * tones crushed toward black (scale <= 0.2), so it is always night, but this
 * palette's night: warm brown for Clay, violet for Iris, and so on.
 */
export function buildGalaxyTheme(id: PaletteId): GalaxyTheme {
	const { c1, c2, c3, c4 } = paletteById(id).shader;
	const tint = mix(c2, c4, 0.5);
	return {
		sky: [rgba(tint, 0.2, 1), rgba(tint, 0.13, 1), rgba(tint, 0.08, 1), rgba(tint, 0.04, 1)],
		haze: [rgba(c2, 1, 0.16), rgba(c3, 1, 0.12), rgba(c4, 1, 0.1)],
		band: rgba(mix(c1, WHITE, 0.5), 1, 0.06),
		starTints: [to255(WHITE), to255(mix(c1, WHITE, 0.6)), to255(mix(c2, WHITE, 0.6))],
		labelText: 'rgba(250, 246, 240, 0.96)',
		labelMuted: 'rgba(250, 246, 240, 0.7)',
		labelChipBg: rgba(tint, 0.06, 0.86),
		vignette: rgba(tint, 0.03, 0.32),
		mutedNode: rgba(mix(tint, GREY, 0.6), 0.55, 1)
	};
}
