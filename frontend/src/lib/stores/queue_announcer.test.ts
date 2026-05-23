import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import {
	announceQueue,
	announceResolved,
	queueAnnouncement,
	_resetForTests,
} from './queue_announcer';

function readAnnouncement(): string {
	return get(queueAnnouncement);
}

describe('queue_announcer', () => {
	beforeEach(() => {
		_resetForTests();
		vi.useFakeTimers();
	});

	it('publishes a user-driven announcement on the next microtask', async () => {
		announceQueue('Added to queue');
		expect(readAnnouncement()).toBe('');
		await vi.advanceTimersByTimeAsync(0);
		expect(readAnnouncement()).toBe('Added to queue');
	});

	it('clears the announcement after the dwell window so a repeat message re-fires', async () => {
		announceQueue('Removed from queue');
		await vi.advanceTimersByTimeAsync(0);
		expect(readAnnouncement()).toBe('Removed from queue');
		await vi.advanceTimersByTimeAsync(2500);
		expect(readAnnouncement()).toBe('');
		announceQueue('Removed from queue');
		await vi.advanceTimersByTimeAsync(0);
		expect(readAnnouncement()).toBe('Removed from queue');
	});

	it('coalesces a burst of resolutions into a single summary', async () => {
		for (let i = 0; i < 30; i += 1) announceResolved(1);
		await vi.advanceTimersByTimeAsync(0);
		expect(readAnnouncement()).toBe('');
		await vi.advanceTimersByTimeAsync(1500);
		// One queued macrotask publishes the summary, then microtask flushes the store.
		await vi.advanceTimersByTimeAsync(0);
		expect(readAnnouncement()).toBe('30 tracks resolved on TIDAL');
	});

	it('keeps rolling the coalesce window while resolutions keep arriving', async () => {
		announceResolved(2);
		await vi.advanceTimersByTimeAsync(1400);
		expect(readAnnouncement()).toBe('');
		announceResolved(3);
		await vi.advanceTimersByTimeAsync(1400);
		expect(readAnnouncement()).toBe('');
		await vi.advanceTimersByTimeAsync(100);
		await vi.advanceTimersByTimeAsync(0);
		expect(readAnnouncement()).toBe('5 tracks resolved on TIDAL');
	});

	it('uses singular grammar for a single resolution', async () => {
		announceResolved(1);
		await vi.advanceTimersByTimeAsync(1500);
		await vi.advanceTimersByTimeAsync(0);
		expect(readAnnouncement()).toBe('1 track resolved on TIDAL');
	});

	it('drops zero or negative deltas', async () => {
		announceResolved(0);
		announceResolved(-3);
		await vi.advanceTimersByTimeAsync(2000);
		expect(readAnnouncement()).toBe('');
	});
});
