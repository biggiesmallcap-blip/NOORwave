import { get } from 'svelte/store';
import { api, type TidalHomeItem, type TidalHomeModule, type TidalSearchVideo } from '$lib/api/client';
import { assertOnline } from '$lib/stores/player';
import { showToast } from '$lib/stores/toast';
import { playVideo, videoSession, type VideoSessionItem } from '$lib/stores/video_session';

// Play paths shared by the video tabs. Each starts playback with a whole shelf,
// mix or playlist as the queue; the video layout then opens the watch page.

function online(): boolean {
	if (assertOnline()) return true;
	showToast('Server is reconnecting.', 'error', 3200);
	return false;
}

function reportFailure(ok: boolean) {
	if (!ok) showToast(get(videoSession).error ?? 'This video could not be loaded.', 'error', 3200);
}

/** Play one video with its shelf queued behind it. */
export async function playFromShelf(
	video: VideoSessionItem,
	queue: VideoSessionItem[],
	label: string,
	options: { autoplay?: boolean; continuous?: boolean; radioScope?: 'artist' | 'library' } = {}
): Promise<void> {
	if (!online()) return;
	const ok = await playVideo(video, {
		queue: queue.length > 0 ? queue : [video],
		source: 'mix',
		sourceLabel: label,
		autoplay: options.autoplay ?? get(videoSession).autoplay,
		continuous: options.continuous ?? false,
		resetRadio: options.continuous ?? false,
		radioScope: options.radioScope ?? 'artist',
	});
	reportFailure(ok);
}

export function editorialItemToVideo(item: TidalHomeItem): TidalSearchVideo {
	return {
		tidal_id: Number(item.id),
		title: item.title,
		duration_ms: item.duration != null ? item.duration * 1000 : null,
		artist_id: item.artist_id ?? null,
		artist_name: item.artist_name ?? null,
		album_tidal_id: item.album_id ?? null,
		artwork_url: item.artwork_url ?? null,
		quality: null,
		explicit: null,
		type: 'Music Video',
	};
}

/** Play a TIDAL mix or video playlist from its first item. */
export async function playVideoCollection(kind: 'mix' | 'playlist', id: string, label?: string | null): Promise<void> {
	if (!online()) return;
	try {
		const items: VideoSessionItem[] = kind === 'mix'
			? (await api.getTidalVideoMixItems(id)).items.map((item) => ({ ...item, mix_id: id }))
			: (await api.getTidalVideoPlaylistItems(id)).items;
		const first = items[0];
		if (!first) {
			showToast(kind === 'mix' ? 'This mix did not return any videos.' : 'This playlist did not return any videos.', 'error', 3200);
			return;
		}
		const ok = await playVideo(first, {
			queue: items,
			source: 'mix',
			sourceLabel: label ?? (kind === 'mix' ? 'Video mix' : 'Video playlist'),
			autoplay: true,
		});
		reportFailure(ok);
	} catch {
		showToast(kind === 'mix' ? 'Video mix could not load.' : 'Playlist videos could not load.', 'error', 3200);
	}
}

/** Claim clicks inside TIDAL editorial shelves on the video tabs. The shelves'
 *  default handling navigates elsewhere; here a video plays with its module
 *  queued behind it and a playlist plays from the top. */
export function playEditorialItem(item: TidalHomeItem, modules: TidalHomeModule[]): boolean {
	if (item.kind === 'video' || item.kind === 'track') {
		const owner = modules.find((m) => m.items.some((i) => i.id === item.id));
		const queue = (owner?.items ?? [item])
			.filter((i) => i.kind === 'video' || i.kind === 'track')
			.map(editorialItemToVideo);
		void playFromShelf(editorialItemToVideo(item), queue, owner?.title ?? "TIDAL's picks");
		return true;
	}
	if (item.kind === 'playlist') {
		void playVideoCollection('playlist', item.id, item.title);
		return true;
	}
	return false;
}
