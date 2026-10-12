import { writable } from 'svelte/store';

export const playlistPicker = writable<{
	title: string;
	getTrackIds: () => Promise<number[]>;
} | null>(null);

export function choosePlaylist(title: string, getTrackIds: () => Promise<number[]>) {
	playlistPicker.set({ title, getTrackIds });
}
