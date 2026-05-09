import { describe, expect, test, beforeEach, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';

// Mock the API client BEFORE importing the store, so the store's
// `import { authFetch } from '$lib/api/client'` resolves to the mock.
const authFetchMock = vi.fn();
vi.mock('$lib/api/client', () => ({
	getApiBase: () => 'http://test',
	authFetch: (...args: unknown[]) => authFetchMock(...args),
}));

import { discoverSpaceStore, loadSpace } from './discover_space_store';

function deferredJson(seedId: number) {
	let resolve!: (v: unknown) => void;
	const promise = new Promise((r) => {
		resolve = r;
	});
	const response = {
		ok: true,
		json: () =>
			Promise.resolve({
				diagnostics: { seed_id: seedId },
				nodes: [],
				edges: [],
				artists: [],
				generated_at: new Date().toISOString(),
				seed_track_id: seedId,
			}),
	};
	return { promise, resolve: () => resolve(response) };
}

describe('loadSpace', () => {
	beforeEach(() => {
		authFetchMock.mockReset();
		discoverSpaceStore.set({
			mode: 'radio',
			nodes: [],
			edges: [],
			radioRoute: [],
			visitedRegions: [],
			lens: 'energy',
			loading: false,
			error: null,
			lockedSeedId: null,
			activeSeedId: null,
			activeSeedSource: null,
			lastDiagnostics: null,
			refreshProgress: null,
		});
	});

	afterEach(() => {
		vi.restoreAllMocks();
	});

	test('a second loadSpace call aborts the in-flight first call', async () => {
		const first = deferredJson(111);
		const second = deferredJson(222);

		// First call: capture the AbortSignal handed to authFetch.
		let firstSignal: AbortSignal | undefined;
		authFetchMock.mockImplementationOnce((_url, init) => {
			firstSignal = (init as RequestInit).signal as AbortSignal;
			return first.promise;
		});
		// Second call: resolve immediately so the store settles on seed 222.
		authFetchMock.mockImplementationOnce(() => second.promise);

		const p1 = loadSpace('radio', 111, undefined, 'locked', null);
		const p2 = loadSpace('radio', 222, undefined, 'locked', null);

		// Settle the second call first — its result is the one the user expects.
		second.resolve();
		await p2;

		expect(firstSignal?.aborted).toBe(true);
		expect(get(discoverSpaceStore).activeSeedId).toBe(222);

		// Now resolve the first (stale) response. It must NOT clobber the store.
		first.resolve();
		await p1;

		expect(get(discoverSpaceStore).activeSeedId).toBe(222);
	});
});
