import { writable } from 'svelte/store';
import { type Track, type Album, type Artist } from '$lib/api/client';
import { cachedApi } from '$lib/cache/api_queries';

export const tracks = writable<Track[]>([]);
export const albums = writable<Album[]>([]);
export const artists = writable<Artist[]>([]);
export const totalTracks = writable(0);
export const totalAlbums = writable(0);
export const isLoading = writable(false);
export const isLoadingMore = writable(false);

export const sortBy = writable('date_added');
export const sortDir = writable<'asc' | 'desc'>('desc');
export const viewMode = writable<'grid' | 'list'>('grid');
export const searchQuery = writable('');

// Selected tracks for batch operations
export const selectedTrackIds = writable<Set<number>>(new Set());
export const selectedAlbumIds = writable<Set<number>>(new Set());
export const lastSelectedTrackId = writable<number | null>(null);
export const lastSelectedAlbumId = writable<number | null>(null);

const PAGE_SIZE = 100;

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

export async function loadAlbums(sort = 'title', dir = 'asc', limit = PAGE_SIZE, offset = 0) {
	if (offset === 0) isLoading.set(true);
	else isLoadingMore.set(true);
	try {
		const data = await cachedApi.getAlbums(sort, dir, limit, offset);
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
	selectedTrackIds.update((set) => {
		const next = new Set(additive ? set : []);
		for (const id of ids) {
			if (next.has(id) && additive && ids.length === 1) {
				next.delete(id);
			} else {
				next.add(id);
			}
		}
		return next;
	});

	lastSelectedTrackId.set(ids.at(-1) ?? null);
}

export function selectAlbumIds(ids: number[], additive = false) {
	selectedAlbumIds.update((set) => {
		const next = new Set(additive ? set : []);
		for (const id of ids) {
			if (next.has(id) && additive && ids.length === 1) {
				next.delete(id);
			} else {
				next.add(id);
			}
		}
		return next;
	});

	lastSelectedAlbumId.set(ids.at(-1) ?? null);
}

export function clearSelection() {
	selectedTrackIds.set(new Set());
	selectedAlbumIds.set(new Set());
	lastSelectedTrackId.set(null);
	lastSelectedAlbumId.set(null);
}

export function updateLibraryTrackFavorite(trackId: number, isFavorite: boolean, track?: Track) {
	let removed = false;
	let appended = false;
	tracks.update((list) => {
		const idx = list.findIndex((t) => t.id === trackId);
		if (idx !== -1) {
			if (!isFavorite) {
				removed = true;
				return list.filter((t) => t.id !== trackId);
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

