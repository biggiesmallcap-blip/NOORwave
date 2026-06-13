import { describe, expect, test } from 'vitest';
import { sliceContextTrackIds } from './player';

// Regression: clicking a track in a list used to either play a single orphan
// track (playTrackNow) or — once routed through context — reorder the clicked
// track to the front, which replayed every track ABOVE it next. The fix slices
// the list at the clicked track so the rows AFTER it are what play next, matching
// TIDAL/Spotify. sliceContextTrackIds is the pure core of that contract — pin it
// so the "queue starts at the click and runs to the end" behavior can't regress.

describe('sliceContextTrackIds', () => {
	test('slices from the clicked track to the end, dropping the tracks above it', () => {
		expect(sliceContextTrackIds([1, 2, 3, 4, 5], 3)).toEqual([3, 4, 5]);
	});

	test('returns the whole list when the clicked track is already first', () => {
		expect(sliceContextTrackIds([1, 2, 3], 1)).toEqual([1, 2, 3]);
	});

	test('returns the whole list when no start track is given (Play all)', () => {
		expect(sliceContextTrackIds([1, 2, 3])).toEqual([1, 2, 3]);
	});

	test('returns the whole list when the start track is not present', () => {
		expect(sliceContextTrackIds([1, 2, 3], 99)).toEqual([1, 2, 3]);
	});

	test('queues only the clicked track when it is the last row', () => {
		expect(sliceContextTrackIds([1, 2, 3], 3)).toEqual([3]);
	});

	test('drops non-positive ids before slicing, so the index stays honest', () => {
		// -500 (ephemeral TIDAL) and 0 (unresolved pending) are filtered first;
		// slicing at id 4 must yield exactly [4, 5], never leak the junk ids.
		expect(sliceContextTrackIds([1, -500, 2, 0, 4, 5], 4)).toEqual([4, 5]);
	});

	test('returns an empty list when there are no playable ids', () => {
		expect(sliceContextTrackIds([0, -1, -2], 0)).toEqual([]);
	});
});
