import { beforeEach, describe, expect, test, vi } from 'vitest';
import { cacheKeys } from './api_queries';
import { dataCache } from './query';
import { applyCacheUpdateForWsMessage, clearWsCacheTimers } from './ws_events';

describe('WebSocket cache updates', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		clearWsCacheTimers();
		dataCache.clear();
	});

	test('patches discovery training progress without dropping cached status', () => {
		dataCache.prime(cacheKeys.settings.discoveryStatus(), {
			status: {
				selected_engine: 'v2',
				selected_engine_trainable: true,
				latest_run: {
					id: 1,
					status: 'running',
					progress: 0.2,
					stage: 'start',
					items_done: 2,
					items_total: 10,
				},
			},
		});

		applyCacheUpdateForWsMessage({
			type: 'training_progress',
			stage: 'train',
			progress: 0.6,
			tracks_done: 6,
			tracks_total: 10,
		});

		expect(dataCache.peek<any>(cacheKeys.settings.discoveryStatus()).status.latest_run).toMatchObject({
			stage: 'train',
			progress: 0.6,
			items_done: 6,
			items_total: 10,
		});
	});

	test('invalidates library caches on library sync without clearing data', () => {
		dataCache.prime(cacheKeys.tracks('date_added', 'desc', 100, 0, true, false), {
			tracks: [{ id: 1 }],
			total: 1,
		});

		applyCacheUpdateForWsMessage({ type: 'library_synced' });

		const state = dataCache.getState(cacheKeys.tracks('date_added', 'desc', 100, 0, true, false));
		expect(state?.data).toEqual({ tracks: [{ id: 1 }], total: 1 });
		expect(state?.stale).toBe(true);
	});

	test('patches radio similarity pairs and schedules a refetch', () => {
		dataCache.prime(cacheKeys.settings.radioSimilarityStatus(), {
			row_count: 5,
			built_at: 'old',
		});

		applyCacheUpdateForWsMessage({ type: 'radio_similarity_computed', pairs: 12 });

		expect(dataCache.peek(cacheKeys.settings.radioSimilarityStatus())).toEqual({
			row_count: 12,
			built_at: 'old',
		});

		vi.advanceTimersByTime(500);
		expect(dataCache.getState(cacheKeys.settings.radioSimilarityStatus())?.stale).toBe(true);
	});
});
