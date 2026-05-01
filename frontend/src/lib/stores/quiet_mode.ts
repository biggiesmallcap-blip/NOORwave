import { writable } from 'svelte/store';

export const quietModeOpen = writable(false);

export function openQuietMode() {
	quietModeOpen.set(true);
}

export function closeQuietMode() {
	quietModeOpen.set(false);
}

export function toggleQuietMode() {
	quietModeOpen.update((v) => !v);
}
