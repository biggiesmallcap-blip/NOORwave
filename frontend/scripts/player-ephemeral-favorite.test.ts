import { get } from 'svelte/store';
import { beforeEach, describe, expect, test, vi } from 'vitest';
import type { Track } from '../src/lib/api/client';

const apiMock = vi.hoisted(() => ({
	getTrackAudioFeatures: vi.fn().mockResolvedValue({ features: null }),
	replacePlaybackQueue: vi.fn(),
	setTrackFavorite: vi.fn().mockResolvedValue({}),
	setPlaybackPosition: vi.fn(),
	getPlaybackState: vi.fn(),
}));

vi.mock('$lib/api/client', async (importOriginal) => {
	const actual = await importOriginal<typeof import('../src/lib/api/client')>();
	return { ...actual, api: apiMock };
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

vi.mock('$lib/stores/library', () => ({ updateLibraryTrackFavorite: vi.fn() }));

import { ApiError } from '../src/lib/api/client';
import {
	buffered,
	currentTrack,
	hydratePlayback,
	playerError,
	playTidalTrackNow,
	position,
	setPlayerPosition,
	toggleTrackFavorite,
} from '../src/lib/stores/player';

function streamedTrack(): Track {
	return {
		id: 98,
		title: 'Streamed Song',
		artist_id: 77,
		artist_name: 'TIDAL Artist',
		artist_tidal_id: 333,
		album_id: 66,
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
		source: 'tidal_stream',
		artwork_url: 'https://example.test/art.jpg',
	};
}

function snapshot(track: Track) {
	return {
		queued_count: 1,
		pending_count: 0,
		state: {
			current_track: track,
			current_queue_item_id: 1,
			position_ms: 0,
			is_playing: true,
			volume: 0.8,
			shuffle_mode: 'off',
			repeat_mode: 'off',
			automix_enabled: false,
			crossfade_ms: 0,
			automix_discover_new: false,
			automix_use_learning: false,
			automix_allow_external: false,
		},
		queue: [],
	};
}

describe('canonical streamed TIDAL playback', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		apiMock.getTrackAudioFeatures.mockResolvedValue({ features: null });
		apiMock.replacePlaybackQueue.mockResolvedValue(snapshot(streamedTrack()));
		apiMock.setTrackFavorite.mockResolvedValue({});
		currentTrack.set(null);
		position.set(0);
		buffered.set(0);
	});

	test('uses the canonical queue route and resets playback progress', async () => {
		position.set(64_000);
		buffered.set(90_000);
		await playTidalTrackNow({
			tidal_id: 777,
			title: 'Fresh Start',
			artist_name: 'TIDAL Artist',
			artist_tidal_id: 333,
			album_title: 'TIDAL Album',
			album_tidal_id: 444,
			artwork_url: 'https://example.test/fresh.jpg',
			duration_ms: 180_000,
			is_favorite: false,
		});

		expect(apiMock.replacePlaybackQueue).toHaveBeenCalledWith(
			expect.arrayContaining([expect.objectContaining({ tidal_id: 777 })]),
			{ startPlayback: true }
		);
		expect(get(currentTrack)?.id).toBe(98);
		expect(get(position)).toBe(0);
		expect(get(buffered)).toBe(0);
	});

	test('likes the resolved transient row without a synthetic id', async () => {
		currentTrack.set(streamedTrack());
		await toggleTrackFavorite(98, false);
		expect(apiMock.setTrackFavorite).toHaveBeenCalledWith(98, true);
		expect(get(currentTrack)?.is_favorite).toBe(true);
	});
});
describe('setPlayerPosition 409 ack handling', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		// Reset any state the SUT touches so each test starts fresh.
		currentTrack.set(null);
		position.set(0);
		buffered.set(0);
		playerError.set(null);
	});

	function buildState(overrides: Record<string, unknown> = {}) {
		// The route-side 409 ack returns a live snapshot - this is what the
		// frontend MUST apply (so the scrubber snaps to the real position
		// instead of holding the user's drag target).
		return {
			current_track: null,
			current_queue_item_id: null,
			position_ms: 12_345,
			is_playing: true,
			volume: 0.7,
			shuffle_mode: 'off',
			repeat_mode: 'off',
			automix_enabled: false,
			crossfade_ms: 0,
			automix_discover_new: false,
			automix_use_learning: false,
			automix_allow_external: false,
			buffered_ms: 30_000,
			...overrides,
		};
	}

	test('applies the 409 body and skips the error toast', async () => {
		const liveState = buildState();
		apiMock.setPlaybackPosition.mockRejectedValueOnce(
			new ApiError(409, 'seek past buffered region', { state: liveState })
		);

		await setPlayerPosition(120_000);

		// State from the 409 body must land in the stores.
		expect(get(position)).toBe(12_345);
		expect(get(buffered)).toBe(30_000);
		// No error toast: the rejection is expected and the corrective
		// snapshot is authoritative.
		expect(get(playerError)).toBeNull();
	});

	test('routes other non-2xx into the error path', async () => {
		apiMock.setPlaybackPosition.mockRejectedValueOnce(
			new ApiError(500, 'server boom', null)
		);

		await setPlayerPosition(120_000);

		expect(get(playerError)).not.toBeNull();
		expect(get(playerError)?.message).toContain('seek');
	});

	test('falls through to error path when 409 body is missing state', async () => {
		// Defensive: backend could return 409 with an unexpected shape; we
		// MUST NOT silently swallow it - the user should see the error.
		apiMock.setPlaybackPosition.mockRejectedValueOnce(
			new ApiError(409, 'rejected', { reason: 'unparseable' })
		);

		await setPlayerPosition(120_000);

		expect(get(playerError)).not.toBeNull();
	});

	test('opts in to segment-seek (option C) on every seek request', async () => {
		// Pin the API contract: setPlayerPosition MUST send
		// allow_segment_seek=true. Drops to the legacy 409 reject path if
		// this regresses to false, so the failure mode would be silent UX
		// degradation rather than a crash - the test backstops that.
		apiMock.setPlaybackPosition.mockResolvedValueOnce({ state: buildState() });

		await setPlayerPosition(75_000);

		expect(apiMock.setPlaybackPosition).toHaveBeenCalledWith(75_000, true);
	});
});
