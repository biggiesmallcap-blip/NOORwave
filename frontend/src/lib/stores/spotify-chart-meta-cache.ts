import type { SpotifyPlaylistDetail } from '$lib/api/client';

const STORAGE_KEY = 'noor:spotify-chart-meta:v1';
const TTL_MS = 6 * 60 * 60 * 1000;

export type SpotifyChartMeta = {
	thumbnail: string | null;
	title: string | null;
};

type CacheEntry = SpotifyChartMeta & {
	insertedAt: number;
};

let cache: Record<string, CacheEntry> = {};
let hydrated = false;

function hydrate(): void {
	if (hydrated || typeof localStorage === 'undefined') return;
	hydrated = true;
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return;
		const parsed = JSON.parse(raw);
		if (parsed && typeof parsed === 'object') cache = parsed;
	} catch {
		cache = {};
	}
}

function persist(): void {
	if (typeof localStorage === 'undefined') return;
	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify(cache));
	} catch {
		// Storage can be disabled or full. The in-memory cache still helps.
	}
}

export function getCachedSpotifyChartMeta(id: string): SpotifyChartMeta | null {
	hydrate();
	const entry = cache[id];
	if (!entry) return null;
	if (Date.now() - entry.insertedAt > TTL_MS) {
		delete cache[id];
		persist();
		return null;
	}
	return { thumbnail: entry.thumbnail, title: entry.title };
}

export function getCachedSpotifyChartMetaMap(ids: string[]): Record<string, SpotifyChartMeta> {
	hydrate();
	const out: Record<string, SpotifyChartMeta> = {};
	for (const id of ids) {
		const entry = getCachedSpotifyChartMeta(id);
		if (entry) out[id] = entry;
	}
	return out;
}

export function putCachedSpotifyChartMeta(id: string, playlist: SpotifyPlaylistDetail): void {
	hydrate();
	cache[id] = {
		thumbnail: playlist.thumbnail,
		title: playlist.title,
		insertedAt: Date.now(),
	};
	persist();
}

export function clearSpotifyChartMetaCache(): void {
	cache = {};
	hydrated = true;
	if (typeof localStorage !== 'undefined') {
		try {
			localStorage.removeItem(STORAGE_KEY);
		} catch {
			/* ignore */
		}
	}
}
