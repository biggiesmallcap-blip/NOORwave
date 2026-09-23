import { afterEach, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api, type TidalSearchVideo } from '$lib/api/client';
import { clearVideoSession, playVideo, videoSession } from './video_session';

vi.mock('$lib/api/client', () => ({
	api: {
		getVideoRadioNext: vi.fn(),
		getTidalVideoStream: vi.fn(),
		recordVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
	},
}));

function video(id: number, artistId: number, artistName: string, title: string): TidalSearchVideo {
	return {
		tidal_id: id, title, duration_ms: 180_000, artist_id: artistId,
		artist_name: artistName, album_tidal_id: null, artwork_url: null,
		quality: null, explicit: null, type: 'Music Video',
	};
}

afterEach(() => {
	clearVideoSession();
	vi.clearAllMocks();
});

test('artist radio drops an unrelated shelf, keeps its seed, and skips alternate cuts', async () => {
	const greenDay = video(1, 10, 'Green Day', 'Basket Case (Live)');
	const unrelated = video(2, 99, 'James Blake', 'Retrograde');
	const blink = video(3, 20, 'Blink-182', 'All the Small Things');
	const alternate = video(4, 10, 'Green Day', 'Basket Case [Visualizer]');
	const holiday = video(5, 10, 'Green Day', 'Holiday');
	vi.mocked(api.getVideoRadioNext)
		.mockResolvedValueOnce({ items: [blink], unfamiliar_video_ids: [blink.tidal_id] })
		.mockResolvedValueOnce({ items: [alternate, holiday], unfamiliar_video_ids: [] });

	await playVideo(greenDay, {
		queue: [greenDay, unrelated], source: 'mix', sourceLabel: 'Green Day radio',
		autoplay: true, continuous: true, resetRadio: true,
	}, { preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null } });
	await vi.waitFor(() => expect(get(videoSession).queue.map((item) => item.tidal_id)).toEqual([1, 3]));
	// The first refill updates the queue before its cleanup callback releases the request slot.
	await new Promise((resolve) => setTimeout(resolve, 0));
	expect(vi.mocked(api.getVideoRadioNext).mock.calls[0][0].seed_artist_id).toBe(10);
	expect(get(videoSession).queue.some((item) => item.artist_id === 99)).toBe(false);

	await playVideo(blink, {
		queue: get(videoSession).queue, source: 'mix', sourceLabel: 'Green Day radio',
		autoplay: true, continuous: true,
	}, { preloaded: { url: 'https://example.test/next.m3u8', expiresAt: null } });
	await vi.waitFor(() => expect(vi.mocked(api.getVideoRadioNext)).toHaveBeenCalledTimes(2));
	await vi.waitFor(() => expect(get(videoSession).queue.some((item) => item.tidal_id === 5)).toBe(true));
	expect(vi.mocked(api.getVideoRadioNext).mock.calls[1][0].seed_artist_id).toBe(10);
	expect(get(videoSession).queue.some((item) => item.tidal_id === 4)).toBe(false);
});

test('starting radio on the already playing video requests a related queue', async () => {
	const greenDay = video(1, 10, 'Green Day', 'Basket Case');
	const blink = video(3, 20, 'Blink-182', 'All the Small Things');
	vi.mocked(api.getVideoRadioNext).mockResolvedValue({ items: [blink], unfamiliar_video_ids: [blink.tidal_id] });

	await playVideo(greenDay, {
		queue: [greenDay], source: 'direct', sourceLabel: 'Video', autoplay: false,
	}, { preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null } });
	await playVideo(greenDay, {
		queue: [greenDay], source: 'direct', sourceLabel: 'Green Day radio',
		autoplay: true, continuous: true, resetRadio: true,
	});

	await vi.waitFor(() => expect(get(videoSession).queue.map((item) => item.tidal_id)).toEqual([1, 3]));
	expect(vi.mocked(api.getVideoRadioNext).mock.calls[0][0].seed_artist_id).toBe(10);
});
