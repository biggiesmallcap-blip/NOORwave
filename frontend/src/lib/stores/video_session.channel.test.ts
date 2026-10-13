import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api } from '$lib/api/client';
import { fixtureTune, fixtureVideo } from '$lib/video/onair/fixtures';
import {
	clearVideoSession,
	nextVideo,
	noteChannelTaste,
	noteVideoProgress,
	playVideo,
	playVideoChannel,
	refillVideoRadio,
	steerChannel,
	steerMessage,
	videoChannelOnAir,
	videoSession,
} from './video_session';

vi.mock('$lib/api/client', () => ({
	api: {
		tuneVideoChannel: vi.fn(),
		getVideoChannelNext: vi.fn(),
		steerVideoChannel: vi.fn(),
		getVideoRadioNext: vi.fn(),
		getVideoStationNext: vi.fn(),
		getTidalVideoStream: vi.fn(),
		recordVideoHistory: vi.fn().mockResolvedValue({ id: 1 }),
		finishVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
	},
}));

beforeEach(() => {
	vi.mocked(api.getTidalVideoStream).mockResolvedValue({ hls_url: 'https://example.test/stream.m3u8', expires_at: null, quality: 'HIGH' });
	vi.mocked(api.getVideoChannelNext).mockResolvedValue({ items: [], exhausted: false });
});

afterEach(() => {
	clearVideoSession();
	vi.clearAllMocks();
});

test('tune-in lands mid-video at the scheduled offset with the schedule queued', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	expect(await playVideoChannel('scene:7', { explore: 'adventurous' })).toBe(true);
	expect(api.tuneVideoChannel).toHaveBeenCalledWith('scene:7', { explore: 'adventurous' });
	const state = get(videoSession);
	expect(state.current?.tidal_id).toBe(201);
	expect(state.startAtS).toBe(95);
	expect(state.positionMs).toBe(95_000);
	expect(state.queue.map((video) => video.tidal_id)).toEqual([201, 202, 203, 204]);
	expect(state.continuous).toBe(true);
	expect(state.autoplay).toBe(true);
	expect(state.sourceLabel).toBe('12 Bay Area Rap');
	expect(get(videoChannelOnAir)?.id).toBe('scene:7');
});

test('a zap right after tuning in mid-video is reported as a zap', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	await vi.waitFor(() => expect(api.recordVideoHistory).toHaveBeenCalled());
	await Promise.resolve();
	// Two seconds of watching, from 1:35 to 1:37.
	noteVideoProgress(97_000, 210_000);
	clearVideoSession();
	await vi.waitFor(() => expect(api.finishVideoHistory).toHaveBeenCalled());
	expect(api.finishVideoHistory).toHaveBeenCalledWith(1, { watched_ms: 2000, video_duration_ms: 210_000, completed: false });
});

test('past 80% of the scheduled video the next item starts from the top', async () => {
	// Mac Dre runs 210 s; 180 s in is past 80%.
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(180));
	await playVideoChannel('scene:7');
	const state = get(videoSession);
	expect(state.current?.tidal_id).toBe(202);
	expect(state.startAtS).toBe(0);
	expect(state.queue.map((video) => video.tidal_id)).toEqual([202, 203, 204]);
});

test('past 80% with nothing after it, it joins the tail anyway', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce({ ...fixtureTune(180), queue: [] });
	await playVideoChannel('scene:7');
	expect(get(videoSession).current?.tidal_id).toBe(201);
	expect(get(videoSession).startAtS).toBe(180);
});

test('a step to the next video starts it at 0:00', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	await nextVideo();
	expect(get(videoSession).current?.tidal_id).toBe(202);
	expect(get(videoSession).startAtS).toBe(0);
});

