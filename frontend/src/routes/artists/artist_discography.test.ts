import { describe, expect, test } from 'vitest';
import type { Track, TidalDiscographyAlbum, TidalDiscographyTrack } from '$lib/api/client';
import {
	buildPopularTrackItems,
	categorizeTidalAlbum,
	discographySectionFor,
	sortTidalAlbumsByReleaseDate,
} from './artist_discography';

function album(overrides: Partial<TidalDiscographyAlbum>): TidalDiscographyAlbum {
	return {
		tidal_id: 1,
		local_id: null,
		title: 'Release',
		artwork_url: null,
		release_date: null,
		release_type: null,
		source_filter: null,
		number_of_tracks: 10,
		artist_name: 'Artist',
		in_library: false,
		...overrides,
	} as TidalDiscographyAlbum;
}

function localTrack(overrides: Partial<Track>): Track {
	return {
		id: 1,
		title: 'Track',
		artist_id: 1,
		artist_name: 'Artist',
		album_id: null,
		album_title: null,
		duration_ms: 1000,
		is_favorite: false,
		play_count: 0,
		tidal_id: null,
		...overrides,
	} as Track;
}

function tidalTrack(overrides: Partial<TidalDiscographyTrack>): TidalDiscographyTrack {
	return {
		tidal_id: 100,
		title: 'Tidal track',
		duration_ms: 1000,
		artwork_url: null,
		album_title: null,
		album_tidal_id: null,
		artist_name: 'Artist',
		artist_tidal_id: null,
		track_number: null,
		disc_number: null,
		...overrides,
	} as TidalDiscographyTrack;
}

describe('categorizeTidalAlbum', () => {
	test('the editorial source filter wins over release_type', () => {
		expect(categorizeTidalAlbum(album({ source_filter: 'COMPILATIONS', release_type: 'ALBUM' }))).toBe('compilation');
		expect(categorizeTidalAlbum(album({ source_filter: 'LIVE' }))).toBe('live');
		expect(categorizeTidalAlbum(album({ source_filter: 'EPSANDSINGLES' }))).toBe('ep_single');
		expect(categorizeTidalAlbum(album({ source_filter: 'ALBUMS' }))).toBe('album');
	});

	test('falls back to release_type, then track count', () => {
		expect(categorizeTidalAlbum(album({ release_type: 'EP' }))).toBe('ep_single');
		expect(categorizeTidalAlbum(album({ release_type: 'LIVE' }))).toBe('live');
		expect(categorizeTidalAlbum(album({ number_of_tracks: 2 }))).toBe('ep_single');
		expect(categorizeTidalAlbum(album({ number_of_tracks: 8 }))).toBe('album');
	});
});

describe('discographySectionFor', () => {
	test('folds live releases into the albums see-all section explicitly', () => {
		// The see-all routes have no /live section; live releases must land in
		// albums THERE while the artist page still shows a separate Live shelf.
		expect(discographySectionFor(album({ source_filter: 'LIVE' }))).toBe('albums');
		expect(discographySectionFor(album({ source_filter: 'ALBUMS' }))).toBe('albums');
		expect(discographySectionFor(album({ source_filter: 'EPSANDSINGLES' }))).toBe('singles');
		expect(discographySectionFor(album({ source_filter: 'COMPILATIONS' }))).toBe('compilations');
	});
});

describe('sortTidalAlbumsByReleaseDate', () => {
	test('sorts newest first with missing dates last and stable title tiebreak', () => {
		const sorted = sortTidalAlbumsByReleaseDate([
			album({ tidal_id: 1, title: 'B', release_date: '2024-01-05' }),
			album({ tidal_id: 2, title: 'A', release_date: '2024-01-05' }),
			album({ tidal_id: 3, title: 'Old', release_date: '1999-12-31' }),
			album({ tidal_id: 4, title: 'Unknown', release_date: null }),
			album({ tidal_id: 5, title: 'New', release_date: '2024-12-01' }),
		]);
		expect(sorted.map((a) => a.tidal_id)).toEqual([5, 2, 1, 3, 4]);
	});
});

describe('buildPopularTrackItems', () => {
	test('follows the TIDAL order, swaps in owned rows, appends leftovers by score', () => {
		const tracks = [
			localTrack({ id: 10, tidal_id: 100, play_count: 1 }),
			localTrack({ id: 11, tidal_id: null, play_count: 50 }),
			localTrack({ id: 12, tidal_id: null, play_count: 99 }),
		];
		const top = [
			tidalTrack({ tidal_id: 200 }),
			tidalTrack({ tidal_id: 100 }),
			tidalTrack({ tidal_id: 200 }), // duplicate must be dropped
		];
		const items = buildPopularTrackItems(tracks, top);
		expect(items.map((i) => (i.kind === 'local' ? `L${i.track.id}` : `T${i.track.tidal_id}`))).toEqual([
			'T200',
			'L10',
			'L12',
			'L11',
		]);
	});

	test('falls back to score-ordered local tracks when TIDAL returns nothing', () => {
		const tracks = [
			localTrack({ id: 1, play_count: 5 }),
			localTrack({ id: 2, play_count: 20 }),
		];
		const items = buildPopularTrackItems(tracks, []);
		expect(items.map((i) => (i.kind === 'local' ? i.track.id : -1))).toEqual([2, 1]);
	});
});
