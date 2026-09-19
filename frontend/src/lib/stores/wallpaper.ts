import { writable } from 'svelte/store';
import type { WallpaperId } from '$lib/components/wallpaper/shaders';
import { createPersistedStore, oneOf } from './persisted';

const STORAGE_KEY = 'noor-wallpaper';
const FPS_STORAGE_KEY = 'noor-wallpaper-fps';
const BLUR_STORAGE_KEY = 'noor-wallpaper-blur';
const REACTIVE_STORAGE_KEY = 'noor-wallpaper-reactive';
const REACTIVITY_STORAGE_KEY = 'noor-wallpaper-reactivity';
const SMOOTHING_STORAGE_KEY = 'noor-wallpaper-smoothing';
const REDUCE_MOTION_STORAGE_KEY = 'noor-wallpaper-reduce-motion';
const COLOR_SOURCE_STORAGE_KEY = 'noor-wallpaper-color-source';
const QUALITY_STORAGE_KEY = 'noor-wallpaper-quality';
const IDLE_STORAGE_KEY = 'noor-wallpaper-idle';
export const WALLPAPER_FPS_MIN = 24;
export const WALLPAPER_FPS_MAX = 60;
// 30 by default: at 4K the wallpaper raster dominates the app's GPU cost and
// 60fps doubles it for ambient motion most shaders don't need. 60 stays opt-in.
export const WALLPAPER_FPS_DEFAULT = 30;
export const WALLPAPER_BLUR_MIN = 0;
export const WALLPAPER_BLUR_MAX = 18;
export const WALLPAPER_BLUR_DEFAULT = 7;
// Beat-reactivity strength as a percentage. 100 = the tuned default; 0 mutes the
// music influence entirely (the reactive shaders fall back to their idle motion).
export const WALLPAPER_REACTIVITY_MIN = 0;
export const WALLPAPER_REACTIVITY_MAX = 200;
export const WALLPAPER_REACTIVITY_DEFAULT = 100;
// Beat envelope shape as a percentage: 0 = snappy (sharp attack, fast decay),
// 100 = floaty (smooth swell). See ShaderWallpaper's u_pulse.
export const WALLPAPER_SMOOTHING_MIN = 0;
export const WALLPAPER_SMOOTHING_MAX = 100;
export const WALLPAPER_SMOOTHING_DEFAULT = 40;

export type WallpaperReduceMotion = 'auto' | 'on' | 'off';
export type WallpaperColorSource = 'palette' | 'art';
export type WallpaperQuality = 'standard' | 'high';
export type WallpaperIdle = 'drift' | 'frozen' | 'demo';
// Exported so a contract test can assert it stays in sync with WALLPAPERS: any id in
// WALLPAPERS that is missing here would silently reset the user's saved wallpaper.
export const VALID: WallpaperId[] = ['none', 'aurora', 'chrome', 'grid', 'nebula', 'topo',
                               'topo-noir', 'aurora-deep', 'chrome-brushed',
                               'zen', 'galaxy',
                               'blackhole', 'kifs', 'voronoi-glass', 'curl-flow', 'raymarch-lattice',
                               'dj', 'analyzer', 'scope-ring', 'synthwave', 'kaleido-beat',
                               'pulse', 'eq-react', 'beat-tunnel',
                               'bass-bloom', 'starfield-warp', 'radial-eq',
                               'joy-division', 'oscilloscope', 'spectrum', 'vinyl', 'tape',
                               'phasing', 'spectrogram', 'lissajous', 'drone', 'reel',
                               'standing-wave',
                               'pattern-grid', 'pattern-dots', 'pattern-hatch',
                               'pattern-truchet', 'pattern-waves', 'pattern-noise',
                               'pattern-plasma', 'pattern-kaleido', 'pattern-tunnel',
                               'pattern-melt', 'pattern-speed', 'pattern-vortex',
                               'pattern-shards', 'pattern-vector'];

// Matches the shader forced during the /onboarding route, so a fresh install
// keeps the wallpaper the user saw on first launch.
const DEFAULT: WallpaperId = 'standing-wave';

function clampSetting(value: number, min: number, max: number): number {
	return Math.min(max, Math.max(min, Math.round(value)));
}

function numberParser(min: number, max: number): (raw: string) => number | undefined {
	return (raw) => {
		if (raw.trim() === '') return undefined;
		const num = Number(raw);
		return Number.isFinite(num) ? clampSetting(num, min, max) : undefined;
	};
}

const numberOptions = (min: number, max: number) => ({
	parse: numberParser(min, max),
	serialize: String,
});

