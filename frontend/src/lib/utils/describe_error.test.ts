import { describe, expect, test } from 'vitest';
import { describeError } from './describe_error';

describe('describeError', () => {
	test('a server error reads as plain language and keeps the raw text as detail', () => {
		const result = describeError('Failed to load artist: ApiError: API error: 500');
		expect(result.message).toBe("NOORwave's server hit a problem loading this. Try again.");
		expect(result.detail).toBe('Failed to load artist: ApiError: API error: 500');
	});

	test('status objects, missing items and network failures', () => {
		expect(describeError({ status: 404 }).message).toBe('It may have been removed or moved.');
		expect(describeError(new TypeError('Failed to fetch')).message).toContain("Can't reach the NOORwave server");
		expect(describeError(null)).toEqual({ message: 'Something went wrong loading this. Try again.', detail: null });
	});
});
