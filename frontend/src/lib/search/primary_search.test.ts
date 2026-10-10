import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { SearchResults, TidalSearchResults, TidalSearchTrack } from '$lib/api/client';
import { runPrimarySearch, type PrimarySearch } from './primary_search';

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (error: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

const noLocal: SearchResults = { tracks: [], albums: [], artists: [] };

function tidalWith(title: string): TidalSearchResults {
	const track = { tidal_id: 1, title, in_library: false } as TidalSearchTrack;
	return { tracks: [track], albums: [], artists: [], videos: [] };
}

function harness(overrides: Partial<PrimarySearch> = {}) {
	const local = deferred<SearchResults>();
	const tidal = deferred<TidalSearchResults>();
	const shown: TidalSearchResults[] = [];
	const events: string[] = [];
	let current = true;
	const run = runPrimarySearch({
		local: local.promise,
		tidal: tidal.promise,
		isCurrent: () => current,
		show: (r) => shown.push(r),
		onTidal: () => events.push('tidal'),
		onTidalError: (_e, hadLocal) => events.push(`tidal-error:${hadLocal}`),
		onLocalSettled: () => events.push('local-settled'),
		onTidalSettled: () => events.push('tidal-settled'),
		holdMs: 300,
		...overrides,
	});
	return { local, tidal, shown, events, run, stale: () => (current = false) };
}

describe('runPrimarySearch', () => {
	beforeEach(() => vi.useFakeTimers({ toFake: ['setTimeout'] }));
	afterEach(() => vi.useRealTimers());

	test('TIDAL arriving inside the hold shows one merged list', async () => {
		const h = harness();
		h.local.resolve(noLocal);
		await vi.advanceTimersByTimeAsync(100);
		h.tidal.resolve(tidalWith('Song'));
		await vi.advanceTimersByTimeAsync(400);
		await h.run;
		expect(h.shown).toHaveLength(1);
		expect(h.shown[0].tracks[0].title).toBe('Song');
		expect(h.events).toContain('tidal');
	});

	test('a slow TIDAL still gets a local-only view after the hold, then the merge', async () => {
		const h = harness();
		h.local.resolve(noLocal);
		await vi.advanceTimersByTimeAsync(300);
		expect(h.shown).toHaveLength(1);
		expect(h.shown[0].tracks).toHaveLength(0);
		h.tidal.resolve(tidalWith('Late'));
		await vi.advanceTimersByTimeAsync(0);
		await h.run;
		expect(h.shown).toHaveLength(2);
		expect(h.shown[1].tracks[0].title).toBe('Late');
	});

	test('a superseded search shows nothing and settles silently', async () => {
		const h = harness();
		h.stale();
		h.local.resolve(noLocal);
		h.tidal.resolve(tidalWith('Old'));
		await vi.advanceTimersByTimeAsync(400);
		await h.run;
		expect(h.shown).toHaveLength(0);
		expect(h.events).toHaveLength(0);
	});

	test('a TIDAL failure reports whether library results are already showing', async () => {
		const h = harness();
		h.local.resolve(noLocal);
		await vi.advanceTimersByTimeAsync(0);
		h.tidal.reject(new Error('timeout'));
		await vi.advanceTimersByTimeAsync(400);
		await h.run;
		expect(h.events).toContain('tidal-error:true');
		expect(h.events).toContain('local-settled');
		expect(h.events).toContain('tidal-settled');
	});

	test('the run resolves only after both providers settle', async () => {
		const h = harness();
		let done = false;
		void h.run.then(() => (done = true));
		h.local.resolve(noLocal);
		await vi.advanceTimersByTimeAsync(0);
		expect(done).toBe(false);
		h.tidal.resolve(tidalWith('x'));
		await vi.advanceTimersByTimeAsync(0);
		await h.run;
		expect(done).toBe(true);
	});
});
