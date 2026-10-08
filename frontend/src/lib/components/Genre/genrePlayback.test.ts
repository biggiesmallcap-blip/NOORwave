import { describe, expect, test } from 'vitest';
import type { Track } from '$lib/api/client';
import { MAX_GENRE_QUEUE, pickSeedTrackId, sampleGenreQueue } from './genrePlayback';

const track = (id: number) => ({ id }) as Track;

describe('genrePlayback', () => {
	test('samples only real library ids, capped', () => {
		const tracks = [-1, 0, ...Array.from({ length: 400 }, (_, index) => index + 1)].map(track);
		const ids = sampleGenreQueue(tracks);
		expect(ids).toHaveLength(MAX_GENRE_QUEUE);
		expect(ids.every((id) => id > 0)).toBe(true);
		expect(new Set(ids).size).toBe(ids.length);
	});

	test('no seed without a playable track', () => {
		expect(pickSeedTrackId([])).toBeNull();
		expect(pickSeedTrackId([track(-3)])).toBeNull();
		expect(pickSeedTrackId([track(7)])).toBe(7);
	});
});
