import { afterEach, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api, type TidalSearchVideo } from '$lib/api/client';
import { advanceVideo, clearVideoSession, playVideo, refillVideoRadio, videoSession } from './video_session';

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

test('a shelf queue plays every artist in order and stops without starting radio', async () => {
	const shelf = [
		video(1, 10, 'Holy Fuck', 'Elevate'),
		video(2, 20, 'Jimmy Cliff', 'Life'),
		video(3, 30, 'JAY-Z', 'Dead Presidents'),
	];
	vi.mocked(api.getTidalVideoStream).mockResolvedValue({
		hls_url: 'https://example.test/stream.m3u8', expires_at: null, quality: 'HIGH',
	});

	await playVideo(shelf[0], {
		queue: shelf, source: 'mix', sourceLabel: 'Your library, on camera', autoplay: true,
	});
	expect(get(videoSession).queue).toEqual(shelf);
	expect(get(videoSession).continuous).toBe(false);
	expect(await advanceVideo()).toBe(true);
	expect(get(videoSession).current).toEqual(shelf[1]);
	expect(await advanceVideo()).toBe(true);
	expect(get(videoSession).current).toEqual(shelf[2]);
	expect(await advanceVideo()).toBe(false);
	expect(api.getVideoRadioNext).not.toHaveBeenCalled();
});

test('library radio keeps a mixed opening queue and appends even a large discovery batch', async () => {
	const picks = [
		video(1, 10, 'Holy Fuck', 'Elevate'),
		video(2, 20, 'Jimmy Cliff', 'Life'),
		video(3, 30, 'JAY-Z', 'Dead Presidents'),
		video(4, 40, 'Bob Dylan', 'Newport'),
	];
	const discoveries = Array.from({ length: 8 }, (_, index) =>
		video(100 + index, 1000 + index, `Related artist ${index}`, `Related video ${index}`));
	vi.mocked(api.getVideoRadioNext).mockResolvedValueOnce({
		items: [picks[1], ...discoveries], unfamiliar_video_ids: discoveries.map((item) => item.tidal_id),
	});

	await playVideo(picks[0], {
		queue: picks, source: 'mix', sourceLabel: 'Video radio', autoplay: true,
		continuous: true, resetRadio: true, radioScope: 'library',
	}, { preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null } });
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));
	expect(get(videoSession).queue).toEqual([...picks, ...discoveries]);
	expect(api.getVideoRadioNext).toHaveBeenCalledWith(expect.objectContaining({
		seed_artist_id: null, seed_artist_name: null, exclude_video_ids: [1, 2, 3, 4],
	}));

	await advanceVideo({ preloaded: { url: 'https://example.test/next.m3u8', expiresAt: null } });
	expect(get(videoSession).current).toEqual(picks[1]);
	expect(get(videoSession).radioSeedArtistId).toBeNull();
	const more = video(200, 2000, 'Another artist', 'Another video');
	vi.mocked(api.getVideoRadioNext).mockResolvedValueOnce({ items: [more], unfamiliar_video_ids: [200] });
	await refillVideoRadio(true);
	expect(get(videoSession).queue).toEqual([...picks, ...discoveries, more]);
});

test('library radio starts on an already playing video without losing the other picks', async () => {
	const seed = video(1, 10, 'Holy Fuck', 'Elevate');
	const next = video(2, 20, 'Jimmy Cliff', 'Life');
	vi.mocked(api.getVideoRadioNext).mockResolvedValueOnce({ items: [], unfamiliar_video_ids: [] });
	await playVideo(seed, {
		queue: [seed], source: 'direct', sourceLabel: 'Video', autoplay: false,
	}, { preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null } });
	await playVideo(seed, {
		queue: [seed, next], source: 'mix', sourceLabel: 'Video radio', autoplay: true,
		continuous: true, resetRadio: true, radioScope: 'library',
	});
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));
	expect(get(videoSession).queue).toEqual([seed, next]);
	expect(get(videoSession).autoplay).toBe(true);
	expect(api.getTidalVideoStream).not.toHaveBeenCalled();
});

test('a late radio response cannot replace a newly selected shelf queue', async () => {
	const seed = video(1, 10, 'Holy Fuck', 'Elevate');
	const shelf = [video(2, 20, 'Jimmy Cliff', 'Life'), video(3, 30, 'JAY-Z', 'Dead Presidents')];
	let deliver!: (value: { items: TidalSearchVideo[]; unfamiliar_video_ids: number[] }) => void;
	vi.mocked(api.getVideoRadioNext).mockImplementationOnce(() => new Promise((resolve) => { deliver = resolve; }));
	await playVideo(seed, {
		queue: [seed], source: 'direct', sourceLabel: 'Artist radio',
		autoplay: true, continuous: true, resetRadio: true,
	}, { preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null } });
	await playVideo(shelf[0], {
		queue: shelf, source: 'mix', sourceLabel: 'Shelf', autoplay: true,
	}, { preloaded: { url: 'https://example.test/shelf.m3u8', expiresAt: null } });
	deliver({ items: [video(4, 10, 'Holy Fuck', 'Evie')], unfamiliar_video_ids: [] });
	await refillVideoRadio();
	expect(get(videoSession).queue).toEqual(shelf);
	expect(get(videoSession).continuous).toBe(false);
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

test('radio reports only videos actually added to the queue', async () => {
	const seed = video(1, 10, 'Green Day', 'American Idiot');
	const blink = video(2, 20, 'Blink-182', 'Dammit');
	const blinkAgain = video(3, 20, 'Blink-182', 'All the Small Things');
	let deliver!: (value: { items: TidalSearchVideo[]; unfamiliar_video_ids: number[] }) => void;
	vi.mocked(api.getVideoRadioNext).mockImplementationOnce(() => new Promise((resolve) => { deliver = resolve; }));

	await playVideo(seed, {
		queue: [seed], source: 'direct', sourceLabel: 'Green Day radio',
		autoplay: true, continuous: true, resetRadio: true,
	}, { preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null } });
	expect(get(videoSession).radioSearching).toBe(true);
	deliver({ items: [seed, blink, blinkAgain], unfamiliar_video_ids: [2, 3] });
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));
	expect(get(videoSession).radioHits).toEqual([{ artist: 'Blink-182', count: 2 }]);
	expect(get(videoSession).radioDiscoveryMessage).toBe('2 new videos added to your queue.');
	videoSession.stopRadio();
	expect(get(videoSession).radioHits).toEqual([]);
});
