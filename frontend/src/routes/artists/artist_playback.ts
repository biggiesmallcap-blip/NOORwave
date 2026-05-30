import type { TidalDiscographyTrack, Track } from '$lib/api/client';

export function artistCurrentTrackMatchesArtist(
	current: Track | null,
	localTracks: readonly Track[],
	artistTidalId: number | null | undefined,
	tidalTopTracks: readonly TidalDiscographyTrack[]
): boolean {
	if (!current) return false;
	if (localTracks.some((track) => track.id === current.id)) return true;
	if (artistTidalId != null && current.artist_tidal_id === artistTidalId) return true;
	if (current.tidal_id != null && tidalTopTracks.some((track) => track.tidal_id === current.tidal_id)) {
		return true;
	}
	return false;
}
