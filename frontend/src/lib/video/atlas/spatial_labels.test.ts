import { describe, expect, test } from 'vitest';
import { decodeAtlas, decodeNames } from './decode';
import { artistRankQuota, planLabels, sceneLabelAlpha, sceneLift, zoomLevel, type LabelPlanInput } from './labels';
import { atlasUrl, parseAtlasParams, sceneArchiveUrl, sceneChannelUrl } from './links';
import { SpatialGrid, sceneAt } from './spatial';
import { ATLAS_SLICE, NAMES_SLICE } from './atlas_fixture.test-data';

describe('spatial grid hit testing', () => {
	const xs = new Float32Array([10, 12, 50, 50.5, 200]);
	const ys = new Float32Array([10, 10, 50, 50, 200]);
	const grid = new SpatialGrid(xs, ys, 5, 8);

	test('finds the nearest point within the radius', () => {
		expect(grid.nearest(11.6, 10, 3)).toBe(1);
		expect(grid.nearest(10.2, 10, 3)).toBe(0);
		expect(grid.nearest(100, 100, 5)).toBe(-1);
	});

	test('a point own radius widens its target, and bigger rims win', () => {
		const radius = (i: number) => (i === 4 ? 20 : 0);
		expect(grid.nearest(215, 200, 1, radius, 20)).toBe(4);
		expect(grid.nearest(215, 200, 1)).toBe(-1);
	});

	test('excluded points (negative own radius) never hit', () => {
		expect(grid.nearest(50, 50, 2, (i) => (i === 2 ? -1e9 : 0), 0)).toBe(3);
	});

	test('rectangle queries visit exactly the points inside', () => {
		const seen: number[] = [];
		grid.forEachIn(0, 0, 60, 60, (i) => seen.push(i));
		expect(seen.sort()).toEqual([0, 1, 2, 3]);
	});

	test('scenes hit by disk, nearest centre first', () => {
		const scenes = [
			{ x: 0, y: 0, r: 10 },
			{ x: 12, y: 0, r: 10 },
		];
		expect(sceneAt(scenes, 8, 0)).toBe(1);
		expect(sceneAt(scenes, 3, 0)).toBe(0);
		expect(sceneAt(scenes, 50, 50)).toBe(-1);
	});
});

describe('label level of detail', () => {
	test('quota grows with zoom: scenes only, then hubs, then everyone', () => {
		expect(artistRankQuota(1)).toBe(-1);
		const mid = artistRankQuota(3);
		expect(mid).toBeGreaterThan(0);
		expect(mid).toBeLessThan(50);
		expect(artistRankQuota(6)).toBeGreaterThan(mid);
		expect(artistRankQuota(10)).toBe(Infinity);
		// Phones wait longer before names appear.
		expect(artistRankQuota(2, true)).toBe(-1);
	});

	test('scene labels step back and lift as you zoom in', () => {
		expect(sceneLabelAlpha(1)).toBeGreaterThan(sceneLabelAlpha(10));
		expect(sceneLift(1)).toBe(0);
		expect(sceneLift(6)).toBe(1);
		expect(zoomLevel(4, 2)).toBe(2);
	});

	const data = decodeAtlas(ATLAS_SLICE);
	const names = decodeNames(NAMES_SLICE);
	const grid = new SpatialGrid(data.x, data.y, data.count, 8);
	const input = (scale: number, cx = 500, cy = 500, pinned: number[] = []): LabelPlanInput => ({
		data,
		names,
		grid,
		camera: { cx, cy, scale },
		viewport: { width: 1600, height: 1000 },
		fitScale: 1,
		measureArtist: (_i, text) => text.length * 6,
		measureScene: () => ({ width: 120, height: 20 }),
		pointRadius: () => 2,
		pinned,
		focusScene: -1,
		compact: false,
		artistLabelHeight: 14,
	});

	test('zoomed out: scene names only', () => {
		const labels = planLabels(input(1));
		expect(labels.every((l) => l.kind === 'scene')).toBe(true);
		expect(labels.map((l) => l.text)).toContain('Trap Now');
	});

	test('a pinned artist is named at any zoom', () => {
		const labels = planLabels(input(1, 500, 500, [data.indexById.get(301)!]));
		expect(labels.some((l) => l.kind === 'artist' && l.text === 'NCT 127')).toBe(true);
	});

	test('close in: every named artist in view gets a label', () => {
		const labels = planLabels(input(12, 850, 670));
		const artists = labels.filter((l) => l.kind === 'artist').map((l) => l.text).sort();
		expect(artists).toEqual(['Beyonc\u00e9 Tribute', 'Carlos Rivera', 'Shakira']);
	});

	test('no label is placed under an overlay', () => {
		const open = planLabels(input(1));
		expect(open.length).toBeGreaterThan(0);
		const base = input(1);
		const cover = { x: 0, y: 0, width: base.viewport.width, height: base.viewport.height };
		expect(planLabels({ ...base, obstacles: [cover] })).toEqual([]);
	});

	test('labels never overlap', () => {
		const labels = planLabels(input(12, 850, 670));
		for (const a of labels) {
			for (const b of labels) {
				if (a === b) continue;
				const ax = (a.wx - 850) * 12 + 800 + a.dx;
				const ay = (a.wy - 670) * 12 + 500 + a.dy;
				const bx = (b.wx - 850) * 12 + 800 + b.dx;
				const by = (b.wy - 670) * 12 + 500 + b.dy;
				const overlap = ax < bx + b.width && bx < ax + a.width && ay < by + b.height && by < ay + a.height;
				expect(overlap).toBe(false);
			}
		}
	});
});

describe('atlas links', () => {
	test('round-trips artist, scene and trail', () => {
		const url = atlasUrl({ artist: 42, trail: [7, 8, 42] });
		expect(url).toBe('/videos/atlas?artist=42&trail=7%2C8%2C42');
		const parsed = parseAtlasParams(new URL(`http://x${url}`).searchParams);
		expect(parsed).toEqual({ artist: 42, scene: null, trail: [7, 8, 42] });
		expect(atlasUrl()).toBe('/videos/atlas');
	});

	test('ignores junk and caps the trail', () => {
		const parsed = parseAtlasParams(new URLSearchParams('artist=abc&scene=-1&trail=1,x,2,' + Array.from({ length: 20 }, (_, i) => i + 10).join(',')));
		expect(parsed.artist).toBeNull();
		expect(parsed.scene).toBeNull();
		expect(parsed.trail).toHaveLength(12);
		expect(parsed.trail.at(-1)).toBe(29);
	});

	test('scene hand-offs', () => {
		expect(sceneChannelUrl(4)).toBe('/videos?channel=scene:4');
		expect(sceneArchiveUrl(4)).toBe('/videos/archive?scene=4');
	});
});
