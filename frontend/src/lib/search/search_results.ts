// Pure rules behind the Search page: section previews, playlist ranking, the
// top-result hero, pagination merges, and the prefetch decision. The page owns
// state and timing; these functions own the decisions, so they can be tested
// by behaviour instead of by grepping the page source.

export type SearchFilterMode = 'all' | 'tracks' | 'albums' | 'artists' | 'playlists' | 'library';

/** How many of each section the combined "all" view previews. */
export const ALL_VIEW_LIMITS = {
	artists: 24,
	albums: 24,
	tracks: 10,
	playlists: 12,
} as const;

/** The all-results view previews each section; category views show the full list. */
export function previewForView<T>(
	mode: SearchFilterMode,
	items: readonly T[],
	section: keyof typeof ALL_VIEW_LIMITS,
): T[] {
	return mode === 'all' ? items.slice(0, ALL_VIEW_LIMITS[section]) : [...items];
}

/**
 * Title relevance on the same ladder as the top-result hero: exact title beats
 * prefix beats substring, with partial credit when only some words of a
 * multi-word query hit. `q` must already be lower-cased.
 */
export function playlistRelevance(title: string | null | undefined, q: string): number {
	if (!title || !q) return 0;
	const t = title.toLowerCase();
	if (t === q) return 1.0;
	if (t.startsWith(q)) return 0.6;
	if (t.includes(q)) return 0.3;
	const tokens = q.split(/\s+/).filter((tok) => tok.length > 0);
	if (tokens.length === 0) return 0;
	const matched = tokens.filter((tok) => t.includes(tok)).length;
	return (matched / tokens.length) * 0.25;
}

export type RankedPlaylist<L, T, S> =
	| { kind: 'local'; key: string; score: number; playlist: L }
	| { kind: 'tidal'; key: string; score: number; playlist: T }
	| { kind: 'spotify'; key: string; score: number; playlist: S };

/**
 * One relevance-ranked rail across all three playlist sources. A stable sort
 * keeps local > TIDAL > Spotify on equal scores, so an exact-title match from
 * any provider is never buried behind weaker matches from another.
 */
export function rankPlaylists<
	L extends { id: number; name: string },
	T extends { uuid: string; title: string },
	S extends { spotifyId: string; title: string | null },
>(sources: { local: readonly L[]; tidal: readonly T[]; spotify: readonly S[] }, query: string): RankedPlaylist<L, T, S>[] {
	const q = query.toLowerCase();
	const entries: RankedPlaylist<L, T, S>[] = [
		...sources.local.map((p) => ({
			kind: 'local' as const,
			key: `local:${p.id}`,
			score: playlistRelevance(p.name, q),
			playlist: p,
		})),
		...sources.tidal.map((p) => ({
			kind: 'tidal' as const,
			key: `tidal:${p.uuid}`,
			score: playlistRelevance(p.title, q),
			playlist: p,
		})),
		...sources.spotify.map((p) => ({
			kind: 'spotify' as const,
			key: `spotify:${p.spotifyId}`,
			score: playlistRelevance(p.title, q),
			playlist: p,
		})),
	];
	return entries.sort((a, b) => b.score - a.score);
}

export type TopResult<Ar, Al, Tr> =
	| { kind: 'artist'; entry: Ar }
	| { kind: 'album'; entry: Al }
	| { kind: 'track'; entry: Tr };

/**
 * The hero: the best first entry across artists, albums and tracks. Exact name
 * beats prefix beats substring, library entries get a boost, and ties prefer
 * artist over album over track.
 */
export function pickTopResult<
	Ar extends { name: string; in_library: boolean },
	Al extends { title: string; in_library: boolean },
	Tr extends { title: string; in_library: boolean },
>(
	firsts: { artist?: Ar; album?: Al; track?: Tr },
	query: string,
): TopResult<Ar, Al, Tr> | null {
	const q = query.toLowerCase();
	const score = (name: string, inLibrary: boolean, kindBias: number) => {
		const n = name.toLowerCase();
		let s = 0;
		if (n === q) s += 1.0;
		else if (n.startsWith(q)) s += 0.6;
		else if (n.includes(q)) s += 0.3;
		if (inLibrary) s += 0.3;
		return s + kindBias;
	};
	const candidates: { tr: TopResult<Ar, Al, Tr>; s: number }[] = [];
	if (firsts.artist) {
		candidates.push({
			tr: { kind: 'artist', entry: firsts.artist },
			s: score(firsts.artist.name, firsts.artist.in_library, 0.05),
		});
	}
	if (firsts.album) {
		candidates.push({
			tr: { kind: 'album', entry: firsts.album },
			s: score(firsts.album.title, firsts.album.in_library, 0.025),
		});
	}
	if (firsts.track) {
		candidates.push({
			tr: { kind: 'track', entry: firsts.track },
			s: score(firsts.track.title, firsts.track.in_library, 0),
		});
	}
	if (candidates.length === 0) return null;
	candidates.sort((a, b) => b.s - a.s);
	return candidates[0].tr;
}

/** Append only the items whose key is not already present (providers overlap pages). */
export function appendUnique<T, K>(existing: readonly T[], incoming: readonly T[], key: (item: T) => K): T[] {
	const seen = new Set(existing.map(key));
	return [...existing, ...incoming.filter((item) => !seen.has(key(item)))];
}

type TidalPage<Tr, Al, Ar> = { tracks: Tr[]; albums: Al[]; artists: Ar[] };

/**
 * Merge the next TIDAL page (tracks, albums and artists share one offset) and
 * say whether the provider is exhausted: every section came back short.
 */
export function mergeTidalPage<
	Tr extends { tidal_id: number },
	Al extends { tidal_id: number },
	Ar extends { tidal_id: number },
>(
	current: TidalPage<Tr, Al, Ar>,
	next: TidalPage<Tr, Al, Ar>,
	pageSize: number,
): { results: TidalPage<Tr, Al, Ar>; exhausted: boolean } {
	const id = (item: { tidal_id: number }) => item.tidal_id;
	return {
		results: {
			tracks: appendUnique(current.tracks, next.tracks, id),
			albums: appendUnique(current.albums, next.albums, id),
			artists: appendUnique(current.artists, next.artists, id),
		},
		exhausted:
			next.tracks.length < pageSize &&
			next.albums.length < pageSize &&
			next.artists.length < pageSize,
	};
}

/**
 * Whether a focused category view should prefetch one deeper page right after
 * the light initial batch (the "all" and "library" views never page).
 */
export function focusedViewNeedsPrefetch(input: {
	mode: SearchFilterMode;
	busy: boolean;
	committedQuery: string;
	audioSearch: boolean;
	hasTidalResults: boolean;
	hasMoreTidal: boolean;
	hasMoreTidalPlaylists: boolean;
	hasMoreSpotifyPlaylists: boolean;
}): boolean {
	if (input.busy || !input.committedQuery.trim() || input.audioSearch) return false;
	switch (input.mode) {
		case 'tracks':
		case 'albums':
		case 'artists':
			return input.hasTidalResults && input.hasMoreTidal;
		case 'playlists':
			return input.hasMoreTidalPlaylists || input.hasMoreSpotifyPlaylists;
		default:
			return false;
	}
}

/** Link to a Spotify playlist that remembers it came from this search. */
export function spotifyPlaylistHref(spotifyId: string, activeQuery: string): string {
	const params = new URLSearchParams({ from: 'search' });
	if (activeQuery) params.set('q', activeQuery);
	return `/spotify-playlist/${encodeURIComponent(spotifyId)}?${params.toString()}`;
}
