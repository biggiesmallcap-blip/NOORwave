import type { MenuItem } from '$lib/stores/context_menu';

/** Mixes and unimported playlists have no local playlist id yet. */
export function buildTidalCollectionMenu(options: {
	onOpen?: () => void;
	onPlay?: () => void;
	onShuffle?: () => void;
	onSave?: () => void;
}): MenuItem[] {
	const items: MenuItem[] = [];
	if (options.onOpen) items.push({ label: 'Open track list', icon: '↗', onSelect: options.onOpen });
	if (options.onPlay) items.push({ label: 'Play', icon: '▶', onSelect: options.onPlay });
	if (options.onShuffle) items.push({ label: 'Shuffle', icon: '⤨', onSelect: options.onShuffle });
	if (options.onSave) items.push({ label: 'Save as playlist', icon: '＋', onSelect: options.onSave });
	return items;
}