test('refills come from the channel with the session nonce and the more/less artists', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	await vi.waitFor(() => expect(api.getVideoChannelNext).toHaveBeenCalled());
	const first = vi.mocked(api.getVideoChannelNext).mock.calls[0];
	expect(first[0]).toBe('scene:7');
	expect(first[1].session_nonce).toMatch(/\w+/);
	expect(first[1].exclude_video_ids).toEqual(expect.arrayContaining([201, 202, 203, 204]));
	expect(first[1].more_artist_ids).toEqual([]);
	expect(first[1].less_artist_ids).toEqual([]);
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));

	noteChannelTaste(22, true);
	noteChannelTaste(23, false);
	vi.mocked(api.getVideoChannelNext).mockClear();
	vi.mocked(api.getVideoChannelNext).mockResolvedValueOnce({
		items: [fixtureVideo(205, 24, 'Mistah F.A.B.', 'Ghost Ride It', 230)], exhausted: false,
	});
	await refillVideoRadio(true);
	const second = vi.mocked(api.getVideoChannelNext).mock.calls[0];
	expect(second[1].session_nonce).toBe(first[1].session_nonce);
	expect(second[1].more_artist_ids).toEqual([22]);
	expect(second[1].less_artist_ids).toEqual([23]);
	expect(get(videoSession).queue.map((video) => video.tidal_id)).toContain(205);
	expect(api.getVideoRadioNext).not.toHaveBeenCalled();
	expect(api.getVideoStationNext).not.toHaveBeenCalled();
});

test('"less like this" drops that artist\'s upcoming picks, keeps the one playing', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));
	// Mac Dre (21) is playing and also queued as 204.
	expect(noteChannelTaste(21, false)).toBe(1);
	const state = get(videoSession);
	expect(state.current?.tidal_id).toBe(201);
	expect(state.queue.map((video) => video.tidal_id)).toEqual([201, 202, 203]);
	// Changing your mind moves the artist between the lists.
	expect(noteChannelTaste(21, true)).toBe(0);
	await refillVideoRadio(true);
	const call = vi.mocked(api.getVideoChannelNext).mock.calls.at(-1)!;
	expect(call[1].more_artist_ids).toEqual([21]);
	expect(call[1].less_artist_ids).toEqual([]);
});

test('picking a shelf afterwards takes the session off the channel', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	expect(get(videoChannelOnAir)?.id).toBe('scene:7');
	const shelf = [fixtureVideo(900, 90, 'Someone', 'Else')];
	await playVideo(shelf[0], { queue: shelf, source: 'mix', sourceLabel: 'A shelf', autoplay: true });
	expect(get(videoChannelOnAir)).toBeNull();
	expect(get(videoSession).startAtS).toBe(0);
});

test('ending the session clears the channel and the taste presses', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	noteChannelTaste(22, true);
	clearVideoSession();
	expect(get(videoChannelOnAir)).toBeNull();
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(10));
	await playVideoChannel('scene:7');
	await vi.waitFor(() => expect(api.getVideoChannelNext).toHaveBeenCalled());
	const call = vi.mocked(api.getVideoChannelNext).mock.calls.at(-1)!;
	expect(call[1].more_artist_ids).toEqual([]);
});

test('"more like this" lines its picks up right after the playing video', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));
	vi.mocked(api.steerVideoChannel).mockResolvedValueOnce({
		items: [
			fixtureVideo(301, 31, 'Neighbour', 'Next Door', 200),
			// Already queued: never twice.
			fixtureVideo(203, 23, 'Already', 'Queued', 200),
			fixtureVideo(302, 21, 'Mac Dre', 'Another One', 200),
		],
		remembered: true,
	});
	const result = await steerChannel(21, 'Mac Dre', true);
	expect(result).toEqual({ dropped: 0, added: 2 });
	expect(api.steerVideoChannel).toHaveBeenCalledWith('scene:7', expect.objectContaining({
		artist_id: 21, artist_name: 'Mac Dre', more: true,
		exclude_video_ids: expect.arrayContaining([201, 202, 203, 204]),
	}));
	const state = get(videoSession);
	expect(state.current?.tidal_id).toBe(201);
	expect(state.queue.map((video) => video.tidal_id)).toEqual([201, 301, 302, 202, 203, 204]);
	expect(steerMessage('Mac Dre', true, result)).toBe('More like Mac Dre: 2 lined up next');
});

test('"less like this" still drops at once when the server cannot be reached', async () => {
	vi.mocked(api.tuneVideoChannel).mockResolvedValueOnce(fixtureTune(95));
	await playVideoChannel('scene:7');
	await vi.waitFor(() => expect(get(videoSession).radioSearching).toBe(false));
	vi.mocked(api.steerVideoChannel).mockRejectedValueOnce(new Error('offline'));
	const result = await steerChannel(21, 'Mac Dre', false);
	expect(result).toEqual({ dropped: 1, added: 0 });
	expect(get(videoSession).queue.map((video) => video.tidal_id)).toEqual([201, 202, 203]);
	expect(steerMessage('Mac Dre', false, result)).toBe('Less like Mac Dre: 1 fewer coming up');
});
