import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import type { AudioSearchParams, AudioSearchResponse, AudioSearchResult, SearchResults, Track } from '$lib/api/client';
import { createLibrarySearch, type LibrarySearchDeps } from './library_search';

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

function track(id: number): Track {
	return {
		id,
		title: `Track ${id}`,
		artist_id: 1,
		artist_name: 'Artist',
		album_id: null,
		album_title: null,
		disc_number: null,
		track_number: null,
		duration_ms: 1000,
		isrc: null,
		tidal_id: null,
		best_quality: null,
		best_source: null,
		fidelity_score: 0,
		is_favorite: true,
		play_count: 0,
		last_played_at: null,
		date_added: null,
		source: 'tidal',
		artwork_url: null,
	} as Track;
}

function audioRow(id: number): AudioSearchResult {
	return {
		id,
		title: `Audio ${id}`,
		artist_name: 'Artist',
		album_title: null,
		duration_ms: 1000,
		tidal_id: null,
		is_favorite: true,
		play_count: 0,
		source: 'tidal',
		artwork_url: null,
	} as AudioSearchResult;
}

function textResults(...ids: number[]): SearchResults {
	return { tracks: ids.map(track), albums: [], artists: [] } as unknown as SearchResults;
}

function audioPage(ids: number[], total: number): AudioSearchResponse {
	return { tracks: ids.map(audioRow), total, unmatched_genres: [] };
}

// Every call returns its own pending promise so tests choose response order.
function harness() {
	const text: { query: string; d: ReturnType<typeof deferred<SearchResults>> }[] = [];
	const audio: { params: AudioSearchParams; signal?: AbortSignal; d: ReturnType<typeof deferred<AudioSearchResponse>> }[] = [];
	const onResults = vi.fn();
	const deps: LibrarySearchDeps = {
		search: vi.fn((query: string) => {
			const d = deferred<SearchResults>();
			text.push({ query, d });
			return d.promise;
		}),
		searchAudio: vi.fn((params: AudioSearchParams, signal?: AbortSignal) => {
			const d = deferred<AudioSearchResponse>();
			audio.push({ params, signal, d });
			return d.promise;
		}),
		onResults,
	};
	const search = createLibrarySearch(deps);
	return { search, text, audio, onResults, deps };
}

const DEBOUNCE = 220;

