import { describe, expect, test } from 'vitest';
import { computePlayNextPos, normalizePlayerError, selectOptimisticNextItem } from './player';

// Regression: moveQueueTrackNext used to rebuild the queue via
// replacePlaybackQueue(track_ids), which silently dropped ephemeral TIDAL
// rows (negative track ids). The fix routes the move through the item-id
// endpoint instead, which is agnostic to track-id sign. These tests pin the
// position math so future refactors can't quietly reintroduce the bug.

describe('computePlayNextPos', () => {
	test('returns null when the queue is too short to reorder', () => {
		expect(computePlayNextPos(0, 0, 0)).toBeNull();
		expect(computePlayNextPos(0, 0, 1)).toBeNull();
	});

	test('returns null when targetIndex is out of bounds', () => {
		expect(computePlayNextPos(-1, 0, 5)).toBeNull();
		expect(computePlayNextPos(5, 0, 5)).toBeNull();
	});

	test('returns null when the row is already in the play-next slot', () => {
		// current at index 2, target at index 3 — already immediately after.
		expect(computePlayNextPos(3, 2, 6)).toBeNull();
	});

	test('moves a row that sits after current to the slot right after current', () => {
		// current at 2, target at 5. After removing index 5, current is still
		// at 2; insert at 3.
		expect(computePlayNextPos(5, 2, 6)).toBe(3);
	});

	test('moves a row that sits before current, accounting for the removal shift', () => {
		// current at 3, target at 1. Removing index 1 shifts current down to
		// index 2; insert at 3 (= 2 + 1).
		expect(computePlayNextPos(1, 3, 6)).toBe(3);
	});

	test('falls back to position 0 when there is no current row', () => {
		expect(computePlayNextPos(4, -1, 6)).toBe(0);
	});

	test('mixed library + ephemeral queue — math is agnostic to track sign', () => {
		// Position math only cares about indices, not track ids. This is the
		// whole point of the fix: ephemeral negative track_ids must round-trip
		// safely. Verify the helper produces a valid index for the row layout
		// regardless of what ids the rows carry.
		const length = 5;
		// Queue could be [lib, ephemeral, lib, ephemeral, lib]; current at
		// index 2 (a library track); user picks "play next" on index 4
		// (ephemeral). New position must be 3.
		expect(computePlayNextPos(4, 2, length)).toBe(3);
		// Reverse: current at 2, user picks ephemeral at 0. Removing 0 shifts
		// current to 1; insert at 2.
		expect(computePlayNextPos(0, 2, length)).toBe(2);
	});
});

describe('selectOptimisticNextItem', () => {
	test('anchors duplicate tracks by current queue item id before falling back to track id', () => {
		const queue = [
			{ id: 10, track: { id: 1, title: 'Duplicate A' } },
			{ id: 11, track: { id: 2, title: 'Middle' } },
			{ id: 12, track: { id: 1, title: 'Duplicate B' } },
			{ id: 13, track: { id: 3, title: 'Expected next' } },
		];

		expect(selectOptimisticNextItem(queue, 1, 12)?.id).toBe(13);
		expect(selectOptimisticNextItem(queue, 1, null)?.id).toBe(11);
	});
});

describe('normalizePlayerError', () => {
	test('maps API timeouts to a concrete retry message', () => {
		expect(
			normalizePlayerError(
				'start playback',
				new Error('API request timed out after 20000 ms: /api/playback/next')
			)
		).toBe('Server took too long. Try again.');
	});
});
