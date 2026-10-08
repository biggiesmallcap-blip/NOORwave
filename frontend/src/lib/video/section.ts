import { writable } from 'svelte/store';

// The video section: four browse tabs plus one watch page, all under
// routes/videos/+layout.svelte. The tabs are for browsing; picking a video on
// any of them opens the watch page, which is the only place the big player
// lives. Leaving it shrinks the player to the corner.

export const WATCH_PATH = '/videos/watch';

export type VideoTab = 'videos' | 'liked' | 'stations' | 'editorial';

export const VIDEO_TABS: readonly { id: VideoTab; href: string; label: string }[] = [
	{ id: 'videos', href: '/videos', label: 'Videos' },
	{ id: 'liked', href: '/videos/liked', label: 'Liked videos' },
	{ id: 'stations', href: '/videos/stations', label: 'Stations' },
	{ id: 'editorial', href: '/videos/editorial', label: 'TIDAL editorial' },
];

export function isVideoSectionPath(pathname: string): boolean {
	return pathname === '/videos' || pathname.startsWith('/videos/');
}

/** The tab a path belongs to; null for the watch page. */
export function videoTabFor(pathname: string): VideoTab | null {
	return VIDEO_TABS.find((tab) => tab.href === pathname)?.id ?? null;
}

/** Deep link to the watch page for one video. Optional fields let a cold open
 *  (reload, copied link) show a title before the stream resolves. */
export function watchUrl(
	videoId: number | string,
	extra: { radio?: boolean; title?: string | null; artistId?: number | null; artistName?: string | null } = {}
): string {
	const params = new URLSearchParams({ videoId: String(videoId) });
	if (extra.radio) params.set('radio', '1');
	if (extra.artistId != null) params.set('artistId', String(extra.artistId));
	if (extra.artistName) params.set('artistName', extra.artistName);
	if (extra.title) params.set('title', extra.title);
	return `${WATCH_PATH}?${params.toString()}`;
}

/** The section's one search field. The Liked tab reads it as a filter over the
 *  likes; every other page treats it as a TIDAL video search. */
export const videoSectionQuery = writable('');
