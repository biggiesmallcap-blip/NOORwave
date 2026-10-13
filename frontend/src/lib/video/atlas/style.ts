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
	uncharted: 'hsla(222, 12%, 66%, 0.34)',
	route: '#ffe6a8',
	trail: 'rgba(255, 236, 196, 0.55)',
} as const;

function hsla(h: number, s: number, l: number, a = 1): string {
	return `hsla(${h.toFixed(1)}, ${s}%, ${l}%, ${a})`;
}

/** Core colour for a point of `light` in a scene of `hue` (null = uncharted). */
export function pointColor(look: AtlasLook, hue: number | null, light: Light): string {
	if (hue === null || light === 0) return STAGE.uncharted;
	if (look === 'place') {
		if (light === 3) return hsla(hue, 92, 80, 1);
		if (light === 2) return hsla(hue, 82, 66, 1);
		return hsla(hue, 70, 60, 0.82);
	}
	if (light === 3) return hsla(hue, 100, 91, 1);
	if (light === 2) return hsla(hue, 86, 64, 0.96);
	return hsla(hue, 20, 64, 0.3);
}

/** Glow colour (the centre stop of a glow sprite); null = no glow. */
export function glowColor(look: AtlasLook, hue: number | null, light: Light): string | null {
	if (hue === null || light === 0) return null;
	if (look === 'place') return hsla(hue, 90, 62, 1);
	if (light === 3) return hsla(hue, 100, 78, 1);
	if (light === 2) return hsla(hue, 90, 60, 1);
	return hsla(hue, 40, 55, 1);
}

/** How strongly a point's glow shows, 0..1. Hubs always glow a little. */
export function glowStrength(look: AtlasLook, light: Light, hub: boolean): number {
	if (light === 0) return 0;
	if (look === 'place') return hub ? 0.55 : 0;
	if (light === 3) return hub ? 0.95 : 0.7;
	if (light === 2) return hub ? 0.5 : 0;
	return hub ? 0.2 : 0;
}

/** Hue-tinted label colour for a scene name. */
export function sceneLabelColor(hue: number, alpha = 1): string {
	return hsla(hue, 70, 84, alpha);
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
	if (light === 3) return 'hsl(40, 100%, 92%)';
	if (light === 2) return hsla(hue, 86, 64, 1);
	return hsla(hue, 18, 52, 1);
}
