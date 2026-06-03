import { beforeEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import type { Track } from '$lib/api/client';

const cachedApiMock = vi.hoisted(() => ({
	getTracks: vi.fn(),
}));

vi.mock('$lib/cache/api_queries', () => ({
	cachedApi: cachedApiMock,
}));

import {
	loadTracks,
	totalTracks,
	tracks,
	updateLibraryTrackFavorite,
} from './library';

function track(overrides: Partial<Track> = {}): Track {
	return {
		id: 1,
		title: 'Album Row',
		artist_id: 2,
		artist_name: 'Artist',
		artist_tidal_id: null,
		album_id: 3,
		album_title: 'Favorited Album',
		album_tidal_id: null,
		disc_number: null,
		track_number: null,
		duration_ms: 180000,
		isrc: null,
		tidal_id: null,
		best_quality: null,
		best_source: null,
		fidelity_score: 0,
		is_favorite: true,
		play_count: 0,
		last_played_at: null,
		date_added: '2026-01-01T00:00:00Z',
		source: 'tidal',
		artwork_url: null,
		...overrides,
	};
}

describe('library track favorite reconciliation', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		tracks.set([]);
		totalTracks.set(0);
	});

	test('keeps unliked tracks in the legacy library-track list', async () => {
		cachedApiMock.getTracks.mockResolvedValueOnce({
			tracks: [track()],
			total: 1,
		});

		await loadTracks('date_added', 'desc', 100, 0, false);
		updateLibraryTrackFavorite(1, false);

		expect(get(tracks)).toEqual([expect.objectContaining({ id: 1, is_favorite: false })]);
		expect(get(totalTracks)).toBe(1);
	});

	test('removes unliked tracks from the strict liked list', async () => {
		cachedApiMock.getTracks.mockResolvedValueOnce({
			tracks: [track()],
			total: 1,
		});

		await loadTracks('date_added', 'desc', 100, 0, true);
		updateLibraryTrackFavorite(1, false);

		expect(get(tracks)).toEqual([]);
		expect(get(totalTracks)).toBe(0);
	});

	test('adds newly liked tracks to the current list optimistically', async () => {
		cachedApiMock.getTracks.mockResolvedValueOnce({
			tracks: [],
			total: 0,
		});

		await loadTracks('date_added', 'desc', 100, 0, true);
		updateLibraryTrackFavorite(2, true, track({ id: 2, title: 'Fresh Like', is_favorite: false }));

		expect(get(tracks)[0]).toEqual(expect.objectContaining({ id: 2, is_favorite: true }));
		expect(get(totalTracks)).toBe(1);
	});
});
