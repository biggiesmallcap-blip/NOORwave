import { describe, expect, test } from 'vitest';
import type {
	QueueItem,
	TidalDiscographyTrack,
	TidalHomeItem,
	TidalSearchTrack,
	Track,
} from '$lib/api/client';
import {
	albumEntryStartIndex,
	albumEntryToMixedQueueItem,
	currentTrackMatchesTracks,
	mergeAlbumTracks,
	queueItemToTidalPlayable,
	tidalDiscographyTrackToPlayable,
	tidalHomeItemToPlayable,
	tidalSearchTrackToPlayable,
	trackToTidalPlayable,
} from './track';

const baseTrack: Track = {
	id: 42,
	title: 'Overlay Track',
	artist_id: 1,
	artist_name: 'Overlay Artist',
	artist_tidal_id: 1001,
	album_id: null,
	album_title: 'Overlay Album',
	album_tidal_id: 2002,
	disc_number: null,
	track_number: null,
	duration_ms: 180000,
	isrc: null,
	tidal_id: 777,
	best_quality: 'LOSSLESS',
	best_source: 'tidal',
	fidelity_score: 0,
	is_favorite: false,
	play_count: 0,
	last_played_at: null,
	date_added: null,
	source: 'tidal_ephemeral',
	artwork_url: null,
};

describe('trackToTidalPlayable', () => {
	test('keeps normal positive library tracks on the library path', () => {
		expect(trackToTidalPlayable({ ...baseTrack, source: 'tidal_stream' })).toBeNull();
	});

	test('keeps enriched TIDAL ephemeral tracks on the TIDAL path', () => {
		expect(trackToTidalPlayable(baseTrack)).toEqual({
			tidal_id: 777,
			title: 'Overlay Track',
			artist_name: 'Overlay Artist',
			album_title: 'Overlay Album',
			artwork_url: null,
			duration_ms: 180000,
			artist_tidal_id: 1001,
			album_tidal_id: 2002,
			local_id: 42,
			is_in_library: true,
			is_favorite: false,
		});
	});
});

describe('queueItemToTidalPlayable', () => {
	test('treats TIDAL mix overlay rows as TIDAL even when the track has a local id', () => {
		const item: QueueItem = {
			id: -1,
			position: 0,
			source: 'tidal_mix',
			reason: null,
			is_pending: false,
			track: baseTrack,
		};

		expect(queueItemToTidalPlayable(item)).toEqual({
			tidal_id: 777,
			title: 'Overlay Track',
			artist_name: 'Overlay Artist',
			album_title: 'Overlay Album',
			artwork_url: null,
			duration_ms: 180000,
			artist_tidal_id: 1001,
			album_tidal_id: 2002,
			local_id: 42,
			is_in_library: true,
			is_favorite: false,
		});
	});
});

describe('tidalHomeItemToPlayable', () => {
	test('normalizes TIDAL home items to the shared playable shape', () => {
		const item: TidalHomeItem = {
			id: '123',
			kind: 'track',
			title: 'Home Track',
			artist_name: 'Home Artist',
			artist_id: 456,
			album_title: 'Home Album',
			album_id: 789,
			artwork_url: 'https://resources.tidal.com/images/abc/320x320.jpg',
			duration: 212,
		};

		expect(tidalHomeItemToPlayable(item)).toEqual({
			tidal_id: 123,
			title: 'Home Track',
			artist_name: 'Home Artist',
			album_title: 'Home Album',
			artwork_url: 'https://resources.tidal.com/images/abc/320x320.jpg',
			duration_ms: 212000,
			artist_tidal_id: 456,
			album_tidal_id: 789,
		});
	});
});

