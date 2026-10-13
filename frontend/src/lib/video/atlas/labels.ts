// Level of detail for Atlas labels. Zoomed out the map reads as scenes; in
// the middle the hubs of each scene get their names; close in every named
// artist does. Collisions are resolved by Genre Galaxy's greedy placer
// (`placeLabels`), which this module feeds and never changes.

import { placeLabels, type LabelRect } from '$lib/components/Genre/galaxyLabels';
import type { Camera, Viewport } from './camera';
import type { AtlasData } from './decode';
import type { SpatialGrid } from './spatial';

/** Zoom relative to the fitted map: 1 = whole map on screen. */
export function zoomLevel(scale: number, fitScale: number): number {
	return fitScale > 0 ? scale / fitScale : 1;
}

/**
 * How deep into each scene's ranking artist names reach at zoom k: -1 means
 * no artist names, Infinity means every name. Monotonic in k.
 */
export function artistRankQuota(k: number, compact = false): number {
	const start = compact ? 2.2 : 1.7;
	if (k < start) return -1;
	if (k >= (compact ? 12 : 9)) return Infinity;
	return Math.floor(2.2 * Math.pow(k - start + 1, 2));
}

/** Scene names stay strong until you are well inside a scene, then step back. */
export function sceneLabelAlpha(k: number): number {
	if (k <= 4) return 0.96;
	if (k >= 11) return 0.4;
	return 0.96 - ((k - 4) / 7) * 0.56;
}

/** 0 at the fitted map, 1 once the scene label has moved to the disk's top edge. */
export function sceneLift(k: number): number {
	const t = Math.max(0, Math.min(1, (k - 1.6) / 2.2));
	return t * t * (3 - 2 * t);
}

/** Scene label font size in CSS px, by the scene's share of the biggest scene. */
export function sceneFontSize(videos: number, maxVideos: number, compact = false): number {
	const share = maxVideos > 0 ? Math.sqrt(Math.max(0, videos) / maxVideos) : 0;
	// Small caps carry weight on their own; the range stays narrow so big
	// scenes lead without shouting.
	return Math.round((compact ? 8.5 : 9.5) + share * (compact ? 3 : 5));
}

export interface PlannedLabel {
	kind: 'scene' | 'artist';
	/** Scene index or artist index. */
	index: number;
	/** World anchor; the label is drawn at its screen position plus (dx, dy). */
	wx: number;
	wy: number;
	dx: number;
	dy: number;
	width: number;
	height: number;
	alpha: number;
	text: string;
}

export interface LabelPlanInput {
	data: AtlasData;
	names: Map<number, string>;
	grid: SpatialGrid;
	camera: Camera;
	viewport: Viewport;
	fitScale: number;
	/** Text width in CSS px of an artist label. */
	measureArtist: (index: number, text: string) => number;
	/** Box of a scene label (name plus meta line). */
	measureScene: (sceneIndex: number) => { width: number; height: number };
	/** Point radius in CSS px for artist i at the current zoom. */
	pointRadius: (index: number) => number;
	/** Artists that always get a name if it fits (selection, hover, route). */
	pinned: ArrayLike<number>;
	/** Scene drawn as focused (its label wins). -1 for none. */
	focusScene: number;
	compact: boolean;
	artistLabelHeight: number;
	maxArtistCandidates?: number;
	/** Screen boxes covered by overlays (toolbars, legend, the open card).
	 *  No label is placed under them. */
	obstacles?: readonly ScreenBox[];
}

export interface ScreenBox {
	x: number;
	y: number;
	width: number;
	height: number;
}

const OBSTACLE_PRIORITY = 1e9;

const SCENE_ID_BASE = 10_000_000;

/** Breathing room around a scene name, in CSS px, so neighbouring scene
 *  names never sit shoulder to shoulder: a smaller scene's name waits for
 *  the zoom instead of squeezing in. Artist names keep the placer's gap. */
export function sceneLabelPad(compact: boolean): { x: number; y: number } {
	return compact ? { x: 8, y: 4 } : { x: 14, y: 6 };
}

