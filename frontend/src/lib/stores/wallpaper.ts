import { writable } from 'svelte/store';
import type { WallpaperId } from '$lib/components/wallpaper/shaders';

const STORAGE_KEY = 'noor-wallpaper';
const FPS_STORAGE_KEY = 'noor-wallpaper-fps';
const BLUR_STORAGE_KEY = 'noor-wallpaper-blur';
const REACTIVE_STORAGE_KEY = 'noor-wallpaper-reactive';
const REACTIVITY_STORAGE_KEY = 'noor-wallpaper-reactivity';
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
// Exported so a contract test can assert it stays in sync with WALLPAPERS: any id in
// WALLPAPERS that is missing here would silently reset the user's saved wallpaper.
export const VALID: WallpaperId[] = ['none', 'aurora', 'chrome', 'grid', 'nebula', 'topo',
                               'topo-noir', 'aurora-deep', 'chrome-brushed',
                               'zen', 'galaxy',
                               'blackhole', 'kifs', 'voronoi-glass', 'curl-flow', 'raymarch-lattice',
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
