import type { Track, TidalPlayable, TidalSearchTrack } from '$lib/api/client';

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
 * is the signal — library tracks always have positive ids.
 */
export function trackToTidalPlayable(track: Track): TidalPlayable | null {
	if (track.id >= 0 || track.tidal_id == null) return null;
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
