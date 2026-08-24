import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import {
	api,
	ApiTimeoutError,
	authFetch,
	BULK_QUEUE_API_TIMEOUT_MS,
	DEFAULT_API_TIMEOUT_MS,
	TIDAL_CATALOG_API_TIMEOUT_MS,
} from './client';

function stubHangingFetch() {
	const signals: AbortSignal[] = [];
	const fetch = vi.fn(
		(_url: string | URL | Request, init?: RequestInit) =>
			new Promise<Response>((_resolve, reject) => {
				const signal = init?.signal;
				if (!signal) return;
				signals.push(signal);
				signal.addEventListener(
					'abort',
					() => reject(signal.reason ?? new DOMException('Aborted', 'AbortError')),
					{ once: true },
				);
			}),
	);
	vi.stubGlobal('fetch', fetch);
	return { fetch, signals };
}

describe('API request timeout policy', () => {
	beforeEach(() => {
		vi.useFakeTimers();
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.unstubAllGlobals();
	});

	test('times out shared API calls instead of waiting forever', async () => {
		const { fetch } = stubHangingFetch();

		const request = api.getPlaybackState();
		const assertion = expect(request).rejects.toMatchObject({
			name: 'ApiTimeoutError',
			path: '/api/playback/state',
			timeoutMs: DEFAULT_API_TIMEOUT_MS,
		});
		await vi.advanceTimersByTimeAsync(DEFAULT_API_TIMEOUT_MS);

		await assertion;
		expect(fetch).toHaveBeenCalledTimes(1);
	});

	test('preserves caller abort signals for cancellable searches', async () => {
		stubHangingFetch();
		const controller = new AbortController();
		const reason = new DOMException('User cancelled search', 'AbortError');

		const request = api.search('burial', 12, controller.signal);
		controller.abort(reason);

		await expect(request).rejects.toBe(reason);
	});

	test('uses longer timeout headroom for bulk queue injection', async () => {
		const { signals } = stubHangingFetch();

		const request = api.queueAppendMany([
			{
				kind: 'tidal',
				tidal_id: 101,
				artist: 'Artist',
				title: 'Track',
			},
		]);

		await vi.advanceTimersByTimeAsync(DEFAULT_API_TIMEOUT_MS);
		expect(signals[0]?.aborted).toBe(false);

		const assertion = expect(request).rejects.toBeInstanceOf(ApiTimeoutError);
		await vi.advanceTimersByTimeAsync(BULK_QUEUE_API_TIMEOUT_MS - DEFAULT_API_TIMEOUT_MS);
		await assertion;
	});

	test('keeps TIDAL mix loading alive through the bounded upstream timeout', async () => {
		const { signals } = stubHangingFetch();
		const request = api.getTidalMixTracks('mix-1');

		await vi.advanceTimersByTimeAsync(DEFAULT_API_TIMEOUT_MS);
		expect(signals[0]?.aborted).toBe(false);

		const assertion = expect(request).rejects.toMatchObject({
			timeoutMs: TIDAL_CATALOG_API_TIMEOUT_MS,
		});
		await vi.advanceTimersByTimeAsync(TIDAL_CATALOG_API_TIMEOUT_MS - DEFAULT_API_TIMEOUT_MS);
		await assertion;
	});

	test('authFetch keeps custom headers while attaching the bearer token', async () => {
		const fetch = vi.fn(async (_url: string | URL | Request, _init?: RequestInit) => new Response('{}'));
		vi.stubGlobal('fetch', fetch);
		vi.stubGlobal('localStorage', {
			getItem: vi.fn(() => 'test-token'),
			setItem: vi.fn(),
			removeItem: vi.fn(),
		});

		await authFetch('http://localhost:17600/api/status', {
			headers: { 'x-custom': 'kept' },
		});

		const [, init = {}] = fetch.mock.calls[0] as [
			string | URL | Request,
			RequestInit | undefined,
		];
		const headers = new Headers(init.headers);
		expect(headers.get('authorization')).toBe('Bearer test-token');
		expect(headers.get('x-custom')).toBe('kept');
	});
});
