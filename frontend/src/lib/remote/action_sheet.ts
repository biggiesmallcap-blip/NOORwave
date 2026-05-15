import { writable } from 'svelte/store';
import type { MenuItem } from '$lib/stores/context_menu';

/**
 * Remote action sheet — the mobile counterpart to the desktop right-click
 * context menu. Renders the same MenuItem[] shape (so we keep one menu
 * source-of-truth via $lib/player/track_menu) but slides up from the bottom
 * with iOS-style row affordances.
 *
 * Usage:
 *   openActionSheet({ title: track.title, subtitle: track.artist_name, items: buildTrackMenu(track) });
 */
export interface ActionSheetState {
	open: boolean;
	title?: string | null;
	subtitle?: string | null;
	items: MenuItem[];
}

const initial: ActionSheetState = { open: false, title: null, subtitle: null, items: [] };

export const actionSheet = writable<ActionSheetState>(initial);

export function openActionSheet(options: {
	title?: string | null;
	subtitle?: string | null;
	items: MenuItem[];
}) {
	const filtered = options.items.filter((item) => !!item);
	if (filtered.length === 0) return;
	actionSheet.set({
		open: true,
		title: options.title ?? null,
		subtitle: options.subtitle ?? null,
		items: filtered,
	});
}

export function closeActionSheet() {
	actionSheet.update((s) => ({ ...s, open: false }));
}
