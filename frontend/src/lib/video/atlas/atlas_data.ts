// One shared copy of the decoded Atlas for the page and the watch page's mini
// map. The page refreshes it on every visit (light changes with every watch),
// but renders the last copy at once so switching tabs never blanks the map.

import { api } from '$lib/api/client';
import { createPersistedStore, oneOf } from '$lib/stores/persisted';
import { decodeAtlas, decodeNames, type AtlasData } from './decode';
import type { AtlasLook } from './style';

export interface LoadedAtlas {
	data: AtlasData;
	names: Map<number, string>;
}

let cached: LoadedAtlas | null = null;
let inflight: Promise<LoadedAtlas> | null = null;
let namesCache: Map<number, string> | null = null;

export function cachedAtlas(): LoadedAtlas | null {
	return cached;
}

async function loadNames(): Promise<Map<number, string>> {
	if (namesCache && namesCache.size > 0) return namesCache;
	try {
		namesCache = decodeNames(await api.getVideoAtlasNames());
	} catch {
		// Names only label the map; a map without them is still a map.
		namesCache = namesCache ?? new Map();
	}
	return namesCache;
}

/** Fetch (or join the fetch of) the atlas. `fresh` skips the cached copy. */
export function loadAtlas(fresh = false): Promise<LoadedAtlas> {
	if (!fresh && cached) return Promise.resolve(cached);
	if (inflight) return inflight;
	inflight = (async () => {
		const [response, names] = await Promise.all([api.getVideoAtlas(), loadNames()]);
		const data = decodeAtlas(response);
		const loaded = { data, names };
		if (data.count > 0) cached = loaded;
		return loaded;
	})().finally(() => {
		inflight = null;
	});
	return inflight;
}

/** Remember a name learned from an artist card (names may be partial). */
export function rememberName(id: number, name: string): void {
	if (!name) return;
	if (!namesCache) namesCache = new Map();
	namesCache.set(id, name);
	cached?.names.set(id, name);
}

export const atlasLook = createPersistedStore<AtlasLook>('videos.atlas.look', 'light', {
	parse: oneOf(['light', 'place'] as const),
});
