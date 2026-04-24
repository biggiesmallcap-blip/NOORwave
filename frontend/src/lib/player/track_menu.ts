import { goto } from '$app/navigation';
import type { MenuItem } from '$lib/stores/context_menu';
import {
	addTrackToQueue,
	playAlbum,
	playTrackNext,
	removeTrackFromQueue,
	shuffleAlbum,
	startArtistRadio,
	startSongRadio,
	toggleTrackFavorite
} from '$lib/stores/player';

// Narrow shape so the builder can accept a Track, a QueueItem.track, or a
// DiscoveryRadioResult mapped through `mapRadioToMenuTrack`. We avoid a hard
// dependency on the full Track type so library rows, discover cards, and
// playlist views can all use one menu.
export interface MenuTrack {
	id: number;
	title: string;
	artist_id?: number | null;
	artist_name?: string | null;
	album_id?: number | null;
	album_title?: string | null;
	is_favorite?: boolean;
}

export interface BuildTrackMenuOptions {
	/** Set when the track is already in the queue so "Remove" becomes available. */
	queueItemId?: number;
	/** Hide album actions (e.g., when the row is an album-page row). */
	hideAlbumActions?: boolean;
	/** Hide artist actions (e.g., when the row is an artist-page row). */
	hideArtistActions?: boolean;
	/** Callback after a destructive action like remove, so caller can refetch. */
	onRemoved?: () => void;
}

const SEPARATOR: MenuItem = { separator: true, label: '' };

/**
 * buildTrackMenu — one factory for every right-click / ⋯ menu on a track row.
 * The same item list is used by the now-playing panel, queue rows, library
 * tables, discover cards, and album/artist track lists. Adding a new action
 * here makes it available everywhere.
 */
export function buildTrackMenu(track: MenuTrack, options: BuildTrackMenuOptions = {}): MenuItem[] {
	const items: MenuItem[] = [];
	const hasAlbum = !options.hideAlbumActions && track.album_id != null;
	const hasArtist = !options.hideArtistActions && track.artist_id != null;

	items.push({
		label: 'Play next',
		icon: '⤴',
		onSelect: () => void playTrackNext(track.id)
	});
	items.push({
		label: 'Add to queue',
		icon: '＋',
		onSelect: () => void addTrackToQueue(track.id)
	});

	items.push(SEPARATOR);

	items.push({
		label: 'Song radio',
		icon: '◉',
		hint: 'Start from this song',
		onSelect: () => void startSongRadio(track.id)
	});

	if (hasAlbum) {
		items.push({
			label: 'Play album',
			icon: '▶',
			onSelect: () => void playAlbum(track.album_id!, track.id)
		});
		items.push({
			label: 'Shuffle album',
			icon: '⤮',
			onSelect: () => void shuffleAlbum(track.album_id!)
		});
	}

	if (hasArtist) {
		items.push({
			label: 'Artist radio',
			icon: '✦',
			onSelect: () => void startArtistRadio(track.artist_id!, track.id)
		});
	}

	items.push(SEPARATOR);

	if (hasArtist) {
		items.push({
			label: `Go to ${track.artist_name ?? 'artist'}`,
			icon: '→',
			onSelect: () => void goto(`/artists/${track.artist_id}`)
		});
	}
	if (hasAlbum) {
		items.push({
			label: `Go to ${track.album_title ?? 'album'}`,
			icon: '→',
			onSelect: () => void goto(`/albums/${track.album_id}`)
		});
	}

	items.push(SEPARATOR);

	items.push({
		label: track.is_favorite ? 'Remove from favourites' : 'Add to favourites',
		icon: track.is_favorite ? '♥' : '♡',
		onSelect: () => void toggleTrackFavorite(track.id)
	});

	if (options.queueItemId != null) {
		items.push({
			label: 'Remove from queue',
			icon: '×',
			danger: true,
			onSelect: async () => {
				await removeTrackFromQueue(options.queueItemId!);
				options.onRemoved?.();
			}
		});
	}

	return items;
}
