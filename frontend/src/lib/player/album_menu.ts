import { goto } from '$app/navigation';
import type { MenuItem } from '$lib/stores/context_menu';
import {
	playAlbum,
	shuffleAlbum,
	startAlbumRadio,
	playTidalAlbum,
	saveTidalAlbumToLibrary,
} from '$lib/stores/player';
import { downloadAlbum } from '$lib/stores/downloads';

// Narrow shape so the builder can accept a local Album, a TIDAL search album,
// a TIDAL discography album, or a compact `{ id, title }` from a carousel.
// Mirror the MenuTrack pattern in track_menu.ts: keep the surface small enough
// that every album-like value in the codebase satisfies it without adapters.
export interface AlbumLike {
	id?: number | null;
	tidal_id?: number | null;
	local_id?: number | null;
	title: string;
	artist_id?: number | null;
	artist_name?: string | null;
	in_library?: boolean;
}

export interface BuildAlbumMenuOptions {
	isLocal?: boolean;
	hideOpen?: boolean;
	hideRadio?: boolean;
	includeSelect?: boolean;
	includeRemove?: boolean;
	addToPlaylistSubmenu?: MenuItem[];
	onSelect?: () => void;
	onRemove?: () => void;
	/**
	 * Route "Go to artist" / "Open album" via the /remote/* counterparts so a
	 * menu opened from the mobile remote stays inside the mobile shell.
	 */
	remoteRoutes?: boolean;
}

const SEPARATOR: MenuItem = { separator: true, label: '' };

function resolveLocalId(album: AlbumLike): number | null {
	if (typeof album.id === 'number') return album.id;
	if (typeof album.local_id === 'number') return album.local_id;
	return null;
}

/**
 * buildAlbumMenu — one factory for every right-click / ⋯ menu on an album.
 * Used by library cards, search results, artist-page discography, the album
 * popup, the album page hero, and TIDAL pages. Adding a new action here
 * makes it available everywhere.
 */
export function buildAlbumMenu(album: AlbumLike, options: BuildAlbumMenuOptions = {}): MenuItem[] {
	const localId = resolveLocalId(album);
	const tidalId = album.tidal_id ?? null;
	const isLocal = options.isLocal ?? (album.in_library ?? localId != null);

	const items: MenuItem[] = [];

	if (isLocal && localId != null) {
		items.push({
			label: 'Play album',
			icon: '▶',
			onSelect: () => void playAlbum(localId),
		});
		items.push({
			label: 'Shuffle album',
			icon: '⤮',
			onSelect: () => void shuffleAlbum(localId),
		});
		if (!options.hideRadio) {
			items.push({
				label: 'Album radio',
				icon: '◉',
				onSelect: () => void startAlbumRadio(localId),
			});
		}
	} else if (tidalId != null) {
		items.push({
			label: 'Play album',
			icon: '▶',
			onSelect: () => void playTidalAlbum(tidalId),
		});
		items.push({
			label: 'Add to library',
			icon: '＋',
			onSelect: () => void saveTidalAlbumToLibrary(tidalId),
		});
	}

	const hasArtist = album.artist_id != null && album.artist_name != null;
	const hasOpen = !options.hideOpen && (localId != null || tidalId != null);

	if (hasArtist || hasOpen || options.addToPlaylistSubmenu) {
		items.push(SEPARATOR);
	}

	const navPrefix = options.remoteRoutes ? '/remote' : '';

	if (hasArtist) {
		items.push({
			label: `Go to ${album.artist_name}`,
			icon: '→',
			onSelect: () => void goto(`${navPrefix}/artists/${album.artist_id}`),
		});
	}

	if (hasOpen) {
		if (isLocal && localId != null) {
			items.push({
				label: 'Open album',
				icon: '↗',
				onSelect: () => void goto(`${navPrefix}/albums/${localId}`),
			});
		} else if (tidalId != null) {
			items.push({
				label: 'Open on Tidal',
				icon: '↗',
				onSelect: () => void goto(`${navPrefix}/tidal/albums/${tidalId}`),
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

	// Batch download (desktop-only; the server writes to the local library folder).
	if (isLocal && localId != null && !options.remoteRoutes) {
		items.push(SEPARATOR);
		items.push({
			label: 'Download album',
			icon: '⤓',
			submenu: [
				{
					label: 'FLAC (lossless)',
					icon: '⤓',
					onSelect: () => void downloadAlbum(localId, 'flac')
				},
				{ label: 'MP3 (320)', icon: '⤓', onSelect: () => void downloadAlbum(localId, 'mp3') }
			]
		});
	}

	if (options.includeSelect && options.onSelect) {
		items.push(SEPARATOR);
		items.push({
			label: 'Select',
			icon: '✓',
			onSelect: options.onSelect,
		});
	}

	if (options.includeRemove && options.onRemove) {
		items.push(SEPARATOR);
		items.push({
			label: 'Remove from library',
			icon: '×',
			danger: true,
			onSelect: options.onRemove,
		});
	}

	return items;
}
