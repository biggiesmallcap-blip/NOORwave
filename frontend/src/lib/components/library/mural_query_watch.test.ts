import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { writable } from 'svelte/store';
import type { CacheState } from '$lib/cache/query';
import { watchMuralQuery } from './mural_query_watch';

type Payload = { tracks: number[] };

function state(data: Payload | undefined, extra: Partial<CacheState<Payload>> = {}): CacheState<Payload> {
	return { data, loading: false, refreshing: false, error: null, lastUpdated: 0, stale: false, hydrated: false, ...extra };
}

// A query whose refresh resolves to the next scripted response, the way the
// real cache moves through refreshing -> settled.
function fakeQuery(initial: CacheState<Payload>, responses: Array<Payload | Error>) {
	const store = writable(initial);
	const refresh = vi.fn(async () => {
		const next = responses.shift() ?? { tracks: [] };
		store.update((s) => ({ ...s, refreshing: true }));
		await Promise.resolve();
		if (next instanceof Error) {
			store.update((s) => ({ ...s, refreshing: false, error: next }));
			throw next;
		}
		store.set(state(next));
		return next;
	});
	return { subscribe: store.subscribe, refresh };
}

describe('watchMuralQuery', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	test('an empty sample refreshes while mounted and the panel fills without navigation', async () => {
		const query = fakeQuery(state({ tracks: [] }), [{ tracks: [] }, { tracks: [1, 2] }]);
		const painted: number[][] = [];
		const stop = watchMuralQuery(query, {
			isEmpty: (d) => d.tracks.length === 0,
			onData: (d) => painted.push(d.tracks),
			delaysMs: [2000, 5000, 15000],
		});

		expect(query.refresh).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(2000);
		expect(query.refresh).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(5000);
		expect(query.refresh).toHaveBeenCalledTimes(2);
		expect(painted.at(-1)).toEqual([1, 2]);

		// Filled: no further refreshes.
		await vi.advanceTimersByTimeAsync(60000);
		expect(query.refresh).toHaveBeenCalledTimes(2);
		stop();
	});

	test('a failed fetch retries, and the attempts are bounded', async () => {
		const query = fakeQuery(state(undefined, { error: new Error('down') }), [new Error('down'), new Error('down')]);
		const stop = watchMuralQuery(query, {
			isEmpty: (d) => d.tracks.length === 0,
			onData: () => undefined,
			delaysMs: [1000, 1000],
		});
		await vi.advanceTimersByTimeAsync(10000);
		expect(query.refresh).toHaveBeenCalledTimes(2);
		stop();
	});

	test('unmount cancels a pending refresh', async () => {
		const query = fakeQuery(state({ tracks: [] }), [{ tracks: [1] }]);
		const stop = watchMuralQuery(query, {
			isEmpty: (d) => d.tracks.length === 0,
			onData: () => undefined,
			delaysMs: [2000],
		});
		stop();
		await vi.advanceTimersByTimeAsync(5000);
		expect(query.refresh).not.toHaveBeenCalled();
	});
});
