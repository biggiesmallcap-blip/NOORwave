// One shared copy of the decoded Atlas for the page and the watch page's mini
// map. The page refreshes it on every visit (light changes with every watch),
// but renders the last copy at once so switching tabs never blanks the map.

import { api } from '$lib/api/client';
import { createPersistedStore } from '$lib/stores/persisted';
import type { AtlasPicturesResponse, AtlasYouResponse } from '$lib/api/video_explore_types';
import { decodeAtlas, decodeNames, type AtlasData } from './decode';
import { parseLook, type AtlasLook } from './style';

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

export const atlasLook = createPersistedStore<AtlasLook>('videos.atlas.look', 'picture', {
	parse: (raw) => parseLook(raw),
});

// The two looks' own data, shared by the page and the mini map. Picture
// fills in while the background read of the artwork goes on, You changes
// with every watch: both are asked again after a minute.
const LOOK_DATA_TTL_MS = 60_000;

function lookLoader<T>(fetch: () => Promise<T>) {
	let value: T | null = null;
	let at = 0;
	let inflight: Promise<T | null> | null = null;
	return {
		cached: (): T | null => value,
		load(fresh = false): Promise<T | null> {
			if (!fresh && value && Date.now() - at < LOOK_DATA_TTL_MS) return Promise.resolve(value);
			if (inflight) return inflight;
			inflight = fetch()
				.then((result) => {
					value = result;
					at = Date.now();
					return result;
				})
				// A server without these endpoints yet: the look draws without them.
				.catch(() => value)
				.finally(() => {
					inflight = null;
				});
			return inflight;
		},
	};
}

export const atlasPictures = lookLoader<AtlasPicturesResponse>(() => api.getVideoAtlasPictures());
export const atlasYou = lookLoader<AtlasYouResponse>(() => api.getVideoAtlasYou());
