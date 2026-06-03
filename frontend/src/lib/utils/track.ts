import type { QueueItem, Track, TidalHomeItem, TidalPlayable, TidalSearchTrack } from '$lib/api/client';

/**
 * Convert an ephemeral `Track` (a now-playing or queue row) into a
 * `TidalPlayable` for `buildTidalTrackMenu` / `startTidalSongRadio`.
 *
 * Returns `null` for tracks that are NOT ephemeral Tidal entries
 * (real library rows have positive ids; tracks without a tidal_id
 * can't be sent to the Tidal-aware path). Callers fall back to
 * `buildTrackMenu` when this returns null.
 *
 * Detection: `play_tidal_ephemeral` on the backend constructs a
 * synthetic `Track { id: -tidal_track_id, ... }`. That negative id
 * is the signal. Library tracks always have positive ids.
 */
export function trackToTidalPlayable(track: Track): TidalPlayable | null {
	if (track.id >= 0 || track.tidal_id == null) return null;
	return trackWithTidalIdToPlayable(track);
}

function trackWithTidalIdToPlayable(track: Track): TidalPlayable | null {
	if (track.tidal_id == null || track.tidal_id <= 0) return null;
	return {
		tidal_id: track.tidal_id,
		title: track.title,
		artist_name: track.artist_name,
		album_title: track.album_title,
		artwork_url: track.artwork_url,
		duration_ms: track.duration_ms,
		artist_tidal_id: track.artist_tidal_id ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
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
