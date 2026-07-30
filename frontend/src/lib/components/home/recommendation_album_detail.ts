import {
	api,
	type Album,
	type ProviderRecommendationItem,
	type TidalDiscographyTrack,
	type Track,
} from '$lib/api/client';
import { cachedApi } from '$lib/cache/api_queries';
import { resolveRecommendationAlbum } from '$lib/player/play_recommendations';

/**
 * Everything AlbumDetailPopup needs for a recommended album.
 *
 * `isLocal` decides whether the popup's own local-id actions are usable. A
 * Last.fm recommendation is usually not in the library, so most of the time it
 * is false and the caller supplies TIDAL play handlers instead.
 */
export type RecommendationAlbumDetail = {
	album: Album;
	tracks: Track[];
	isLocal: boolean;
	localAlbumId: number | null;
	tidalAlbumId: number | null;
};

/**
 * A TIDAL album track rendered as the local `Track` shape the popup expects.
 *
 * `id` is the local track id when the track happens to be owned, and otherwise
 * the negated TIDAL id. It has to be unique and stable because the popup keys
 * its rows on it and compares it against the currently playing track; a
 * negative number cannot collide with a real local id, and it makes an
 * unowned row obvious to anything that reads it.
 *
 * `track_id` is checked for truthiness rather than null, because the endpoint
 * sends 0 for a track that is not in the library. `??` accepted that 0 and gave
 * every unowned row the same id, which made the popup throw
 * `each_key_duplicate` and render nothing at all.
 */
function tidalTrackToLocalShape(
	track: TidalDiscographyTrack,
	album: Album,
	index: number,
): Track {
	return {
		id: track.track_id ? track.track_id : -track.tidal_id,
		tidal_id: track.tidal_id,
		title: track.title,
		artist_id: 0,
		artist_name: track.artist_name ?? album.artist_name,
		album_id: album.id,
		album_title: track.album_title ?? album.title,
		duration_ms: track.duration_ms,
		track_number: track.track_number ?? index + 1,
		disc_number: track.disc_number ?? 1,
		artwork_url: track.artwork_url ?? album.artwork_url,
		is_favorite: track.is_favorite ?? false,
		play_count: 0,
	} as unknown as Track;
}

/**
 * Resolve a recommended album to the popup's inputs.
 *
 * Prefers the library copy, because then every action in the popup works
 * natively. Falls back to the TIDAL album, whose tracks are mapped into the
 * local shape. Returns null when the album cannot be found at all, and the
 * caller should fall back to opening the album page.
 */
export async function loadRecommendationAlbumDetail(
	item: ProviderRecommendationItem,
): Promise<RecommendationAlbumDetail | null> {
	const resolved = await resolveRecommendationAlbum(item);
	if (!resolved) return null;

	if (resolved.localId) {
		try {
			const { tracks } = await cachedApi.getAlbumTracks(resolved.localId);
			// There is no single-album endpoint, so the row is assembled from the
			// tracks - which carry the canonical artist id and title, and are the
			// reason to prefer the library copy in the first place.
			const first = tracks[0];
			const album: Album = {
				...synthesiseAlbum(item, {
					id: resolved.localId,
					tidalId: resolved.tidalId,
					source: 'Library',
					trackCount: tracks.length,
				}),
				title: first?.album_title ?? item.title,
				artist_id: first?.artist_id ?? item.local_artist_id ?? 0,
				artist_name: first?.artist_name ?? item.artist_name,
				artwork_url: first?.artwork_url ?? item.artwork_url,
			};
			return {
				album,
				tracks,
				isLocal: true,
				localAlbumId: resolved.localId,
				tidalAlbumId: resolved.tidalId,
			};
		} catch {
			// Fall through to the TIDAL path below if the library read fails.
		}
	}

	if (!resolved.tidalId) return null;

	try {
		const { tracks } = await api.getTidalAlbumTracks(resolved.tidalId);
		const album = synthesiseAlbum(item, {
			id: 0,
			tidalId: resolved.tidalId,
			source: 'Tidal',
			trackCount: tracks.length,
		});
		return {
			album,
			tracks: tracks.map((track, index) => tidalTrackToLocalShape(track, album, index)),
			isLocal: false,
			localAlbumId: null,
			tidalAlbumId: resolved.tidalId,
		};
	} catch {
		return null;
	}
}

/**
 * Build an `Album` from what the recommendation already carries.
 *
 * There is no endpoint that returns an album row for a TIDAL id we do not own,
 * and the popup only reads title, artist, artwork, year, release type, track
 * count and source - all of which are either on the recommendation or knowable
 * from the resolution. `id: 0` is never used for playback in this case because
 * `isLocal` is false and the caller passes explicit handlers.
 */
function synthesiseAlbum(
	item: ProviderRecommendationItem,
	opts: { id: number; tidalId: number | null; source: string; trackCount: number },
): Album {
	return {
		id: opts.id,
		tidal_id: opts.tidalId,
		title: item.title,
		artist_id: item.local_artist_id ?? 0,
		artist_name: item.artist_name,
		year: null,
		artwork_url: item.artwork_url,
		release_type: null,
		track_count: opts.trackCount,
		source: opts.source,
	};
}
