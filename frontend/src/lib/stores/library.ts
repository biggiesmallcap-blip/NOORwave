import { derived, get, writable } from 'svelte/store';
import { type Track, type Album, type Artist } from '$lib/api/client';
import { cachedApi } from '$lib/cache/api_queries';
import { createLatestRequestGate } from '$lib/search/latest_request';
import { createPersistedStore, oneOf } from './persisted';
import { createSelection } from './selection';

export const tracks = writable<Track[]>([]);
export const albums = writable<Album[]>([]);
export const artists = writable<Artist[]>([]);
export const totalTracks = writable(0);
export const totalAlbums = writable(0);
// Albums/artists still share these flags; the track list owns its own so an
// album load finishing cannot clear a track load (or the reverse).
const browseLoading = writable(false);
const browseLoadingMore = writable(false);
const trackListLoading = writable(false);
const trackListLoadingMore = writable(false);
const trackListErrorState = writable<{ error: unknown; append: boolean } | null>(null);
export const isLoading = derived(
	[browseLoading, trackListLoading],
	([browse, trackList]) => browse || trackList,
);
export const isLoadingMore = derived(
	[browseLoadingMore, trackListLoadingMore],
	([browse, trackList]) => browse || trackList,
);
// The newest track-list failure, null while loading and after success, so a
// failure never reads as an empty library. append=false means the requested
// query itself failed, so the rows still in $tracks belong to an older query
// and must not be presented as its result; append=true means only a later
// page failed and the loaded rows are still this query's.
export const trackListError = { subscribe: trackListErrorState.subscribe };

export const sortBy = writable('date_added');
export const sortDir = writable<'asc' | 'desc'>('desc');

// Album grid/list choice persists across sessions (localStorage), not just per
// history entry, so the layout the user picked survives a reload/relaunch.
// `createPersistedStore` carries the storage guards; see its header for why
// each one is load-bearing.
export const viewMode = createPersistedStore<'grid' | 'list'>(
	'library.viewMode',
	'grid',
	{ parse: oneOf(['grid', 'list'] as const) },
);
export const searchQuery = writable('');

// Selected tracks and albums for batch operations. Library-scoped instances;
// other surfaces build their own via `createSelection()` rather than sharing
// these, so a selection on one page does not appear in library's batch bar.
const trackSelection = createSelection();
const albumSelection = createSelection();

export const selectedTrackIds = trackSelection.ids;
export const selectedAlbumIds = albumSelection.ids;
export const lastSelectedTrackId = trackSelection.lastId;
export const lastSelectedAlbumId = albumSelection.lastId;

const PAGE_SIZE = 100;

interface TrackListQuery {
	sort: string;
	dir: string;
	likedOnly: boolean;
}

interface TrackListPage extends TrackListQuery {
	limit: number;
	offset: number;
}

function sameTrackListQuery(a: TrackListQuery | null, b: TrackListQuery): boolean {
	return a !== null && a.sort === b.sort && a.dir === b.dir && a.likedOnly === b.likedOnly;
}

// One owner for the track list: only the newest first-page request (a query)
// may write rows, totals, loading or error state, and a page appends only to
// the query it was requested for. Responses are not cancelled at the source
// because cachedApi coalesces them for other callers; stale ones are just not
// applied.
const trackListGate = createLatestRequestGate();
let trackListToken = trackListGate.begin().token;
// The query whose rows $tracks holds. Its likedOnly decides favorite
// reconciliation, so it changes only when a response is applied.
let appliedTrackListQuery: TrackListQuery | null = null;
// The newest query asked for, set before it resolves so a caller can tell
// whether $tracks already is (or is becoming) the list it wants.
let requestedTrackListQuery: TrackListQuery | null = null;
let pendingTrackListPage: { offset: number; promise: Promise<void> } | null = null;
let failedTrackListPage: TrackListPage | null = null;

export function trackListRequestMatches(sort: string, dir: string, likedOnly: boolean): boolean {
	return sameTrackListQuery(requestedTrackListQuery, { sort, dir, likedOnly });
}

export function loadTracks(
	sort = 'date_added',
	dir = 'desc',
	limit = PAGE_SIZE,
	offset = 0,
	likedOnly = false,
): Promise<void> {
	const page = { sort, dir, likedOnly, limit, offset };
	return offset === 0 ? loadTrackListQuery(page) : loadTrackListPage(page);
}

async function loadTrackListQuery(page: TrackListPage) {
	const { token } = trackListGate.begin();
	trackListToken = token;
	pendingTrackListPage = null;
	failedTrackListPage = null;
	requestedTrackListQuery = { sort: page.sort, dir: page.dir, likedOnly: page.likedOnly };
	trackListErrorState.set(null);
	trackListLoadingMore.set(false);
	trackListLoading.set(true);
	try {
		const data = await fetchTrackListPage(page);
		if (!trackListGate.isCurrent(token)) return;
		appliedTrackListQuery = requestedTrackListQuery;
		tracks.set(data.tracks);
		totalTracks.set(data.total);
	} catch (e) {
		if (!trackListGate.isCurrent(token)) return;
		console.error('Failed to load tracks:', e);
		failedTrackListPage = page;
		trackListErrorState.set({ error: e ?? new Error('Track list request failed'), append: false });
	} finally {
		if (trackListGate.isCurrent(token)) trackListLoading.set(false);
	}
}

