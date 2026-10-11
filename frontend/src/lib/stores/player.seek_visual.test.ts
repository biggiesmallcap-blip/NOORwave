import { get } from 'svelte/store';
import { beforeEach, describe, expect, test, vi } from 'vitest';
import type { PlaybackState } from '$lib/api/client';

vi.mock('$lib/api/client', () => ({
	api: { setPlaybackPosition: vi.fn(), getTrackAudioFeatures: vi.fn(async () => ({ features: null })) },
	ApiError: class ApiError extends Error { status = 409; body = null; }
}));

import { api } from '$lib/api/client';
import { playbackSeekRevision, setPlayerPosition, setPlayerStateForTests } from './player';

const state = {
	current_track: null, current_queue_item_id: null, is_playing: false, position_ms: 5000,
	buffered_ms: 0, volume: 0.5, shuffle_mode: 'off', repeat_mode: 'off', crossfade_ms: 0,
	automix_enabled: false, automix_discover_new: false, automix_use_learning: true, automix_allow_external: false
} as PlaybackState;

beforeEach(() => { vi.mocked(api.setPlaybackPosition).mockReset(); setPlayerStateForTests({ playbackSeekRevision: 0 }); });

describe('accepted seeks invalidate a cached DJ animation', () => {
	test('a successful seek advances the revision before the next DJ status poll', async () => {
		vi.mocked(api.setPlaybackPosition).mockResolvedValue({ state });
		await setPlayerPosition(5000);
		expect(get(playbackSeekRevision)).toBe(1);
	});
	test('failed seek requests do not invalidate audio that is still playing', async () => {
		vi.mocked(api.setPlaybackPosition).mockRejectedValue(new Error('500'));
		await setPlayerPosition(5000);
		expect(get(playbackSeekRevision)).toBe(0);
	});
	test('an older accepted response cannot overwrite a newer seek revision', async () => {
		let resolveFirst!: (value: { state: PlaybackState }) => void;
		vi.mocked(api.setPlaybackPosition).mockImplementationOnce(() => new Promise((resolve) => { resolveFirst = resolve; }))
			.mockResolvedValueOnce({ state });
		const first = setPlayerPosition(1000);
		await setPlayerPosition(5000);
		resolveFirst({ state: { ...state, position_ms: 1000 } });
		await first;
		expect(get(playbackSeekRevision)).toBe(1);
	});
});
