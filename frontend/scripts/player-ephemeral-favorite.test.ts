import { get } from 'svelte/store';
import { beforeEach, describe, expect, test, vi } from 'vitest';
import type { Track } from '../src/lib/api/client';

const apiMock = vi.hoisted(() => ({
	getTrackAudioFeatures: vi.fn().mockResolvedValue({ features: null }),
	importTidalTrackForRadio: vi.fn(),
	playTidalTrack: vi.fn().mockResolvedValue({}),
	setTrackFavorite: vi.fn().mockResolvedValue({}),
}));

vi.mock('$lib/api/client', async (importOriginal) => {
	const actual = await importOriginal<typeof import('../src/lib/api/client')>();
	return {
		...actual,
		api: apiMock,
		ApiError: class ApiError extends Error {},
	};
});

vi.mock('$lib/stores/exclusive_status', () => ({
	setExclusiveEngaged: vi.fn(),
	setExclusiveReleased: vi.fn(),
}));

vi.mock('$lib/stores/toast', () => ({
	dismissToast: vi.fn(),
	showToast: vi.fn(() => 'toast-id'),
}));

vi.mock('$lib/api/ws', async () => {
	const { writable } = await import('svelte/store');
	return { wsConnected: writable(true) };
});

vi.mock('$lib/stores/library', () => ({
	updateLibraryTrackFavorite: vi.fn(),
}));

import { currentTrack, hydratePlayback, playTidalTrackNow, toggleTrackFavorite } from '../src/lib/stores/player';

function ephemeralTrack(): Track {
	return {
		id: -222,
		title: 'Ephemeral Song',
		artist_id: -1,
		artist_name: 'TIDAL Artist',
		artist_tidal_id: 333,
		album_id: null,
		album_title: 'TIDAL Album',
		album_tidal_id: 444,
		disc_number: null,
		track_number: null,
		duration_ms: 180000,
		isrc: null,
		tidal_id: 222,
		best_quality: 'LOSSLESS',
		best_source: 'tidal',
		fidelity_score: 0,
		is_favorite: false,
		play_count: 0,
		last_played_at: null,
		date_added: null,
		source: 'tidal_ephemeral',
		artwork_url: 'https://example.test/art.jpg',
	};
}

describe('ephemeral TIDAL favorite import', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		apiMock.getTrackAudioFeatures.mockResolvedValue({ features: null });
		apiMock.importTidalTrackForRadio.mockResolvedValue({
			tidal_id: 222,
			local_id: 98,
			artist_id: 77,
			album_id: 66,
		});
		apiMock.playTidalTrack.mockResolvedValue({});
		apiMock.setTrackFavorite.mockResolvedValue({});
		currentTrack.set(null);
	});

	test('preserves TIDAL artist and album IDs when liking an ephemeral track', async () => {
		currentTrack.set(ephemeralTrack());

		await toggleTrackFavorite(-222, false);

		expect(apiMock.importTidalTrackForRadio).toHaveBeenCalledWith(
			expect.objectContaining({
				tidal_id: 222,
				artist_tidal_id: 333,
				album_tidal_id: 444,
			})
		);
		expect(apiMock.setTrackFavorite).toHaveBeenCalledWith(98, true);
		expect(get(currentTrack)?.id).toBe(98);
		expect(get(currentTrack)?.artist_id).toBe(77);
		expect(get(currentTrack)?.album_id).toBe(66);
	});

	test('keeps the liked state when playback hydrates the saved track', async () => {
		await playTidalTrackNow({
			tidal_id: 222,
			title: 'Ephemeral Song',
			artist_name: 'TIDAL Artist',
			artist_tidal_id: 333,
			album_title: 'TIDAL Album',
			album_tidal_id: 444,
			artwork_url: 'https://example.test/art.jpg',
			duration_ms: 180000,
			is_favorite: false,
		});

		await toggleTrackFavorite(-222, false);

		const savedTrack = {
			...ephemeralTrack(),
			id: 98,
			artist_id: 77,
			album_id: 66,
			is_favorite: true,
			source: 'tidal',
		};
		hydratePlayback({
			state: {
				current_track: savedTrack,
				current_queue_item_id: null,
				position_ms: 0,
				is_playing: true,
				volume: 1,
				shuffle_mode: 'off',
				repeat_mode: 'one',
				automix_enabled: false,
				crossfade_ms: 0,
				automix_discover_new: false,
				automix_use_learning: false,
				automix_allow_external: false,
			},
			queue: [],
		});

		expect(get(currentTrack)?.is_favorite).toBe(true);
	});
});
