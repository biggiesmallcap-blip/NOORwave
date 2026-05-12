import { writable } from 'svelte/store';
import type { WallpaperId } from '$lib/components/wallpaper/shaders';

const STORAGE_KEY = 'noor-wallpaper';
const VALID: WallpaperId[] = ['none', 'aurora', 'chrome', 'grid', 'nebula', 'topo',
                               'topo-noir', 'aurora-deep', 'chrome-brushed',
                               'zen', 'galaxy',
                               'joy-division', 'oscilloscope', 'spectrum', 'vinyl', 'tape',
                               'phasing', 'spectrogram', 'lissajous', 'drone', 'reel',
                               'standing-wave',
                               'pattern-grid', 'pattern-dots', 'pattern-hatch',
                               'pattern-truchet', 'pattern-waves', 'pattern-noise',
                               'pattern-plasma', 'pattern-kaleido', 'pattern-tunnel',
                               'pattern-melt', 'pattern-speed', 'pattern-vortex',
                               'pattern-shards', 'pattern-vector'];

const DEFAULT: WallpaperId = 'pattern-speed';

function readInitial(): WallpaperId {
	if (typeof localStorage === 'undefined') return DEFAULT;
	const raw = localStorage.getItem(STORAGE_KEY);
	return (VALID as string[]).includes(raw ?? '') ? (raw as WallpaperId) : DEFAULT;
}

export const wallpaper = writable<WallpaperId>(readInitial());

export function setWallpaper(id: WallpaperId) {
	wallpaper.set(id);
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(STORAGE_KEY, id);
	}
}