describe('tidalSearchTrackToPlayable', () => {
	test('maps raw TIDAL search metadata to the shared playable shape', () => {
		const track: TidalSearchTrack = {
			tidal_id: 909,
			title: 'Search Track',
			duration_ms: 210000,
			artist_id: 303,
			artist_name: 'Search Artist',
			album_title: 'Search Album',
			album_tidal_id: 404,
			artwork_url: 'https://resources.tidal.com/images/search/320x320.jpg',
			audio_quality: 'LOSSLESS',
			stream_ready: true,
			local_id: 55,
			in_library: true,
		};

		expect(tidalSearchTrackToPlayable(track)).toEqual({
			...track,
			artist_tidal_id: 303,
			album_tidal_id: 404,
			local_id: 55,
			is_in_library: true,
		});
	});
});

describe('tidalDiscographyTrackToPlayable', () => {
	test('preserves library metadata from TIDAL discography rows', () => {
		const track: TidalDiscographyTrack = {
			tidal_id: 808,
			title: 'Discography Track',
			duration_ms: 190000,
			artwork_url: 'https://resources.tidal.com/images/disco/320x320.jpg',
			album_title: 'Discography Album',
			album_tidal_id: 909,
			artist_name: 'Discography Artist',
			artist_tidal_id: 1001,
			track_id: 66,
			is_in_library: true,
			is_favorite: true,
		};

		expect(tidalDiscographyTrackToPlayable(track)).toEqual({
			tidal_id: 808,
			title: 'Discography Track',
			artist_name: 'Discography Artist',
			album_title: 'Discography Album',
			artwork_url: 'https://resources.tidal.com/images/disco/320x320.jpg',
			duration_ms: 190000,
			artist_tidal_id: 1001,
			album_tidal_id: 909,
			track_id: 66,
			local_id: 66,
			is_in_library: true,
			is_favorite: true,
		});
	});

	test('uses a route artist id when a discography row omits artist_tidal_id', () => {
		const track: TidalDiscographyTrack = {
			tidal_id: 808,
			title: 'Artist Page Track',
			duration_ms: 190000,
			artwork_url: null,
			album_title: null,
		};

		expect(tidalDiscographyTrackToPlayable(track, { artistTidalId: 1001 })).toMatchObject({
			tidal_id: 808,
			artist_tidal_id: 1001,
		});
	});
});

