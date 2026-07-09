import type { Track, TidalDiscographyAlbum, TidalDiscographyTrack } from '$lib/api/client';

// Single source of truth for how the artist surfaces bucket and order TIDAL
// releases and merge Top-tracks lists. ArtistDetail.svelte and
// ArtistDiscographySection.svelte used to carry private copies of this logic
// and they drifted (the see-all page folded LIVE releases into Albums while
// the artist page kept a separate Live shelf, so the same album could appear
// in different buckets depending on which page you were on).

export type DiscoCategory = 'album' | 'ep_single' | 'compilation' | 'live';

export function categorizeTidalAlbum(album: TidalDiscographyAlbum): DiscoCategory {
	// The TIDAL editorial filter is more authoritative than the per-album
	// release_type body field; a compilation tagged release_type:"ALBUM"
	// used to land in the Albums shelf and the Compilations shelf stayed
	// empty even though the data was fetched.
	switch (album.source_filter) {
		case 'COMPILATIONS':
			return 'compilation';
		case 'LIVE':
			return 'live';
		case 'EPSANDSINGLES':
			return 'ep_single';
		case 'ALBUMS':
			return 'album';
	}
	const type = (album.release_type ?? '').toUpperCase();
	if (type === 'COMPILATION') return 'compilation';
	if (type === 'LIVE') return 'live';
	if (type === 'SINGLE' || type === 'EP') return 'ep_single';
	if (type === 'ALBUM') return 'album';
	return (album.number_of_tracks ?? 0) >= 3 ? 'album' : 'ep_single';
}

export type DiscographySection = 'albums' | 'singles' | 'compilations';

// The see-all pages have three release sections (there is no /live route),
// so Live releases surface under Albums THERE - but through this explicit
// mapping, not a divergent categorize copy. The artist page itself still
// renders Live as its own shelf via `categorizeTidalAlbum`.
export function discographySectionFor(album: TidalDiscographyAlbum): DiscographySection {
	switch (categorizeTidalAlbum(album)) {
		case 'compilation':
			return 'compilations';
		case 'ep_single':
			return 'singles';
		case 'album':
		case 'live':
			return 'albums';
	}
}

export function sortTidalAlbumsByReleaseDate(
	list: TidalDiscographyAlbum[]
): TidalDiscographyAlbum[] {
	// Compare full ISO date strings (YYYY-MM-DD sorts lexicographically)
	// not just the year: a Dec 2024 release should sit above a Jan 2024 one.
	// Missing dates sort to the bottom; equal dates fall back to the title
	// so the order is stable across fetches.
	return [...list].sort((a, b) => {
		const ad = a.release_date ?? '';
		const bd = b.release_date ?? '';
		if (ad === bd) return a.title.localeCompare(b.title);
		if (!ad) return 1;
		if (!bd) return -1;
		return bd.localeCompare(ad);
	});
}

export type PopularTrackItem =
	| { kind: 'local'; track: Track }
	| { kind: 'tidal'; track: TidalDiscographyTrack };

export function popularTrackItemKey(item: PopularTrackItem): string {
	return item.kind === 'local' ? `local-${item.track.id}` : `tidal-${item.track.tidal_id}`;
}

/**
 * "Top tracks" follows TIDAL's popularity-ranked top-tracks order when
 * available, replacing TIDAL rows with local rows where the user owns them
 * (deduped by tidal_id). Local-only leftovers are appended, ordered by
 * `localScore` (defaults to play count).
 */
export function buildPopularTrackItems(
	tracks: Track[],
	tidalTopTracks: TidalDiscographyTrack[],
	localScore: (track: Track) => number = (track) => track.play_count ?? 0
): PopularTrackItem[] {
	const byTidalId = new Map<number, Track>();
	for (const track of tracks) {
		if (track.tidal_id != null && track.tidal_id > 0) byTidalId.set(track.tidal_id, track);
	}

	const seenLocalIds = new Set<number>();
	const seenTidalIds = new Set<number>();
	const ordered: PopularTrackItem[] = [];

	for (const tidalTrack of tidalTopTracks) {
		if (seenTidalIds.has(tidalTrack.tidal_id)) continue;
		seenTidalIds.add(tidalTrack.tidal_id);
		const localTrack = byTidalId.get(tidalTrack.tidal_id);
		if (localTrack) {
			seenLocalIds.add(localTrack.id);
			ordered.push({ kind: 'local', track: localTrack });
		} else {
			ordered.push({ kind: 'tidal', track: tidalTrack });
		}
	}

	const localRemainder = tracks
		.filter((track) => !seenLocalIds.has(track.id))
		.sort((a, b) => localScore(b) - localScore(a));

	if (ordered.length === 0) {
		return localRemainder.map((track) => ({ kind: 'local', track }));
	}

	ordered.push(...localRemainder.map((track) => ({ kind: 'local' as const, track })));
	return ordered;
}
