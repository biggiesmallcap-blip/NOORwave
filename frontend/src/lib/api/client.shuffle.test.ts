import { afterEach, describe, expect, test, vi } from 'vitest';

import { api } from './client';

describe('canonical queue replacement payloads', () => {
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	test('sends library rows and an explicit shuffle mode', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ queue: [] })));
		vi.stubGlobal('fetch', fetch);

		await api.replacePlaybackQueue(
			[{ track_id: 1 }, { track_id: 2 }, { track_id: 3 }],
			{ shuffleMode: 'true', startPlayback: true }
		);

		const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
		expect(new URL(url, 'http://localhost:17600').pathname).toBe('/api/playback/queue');
		expect(JSON.parse(String(init.body))).toEqual({
			items: [{ track_id: 1 }, { track_id: 2 }, { track_id: 3 }],
			shuffle_mode: 'true',
			start_playback: true,
		});
	});

	test('serializes a metadata-rich TIDAL album in order', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ queue: [] })));
		vi.stubGlobal('fetch', fetch);
		const items = Array.from({ length: 45 }, (_, index) => {
			const trackNumber = index + 1;
			return {
				tidal_id: 5852079300 + trackNumber,
				title: `Anthology Track ${trackNumber}`,
				artist: 'The Beatles',
				artist_tidal_id: 3634161,
				album_title: 'Anthology 2',
				album_tidal_id: 58520793,
				artwork_url: `https://resources.tidal.com/images/anthology-${trackNumber}/640x640.jpg`,
				duration_ms: 180_000 + trackNumber,
			};
		});

		await api.replacePlaybackQueue(items, { startPlayback: true });

		const [, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
		const body = JSON.parse(String(init.body)) as { items: typeof items; start_playback?: boolean };
		expect(body.start_playback).toBe(true);
		expect(body.items).toHaveLength(45);
		expect(body.items.map((track) => track.tidal_id)).toEqual(items.map((track) => track.tidal_id));
		expect(body.items.every((track) => track.album_tidal_id === 58520793)).toBe(true);
		expect(body.items.every((track) => track.artist_tidal_id === 3634161)).toBe(true);
	});
});