/** Pick and place every label for one camera position. */
export function planLabels(input: LabelPlanInput): PlannedLabel[] {
	const { data, names, camera, viewport, fitScale, compact } = input;
	const k = zoomLevel(camera.scale, fitScale);
	const halfW = viewport.width / 2;
	const halfH = viewport.height / 2;
	const toSx = (wx: number) => (wx - camera.cx) * camera.scale + halfW;
	const toSy = (wy: number) => (wy - camera.cy) * camera.scale + halfH;

	const rects: LabelRect[] = [];
	const meta = new Map<number, PlannedLabel>();

	// Overlays claim their space first; they are accepted by the placer but
	// never drawn (they have no meta). Clamped so the placer accepts them.
	(input.obstacles ?? []).forEach((box, n) => {
		const x = Math.max(0, box.x);
		const y = Math.max(0, box.y);
		const width = Math.min(viewport.width, box.x + box.width) - x;
		const height = Math.min(viewport.height, box.y + box.height) - y;
		if (width <= 0 || height <= 0) return;
		rects.push({ id: -(n + 1), x, y, width, height, priority: OBSTACLE_PRIORITY });
	});

	// Scenes.
	const lift = sceneLift(k);
	const pad = sceneLabelPad(compact);
	const maxVideos = data.scenes.reduce((m, s) => Math.max(m, s.videos), 0);
	const sAlpha = sceneLabelAlpha(k);
	data.scenes.forEach((scene, index) => {
		const box = input.measureScene(index);
		const wx = scene.x;
		const wy = scene.y - scene.r * 0.9 * lift;
		const sx = toSx(wx);
		const sy = toSy(wy);
		// Lifted labels sit above the anchor, centred labels on it.
		const dy = -box.height / 2 - (box.height / 2 + 6) * lift;
		const id = SCENE_ID_BASE + index;
		const focus = index === input.focusScene;
		rects.push({
			id,
			x: sx - box.width / 2 - pad.x,
			y: sy + dy - pad.y,
			width: box.width + pad.x * 2,
			height: box.height + pad.y * 2,
			priority: (focus ? 950 : 500) + (maxVideos > 0 ? (scene.videos / maxVideos) * 100 : 0),
		});
		meta.set(id, {
			kind: 'scene',
			index,
			wx,
			wy,
			dx: -box.width / 2,
			dy,
			width: box.width,
			height: box.height,
			alpha: focus ? 1 : sAlpha,
			text: scene.name,
		});
	});

	// Artists.
	const quota = artistRankQuota(k, compact);
	const pinned = new Set<number>();
	for (let p = 0; p < input.pinned.length; p++) if (input.pinned[p] >= 0) pinned.add(input.pinned[p]);
	const candidates: number[] = [];
	if (quota >= 0) {
		const minX = camera.cx - halfW / camera.scale;
		const maxX = camera.cx + halfW / camera.scale;
		const minY = camera.cy - halfH / camera.scale;
		const maxY = camera.cy + halfH / camera.scale;
		input.grid.forEachIn(minX, minY, maxX, maxY, (i) => {
			if (pinned.has(i)) return;
			if (data.rankInScene[i] > quota) return;
			// Uncharted dust only gets names close in.
			if (data.scene[i] < 0 && quota !== Infinity && data.rankInScene[i] > quota / 4) return;
			if (!names.has(data.ids[i])) return;
			candidates.push(i);
		});
	}
	const cap = input.maxArtistCandidates ?? (compact ? 160 : 420);
	if (candidates.length > cap) {
		candidates.sort((a, b) => data.globalRank[a] - data.globalRank[b]);
		candidates.length = cap;
	}
	for (const i of pinned) candidates.push(i);

	const total = Math.max(1, data.count);
	for (const i of candidates) {
		const text = names.get(data.ids[i]);
		if (!text) continue;
		const width = input.measureArtist(i, text);
		const height = input.artistLabelHeight;
		const r = input.pointRadius(i);
		const sx = toSx(data.x[i]);
		const sy = toSy(data.y[i]);
		const dx = r + 4;
		const dy = -height / 2;
		const isPinned = pinned.has(i);
		rects.push({
			id: i,
			x: sx + dx,
			y: sy + dy,
			width,
			height,
			priority: isPinned ? 1000 : 100 + (1 - data.globalRank[i] / total) * 300,
			altY: sy - r - height - 2,
		});
		meta.set(i, {
			kind: 'artist',
			index: i,
			wx: data.x[i],
			wy: data.y[i],
			dx,
			dy,
			width,
			height,
			alpha: isPinned ? 1 : 0.86,
			text,
		});
	}

	const placed = placeLabels(rects, viewport, compact ? 3 : 4);
	const out: PlannedLabel[] = [];
	for (const rect of rects) {
		if (!placed.has(rect.id)) continue;
		const label = meta.get(rect.id);
		if (!label) continue;
		// placeLabels may have moved the rect to its altY spot. Scene rects
		// carry padding the drawn label does not.
		const anchorSy = toSy(label.wy);
		label.dy = rect.y + (label.kind === 'scene' ? pad.y : 0) - anchorSy;
		out.push(label);
	}
	return out;
}
