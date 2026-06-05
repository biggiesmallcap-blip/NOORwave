import type {
	QueueItem,
	Track,
	TidalDiscographyTrack,
	TidalHomeItem,
	TidalPlayable,
	TidalSearchTrack,
} from '$lib/api/client';

/**
 * Convert an ephemeral `Track` (a now-playing or queue row) into a
 * `TidalPlayable` for `buildTidalTrackMenu` / `startTidalSongRadio`.
 *
 * Returns `null` for tracks that are NOT ephemeral Tidal entries.
 * Real `tidal_stream` library rows stay on the library path; enriched
 * `tidal_ephemeral` rows keep the Tidal path even after they gain a local id.
 * Callers fall back to `buildTrackMenu` when this returns null.
 *
 * Detection: `play_tidal_ephemeral` on the backend constructs a
 * synthetic `Track { id: -tidal_track_id, source: 'tidal_ephemeral', ... }`.
 * Store-side enrichment can later replace the id with a local id, so source is
 * also part of the signal.
 */
export function trackToTidalPlayable(track: Track): TidalPlayable | null {
	if (track.tidal_id == null) return null;
	if (track.id >= 0 && track.source !== 'tidal_ephemeral') return null;
	return trackWithTidalIdToPlayable(track);
}

function trackWithTidalIdToPlayable(track: Track): TidalPlayable | null {
	if (track.tidal_id == null || track.tidal_id <= 0) return null;
	const localId = track.id > 0 ? track.id : null;
	return {
		tidal_id: track.tidal_id,
		title: track.title,
		artist_name: track.artist_name,
		album_title: track.album_title,
		artwork_url: track.artwork_url,
		duration_ms: track.duration_ms,
		artist_tidal_id: track.artist_tidal_id ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
		local_id: localId,
		is_in_library: localId != null,
		is_favorite: track.is_favorite ?? false,
	};
}

export function queueItemToTidalPlayable(item: QueueItem): TidalPlayable | null {
	const direct = trackToTidalPlayable(item.track);
	if (direct) return direct;
	if (item.id >= 0 || item.source !== 'tidal_mix') return null;
	return trackWithTidalIdToPlayable(item.track);
}

/**
 * Convert a `TidalSearchTrack` (from `/api/tidal/search` results)
 * into a `TidalPlayable`. Lifted from search/+page.svelte's local
 * helper so frontend has one place to do this conversion.
 */
export function tidalSearchTrackToPlayable(track: TidalSearchTrack): TidalPlayable {
	return {
		...track,
		artist_tidal_id: track.artist_id ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
		local_id: track.local_id ?? null,
		is_in_library: track.in_library,
	};
}

export function tidalDiscographyTrackToPlayable(track: TidalDiscographyTrack): TidalPlayable {
	return {
		tidal_id: track.tidal_id,
		title: track.title,
		artist_name: track.artist_name ?? null,
		album_title: track.album_title ?? null,
		artwork_url: track.artwork_url ?? null,
		duration_ms: track.duration_ms,
		artist_tidal_id: track.artist_tidal_id ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
		track_id: track.track_id,
		local_id: track.track_id ?? null,
		is_in_library: track.is_in_library,
		is_favorite: track.is_favorite,
	};
}

export function tidalHomeItemToPlayable(item: TidalHomeItem): TidalPlayable {
	return {
		tidal_id: Number(item.id),
		title: item.title,
		artist_name: item.artist_name ?? null,
		album_title: item.album_title ?? null,
		artwork_url: item.artwork_url ?? null,
		duration_ms: item.duration != null ? item.duration * 1000 : null,
		artist_tidal_id: item.artist_id ?? null,
		album_tidal_id: item.album_id ?? null,
	};
}
