import { writable } from 'svelte/store';

// The video section: four modes plus one watch page, all under
// routes/videos/+layout.svelte. On Air leans back (numbered channels), Atlas
// looks across (the map of every artist), Archive finds one thing (lenses over
// songs), Logbook is yours (liked, saved, history, census). Picking a video on
// any of them opens the watch page, the only place the big player lives.
// Leaving it shrinks the player to the corner.

export const WATCH_PATH = '/videos/watch';

export type VideoTab = 'onair' | 'atlas' | 'archive' | 'logbook';

export const VIDEO_TABS: readonly { id: VideoTab; href: string; label: string }[] = [
	{ id: 'onair', href: '/videos', label: 'On Air' },
	{ id: 'atlas', href: '/videos/atlas', label: 'Atlas' },
	{ id: 'archive', href: '/videos/archive', label: 'Archive' },
	{ id: 'logbook', href: '/videos/logbook', label: 'Logbook' },
];

/** Pages that live under a tab without being its landing: they light it. */
const TAB_CHILDREN: Record<string, VideoTab> = {
	'/videos/stations': 'onair',
	'/videos/editorial': 'archive',
	'/videos/liked': 'logbook',
	'/videos/history': 'logbook',
};

export function isVideoSectionPath(pathname: string): boolean {
	return pathname === '/videos' || pathname.startsWith('/videos/');
}

/** The tab a path belongs to; null for the watch page. */
export function videoTabFor(pathname: string): VideoTab | null {
	const path = pathname.length > 1 ? pathname.replace(/\/$/, '') : pathname;
	return VIDEO_TABS.find((tab) => tab.href === path)?.id ?? TAB_CHILDREN[path] ?? null;
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
