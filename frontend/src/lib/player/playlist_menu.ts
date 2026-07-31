import { api, type Playlist } from '$lib/api/client';
import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
import { showToast } from '$lib/stores/toast';
import type { MenuItem } from '$lib/stores/context_menu';

// Shared context-menu builders for playlists, alongside the track/album/artist
// builders in this directory. STYLING.md ("Media links and context menus")
// requires shared builders over inline menu arrays; before this module every
// playlist menu in the app was hand-rolled at its call site, which is why they
// all offered different actions.

export interface BuildPlaylistMenuOptions {
	onPlay?: () => void;
	onShuffle?: () => void;
	onRadio?: () => void;
	onToggleFavorite?: () => void;
	onOpen?: () => void;
	onRename?: () => void;
	onEditRules?: () => void;
	onDuplicate?: () => void;
	onRefreshFromTidal?: () => void;
	onDownload?: () => void;
	onDelete?: () => void;
}

/**
 * The canonical playlist right-click menu. Every action is opt-in: a caller
 * that cannot perform one simply omits the handler and the item disappears,
 * so a read-only surface and the detail page share this one builder.
 *
 * Icons are real glyphs, not letters. `menuIconForDisplay` strips anything that
 * looks like a text placeholder, so the old single-letter icons rendered as
 * nothing at all.
 */
export function buildPlaylistMenu(
	playlist: Playlist,
	options: BuildPlaylistMenuOptions = {},
): MenuItem[] {
	const items: MenuItem[] = [];

	if (options.onPlay) items.push({ label: 'Play', icon: '▶', onSelect: options.onPlay });
	if (options.onShuffle) items.push({ label: 'Shuffle', icon: '⤨', onSelect: options.onShuffle });
	if (options.onRadio) items.push({ label: 'Start radio', icon: '◉', onSelect: options.onRadio });
	if (options.onOpen) items.push({ label: 'Open playlist', icon: '↗', onSelect: options.onOpen });

	if (items.length > 0) items.push({ label: '', separator: true });

	if (options.onToggleFavorite) {
		items.push({
			label: playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites',
			icon: '♥',
			onSelect: options.onToggleFavorite,
		});
	}
	if (options.onDownload) {
		items.push({ label: 'Download', icon: '⤓', onSelect: options.onDownload });
	}

	const edits: MenuItem[] = [];
	if (options.onRename) edits.push({ label: 'Rename', icon: '✎', onSelect: options.onRename });
	if (options.onEditRules) {
		edits.push({ label: 'Edit rules', icon: '⚙', onSelect: options.onEditRules });
	}
	if (options.onDuplicate) {
		edits.push({ label: 'Duplicate', icon: '⧉', onSelect: options.onDuplicate });
	}
	if (options.onRefreshFromTidal && playlist.tidal_uuid) {
		edits.push({ label: 'Refresh from TIDAL', icon: '⟳', onSelect: options.onRefreshFromTidal });
	}
	if (edits.length > 0) {
		items.push({ label: '', separator: true });
		items.push(...edits);
	}

	if (options.onDelete) {
		items.push({ label: '', separator: true });
		items.push({ label: 'Delete', icon: '×', danger: true, onSelect: options.onDelete });
	}

	return items;
}

/**
 * An "Add to playlist" submenu listing the user's regular playlists, favourites
 * first. `getTrackIds` is lazy so a caller can resolve an album's or artist's
 * tracks only once the user actually picks a destination.
 *
 * Smart playlists are excluded: their contents come from rules, so adding a
 * track to one is meaningless.
 */
export function buildAddToPlaylistSubmenu(
	playlists: Playlist[],
	getTrackIds: () => Promise<number[]>,
): MenuItem[] {
	return [...playlists]
		.filter((playlist) => !playlist.is_smart)
		.sort((a, b) => {
			if (a.is_favorite !== b.is_favorite) return a.is_favorite ? -1 : 1;
			return a.name.localeCompare(b.name);
		})
		.map((playlist) => ({
			label: playlist.name,
			icon: playlist.is_favorite ? '♥' : '♩',
			onSelect: async () => {
				const trackIds = await getTrackIds();
				if (!trackIds.length) return;
				try {
					const { added } = await api.addTracksToPlaylist(playlist.id, trackIds);
					invalidatePlaylistCaches();
					showToast(
						`Added ${added} track${added !== 1 ? 's' : ''} to ${playlist.name}`,
						'success',
					);
				} catch {
					showToast(`Could not add to ${playlist.name}`, 'error');
				}
			},
		}));
}
