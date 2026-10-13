import { describe, expect, test } from 'vitest';
import { actionErrorMessage, describeError } from './describe_error';

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

describe('actionErrorMessage', () => {
	const apiError = (status: number, message: string) => Object.assign(new Error(message), { status });

	test('shows the server message for a client error', () => {
		expect(actionErrorMessage(apiError(400, 'This track is not available on TIDAL'), 'Try again.'))
			.toBe('This track is not available on TIDAL');
		expect(actionErrorMessage(apiError(409, 'This playlist changed on TIDAL. Refresh it and try again.'), 'x'))
			.toBe('This playlist changed on TIDAL. Refresh it and try again.');
	});

	test('falls back for server errors, bare status text and non-errors', () => {
		expect(actionErrorMessage(apiError(502, 'TIDAL rejected the change'), 'Fallback')).toBe('Fallback');
		expect(actionErrorMessage(apiError(404, 'API error: 404'), 'Fallback')).toBe('Fallback');
		expect(actionErrorMessage('boom', 'Fallback')).toBe('Fallback');
	});
});
