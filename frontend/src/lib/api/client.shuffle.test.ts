import { afterEach, describe, expect, test, vi } from 'vitest';

import { api, type TidalPlayable } from './client';

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

	test('playTidalMix serializes a 45 track loaded album body in order', async () => {
		const fetch = vi.fn(async () => new Response(JSON.stringify({ ok: true })));
		vi.stubGlobal('fetch', fetch);
		const tracks: TidalPlayable[] = Array.from({ length: 45 }, (_, index) => {
			const trackNumber = index + 1;
			return {
				tidal_id: 5852079300 + trackNumber,
				title: `Anthology Track ${trackNumber}`,
				artist_name: 'The Beatles',
				artist_tidal_id: 3634161,
				album_title: 'Anthology 2',
				album_tidal_id: 58520793,
				artwork_url: `https://resources.tidal.com/images/anthology-${trackNumber}/640x640.jpg`,
				duration_ms: 180_000 + trackNumber,
			};
		});

		await api.playTidalMix(tracks);

		const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
		const body = JSON.parse(String(init.body)) as {
			shuffle_mode?: string;
			tracks: Array<{
				tidal_track_id: number;
				title: string;
				artist_name: string | null;
				artist_tidal_id: number | null;
				album_title: string | null;
				album_tidal_id: number | null;
				artwork_url: string | null;
				duration_ms: number | null;
			}>;
		};

		expect(new URL(url, 'http://localhost:3334').pathname).toBe('/api/tidal/play-mix');
		expect(body.shuffle_mode).toBeUndefined();
		expect(body.tracks).toHaveLength(45);
		expect(body.tracks.map((track) => track.tidal_track_id)).toEqual(
			tracks.map((track) => track.tidal_id)
		);
		expect(body.tracks.every((track) => track.album_tidal_id === 58520793)).toBe(true);
		expect(body.tracks.every((track) => track.artist_tidal_id === 3634161)).toBe(true);
		expect(body.tracks.every((track) => track.artwork_url?.includes('resources.tidal.com'))).toBe(true);
		expect(body.tracks.at(0)).toMatchObject({
			tidal_track_id: tracks[0].tidal_id,
			title: 'Anthology Track 1',
			album_title: 'Anthology 2',
			duration_ms: 180_001,
		});
		expect(body.tracks.at(-1)).toMatchObject({
			tidal_track_id: tracks[44].tidal_id,
			title: 'Anthology Track 45',
			album_title: 'Anthology 2',
			duration_ms: 180_045,
		});
	});
});
