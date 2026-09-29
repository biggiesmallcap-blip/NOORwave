import { describe, expect, test, vi } from 'vitest';
import type {
	ArtistReleaseFilterStatuses,
	TidalDiscographyAlbum,
} from '$lib/api/client';
import {
	continueReleaseSection,
	failedPreviewReleaseLinks,
	previewReleaseState,
	releaseSectionComplete,
} from './artist_release_loading';

function albums(first: number, count: number): TidalDiscographyAlbum[] {
	return Array.from({ length: count }, (_, index) => ({
		tidal_id: first + index,
		title: `Release ${first + index}`,
		source_filter: 'EPSANDSINGLES',
	} as TidalDiscographyAlbum));
}

function statuses(single: { failed: boolean; has_more: boolean | null }): ArtistReleaseFilterStatuses {
	return {
		ALBUMS: { failed: false, has_more: false },
		EPSANDSINGLES: single,
		COMPILATIONS: { failed: false, has_more: false },
		LIVE: { failed: false, has_more: false },
	};
}

describe('artist release continuation', () => {
	test('a failed preview filter keeps a navigation path and retries from page zero', async () => {
		const preview = statuses({ failed: true, has_more: null });
		expect(failedPreviewReleaseLinks(preview)).toEqual([
			{ section: 'singles', label: 'Browse singles and EPs →', path: '/discography/singles' },
		]);
		const initial = previewReleaseState('singles', [], preview);
		const fetchPage = vi.fn(async () => ({ albums: albums(1, 3), status: { failed: false, has_more: false } }));
		const result = await continueReleaseSection(initial, fetchPage);
		expect(fetchPage).toHaveBeenCalledWith('EPSANDSINGLES', 0);
		expect(result.albums).toHaveLength(3);
		expect(releaseSectionComplete(result)).toBe(true);
	});

	test('a failed continuation retains 50 preview releases and stays retryable', async () => {
		const initial = previewReleaseState('singles', albums(1, 50), statuses({ failed: false, has_more: true }));
		const result = await continueReleaseSection(initial, async () => { throw new Error('TIDAL timeout'); });
		expect(result.albums.map((album) => album.tidal_id)).toEqual(albums(1, 50).map((album) => album.tidal_id));
		expect(result.filters[0]).toEqual({
			filter: 'EPSANDSINGLES',
			status: { failed: true, has_more: null },
			nextOffset: 50,
		});
		expect(releaseSectionComplete(result)).toBe(false);
	});

	test('continues only Singles and merges 70 more unique releases', async () => {
		const initial = previewReleaseState('singles', albums(1, 50), statuses({ failed: false, has_more: true }));
		const fetchPage = vi.fn(async (filter: string, offset: number) => {
			expect(filter).toBe('EPSANDSINGLES');
			return offset === 50
				? { albums: albums(51, 50), status: { failed: false, has_more: true } }
				: { albums: [albums(100, 1)[0], ...albums(101, 20)], status: { failed: false, has_more: false } };
		});
		const result = await continueReleaseSection(initial, fetchPage);
		expect(fetchPage.mock.calls.map(([, offset]) => offset)).toEqual([50, 100]);
		expect(result.albums).toHaveLength(120);
		expect(new Set(result.albums.map((album) => album.tidal_id)).size).toBe(120);
		expect(releaseSectionComplete(result)).toBe(true);
	});

	test('retry resumes at the failed offset without fetching the preview again', async () => {
		const initial = previewReleaseState('singles', albums(1, 50), statuses({ failed: false, has_more: true }));
		const failed = await continueReleaseSection(initial, async () => ({
			albums: [], status: { failed: true, has_more: null },
		}));
		const fetchPage = vi.fn(async () => ({ albums: albums(51, 1), status: { failed: false, has_more: false } }));
		const recovered = await continueReleaseSection(failed, fetchPage);
		expect(fetchPage).toHaveBeenCalledTimes(1);
		expect(fetchPage).toHaveBeenCalledWith('EPSANDSINGLES', 50);
		expect(recovered.albums).toHaveLength(51);
		expect(releaseSectionComplete(recovered)).toBe(true);
	});
});
