import { describe, expect, test, vi, beforeEach, afterEach } from 'vitest';
import type { SpotifyPlaylistDetail } from '$lib/api/client';
import {
	clearSpotifyChartMetaCache,
	getCachedSpotifyChartMeta,
	getCachedSpotifyChartMetaMap,
	putCachedSpotifyChartMeta,
} from './spotify-chart-meta-cache';

function playlist(title: string, thumbnail: string | null): SpotifyPlaylistDetail {
	return {
		source: 'spotify',
		spotifyId: 'spotify-id',
		type: 'playlist',
		title,
		description: null,
		thumbnail,
		owner: null,
		followers: null,
		totalTracks: null,
		snapshotId: null,
		tracks: [],
	};
}

describe('Spotify chart metadata cache', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-05-29T00:00:00Z'));
		clearSpotifyChartMetaCache();
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	test('returns cached playlist title and artwork without refetching', () => {
		putCachedSpotifyChartMeta('chart-1', playlist('Top 50', 'https://img.example/top.jpg'));

		expect(getCachedSpotifyChartMeta('chart-1')).toEqual({
			title: 'Top 50',
			thumbnail: 'https://img.example/top.jpg',
		});
		expect(getCachedSpotifyChartMetaMap(['chart-1', 'missing'])).toEqual({
			'chart-1': {
				title: 'Top 50',
				thumbnail: 'https://img.example/top.jpg',
			},
		});
	});

	test('expires playlist metadata after six hours', () => {
		putCachedSpotifyChartMeta('chart-1', playlist('Top 50', null));

		vi.setSystemTime(new Date('2026-05-29T07:00:00Z'));

		expect(getCachedSpotifyChartMeta('chart-1')).toBeNull();
	});
});
