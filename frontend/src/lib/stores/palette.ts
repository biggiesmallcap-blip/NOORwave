import { writable } from 'svelte/store';
import { DEFAULT_PALETTE, PALETTES, type PaletteId } from '$lib/components/wallpaper/palettes';

const STORAGE_KEY = 'noor-palette';
const VALID: PaletteId[] = PALETTES.map((p) => p.id);

function readInitial(): PaletteId {
	if (typeof localStorage === 'undefined') return DEFAULT_PALETTE;
	const raw = localStorage.getItem(STORAGE_KEY);
	return (VALID as string[]).includes(raw ?? '') ? (raw as PaletteId) : DEFAULT_PALETTE;
}

export const palette = writable<PaletteId>(readInitial());

export function setPalette(id: PaletteId) {
	palette.set(id);
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(STORAGE_KEY, id);
	}
}
