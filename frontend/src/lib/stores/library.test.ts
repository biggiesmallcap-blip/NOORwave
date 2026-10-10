import { beforeEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import type { Track } from '$lib/api/client';

const cachedApiMock = vi.hoisted(() => ({
	getTracks: vi.fn(),
}));

vi.mock('$lib/cache/api_queries', () => ({
	cachedApi: cachedApiMock,
}));

import {
	cancelTrackListRequests,
	isLoading,
	isLoadingMore,
	loadTracks,
	retryTrackList,
	totalTracks,
	trackListError,
	trackListRequestMatches,
	tracks,
	updateLibraryTrackFavorite,
} from './library';

type TracksPage = { tracks: Track[]; total: number };

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

// Each getTracks call gets its own pending promise so a test decides the
// order responses land in, independent of the order requests started.
function queueTrackResponses() {
	const pending: ReturnType<typeof deferred<TracksPage>>[] = [];
	cachedApiMock.getTracks.mockImplementation(() => {
		const d = deferred<TracksPage>();
		pending.push(d);
		return d.promise;
	});
	return pending;
}

function track(overrides: Partial<Track> = {}): Track {
	return {
		id: 1,
		title: 'Album Row',
		artist_id: 2,
		artist_name: 'Artist',
		artist_tidal_id: null,
		album_id: 3,
		album_title: 'Favorited Album',
		album_tidal_id: null,
		disc_number: null,
		track_number: null,
		duration_ms: 180000,
		isrc: null,
		tidal_id: null,
		best_quality: null,
		best_source: null,
		fidelity_score: 0,
		is_favorite: true,
		play_count: 0,
		last_played_at: null,
		date_added: '2026-01-01T00:00:00Z',
		source: 'tidal',
		artwork_url: null,
		...overrides,
	};
}

describe('library track favorite reconciliation', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		tracks.set([]);
		totalTracks.set(0);
	});

	test('keeps unliked tracks in the legacy library-track list', async () => {
		cachedApiMock.getTracks.mockResolvedValueOnce({
			tracks: [track()],
			total: 1,
		});

		await loadTracks('date_added', 'desc', 100, 0, false);
		updateLibraryTrackFavorite(1, false);

		expect(get(tracks)).toEqual([expect.objectContaining({ id: 1, is_favorite: false })]);
		expect(get(totalTracks)).toBe(1);
	});

	test('removes unliked tracks from the strict liked list', async () => {
		cachedApiMock.getTracks.mockResolvedValueOnce({
			tracks: [track()],
			total: 1,
		});

		await loadTracks('date_added', 'desc', 100, 0, true);
		updateLibraryTrackFavorite(1, false);

		expect(get(tracks)).toEqual([]);
		expect(get(totalTracks)).toBe(0);
	});

	test('adds newly liked tracks to the current list optimistically', async () => {
		cachedApiMock.getTracks.mockResolvedValueOnce({
			tracks: [],
			total: 0,
		});

		await loadTracks('date_added', 'desc', 100, 0, true);
		updateLibraryTrackFavorite(2, true, track({ id: 2, title: 'Fresh Like', is_favorite: false }));

		expect(get(tracks)[0]).toEqual(expect.objectContaining({ id: 2, is_favorite: true }));
		expect(get(totalTracks)).toBe(1);
	});
});

