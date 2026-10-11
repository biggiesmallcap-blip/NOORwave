import { describe, expect, test } from 'vitest';
import {
	ALL_VIEW_LIMITS,
	appendUnique,
	focusedViewNeedsPrefetch,
	mergeTidalPage,
	pickTopResult,
	playlistRelevance,
	previewForView,
	rankPlaylists,
	spotifyPlaylistHref,
} from './search_results';

describe('previewForView', () => {
	const many = Array.from({ length: 40 }, (_, i) => i);
	test('the all view caps each section', () => {
		expect(previewForView('all', many, 'tracks')).toHaveLength(ALL_VIEW_LIMITS.tracks);
		expect(previewForView('all', many, 'artists')).toHaveLength(ALL_VIEW_LIMITS.artists);
	});
	test('category views keep the full list', () => {
		expect(previewForView('tracks', many, 'tracks')).toHaveLength(40);
	});
});

describe('playlistRelevance', () => {
	test('exact beats prefix beats substring beats partial words', () => {
		const exact = playlistRelevance('Chill Mix', 'chill mix');
		const prefix = playlistRelevance('Chill Mix Deluxe', 'chill mix');
		const contains = playlistRelevance('My Chill Mix', 'chill mix');
		const partial = playlistRelevance('Chill Vibes', 'chill mix');
		expect(exact).toBeGreaterThan(prefix);
		expect(prefix).toBeGreaterThan(contains);
		expect(contains).toBeGreaterThan(partial);
		expect(partial).toBeGreaterThan(0);
		expect(playlistRelevance(null, 'x')).toBe(0);
	});
});

describe('rankPlaylists', () => {
	test('an exact Spotify title outranks fuzzy local and TIDAL matches', () => {
		const ranked = rankPlaylists(
			{
				local: [{ id: 1, name: 'Road Trip Songs' }],
				tidal: [{ uuid: 't1', title: 'Summer Road Trip' }],
				spotify: [{ spotifyId: 's1', title: 'Road Trip' }],
			},
			'Road Trip',
		);
		expect(ranked.map((e) => e.key)).toEqual(['spotify:s1', 'local:1', 'tidal:t1']);
	});
	test('equal scores keep local, then TIDAL, then Spotify', () => {
		const ranked = rankPlaylists(
			{
				local: [{ id: 1, name: 'Focus' }],
				tidal: [{ uuid: 't1', title: 'Focus' }],
				spotify: [{ spotifyId: 's1', title: 'Focus' }],
			},
			'focus',
		);
		expect(ranked.map((e) => e.kind)).toEqual(['local', 'tidal', 'spotify']);
	});
});

describe('pickTopResult', () => {
	test('an exact track name beats a fuzzy artist match', () => {
		const top = pickTopResult(
			{
				artist: { name: 'Karma Police Tribute Band', in_library: false },
				track: { title: 'Karma Police', in_library: false },
			},
			'karma police',
		);
		expect(top?.kind).toBe('track');
	});
	test('ties prefer artist over album over track', () => {
		const top = pickTopResult(
			{
				artist: { name: 'Blue', in_library: false },
				album: { title: 'Blue', in_library: false },
				track: { title: 'Blue', in_library: false },
			},
			'blue',
		);
		expect(top?.kind).toBe('artist');
	});
	test('nothing to pick from gives null', () => {
		expect(pickTopResult({}, 'x')).toBeNull();
	});
});

describe('pagination merges', () => {
	test('appendUnique drops overlapping ids', () => {
		expect(appendUnique([{ id: 1 }, { id: 2 }], [{ id: 2 }, { id: 3 }], (x) => x.id)).toEqual([
			{ id: 1 },
			{ id: 2 },
			{ id: 3 },
		]);
	});
	test('a short page in every section exhausts TIDAL', () => {
		const page = (n: number) => Array.from({ length: n }, (_, i) => ({ tidal_id: 100 + i }));
		const current = { tracks: page(2), albums: [], artists: [] };
		const full = mergeTidalPage(current, { tracks: page(50), albums: [], artists: [] }, 50);
		expect(full.exhausted).toBe(false);
		expect(full.results.tracks).toHaveLength(50);
		const short = mergeTidalPage(current, { tracks: page(3), albums: [], artists: [] }, 50);
		expect(short.exhausted).toBe(true);
	});
});

describe('focusedViewNeedsPrefetch', () => {
	const base = {
		mode: 'tracks' as const,
		busy: false,
		committedQuery: 'radiohead',
		audioSearch: false,
		hasTidalResults: true,
		hasMoreTidal: true,
		hasMoreTidalPlaylists: false,
		hasMoreSpotifyPlaylists: false,
	};
	test('a focused TIDAL category with more pages prefetches', () => {
		expect(focusedViewNeedsPrefetch(base)).toBe(true);
	});
	test('never while busy, for an audio search, or in the all view', () => {
		expect(focusedViewNeedsPrefetch({ ...base, busy: true })).toBe(false);
		expect(focusedViewNeedsPrefetch({ ...base, audioSearch: true })).toBe(false);
		expect(focusedViewNeedsPrefetch({ ...base, mode: 'all' })).toBe(false);
	});
	test('the playlists view prefetches while either provider has more', () => {
		expect(
			focusedViewNeedsPrefetch({ ...base, mode: 'playlists', hasMoreSpotifyPlaylists: true }),
		).toBe(true);
	});
});

describe('spotifyPlaylistHref', () => {
	test('remembers the search query for the back link', () => {
		expect(spotifyPlaylistHref('ab c', 'road trip')).toBe(
			'/spotify-playlist/ab%20c?from=search&q=road+trip',
		);
		expect(spotifyPlaylistHref('x', '')).toBe('/spotify-playlist/x?from=search');
	});
});
