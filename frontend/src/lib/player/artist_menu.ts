import { goto } from '$app/navigation';
import type { MenuItem } from '$lib/stores/context_menu';
import {
	playArtist,
	shuffleArtist,
	startArtistRadio,
} from '$lib/stores/player';

export interface ArtistLike {
	id?: number | null;
	tidal_id?: number | null;
	local_id?: number | null;
	name: string;
	in_library?: boolean;
}

export interface BuildArtistMenuOptions {
	isLocal?: boolean;
	hideOpen?: boolean;
	hideRadio?: boolean;
	addToPlaylistSubmenu?: MenuItem[];
	includeRemove?: boolean;
	onRemove?: () => void;
}

const SEPARATOR: MenuItem = { separator: true, label: '' };

function resolveLocalId(artist: ArtistLike): number | null {
	if (typeof artist.id === 'number') return artist.id;
	if (typeof artist.local_id === 'number') return artist.local_id;
	return null;
}

/**
 * buildArtistMenu — one factory for every right-click / ⋯ menu on an artist.
 * Used by library artist circles, search results, now-playing artist link,
 * track-row inner artist link, album hero artist link, and TIDAL artist pages.
 */
export function buildArtistMenu(artist: ArtistLike, options: BuildArtistMenuOptions = {}): MenuItem[] {
	const localId = resolveLocalId(artist);
	const tidalId = artist.tidal_id ?? null;
	const isLocal = options.isLocal ?? (artist.in_library ?? localId != null);

	const items: MenuItem[] = [];

	if (isLocal && localId != null) {
		items.push({
			label: 'Play artist',
			icon: '▶',
			onSelect: () => void playArtist(localId),
		});
		items.push({
			label: 'Shuffle artist',
			icon: '⤮',
			onSelect: () => void shuffleArtist(localId),
		});
		if (!options.hideRadio) {
			items.push({
				label: 'Artist radio',
				icon: '◉',
				onSelect: () => void startArtistRadio(localId),
			});
		}
	}

	if (!options.hideOpen) {
		if (items.length > 0) items.push(SEPARATOR);
		if (isLocal && localId != null) {
			items.push({
				label: 'Open artist',
				icon: '↗',
				onSelect: () => void goto(`/artists/${localId}`),
			});
		} else if (tidalId != null) {
			items.push({
				label: 'Open on Tidal',
				icon: '↗',
				onSelect: () => void goto(`/tidal/artists/${tidalId}`),
			});
		}
	}

	if (isLocal && options.addToPlaylistSubmenu && options.addToPlaylistSubmenu.length > 0) {
		items.push({
			label: 'Add to playlist',
			icon: '＋',
			submenu: options.addToPlaylistSubmenu,
		});
	}

	if (options.includeRemove && options.onRemove) {
		items.push(SEPARATOR);
		items.push({
			label: "Remove all artist's tracks",
			icon: '×',
			danger: true,
			onSelect: options.onRemove,
		});
	}

	return items;
}
