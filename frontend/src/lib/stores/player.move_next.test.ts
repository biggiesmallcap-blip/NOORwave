import { describe, expect, test } from 'vitest';
import {
	computePlayNextPos,
	normalizePlayerError,
	reorderDropIndex,
	selectAppendedQueueRow,
	selectOptimisticNextItem
} from './player';

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

describe('reorderDropIndex', () => {
	// Regression: handleQueueDrop passed the pre-removal target index straight to
	// moveQueueItem, which removes the dragged row first. Downward drags landed
	// one slot too low. reorderDropIndex applies the removal-shift correction.
	test('downward drag (source above target) subtracts one', () => {
		// Queue [cur,A,B,C] (0..3). Drag A(1) onto C(3): after removing A the
		// target slot is 2, so the row lands on C's top edge, not below it.
		expect(reorderDropIndex(1, 3)).toBe(2);
	});

	test('upward drag (source below target) keeps the target index', () => {
		// Drag C(3) onto A(1): removing C does not shift A, so insert at 1.
		expect(reorderDropIndex(3, 1)).toBe(1);
	});

	test('missing source index keeps the target index', () => {
		expect(reorderDropIndex(-1, 4)).toBe(4);
	});

	test('adjacent downward drag collapses to a no-op-ish same slot', () => {
		// Drag A(1) onto B(2): subtract one -> 1, i.e. stays put (drop just below).
		expect(reorderDropIndex(1, 2)).toBe(1);
	});
});

describe('selectAppendedQueueRow', () => {
	const row = (id: number, trackId: number) => ({ id, track: { id: trackId } });

	test('picks the genuinely-new row when the track was already queued', () => {
		// Regression: a track-id match returned the pre-existing earlier copy and
		// the freshly appended row (id 99) was stranded at the bottom -> "Play
		// next went to the bottom".
		const before = [row(10, 1), row(11, 2)];
		const after = [row(10, 1), row(11, 2), row(99, 1)];
		expect(selectAppendedQueueRow(before, after, 1)?.id).toBe(99);
	});

	test('picks the new row for a track not previously queued', () => {
		const before = [row(10, 1)];
		const after = [row(10, 1), row(12, 5)];
		expect(selectAppendedQueueRow(before, after, 5)?.id).toBe(12);
	});

	test('falls back to a track-id match when no new id appears', () => {
		const before = [row(10, 1), row(11, 2)];
		const after = [row(10, 1), row(11, 2)]; // dedupe: nothing appended
		expect(selectAppendedQueueRow(before, after, 2)?.id).toBe(11);
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
