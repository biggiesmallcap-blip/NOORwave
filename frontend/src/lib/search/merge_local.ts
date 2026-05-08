// Merges local-DB search results into TIDAL-shape so the /search page can
// render a single unified list. Local rows that have a `tidal_id` are
// projected into TidalSearchTrack/Album/Artist with in_library=true and
// local_id set, then prepended ahead of TIDAL rows (same tidal_id deduped —
// local wins because it carries `local_id` and `is_favorite`).
//
// Local rows without a `tidal_id` (rare — local files, Spotify-only imports)
// are skipped here. Adding them requires a row component that handles a
// non-TIDAL playback path; deferred to a later pass.

import type {
	Album,
	Artist,
	SearchResults,
	TidalSearchAlbum,
	TidalSearchArtist,
	TidalSearchResults,
	TidalSearchTrack,
	Track,
} from '$lib/api/client';

function trackToTidalShape(t: Track): TidalSearchTrack | null {
	if (t.tidal_id == null) return null;
	return {
		tidal_id: t.tidal_id,
		title: t.title,
		duration_ms: t.duration_ms ?? 0,
		artist_id: t.artist_tidal_id ?? null,
		artist_name: t.artist_name,
		album_title: t.album_title,
		album_tidal_id: null,
		artwork_url: t.artwork_url,
		audio_quality: t.best_quality,
		stream_ready: null,
		local_id: t.id,
		in_library: true,
	};
}

function albumToTidalShape(a: Album): TidalSearchAlbum | null {
	if (a.tidal_id == null) return null;
	return {
		tidal_id: a.tidal_id,
		title: a.title,
		artist_name: a.artist_name,
		artwork_url: a.artwork_url,
		local_id: a.id,
		in_library: true,
	};
}

function artistToTidalShape(ar: Artist): TidalSearchArtist | null {
	if (ar.tidal_id == null) return null;
	return {
		tidal_id: ar.tidal_id,
		name: ar.name,
		artwork_url: ar.photo_url,
		local_id: ar.id,
		in_library: true,
	};
}

export function mergeLocalIntoTidal(
	local: SearchResults,
	tidal: TidalSearchResults,
): TidalSearchResults {
	const localTracks = local.tracks
		.map(trackToTidalShape)
		.filter((t): t is TidalSearchTrack => t != null);
	const localAlbums = local.albums
		.map(albumToTidalShape)
		.filter((a): a is TidalSearchAlbum => a != null);
	const localArtists = local.artists
		.map(artistToTidalShape)
		.filter((a): a is TidalSearchArtist => a != null);

	const tidalTrackIds = new Set(tidal.tracks.map((t) => t.tidal_id));
	const tidalAlbumIds = new Set(tidal.albums.map((a) => a.tidal_id));
	const tidalArtistIds = new Set(tidal.artists.map((a) => a.tidal_id));

	const extraTracks = localTracks.filter((t) => !tidalTrackIds.has(t.tidal_id));
	const extraAlbums = localAlbums.filter((a) => !tidalAlbumIds.has(a.tidal_id));
	const extraArtists = localArtists.filter((a) => !tidalArtistIds.has(a.tidal_id));

	return {
		tracks: [...extraTracks, ...tidal.tracks],
		albums: [...extraAlbums, ...tidal.albums],
		artists: [...extraArtists, ...tidal.artists],
		videos: tidal.videos,
	};
}
