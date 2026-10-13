// Saved ("liked") videos, shared by the watch page's Save pill and the
// fullscreen player's heart so both always agree on the current video.
import { get, writable } from 'svelte/store';
import { api, type TidalSearchVideo } from '$lib/api/client';
import { likeSongForVideo, likeSongOnVideoSave } from '$lib/videos/like_song_for_video';
import { showToast } from '$lib/stores/toast';

export const savedVideoIds = writable<Set<number>>(new Set());
export const savingVideo = writable(false);

// A load that started before a toggle must not overwrite the toggle's result.
let changes = 0;

export async function loadSavedVideos(): Promise<void> {
	const changesAtLoad = changes;
	try {
		const { items } = await api.getSavedVideos();
		if (changes === changesAtLoad) savedVideoIds.set(new Set(items.map((item) => item.tidal_id)));
	} catch {
		// Keep whatever we already know; the heart just shows unsaved.
	}
}

// Settings > Library: saving a video can also like its song.
async function likeSongAfterSave(item: TidalSearchVideo) {
	try {
		const title = await likeSongForVideo(item);
		showToast(title ? `Also liked the song "${title}".` : 'No matching song to like on TIDAL.');
	} catch {
		showToast('Could not like the song for this video.', 'error');
	}
}

export async function toggleSavedVideo(item: TidalSearchVideo | null | undefined): Promise<void> {
	if (!item || get(savingVideo)) return;
	savingVideo.set(true);
	const saved = !get(savedVideoIds).has(item.tidal_id);
	try {
		const result = await api.setVideoSaved(item, saved);
		if (!result.ok) throw new Error('Could not save video.');
		changes += 1;
		savedVideoIds.update((ids) => {
			const next = new Set(ids);
			if (saved) next.add(item.tidal_id);
			else next.delete(item.tidal_id);
			return next;
		});
		showToast(saved ? 'Saved to liked videos.' : 'Removed from liked videos.');
		if (saved && get(likeSongOnVideoSave) === 'on') void likeSongAfterSave(item);
	} catch (err) {
		showToast(err instanceof Error ? err.message : 'Could not update liked videos.', 'error');
	} finally {
		savingVideo.set(false);
	}
}
