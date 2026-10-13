import { describe, expect, test, vi } from 'vitest';
import type { AtlasRouteStop, ExploreVideo } from '$lib/api/video_explore_types';

vi.mock('$lib/stores/video_session', () => ({ playVideo: vi.fn().mockResolvedValue(true), videoSession: { subscribe: () => () => {} } }));
vi.mock('$lib/stores/toast', () => ({ showToast: vi.fn() }));

const { formatRuntime, playAtlasQueue, probeMessage, routeLabel, routeQueue } = await import('./play');
const { playVideo } = await import('$lib/stores/video_session');

const video = (id: number): ExploreVideo => ({
	tidal_id: id,
	title: `Video ${id}`,
	duration_ms: 200_000,
	artist_id: 1,
	artist_name: 'A',
	album_tidal_id: null,
	artwork_url: null,
	quality: null,
	explicit: null,
	type: 'video',
});

const stop = (artist_id: number, name: string, v: ExploreVideo | null): AtlasRouteStop => ({ artist_id, name, via: null, video: v });

describe('atlas playback helpers', () => {
	test('a route plays one video per stop, in order, skipping gaps and repeats', () => {
		const stops = [stop(1, 'Future', video(10)), stop(2, 'Gap', null), stop(3, 'Steps', video(30)), stop(4, 'Again', video(10)), stop(5, 'Shakira', video(50))];
		expect(routeQueue(stops).map((v) => v.tidal_id)).toEqual([10, 30, 50]);
		expect(routeLabel(stops)).toBe('Route: Future to Shakira');
		expect(routeLabel([])).toBe('Atlas route');
	});

	test('a queue plays through with autoplay', async () => {
		const list = [video(1), video(2)];
		await playAtlasQueue(list[1], list, 'Scene');
		expect(playVideo).toHaveBeenCalledWith(list[1], expect.objectContaining({ queue: list, autoplay: true, sourceLabel: 'Scene' }));
	});

	test('runtime and probe copy', () => {
		expect(formatRuntime(65_000)).toBe('1:05');
		expect(formatRuntime(null)).toBe('');
		expect(probeMessage('Pezet', true)).toContain('Probe sent');
		expect(probeMessage('Pezet', false, 'discovery off')).toContain('Video discovery is off');
		expect(probeMessage('Pezet', false, 'already checked')).toContain('Already checked');
		expect(probeMessage('Pezet', false, 'busy')).toBe('Not sent: busy.');
	});
});
