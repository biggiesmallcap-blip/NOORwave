type ArtworkItem = {
	artwork_url?: string | null;
	picture_url?: string | null;
	photo_url?: string | null;
};

type ArtworkSource = string | null | undefined | ArtworkItem | ArtworkItem[];

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
