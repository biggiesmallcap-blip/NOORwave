import { afterEach, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api, type TidalSearchVideo } from '$lib/api/client';

import { advanceVideo, clearVideoSession, noteVideoProgress, playVideo, playVideoStation, radioRetryPolicy, refillVideoRadio, reportVideoEnded, videoSession, videoStationOnAir } from './video_session';

vi.mock('$lib/api/client', () => ({
	api: {
		getVideoRadioNext: vi.fn(),
		getVideoStationNext: vi.fn(),
		getTidalVideoStream: vi.fn(),
		recordVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
		finishVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
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
	radioRetryPolicy.delayMs = 4000;
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

const preloaded = { preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null } };

test('a station still being built is retried instead of ending radio', async () => {
	radioRetryPolicy.delayMs = 0;
	const seed = video(1, 10, 'Seed', 'Seed song');
	const found = video(2, 20, 'Neighbor', 'Neighbor song');
	const empty = { items: [], unfamiliar_video_ids: [], building: false };
	vi.mocked(api.getVideoRadioNext)
		.mockResolvedValue(empty)
		.mockResolvedValueOnce({ items: [], unfamiliar_video_ids: [], building: true })
		.mockResolvedValueOnce({ items: [], unfamiliar_video_ids: [], building: true })
		.mockResolvedValueOnce({ items: [found], unfamiliar_video_ids: [2], building: false });
	vi.mocked(api.getTidalVideoStream).mockResolvedValue({
		hls_url: 'https://example.test/next.m3u8', expires_at: null, quality: 'HIGH',
	});

	await playVideo(seed, {
		queue: [seed], source: 'direct', sourceLabel: 'Seed radio', autoplay: true,
		continuous: true, resetRadio: true,
	}, preloaded);
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));
	expect(await advanceVideo(preloaded)).toBe(true);
	expect(get(videoSession).current).toEqual(found);
	expect(get(videoSession).radioIssue).toBeNull();
	expect(api.getVideoRadioNext).toHaveBeenCalledWith(expect.objectContaining({ seed_video_id: 1 }));
});

test('watch time is reported when a video ends and when it is replaced', async () => {
	vi.mocked(api.recordVideoHistory)
		.mockResolvedValueOnce({ ok: true, id: 7 })
		.mockResolvedValueOnce({ ok: true, id: 8 })
		.mockResolvedValue({ ok: true, id: 9 });
	const first = video(1, 10, 'A', 'One');
	const second = video(2, 20, 'B', 'Two');
	const third = video(3, 30, 'C', 'Three');
	const ctx = { queue: [first, second, third], source: 'mix' as const, sourceLabel: 'Shelf', autoplay: true };

	await playVideo(first, ctx, preloaded);
	await new Promise((resolve) => setTimeout(resolve, 0));
	noteVideoProgress(170_000, 180_000);
	reportVideoEnded();
	expect(api.finishVideoHistory).toHaveBeenCalledWith(7, {
		watched_ms: 170_000, video_duration_ms: 180_000, completed: true,
	});

	await playVideo(second, ctx, preloaded);
	await new Promise((resolve) => setTimeout(resolve, 0));
	noteVideoProgress(5_000);
	await playVideo(third, ctx, preloaded);
	expect(api.finishVideoHistory).toHaveBeenLastCalledWith(8, {
		watched_ms: 5_000, video_duration_ms: 180_000, completed: false,
	});
	expect(api.finishVideoHistory).toHaveBeenCalledTimes(2);
});

test('a station refills from its own endpoint and ends with the station message', async () => {
	radioRetryPolicy.delayMs = 0;
	const first = video(1, 10, 'A', 'One');
	const second = video(2, 20, 'B', 'Two');
	vi.mocked(api.getVideoStationNext)
		.mockResolvedValueOnce({ items: [first], exhausted: false })
		.mockResolvedValueOnce({ items: [second], exhausted: false })
		.mockResolvedValue({ items: [], exhausted: true });
	vi.mocked(api.getTidalVideoStream).mockResolvedValue({
		hls_url: 'https://example.test/station.m3u8', expires_at: null, quality: 'HIGH',
	});

	expect(await playVideoStation({ id: 'wild-card', title: 'Wild card' })).toBe(true);
	await vi.waitFor(() => expect(get(videoSession).queue.map((v) => v.tidal_id)).toEqual([1, 2]));
	expect(api.getVideoRadioNext).not.toHaveBeenCalled();
	expect(vi.mocked(api.getVideoStationNext).mock.calls[1]).toEqual([
		'wild-card', expect.objectContaining({ exclude_video_ids: [1] }),
	]);
	expect(get(videoSession).sourceLabel).toBe('Wild card station');

	expect(await advanceVideo()).toBe(true);
	expect(get(videoSession).current?.tidal_id).toBe(2);
	expect(await advanceVideo()).toBe(false);
	videoSession.radioExhausted(2);
	expect(get(videoSession).radioIssue).toContain("You've seen everything in Wild card station");
});

test('a station started from a frame plays that video first and asks the station for the rest', async () => {
	const frame = video(7, 70, 'C', 'Frame');
	const rest = video(8, 80, 'D', 'Rest');
	vi.mocked(api.getVideoStationNext)
		.mockResolvedValueOnce({ items: [frame, rest], exhausted: false })
		.mockResolvedValue({ items: [], exhausted: true });
	vi.mocked(api.getTidalVideoStream).mockResolvedValue({
		hls_url: 'https://example.test/frame.m3u8', expires_at: null, quality: 'HIGH',
	});

	expect(await playVideoStation({ id: 'genre:rock', title: 'Rock' }, { startWith: frame })).toBe(true);
	expect(get(videoSession).current?.tidal_id).toBe(7);
	expect(get(videoSession).queue.map((v) => v.tidal_id).slice(0, 2)).toEqual([7, 8]);
	expect(vi.mocked(api.getVideoStationNext).mock.calls[0]).toEqual([
		'genre:rock', expect.objectContaining({ exclude_video_ids: [7] }),
	]);
	expect(get(videoStationOnAir)).toBe('genre:rock');

	clearVideoSession();
	expect(get(videoStationOnAir)).toBeNull();
});

test('a frame still plays when the station has nothing else yet', async () => {
	const frame = video(9, 90, 'E', 'Alone');
	vi.mocked(api.getVideoStationNext).mockResolvedValue({ items: [], exhausted: false });
	vi.mocked(api.getTidalVideoStream).mockResolvedValue({
		hls_url: 'https://example.test/alone.m3u8', expires_at: null, quality: 'HIGH',
	});

	expect(await playVideoStation({ id: 'live', title: 'Live and acoustic' }, { startWith: frame })).toBe(true);
	expect(get(videoSession).current?.tidal_id).toBe(9);
});