export const wallpaper = createPersistedStore<WallpaperId>(STORAGE_KEY, DEFAULT, {
	parse: oneOf(VALID),
});
export const wallpaperFps = createPersistedStore(
	FPS_STORAGE_KEY,
	WALLPAPER_FPS_DEFAULT,
	numberOptions(WALLPAPER_FPS_MIN, WALLPAPER_FPS_MAX),
);
export const wallpaperBlur = createPersistedStore(
	BLUR_STORAGE_KEY,
	WALLPAPER_BLUR_DEFAULT,
	numberOptions(WALLPAPER_BLUR_MIN, WALLPAPER_BLUR_MAX),
);
// Whether the playing track drives the beat-reactive shaders at all.
export const wallpaperReactive = createPersistedStore(REACTIVE_STORAGE_KEY, true, {
	parse: (raw) => raw === '1' || raw === 'true',
	serialize: (on) => (on ? '1' : '0'),
});
// Strength of that reaction, as a percentage (see WALLPAPER_REACTIVITY_*).
export const wallpaperReactivity = createPersistedStore(
	REACTIVITY_STORAGE_KEY,
	WALLPAPER_REACTIVITY_DEFAULT,
	numberOptions(WALLPAPER_REACTIVITY_MIN, WALLPAPER_REACTIVITY_MAX),
);
// Beat envelope shape (snappy..floaty), as a percentage.
export const wallpaperBeatSmoothing = createPersistedStore(
	SMOOTHING_STORAGE_KEY,
	WALLPAPER_SMOOTHING_DEFAULT,
	numberOptions(WALLPAPER_SMOOTHING_MIN, WALLPAPER_SMOOTHING_MAX),
);
// 'auto' follows the OS prefers-reduced-motion; 'on'/'off' force it. When active,
// the renderer clamps beat/energy amplitude to a calm cap (accessibility + battery).
export const wallpaperReduceMotion = createPersistedStore<WallpaperReduceMotion>(REDUCE_MOTION_STORAGE_KEY, 'auto', {
	parse: oneOf(['auto', 'on', 'off'] as const),
});
// Where the reactive shaders get their colours: the fixed palette, or colours
// pulled from the playing track's cover art (falls back to palette on failure).
export const wallpaperColorSource = createPersistedStore<WallpaperColorSource>(COLOR_SOURCE_STORAGE_KEY, 'palette', {
	parse: oneOf(['palette', 'art'] as const),
});
// Render scale: 'standard' caps device-pixel-ratio at 1; 'high' allows 2 for a
// crisper (but heavier) background on capable GPUs.
export const wallpaperQuality = createPersistedStore<WallpaperQuality>(QUALITY_STORAGE_KEY, 'standard', {
	parse: oneOf(['standard', 'high'] as const),
});
// What the reactive shaders do when nothing is playing.
export const wallpaperIdle = createPersistedStore<WallpaperIdle>(IDLE_STORAGE_KEY, 'drift', {
	parse: oneOf(['drift', 'frozen', 'demo'] as const),
});

// Effective reduce-motion state: resolves 'auto' against the live media query so
// the renderer can just read a boolean. Updated on setting change and on OS change.
export const wallpaperReduceMotionActive = writable<boolean>(false);
if (typeof window !== 'undefined' && window.matchMedia) {
	const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
	let mode: WallpaperReduceMotion = 'auto';
	const recompute = () => {
		wallpaperReduceMotionActive.set(mode === 'on' || (mode === 'auto' && mq.matches));
	};
	wallpaperReduceMotion.subscribe((v) => {
		mode = v;
		recompute();
	});
	// Safari <14 uses addListener; modern browsers use addEventListener.
	if (mq.addEventListener) mq.addEventListener('change', recompute);
	else if (mq.addListener) mq.addListener(recompute);
}

if (typeof document !== 'undefined') {
	wallpaperBlur.subscribe((value) => {
		document.documentElement.style.setProperty('--wallpaper-blur', `${value}px`);
		document.documentElement.style.setProperty('--wallpaper-scale', (1 + value * 0.0025).toFixed(3));
	});
}

export function setWallpaper(id: WallpaperId) {
	wallpaper.set(id);
}

export function setWallpaperFps(value: number) {
	const next = clampSetting(value, WALLPAPER_FPS_MIN, WALLPAPER_FPS_MAX);
	wallpaperFps.set(next);
}

export function setWallpaperBlur(value: number) {
	const next = clampSetting(value, WALLPAPER_BLUR_MIN, WALLPAPER_BLUR_MAX);
	wallpaperBlur.set(next);
}

export function setWallpaperReactive(on: boolean) {
	wallpaperReactive.set(on);
}

export function setWallpaperReactivity(value: number) {
	const next = clampSetting(value, WALLPAPER_REACTIVITY_MIN, WALLPAPER_REACTIVITY_MAX);
	wallpaperReactivity.set(next);
}

export function setWallpaperBeatSmoothing(value: number) {
	const next = clampSetting(value, WALLPAPER_SMOOTHING_MIN, WALLPAPER_SMOOTHING_MAX);
	wallpaperBeatSmoothing.set(next);
}

export function setWallpaperReduceMotion(value: WallpaperReduceMotion) {
	wallpaperReduceMotion.set(value);
}

export function setWallpaperColorSource(value: WallpaperColorSource) {
	wallpaperColorSource.set(value);
}

export function setWallpaperQuality(value: WallpaperQuality) {
	wallpaperQuality.set(value);
}

export function setWallpaperIdle(value: WallpaperIdle) {
	wallpaperIdle.set(value);
}
