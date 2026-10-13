// The Archive's result feed: one debounced, abortable request per lens state,
// then pages appended on scroll. A newer lens state aborts whatever is in
// flight (first page or a page of more), and a late answer for an old state
// is dropped by sequence number even if the abort came too late.

import type { ArchiveQuery, ArchiveResponse, ArchiveSong } from '$lib/api/video_explore_types';
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
}

export const EMPTY_FEED: FeedState = {
	phase: 'idle',
	key: null,
	items: [],
	total: 0,
	facets: null,
	loadingMore: false,
	error: null,
};

export interface ArchiveFeedOptions {
	fetch: ArchiveFetch;
	onChange: (state: FeedState) => void;
	pageSize?: number;
	debounceMs?: number;
}

export interface ArchiveFeed {
	/** Ask for a lens state. Equal to the current one: no-op unless it failed. */
	request(lenses: ArchiveLenses, opts?: { immediate?: boolean }): void;
	/** Append the next page, if there is one and nothing else is loading. */
	loadMore(): void;
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
	let state: FeedState = { ...EMPTY_FEED };
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

	async function runFirstPage(target: ArchiveLenses, mySeq: number) {
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
			set({
				phase: 'ready',
				items: uniqueSongs(response.items),
				total: response.total,
				facets: response.facets,
				loadingMore: false,
				error: null,
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
			const failed = state.phase === 'error' || state.phase === 'unavailable';
			if (key === state.key && state.phase !== 'idle' && !failed) return;
			// The very first ask has nothing on screen to protect: no debounce.
			start(target, opts.immediate ?? state.phase === 'idle');
		},
		loadMore() {
			if (disposed || !lenses || state.phase !== 'ready' || state.loadingMore) return;
			if (state.items.length >= state.total) return;
			const mySeq = seq;
			const target = lenses;
			const ctrl = new AbortController();
			controller = ctrl;
			set({ loadingMore: true });
			void options
				.fetch(lensesToQuery(target, state.items.length, pageSize), ctrl.signal)
				.then((response) => {
					if (mySeq !== seq) return;
					controller = null;
					const seen = new Set(state.items.map((item) => item.song_key));
					const fresh = response.items.filter((item) => !seen.has(item.song_key));
					set({
						items: [...state.items, ...fresh],
						total: fresh.length === 0 ? state.items.length : response.total,
						loadingMore: false,
					});
				})
				.catch((error) => {
					if (mySeq !== seq || isAbort(error)) return;
					controller = null;
					// A failed page ends the scroll quietly; the rows already shown stay.
					set({ loadingMore: false, total: state.items.length });
				});
		},
		retry() {
			if (disposed || !lenses) return;
			start(lenses, true);
		},
		get state() {
			return state;
		},
		get hasMore() {
			return state.phase === 'ready' && state.items.length < state.total;
		},
		dispose() {
			disposed = true;
			abortInFlight();
			seq += 1;
		},
	};
}
