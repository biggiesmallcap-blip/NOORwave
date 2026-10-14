// The Archive's result feed: one debounced, abortable request per lens state,
// then pages appended on scroll. A newer lens state aborts whatever is in
// flight (first page or a page of more), and a late answer for an old state
// is dropped by sequence number even if the abort came too late.
//
// The loaded window need not start at the top: a jump (the scrubber's letter
// or decade) loads the page at that offset, and the window then grows both
// ways, later pages appended and earlier ones prepended. `base` is the
// offset of the first loaded item.

import type { ArchiveJump, ArchiveQuery, ArchiveResponse, ArchiveSong } from '$lib/api/video_explore_types';
import { lensKey, lensesToQuery, type ArchiveLenses } from './lenses';

export type ArchiveFetch = (query: ArchiveQuery, signal: AbortSignal) => Promise<ArchiveResponse>;

export type FeedPhase =
	/** Nothing asked yet. */
	| 'idle'
	/** First page for a new lens state, nothing to show yet. */
	| 'loading'
	/** First page for a new lens state while the previous results stay up. */
	| 'refreshing'
	| 'ready'
	/** The endpoint answered 501: the catalog build has not shipped yet. */
	| 'unavailable'
	| 'error';

export interface FeedState {
	phase: FeedPhase;
	key: string | null;
	items: ArchiveSong[];
	total: number;
	facets: ArchiveResponse['facets'] | null;
	loadingMore: boolean;
	error: unknown;
	/** Offset of items[0] in the whole result. */
	base: number;
	/** Sections of the whole result (A-Z and year sorts). */
	jumps: ArchiveJump[];
	loadingEarlier: boolean;
}

export const EMPTY_FEED: FeedState = {
	phase: 'idle',
	key: null,
	items: [],
	total: 0,
	facets: null,
	loadingMore: false,
	error: null,
	base: 0,
	jumps: [],
	loadingEarlier: false,
};

export interface ArchiveFeedOptions {
	fetch: ArchiveFetch;
	onChange: (state: FeedState) => void;
	pageSize?: number;
	debounceMs?: number;
	/** A first page kept from an earlier visit: shown at once, then asked
	 *  again quietly (no refreshing state) on the first request for it. */
	seed?: FeedState | null;
}

export interface ArchiveFeed {
	/** Ask for a lens state. Equal to the current one: no-op unless it failed. */
	request(lenses: ArchiveLenses, opts?: { immediate?: boolean }): void;
	/** Append the next page, if there is one and nothing else is loading. */
	loadMore(): void;
	/** Prepend the page before the loaded window, after a jump. */
	loadEarlier(): void;
	/** Show the result from `offset`: the index of that item in `items`
	 *  once it is there (no request when it is already loaded), or null
	 *  when the feed moved on meanwhile. */
	jumpTo(offset: number): Promise<number | null>;
	/** Ask again for the current lens state (Retry, or a building poll). */
	retry(): void;
	readonly state: FeedState;
	readonly hasMore: boolean;
	dispose(): void;
}

export function isAbort(error: unknown): boolean {
	return (error as { name?: string } | null)?.name === 'AbortError';
}

export function errorStatus(error: unknown): number | null {
	const status = (error as { status?: unknown } | null)?.status;
	return typeof status === 'number' ? status : null;
}

/** One tile per song: the grid is keyed by song key, so a repeat would break
 *  the render rather than just show twice. */
export function uniqueSongs(items: readonly ArchiveSong[]): ArchiveSong[] {
	const seen = new Set<string>();
	return items.filter((item) => {
		if (seen.has(item.song_key)) return false;
		seen.add(item.song_key);
		return true;
	});
}

