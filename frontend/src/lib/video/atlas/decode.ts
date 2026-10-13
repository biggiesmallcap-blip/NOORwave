// Turns the columnar AtlasResponse into typed arrays the renderer can walk
// without allocating, plus the lookups every interaction needs (id -> index,
// scene id -> scene index, per-scene rank for label level of detail).
//
// The payload is trusted for shape but not for consistency: arrays of
// different lengths are cut to the shortest, non-finite positions are dropped,
// unknown scene ids become uncharted and light is clamped to 0..3. A broken
// row never takes the map down.

import type { AtlasNamesResponse, AtlasResponse, AtlasScene, Light } from '$lib/api/video_explore_types';

export interface AtlasBounds {
	minX: number;
	minY: number;
	maxX: number;
	maxY: number;
}

export interface AtlasData {
	/** Artists kept after validation; every typed array has this length. */
	count: number;
	ids: Float64Array;
	x: Float32Array;
	y: Float32Array;
	/** Index into `scenes`, -1 for uncharted. */
	scene: Int16Array;
	videos: Uint32Array;
	light: Uint8Array;
	/** 0 = the artist with the most videos in its scene (uncharted ranks among
	 *  uncharted). Ties break by id so the order is stable across loads. */
	rankInScene: Uint32Array;
	/** 0 = the artist with the most videos on the whole map. */
	globalRank: Uint32Array;
	scenes: AtlasScene[];
	sceneIndexById: Map<number, number>;
	indexById: Map<number, number>;
	/** Artist indices per scene, biggest first. */
	sceneMembers: Int32Array[];
	/** Indices of artists the crawler touched in the last hour. */
	recent: Int32Array;
	lightCounts: [number, number, number, number];
	bounds: AtlasBounds;
	size: number;
	builtAt: string | null;
	building: boolean;
}

function clampLight(value: unknown): Light {
	const n = Number(value);
	if (!Number.isFinite(n)) return 1;
	return Math.max(0, Math.min(3, Math.round(n))) as Light;
}

function finiteScene(scene: AtlasScene): boolean {
	return Number.isFinite(scene.x) && Number.isFinite(scene.y) && Number.isFinite(scene.r);
}

export function decodeAtlas(response: AtlasResponse): AtlasData {
	const size = Number.isFinite(response.size) && response.size > 0 ? response.size : 1000;
	const scenes = (response.scenes ?? []).filter(finiteScene).map((scene) => ({
		...scene,
		hue: Number.isFinite(scene.hue) ? ((scene.hue % 360) + 360) % 360 : 210,
		exemplars: scene.exemplars ?? [],
		tags: scene.tags ?? [],
	}));
	const sceneIndexById = new Map<number, number>();
	scenes.forEach((scene, index) => sceneIndexById.set(scene.id, index));

	const cols = response.artists ?? { ids: [], x: [], y: [], scene: [], videos: [], light: [] };
	const raw = Math.min(
		cols.ids?.length ?? 0,
		cols.x?.length ?? 0,
		cols.y?.length ?? 0,
		cols.scene?.length ?? 0,
		cols.videos?.length ?? 0,
		cols.light?.length ?? 0
	);

	// First pass: which rows survive (finite position, valid id, first seen).
	const keep: number[] = [];
	const seen = new Set<number>();
	for (let i = 0; i < raw; i++) {
		const id = Number(cols.ids[i]);
		if (!Number.isFinite(id) || seen.has(id)) continue;
		if (!Number.isFinite(cols.x[i]) || !Number.isFinite(cols.y[i])) continue;
		seen.add(id);
		keep.push(i);
	}

	const count = keep.length;
	const ids = new Float64Array(count);
	const x = new Float32Array(count);
	const y = new Float32Array(count);
	const scene = new Int16Array(count);
	const videos = new Uint32Array(count);
	const light = new Uint8Array(count);
	const indexById = new Map<number, number>();
	const lightCounts: [number, number, number, number] = [0, 0, 0, 0];
	let minX = Infinity;
	let minY = Infinity;
	let maxX = -Infinity;
	let maxY = -Infinity;

	for (let k = 0; k < count; k++) {
		const i = keep[k];
		ids[k] = Number(cols.ids[i]);
		x[k] = cols.x[i];
		y[k] = cols.y[i];
		const sceneIndex = sceneIndexById.get(Number(cols.scene[i]));
		scene[k] = sceneIndex === undefined ? -1 : sceneIndex;
		const v = Number(cols.videos[i]);
		videos[k] = Number.isFinite(v) && v > 0 ? Math.min(v, 4_000_000_000) : 0;
		// An artist outside every scene has no graph edge: uncharted by definition.
		light[k] = scene[k] < 0 ? 0 : clampLight(cols.light[i]);
		lightCounts[light[k]] += 1;
		indexById.set(ids[k], k);
		if (x[k] < minX) minX = x[k];
		if (y[k] < minY) minY = y[k];
		if (x[k] > maxX) maxX = x[k];
		if (y[k] > maxY) maxY = y[k];
	}
	for (const s of scenes) {
		minX = Math.min(minX, s.x - s.r);
		minY = Math.min(minY, s.y - s.r);
		maxX = Math.max(maxX, s.x + s.r);
		maxY = Math.max(maxY, s.y + s.r);
	}
	if (!Number.isFinite(minX)) {
		minX = 0;
		minY = 0;
		maxX = size;
		maxY = size;
	}

	// Ranks: biggest first, ties by id.
	const order = Array.from({ length: count }, (_, i) => i).sort(
		(a, b) => videos[b] - videos[a] || ids[a] - ids[b]
	);
	const globalRank = new Uint32Array(count);
	const rankInScene = new Uint32Array(count);
	const sceneCounters = new Uint32Array(scenes.length + 1);
	const memberLists: number[][] = scenes.map(() => []);
	order.forEach((index, rank) => {
		globalRank[index] = rank;
		const slot = scene[index] + 1; // uncharted counts in slot 0
		rankInScene[index] = sceneCounters[slot];
		sceneCounters[slot] += 1;
		if (scene[index] >= 0) memberLists[scene[index]].push(index);
	});

	const recentList: number[] = [];
	for (const id of response.recent_artist_ids ?? []) {
		const index = indexById.get(Number(id));
		if (index !== undefined) recentList.push(index);
	}

	return {
		count,
		ids,
		x,
		y,
		scene,
		videos,
		light,
		rankInScene,
		globalRank,
		scenes,
		sceneIndexById,
		indexById,
		sceneMembers: memberLists.map((list) => Int32Array.from(list)),
		recent: Int32Array.from(recentList),
		lightCounts,
		bounds: { minX, minY, maxX, maxY },
		size,
		builtAt: response.built_at ?? null,
		building: Boolean(response.building),
	};
}

/** Names are a separate, cacheable payload; a missing name is not an error. */
export function decodeNames(response: AtlasNamesResponse | null | undefined): Map<number, string> {
	const names = new Map<number, string>();
	if (!response) return names;
	const n = Math.min(response.ids?.length ?? 0, response.names?.length ?? 0);
	for (let i = 0; i < n; i++) {
		const id = Number(response.ids[i]);
		const name = typeof response.names[i] === 'string' ? response.names[i].trim() : '';
		if (Number.isFinite(id) && name) names.set(id, name);
	}
	return names;
}

/** True when the response carries a drawable map. */
export function hasMap(data: AtlasData | null): data is AtlasData {
	return Boolean(data) && (data as AtlasData).count > 0;
}