describe('library search request ownership', () => {
	beforeEach(() => {
		vi.useFakeTimers();
	});
	afterEach(() => {
		vi.useRealTimers();
	});

	test('debounces, then applies results for the current query', async () => {
		const { search, text, onResults } = harness();
		search.setQuery('  daft ');
		expect(get(search).status).toBe('pending');
		expect(text).toHaveLength(0);
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		expect(text.map((r) => r.query)).toEqual(['daft']);

		text[0].d.resolve(textResults(1, 2));
		await vi.runAllTimersAsync();
		expect(get(search)).toMatchObject({ query: 'daft', status: 'ready', error: null });
		expect(get(search).results.tracks.map((t) => t.id)).toEqual([1, 2]);
		expect(onResults).toHaveBeenCalledTimes(1);
	});

	test('an older query response cannot replace the newer query', async () => {
		const { search, text } = harness();
		search.setQuery('old');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		search.setQuery('new');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);

		text[1].d.resolve(textResults(2));
		await vi.runAllTimersAsync();
		text[0].d.resolve(textResults(1));
		await vi.runAllTimersAsync();

		expect(get(search).query).toBe('new');
		expect(get(search).results.tracks.map((t) => t.id)).toEqual([2]);
	});

	test('a query change invalidates in-flight work during the debounce window', async () => {
		const { search, text } = harness();
		search.setQuery('old');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		search.setQuery('newer');
		// Old response lands before the new request has even started.
		text[0].d.resolve(textResults(1));
		await vi.advanceTimersByTimeAsync(10);
		expect(get(search)).toMatchObject({ query: 'newer', status: 'pending' });
		expect(get(search).results.tracks).toEqual([]);
	});

	test('clearing the query resets immediately and ignores late success and failure', async () => {
		const { search, text, audio } = harness();
		search.setQuery('bpm:120');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		search.setQuery('');
		expect(get(search)).toMatchObject({ query: '', status: 'idle', total: null, error: null });
		expect(audio[0].signal?.aborted).toBe(true);

		audio[0].d.reject(new Error('late failure'));
		await vi.runAllTimersAsync();
		expect(get(search)).toMatchObject({ status: 'idle', error: null });
		expect(text).toHaveLength(0);
	});

	test('a stale failure cannot mark the current query failed', async () => {
		const { search, text } = harness();
		search.setQuery('one');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		search.setQuery('two');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		text[0].d.reject(new Error('one failed'));
		await vi.runAllTimersAsync();
		expect(get(search)).toMatchObject({ query: 'two', status: 'pending', error: null });
	});

	test('failure is distinct from an empty result, and retry recovers', async () => {
		const { search, text } = harness();
		search.setQuery('nothing');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		text[0].d.reject(new Error('server down'));
		await vi.runAllTimersAsync();
		expect(get(search)).toMatchObject({ status: 'error' });
		expect(get(search).error).toEqual(new Error('server down'));

		const retry = search.retry();
		expect(get(search)).toMatchObject({ status: 'pending', error: null });
		expect(text.map((r) => r.query)).toEqual(['nothing', 'nothing']);
		text[1].d.resolve(textResults());
		await retry;
		expect(get(search)).toMatchObject({ status: 'ready', error: null });
		expect(get(search).results.tracks).toEqual([]);
	});

	test('an old filtered page cannot append after the query changes', async () => {
		const { search, audio } = harness();
		search.setQuery('bpm:120');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		audio[0].d.resolve(audioPage([1, 2], 4));
		await vi.runAllTimersAsync();

		const more = search.loadMore();
		expect(audio[1].params.offset).toBe(2);
		search.setQuery('bpm:90');
		expect(audio[1].signal?.aborted).toBe(true);
		audio[1].d.resolve(audioPage([3, 4], 4));
		await more;
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		audio[2].d.resolve(audioPage([9], 1));
		await vi.runAllTimersAsync();

		expect(get(search).query).toBe('bpm:90');
		expect(get(search).results.tracks.map((t) => t.id)).toEqual([9]);
		expect(get(search).loadingMore).toBe(false);
	});

	test('overlapping Show more requests fetch and insert the page once', async () => {
		const { search, audio } = harness();
		search.setQuery('energy:>0.5');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		audio[0].d.resolve(audioPage([1, 2], 4));
		await vi.runAllTimersAsync();

		const a = search.loadMore();
		const b = search.loadMore();
		expect(audio).toHaveLength(2);
		audio[1].d.resolve(audioPage([2, 3], 4));
		await Promise.all([a, b]);
		expect(get(search).results.tracks.map((t) => t.id)).toEqual([1, 2, 3]);
		expect(get(search).total).toBe(4);
	});

	test('a failed page keeps results and retry reruns the same offset', async () => {
		const { search, audio } = harness();
		search.setQuery('key:Am');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		audio[0].d.resolve(audioPage([1], 2));
		await vi.runAllTimersAsync();

		const more = search.loadMore();
		audio[1].d.reject(new Error('page failed'));
		await more;
		expect(get(search)).toMatchObject({ status: 'ready', loadingMore: false });
		expect(get(search).moreError).toEqual(new Error('page failed'));
		expect(get(search).results.tracks.map((t) => t.id)).toEqual([1]);

		const retry = search.retry();
		expect(audio[2].params.offset).toBe(1);
		audio[2].d.resolve(audioPage([2], 2));
		await retry;
		expect(get(search).moreError).toBeNull();
		expect(get(search).results.tracks.map((t) => t.id)).toEqual([1, 2]);
	});

	test('plain-text search stays on the shared cached path (no abort signal)', async () => {
		const { search, deps } = harness();
		search.setQuery('cached');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		expect(deps.search).toHaveBeenCalledWith('cached', 100);
	});

	test('dispose cancels the pending debounce and drops in-flight results', async () => {
		const { search, text } = harness();
		search.setQuery('first');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		search.setQuery('second');
		search.dispose();
		await vi.advanceTimersByTimeAsync(DEBOUNCE * 2);
		expect(text).toHaveLength(1);
		text[0].d.resolve(textResults(1));
		await vi.runAllTimersAsync();
		expect(get(search).results.tracks).toEqual([]);
	});

	test('removeItems drops deleted rows from the current results', async () => {
		const { search, text } = harness();
		search.setQuery('x');
		await vi.advanceTimersByTimeAsync(DEBOUNCE);
		text[0].d.resolve(textResults(1, 2, 3));
		await vi.runAllTimersAsync();
		search.removeItems({ trackIds: new Set([2]) });
		expect(get(search).results.tracks.map((t) => t.id)).toEqual([1, 3]);
	});
});
