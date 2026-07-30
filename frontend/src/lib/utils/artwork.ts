type ArtworkItem = {
	artwork_url?: string | null;
	picture_url?: string | null;
	photo_url?: string | null;
};

type ArtworkSource = string | null | undefined | ArtworkItem | ArtworkItem[];

export const TIDAL_ARTWORK_SIZES = [80, 160, 320, 640, 750, 1080, 1280] as const;
export type TidalArtworkSize = (typeof TIDAL_ARTWORK_SIZES)[number];

// Last.fm serves this exact image for anything it has no artwork for. It is a
// perfectly valid non-null URL of a grey star, which is worse than null: code
// that falls back "when artwork is missing" never fires, so the tile shows the
// star forever and the TIDAL lookup that would have found real art never runs.
const LASTFM_PLACEHOLDER_HASH = '2a96cbd8b46e442fc41c2b86b821562f';

/**
 * First candidate that is a real image. Treats the Last.fm placeholder as
 * absent so callers can keep using "is it null" to decide whether to resolve
 * artwork from somewhere else.
 */
export function usableArtwork(...candidates: (string | null | undefined)[]): string | null {
	for (const candidate of candidates) {
		if (!candidate) continue;
		const trimmed = candidate.trim();
		if (!trimmed) continue;
		if (trimmed.includes(LASTFM_PLACEHOLDER_HASH)) continue;
		return trimmed;
	}
	return null;
}

export function firstArtworkUrl(...sources: ArtworkSource[]): string | null {
	for (const source of sources) {
		const url = artworkFromSource(source);
		if (url) return url;
	}
	return null;
}

function artworkFromSource(source: ArtworkSource): string | null {
	if (typeof source === 'string') return source.trim() ? source : null;
	if (!source) return null;
	if (Array.isArray(source)) {
		for (const item of source) {
			const url = artworkFromItem(item);
			if (url) return url;
		}
		return null;
	}
	return artworkFromItem(source);
}

function artworkFromItem(item: ArtworkItem): string | null {
	return firstString(item.artwork_url, item.picture_url, item.photo_url);
}

function firstString(...values: (string | null | undefined)[]): string | null {
	for (const value of values) {
		if (typeof value === 'string' && value.trim()) return value;
	}
	return null;
}

// TIDAL bakes the resolution into the artwork path (`.../640x640.jpg`). The
// backend hands us 640px covers, which upscale badly on a phone showing art at
// 2-3x device-pixel density. Swap in a larger size for surfaces that render art
// big; leave non-TIDAL URLs untouched.
const TIDAL_ARTWORK_HOST = 'resources.tidal.com';
const TIDAL_ARTWORK_SIZE = /\/\d+x\d+\.jpg(\?.*)?$/i;

export function normalizeTidalArtworkSize(size: number): TidalArtworkSize {
	for (const allowed of TIDAL_ARTWORK_SIZES) {
		if (size <= allowed) return allowed;
	}
	return 1280;
}

export function isTidalArtworkUrl(url: string | null | undefined): boolean {
	if (!url) return false;
	return isRenderableTidalArtworkUrl(url);
}

export function tidalArtworkFallbackSizes(
	url: string | null | undefined,
	size: TidalArtworkSize = 1280,
): TidalArtworkSize[] {
	const safeSize = normalizeTidalArtworkSize(Number(size));
	const rawUrl = url?.trim() ?? '';
	if (isTidalResourceUrl(rawUrl) && !isRenderableTidalArtworkUrl(rawUrl)) return [];
	if (!isTidalArtworkUrl(rawUrl)) return [safeSize];

	const candidates: TidalArtworkSize[] = [safeSize, 320, 640, 750, 1080, 1280, 160, 80];
	return candidates.filter((candidate, index) => candidates.indexOf(candidate) === index);
}

export function upscaleTidalArtwork(
	url: string | null | undefined,
	size: TidalArtworkSize = 1280,
): string | null {
	const rawUrl = url?.trim() ?? '';
	if (!rawUrl) return null;
	if (isTidalResourceUrl(rawUrl) && !isRenderableTidalArtworkUrl(rawUrl)) return null;
	if (!isTidalArtworkUrl(rawUrl)) return rawUrl;
	const safeSize = normalizeTidalArtworkSize(Number(size));
	return rawUrl.replace(TIDAL_ARTWORK_SIZE, `/${safeSize}x${safeSize}.jpg$1`);
}

function isTidalResourceUrl(url: string | null | undefined): boolean {
	if (!url) return false;
	try {
		return new URL(url).hostname === TIDAL_ARTWORK_HOST;
	} catch {
		return false;
	}
}

function isRenderableTidalArtworkUrl(url: string): boolean {
	try {
		const parsed = new URL(url);
		if (parsed.hostname !== TIDAL_ARTWORK_HOST) return false;
		const parts = parsed.pathname.split('/').filter(Boolean);
		return parts[0] === 'images' && parts.length >= 3 && TIDAL_ARTWORK_SIZE.test(parsed.pathname);
	} catch {
		return false;
	}
}
