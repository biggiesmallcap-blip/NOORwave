import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import {
	cacheKeys,
	ensureCacheScope,
	invalidateHomeCaches,
	invalidateLibraryCaches,
	patchDiscoveryProgress,
} from './api_queries';
import { dataCache } from './query';

describe('cached API helpers', () => {
	beforeEach(() => {
		dataCache.clear();
	});

	afterEach(() => {
		vi.unstubAllGlobals();
	});

	test('invalidates library and home prefixes without clearing snapshots', () => {
		dataCache.prime(cacheKeys.tracks('date_added', 'desc', 100, 0, true, false), {
			tracks: [{ id: 1 }],
			total: 1,
		});
		dataCache.prime(cacheKeys.search('burial', 100), {
			tracks: [{ id: 2 }],
			albums: [],
			artists: [],
		});
		dataCache.prime(cacheKeys.homeArticles(), { articles: [{ title: 'Saved' }] });

		invalidateLibraryCaches();
		invalidateHomeCaches();

		expect(dataCache.getState(cacheKeys.tracks('date_added', 'desc', 100, 0, true, false))).toMatchObject({
			data: { tracks: [{ id: 1 }], total: 1 },
			stale: true,
		});
		expect(dataCache.getState(cacheKeys.homeArticles())).toMatchObject({
			data: { articles: [{ title: 'Saved' }] },
			stale: true,
		});
		expect(dataCache.getState(cacheKeys.search('burial', 100))).toMatchObject({
			data: { tracks: [{ id: 2 }], albums: [], artists: [] },
			stale: true,
		});
	});

	test('patches cached discovery progress from WebSocket payload shape', () => {
		dataCache.prime(cacheKeys.settings.discoveryStatus(), {
			status: {
				latest_run: {
					progress: 0.1,
					stage: 'queued',
					items_done: 1,
					items_total: 20,
				},
			},
		});

		patchDiscoveryProgress({
			progress: 0.5,
			stage: 'train',
			tracks_done: 10,
			tracks_total: 20,
		});

		expect(dataCache.peek<any>(cacheKeys.settings.discoveryStatus()).status.latest_run).toMatchObject({
			progress: 0.5,
			stage: 'train',
			items_done: 10,
			items_total: 20,
		});
	});

	test('clears in-memory API cache when the auth scope changes', () => {
		const storage = new Map<string, string>();
		vi.stubGlobal('localStorage', {
			getItem: (key: string) => storage.get(key) ?? null,
			setItem: (key: string, value: string) => storage.set(key, value),
			removeItem: (key: string) => storage.delete(key),
			clear: () => storage.clear(),
		});

		localStorage.setItem('noor_api_token', 'token-a');
		ensureCacheScope();
		dataCache.prime(cacheKeys.homeArticles(), { articles: [{ title: 'Old session' }] });

		localStorage.setItem('noor_api_token', 'token-b');
		ensureCacheScope();

		expect(dataCache.peek(cacheKeys.homeArticles())).toBeUndefined();
	});

	test('keeps existing in-memory cache on first scope initialization', async () => {
		vi.resetModules();
		const freshQuery = await import('./query');
		const freshApiQueries = await import('./api_queries');

		freshQuery.dataCache.prime(freshApiQueries.cacheKeys.homeArticles(), {
			articles: [{ title: 'Existing data' }],
		});

		freshApiQueries.ensureCacheScope();

		expect(freshQuery.dataCache.peek(freshApiQueries.cacheKeys.homeArticles())).toEqual({
			articles: [{ title: 'Existing data' }],
		});
	});
});
