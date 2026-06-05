import { describe, expect, test } from 'vitest';
import type { QueueItem, TidalHomeItem, Track } from '$lib/api/client';
import { queueItemToTidalPlayable, tidalHomeItemToPlayable, trackToTidalPlayable } from './track';

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