describe('track list request ownership', () => {
	beforeEach(() => {
		cancelTrackListRequests();
		cachedApiMock.getTracks.mockReset();
		tracks.set([]);
		totalTracks.set(0);
		vi.spyOn(console, 'error').mockImplementation(() => {});
	});

	test('an older Tracks response cannot replace a newer Liked selection', async () => {
		const pending = queueTrackResponses();
		const older = loadTracks('date_added', 'desc', 100, 0, false);
		const newer = loadTracks('date_added', 'desc', 100, 0, true);

		pending[1].resolve({ tracks: [track({ id: 7, title: 'Liked' })], total: 1 });
		await newer;
		pending[0].resolve({ tracks: [track({ id: 8 }), track({ id: 9 })], total: 500 });
		await older;

		expect(get(tracks).map((t) => t.id)).toEqual([7]);
		expect(get(totalTracks)).toBe(1);
		expect(get(isLoading)).toBe(false);
		// Strict Liked reconciliation still owns the visible list.
		updateLibraryTrackFavorite(7, false);
		expect(get(tracks)).toEqual([]);
		expect(get(totalTracks)).toBe(0);
	});

	test('a stale failure and finally cannot touch the current load', async () => {
		const pending = queueTrackResponses();
		const older = loadTracks('date_added', 'desc', 100, 0, false);
		const newer = loadTracks('title', 'asc', 100, 0, false);

		pending[0].reject(new Error('older failed'));
		await older;
		expect(get(isLoading)).toBe(true);
		expect(get(trackListError)).toBeNull();

		pending[1].resolve({ tracks: [track({ id: 3 })], total: 1 });
		await newer;
		expect(get(isLoading)).toBe(false);
		expect(get(tracks).map((t) => t.id)).toEqual([3]);
	});

	test('a page for an old query cannot append after the query changes', async () => {
		const pending = queueTrackResponses();
		const first = loadTracks('date_added', 'desc', 1, 0, false);
		pending[0].resolve({ tracks: [track({ id: 1 })], total: 3 });
		await first;

		const oldPage = loadTracks('date_added', 'desc', 1, 1, false);
		const reload = loadTracks('date_added', 'desc', 1, 0, true);
		pending[1].resolve({ tracks: [track({ id: 2 })], total: 3 });
		await oldPage;
		pending[2].resolve({ tracks: [track({ id: 5 })], total: 1 });
		await reload;

		expect(get(tracks).map((t) => t.id)).toEqual([5]);
		expect(get(totalTracks)).toBe(1);
	});

	test('a page request for a query other than the loaded one is ignored', async () => {
		const pending = queueTrackResponses();
		const first = loadTracks('date_added', 'desc', 1, 0, false);
		pending[0].resolve({ tracks: [track({ id: 1 })], total: 3 });
		await first;

		await loadTracks('title', 'asc', 1, 1, false);
		expect(cachedApiMock.getTracks).toHaveBeenCalledTimes(1);
		expect(get(tracks).map((t) => t.id)).toEqual([1]);
	});

	test('overlapping requests for the same page fetch and insert it once', async () => {
		const pending = queueTrackResponses();
		const first = loadTracks('date_added', 'desc', 1, 0, false);
		pending[0].resolve({ tracks: [track({ id: 1 })], total: 3 });
		await first;

		const a = loadTracks('date_added', 'desc', 1, 1, false);
		const b = loadTracks('date_added', 'desc', 1, 1, false);
		expect(get(isLoadingMore)).toBe(true);
		pending[1].resolve({ tracks: [track({ id: 2 })], total: 3 });
		await Promise.all([a, b]);

		expect(cachedApiMock.getTracks).toHaveBeenCalledTimes(2);
		expect(get(tracks).map((t) => t.id)).toEqual([1, 2]);
		expect(get(isLoadingMore)).toBe(false);
	});

	test('a failed first page is retryable and distinct from an empty list', async () => {
		const pending = queueTrackResponses();
		const failed = loadTracks('date_added', 'desc', 100, 0, true);
		pending[0].reject(new Error('offline'));
		await failed;

		expect(get(trackListError)).toEqual({ error: new Error('offline'), append: false });
		expect(get(isLoading)).toBe(false);

		const retry = retryTrackList();
		expect(get(trackListError)).toBeNull();
		expect(get(isLoading)).toBe(true);
		pending[1].resolve({ tracks: [], total: 0 });
		await retry;

		expect(cachedApiMock.getTracks).toHaveBeenLastCalledWith('date_added', 'desc', 100, 0, true, true);
		expect(get(trackListError)).toBeNull();
		expect(get(tracks)).toEqual([]);
	});

	test('a failed page keeps loaded rows and retries the same offset', async () => {
		const pending = queueTrackResponses();
		const first = loadTracks('date_added', 'desc', 1, 0, false);
		pending[0].resolve({ tracks: [track({ id: 1 })], total: 2 });
		await first;

		const page = loadTracks('date_added', 'desc', 1, 1, false);
		pending[1].reject(new Error('timeout'));
		await page;
		expect(get(tracks).map((t) => t.id)).toEqual([1]);
		expect(get(trackListError)).toEqual({ error: new Error('timeout'), append: true });

		const retry = retryTrackList();
		pending[2].resolve({ tracks: [track({ id: 2 })], total: 2 });
		await retry;
		expect(cachedApiMock.getTracks).toHaveBeenLastCalledWith('date_added', 'desc', 1, 1, true, false);
		expect(get(tracks).map((t) => t.id)).toEqual([1, 2]);
		expect(get(trackListError)).toBeNull();
	});

	test('cancelling (leaving the route) drops in-flight work and its loading state', async () => {
		const pending = queueTrackResponses();
		const first = loadTracks('date_added', 'desc', 100, 0, false);
		pending[0].resolve({ tracks: [track({ id: 1 })], total: 1 });
		await first;

		const liked = loadTracks('date_added', 'desc', 100, 0, true);
		expect(trackListRequestMatches('date_added', 'desc', true)).toBe(true);
		cancelTrackListRequests();
		expect(get(isLoading)).toBe(false);
		// The requested scope falls back to the rows held, so a return visit reloads.
		expect(trackListRequestMatches('date_added', 'desc', true)).toBe(false);
		expect(trackListRequestMatches('date_added', 'desc', false)).toBe(true);

		pending[1].resolve({ tracks: [track({ id: 9 })], total: 1 });
		await liked;
		expect(get(tracks).map((t) => t.id)).toEqual([1]);
		// Library semantics still own reconciliation for the rows on screen.
		updateLibraryTrackFavorite(1, false);
		expect(get(tracks)).toEqual([expect.objectContaining({ id: 1, is_favorite: false })]);
	});
});
