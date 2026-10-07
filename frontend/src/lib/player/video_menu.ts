import { goto } from '$app/navigation';
import type { TidalSearchVideo, TidalVideoMix, TidalVideoMixItem } from '$lib/api/client';
import type { MenuItem } from '$lib/stores/context_menu';
import { addVideoToQueue, playQueuedVideo, playVideo, playVideoNext, removeVideoFromQueue } from '$lib/stores/video_session';
import { showToast } from '$lib/stores/toast';
import { isVideoSectionPath, watchUrl } from '$lib/video/section';

type VideoLike = TidalSearchVideo | TidalVideoMix | TidalVideoMixItem;

const SEPARATOR: MenuItem = { separator: true, label: '' };

function copyText(text: string) {
	if (typeof navigator !== 'undefined' && navigator.clipboard) {
		void navigator.clipboard.writeText(text);
	}
}

export function isVideoMix(video: VideoLike): video is TidalVideoMix {
	return 'id' in video && video.type === 'mix';
}

export function videoPageUrl(videoId: number | string): string {
	return watchUrl(videoId);
}

// Search results and mix items satisfy this directly. Artist-page rails add
// their parent artist ID so radio can seed from that artist's graph.
export type VideoMenuSource = Pick<TidalSearchVideo, 'tidal_id'> & Partial<Omit<TidalSearchVideo, 'tidal_id'>>;

export interface VideoMenuOptions {
	inQueue?: boolean;
}

function menuVideo(video: VideoMenuSource): TidalSearchVideo {
	return {
		...video,
		tidal_id: video.tidal_id, title: video.title ?? `TIDAL video ${video.tidal_id}`,
		duration_ms: video.duration_ms ?? null, artist_id: video.artist_id ?? null,
		artist_name: video.artist_name ?? null, album_tidal_id: video.album_tidal_id ?? null,
		artwork_url: video.artwork_url ?? null, quality: video.quality ?? null,
		explicit: video.explicit ?? null, type: video.type ?? 'video',
	};
}

export function buildVideoMenu(video: VideoMenuSource, options: VideoMenuOptions = {}): MenuItem[] {
	const item = menuVideo(video);
	const items: MenuItem[] = [
		{
			label: 'Play video',
			icon: '▶',
			onSelect: () => options.inQueue ? void playQueuedVideo(video.tidal_id) : void goto(videoPageUrl(video.tidal_id)),
		},
		{
			label: options.inQueue ? 'Move next' : 'Play next',
			icon: '⤴',
			onSelect: () => {
				if (playVideoNext(item)) showToast('Video will play next.');
				else showToast('This video is already playing.');
			},
		},
		...(!options.inQueue ? [{
			label: 'Add to queue', icon: '＋',
			onSelect: () => {
				showToast(addVideoToQueue(item) ? 'Added to video queue.' : 'This video is already in the queue.');
			},
		}] : []),
		{
			label: 'Start video radio',
			icon: '◉',
			onSelect: () => {
				// Inside the video section the pick itself opens the watch page;
				// elsewhere, the watch page's deep link starts the radio.
				if (isVideoSectionPath(location.pathname)) {
					void playVideo(item, { queue: [item], source: 'direct', sourceLabel: `${item.artist_name ?? 'Video'} radio`, autoplay: true, continuous: true, resetRadio: true });
					return;
				}
				void goto(watchUrl(video.tidal_id, { radio: true, title: video.title, artistId: video.artist_id, artistName: video.artist_name }));
			},
		},
		{
			label: 'Copy link',
			icon: '⧉',
			onSelect: () => copyText(`${location.origin}/videos?videoId=${video.tidal_id}`),
		},
	];

	if (options.inQueue) {
		items.push(SEPARATOR, {
			label: 'Remove from queue', icon: '×', danger: true,
			onSelect: () => { if (removeVideoFromQueue(video.tidal_id)) showToast('Removed from video queue.'); },
		});
	}

	if (video.artist_id != null) {
		items.push(SEPARATOR);
		items.push({
			label: `Go to ${video.artist_name ?? 'artist'}`,
			icon: '→',
			onSelect: () => void goto(`/tidal/artists/${video.artist_id}`),
		});
	}

	items.push(SEPARATOR);
	items.push({
		label: 'Open video',
		icon: '↗',
		onSelect: () => void goto(videoPageUrl(video.tidal_id)),
	});

	return items;
}

export function buildVideoMixMenu(mix: TidalVideoMix): MenuItem[] {
	return [
		{
			label: 'Play video mix',
			icon: '▶',
			onSelect: () => void goto(`/videos?mixId=${encodeURIComponent(String(mix.id))}&play=1`),
		},
		{
			label: 'Copy link',
			icon: '⧉',
			onSelect: () => copyText(`${location.origin}/videos?mixId=${encodeURIComponent(String(mix.id))}`),
		},
	];
}
