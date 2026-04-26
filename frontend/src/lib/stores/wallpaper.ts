import { writable } from 'svelte/store';
import type { WallpaperId } from '$lib/components/wallpaper/shaders';

const STORAGE_KEY = 'noor-wallpaper';
const VALID: WallpaperId[] = ['none', 'aurora', 'chrome', 'grid', 'nebula', 'topo',
                               'topo-noir', 'aurora-deep', 'chrome-brushed',
                               'zen', 'galaxy'];

function readInitial(): WallpaperId {
	if (typeof localStorage === 'undefined') return 'none';
	const raw = localStorage.getItem(STORAGE_KEY);
	return (VALID as string[]).includes(raw ?? '') ? (raw as WallpaperId) : 'none';
}

export const wallpaper = writable<WallpaperId>(readInitial());

export function setWallpaper(id: WallpaperId) {
	wallpaper.set(id);
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(STORAGE_KEY, id);
	}
}
