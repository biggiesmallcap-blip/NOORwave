import { writable, type Readable } from 'svelte/store';
import type {
	Album,
	Artist,
	AudioSearchParams,
	AudioSearchResponse,
	AudioSearchResult,
	SearchResults,
	Track,
} from '$lib/api/client';
import { buildAudioParams, hasAnyFilter } from '$lib/search/audio_params';
import { createLatestRequestGate } from '$lib/search/latest_request';
import { parseQuery } from '$lib/search/query_parser';

// Owns the library page's search: which query the results belong to, which
// response may apply, Show more paging, and a retryable failure. A newer query
// (or clearing, or disposing) invalidates every older success, failure and
// page immediately, including during the typing debounce.

export interface LibrarySearchResults {
	tracks: Track[];
	albums: Album[];
	artists: Artist[];
}

export interface LibrarySearchState {
	/** Trimmed query this state belongs to; '' when idle. */
	query: string;
	/** pending: typed or loading; error: the query failed (not an empty result). */
	status: 'idle' | 'pending' | 'ready' | 'error';
	results: LibrarySearchResults;
	/** Full matching-set size for filtered searches; null for plain text. */
	total: number | null;
	unmatchedGenres: string[];
	loadingMore: boolean;
	error: unknown;
	/** A Show more page failed; the loaded results stay. */
	moreError: unknown;
}

export interface LibrarySearchDeps {
	search(query: string, limit: number): Promise<SearchResults>;
	searchAudio(params: AudioSearchParams, signal?: AbortSignal): Promise<AudioSearchResponse>;
	/** Called after a new result set replaces the old one. */
	onResults?: () => void;
	debounceMs?: number;
}

export interface LibrarySearch extends Readable<LibrarySearchState> {
	setQuery(raw: string): void;
	loadMore(): Promise<void>;
	retry(): Promise<void>;
	removeItems(ids: { trackIds?: Set<number>; albumIds?: Set<number> }): void;
	dispose(): void;
}

const TEXT_SEARCH_LIMIT = 100;
const EMPTY_RESULTS: LibrarySearchResults = { tracks: [], albums: [], artists: [] };

function idleState(): LibrarySearchState {
	return {
		query: '',
		status: 'idle',
		results: EMPTY_RESULTS,
		total: null,
		unmatchedGenres: [],
		loadingMore: false,
		error: null,
		moreError: null,
	};
}