describe('album track merging', () => {
	function albumTrack(overrides: Partial<Track>): Track {
		return { ...baseTrack, source: 'tidal_stream', ...overrides };
	}
	function discoTrack(overrides: Partial<TidalDiscographyTrack>): TidalDiscographyTrack {
		return {
			tidal_id: 0,
			title: '',
			duration_ms: 180000,
			artwork_url: null,
			album_title: 'Album',
			...overrides,
		};
	}
	function titles(entries: ReturnType<typeof mergeAlbumTracks>): string[] {
		return entries.map((e) => (e.kind === 'local' ? e.local.title : e.tidal.title));
	}

	test('merges owned + TIDAL-only rows into one list ordered 1..N by track number', () => {
		// The regression: owned tracks (2, 5, 9) are scattered through the album,
		// so concatenating owned-then-TIDAL would play them out of order. The
		// merge must interleave by track_number so the queue runs the real album.
		const owned = [
			albumTrack({ id: 22, tidal_id: 8002, title: 'T2', track_number: 2, disc_number: 1 }),
			albumTrack({ id: 25, tidal_id: 8005, title: 'T5', track_number: 5, disc_number: 1 }),
			albumTrack({ id: 29, tidal_id: 8009, title: 'T9', track_number: 9, disc_number: 1 }),
		];
		const tidalOnly = [1, 3, 4, 6, 7, 8].map((n) =>
			discoTrack({ tidal_id: 8000 + n, title: `T${n}`, track_number: n, disc_number: 1 }),
		);

		const merged = mergeAlbumTracks(owned, tidalOnly);
		expect(titles(merged)).toEqual(['T1', 'T2', 'T3', 'T4', 'T5', 'T6', 'T7', 'T8', 'T9']);
		expect(merged[1]).toMatchObject({ kind: 'local' });
		expect(merged[2]).toMatchObject({ kind: 'tidal' });
	});

	test('orders across discs before track number', () => {
		const owned = [albumTrack({ id: 1, tidal_id: 1, title: 'D2T1', track_number: 1, disc_number: 2 })];
		const tidalOnly = [
			discoTrack({ tidal_id: 2, title: 'D1T2', track_number: 2, disc_number: 1 }),
			discoTrack({ tidal_id: 3, title: 'D1T1', track_number: 1, disc_number: 1 }),
		];
		expect(titles(mergeAlbumTracks(owned, tidalOnly))).toEqual(['D1T1', 'D1T2', 'D2T1']);
	});

	test('keeps owned rows that have no tidal id (local-only rips play from the library)', () => {
		const owned = [
			albumTrack({ id: 5, tidal_id: null, title: 'LocalOnly', track_number: 1, disc_number: 1 }),
			albumTrack({ id: 6, tidal_id: 6006, title: 'Owned', track_number: 2, disc_number: 1 }),
		];
		const tidalOnly = [discoTrack({ tidal_id: 3003, title: 'Streamed', track_number: 3, disc_number: 1 })];
		expect(titles(mergeAlbumTracks(owned, tidalOnly))).toEqual(['LocalOnly', 'Owned', 'Streamed']);
	});

	test('albumEntryStartIndex matches local ids first, then tidal ids, else 0', () => {
		const entries = mergeAlbumTracks(
			[
				albumTrack({ id: 22, tidal_id: 8002, title: 'T2', track_number: 2, disc_number: 1 }),
				albumTrack({ id: 25, tidal_id: null, title: 'T5', track_number: 5, disc_number: 1 }),
			],
			[1, 3, 4].map((n) =>
				discoTrack({ tidal_id: 8000 + n, title: `T${n}`, track_number: n, disc_number: 1 }),
			),
		);
		// Order: T1 T2 T3 T4 T5
		expect(albumEntryStartIndex(entries, 22)).toBe(1); // owned row by local id
		expect(albumEntryStartIndex(entries, 25)).toBe(4); // owned local-only rip by local id
		expect(albumEntryStartIndex(entries, 8003)).toBe(2); // TIDAL-only row by tidal id
		expect(albumEntryStartIndex(entries, 99999)).toBe(0); // unknown id -> top
		expect(albumEntryStartIndex(entries, undefined)).toBe(0);
	});

	test('albumEntryToMixedQueueItem sends local ids for owned rows and tidal ids otherwise', () => {
		const localEntry = mergeAlbumTracks(
			[albumTrack({ id: 42, tidal_id: 777, title: 'Owned', track_number: 1, disc_number: 1 })],
			[],
		)[0];
		expect(albumEntryToMixedQueueItem(localEntry)).toEqual({
			track_id: 42,
			artist: 'Overlay Artist',
			title: 'Owned',
		});

		const tidalEntry = mergeAlbumTracks(
			[],
			[discoTrack({ tidal_id: 8001, title: 'Streamed', artist_name: 'Stream Artist' })],
		)[0];
		expect(albumEntryToMixedQueueItem(tidalEntry)).toEqual({
			tidal_id: 8001,
			artist: 'Stream Artist',
			title: 'Streamed',
		});
	});
});

describe('currentTrackMatchesTracks', () => {
	const local = { ...baseTrack, id: 7, tidal_id: null, source: 'local' };
	const tidalRow: TidalDiscographyTrack = {
		tidal_id: 909,
		title: 'Streamed',
		duration_ms: 180000,
		artwork_url: null,
		album_title: 'Album',
	};

	test('matches owned rows by local id', () => {
		expect(currentTrackMatchesTracks({ ...baseTrack, id: 7 }, [local], [])).toBe(true);
	});

	test('matches streamed rows by tidal id even with a synthetic negative id', () => {
		const streaming = { ...baseTrack, id: -909, tidal_id: 909 };
		expect(currentTrackMatchesTracks(streaming, [local], [tidalRow])).toBe(true);
	});

	test('rejects unrelated tracks and null', () => {
		expect(currentTrackMatchesTracks({ ...baseTrack, id: 1, tidal_id: 1 }, [local], [tidalRow])).toBe(false);
		expect(currentTrackMatchesTracks(null, [local], [tidalRow])).toBe(false);
	});
});
