import { api } from '$lib/api/client';
import type { MenuItem } from '$lib/stores/context_menu';
import { showToast } from '$lib/stores/toast';

export type NotForMeKind = 'track' | 'artist';

/**
 * "Not for me": keeps a track or artist out of every recommendation (automix,
 * radio, Discovery Space, external picks) from now on. Offered once, on queue
 * rows, where suggestions show up; with a known artist it opens a submenu
 * (this song, or anything by the artist).
 */
export function notForMeMenuItem(track: {
	id: number;
	artist_id?: number | null;
	artist_name?: string | null;
}): MenuItem {
	const base = {
		label: 'Not for me',
		// Circled slash, written as an escape to keep the source ASCII.
		icon: '\u2298',
	};
	if (track.artist_id == null || track.artist_id <= 0) {
		return { ...base, hint: 'Never suggest again', onSelect: () => void markNotForMe('track', track.id) };
	}
	const artistId = track.artist_id;
	return {
		...base,
		submenu: [
			{ label: 'This song', onSelect: () => void markNotForMe('track', track.id) },
			{
				label: `Anything by ${track.artist_name || 'this artist'}`,
				onSelect: () => void markNotForMe('artist', artistId),
			},
		],
	};
}

export async function markNotForMe(kind: NotForMeKind, id: number): Promise<void> {
	try {
		await api.setNotForMe(kind, id, true);
		showToast(
			kind === 'track'
				? 'Got it: this track will not be suggested again.'
				: 'Got it: this artist will not be suggested again.'
		);
	} catch {
		showToast('Could not save that. Try again.', 'error');
	}
}
