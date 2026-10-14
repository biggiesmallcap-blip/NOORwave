// Level of detail for Atlas labels. Scene names always; artist names follow
// the pointer: the scene under it names its artists, densest near the
// pointer, and the scenes beside it only name a few close to the pointer.
// Without a pointer (touch, keyboard, the mouse off the map) zoom decides:
// zoomed out the map reads as scenes, in the middle the hubs of each scene
// get their names, close in every named artist does. Names already on screen
// keep their place over new ones, so moving the pointer never reshuffles
// them. Collisions are resolved by Genre Galaxy's greedy placer
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

/** Where artist names gather while the pointer is over the map. */
export interface Spotlight {
	/** Pointer in CSS px. */
	x: number;
	y: number;
	/** Scene under the pointer, -1 for none. */
	scene: number;
}

/** Spotlight reach in CSS px: the hovered scene's names thin out past
 *  `far`; other scenes only get names within `near` of the pointer. */
export function spotlightRadii(compact: boolean): { near: number; far: number } {
	return compact ? { near: 110, far: 300 } : { near: 150, far: 420 };
}

/** Names a hovered scene shows even zoomed right out, and how deep a scene
 *  beside it reaches near the pointer. */
export const SPOT_SCENE_TOP = 6;
export const SPOT_NEAR_TOP = 2;
/** A name on screen in the last plan wins its spot over a new one, and
 *  reaches this much further from the pointer before it is dropped. */
const STICKY_PRIORITY = 300;
const STICKY_REACH = 1.5;

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
	/** The selected artist: placed before any other pinned name. */
	selected?: number;
	/** Scene drawn as focused (its label wins). -1 for none. */
	focusScene: number;
	compact: boolean;
	artistLabelHeight: number;
	maxArtistCandidates?: number;
	/** Screen boxes covered by overlays (toolbars, legend, the open card).
	 *  No label is placed under them. */
	obstacles?: readonly ScreenBox[];
	/** The pointer and the scene under it; null or absent when there is
	 *  no pointer over the map. */
	spotlight?: Spotlight | null;
	/** Artists named in the last plan. */
	sticky?: ReadonlySet<number>;
	/** A tighter spotlight than the default (thumbnails on screen name
	 *  themselves, so names gather closer to the pointer). */
	reach?: { near: number; far: number };
	/** The deepest rank in scene named without a pointer (thumbnails). */
	quotaCap?: number;
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
			// Above every artist name but a pinned one (artists top out at 800).
			priority: (focus ? 960 : 840) + (maxVideos > 0 ? (scene.videos / maxVideos) * 100 : 0),
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
	const sticky = input.sticky;
	const spot = input.spotlight ?? null;
	const { near, far } = input.reach ?? spotlightRadii(compact);
	const candidates: number[] = [];
	const distance = new Map<number, number>();
	const minX = camera.cx - halfW / camera.scale;
	const maxX = camera.cx + halfW / camera.scale;
	const minY = camera.cy - halfH / camera.scale;
	const maxY = camera.cy + halfH / camera.scale;
	if (spot) {
		// Around the pointer: the hovered (or picked) scene names its
		// artists, densest near the pointer; scenes beside it name a few,
		// close to the pointer only.
		const sceneQuota = Math.max(quota, SPOT_SCENE_TOP);
		const nearQuota = quota < 0 ? SPOT_NEAR_TOP : Math.max(SPOT_NEAR_TOP, Math.floor(quota / 3));
		input.grid.forEachIn(minX, minY, maxX, maxY, (i) => {
			if (pinned.has(i)) return;
			const scene = data.scene[i];
			const rank = data.rankInScene[i];
			const d = Math.hypot(toSx(data.x[i]) - spot.x, toSy(data.y[i]) - spot.y);
			const home = scene >= 0 && (scene === spot.scene || scene === input.focusScene);
			const reach = sticky?.has(i) ? STICKY_REACH : 1;
			if (home) {
				if (rank > sceneQuota || (d > far * reach && rank >= 3)) return;
			} else if (d > near * reach || rank > nearQuota) {
				return;
			}
			if (!names.has(data.ids[i])) return;
			candidates.push(i);
			distance.set(i, home ? d : d + far);
		});
	} else if (quota >= 0) {
		// The cap (thumbnails) applies only here, without a pointer: around
		// the pointer everyone within reach is named.
		const capped = Math.min(quota, input.quotaCap ?? Infinity);
		input.grid.forEachIn(minX, minY, maxX, maxY, (i) => {
			if (pinned.has(i)) return;
			if (data.rankInScene[i] > capped) return;
			// Uncharted dust only gets names close in.
			if (data.scene[i] < 0 && capped !== Infinity && data.rankInScene[i] > capped / 4) return;
			if (!names.has(data.ids[i])) return;
			candidates.push(i);
		});
	}
	const cap = input.maxArtistCandidates ?? (compact ? 160 : 420);
	if (candidates.length > cap) {
		if (spot) candidates.sort((a, b) => (distance.get(a) ?? 0) - (distance.get(b) ?? 0));
		else candidates.sort((a, b) => data.globalRank[a] - data.globalRank[b]);
		candidates.length = cap;
	}
	for (const i of pinned) candidates.push(i);

	const total = Math.max(1, data.count);
	const priorityOf = (i: number): number => {
		if (i === input.selected) return 1100;
		if (pinned.has(i)) return 1000;
		const rankPart = (1 - data.globalRank[i] / total) * (spot ? 150 : 300);
		const stickyPart = sticky?.has(i) ? STICKY_PRIORITY : 0;
		if (!spot) return 100 + rankPart + stickyPart;
		// Closer to the pointer wins; the hovered scene's names before its
		// neighbours' (their distance carries `far` on top).
		const d = distance.get(i) ?? far * 2;
		return 100 + rankPart + stickyPart + Math.max(0, 1 - d / (far * 2)) * 250;
	};
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
			priority: priorityOf(i),
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
			alpha: isPinned ? 1 : 0.95,
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
