import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api } from '$lib/api/client';
import { fixtureTune, fixtureVideo } from '$lib/video/onair/fixtures';
import { clearVideoSession, nextVideo, playVideoChannel, previousVideo, videoChannelOnAir, videoSession } from './video_session';

vi.mock('$lib/api/client', () => ({
	api: {
		tuneVideoChannel: vi.fn(),
		getVideoChannelNext: vi.fn(),
		getTidalVideoStream: vi.fn(),
		recordVideoHistory: vi.fn().mockResolvedValue({ id: 1 }),
		finishVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
	},
}));

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((done) => {
		resolve = done;
	});
	return { promise, resolve };
}

beforeEach(() => {
	vi.mocked(api.getTidalVideoStream).mockResolvedValue({ hls_url: 'https://example.test/video.m3u8', expires_at: null, quality: 'HIGH' });
	vi.mocked(api.getVideoChannelNext).mockResolvedValue({ items: [], exhausted: false });
});

afterEach(() => {
	clearVideoSession();
	vi.clearAllMocks();
});

describe('tuning in to a channel', () => {
	test('the newest tune wins when answers arrive out of order', async () => {
		const slow = deferred<ReturnType<typeof fixtureTune>>();
		vi.mocked(api.tuneVideoChannel)
			.mockReturnValueOnce(slow.promise)
			.mockResolvedValueOnce({
				...fixtureTune(0),
				channel: { ...fixtureTune(0).channel, id: 'wild', title: 'Wild Card' },
				video: fixtureVideo(900, 90, 'New', 'New'),
			});
		const earlier = playVideoChannel('scene:7');
		await playVideoChannel('wild');
		slow.resolve(fixtureTune(0));
		expect(await earlier).toBe(false);
		expect(get(videoChannelOnAir)?.id).toBe('wild');
	});

	test('Stop cancels a tune whose schedule is still loading', async () => {
		const slow = deferred<ReturnType<typeof fixtureTune>>();
		vi.mocked(api.tuneVideoChannel).mockReturnValueOnce(slow.promise);
		const earlier = playVideoChannel('scene:7');
		clearVideoSession();
		slow.resolve(fixtureTune(0));
		expect(await earlier).toBe(false);
		expect(get(videoSession).active).toBe(false);
	});

	test('a video scheduled twice does not send the queue cursor back', async () => {
		const a = fixtureVideo(201, 21, 'Artist', 'A');
		const b = fixtureVideo(202, 21, 'Artist', 'B');
		const c = fixtureVideo(203, 21, 'Artist', 'C');
		vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce({ ...fixtureTune(0), video: a, queue: [b, c, b, c] });
		await playVideoChannel('spotlight');
		await nextVideo();
		await nextVideo();
		await nextVideo();
		let state = get(videoSession);
		expect(state.currentIndex).toBe(3);
		expect(state.queue.slice(state.currentIndex + 1).map((video) => video.tidal_id)).toEqual([203]);
		// And back again, one step at a time.
		await previousVideo();
		state = get(videoSession);
		expect(state.currentIndex).toBe(2);
		expect(state.current?.tidal_id).toBe(203);
	});
});
