// The Atlas's visual language in one place: the two looks, their point
// colours, point sizes, and the stage palette. The stage is a night sky in
// every app theme (like a video stage), so these colours are not theme
// tokens; the chrome around the stage still follows the theme.
//
// Each look encodes something position does not:
// - Picture: every artist in the dominant colour of its own video artwork.
// - You: your watching as footprints that fade with time, and the frontier
//   (unwatched artists right next to the ones you watch).

import type { Light } from '$lib/api/video_explore_types';

export type AtlasLook = 'picture' | 'you';

export const ATLAS_LOOKS: readonly { value: AtlasLook; label: string; help: string }[] = [
	{ value: 'picture', label: 'Picture', help: 'Every artist in the colour of its own videos' },
	{ value: 'you', label: 'You', help: 'Where you have been, and the frontier next to it' },
];

/** A stored look, including the retired ones ('light' was about you,
 *  'place' about colour). */
export function parseLook(raw: unknown): AtlasLook {
	return raw === 'you' || raw === 'light' ? 'you' : 'picture';
}

export const LIGHT_NAMES: Record<Light, string> = {
	0: 'Uncharted',
	1: 'Dim',
	2: 'Warm',
	3: 'Lit',
};

export const LIGHT_HELP: Record<Light, string> = {
	0: 'Not linked to the graph yet',
	1: 'Charted, not explored yet',
	2: 'In your library, or one step from it',
	3: 'Watched, liked or saved',
};

/** Stage colours (dark in every theme). */
export const STAGE = {
	top: '#070912',
	mid: '#05060c',
	edge: '#020306',
	grid: 'rgba(150, 170, 220, 0.055)',
	gridStrong: 'rgba(150, 170, 220, 0.09)',
	ring: 'rgba(170, 180, 205, 0.16)',
	ringText: 'rgba(170, 180, 205, 0.42)',
	text: 'rgba(236, 239, 247, 0.92)',
	textDim: 'rgba(200, 206, 222, 0.62)',
	textShadow: 'rgba(3, 4, 9, 0.82)',
	/** A thin dark halo behind names, enough to read them over stars. */
	textHalo: 'rgba(4, 5, 10, 0.55)',
	/** Artist names: brighter, with a fuller halo, so small ones still read. */
	label: 'rgba(242, 244, 251, 0.97)',
	labelHalo: 'rgba(4, 5, 10, 0.88)',
	uncharted: 'hsla(222, 12%, 66%, 0.34)',
	route: '#ffe6a8',
	trail: 'rgba(255, 236, 196, 0.55)',
} as const;

function hsla(h: number, s: number, l: number, a = 1): string {
	return `hsla(${h.toFixed(1)}, ${s}%, ${l}%, ${a})`;
}

/** The warm white every lit artist shares: "known" reads the same in every scene. */
export const LIT_COLOR = 'hsla(40, 62%, 90%, 1)';

/** Scene name colour on the map: near white. (A scene's hue is its
 *  identity colour elsewhere, on its channel and Archive chip; on the map
 *  it would only repeat where the scene is.) */
export function sceneLabelColor(_hue: number, alpha = 1): string {
	return hsla(222, 14, 88, alpha);
}

// ------------------------------------------------------------- Picture

/** RGB (0xRRGGBB) to HSL with h in degrees, s and l in 0..1. */
export function rgbToHsl(rgb: number): { h: number; s: number; l: number } {
	const r = ((rgb >> 16) & 255) / 255;
	const g = ((rgb >> 8) & 255) / 255;
	const b = (rgb & 255) / 255;
	const max = Math.max(r, g, b);
	const min = Math.min(r, g, b);
	const l = (max + min) / 2;
	const d = max - min;
	if (d < 1e-6) return { h: 0, s: 0, l };
	const s = d / (1 - Math.abs(2 * l - 1));
	let h: number;
	if (max === r) h = ((g - b) / d) % 6;
	else if (max === g) h = (b - r) / d + 2;
	else h = (r - g) / d + 4;
	return { h: (h * 60 + 360) % 360, s: Math.min(1, s), l };
}

const PICTURE_HUES = 24;
const PICTURE_GREYS = 3;
/** Colour groups points are drawn in: 24 hues x 2 saturations x 2
 *  lightnesses, 3 greys, and one for "not read yet". */
export const PICTURE_BUCKETS = PICTURE_HUES * 4 + PICTURE_GREYS + 1;
export const PICTURE_UNKNOWN = PICTURE_BUCKETS - 1;

/** The colour group of an artwork colour. Near-greys (black-and-white
 *  videos) keep their grey; colours keep their hue to 15 degrees and a
 *  muted or vivid, deep or light step. */
