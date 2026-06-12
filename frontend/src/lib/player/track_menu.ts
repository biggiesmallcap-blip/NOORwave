import { goto } from '$app/navigation';
import type { MenuItem } from '$lib/stores/context_menu';
import {
	addTrackToQueue,
	moveQueueTrackNext,
	playAlbum,
	playTrackNext,
	removeTrackFromQueue,
	shuffleAlbum,
	startArtistRadio,
	startSongRadio,
	toggleTrackFavorite,
	playTidalTrackNow,
	playTidalTrackNext,
	addTidalTrackToQueue,
	startTidalSongRadio,
} from '$lib/stores/player';
import type { TidalPlayable } from '$lib/api/client';
import { canPlayTrack, getPlayableLabel } from '$lib/player/playable';

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
	/** Phase 2c-ii-a: pending rows have no resolved track_id; strip all actions
	 *  that require one, leaving only Remove from queue. */
	isPending?: boolean;
	/**
	 * When true, "Go to artist" / "Go to album" route to the /remote/* mobile
	 * counterparts instead of the desktop pages. Used by surfaces under /remote
	 * so navigation stays inside the mobile shell.
	 */
	remoteRoutes?: boolean;
}

export interface BuildTidalTrackMenuOptions {
	inQueue?: boolean;
	/**
	 * Set when this TIDAL track is a real, mutable queue row (an ephemeral
	 * mix/album/playlist row). Enables "Move next" + "Remove from queue" against
	 * the row instead of the library-less defaults.
	 */
	queueItemId?: number;
	/** Callback after a destructive action like remove, so caller can refetch. */
	onRemoved?: () => void;
	/** See BuildTrackMenuOptions.remoteRoutes. */
	remoteRoutes?: boolean;
}

function tidalLocalTrackId(track: TidalPlayable): number | null {
	const id = track.track_id ?? track.local_id ?? null;
	return typeof id === 'number' && id > 0 ? id : null;
}

const SEPARATOR: MenuItem = { separator: true, label: '' };

/**
 * buildTrackMenu — one factory for every right-click / ⋯ menu on a track row.
 * The same item list is used by the now-playing panel, queue rows, library
 * tables, discover cards, and album/artist track lists. Adding a new action
 * here makes it available everywhere.
 */
export function buildTrackMenu(track: MenuTrack, options: BuildTrackMenuOptions = {}): MenuItem[] {
	// Pending rows haven't resolved to a library track yet; only removal is safe.
	if (options.isPending) {
		if (options.queueItemId != null) {
			return [{
				label: 'Remove from queue',
				icon: '×',
				danger: true,
				onSelect: async () => {
					await removeTrackFromQueue(options.queueItemId!);
					options.onRemoved?.();
				}
			}];
		}
		return [];
	}

	const items: MenuItem[] = [];
	const hasAlbum = !options.hideAlbumActions && track.album_id != null;
	const hasArtist = !options.hideArtistActions && track.artist_id != null && track.artist_id > 0;
	const isQueueItem = options.queueItemId != null;

	items.push({
		label: isQueueItem ? 'Move next' : 'Play next',
		icon: '⤴',
		onSelect: () => {
			if (isQueueItem) {
				void moveQueueTrackNext(options.queueItemId!);
				return;
			}
			void playTrackNext(track.id);
		}
	});
	if (!isQueueItem) {
		items.push({
			label: 'Add to queue',
			icon: '＋',
			onSelect: () => void addTrackToQueue(track.id)
		});
	}

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

	const navPrefix = options.remoteRoutes ? '/remote' : '';
	if (hasArtist) {
		items.push({
			label: `Go to ${track.artist_name ?? 'artist'}`,
			icon: '→',
			onSelect: () => void goto(`${navPrefix}/artists/${track.artist_id}`)
		});
	}
	if (hasAlbum) {
		items.push({
			label: `Go to ${track.album_title ?? 'album'}`,
			icon: '→',
			onSelect: () => void goto(`${navPrefix}/albums/${track.album_id}`)
		});
	}

	items.push(SEPARATOR);

	items.push({
		label: track.is_favorite ? 'Remove from favourites' : 'Add to favourites',
		icon: track.is_favorite ? '♥' : '♡',
		onSelect: () => void toggleTrackFavorite(track.id, track.is_favorite ?? false)
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

export function buildTidalTrackMenu(track: TidalPlayable, options: BuildTidalTrackMenuOptions = {}): MenuItem[] {
	const playable = canPlayTrack(track);
	const playableLabel = getPlayableLabel(track);
	const queueItemId = options.queueItemId;
	const items: MenuItem[] = [
		queueItemId != null
			? {
					label: 'Move next',
					icon: '⤴',
					onSelect: () => void moveQueueTrackNext(queueItemId),
				}
			: {
					label: 'Play next',
					icon: '⤴',
					disabled: !playable,
					hint: playable ? undefined : playableLabel,
					onSelect: () => void playTidalTrackNext(track),
				},
		...(options.inQueue
			? []
			: [{
					label: 'Add to queue',
					icon: '＋',
					disabled: !playable,
					hint: playable ? undefined : playableLabel,
					onSelect: () => void addTidalTrackToQueue(track),
				}]),
		SEPARATOR,
		{
			label: 'Song radio',
			icon: '◉',
			disabled: !playable,
			hint: playable ? 'Start from this song' : playableLabel,
			onSelect: () => void startTidalSongRadio(track),
		},
		SEPARATOR,
	];

	const tidalNavPrefix = options.remoteRoutes ? '/remote' : '';
	if (track.artist_tidal_id != null) {
		items.push({
			label: `Go to ${track.artist_name ?? 'artist'}`,
			icon: '→',
			onSelect: () => void goto(`${tidalNavPrefix}/tidal/artists/${track.artist_tidal_id}`),
		});
	}
	if (track.album_tidal_id != null) {
		items.push({
			label: `Go to ${track.album_title ?? 'album'}`,
			icon: '→',
			onSelect: () => void goto(`${tidalNavPrefix}/tidal/albums/${track.album_tidal_id}`),
		});
	}
	if (track.artist_tidal_id != null || track.album_tidal_id != null) {
		items.push(SEPARATOR);
	}

	const localId = tidalLocalTrackId(track);
	if (localId != null) {
		items.push({
			label: track.is_favorite ? 'Remove from favourites' : 'Add to favourites',
			icon: track.is_favorite ? 'â™¥' : 'â™¡',
			onSelect: () => void toggleTrackFavorite(localId, track.is_favorite ?? false),
		});
		items.push(SEPARATOR);
	}

	items.push({
		label: 'Play now',
		icon: '▶',
		disabled: !playable,
		hint: playable ? undefined : playableLabel,
		onSelect: () => void playTidalTrackNow(track),
	});

	if (queueItemId != null) {
		items.push(SEPARATOR);
		items.push({
			label: 'Remove from queue',
			icon: '×',
			danger: true,
			onSelect: async () => {
				await removeTrackFromQueue(queueItemId);
				options.onRemoved?.();
			},
		});
	}

	return items;
}
