import { afterEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';

const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));

vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$lib/api/client', () => ({ api: {
	getTidalVideoStream: vi.fn().mockResolvedValue({ hls_url: 'https://example.test/stream.m3u8', expires_at: null, quality: 'HIGH' }),
	recordVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
} }));

import { buildVideoMenu } from '../src/lib/player/video_menu';
import { api, type TidalSearchVideo } from '../src/lib/api/client';
import { clearVideoSession, playVideo, videoSession } from '../src/lib/stores/video_session';

afterEach(() => { clearVideoSession(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

function video(tidal_id: number): TidalSearchVideo {
	return {
		tidal_id, title: `Clip ${tidal_id}`, artist_id: 42, artist_name: 'Artist', duration_ms: 240_000,
		artwork_url: 'https://example.test/poster.jpg', album_tidal_id: 50, quality: 'HIGH', explicit: false, type: 'Music Video',
	};
}

describe('video menu contracts', () => {
	test('card menu actions queue full video metadata without navigating or starting playback', () => {
		buildVideoMenu(video(1)).find(item => item.label === 'Add to queue')?.onSelect?.();
		buildVideoMenu(video(2)).find(item => item.label === 'Play next')?.onSelect?.();
		expect(get(videoSession).queue).toEqual([video(2), video(1)]);
		expect(get(videoSession).current).toBeNull();
		expect(goto).not.toHaveBeenCalled();
		expect(api.getTidalVideoStream).not.toHaveBeenCalled();
	});

	test('queue menu moves, removes, and plays videos using the live queue', async () => {
		const queue = [video(1), video(2), video(3)];
		await playVideo(queue[0], { queue, source: 'mix', sourceLabel: 'Shelf' },
			{ preloaded: { url: 'https://example.test/current.m3u8', expiresAt: null } });
		const menu = buildVideoMenu(queue[2], { inQueue: true });
		expect(menu.some(item => item.label === 'Add to queue')).toBe(false);
		menu.find(item => item.label === 'Move next')?.onSelect?.();
		expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 3, 2]);
		buildVideoMenu(queue[1], { inQueue: true }).find(item => item.label === 'Remove from queue')?.onSelect?.();
		expect(get(videoSession).queue.map(v => v.tidal_id)).toEqual([1, 3]);
		menu.find(item => item.label === 'Play video')?.onSelect?.();
		await vi.waitFor(() => expect(get(videoSession).loading).toBe(false));
		expect(get(videoSession).current?.tidal_id).toBe(3);
		expect(get(videoSession).sourceLabel).toBe('Shelf');
		expect(goto).not.toHaveBeenCalled();
	});
	test('video open action resolves inside NOORwave instead of opening TIDAL externally', () => {
		const open = vi.fn();
		vi.stubGlobal('window', { open });
		vi.stubGlobal('location', { origin: 'http://localhost:17601' });

		const items = buildVideoMenu({
			tidal_id: 12345,
			title: 'Live Clip',
			duration_ms: 1000,
			artist_id: 678,
			artist_name: 'Video Artist',
			album_tidal_id: null,
			artwork_url: null,
			quality: null,
			explicit: null,
			type: 'video',
		});

		const openItem = items.find((item) => item.label === 'Open video');
		expect(openItem).toBeDefined();
		openItem?.onSelect?.();

		expect(goto).toHaveBeenCalledWith('/videos?videoId=12345');
		expect(open).not.toHaveBeenCalled();
	});
});
