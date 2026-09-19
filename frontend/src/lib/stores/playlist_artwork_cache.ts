// Local cache for playlist cover mosaics. The artwork URLs themselves are
// served from the browser's HTTP cache; this module just remembers which 4
// URLs to render so the cover paints instantly on every reload.

import { readPersistedJson, writePersisted } from './persisted';

const STORAGE_KEY = 'noor:playlist-mosaic:v1';

export type CachedMosaic = {
	urls: string[];
	track_count: number;
};

let cache: Record<number, CachedMosaic> = {};
let hydrated = false;

function isMosaicCache(value: unknown): value is Record<number, CachedMosaic> {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	return Object.values(value).every((entry) => {
		if (!entry || typeof entry !== 'object') return false;
		const candidate = entry as Partial<CachedMosaic>;
		return Array.isArray(candidate.urls)
			&& candidate.urls.every((url) => typeof url === 'string')
			&& typeof candidate.track_count === 'number';
	});
}

function hydrate(): void {
	if (hydrated) return;
	hydrated = true;
	cache = readPersistedJson(STORAGE_KEY, {}, isMosaicCache);
}

function persist(): void {
	writePersisted(STORAGE_KEY, JSON.stringify(cache));
}

export function getCachedMosaic(id: number, expectedCount: number): string[] | null {
	hydrate();
	const entry = cache[id];
	if (!entry) return null;
	if (entry.track_count !== expectedCount) return null;
	return entry.urls;
}

export function setCachedMosaic(id: number, urls: string[], trackCount: number): void {
	hydrate();
	cache[id] = { urls: urls.slice(0, 4), track_count: trackCount };
	persist();
}

export function snapshotCache(): Record<number, CachedMosaic> {
	hydrate();
	return { ...cache };
}

export function pickArtworkUrls(tracks: Array<{ artwork_url: string | null }>): string[] {
	const seen = new Set<string>();
	const urls: string[] = [];
	for (const t of tracks) {
		const url = t.artwork_url;
		if (!url || seen.has(url)) continue;
		seen.add(url);
		urls.push(url);
		if (urls.length === 4) break;
	}
	return urls;
}

// Stable hash → HSL gradient so cards without artwork still feel distinct.
export function nameToGradient(name: string): string {
	let h = 0;
	for (let i = 0; i < name.length; i += 1) {
		h = (h * 31 + name.charCodeAt(i)) | 0;
	}
	const hue1 = Math.abs(h) % 360;
	const hue2 = (hue1 + 47) % 360;
	return `linear-gradient(145deg, hsl(${hue1} 60% 38%), hsl(${hue2} 55% 22%))`;
}
