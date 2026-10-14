import { describe, expect, test } from 'vitest';
import type { AtlasResponse } from '$lib/api/video_explore_types';
import { decodeAtlas, decodeNames, hasMap } from './decode';
import { buildNameIndex, foldName, searchArtists } from './search';
import { ATLAS_SLICE, NAMES_SLICE } from './atlas_fixture.test-data';

describe('atlas payload decoder', () => {
	test('decodes columns into typed arrays and lookups', () => {
		const data = decodeAtlas(ATLAS_SLICE);
		expect(data.count).toBe(7);
		expect(data.x).toBeInstanceOf(Float32Array);
		expect(data.indexById.get(201)).toBe(3);
		// Scene ids map to scene indices (id 31 is the third scene).
		expect(data.scene[5]).toBe(2);
		expect(data.scene[6]).toBe(-1);
		expect(data.lightCounts).toEqual([1, 2, 2, 2]);
		expect(hasMap(data)).toBe(true);
	});

	test('ranks artists inside their scene, biggest first', () => {
		const data = decodeAtlas(ATLAS_SLICE);
		expect(data.rankInScene[0]).toBe(0); // 120 videos
		expect(data.rankInScene[2]).toBe(1); // 30
		expect(data.rankInScene[1]).toBe(2); // 9
		expect(data.globalRank[3]).toBe(0); // 400, the biggest on the map
		expect(Array.from(data.sceneMembers[0])).toEqual([0, 2, 1]);
	});

	test('keeps only recent ids that are on the map', () => {
		const data = decodeAtlas(ATLAS_SLICE);
		expect(Array.from(data.recent)).toEqual([4]);
	});

	test('survives an inconsistent payload', () => {
		const broken = {
			...ATLAS_SLICE,
			scenes: [...ATLAS_SLICE.scenes, { ...ATLAS_SLICE.scenes[0], id: 77, x: Number.NaN }],
			artists: {
				ids: [1, 2, 2, 3, 4],
				x: [10, Number.NaN, 30, 40, 50],
				y: [10, 20, 30, 40],
				scene: [0, 0, 0, 77, 0],
				videos: [5, 5, 5, 5, 5],
				light: [9, 1, 1, 1, 1] as never,
			},
		} as AtlasResponse;
		const data = decodeAtlas(broken);
		// Cut to the shortest column (4); the NaN row is dropped, so the
		// second row with id 2 is the first valid one and is kept.
		expect(Array.from(data.ids)).toEqual([1, 2, 3]);
		expect(data.x[1]).toBe(30);
		expect(data.light[0]).toBe(3);
		// A scene id with no drawable scene is uncharted.
		expect(data.scene[2]).toBe(-1);
		expect(data.light[2]).toBe(0);
		expect(data.scenes).toHaveLength(3);
	});

	test('an empty building response has no map but keeps the flag', () => {
		const data = decodeAtlas({ built_at: null, building: true, size: 1000, scenes: [], artists: { ids: [], x: [], y: [], scene: [], videos: [], light: [] }, recent_artist_ids: [] });
		expect(data.building).toBe(true);
		expect(hasMap(data)).toBe(false);
		expect(data.bounds).toEqual({ minX: 0, minY: 0, maxX: 1000, maxY: 1000 });
	});

	test('names decode to a map and skip blanks', () => {
		const names = decodeNames({ ids: [1, 2, 3], names: ['A', '  ', 'C'] });
		expect(names.get(1)).toBe('A');
		expect(names.has(2)).toBe(false);
		expect(decodeNames(null).size).toBe(0);
	});
});

describe('atlas artist search', () => {
	const data = decodeAtlas(ATLAS_SLICE);
	const index = buildNameIndex(data, decodeNames(NAMES_SLICE));

	test('folds accents and case', () => {
		expect(foldName('  Beyonc\u00e9   Tribute ')).toBe('beyonce tribute');
		expect(searchArtists(index, data, 'beyonce')[0]?.id).toBe(103);
	});

	test('prefix beats word start beats anywhere', () => {
		const hits = searchArtists(index, data, 'ri');
		// "Rivera" is a word start in "Carlos Rivera"; nothing starts with "ri".
		expect(hits[0].name).toBe('Carlos Rivera');
		expect(searchArtists(index, data, 'future')[0].name).toBe('Future');
		expect(searchArtists(index, data, '')).toEqual([]);
	});
});
