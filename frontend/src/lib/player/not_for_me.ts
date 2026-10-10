import { api } from '$lib/api/client';
import type { MenuItem } from '$lib/stores/context_menu';
import { showToast } from '$lib/stores/toast';

export type NotForMeKind = 'track' | 'artist';

/**
 * "Not for me": keeps a track or artist out of every recommendation (automix,
 * radio, external picks) from now on.
 */
export function notForMeMenuItem(kind: NotForMeKind, id: number, label: string): MenuItem {
	return {
		label,
		// Circled slash, written as an escape to keep the source ASCII.
		icon: '\u2298',
		hint: 'Never suggest again',
		onSelect: () => void markNotForMe(kind, id),
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
