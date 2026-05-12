import { afterEach, describe, expect, test, vi } from 'vitest';

import { startTidalSync } from './tidal';

describe('TIDAL sync requests', () => {
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	test('starts normal sync without forcing full mode', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ status: 'sync_started' })));
		vi.stubGlobal('fetch', fetch);

		await startTidalSync();

		expect(fetch).toHaveBeenCalledWith(
			'http://localhost:3334/api/tidal/sync',
			expect.objectContaining({ method: 'POST' })
		);
	});

	test('starts full resync with explicit full mode', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ status: 'sync_started' })));
		vi.stubGlobal('fetch', fetch);

		await startTidalSync('full');

		expect(fetch).toHaveBeenCalledWith(
			'http://localhost:3334/api/tidal/sync?mode=full',
			expect.objectContaining({ method: 'POST' })
		);
	});
});
