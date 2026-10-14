import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { ArchiveQuery, ArchiveResponse, ArchiveSong } from '$lib/api/video_explore_types';
import { createArchiveFeed, type FeedState } from './archive_feed';
import { DEFAULT_LENSES, lensKey } from './lenses';

function song(n: number): ArchiveSong {
	return {
		song_key: `id:1:song ${n}`,
		title: `Song ${n}`,
		artist_id: 1,
		artist_name: 'Artist',
		best: {
			tidal_id: n,
			title: `Song ${n}`,
			duration_ms: 200_000,
			artist_id: 1,
			artist_name: 'Artist',
			album_tidal_id: null,
			artwork_url: null,
			quality: null,
			explicit: null,
			type: 'video',
		},
		cut_count: 1,
		cuts: ['official'],
		song_year: 1990,
		scene_id: null,
		light: 1,
	};
}

function response(items: ArchiveSong[], total = items.length): ArchiveResponse {
	return {
		building: false,
		total,
		items,
		facets: { scene: [], cut: [], decade: [{ decade: 1990, count: total }], depth: [], length: [] },
	};
}

interface Call {
	query: ArchiveQuery;
	signal: AbortSignal;
	resolve: (value: ArchiveResponse) => void;
	reject: (error: unknown) => void;
}

function harness(debounceMs = 200) {
	const calls: Call[] = [];
	const states: FeedState[] = [];
	const feed = createArchiveFeed({
		debounceMs,
		pageSize: 2,
		onChange: (state) => states.push(state),
		fetch: (query, signal) => new Promise<ArchiveResponse>((resolve, reject) => {
			const call = { query, signal, resolve, reject };
			calls.push(call);
			signal.addEventListener('abort', () => reject(Object.assign(new Error('aborted'), { name: 'AbortError' })));
		}),
	});
	return { feed, calls, states };
}

const flush = async () => {
	for (let i = 0; i < 10; i += 1) await Promise.resolve();
};

beforeEach(() => vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] }));
afterEach(() => vi.useRealTimers());

describe('archive feed', () => {
	test('a kept first page shows at once and is checked quietly', async () => {
		const calls: Call[] = [];
		const states: FeedState[] = [];
		const seed: FeedState = {
			phase: 'ready',
			key: lensKey(DEFAULT_LENSES),
			items: [song(1), song(2)],
			total: 9,
			facets: null,
			loadingMore: false,
			error: null,
			base: 0,
			jumps: [],
			loadingEarlier: false,
		};
		const feed = createArchiveFeed({
			pageSize: 2,
			seed,
			onChange: (state) => states.push(state),
			fetch: (query, signal) => new Promise<ArchiveResponse>((resolve, reject) => calls.push({ query, signal, resolve, reject })),
		});
		expect(feed.state.items.map((item) => item.title)).toEqual(['Song 1', 'Song 2']);
		feed.request(DEFAULT_LENSES);
		expect(calls).toHaveLength(1);
		// No refreshing or loading state while it checks.
		expect(states.every((state) => state.phase === 'ready')).toBe(true);
		calls[0].resolve(response([song(3), song(1)], 12));
		await flush();
		expect(feed.state.items.map((item) => item.title)).toEqual(['Song 3', 'Song 1']);
		expect(feed.state.total).toBe(12);
	});

	test('the first ask goes out at once; later asks are debounced', async () => {
		const { feed, calls } = harness();
		feed.request(DEFAULT_LENSES);
		expect(calls).toHaveLength(1);
		calls[0].resolve(response([song(1)]));
		await vi.runAllTimersAsync();
		feed.request({ ...DEFAULT_LENSES, cut: 'live' });
		expect(calls).toHaveLength(1);
		vi.advanceTimersByTime(199);
		expect(calls).toHaveLength(1);
		vi.advanceTimersByTime(1);
		expect(calls).toHaveLength(2);
		expect(calls[1].query.cut).toBe('live');
	});

	test('a burst of lens changes sends one request for the last state', async () => {
		const { feed, calls } = harness();
		feed.request(DEFAULT_LENSES);
		calls[0].resolve(response([song(1)]));
		await vi.runAllTimersAsync();
		feed.request({ ...DEFAULT_LENSES, scene: 1 });
		vi.advanceTimersByTime(50);
		feed.request({ ...DEFAULT_LENSES, scene: 2 });
		vi.advanceTimersByTime(50);
		feed.request({ ...DEFAULT_LENSES, scene: 3 });
		vi.advanceTimersByTime(250);
		expect(calls).toHaveLength(2);
		expect(calls[1].query.scene).toBe(3);
	});

	test('a newer state aborts the request in flight and ignores its answer', async () => {
		const { feed, calls } = harness(0);
		feed.request({ ...DEFAULT_LENSES, scene: 1 });
		feed.request({ ...DEFAULT_LENSES, scene: 2 });
		expect(calls).toHaveLength(2);
		expect(calls[0].signal.aborted).toBe(true);
		calls[1].resolve(response([song(2)]));
		calls[0].resolve(response([song(1)]));
		await flush();
		expect(feed.state.phase).toBe('ready');
		expect(feed.state.items.map((item) => item.best.tidal_id)).toEqual([2]);
	});

	test('keeps the previous results up while a new state loads', async () => {
		const { feed, calls } = harness(0);
		feed.request(DEFAULT_LENSES);
		calls[0].resolve(response([song(1)]));
		await flush();
		feed.request({ ...DEFAULT_LENSES, cut: 'live' });
		expect(feed.state.phase).toBe('refreshing');
		expect(feed.state.items).toHaveLength(1);
	});

	test('the same state twice is one request', async () => {
		const { feed, calls } = harness(0);
		feed.request(DEFAULT_LENSES);
		feed.request({ ...DEFAULT_LENSES });
		expect(calls).toHaveLength(1);
	});

	test('pages append without duplicates and stop at the total', async () => {
		const { feed, calls } = harness(0);
		feed.request(DEFAULT_LENSES);
		calls[0].resolve(response([song(1), song(2)], 3));
		await flush();
		expect(feed.hasMore).toBe(true);
		feed.loadMore();
		feed.loadMore();
		expect(calls).toHaveLength(2);
		expect(calls[1].query.offset).toBe(2);
		calls[1].resolve(response([song(2), song(3)], 3));
		await flush();
		expect(feed.state.items.map((item) => item.best.tidal_id)).toEqual([1, 2, 3]);
		expect(feed.hasMore).toBe(false);
	});

	test('a lens change aborts a page of more in flight', async () => {
		const { feed, calls } = harness(0);
		feed.request(DEFAULT_LENSES);
		calls[0].resolve(response([song(1), song(2)], 10));
		await flush();
		feed.loadMore();
		feed.request({ ...DEFAULT_LENSES, depth: 'deep' });
		expect(calls[1].signal.aborted).toBe(true);
		calls[2].resolve(response([song(9)]));
		await flush();
		expect(feed.state.items.map((item) => item.best.tidal_id)).toEqual([9]);
	});

	test('501 is the honest not-built-yet state, and retry asks again', async () => {
		const { feed, calls } = harness(0);
		feed.request(DEFAULT_LENSES);
		calls[0].reject(Object.assign(new Error('not implemented'), { status: 501 }));
		await flush();
		expect(feed.state.phase).toBe('unavailable');
		feed.retry();
		expect(calls).toHaveLength(2);
		calls[1].reject(new Error('boom'));
		await flush();
		expect(feed.state.phase).toBe('error');
	});
});

