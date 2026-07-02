import { writable } from 'svelte/store';
import type { WallpaperId } from '$lib/components/wallpaper/shaders';

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
export const WALLPAPER_FPS_DEFAULT = 60;
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

function readInitial(): WallpaperId {
	if (typeof localStorage === 'undefined') return DEFAULT;
	const raw = localStorage.getItem(STORAGE_KEY);
	return (VALID as string[]).includes(raw ?? '') ? (raw as WallpaperId) : DEFAULT;
}

function clampSetting(value: number, min: number, max: number): number {
	return Math.min(max, Math.max(min, Math.round(value)));
}

function readNumberSetting(key: string, fallback: number, min: number, max: number): number {
	if (typeof localStorage === 'undefined') return fallback;
	const raw = Number(localStorage.getItem(key));
	return Number.isFinite(raw) ? clampSetting(raw, min, max) : fallback;
}

function readBoolSetting(key: string, fallback: boolean): boolean {
	if (typeof localStorage === 'undefined') return fallback;
	const raw = localStorage.getItem(key);
	if (raw === null) return fallback;
	return raw === '1' || raw === 'true';
}

function readEnumSetting<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
	if (typeof localStorage === 'undefined') return fallback;
	const raw = localStorage.getItem(key);
	return (allowed as readonly string[]).includes(raw ?? '') ? (raw as T) : fallback;
}

function writeStringSetting(key: string, value: string) {
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(key, value);
	}
}

function writeNumberSetting(key: string, value: number) {
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(key, String(value));
	}
}

export const wallpaper = writable<WallpaperId>(readInitial());
export const wallpaperFps = writable<number>(
	readNumberSetting(FPS_STORAGE_KEY, WALLPAPER_FPS_DEFAULT, WALLPAPER_FPS_MIN, WALLPAPER_FPS_MAX)
);
export const wallpaperBlur = writable<number>(
	readNumberSetting(BLUR_STORAGE_KEY, WALLPAPER_BLUR_DEFAULT, WALLPAPER_BLUR_MIN, WALLPAPER_BLUR_MAX)
);
// Whether the playing track drives the beat-reactive shaders at all.
export const wallpaperReactive = writable<boolean>(readBoolSetting(REACTIVE_STORAGE_KEY, true));
// Strength of that reaction, as a percentage (see WALLPAPER_REACTIVITY_*).
export const wallpaperReactivity = writable<number>(
	readNumberSetting(
		REACTIVITY_STORAGE_KEY,
		WALLPAPER_REACTIVITY_DEFAULT,
		WALLPAPER_REACTIVITY_MIN,
		WALLPAPER_REACTIVITY_MAX
	)
);
// Beat envelope shape (snappy..floaty), as a percentage.
export const wallpaperBeatSmoothing = writable<number>(
	readNumberSetting(
		SMOOTHING_STORAGE_KEY,
		WALLPAPER_SMOOTHING_DEFAULT,
		WALLPAPER_SMOOTHING_MIN,
		WALLPAPER_SMOOTHING_MAX
	)
);
// 'auto' follows the OS prefers-reduced-motion; 'on'/'off' force it. When active,
// the renderer clamps beat/energy amplitude to a calm cap (accessibility + battery).
export const wallpaperReduceMotion = writable<WallpaperReduceMotion>(
	readEnumSetting<WallpaperReduceMotion>(REDUCE_MOTION_STORAGE_KEY, ['auto', 'on', 'off'], 'auto')
);
// Where the reactive shaders get their colours: the fixed palette, or colours
// pulled from the playing track's cover art (falls back to palette on failure).
export const wallpaperColorSource = writable<WallpaperColorSource>(
	readEnumSetting<WallpaperColorSource>(COLOR_SOURCE_STORAGE_KEY, ['palette', 'art'], 'palette')
);
// Render scale: 'standard' caps device-pixel-ratio at 1; 'high' allows 2 for a
// crisper (but heavier) background on capable GPUs.
export const wallpaperQuality = writable<WallpaperQuality>(
	readEnumSetting<WallpaperQuality>(QUALITY_STORAGE_KEY, ['standard', 'high'], 'standard')
);
// What the reactive shaders do when nothing is playing.
export const wallpaperIdle = writable<WallpaperIdle>(
	readEnumSetting<WallpaperIdle>(IDLE_STORAGE_KEY, ['drift', 'frozen', 'demo'], 'drift')
);

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
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(STORAGE_KEY, id);
	}
}

export function setWallpaperFps(value: number) {
	const next = clampSetting(value, WALLPAPER_FPS_MIN, WALLPAPER_FPS_MAX);
	wallpaperFps.set(next);
	writeNumberSetting(FPS_STORAGE_KEY, next);
}

export function setWallpaperBlur(value: number) {
	const next = clampSetting(value, WALLPAPER_BLUR_MIN, WALLPAPER_BLUR_MAX);
	wallpaperBlur.set(next);
	writeNumberSetting(BLUR_STORAGE_KEY, next);
}

export function setWallpaperReactive(on: boolean) {
	wallpaperReactive.set(on);
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(REACTIVE_STORAGE_KEY, on ? '1' : '0');
	}
}

export function setWallpaperReactivity(value: number) {
	const next = clampSetting(value, WALLPAPER_REACTIVITY_MIN, WALLPAPER_REACTIVITY_MAX);
	wallpaperReactivity.set(next);
	writeNumberSetting(REACTIVITY_STORAGE_KEY, next);
}

export function setWallpaperBeatSmoothing(value: number) {
	const next = clampSetting(value, WALLPAPER_SMOOTHING_MIN, WALLPAPER_SMOOTHING_MAX);
	wallpaperBeatSmoothing.set(next);
	writeNumberSetting(SMOOTHING_STORAGE_KEY, next);
}

export function setWallpaperReduceMotion(value: WallpaperReduceMotion) {
	wallpaperReduceMotion.set(value);
	writeStringSetting(REDUCE_MOTION_STORAGE_KEY, value);
}

export function setWallpaperColorSource(value: WallpaperColorSource) {
	wallpaperColorSource.set(value);
	writeStringSetting(COLOR_SOURCE_STORAGE_KEY, value);
}

export function setWallpaperQuality(value: WallpaperQuality) {
	wallpaperQuality.set(value);
	writeStringSetting(QUALITY_STORAGE_KEY, value);
}

export function setWallpaperIdle(value: WallpaperIdle) {
	wallpaperIdle.set(value);
	writeStringSetting(IDLE_STORAGE_KEY, value);
}
