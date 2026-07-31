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
	toggleTidalTrackFavorite,
} from '$lib/stores/player';
import type { TidalPlayable } from '$lib/api/client';
import { canPlayTrack, getPlayableLabel } from '$lib/player/playable';
import { downloadTrack, downloadTidalTrack, type DownloadFormat } from '$lib/stores/downloads';

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
	/**
	 * Set when the row is inside a playlist, to offer "Remove from playlist".
	 * The caller owns the removal because only it knows the row's position, and
	 * position is what identifies a playlist entry - the same track can appear
	 * twice.
	 */
	onRemoveFromPlaylist?: () => void;
	/** Submenu of playlists to add this track to. See `playlist_menu.ts`. */
	addToPlaylistSubmenu?: MenuItem[];
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

const SEPARATOR: MenuItem = { separator: true, label: '' };

// ─── Shared menu items ──────────────────────────────────────────────────────
// Items that are byte-identical between the library (`buildTrackMenu`) and TIDAL
// (`buildTidalTrackMenu`) builders live here once, so the two can't drift apart
// on them. Keeping the favourites glyph in a single place is how we stop a
// repeat of the mojibake that corrupted one copy's heart icon (the two builders
// had independent copies; only one rotted).

function favouriteMenuItem(localId: number, isFavorite: boolean): MenuItem {
	return {
		label: isFavorite ? 'Remove from favourites' : 'Add to favourites',
		icon: isFavorite ? '♥' : '♡',
		onSelect: () => void toggleTrackFavorite(localId, isFavorite)
	};
}

function goToMenuItem(label: string, href: string): MenuItem {
	return { label, icon: '→', onSelect: () => void goto(href) };
}

// Download-to-disk submenu (FLAC/AAC/MP3). Desktop-only: the server writes to the
// desktop's library folder, so callers hide it on the /remote mobile surface. Shared by
// the track, album, and playlist menus so the format list can't drift between them; they
// only differ in the label and in how a format maps to a trigger (local id vs TIDAL
// metadata, single track vs whole container).
export function downloadMenuItem(
	trigger: (format: DownloadFormat) => void,
	label = 'Download'
): MenuItem {
	return {
		label,
		icon: '⤓',
		submenu: [
			{ label: 'FLAC (lossless)', icon: '⤓', onSelect: () => trigger('flac') },
			{ label: 'AAC (M4A, 320)', icon: '⤓', onSelect: () => trigger('aac') },
			{ label: 'MP3 (320)', icon: '⤓', onSelect: () => trigger('mp3') }
		]
	};
}

function removeFromQueueMenuItem(queueItemId: number, onRemoved?: () => void): MenuItem {
	return {
		label: 'Remove from queue',
		icon: '×',
		danger: true,
		onSelect: async () => {
			await removeTrackFromQueue(queueItemId);
			onRemoved?.();
		}
	};
}

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
			return [removeFromQueueMenuItem(options.queueItemId, options.onRemoved)];
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
		items.push(goToMenuItem(`Go to ${track.artist_name ?? 'artist'}`, `${navPrefix}/artists/${track.artist_id}`));
	}
	if (hasAlbum) {
		items.push(goToMenuItem(`Go to ${track.album_title ?? 'album'}`, `${navPrefix}/albums/${track.album_id}`));
	}

	items.push(SEPARATOR);

	items.push(favouriteMenuItem(track.id, track.is_favorite ?? false));

	if (options.addToPlaylistSubmenu && options.addToPlaylistSubmenu.length > 0) {
		items.push({
			label: 'Add to playlist',
			icon: '＋',
			submenu: options.addToPlaylistSubmenu
		});
	}

	if (!options.remoteRoutes) {
		items.push(downloadMenuItem((format) => void downloadTrack(track.id, format)));
	}

	if (options.queueItemId != null) {
		items.push(removeFromQueueMenuItem(options.queueItemId, options.onRemoved));
	}

	if (options.onRemoveFromPlaylist) {
		items.push({
			label: 'Remove from playlist',
			icon: '×',
			danger: true,
			onSelect: options.onRemoveFromPlaylist
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
		items.push(goToMenuItem(
			`Go to ${track.artist_name ?? 'artist'}`,
			`${tidalNavPrefix}/tidal/artists/${track.artist_tidal_id}`,
		));
	}
	if (track.album_tidal_id != null) {
		items.push(goToMenuItem(
			`Go to ${track.album_title ?? 'album'}`,
			`${tidalNavPrefix}/tidal/albums/${track.album_tidal_id}`,
		));
	}
	if (track.artist_tidal_id != null || track.album_tidal_id != null) {
		items.push(SEPARATOR);
	}

	// Favouriting is always offered, even for a purely external track with no
	// local row yet: toggleTidalTrackFavorite imports on demand to mint a local
	// id first (same trick as song radio / download), so the user is never stuck
	// with no way to like the song.
	const isFavorite = track.is_favorite ?? false;
	items.push({
		label: isFavorite ? 'Remove from favourites' : 'Add to favourites',
		icon: isFavorite ? '♥' : '♡',
		onSelect: () => void toggleTidalTrackFavorite(track, isFavorite),
	});
	items.push(SEPARATOR);

	items.push({
		label: 'Play now',
		icon: '▶',
		disabled: !playable,
		hint: playable ? undefined : playableLabel,
		onSelect: () => void playTidalTrackNow(track),
	});

	// Download to disk works for any TIDAL track, library or not: the server imports it
	// on demand to mint a local id. Desktop-only, same as the library menu.
	if (!options.remoteRoutes) {
		items.push(SEPARATOR);
		items.push(downloadMenuItem((format) => void downloadTidalTrack(track, format)));
	}

	if (queueItemId != null) {
		items.push(SEPARATOR);
		items.push(removeFromQueueMenuItem(queueItemId, options.onRemoved));
	}

	return items;
}