describe('jumping deep into a sorted list', () => {
	test('a jump loads the window at that offset; it then grows both ways', async () => {
		const { feed, calls } = harness(0);
		feed.request({ ...DEFAULT_LENSES, sort: 'title' });
		calls[0].resolve({ ...response([song(1), song(2)], 10), jumps: [{ label: 'A', offset: 0 }, { label: 'M', offset: 6 }] });
		await flush();
		expect(feed.state.jumps.map((j) => j.label)).toEqual(['A', 'M']);

		// Already loaded: no request, just where it is.
		expect(await feed.jumpTo(1)).toBe(1);
		expect(calls).toHaveLength(1);

		const jumped = feed.jumpTo(6);
		expect(calls[1].query).toMatchObject({ sort: 'title', offset: 6, limit: 2 });
		calls[1].resolve(response([song(7), song(8)], 10));
		expect(await jumped).toBe(0);
		expect(feed.state.base).toBe(6);
		expect(feed.hasMore).toBe(true);

		feed.loadMore();
		expect(calls[2].query).toMatchObject({ offset: 8, limit: 2 });
		calls[2].resolve(response([song(9), song(10)], 10));
		await flush();
		expect(feed.hasMore).toBe(false);

		feed.loadEarlier();
		expect(calls[3].query).toMatchObject({ offset: 4, limit: 2 });
		calls[3].resolve(response([song(5), song(6)], 10));
		await flush();
		expect(feed.state.base).toBe(4);
		expect(feed.state.items.map((s) => s.title)).toEqual(['Song 5', 'Song 6', 'Song 7', 'Song 8', 'Song 9', 'Song 10']);
	});

	test('a new lens state starts again at the top', async () => {
		const { feed, calls } = harness(0);
		feed.request({ ...DEFAULT_LENSES, sort: 'title' });
		calls[0].resolve(response([song(1), song(2)], 10));
		await flush();
		const jumped = feed.jumpTo(6);
		calls[1].resolve(response([song(7), song(8)], 10));
		await jumped;
		feed.request({ ...DEFAULT_LENSES, sort: 'artist' });
		expect(calls[2].query).toMatchObject({ sort: 'artist', offset: 0 });
		calls[2].resolve(response([song(3)], 1));
		await flush();
		expect(feed.state.base).toBe(0);
		expect(feed.state.jumps).toEqual([]);
	});
});
