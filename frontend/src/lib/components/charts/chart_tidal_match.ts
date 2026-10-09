import type { TidalPlayable, TidalSearchTrack } from '$lib/api/client';
import { gatedTidalSearch } from '$lib/actions/lazy-tidal-art';
import { QueryCache, type QueryOptions } from '$lib/cache/query';
import { tidalSearchTrackToPlayable } from '$lib/utils/track';

/// Chart entry -> TIDAL track matches, shared by every chart surface.
///
/// Keyed by the normalised "artist title" rather than a snapshot entry id: the
/// same song sits in a dozen region/provider cells, and the old per-entry,
/// per-mount map re-ran the whole page of TIDAL searches on every visit and
/// every region switch. In-memory only and deduped in flight, so two cells
/// asking for one song issue one search. Its own cache so a page of matches
/// never evicts the shared API cache's entries.

const matches = new QueryCache({ maxEntries: 500 });
const MATCH_OPTIONS: QueryOptions = { staleMs: 6 * 60 * 60 * 1000 };

export function chartMatchKey(artist: string | null | undefined, title: string): string {
	return `${artist ?? ''} ${title}`.toLowerCase().replace(/\s+/g, ' ').trim();
}

function cacheKey(key: string) {
	return ['charts', 'tidalMatch', key] as const;
}

/** Cached match: a track, null for a known miss, undefined when not looked up yet. */
export function peekChartMatch(key: string): TidalSearchTrack | null | undefined {
	return matches.peek<TidalSearchTrack | null>(cacheKey(key));
}

/**
 * Find the chart entry on TIDAL. Resolves null for a real miss (cached), and
 * rejects when the shared search breaker is open or the search fails, so a
 * transient outage is retried next time instead of being remembered as "not
 * on TIDAL" for the rest of the session.
 */
export function matchChartTrack(artist: string | null | undefined, title: string): Promise<TidalSearchTrack | null> {
	const key = chartMatchKey(artist, title);
	return matches.fetchQuery(
		cacheKey(key),
		async () => {
			// Shared gate, not a bare searchTidal: a chart page resolves its whole
			// visible list at once, and an uncapped fan-out would blow past the
			// in-flight limit every other surface respects.
			const results = await gatedTidalSearch([artist, title].filter(Boolean).join(' '), 3);
			if (!results) throw new Error('TIDAL search paused');
			return results.tracks.find((track) => track.stream_ready !== false) ?? null;
		},
		MATCH_OPTIONS,
	);
}

export function playableFromMatch(hit: TidalSearchTrack, fallbackArtwork: string | null): TidalPlayable {
	const playable = tidalSearchTrackToPlayable(hit);
	return { ...playable, artwork_url: playable.artwork_url ?? fallbackArtwork };
}
