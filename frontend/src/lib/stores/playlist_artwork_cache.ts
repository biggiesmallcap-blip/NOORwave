// Local cache for playlist cover mosaics. The artwork URLs themselves are
// served from the browser's HTTP cache; this module just remembers which 4
// URLs to render so the cover paints instantly on every reload.

const STORAGE_KEY = 'noor:playlist-mosaic:v1';

export type CachedMosaic = {
	urls: string[];
	track_count: number;
};

let cache: Record<number, CachedMosaic> = {};
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
		// Quota / disabled storage — ignore.
	}
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
