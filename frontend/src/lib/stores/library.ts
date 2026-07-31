import { writable } from 'svelte/store';
import { type Track, type Album, type Artist } from '$lib/api/client';
import { cachedApi } from '$lib/cache/api_queries';
import { createPersistedStore, oneOf } from './persisted';
import { createSelection } from './selection';

export const tracks = writable<Track[]>([]);
export const albums = writable<Album[]>([]);
export const artists = writable<Artist[]>([]);
export const totalTracks = writable(0);
export const totalAlbums = writable(0);
export const isLoading = writable(false);
export const isLoadingMore = writable(false);

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
let currentTrackListLikedOnly = false;

export async function loadTracks(
	sort = 'date_added',
	dir = 'desc',
	limit = PAGE_SIZE,
	offset = 0,
	likedOnly = false,
) {
	if (offset === 0) isLoading.set(true);
	else isLoadingMore.set(true);
	try {
		// favoriteOnly stays true so the legacy "library tracks" semantics are unchanged
		// for the Tracks tab; likedOnly takes precedence server-side.
		const data = await cachedApi.getTracks(sort, dir, limit, offset, true, likedOnly);
		currentTrackListLikedOnly = likedOnly;
		if (offset === 0) {
			tracks.set(data.tracks);
		} else {
			tracks.update((t) => [...t, ...data.tracks]);
		}
		totalTracks.set(data.total);
	} catch (e) {
		console.error('Failed to load tracks:', e);
	} finally {
		isLoading.set(false);
		isLoadingMore.set(false);
	}
}

export async function loadAlbums(
	sort = 'title',
	dir = 'asc',
	limit = PAGE_SIZE,
	offset = 0,
	decade: number | null = null,
) {
	if (offset === 0) isLoading.set(true);
	else isLoadingMore.set(true);
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
		isLoading.set(false);
		isLoadingMore.set(false);
	}
}

export async function loadArtists(sort = 'name', dir = 'asc', limit = PAGE_SIZE, offset = 0) {
	if (offset === 0) isLoading.set(true);
	else isLoadingMore.set(true);
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
		isLoading.set(false);
		isLoadingMore.set(false);
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
				if (currentTrackListLikedOnly) {
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

