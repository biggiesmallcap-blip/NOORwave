import { api, type ProviderRecommendationItem, type TidalPlayable } from '$lib/api/client';
import {
	playAlbum,
	playArtist,
	playTidalAlbum,
	playTidalTracksNow,
	playerError,
} from '$lib/stores/player';
import {
	findAlbumMatch,
	findArtistMatch,
	recommendationSearchQuery,
} from '$lib/components/home/recommendation_navigation';

// Last.fm album/artist recommendations usually arrive with no resolved ids (the
// backend only fills them when the entity is already in the library), so a Play
// action has to resolve through a TIDAL search first. These helpers centralise
// that resolve-then-play dance so the shelf, its context menus, and double-click
// all behave identically. Mirrors play_trending.ts for chart entries.

export interface ResolvedEntity {
	localId: number | null;
	tidalId: number | null;
}

export async function resolveRecommendationAlbum(
	item: ProviderRecommendationItem,
): Promise<ResolvedEntity | null> {
	if (item.local_album_id) return { localId: item.local_album_id, tidalId: item.tidal_album_id ?? null };
	if (item.tidal_album_id) return { localId: null, tidalId: item.tidal_album_id };
	try {
		// 12, not 5: TIDAL ranks album search over the whole catalogue, so
		// "<artist> <title>" leads with the artist's most-played albums and the one
		// asked for often sat just outside the top 5. 12 is also the bucket
		// ARTWORK_SEARCH_LIMIT uses server-side, so this shares that cache row.
		// Measured over the 50-item album shelf, 12 and 20 resolve the same items.
		const results = await api.searchTidal(recommendationSearchQuery(item), 12);
		const match = findAlbumMatch(item, results.albums);
		if (match) return { localId: match.local_id ?? null, tidalId: match.tidal_id };
	} catch {
		// Fall through to the not-found toast below.
	}
	return null;
}

export async function resolveRecommendationArtist(
	item: ProviderRecommendationItem,
): Promise<ResolvedEntity | null> {
	if (item.local_artist_id) return { localId: item.local_artist_id, tidalId: item.tidal_artist_id ?? null };
	if (item.tidal_artist_id) return { localId: null, tidalId: item.tidal_artist_id };
	try {
		// Same 12 as the album path and the server-side pass, so all three share
		// one tidal_search_cache row per query.
		const results = await api.searchTidal(recommendationSearchQuery(item), 12);
		const match = findArtistMatch(item, results.artists);
		if (match) return { localId: match.local_id ?? null, tidalId: match.tidal_id };
	} catch {
		// Fall through to the not-found toast below.
	}
	return null;
}

export async function playRecommendationAlbum(item: ProviderRecommendationItem): Promise<void> {
	const resolved = await resolveRecommendationAlbum(item);
	if (resolved?.localId) return playAlbum(resolved.localId);
	if (resolved?.tidalId) return playTidalAlbum(resolved.tidalId);
	playerError.set({ message: "Couldn't find that album on Tidal." });
}

export async function playRecommendationArtist(item: ProviderRecommendationItem): Promise<void> {
	const resolved = await resolveRecommendationArtist(item);
	if (resolved?.localId) return playArtist(resolved.localId);
	const tidalId = resolved?.tidalId ?? null;
	if (!tidalId) {
		playerError.set({ message: "Couldn't find that artist on Tidal." });
		return;
	}
	try {
		const profile = await api.getTidalArtistProfile(tidalId);
		const tracks: TidalPlayable[] = (profile.top_tracks ?? []).map((track) => ({
			tidal_id: track.tidal_id,
			title: track.title,
			artist_name: track.artist_name ?? item.artist_name ?? item.title,
			album_title: track.album_title,
			artwork_url: track.artwork_url,
			duration_ms: track.duration_ms,
			artist_tidal_id: track.artist_tidal_id ?? tidalId,
			album_tidal_id: track.album_tidal_id ?? null,
			track_id: track.track_id,
			local_id: track.track_id ?? null,
			is_in_library: track.is_in_library,
			is_favorite: track.is_favorite,
		}));
		if (!tracks.length) {
			playerError.set({ message: 'No playable tracks for that artist yet.' });
			return;
		}
		await playTidalTracksNow(tracks, item.title);
	} catch {
		playerError.set({ message: "Couldn't load that artist's top tracks." });
	}
}
