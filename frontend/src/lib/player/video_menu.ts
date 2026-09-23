import { goto } from '$app/navigation';
import type { TidalSearchVideo, TidalVideoMix, TidalVideoMixItem } from '$lib/api/client';
import type { MenuItem } from '$lib/stores/context_menu';
import { playVideo } from '$lib/stores/video_session';

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
	return `/videos?videoId=${videoId}`;
}

// Search results and mix items satisfy this directly. Artist-page rails add
// their parent artist ID so radio can seed from that artist's graph.
export type VideoMenuSource = {
	tidal_id: number;
	title?: string;
	duration_ms?: number | null;
	artwork_url?: string | null;
	artist_id?: number | null;
	artist_name?: string | null;
};

export function buildVideoMenu(video: VideoMenuSource): MenuItem[] {
	const items: MenuItem[] = [
		{
			label: 'Play video',
			icon: '▶',
			onSelect: () => void goto(videoPageUrl(video.tidal_id)),
		},
		{
			label: 'Start video radio',
			icon: '◉',
			onSelect: () => {
				const params = new URLSearchParams({ videoId: String(video.tidal_id), radio: '1' });
				if (video.artist_id != null) params.set('artistId', String(video.artist_id));
				if (video.artist_name) params.set('artistName', video.artist_name);
				if (video.title) params.set('title', video.title);
				if (location.pathname === '/videos') {
					const item: TidalSearchVideo = {
						tidal_id: video.tidal_id, title: video.title ?? `TIDAL video ${video.tidal_id}`,
						duration_ms: video.duration_ms ?? null, artist_id: video.artist_id ?? null,
						artist_name: video.artist_name ?? null, album_tidal_id: null,
						artwork_url: video.artwork_url ?? null, quality: null, explicit: null, type: 'video',
					};
					void playVideo(item, { queue: [item], source: 'direct', sourceLabel: `${item.artist_name ?? 'Video'} radio`, autoplay: true, continuous: true, resetRadio: true });
					return;
				}
				void goto(`/videos?${params.toString()}`);
			},
		},
		{
			label: 'Copy link',
			icon: '⧉',
			onSelect: () => copyText(`${location.origin}/videos?videoId=${video.tidal_id}`),
		},
	];

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
