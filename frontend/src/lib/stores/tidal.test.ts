import { afterEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';

import { loadTidalStatus, startTidalSync, tidalStatus } from './tidal';

describe('TIDAL sync requests', () => {
	afterEach(() => {
		vi.clearAllTimers();
		vi.useRealTimers();
		vi.unstubAllGlobals();
	});

	test('starts normal sync without forcing full mode', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ status: 'sync_started' })));
		vi.stubGlobal('fetch', fetch);

		await startTidalSync();

		expect(fetch).toHaveBeenCalledWith(
			'http://localhost:17600/api/tidal/sync',
			expect.objectContaining({ method: 'POST' })
		);
	});

	test('starts full resync with explicit full mode', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ status: 'sync_started' })));
		vi.stubGlobal('fetch', fetch);

		await startTidalSync('full');

		expect(fetch).toHaveBeenCalledWith(
			'http://localhost:17600/api/tidal/sync?mode=full',
			expect.objectContaining({ method: 'POST' })
		);
	});
});

describe('TIDAL status recovery', () => {
	afterEach(() => {
		vi.clearAllTimers();
		vi.useRealTimers();
		vi.unstubAllGlobals();
	});

	test('coalesces concurrent status checks into one request', async () => {
		let resolveFetch!: (response: Response) => void;
		const fetch = vi.fn(
			() =>
				new Promise<Response>((resolve) => {
					resolveFetch = resolve;
				})
		);
		vi.stubGlobal('fetch', fetch);

		const first = loadTidalStatus();
		const second = loadTidalStatus();

		expect(fetch).toHaveBeenCalledTimes(1);
		expect(second).toBe(first);
		resolveFetch(new Response(JSON.stringify({ connected: true, user_id: 'u-1' })));
		await Promise.all([first, second]);
		expect(get(tidalStatus)).toBe('connected');
	});

	test('continues the retry budget after a transient network failure', async () => {
		vi.useFakeTimers();
		const fetch = vi
			.fn()
			.mockRejectedValueOnce(new TypeError('network unavailable'))
			.mockResolvedValueOnce(
				new Response(JSON.stringify({ connected: true, user_id: 'u-1' }))
			);
		vi.stubGlobal('fetch', fetch);

		await loadTidalStatus();
		expect(fetch).toHaveBeenCalledTimes(1);

		await vi.advanceTimersByTimeAsync(1500);
		expect(fetch).toHaveBeenCalledTimes(2);
		expect(get(tidalStatus)).toBe('connected');
	});
});
