import { goto } from '$app/navigation';
import {
	api,
	type ProviderRecommendationItem,
	type ProviderRecommendationShelf,
	type TidalPlayable,
} from '$lib/api/client';
import { buildAlbumMenu } from '$lib/player/album_menu';
import { buildArtistMenu } from '$lib/player/artist_menu';
import { buildTidalTrackMenu, buildTrackMenu } from '$lib/player/track_menu';
import {
	playRecommendationAlbum,
	playRecommendationArtist,
} from '$lib/player/play_recommendations';
import type { MenuItem } from '$lib/stores/context_menu';
import {
	recommendationEntity,
	recommendationHrefFromSearch,
	recommendationKnownHref,
	recommendationSearchHref,
	recommendationSearchQuery,
} from './recommendation_navigation';

/**
 * Shared behaviour for a recommendation card, wherever it is rendered.
 *
 * The Home shelf owned all of this privately until the shelves gained a
 * "View all" page. Two surfaces showing the same cards must open the same menu
 * and resolve links the same way, and the repo rule is that every asset
 * reference carries the shared context menu - so this is the one copy rather
 * than a second implementation on the new route.
 */
export function recommendationItemToTidalPlayable(
	item: ProviderRecommendationItem,
): TidalPlayable {
	return {
		tidal_id: item.tidal_id ?? 0,
		title: item.title,
		artist_name: item.artist_name,
		album_title: item.album_title,
		artwork_url: item.artwork_url,
		duration_ms: null,
		artist_tidal_id: null,
		track_id: item.local_track_id ?? undefined,
		local_id: item.local_track_id,
		is_in_library: Boolean(item.local_track_id),
	};
}

/**
 * True when this album card is really a single: TIDAL has the track but no album.
 *
 * The server sets `is_single` after failing to match an album and succeeding on
 * the track, so the client does not have to search to find out. There is no
 * tracklist to open, so the card seeds song radio from the track instead - which
 * is the closest thing to "listen to this" a single supports.
 */
export function isRecommendationSingle(item: ProviderRecommendationItem): boolean {
	return (
		recommendationEntity(item) === 'album' &&
		Boolean(item.is_single) &&
		Boolean(item.tidal_id) &&
		!item.local_album_id &&
		!item.tidal_album_id
	);
}

/** Start song radio from the single behind an album card. */
export async function playRecommendationSingle(item: ProviderRecommendationItem): Promise<void> {
	const { startTidalSongRadio } = await import('$lib/stores/player');
	await startTidalSongRadio(recommendationItemToTidalPlayable(item));
}

/**
 * Open an artist or album card.
 *
 * Prefers ids we already hold, then a TIDAL lookup, and finally the search page
 * rather than guessing - see `findArtistMatch` for why a wrong guess is worse
 * than a search result.
 */
export async function openRecommendationItem(item: ProviderRecommendationItem) {
	const entity = recommendationEntity(item);
	if (entity !== 'artist' && entity !== 'album') return;
	const knownHref = recommendationKnownHref(item);
	if (knownHref) return goto(knownHref);
	try {
		const results = await api.searchTidal(recommendationSearchQuery(item), 5);
		const resolvedHref = recommendationHrefFromSearch(item, results);
		if (resolvedHref) return goto(resolvedHref);
	} catch {
		// Search route fallback keeps the user moving when TIDAL lookup fails.
	}
	return goto(recommendationSearchHref(item));
}

/**
 * One right-click menu per entity, reusing the shared builders the rest of the
 * app uses. Unresolved Last.fm albums/artists (no ids yet) get a
 * resolve-then-act menu so queue-style actions still work before a TIDAL match
 * exists.
 */
export function recommendationItemMenu(item: ProviderRecommendationItem): MenuItem[] {
	const entity = recommendationEntity(item);
	if (entity === 'track') {
		if (item.local_track_id) {
			return buildTrackMenu({
				id: item.local_track_id,
				title: item.title,
				artist_id: item.local_artist_id ?? null,
				artist_name: item.artist_name,
				album_id: item.local_album_id ?? null,
				album_title: item.album_title,
			});
		}
		return buildTidalTrackMenu(recommendationItemToTidalPlayable(item));
	}
	if (entity === 'album') {
		if (item.local_album_id || item.tidal_album_id) {
			return buildAlbumMenu({
				id: item.local_album_id ?? null,
				tidal_id: item.tidal_album_id ?? null,
				title: item.title,
				artist_id: item.local_artist_id ?? null,
				artist_name: item.artist_name,
				in_library: Boolean(item.local_album_id),
			});
		}
		if (isRecommendationSingle(item)) {
			// A single has no album menu to build: the track is the whole thing.
			return buildTidalTrackMenu(recommendationItemToTidalPlayable(item));
		}
		return [
			{ label: 'Play album', icon: '▶', onSelect: () => void playRecommendationAlbum(item) },
			{ separator: true, label: '' },
			{ label: 'Open album page', icon: '↗', onSelect: () => void openRecommendationItem(item) },
		];
	}
	// artist
	if (item.local_artist_id) {
		return buildArtistMenu({
			id: item.local_artist_id,
			tidal_id: item.tidal_artist_id ?? null,
			name: item.title,
			in_library: true,
		});
	}
	return [
		{ label: 'Play top tracks', icon: '▶', onSelect: () => void playRecommendationArtist(item) },
		{ separator: true, label: '' },
		{ label: 'Open artist', icon: '↗', onSelect: () => void openRecommendationItem(item) },
	];
}

/**
 * Route slug for a shelf's "View all" page.
 *
 * Derived from provider + entity rather than the shelf title, so the URL does
 * not change when the copy does. Both the link and the route resolve through
 * these two functions, so they cannot disagree.
 */
export function recommendationShelfSlug(shelf: {
	provider: string;
	entity_type?: string;
}): string {
	return `${shelf.provider}-${shelf.entity_type ?? 'track'}`;
}

export function matchesRecommendationShelfSlug(
	shelf: ProviderRecommendationShelf,
	slug: string,
): boolean {
	return recommendationShelfSlug(shelf) === slug;
}
