import { describe, expect, it, vi, beforeEach } from 'vitest';

// "Include new" weaves new songs only into library, liked and artist sessions,
// so those queue loads tell the server where they came from; albums,
// playlists and everything else send no context.
vi.mock('$lib/api/client', async () => {
	const replacePlaybackQueue = vi.fn(async () => ({
		queued_count: 0,
		pending_count: 0,
		state: null,
		queue: [],
	}));
	const track = (id: number) => ({ id, title: `Track ${id}` });
	return {
		api: {
			replacePlaybackQueue,
			getTracks: vi.fn(async () => ({ tracks: [track(1), track(2)] })),
			getArtistTracks: vi.fn(async () => ({ tracks: [track(3), track(4)] })),
			getAlbumTracks: vi.fn(async () => ({
				tracks: [track(5), track(6)],
				tidal_tracks: [],
				album_tidal_id: null,
				album_is_favorite: false,
			})),
			getPlaybackState: vi.fn(async () => ({ queue: [] })),
		},
		ApiError: class ApiError extends Error {},
	};
});

import { api } from '$lib/api/client';
import {
	playAlbum,
	playArtist,
	playLibrary,
	playTracksInContext,
	setPlayerStateForTests,
	shuffleArtist,
} from './player';

function sentContext() {
	const calls = vi.mocked(api.replacePlaybackQueue).mock.calls;
	expect(calls).toHaveLength(1);
	return calls[0][1]?.context;
}

describe('queue context for Include new', () => {
	beforeEach(() => {
		vi.mocked(api.replacePlaybackQueue).mockClear();
		setPlayerStateForTests({ lastSuccessfulCallAt: Date.now() });
	});

	it('marks library and liked playback', async () => {
		await playLibrary({ shuffle: true });
		expect(sentContext()).toBe('library');

		vi.mocked(api.replacePlaybackQueue).mockClear();
		await playLibrary({ likedOnly: true });
		expect(sentContext()).toBe('liked');
	});

	it('marks artist playback', async () => {
		await playArtist(7);
		expect(sentContext()).toBe('artist');

		vi.mocked(api.replacePlaybackQueue).mockClear();
		await shuffleArtist(7);
		expect(sentContext()).toBe('artist');
	});

	it('passes a list context through and sends none by default', async () => {
		await playTracksInContext([1, 2], 2, { context: 'liked' });
		expect(sentContext()).toBe('liked');

		vi.mocked(api.replacePlaybackQueue).mockClear();
		await playTracksInContext([1, 2]);
		expect(sentContext()).toBeUndefined();
	});

	it('sends no context for an album', async () => {
		await playAlbum(9);
		expect(sentContext()).toBeUndefined();
	});
});
