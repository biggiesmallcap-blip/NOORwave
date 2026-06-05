import { describe, expect, it } from 'vitest';
import type { QueueItem, Track } from '$lib/api/client';
import { isQueueItemActive } from './queue_active';

function track(id: number, title: string): Track {
	return {
		id,
		title,
		tidal_id: null,
		source: 'tidal_stream',
	} as Track;
}

function row(id: number, itemTrack: Track, isPending = false): QueueItem {
	return {
		id,
		position: id,
		source: isPending ? 'radio_pending' : 'library',
		is_pending: isPending,
		track: itemTrack,
	};
}

describe('isQueueItemActive', () => {
	it('uses the queue item anchor when it matches the current track', () => {
		const current = track(2, 'Current');
		const queue = [row(10, track(1, 'Before')), row(11, current), row(12, track(3, 'After'))];

		expect(isQueueItemActive(queue[1], current, 11, queue)).toBe(true);
		expect(isQueueItemActive(queue[0], current, 11, queue)).toBe(false);
	});

	it('falls back to the current track when the queue item anchor is stale', () => {
		const current = track(2, 'Current');
		const queue = [row(10, track(1, 'Stale Anchor')), row(11, current)];

		expect(isQueueItemActive(queue[0], current, 10, queue)).toBe(false);
		expect(isQueueItemActive(queue[1], current, 10, queue)).toBe(true);
	});

	it('marks only the first current-track row when no queue item anchor exists', () => {
		const current = track(2, 'Current');
		const queue = [row(10, current), row(11, current), row(12, track(3, 'After'))];

		expect(isQueueItemActive(queue[0], current, null, queue)).toBe(true);
		expect(isQueueItemActive(queue[1], current, null, queue)).toBe(false);
		expect(isQueueItemActive(queue[2], current, null, queue)).toBe(false);
	});

	it('marks only one duplicate row when the queue item anchor is stale', () => {
		const current = track(2, 'Current');
		const queue = [row(10, track(1, 'Stale Anchor')), row(11, current), row(12, current)];

		expect(isQueueItemActive(queue[0], current, 10, queue)).toBe(false);
		expect(isQueueItemActive(queue[1], current, 10, queue)).toBe(true);
		expect(isQueueItemActive(queue[2], current, 10, queue)).toBe(false);
	});

	it('keeps unresolved pending current rows highlighted by queue item id', () => {
		const pending = row(10, track(0, 'Resolving'), true);
		const queue = [pending, row(11, track(2, 'Next'))];

		expect(isQueueItemActive(pending, null, 10, queue)).toBe(true);
		expect(isQueueItemActive(queue[1], null, 10, queue)).toBe(false);
	});
});