function loadTrackListPage(page: TrackListPage): Promise<void> {
	const token = trackListToken;
	// A page belongs to the rows on screen: never while a new query is loading,
	// and never for a sort/scope other than the one those rows came from.
	if (!trackListGate.isCurrent(token) || get(trackListLoading)) return Promise.resolve();
	if (!sameTrackListQuery(appliedTrackListQuery, page)) return Promise.resolve();
	if (pendingTrackListPage) {
		return pendingTrackListPage.offset === page.offset ? pendingTrackListPage.promise : Promise.resolve();
	}
	failedTrackListPage = null;
	trackListErrorState.set(null);
	trackListLoadingMore.set(true);
	const promise = (async () => {
		try {
			const data = await fetchTrackListPage(page);
			if (!trackListGate.isCurrent(token)) return;
			tracks.update((list) => {
				// Optimistic likes shift offsets; a row already shown must not be
				// inserted twice (rows are keyed by id).
				const seen = new Set(list.map((t) => t.id));
				return [...list, ...data.tracks.filter((t) => !seen.has(t.id))];
			});
			totalTracks.set(data.total);
		} catch (e) {
			if (!trackListGate.isCurrent(token)) return;
			console.error('Failed to load more tracks:', e);
			failedTrackListPage = page;
			trackListErrorState.set({ error: e ?? new Error('Track list request failed'), append: true });
		} finally {
			if (trackListGate.isCurrent(token)) {
				pendingTrackListPage = null;
				trackListLoadingMore.set(false);
			}
		}
	})();
	pendingTrackListPage = { offset: page.offset, promise };
	return promise;
}

function fetchTrackListPage(page: TrackListPage) {
	// favoriteOnly stays true so the legacy "library tracks" semantics are unchanged
	// for the Tracks tab; likedOnly takes precedence server-side.
	return cachedApi.getTracks(page.sort, page.dir, page.limit, page.offset, true, page.likedOnly);
}

/** Re-run the track-list request that failed (first page or a later page). */
export function retryTrackList(): Promise<void> {
	const page = failedTrackListPage;
	if (!page) return Promise.resolve();
	return page.offset === 0 ? loadTrackListQuery(page) : loadTrackListPage(page);
}

/** Drop in-flight track-list work, e.g. when the library route unmounts. */
export function cancelTrackListRequests() {
	trackListGate.invalidate();
	trackListToken = trackListGate.begin().token;
	pendingTrackListPage = null;
	failedTrackListPage = null;
	requestedTrackListQuery = appliedTrackListQuery;
	trackListErrorState.set(null);
	trackListLoading.set(false);
	trackListLoadingMore.set(false);
}

export async function loadAlbums(
	sort = 'title',
	dir = 'asc',
	limit = PAGE_SIZE,
	offset = 0,
	decade: number | null = null,
) {
	if (offset === 0) browseLoading.set(true);
	else browseLoadingMore.set(true);
	try {
		const data = await cachedApi.getAlbums(sort, dir, limit, offset, true, decade);
		if (offset === 0) {
			albums.set(data.albums);
		} else {
			albums.update((a) => [...a, ...data.albums]);
		}
		if (data.total !== undefined) totalAlbums.set(data.total);
	} catch (e) {
		console.error('Failed to load albums:', e);
	} finally {
		browseLoading.set(false);
		browseLoadingMore.set(false);
	}
}

export async function loadArtists(sort = 'name', dir = 'asc', limit = PAGE_SIZE, offset = 0) {
	if (offset === 0) browseLoading.set(true);
	else browseLoadingMore.set(true);
	try {
		const data = await cachedApi.getArtists(sort, dir, limit, offset);
		if (offset === 0) {
			artists.set(data.artists);
		} else {
			artists.update((a) => [...a, ...data.artists]);
		}
	} catch (e) {
		console.error('Failed to load artists:', e);
	} finally {
		browseLoading.set(false);
		browseLoadingMore.set(false);
	}
}

export function selectTrackIds(ids: number[], additive = false) {
	trackSelection.select(ids, additive);
}

export function selectAlbumIds(ids: number[], additive = false) {
	albumSelection.select(ids, additive);
}

export function clearSelection() {
	trackSelection.clear();
	albumSelection.clear();
}

export function updateLibraryTrackFavorite(trackId: number, isFavorite: boolean, track?: Track) {
	let removed = false;
	let appended = false;
	tracks.update((list) => {
		const idx = list.findIndex((t) => t.id === trackId);
		if (idx !== -1) {
			if (!isFavorite) {
				if (appliedTrackListQuery?.likedOnly) {
					removed = true;
					return list.filter((t) => t.id !== trackId);
				}
				return list.map((t) => (t.id === trackId ? { ...t, is_favorite: false } : t));
			}
			return list.map((t) => (t.id === trackId ? { ...t, is_favorite: true } : t));
		}
		if (isFavorite && track) {
			appended = true;
			return [{ ...track, is_favorite: true, date_added: new Date().toISOString() }, ...list];
		}
		return list;
	});
	// Keep totalTracks in sync with the optimistic mutation so summaries like
	// "X of Y liked tracks loaded" stay truthful between refetches.
	if (removed) totalTracks.update((n) => Math.max(0, n - 1));
	else if (appended) totalTracks.update((n) => n + 1);
}

