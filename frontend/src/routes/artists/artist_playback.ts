import type { TidalDiscographyTrack, Track } from '$lib/api/client';
import { currentTrackMatchesTracks } from '$lib/utils/track';

export function artistCurrentTrackMatchesArtist(
	current: Track | null,
	localTracks: readonly Track[],
	artistTidalId: number | null | undefined,
	tidalTopTracks: readonly TidalDiscographyTrack[]
): boolean {
	if (!current) return false;
	if (artistTidalId != null && current.artist_tidal_id === artistTidalId) return true;
	// List membership (local id or tidal id) is the shared contract also used
	// by the album page; only the artist-id shortcut above is artist-specific.
	return currentTrackMatchesTracks(current, localTracks, tidalTopTracks);
}
