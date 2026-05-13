import { describe, expect, test } from 'vitest';
import type { QueueItem, Track } from '$lib/api/client';
import { queueItemToTidalPlayable, trackToTidalPlayable } from './track';

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
		});
	});
});