export function createArchiveFeed(options: ArchiveFeedOptions): ArchiveFeed {
	const pageSize = options.pageSize ?? 60;
	const debounceMs = options.debounceMs ?? 220;
	let state: FeedState =
		options.seed?.phase === 'ready' ? { ...options.seed, loadingMore: false, loadingEarlier: false } : { ...EMPTY_FEED };
	let seeded = state.phase === 'ready';
	let lenses: ArchiveLenses | null = null;
	let seq = 0;
	let controller: AbortController | null = null;
	let timer: ReturnType<typeof setTimeout> | null = null;
	let disposed = false;

	function set(patch: Partial<FeedState>) {
		state = { ...state, ...patch };
		if (!disposed) options.onChange(state);
	}

	function abortInFlight() {
		if (timer) clearTimeout(timer);
		timer = null;
		controller?.abort();
		controller = null;
	}

	async function runFirstPage(target: ArchiveLenses, mySeq: number, quiet = false) {
		timer = null;
		const ctrl = new AbortController();
		controller = ctrl;
		try {
			const response = await options.fetch(lensesToQuery(target, 0, pageSize), ctrl.signal);
			if (mySeq !== seq) return;
			controller = null;
			// The server says when no catalog build has finished yet; an empty
			// answer then means "not catalogued", not "no matches".
			if (response.building && response.total === 0) {
				set({ phase: 'unavailable', loadingMore: false, error: null });
				return;
			}
			// A quiet check of a kept page that the listener has since scrolled
			// on from only updates the counts, never the tiles under them.
			if (quiet && (state.base !== 0 || state.items.length > pageSize)) {
				set({ total: response.total, facets: response.facets });
				return;
			}
			set({
				phase: 'ready',
				items: uniqueSongs(response.items),
				total: response.total,
				facets: response.facets,
				loadingMore: false,
				loadingEarlier: false,
				error: null,
				base: 0,
				jumps: response.jumps ?? [],
			});
		} catch (error) {
			if (mySeq !== seq || isAbort(error)) return;
			controller = null;
			set({
				phase: errorStatus(error) === 501 ? 'unavailable' : 'error',
				loadingMore: false,
				error,
			});
		}
	}

	function start(target: ArchiveLenses, immediate: boolean) {
		abortInFlight();
		const mySeq = ++seq;
		lenses = target;
		const hasResults = state.phase === 'ready' || state.phase === 'refreshing';
		set({
			phase: hasResults && state.items.length > 0 ? 'refreshing' : 'loading',
			key: lensKey(target),
			loadingMore: false,
			error: null,
		});
		if (immediate || debounceMs <= 0) void runFirstPage(target, mySeq);
		else timer = setTimeout(() => void runFirstPage(target, mySeq), debounceMs);
	}

	return {
		request(target, opts = {}) {
			if (disposed) return;
			const key = lensKey(target);
			if (seeded) {
				seeded = false;
				if (key === state.key) {
					lenses = target;
					void runFirstPage(target, ++seq, true);
					return;
				}
			}
			const failed = state.phase === 'error' || state.phase === 'unavailable';
			if (key === state.key && state.phase !== 'idle' && !failed) return;
			// The very first ask has nothing on screen to protect: no debounce.
			start(target, opts.immediate ?? state.phase === 'idle');
		},
		loadMore() {
			if (disposed || !lenses || state.phase !== 'ready' || state.loadingMore) return;
			if (state.base + state.items.length >= state.total) return;
			const mySeq = seq;
			const target = lenses;
			const ctrl = new AbortController();
			controller = ctrl;
			set({ loadingMore: true });
			void options
				.fetch(lensesToQuery(target, state.base + state.items.length, pageSize), ctrl.signal)
				.then((response) => {
					if (mySeq !== seq) return;
					controller = null;
					const seen = new Set(state.items.map((item) => item.song_key));
					const fresh = response.items.filter((item) => !seen.has(item.song_key));
					set({
						items: [...state.items, ...fresh],
						total: fresh.length === 0 ? state.base + state.items.length : response.total,
						loadingMore: false,
					});
				})
				.catch((error) => {
					if (mySeq !== seq || isAbort(error)) return;
					controller = null;
					// A failed page ends the scroll quietly; the rows already shown stay.
					set({ loadingMore: false, total: state.base + state.items.length });
				});
		},
		loadEarlier() {
			if (disposed || !lenses || state.phase !== 'ready' || state.loadingEarlier || state.base <= 0) return;
			const mySeq = seq;
			const target = lenses;
			const from = Math.max(0, state.base - pageSize);
			const limit = state.base - from;
			set({ loadingEarlier: true });
			void options
				.fetch(lensesToQuery(target, from, limit), new AbortController().signal)
				.then((response) => {
					if (mySeq !== seq) return;
					const seen = new Set(state.items.map((item) => item.song_key));
					const fresh = response.items.filter((item) => !seen.has(item.song_key));
					set({ items: [...fresh, ...state.items], base: from, loadingEarlier: false });
				})
				.catch(() => {
					if (mySeq !== seq) return;
					set({ loadingEarlier: false });
				});
		},
		async jumpTo(offset) {
			if (disposed || !lenses || state.phase !== 'ready') return null;
			const target = Math.max(0, Math.min(offset, Math.max(0, state.total - 1)));
			if (target >= state.base && target < state.base + state.items.length) return target - state.base;
			// A window of its own: whatever was loading for the old one stops.
			controller?.abort();
			controller = null;
			const mySeq = ++seq;
			const ctrl = new AbortController();
			controller = ctrl;
			set({ loadingMore: false, loadingEarlier: false });
			try {
				const response = await options.fetch(lensesToQuery(lenses, target, pageSize), ctrl.signal);
				if (mySeq !== seq) return null;
				controller = null;
				set({
					items: uniqueSongs(response.items),
					total: response.total,
					base: target,
					jumps: response.jumps ?? state.jumps,
				});
				return 0;
			} catch (error) {
				if (mySeq !== seq || isAbort(error)) return null;
				controller = null;
				return null;
			}
		},
		retry() {
			if (disposed || !lenses) return;
			start(lenses, true);
		},
		get state() {
			return state;
		},
		get hasMore() {
			return state.phase === 'ready' && state.base + state.items.length < state.total;
		},
		dispose() {
			disposed = true;
			abortInFlight();
			seq += 1;
		},
	};
}
