import type { TidalSearchVideo, VideoDiscoverSet } from '$lib/api/client';

/** Interleave the browse shelves into one queue with room for linked artists. */
export function buildBrowseMix(sets: VideoDiscoverSet[], limit = 24): TidalSearchVideo[] {
	const daily = sets.find((set) => set.slug === 'daily-picks');
	const linked = sets.find((set) => set.slug === 'one-step-out');
	const ordered = [daily, ...sets.filter((set) => set !== daily && set !== linked)].filter(
		(set): set is VideoDiscoverSet => Boolean(set?.items.length)
	);
	const familiarPool: TidalSearchVideo[] = [];
	const discoveryPool = linked?.items ?? [];
	const depth = Math.max(0, ...ordered.map((set) => set.items.length));
	for (let index = 0; index < depth; index += 1) {
		for (const set of ordered) {
			const item = set.items[index];
			if (item) familiarPool.push(item);
		}
	}

	const result: TidalSearchVideo[] = [];
	const used = new Set<number>();
	const artistCount = new Map<string, number>();
	while (result.length < limit) {
		const lastArtist = result.at(-1)?.artist_name?.toLocaleLowerCase() ?? null;
		const eligible = (item: TidalSearchVideo) => {
			const artist = item.artist_name?.toLocaleLowerCase() ?? '';
			return !used.has(item.tidal_id) && (artistCount.get(artist) ?? 0) < 2;
		};
		const preferDiscovery = result.length % 3 === 2;
		const preferred = preferDiscovery ? discoveryPool : familiarPool;
		const other = preferDiscovery ? familiarPool : discoveryPool;
		const apart = (item: TidalSearchVideo) =>
			eligible(item) && item.artist_name?.toLocaleLowerCase() !== lastArtist;
		const next = preferred.find(apart) ?? other.find(apart) ?? preferred.find(eligible) ?? other.find(eligible);
		if (!next) break;
		result.push(next);
		used.add(next.tidal_id);
		const artist = next.artist_name?.toLocaleLowerCase() ?? '';
		artistCount.set(artist, (artistCount.get(artist) ?? 0) + 1);
	}
	return result;
}
