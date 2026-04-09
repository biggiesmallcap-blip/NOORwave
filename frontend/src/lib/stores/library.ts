import { writable } from 'svelte/store';
import { api, type Track, type Album, type Artist } from '$lib/api/client';

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

export async function loadTracks(sort = 'date_added', dir = 'desc', limit = PAGE_SIZE, offset = 0) {
	if (offset === 0) isLoading.set(true);
	else isLoadingMore.set(true);
	try {
		const data = await api.getTracks(sort, dir, limit, offset);
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
		const data = await api.getAlbums(sort, dir, limit, offset);
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
		const data = await api.getArtists(sort, dir, limit, offset);
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

export function formatDuration(ms: number | null): string {
	if (!ms) return '--:--';
	const totalSeconds = Math.floor(ms / 1000);
	const minutes = Math.floor(totalSeconds / 60);
	const seconds = totalSeconds % 60;
	return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}

export function getQualityClass(quality: string | null): string {
	if (!quality) return 'lossy';
	if (quality.includes('HI_RES')) return 'hires';
	if (quality === 'LOSSLESS') return 'lossless';
	return 'lossy';
}
