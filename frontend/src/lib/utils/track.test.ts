import { describe, expect, test } from 'vitest';
import type {
	QueueItem,
	TidalDiscographyTrack,
	TidalHomeItem,
	TidalSearchTrack,
	Track,
} from '$lib/api/client';
import {
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
});
