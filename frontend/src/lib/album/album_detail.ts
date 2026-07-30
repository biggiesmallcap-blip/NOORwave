import {
	api,
	type Album,
	type TidalDiscographyTrack,
	type Track,
} from '$lib/api/client';
import { cachedApi } from '$lib/cache/api_queries';

/**
 * Everything AlbumDetailPopup needs, loaded from whatever the caller has.
 *
 * Two entry points, because a card knows one of two things about an album: a
 * local id (Library, Search's local results) or a TIDAL id (the discover
 * shelves, editorial pages, Last.fm recommendations). `isLocal` decides whether
 * the popup's own local-id actions are usable; when false the caller supplies
 * play handlers instead.
 */
export type AlbumDetail = {
	album: Album;
	tracks: Track[];
	isLocal: boolean;
	localAlbumId: number | null;
	tidalAlbumId: number | null;
};

/**
 * What the card already knew, used to fill the popup's header.
 *
 * Neither endpoint returns an album row - `getAlbumTracks` returns tracks and
 * TIDAL has no single-album lookup - so the header is assembled from the track
 * rows plus whatever the card was already displaying. That also means the popup
 * shows the right title and cover on the first frame instead of flashing empty.
 */
export type AlbumHints = {
	title: string;
	artistName?: string | null;
	artworkUrl?: string | null;
	artistId?: number | null;
	year?: number | null;
	releaseType?: string | null;
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

function albumFromHints(
	hints: AlbumHints,
	opts: { id: number; tidalId: number | null; source: string; trackCount: number },
): Album {
	return {
		id: opts.id,
		tidal_id: opts.tidalId,
		title: hints.title,
		artist_id: hints.artistId ?? 0,
		artist_name: hints.artistName ?? null,
		year: hints.year ?? null,
		artwork_url: hints.artworkUrl ?? null,
		release_type: hints.releaseType ?? null,
		track_count: opts.trackCount,
		source: opts.source,
	};
}

/** The library copy, where every action in the popup works natively. */
export async function loadLocalAlbumDetail(
	localAlbumId: number,
	hints: AlbumHints,
	tidalAlbumId: number | null = null,
): Promise<AlbumDetail | null> {
	try {
		const { tracks } = await cachedApi.getAlbumTracks(localAlbumId);
		// The tracks carry the canonical artist id and title, which is the reason
		// to prefer the library copy in the first place.
		const first = tracks[0];
		const album: Album = {
			...albumFromHints(hints, {
				id: localAlbumId,
				tidalId: tidalAlbumId,
				source: 'Library',
				trackCount: tracks.length,
			}),
			title: first?.album_title ?? hints.title,
			artist_id: first?.artist_id ?? hints.artistId ?? 0,
			artist_name: first?.artist_name ?? hints.artistName ?? null,
			artwork_url: first?.artwork_url ?? hints.artworkUrl ?? null,
		};
		return {
			album,
			tracks,
			isLocal: true,
			localAlbumId,
			tidalAlbumId,
		};
	} catch {
		return null;
	}
}

/** A TIDAL album, owned or not, with its tracks mapped into the local shape. */
export async function loadTidalAlbumDetail(
	tidalAlbumId: number,
	hints: AlbumHints,
): Promise<AlbumDetail | null> {
	try {
		const { tracks } = await api.getTidalAlbumTracks(tidalAlbumId);
		const album = albumFromHints(hints, {
			id: 0,
			tidalId: tidalAlbumId,
			source: 'Tidal',
			trackCount: tracks.length,
		});
		return {
			album,
			tracks: tracks.map((track, index) => tidalTrackToLocalShape(track, album, index)),
			isLocal: false,
			localAlbumId: null,
			tidalAlbumId,
		};
	} catch {
		return null;
	}
}

/**
 * Prefer the library copy, fall back to TIDAL.
 *
 * `id: 0` on the TIDAL path is never used for playback, because `isLocal` is
 * false and the caller passes explicit handlers.
 */
export async function loadAlbumDetail(
	ids: { localAlbumId?: number | null; tidalAlbumId?: number | null },
	hints: AlbumHints,
): Promise<AlbumDetail | null> {
	if (ids.localAlbumId) {
		const local = await loadLocalAlbumDetail(ids.localAlbumId, hints, ids.tidalAlbumId ?? null);
		if (local) return local;
	}
	if (ids.tidalAlbumId) return loadTidalAlbumDetail(ids.tidalAlbumId, hints);
	return null;
}