export function pictureBucket(rgb: number): number {
	const { h, s, l } = rgbToHsl(rgb);
	if (s < 0.14 || l < 0.06 || l > 0.97) {
		return PICTURE_HUES * 4 + (l < 0.38 ? 0 : l < 0.66 ? 1 : 2);
	}
	const hue = Math.floor(((h + 7.5) % 360) / (360 / PICTURE_HUES));
	const vivid = s >= 0.45 ? 1 : 0;
	const light = l >= 0.5 ? 1 : 0;
	return hue * 4 + vivid * 2 + light;
}

/**
 * A colour group as it is drawn on the night stage: the hue kept, the
 * saturation tamed and the lightness lifted into a band where every artist
 * reads, so a dark, moody video is still a visible point, just a deeper one.
 */
export function pictureColor(bucket: number, alpha = 0.92): string {
	if (bucket === PICTURE_UNKNOWN) return 'hsla(222, 10%, 60%, 0.26)';
	if (bucket >= PICTURE_HUES * 4) {
		const step = bucket - PICTURE_HUES * 4;
		return hsla(220, 6, [52, 66, 82][step], alpha * 0.85);
	}
	const hue = Math.floor(bucket / 4) * (360 / PICTURE_HUES);
	const vivid = (bucket >> 1) & 1;
	const light = bucket & 1;
	return hsla(hue, vivid ? 64 : 34, light ? 74 : 60, alpha);
}

/** The nebula's stamp colour for a group: the same hue, softer. */
export function pictureNebulaColor(bucket: number): { h: number; s: number; l: number } | null {
	if (bucket === PICTURE_UNKNOWN) return null;
	if (bucket >= PICTURE_HUES * 4) return { h: 220, s: 6, l: 50 };
	const vivid = (bucket >> 1) & 1;
	return { h: Math.floor(bucket / 4) * (360 / PICTURE_HUES), s: vivid ? 46 : 24, l: 52 };
}

// ----------------------------------------------------------------- You

/** Footprint steps by how long ago: this week, this month, earlier. */
export const FOOTPRINT_WEEK = 7;
export const FOOTPRINT_MONTH = 30;

export function footprintStep(ageDays: number): 0 | 1 | 2 {
	return ageDays <= FOOTPRINT_WEEK ? 0 : ageDays <= FOOTPRINT_MONTH ? 1 : 2;
}

/** Warm, brightest where you were this week, fading with time. */
export const FOOTPRINT_COLORS = ['hsla(38, 92%, 82%, 1)', 'hsla(36, 70%, 72%, 0.88)', 'hsla(32, 42%, 64%, 0.62)'] as const;
export const FOOTPRINT_GLOW = 'hsla(38, 90%, 70%, 1)';

/** Frontier steps by score (edge strength times how fresh the footprint is). */
export function frontierStep(score: number): 0 | 1 | 2 {
	return score >= 0.45 ? 0 : score >= 0.2 ? 1 : 2;
}

/** Cool, so the edge of your world reads apart from where you have been. */
export const FRONTIER_COLORS = ['hsla(186, 78%, 72%, 0.98)', 'hsla(186, 58%, 66%, 0.78)', 'hsla(188, 40%, 60%, 0.56)'] as const;
export const FRONTIER_GLOW = 'hsla(186, 80%, 62%, 1)';

/** Everything else in You: present, but stepping back. */
export const YOU_REST = 'hsla(222, 10%, 62%, 0.2)';

/** A trail's opacity: half every two weeks. */
export function trailAlpha(ageDays: number): number {
	return 0.62 * Math.pow(0.5, Math.max(0, ageDays) / 14);
}

/** Point radius in CSS px at fit zoom: area grows with videos. */
export function baseRadius(videos: number): number {
	return 0.55 + 0.36 * Math.sqrt(Math.min(Math.max(videos, 1), 600));
}

/** Points grow gently as you zoom in, so clusters open up without turning into blobs. */
export function zoomGrowth(k: number): number {
	return Math.min(3.4, Math.max(0.7, Math.pow(Math.max(k, 0.01), 0.5)));
}

/** A hub: one of the biggest few artists in its scene, or simply large. */
export function isHub(rankInScene: number, videos: number): boolean {
	return rankInScene < 6 || videos >= 80;
}

/** Number formatting for the HUD and cards. */
export function formatCount(n: number): string {
	return Math.round(n).toLocaleString('en-US');
}

export function formatHours(hours: number): string {
	if (hours >= 100) return `${formatCount(hours)} h`;
	if (hours >= 10) return `${hours.toFixed(0)} h`;
	return `${hours.toFixed(1)} h`;
}

/** Swatch for a light state in cards and the legend. */
export function lightSwatch(light: Light, hue: number | null): string {
	if (light === 0 || hue === null) return 'hsl(222, 10%, 58%)';
	if (light === 3) return LIT_COLOR;
	if (light === 2) return hsla(hue, 40, 66, 1);
	return hsla(hue, 12, 52, 1);
}
