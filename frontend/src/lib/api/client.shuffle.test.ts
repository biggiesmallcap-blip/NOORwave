import { afterEach, describe, expect, test, vi } from 'vitest';

import { api } from './client';

describe('shuffle request payloads', () => {
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	test('replacePlaybackQueue sends a one shot shuffle mode when requested', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ queue: [] })));
		vi.stubGlobal('fetch', fetch);

		await api.replacePlaybackQueue([1, 2, 3], undefined, undefined, 'true');

		const init = (fetch.mock.calls[0] as unknown as [string, RequestInit])[1];
		expect(JSON.parse(String(init.body))).toEqual({
			track_ids: [1, 2, 3],
			shuffle_mode: 'true',
		});
	});

	test('playTidalMix sends a one shot shuffle mode when requested', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ ok: true })));
		vi.stubGlobal('fetch', fetch);

		await api.playTidalMix(
			[
				{
					tidal_id: 101,
					title: 'A',
					artist_name: null,
					album_title: null,
					artwork_url: null,
					duration_ms: null,
				},
				{
					tidal_id: 102,
					title: 'B',
					artist_name: null,
					album_title: null,
					artwork_url: null,
					duration_ms: null,
				},
			],
			'true'
		);

		const init = (fetch.mock.calls[0] as unknown as [string, RequestInit])[1];
		expect(JSON.parse(String(init.body))).toMatchObject({
			shuffle_mode: 'true',
			tracks: [
				{ tidal_track_id: 101, title: 'A' },
				{ tidal_track_id: 102, title: 'B' },
			],
		});
	});
});
