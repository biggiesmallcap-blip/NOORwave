import { goto } from '$app/navigation';
import type { TidalSearchVideo, TidalVideoMix, TidalVideoMixItem } from '$lib/api/client';
import type { MenuItem } from '$lib/stores/context_menu';

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

// Structural parameter: search results, mix items, and artist-page video
// rails all satisfy this without adapter objects (the artist rail's
// TidalArtistVideo has no artist_id, so its menu simply omits the
// go-to-artist entry).
export type VideoMenuSource = {
	tidal_id: number;
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