export function createLibrarySearch(deps: LibrarySearchDeps): LibrarySearch {
	const debounceMs = deps.debounceMs ?? 220;
	const store = writable<LibrarySearchState>(idleState());
	const gate = createLatestRequestGate();
	let state = idleState();
	let timer: ReturnType<typeof setTimeout> | null = null;
	// The request generation that loaded the current results; pages reuse it
	// (and its abort signal) so a page dies with its query.
	let current: { token: number; signal: AbortSignal } | null = null;
	let pendingPage: Promise<void> | null = null;

	function set(next: Partial<LibrarySearchState>) {
		state = { ...state, ...next };
		store.set(state);
	}

	function stop() {
		if (timer) clearTimeout(timer);
		timer = null;
		gate.invalidate();
		current = null;
		pendingPage = null;
	}

	function setQuery(raw: string) {
		const query = raw.trim();
		if (query === state.query && (state.status === 'pending' || state.status === 'ready')) return;
		stop();
		if (!query) {
			state = idleState();
			store.set(state);
			return;
		}
		// Keep the previous rows on screen while typing; status says they are
		// not this query's yet.
		set({ query, status: 'pending', error: null, moreError: null, loadingMore: false });
		timer = setTimeout(() => {
			timer = null;
			void run();
		}, debounceMs);
	}

	async function run() {
		const query = state.query;
		if (!query) return;
		const request = gate.begin();
		current = request;
		pendingPage = null;
		set({ status: 'pending', error: null, moreError: null, loadingMore: false });
		try {
			const parsed = parseQuery(query);
			if (hasAnyFilter(parsed)) {
				// DSP/filter syntax (bpm:138, key:Am, genre:dnb, ...) goes to audio search.
				const audio = await deps.searchAudio(buildAudioParams(parsed), request.signal);
				if (!gate.isCurrent(request.token)) return;
				set({
					status: 'ready',
					results: { tracks: adaptAudioTracks(audio.tracks), albums: [], artists: [] },
					total: audio.total ?? null,
					unmatchedGenres: audio.unmatched_genres ?? [],
				});
			} else {
				// Plain text uses the shared cached FTS query. No signal: passing one
				// bypasses the cache other callers share.
				const r = await deps.search(query, TEXT_SEARCH_LIMIT);
				if (!gate.isCurrent(request.token)) return;
				set({
					status: 'ready',
					results: { tracks: r.tracks, albums: r.albums, artists: r.artists },
					total: null,
					unmatchedGenres: [],
				});
			}
			deps.onResults?.();
		} catch (error) {
			if (!gate.isCurrent(request.token)) return;
			set({
				status: 'error',
				error: error ?? new Error('Search failed'),
				results: EMPTY_RESULTS,
				total: null,
				unmatchedGenres: [],
			});
		}
	}

	// "Show more" for filtered searches: page past the display cap with the
	// server-side offset, appending without disturbing already-loaded rows.
	function loadMore(): Promise<void> {
		if (pendingPage) return pendingPage;
		const request = current;
		if (!request || !gate.isCurrent(request.token) || state.status !== 'ready') return Promise.resolve();
		const parsed = parseQuery(state.query);
		if (!hasAnyFilter(parsed)) return Promise.resolve();
		if (state.total !== null && state.results.tracks.length >= state.total) return Promise.resolve();
		const offset = state.results.tracks.length;
		set({ loadingMore: true, moreError: null });
		const page = (async () => {
			try {
				const audio = await deps.searchAudio({ ...buildAudioParams(parsed), offset }, request.signal);
				if (!gate.isCurrent(request.token)) return;
				const seen = new Set(state.results.tracks.map((t) => t.id));
				set({
					results: {
						...state.results,
						tracks: [...state.results.tracks, ...adaptAudioTracks(audio.tracks).filter((t) => !seen.has(t.id))],
					},
					total: audio.total ?? state.total,
				});
			} catch (error) {
				if (!gate.isCurrent(request.token)) return;
				set({ moreError: error ?? new Error('Search failed') });
			} finally {
				if (gate.isCurrent(request.token)) {
					pendingPage = null;
					set({ loadingMore: false });
				}
			}
		})();
		pendingPage = page;
		return page;
	}

	function retry(): Promise<void> {
		if (state.status === 'error') {
			if (timer) clearTimeout(timer);
			timer = null;
			return run();
		}
		if (state.moreError) return loadMore();
		return Promise.resolve();
	}

	function removeItems({ trackIds, albumIds }: { trackIds?: Set<number>; albumIds?: Set<number> }) {
		set({
			results: {
				tracks: trackIds ? state.results.tracks.filter((t) => !trackIds.has(t.id)) : state.results.tracks,
				albums: albumIds ? state.results.albums.filter((a) => !albumIds.has(a.id)) : state.results.albums,
				artists: state.results.artists,
			},
		});
	}

	return {
		subscribe: store.subscribe,
		setQuery,
		loadMore,
		retry,
		removeItems,
		dispose: stop,
	};
}

export function adaptAudioTracks(rows: AudioSearchResult[]): Track[] {
	return rows.map((r) => ({
		id: r.id,
		title: r.title,
		artist_id: 0,
		artist_name: r.artist_name,
		album_id: null,
		album_title: r.album_title,
		disc_number: null,
		track_number: null,
		duration_ms: r.duration_ms,
		isrc: null,
		tidal_id: r.tidal_id,
		best_quality: null,
		best_source: null,
		fidelity_score: 0,
		is_favorite: r.is_favorite,
		play_count: r.play_count,
		last_played_at: null,
		date_added: null,
		source: r.source,
		artwork_url: r.artwork_url,
		bpm: r.bpm,
		key_signature: r.key_signature,
		camelot_key: r.camelot_key,
		energy: r.energy,
		danceability: r.danceability,
	}));
}
