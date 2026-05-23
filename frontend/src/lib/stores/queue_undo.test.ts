import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import {
	pendingUndo,
	offerUndo,
	consumeUndo,
	dismissUndo,
	_resetForTests,
} from './queue_undo';
import type { QueueItem } from '$lib/api/client';

function fakeItem(id: number, trackId: number, source = 'library'): QueueItem {
	return {
		id,
		position: id,
		source,
		track: {
			id: trackId,
			title: `t${id}`,
			tidal_id: trackId < 0 ? -trackId : null,
		} as QueueItem['track'],
	};
}

describe('queue_undo store', () => {
	beforeEach(() => {
		_resetForTests();
		vi.useFakeTimers();
	});

	it('offerUndo with non-empty items publishes a pending undo', () => {
		offerUndo([fakeItem(1, 100), fakeItem(2, 101)], 6000);
		const value = get(pendingUndo);
		expect(value?.count).toBe(2);
		expect(value?.items.length).toBe(2);
		expect(value?.expiresAt).toBeGreaterThan(Date.now());
	});

	it('offerUndo with empty items clears the store', () => {
		offerUndo([fakeItem(1, 100)], 6000);
		offerUndo([], 6000);
		expect(get(pendingUndo)).toBeNull();
	});

	it('auto-clears after the TTL', () => {
		offerUndo([fakeItem(1, 100)], 6000);
		expect(get(pendingUndo)).not.toBeNull();
		vi.advanceTimersByTime(6001);
		expect(get(pendingUndo)).toBeNull();
	});

	it('consumeUndo returns the items and clears the store', () => {
		const items = [fakeItem(1, 100), fakeItem(2, -200, 'tidal_mix')];
		offerUndo(items, 6000);
		const taken = consumeUndo();
		expect(taken).toEqual(items);
		expect(get(pendingUndo)).toBeNull();
	});

	it('consumeUndo returns null when nothing pending', () => {
		expect(consumeUndo()).toBeNull();
	});

	it('dismissUndo clears the store and cancels the TTL', () => {
		offerUndo([fakeItem(1, 100)], 6000);
		dismissUndo();
		expect(get(pendingUndo)).toBeNull();
		vi.advanceTimersByTime(7000);
		expect(get(pendingUndo)).toBeNull();
	});

	it('a second offerUndo cancels the first TTL', () => {
		offerUndo([fakeItem(1, 100)], 6000);
		vi.advanceTimersByTime(3000);
		offerUndo([fakeItem(2, 101), fakeItem(3, 102)], 6000);
		vi.advanceTimersByTime(4000);
		// At t=7000ms, first TTL would have expired but the second offer
		// reset it, so the new items should still be present.
		const value = get(pendingUndo);
		expect(value?.count).toBe(2);
	});
});
