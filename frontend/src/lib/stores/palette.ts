import { DEFAULT_PALETTE, PALETTES, type PaletteId } from '$lib/components/wallpaper/palettes';
import { createPersistedStore, oneOf } from './persisted';

const STORAGE_KEY = 'noor-palette';
const VALID: PaletteId[] = PALETTES.map((p) => p.id);

export const palette = createPersistedStore<PaletteId>(STORAGE_KEY, DEFAULT_PALETTE, {
	parse: oneOf(VALID),
});

export function setPalette(id: PaletteId) {
	palette.set(id);
}
