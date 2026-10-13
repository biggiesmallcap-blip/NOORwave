// The Atlas's visual language in one place: the two looks, point colours per
// scene hue and light, point sizes, and the stage palette. The stage is a
// night sky in every app theme (like a video stage), so these colours are not
// theme tokens; the chrome around the stage still follows the theme.

import type { Light } from '$lib/api/video_explore_types';

/** "Light is knowledge" or "Color is place". */
export type AtlasLook = 'light' | 'place';

export const ATLAS_LOOKS: readonly { value: AtlasLook; label: string }[] = [
	{ value: 'light', label: 'Light is knowledge' },
	{ value: 'place', label: 'Color is place' },
];

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
	uncharted: 'hsla(222, 12%, 66%, 0.34)',
	route: '#ffe6a8',
	trail: 'rgba(255, 236, 196, 0.55)',
} as const;

function hsla(h: number, s: number, l: number, a = 1): string {
	return `hsla(${h.toFixed(1)}, ${s}%, ${l}%, ${a})`;
}

/** The warm white every lit artist shares: "known" reads the same in every scene. */
export const LIT_COLOR = 'hsla(40, 62%, 90%, 1)';

/**
 * Core colour for a point of `light` in a scene of `hue` (null = uncharted).
 * Restrained on purpose: hue says where, saturation stays low enough that no
 * scene shouts, and light (in the knowledge look) is the one bright accent.
 */
export function pointColor(look: AtlasLook, hue: number | null, light: Light): string {
	if (hue === null || light === 0) return STAGE.uncharted;
	if (look === 'place') {
		if (light === 3) return hsla(hue, 46, 80, 1);
		if (light === 2) return hsla(hue, 42, 68, 0.95);
		return hsla(hue, 34, 60, 0.78);
	}
	if (light === 3) return LIT_COLOR;
	if (light === 2) return hsla(hue, 40, 66, 0.88);
	return hsla(hue, 12, 60, 0.3);
}

/** Glow colour (the centre stop of a glow sprite); null = no glow. Only lit
 *  artists glow, and only in the knowledge look. */
export function glowColor(look: AtlasLook, hue: number | null, light: Light): string | null {
	if (hue === null || light !== 3 || look !== 'light') return null;
	return 'hsla(40, 70%, 78%, 1)';
}

/** How strongly a point's glow shows, 0..1: a soft halo on lit hubs only. */
export function glowStrength(look: AtlasLook, light: Light, hub: boolean): number {
	if (look !== 'light' || light !== 3) return 0;
	return hub ? 0.32 : 0;
}

/** Scene name colour: near white with a breath of the scene's hue. */
export function sceneLabelColor(hue: number, alpha = 1): string {
	return hsla(hue, 22, 84, alpha);
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
