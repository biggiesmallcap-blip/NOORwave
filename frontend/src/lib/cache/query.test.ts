import { beforeEach, describe, expect, test, vi } from 'vitest';
import { QueryCache, stableCacheKey } from './query';

class MemoryStorage {
	private values = new Map<string, string>();

	getItem(key: string): string | null {
		return this.values.get(key) ?? null;
	}

	setItem(key: string, value: string): void {
		this.values.set(key, value);
	}

	removeItem(key: string): void {
		this.values.delete(key);
	}
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason?: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

describe('QueryCache', () => {
	let now = 1000;
	let cache: QueryCache;

	beforeEach(() => {
		now = 1000;
		cache = new QueryCache({ now: () => now });
	});

	test('builds stable keys from object params', () => {
		expect(stableCacheKey(['api', 'getTracks', { offset: 0, sort: 'title' }])).toBe(
			stableCacheKey(['api', 'getTracks', { sort: 'title', offset: 0 }]),
		);
	});

	test('deduplicates concurrent requests for the same key', async () => {
		let calls = 0;
		const gate = deferred<{ value: number }>();
		const fetcher = vi.fn(() => {
			calls += 1;
			return gate.promise;
		});

		const first = cache.fetchQuery(['api', 'dedupe'], fetcher);
		const second = cache.fetchQuery(['api', 'dedupe'], fetcher);
		gate.resolve({ value: 1 });

		await expect(first).resolves.toEqual({ value: 1 });
		await expect(second).resolves.toEqual({ value: 1 });
		expect(calls).toBe(1);
	});

	test('returns cached data while stale data refreshes in the background', async () => {
		await cache.fetchQuery(['api', 'swr'], async () => ({ value: 1 }), { staleMs: 50 });
		now += 100;
		const gate = deferred<{ value: number }>();
		const query = cache.query(['api', 'swr'], () => gate.promise, { staleMs: 50 });

		expect(query.getSnapshot()).toMatchObject({
			data: { value: 1 },
			loading: false,
			refreshing: true,
			stale: true,
		});

		gate.resolve({ value: 2 });
		await expect(query.refresh()).resolves.toEqual({ value: 2 });
		expect(query.getSnapshot()).toMatchObject({
			data: { value: 2 },
			loading: false,
			refreshing: false,
			stale: false,
		});
	});

	test('fetchQuery waits for fresh data when cached data is stale', async () => {
		await cache.fetchQuery(['api', 'fetch-swr'], async () => ({ value: 1 }), { staleMs: 50 });
		now += 100;
		const gate = deferred<{ value: number }>();

		let settled = false;
		const result = cache
			.fetchQuery(['api', 'fetch-swr'], () => gate.promise, { staleMs: 50 })
			.then((value) => {
				settled = true;
				return value;
			});

		expect(cache.getState<{ value: number }>(['api', 'fetch-swr'])).toMatchObject({
			data: { value: 1 },
			refreshing: true,
			stale: true,
		});
		expect(settled).toBe(false);

		gate.resolve({ value: 2 });
		await expect(result).resolves.toEqual({ value: 2 });
		expect(cache.peek<{ value: number }>(['api', 'fetch-swr'])).toEqual({ value: 2 });
	});

	test('fetchQuery can return stale data immediately when requested', async () => {
		await cache.fetchQuery(['api', 'fetch-stale-ok'], async () => ({ value: 1 }), { staleMs: 50 });
		now += 100;
		const gate = deferred<{ value: number }>();

		await expect(
			cache.fetchQuery(['api', 'fetch-stale-ok'], () => gate.promise, {
				staleMs: 50,
				returnStale: true,
			}),
		).resolves.toEqual({ value: 1 });

		expect(cache.getState<{ value: number }>(['api', 'fetch-stale-ok'])).toMatchObject({
			data: { value: 1 },
			refreshing: true,
			stale: true,
		});

		gate.resolve({ value: 2 });
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(cache.peek<{ value: number }>(['api', 'fetch-stale-ok'])).toEqual({ value: 2 });
	});

	test('invalidates exact keys, prefixes, and predicates without clearing data', async () => {
		await cache.fetchQuery(['api', 'getTracks', { offset: 0 }], async () => ({ tracks: [1] }));
		await cache.fetchQuery(['api', 'getAlbums', { offset: 0 }], async () => ({ albums: [1] }));

		cache.invalidateKey(['api', 'getTracks', { offset: 0 }]);
		expect(cache.getState<{ tracks: number[] }>(['api', 'getTracks', { offset: 0 }])?.stale).toBe(true);
		expect(cache.getState<{ tracks: number[] }>(['api', 'getTracks', { offset: 0 }])?.data).toEqual({
			tracks: [1],
		});

		cache.invalidatePrefix(['api', 'getAlbums']);
		expect(cache.getState<{ albums: number[] }>(['api', 'getAlbums', { offset: 0 }])?.stale).toBe(true);

		cache.invalidateWhere((key) => key.includes('getTracks'));
		expect(cache.getState<{ tracks: number[] }>(['api', 'getTracks', { offset: 0 }])?.stale).toBe(true);
	});

	test('hydrates persisted data and keeps it visible after refresh errors', async () => {
		const storage = new MemoryStorage();
		const key = stableCacheKey(['api', 'persisted']);
		storage.setItem(
			`noor.query.${key}`,
			JSON.stringify({ version: 1, lastUpdated: now - 500, data: { value: 'saved' } }),
		);

		const query = cache.query(
			['api', 'persisted'],
			async () => {
				throw new Error('offline');
			},
			{ staleMs: 10, persist: { storage, maxAgeMs: 10_000 } },
		);

		expect(query.getSnapshot().data).toEqual({ value: 'saved' });
		await expect(query.refresh()).rejects.toThrow('offline');
		expect(query.getSnapshot()).toMatchObject({
			data: { value: 'saved' },
			loading: false,
			refreshing: false,
			stale: true,
		});
		expect(query.getSnapshot().error).toBeInstanceOf(Error);
	});

	test('scopes persisted data by namespace without storing raw scope values', async () => {
		const storage = new MemoryStorage();
		const key = stableCacheKey(['api', 'persisted']);
		storage.setItem(
			`noor.query.scope-a.${key}`,
			JSON.stringify({ version: 1, lastUpdated: now, data: { value: 'a' } }),
		);
		storage.setItem(
			`noor.query.scope-b.${key}`,
			JSON.stringify({ version: 1, lastUpdated: now, data: { value: 'b' } }),
		);

		const query = cache.query(['api', 'persisted'], async () => ({ value: 'fresh' }), {
			staleMs: 10_000,
			persist: { storage, namespace: () => 'scope-b' },
		});

		expect(query.getSnapshot().data).toEqual({ value: 'b' });
	});

	test('patches known records in cached data', async () => {
		await cache.fetchQuery(['api', 'tracks'], async () => ({ tracks: [{ id: 1, title: 'Old' }] }));

		cache.patch<{ tracks: Array<{ id: number; title: string }> }>(['api', 'tracks'], (current) => ({
			tracks: (current?.tracks ?? []).map((track) =>
				track.id === 1 ? { ...track, title: 'New' } : track,
			),
		}));

		expect(cache.peek<{ tracks: Array<{ id: number; title: string }> }>(['api', 'tracks'])).toEqual({
			tracks: [{ id: 1, title: 'New' }],
		});
	});

	test('caps entries, evicting least-recently-used first', async () => {
		const capped = new QueryCache({ now: () => now, maxEntries: 3 });
		await capped.fetchQuery(['k', 1], async () => 1);
		await capped.fetchQuery(['k', 2], async () => 2);
		await capped.fetchQuery(['k', 3], async () => 3);

		// Fresh-hit on k1 touches it, so k2 is now the oldest.
		await capped.fetchQuery(['k', 1], async () => -1);
		await capped.fetchQuery(['k', 4], async () => 4);

		expect(capped.size).toBe(3);
		expect(capped.peek(['k', 2])).toBeUndefined();
		expect(capped.peek(['k', 1])).toBe(1);
		expect(capped.peek(['k', 4])).toBe(4);
	});

	test('never evicts entries with a live subscriber', async () => {
		const capped = new QueryCache({ now: () => now, maxEntries: 2 });
		const query = capped.query(['sub', 1], async () => 1);
		const unsubscribe = query.subscribe(() => {});

		await capped.fetchQuery(['sub', 2], async () => 2);
		await capped.fetchQuery(['sub', 3], async () => 3);
		await capped.fetchQuery(['sub', 4], async () => 4);
		expect(capped.getState(['sub', 1])).not.toBeNull();

		unsubscribe();
		await capped.fetchQuery(['sub', 5], async () => 5);
		expect(capped.getState(['sub', 1])).toBeNull();
		expect(capped.size).toBe(2);
	});
	// Regression: staleness used to be derived purely from `lastUpdated`, so
	// `refreshStaleFlag` overwrote the `stale: true` that invalidation had just
	// written and the next read handed back the cached payload. Invalidation was
	// a no-op for the whole staleMs window. This is why a newly created playlist
	// stayed invisible on /playlists until the window lapsed, and why the
	// existing invalidateLibraryCaches() calls appeared to do nothing.
	test('invalidateKey forces a refetch inside the staleMs window', async () => {
		const cache = new QueryCache();
		let calls = 0;
		const fetcher = async () => ++calls;

		expect(await cache.fetchQuery('k', fetcher, { staleMs: 60_000 })).toBe(1);
		// Still fresh by age: a second read must not hit the network.
		expect(await cache.fetchQuery('k', fetcher, { staleMs: 60_000 })).toBe(1);
		expect(calls).toBe(1);

		cache.invalidateKey('k');
		expect(await cache.fetchQuery('k', fetcher, { staleMs: 60_000 })).toBe(2);
		expect(calls).toBe(2);
	});

	test('invalidatePrefix forces a refetch inside the staleMs window', async () => {
		const cache = new QueryCache();
		let calls = 0;
		await cache.fetchQuery(['api', 'getPlaylists'], async () => ++calls, { staleMs: 60_000 });
		expect(calls).toBe(1);

		cache.invalidatePrefix(['api', 'getPlaylists']);
		await cache.fetchQuery(['api', 'getPlaylists'], async () => ++calls, { staleMs: 60_000 });
		expect(calls).toBe(2);
	});

	test('a satisfied invalidation does not refetch forever', async () => {
		const cache = new QueryCache();
		let calls = 0;
		const fetcher = async () => ++calls;
		await cache.fetchQuery('k', fetcher, { staleMs: 60_000 });
		cache.invalidateKey('k');
		await cache.fetchQuery('k', fetcher, { staleMs: 60_000 });
		expect(calls).toBe(2);
		// The refetch cleared the flag, so the entry is fresh again.
		await cache.fetchQuery('k', fetcher, { staleMs: 60_000 });
		expect(calls).toBe(2);
	});

	test('prime satisfies a pending invalidation', async () => {
		const cache = new QueryCache();
		let calls = 0;
		const fetcher = async () => ++calls;
		await cache.fetchQuery('k', fetcher, { staleMs: 60_000 });
		cache.invalidateKey('k');
		// Writing known-good data is as good as fetching it.
		cache.prime('k', 99, { staleMs: 60_000 });
		expect(await cache.fetchQuery('k', fetcher, { staleMs: 60_000 })).toBe(99);
		expect(calls).toBe(1);
	});
	// Regression: three rapid deletes fired three invalidations, but fetchEntry
	// deduped callers two and three onto the request issued for the first. That
	// response predated the later deletes, and completing it cleared the flag, so
	// the last deleted playlist stayed on screen.
	test('an invalidation raised mid-flight is not satisfied by the older request', async () => {
		const cache = new QueryCache();
		const gates = [deferred<number>(), deferred<number>()];
		let call = 0;
		const fetcher = () => gates[call++].promise;

		const first = cache.fetchQuery('k', fetcher, { staleMs: 60_000 });
		// A change lands while the first request is still open.
		cache.invalidateKey('k');
		gates[0].resolve(1);

		// The caller must not receive the pre-invalidation value.
		gates[1].resolve(2);
		expect(await first).toBe(2);
		expect(call).toBe(2);
	});
	// The burst case: several callers join one in-flight request while further
	// invalidations land. Every one of them must end up with post-invalidation
	// data, not the internal supersede sentinel.
	test('callers joining an in-flight request also retry when it is superseded', async () => {
		const cache = new QueryCache();
		const gates = [deferred<number>(), deferred<number>()];
		let call = 0;
		const fetcher = () => gates[call++].promise;

		const first = cache.fetchQuery('k', fetcher, { staleMs: 60_000 });
		const joiner = cache.fetchQuery('k', fetcher, { staleMs: 60_000 });
		cache.invalidateKey('k');
		gates[0].resolve(1);
		gates[1].resolve(2);

		expect(await first).toBe(2);
		expect(await joiner).toBe(2);
		expect(call).toBe(2);
	});
});
