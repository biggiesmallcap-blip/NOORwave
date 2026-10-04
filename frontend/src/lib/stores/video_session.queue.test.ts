import { afterEach, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api, type TidalSearchVideo } from '$lib/api/client';
import {
	addVideoToQueue, clearVideoSession, playQueuedVideo, playVideo,
	playVideoNext, refillVideoRadio, removeVideoFromQueue, videoSession, videoSessionUpcoming,
} from './video_session';

vi.mock('$lib/api/client', () => ({ api: {
	getTidalVideoStream: vi.fn().mockResolvedValue({ hls_url: 'https://example.test/next.m3u8', expires_at: null, quality: 'HIGH' }),
	recordVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
	getVideoRadioNext: vi.fn().mockResolvedValue({ items: [], unfamiliar_video_ids: [] }),
} }));

function video(tidal_id: number): TidalSearchVideo {
	return {
		tidal_id, title: `Video ${tidal_id}`, duration_ms: 180_000, artist_id: tidal_id * 10,
		artist_name: `Artist ${tidal_id}`, album_tidal_id: null, artwork_url: 'https://example.test/art.jpg',
		quality: null, explicit: null, type: 'Music Video',
	};
}

async function start(queue = [video(1), video(2), video(3)]) {
	await playVideo(queue[0], { queue, source: 'mix', sourceLabel: 'Shelf', autoplay: true },
		{ preloaded: { url: 'https://example.test/current.m3u8', expiresAt: null } });
	videoSession.setPosition(42_000);
	videoSession.setPlaying(true);
}

afterEach(() => { clearVideoSession(); vi.clearAllMocks(); });

test('adding and playing next preserve the current stream, position, and autoplay', async () => {
	await start();
	const before = get(videoSession);
	expect(addVideoToQueue(video(4))).toBe(true);
	expect(playVideoNext(video(5))).toBe(true);
	expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 5, 2, 3, 4]);
	expect(get(videoSession)).toMatchObject({
		current: before.current, streamUrl: before.streamUrl, positionMs: 42_000,
		playing: true, autoplay: true, sourceLabel: 'Shelf', continuous: false,
	});
	expect(api.getTidalVideoStream).not.toHaveBeenCalled();
});

test('play next moves an existing upcoming video without duplicating it', async () => {
	await start();
	expect(playVideoNext(video(3))).toBe(true);
	expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 3, 2]);
	expect(addVideoToQueue(video(2))).toBe(false);
	expect(playVideoNext(video(1))).toBe(false);
	expect(get(videoSession).currentIndex).toBe(0);
});

test('videos can be queued before playback without fetching or starting a stream', async () => {
	addVideoToQueue(video(2));
	playVideoNext(video(1));
	expect(get(videoSession).active).toBe(false);
	expect(get(videoSession).current).toBeNull();
	expect(get(videoSessionUpcoming).map(v => v.tidal_id)).toEqual([1, 2]);
	expect(api.getTidalVideoStream).not.toHaveBeenCalled();
	expect(await playQueuedVideo(1)).toBe(true);
	expect(get(videoSession).current?.tidal_id).toBe(1);
	expect(get(videoSessionUpcoming).map(v => v.tidal_id)).toEqual([2]);
});

test('queue playback preserves the existing queue independently of route search results', async () => {
	await start();
	addVideoToQueue(video(4));
	expect(await playQueuedVideo(2)).toBe(true);
	expect(get(videoSession).current?.tidal_id).toBe(2);
	expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 2, 3, 4]);
	expect(get(videoSession).sourceLabel).toBe('Shelf');
	expect(await playQueuedVideo(999)).toBe(false);
});

test('removing an upcoming video leaves current playback and the rest of the queue intact', async () => {
	await start();
	expect(removeVideoFromQueue(2)).toBe(true);
	expect(removeVideoFromQueue(1)).toBe(false);
	expect(removeVideoFromQueue(999)).toBe(false);
	expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 3]);
	expect(get(videoSession).positionMs).toBe(42_000);
	expect(api.getTidalVideoStream).not.toHaveBeenCalled();
});

test('queue additions repair a current video missing from its browse queue', async () => {
	await playVideo(video(1), { queue: [], source: 'search', sourceLabel: 'Search' },
		{ preloaded: { url: 'https://example.test/current.m3u8', expiresAt: null } });
	playVideoNext(video(2));
	addVideoToQueue(video(3));
	expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 2, 3]);
	expect(get(videoSession).currentIndex).toBe(0);
});

test('an in-flight radio refill preserves manual ordering and does not restore removed videos', async () => {
	let deliver!: (value: { items: TidalSearchVideo[]; unfamiliar_video_ids: number[] }) => void;
	vi.mocked(api.getVideoRadioNext).mockImplementationOnce(() => new Promise(resolve => { deliver = resolve; }));
	const queue = [video(1), video(2), video(3)];
	await playVideo(queue[0], {
		queue, source: 'mix', sourceLabel: 'Video radio', autoplay: true,
		continuous: true, resetRadio: true, radioScope: 'library',
	}, { preloaded: { url: 'https://example.test/current.m3u8', expiresAt: null } });
	addVideoToQueue(video(4));
	playVideoNext(video(5));
	removeVideoFromQueue(2);
	deliver({ items: [video(2), video(4), video(6)], unfamiliar_video_ids: [6] });
	await refillVideoRadio();
	expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 5, 3, 4, 6]);
	expect(get(videoSession).continuous).toBe(true);
	expect(get(videoSession).radioSeedArtistId).toBeNull();
});
