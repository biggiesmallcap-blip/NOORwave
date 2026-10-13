// The Atlas renderer: a Canvas 2D night sky of every artist with videos.
//
// Layers, back to front: a deep gradient, two parallax star tiles, the star
// chart graticule and bezel, scene nebulae (one world-space texture built
// from every member's soft stamp, blurred once), the trail and route lines,
// glows (cached sprites per colour), crisp point cores batched into one path
// per (scene, light) bucket, rings and pulses, then labels.
//
// Everything expensive is cached when data, look or size change. The frame
// loop only runs while something moves (a flight, a wheel glide, a fling, a
// pulse, the route drawing in) or after requestDraw(); it allocates nothing.

import type { Light } from '$lib/api/video_explore_types';
import {
	approach,
	clamp,
	constrainCamera,
	copyCamera,
	easeInOutCubic,
	fitCamera,
	fitScale as computeFitScale,
	planFlight,
	screenToWorld,
	zoomAround,
	type Camera,
	type Flight,
	type Point,
	type Viewport,
} from './camera';
import type { AtlasData } from './decode';
import { planLabels, sceneFontSize, zoomLevel, type PlannedLabel } from './labels';
import { SpatialGrid, sceneAt } from './spatial';
import {
	STAGE,
	baseRadius,
	glowColor,
	glowStrength,
	isHub,
	pointColor,
	sceneLabelColor,
	zoomGrowth,
	formatCount,
	formatHours,
	type AtlasLook,
} from './style';

const TAU = Math.PI * 2;
const SCENE_FONT = '"Newsreader", "Iowan Old Style", Georgia, serif';
const BODY_FONT = '"Manrope", "Segoe UI", system-ui, sans-serif';
const MONO_FONT = '"JetBrains Mono", "Cascadia Code", monospace';
const ARTIST_FONT_PX = 11;
const META_FONT_PX = 9.5;
const PULSE_MS = 1200;
const MAX_PULSES = 3;
const DASH_TRAIL = [2, 5];
const DASH_NONE: number[] = [];

export interface HitResult {
	kind: 'artist' | 'scene' | 'none';
	index: number;
}

export interface RendererOptions {
	/** Static mini map: no parallax, no pulses, only pinned names. */
	mini?: boolean;
	/** Called after a frame in which the camera moved. */
	onCamera?: (camera: Camera) => void;
}

// ------------------------------------------------------------- sprite caches

const spriteCache = new Map<string, HTMLCanvasElement>();

function makeCanvas(width: number, height: number): HTMLCanvasElement {
	const canvas = document.createElement('canvas');
	canvas.width = Math.max(1, Math.round(width));
	canvas.height = Math.max(1, Math.round(height));
	return canvas;
}

/** A soft glow: bright centre, long falloff. Drawn with 'lighter'. */
function glowSprite(color: string): HTMLCanvasElement {
	const key = `g:${color}`;
	const hit = spriteCache.get(key);
	if (hit) return hit;
	const size = 64;
	const canvas = makeCanvas(size, size);
	const ctx = canvas.getContext('2d')!;
	const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
	g.addColorStop(0, color);
	g.addColorStop(0.12, color.replace(/, 1\)$/, ', 0.55)'));
	g.addColorStop(0.35, color.replace(/, 1\)$/, ', 0.16)'));
	g.addColorStop(0.7, color.replace(/, 1\)$/, ', 0.04)'));
	g.addColorStop(1, color.replace(/, 1\)$/, ', 0)'));
	ctx.fillStyle = g;
	ctx.fillRect(0, 0, size, size);
	spriteCache.set(key, canvas);
	return canvas;
}

/** A nebula stamp: a wide soft disc in one hue. */
function stampSprite(hue: number, sat: number, light: number): HTMLCanvasElement {
	const key = `s:${hue.toFixed(1)}:${sat}:${light}`;
	const hit = spriteCache.get(key);
	if (hit) return hit;
	const size = 64;
	const canvas = makeCanvas(size, size);
	const ctx = canvas.getContext('2d')!;
	const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
	g.addColorStop(0, `hsla(${hue}, ${sat}%, ${light}%, 1)`);
	g.addColorStop(0.35, `hsla(${hue}, ${sat}%, ${light - 6}%, 0.5)`);
	g.addColorStop(0.7, `hsla(${hue}, ${sat}%, ${light - 12}%, 0.14)`);
	g.addColorStop(1, `hsla(${hue}, ${sat}%, ${light - 12}%, 0)`);
	ctx.fillStyle = g;
	ctx.fillRect(0, 0, size, size);
	spriteCache.set(key, canvas);
	return canvas;
}

function mulberry32(seed: number): () => number {
	let a = seed >>> 0;
	return () => {
		a = (a + 0x6d2b79f5) >>> 0;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

/** A tile of background stars, in device pixels. */
function starTile(seed: number, cssSize: number, dpr: number, count: number, maxAlpha: number): HTMLCanvasElement {
	const canvas = makeCanvas(cssSize * dpr, cssSize * dpr);
	const ctx = canvas.getContext('2d')!;
	ctx.scale(dpr, dpr);
	const rand = mulberry32(seed);
	for (let i = 0; i < count; i++) {
		const x = rand() * cssSize;
		const y = rand() * cssSize;
		const big = rand() > 0.93;
		const r = big ? 0.7 + rand() * 0.6 : 0.3 + rand() * 0.45;
		const a = (big ? 0.5 + rand() * 0.5 : 0.18 + rand() * 0.5) * maxAlpha;
		const tint = rand();
		const hue = tint < 0.33 ? 215 : tint < 0.66 ? 40 : 260;
		ctx.fillStyle = `hsla(${hue}, ${big ? 40 : 18}%, 88%, ${a})`;
		ctx.beginPath();
		ctx.arc(x, y, r, 0, TAU);
		ctx.fill();
		if (big && rand() > 0.5) {
			const g = ctx.createRadialGradient(x, y, 0, x, y, r * 6);
			g.addColorStop(0, `hsla(${hue}, 60%, 85%, ${a * 0.25})`);
			g.addColorStop(1, `hsla(${hue}, 60%, 85%, 0)`);
			ctx.fillStyle = g;
			ctx.fillRect(x - r * 6, y - r * 6, r * 12, r * 12);
		}
	}
	return canvas;
}

interface NebulaTexture {
	canvas: HTMLCanvasElement;
	/** World rectangle the texture covers. */
	x0: number;
	y0: number;
	span: number;
}

const nebulaCache = new WeakMap<AtlasData, Map<AtlasLook, NebulaTexture>>();

/**
 * Scene nebulae as one world-space texture: every charted artist stamps a soft
 * disc in its scene hue, so a nebula takes the real shape of its cluster
 * (never a hard circle); a faint halo per scene and a grey band of dust for
 * the uncharted ring sit underneath; one blur pass melts it together.
 */
function buildNebula(data: AtlasData, look: AtlasLook): NebulaTexture {
	let perLook = nebulaCache.get(data);
	if (!perLook) {
		perLook = new Map();
		nebulaCache.set(data, perLook);
	}
	const cached = perLook.get(look);
	if (cached) return cached;

	const size = 1024;
	const pad = 60;
	const { minX, minY, maxX, maxY } = data.bounds;
	const span = Math.max(maxX - minX, maxY - minY) + pad * 2;
	const x0 = (minX + maxX) / 2 - span / 2;
	const y0 = (minY + maxY) / 2 - span / 2;
	const k = size / span;
	const raw = makeCanvas(size, size);
	const ctx = raw.getContext('2d')!;
	ctx.globalCompositeOperation = 'lighter';

	const place = look === 'place';
	const sat = place ? 80 : 42;
	const lightness = place ? 58 : 50;

	// Halos: the scene's whole extent, very faint.
	for (const scene of data.scenes) {
		const cx = (scene.x - x0) * k;
		const cy = (scene.y - y0) * k;
		const r = scene.r * 1.45 * k;
		const g = ctx.createRadialGradient(cx, cy, 0, cx, cy, r);
		g.addColorStop(0, `hsla(${scene.hue}, ${sat}%, ${lightness}%, ${place ? 0.2 : 0.13})`);
		g.addColorStop(0.55, `hsla(${scene.hue}, ${sat}%, ${lightness - 8}%, ${place ? 0.07 : 0.05})`);
		g.addColorStop(1, `hsla(${scene.hue}, ${sat}%, ${lightness - 10}%, 0)`);
		ctx.fillStyle = g;
		ctx.fillRect(cx - r, cy - r, r * 2, r * 2);
	}

	// Member stamps.
	const stamps = data.scenes.map((scene) => stampSprite(scene.hue, sat, lightness));
	const dust = stampSprite(222, 12, 62);
	for (let i = 0; i < data.count; i++) {
		const s = data.scene[i];
		const v = Math.min(data.videos[i], 300);
		const cx = (data.x[i] - x0) * k;
		const cy = (data.y[i] - y0) * k;
		if (s < 0) {
			const r = 9 * k;
			ctx.globalAlpha = 0.05;
			ctx.drawImage(dust, cx - r, cy - r, r * 2, r * 2);
			continue;
		}
		const r = (7 + 1.5 * Math.sqrt(v)) * k;
		ctx.globalAlpha = (place ? 0.05 : 0.038) + Math.log1p(v) * (place ? 0.012 : 0.009);
		ctx.drawImage(stamps[s], cx - r, cy - r, r * 2, r * 2);
	}
	ctx.globalAlpha = 1;

	const out = makeCanvas(size, size);
	const octx = out.getContext('2d')!;
	if ('filter' in octx) {
		octx.filter = 'blur(7px)';
		octx.drawImage(raw, 0, 0);
		octx.filter = 'none';
		// Keep a little of the unblurred structure for texture.
		octx.globalAlpha = 0.35;
		octx.drawImage(raw, 0, 0);
		octx.globalAlpha = 1;
	} else {
		octx.drawImage(raw, 0, 0);
	}
	const texture = { canvas: out, x0, y0, span };
	perLook.set(look, texture);
	return texture;
}

// ------------------------------------------------------------------ renderer

export class AtlasRenderer {
	readonly canvas: HTMLCanvasElement;
	private readonly ctx: CanvasRenderingContext2D;
	private readonly mini: boolean;
	private readonly onCamera?: (camera: Camera) => void;

	viewport: Viewport = { width: 1, height: 1 };
	private dpr = 1;
	camera: Camera = { cx: 500, cy: 500, scale: 1 };
	fitScale = 1;
	minScale = 0.5;
	maxScale = 60;

	data: AtlasData | null = null;
	names: Map<number, string> = new Map();
	grid: SpatialGrid | null = null;
	look: AtlasLook = 'light';
	reducedMotion = false;
	compact = false;

	// Interaction state (indices into data; -1 for none).
	hover = -1;
	hoverScene = -1;
	selectedArtist = -1;
	selectedScene = -1;
	routeIdx: Int32Array | null = null;
	private routeFlag = new Uint8Array(0);
	trailIdx: Int32Array | null = null;
	/** Pinned names: selection, hover, route stops, trail. */
	private pinned: number[] = [];

	// Per-artist caches.
	private baseR = new Float32Array(0);
	private bucketOrder = new Int32Array(0);
	private bucketStart = new Int32Array(1);
	private bucketScene = new Int16Array(0);
	private bucketColor: string[] = [];
	private glowList = new Int32Array(0);
	private glowSprites: HTMLCanvasElement[] = [];
	private glowAlpha = new Float32Array(0);
	private sparkList = new Int32Array(0);
	private artistWidth = new Float32Array(0);
	private sceneBoxes: { width: number; height: number; size: number; meta: string }[] = [];
	private nebula: NebulaTexture | null = null;

	// Static layers.
	private farStars: HTMLCanvasElement | null = null;
	private nearStars: HTMLCanvasElement | null = null;
	private bgGradient: CanvasGradient | null = null;

	// Labels.
	private labels: PlannedLabel[] = [];
	private labelsAt = -Infinity;
	private labelsStale = true;

	// Motion.
	private raf = 0;
	private lastFrame = 0;
	private dirty = false;
	private flight: { plan: Flight; start: number } | null = null;
	private glide: Camera | null = null;
	private glideAnchor: Point = { x: 0, y: 0 };
	private fling = { vx: 0, vy: 0, active: false };
	private reveal = 1;
	private revealStart = 0;
	private routeProgress = 1;
	private routeStart = 0;
	private pulses = Array.from({ length: MAX_PULSES }, () => ({ index: -1, start: 0 }));
	private pulseTimer: ReturnType<typeof setTimeout> | null = null;
	private scratchCam: Camera = { cx: 0, cy: 0, scale: 1 };
	private scratchPt: Point = { x: 0, y: 0 };
	private lastCam: Camera = { cx: NaN, cy: NaN, scale: NaN };
	private destroyed = false;

	readonly hit: HitResult = { kind: 'none', index: -1 };

	constructor(canvas: HTMLCanvasElement, options: RendererOptions = {}) {
		this.canvas = canvas;
		const ctx = canvas.getContext('2d', { alpha: false });
		if (!ctx) throw new Error('Canvas 2D is not available');
		this.ctx = ctx;
		this.mini = Boolean(options.mini);
		this.onCamera = options.onCamera;
	}

	// ------------------------------------------------------------- setup

	resize(width: number, height: number, dpr: number): void {
		const w = Math.max(1, Math.round(width));
		const h = Math.max(1, Math.round(height));
		const ratio = clamp(dpr || 1, 1, 3);
		const first = this.viewport.width <= 1;
		const prevFit = this.fitScale;
		this.viewport = { width: w, height: h };
		this.dpr = ratio;
		this.canvas.width = Math.round(w * ratio);
		this.canvas.height = Math.round(h * ratio);
		this.compact = w < 640;
		if (this.data) this.rebuildRadii();
		this.bgGradient = null;
		this.farStars = starTile(7, 512, ratio, this.mini ? 60 : 150, 0.8);
		this.nearStars = this.mini ? null : starTile(23, 384, ratio, 34, 1);
		if (this.data) {
			this.updateFit();
			// Keep the same relative zoom when the window changes size.
			if (!first && prevFit > 0) this.camera.scale = clamp((this.camera.scale / prevFit) * this.fitScale, this.minScale, this.maxScale);
			constrainCamera(this.camera, this.data.bounds, this.viewport);
		}
		this.labelsStale = true;
		this.requestDraw();
	}

	setData(data: AtlasData, names: Map<number, string>, opts: { intro?: boolean } = {}): void {
		const firstData = this.data === null;
		this.data = data;
		this.names = names;
		this.grid = new SpatialGrid(data.x, data.y, data.count, 8);
		this.rebuildRadii();
		this.artistWidth = new Float32Array(data.count).fill(-1);
		this.sceneBoxes = [];
		this.buildBuckets();
		this.buildLookCaches();
		this.updateFit();
		if (firstData) {
			copyCamera(fitCamera(data.bounds, this.viewport, this.padding()), this.camera);
			if (opts.intro && !this.reducedMotion) {
				const to = copyCamera(this.camera, { cx: 0, cy: 0, scale: 0 });
				this.camera.scale *= 0.82;
				this.flyTo(to, 0.55);
				this.reveal = 0;
				this.revealStart = performance.now();
			}
		}
		this.selectedArtist = this.selectedArtist < data.count ? this.selectedArtist : -1;
		this.labelsStale = true;
		this.schedulePulse();
		this.requestDraw();
	}

	setNames(names: Map<number, string>): void {
		this.names = names;
		this.labelsStale = true;
		this.requestDraw();
	}

	setLook(look: AtlasLook): void {
		if (look === this.look) return;
		this.look = look;
		if (this.data) {
			this.buildBuckets();
			this.buildLookCaches();
		}
		this.requestDraw();
	}

	setReducedMotion(reduce: boolean): void {
		this.reducedMotion = reduce;
		if (reduce) {
			this.flight = null;
			this.reveal = 1;
			this.routeProgress = 1;
			for (const p of this.pulses) p.index = -1;
			if (this.pulseTimer) clearTimeout(this.pulseTimer);
			this.pulseTimer = null;
		} else {
			this.schedulePulse();
		}
		this.requestDraw();
	}

	/** Fonts arrived: label widths change. */
	fontsChanged(): void {
		this.artistWidth.fill(-1);
		this.sceneBoxes = [];
		this.labelsStale = true;
		this.requestDraw();
	}

	destroy(): void {
		this.destroyed = true;
		if (this.raf) cancelAnimationFrame(this.raf);
		if (this.pulseTimer) clearTimeout(this.pulseTimer);
		this.raf = 0;
	}

	/** Point sizes follow the stage: a phone-sized map gets proportionally smaller stars. */
	private rebuildRadii(): void {
		const data = this.data!;
		const stage = clamp(Math.min(this.viewport.width, this.viewport.height) / 1000, this.mini ? 0.42 : 0.5, 1);
		if (this.baseR.length !== data.count) this.baseR = new Float32Array(data.count);
		for (let i = 0; i < data.count; i++) {
			this.baseR[i] = baseRadius(data.videos[i]) * (data.scene[i] < 0 ? 0.8 : 1) * stage;
		}
	}

	private padding(): number {
		return this.mini ? 4 : this.compact ? 12 : 36;
	}

	private updateFit(): void {
		if (!this.data) return;
		this.fitScale = computeFitScale(this.data.bounds, this.viewport, this.padding());
		this.minScale = this.fitScale * 0.55;
		this.maxScale = this.fitScale * (this.mini ? 40 : 70);
	}

	/** Order artists into (scene, light) buckets so one path fills each colour. */
	private buildBuckets(): void {
		const data = this.data!;
		const sceneCount = data.scenes.length;
		const keyOf = (i: number) => (data.scene[i] < 0 ? 0 : 1 + data.scene[i] * 4 + data.light[i]);
		const buckets = 1 + sceneCount * 4;
		const counts = new Int32Array(buckets + 1);
		for (let i = 0; i < data.count; i++) counts[keyOf(i) + 1] += 1;
		for (let b = 0; b < buckets; b++) counts[b + 1] += counts[b];
		const cursor = counts.slice(0, buckets);
		const order = new Int32Array(data.count);
		// Within a bucket, small first so big points paint on top.
		const bySize = Array.from({ length: data.count }, (_, i) => i).sort((a, b) => data.videos[a] - data.videos[b]);
		for (const i of bySize) order[cursor[keyOf(i)]++] = i;
		this.bucketOrder = order;
		this.bucketStart = counts;
		this.bucketScene = new Int16Array(buckets);
		this.bucketColor = new Array(buckets);
		this.bucketScene[0] = -1;
		this.bucketColor[0] = pointColor(this.look, null, 0);
		for (let s = 0; s < sceneCount; s++) {
			for (let l = 0; l < 4; l++) {
				const b = 1 + s * 4 + l;
				this.bucketScene[b] = s;
				this.bucketColor[b] = pointColor(this.look, data.scenes[s].hue, l as Light);
			}
		}
	}

	private buildLookCaches(): void {
		const data = this.data!;
		const list: number[] = [];
		const sprites: HTMLCanvasElement[] = [];
		const alphas: number[] = [];
		const sparks: number[] = [];
		for (let i = 0; i < data.count; i++) {
			const s = data.scene[i];
			if (s < 0) continue;
			const light = data.light[i] as Light;
			const hub = isHub(data.rankInScene[i], data.videos[i]);
			const strength = glowStrength(this.look, light, hub);
			if (strength <= 0) continue;
			const color = glowColor(this.look, data.scenes[s].hue, light);
			if (!color) continue;
			list.push(i);
			sprites.push(glowSprite(color));
			alphas.push(strength);
			if (this.look === 'light' && light === 3 && (data.rankInScene[i] < 12 || data.videos[i] >= 40)) sparks.push(i);
		}
		// Draw small glows first so big ones sit on top.
		const order = list.map((_, k) => k).sort((a, b) => data.videos[list[a]] - data.videos[list[b]]);
		this.glowList = Int32Array.from(order.map((k) => list[k]));
		this.glowSprites = order.map((k) => sprites[k]);
		this.glowAlpha = Float32Array.from(order.map((k) => alphas[k]));
		this.sparkList = Int32Array.from(sparks);
		this.nebula = buildNebula(data, this.look);
	}

	// ------------------------------------------------------------- state

	setHover(index: number, scene: number): void {
		if (index === this.hover && scene === this.hoverScene) return;
		this.hover = index;
		this.hoverScene = scene;
		this.refreshPinned();
		this.requestDraw();
	}

	setSelection(artist: number, scene: number): void {
		if (artist === this.selectedArtist && scene === this.selectedScene) return;
		this.selectedArtist = artist;
		this.selectedScene = scene;
		this.refreshPinned();
		this.requestDraw();
	}

	setRoute(indices: number[] | null): void {
		this.routeIdx = indices && indices.length > 1 ? Int32Array.from(indices) : null;
		this.routeFlag = new Uint8Array(this.data?.count ?? 0);
		if (this.routeIdx) for (const i of this.routeIdx) this.routeFlag[i] = 1;
		this.routeProgress = this.routeIdx && !this.reducedMotion ? 0 : 1;
		this.routeStart = performance.now();
		this.refreshPinned();
		this.requestDraw();
	}

	setTrail(indices: number[] | null): void {
		this.trailIdx = indices && indices.length > 0 ? Int32Array.from(indices) : null;
		this.refreshPinned();
		this.requestDraw();
	}

	private refreshPinned(): void {
		const pins: number[] = [];
		if (this.selectedArtist >= 0) pins.push(this.selectedArtist);
		if (this.hover >= 0) pins.push(this.hover);
		if (this.routeIdx) for (const i of this.routeIdx) pins.push(i);
		if (this.trailIdx && this.trailIdx.length) pins.push(this.trailIdx[this.trailIdx.length - 1]);
		this.pinned = pins;
		this.labelsStale = true;
	}

	// ------------------------------------------------------------- camera

	zoomLevel(): number {
		return zoomLevel(this.camera.scale, this.fitScale);
	}

	private stopMotion(): void {
		this.flight = null;
		this.glide = null;
		this.fling.active = false;
	}

	panBy(dx: number, dy: number): void {
		if (!this.data) return;
		this.flight = null;
		this.glide = null;
		this.camera.cx -= dx / this.camera.scale;
		this.camera.cy -= dy / this.camera.scale;
		constrainCamera(this.camera, this.data.bounds, this.viewport);
		this.requestDraw();
	}

	/** Zoom by `factor` around screen (sx, sy). Smooth unless reduced motion or `instant`. */
	zoomAt(sx: number, sy: number, factor: number, instant = false): void {
		if (!this.data) return;
		this.flight = null;
		this.fling.active = false;
		if (instant || this.reducedMotion) {
			this.glide = null;
			zoomAround(this.camera, this.viewport, sx, sy, factor, this.minScale, this.maxScale, this.camera);
			constrainCamera(this.camera, this.data.bounds, this.viewport);
			this.requestDraw();
			return;
		}
		// Accumulate into a target; the frame loop glides toward it around the
		// same anchor, so a burst of wheel ticks reads as one smooth zoom.
		const from = this.glide ?? this.camera;
		const target = this.glide ?? { cx: 0, cy: 0, scale: 0 };
		zoomAround(from, this.viewport, sx, sy, factor, this.minScale, this.maxScale, target);
		this.glide = target;
		this.glideAnchor.x = sx;
		this.glideAnchor.y = sy;
		this.requestDraw();
	}

	/** Release velocity from a drag, in CSS px per ms. */
	startFling(vx: number, vy: number): void {
		if (this.reducedMotion || !this.data) return;
		const speed = Math.hypot(vx, vy);
		if (speed < 0.12) return;
		const cap = 3.2;
		this.fling.vx = speed > cap ? (vx / speed) * cap : vx;
		this.fling.vy = speed > cap ? (vy / speed) * cap : vy;
		this.fling.active = true;
		this.requestDraw();
	}

	cancelMotion(): void {
		this.stopMotion();
	}

	flyTo(target: Camera, speed = 1): void {
		if (!this.data) return;
		const to = {
			cx: target.cx,
			cy: target.cy,
			scale: clamp(target.scale, this.minScale, this.maxScale),
		};
		this.glide = null;
		this.fling.active = false;
		if (this.reducedMotion) {
			copyCamera(to, this.camera);
			this.flight = null;
			this.labelsStale = true;
			this.requestDraw();
			return;
		}
		this.flight = { plan: planFlight(copyCamera(this.camera, { cx: 0, cy: 0, scale: 0 }), to, this.viewport, speed), start: performance.now() };
		this.requestDraw();
	}

	fit(): void {
		if (!this.data) return;
		this.flyTo(fitCamera(this.data.bounds, this.viewport, this.padding()));
	}

	/** Fly so artist i sits in view at zoom k (relative to fit), leaving room for a card. */
	flyToArtist(i: number, k = 9, offsetX = 0): void {
		if (!this.data || i < 0 || i >= this.data.count) return;
		const scale = clamp(this.fitScale * Math.max(k, this.zoomLevel()), this.minScale, this.maxScale);
		this.flyTo({ cx: this.data.x[i] + offsetX / scale, cy: this.data.y[i], scale });
	}

	flyToScene(s: number, offsetX = 0): void {
		if (!this.data || s < 0 || s >= this.data.scenes.length) return;
		const scene = this.data.scenes[s];
		const side = Math.min(this.viewport.width - Math.abs(offsetX) * 2, this.viewport.height);
		const scale = clamp((Math.max(160, side) * 0.42) / Math.max(scene.r, 8), this.minScale, this.maxScale);
		this.flyTo({ cx: scene.x + offsetX / scale, cy: scene.y, scale });
	}

	/** Frame a set of artists (a route, a trail). */
	flyToIndices(indices: ArrayLike<number>, offsetX = 0): void {
		if (!this.data || indices.length === 0) return;
		let minX = Infinity;
		let minY = Infinity;
		let maxX = -Infinity;
		let maxY = -Infinity;
		for (let k = 0; k < indices.length; k++) {
			const i = indices[k];
			if (i < 0) continue;
			minX = Math.min(minX, this.data.x[i]);
			minY = Math.min(minY, this.data.y[i]);
			maxX = Math.max(maxX, this.data.x[i]);
			maxY = Math.max(maxY, this.data.y[i]);
		}
		if (!Number.isFinite(minX)) return;
		const pad = 80;
		const vw = Math.max(120, this.viewport.width - Math.abs(offsetX) * 2);
		const scale = clamp(
			Math.min((vw - pad * 2) / Math.max(20, maxX - minX), (this.viewport.height - pad * 2) / Math.max(20, maxY - minY)),
			this.minScale,
			this.fitScale * 14
		);
		this.flyTo({ cx: (minX + maxX) / 2 + offsetX / scale, cy: (minY + maxY) / 2, scale });
	}

	/** Snap without animation (mini map, deep links under reduced motion). */
	setCamera(camera: Camera): void {
		this.stopMotion();
		copyCamera(camera, this.camera);
		this.camera.scale = clamp(this.camera.scale, this.minScale, this.maxScale);
		this.labelsStale = true;
		this.requestDraw();
	}

	// ------------------------------------------------------------- queries

	/** Point radius in CSS px for artist i at the current zoom. */
	pointRadius(i: number): number {
		return this.baseR[i] * zoomGrowth(this.zoomLevel());
	}

	screenOf(i: number, out: Point = this.scratchPt): Point {
		const d = this.data!;
		out.x = (d.x[i] - this.camera.cx) * this.camera.scale + this.viewport.width / 2;
		out.y = (d.y[i] - this.camera.cy) * this.camera.scale + this.viewport.height / 2;
		return out;
	}

	sceneScreen(s: number, out: Point = this.scratchPt): Point {
		const scene = this.data!.scenes[s];
		out.x = (scene.x - this.camera.cx) * this.camera.scale + this.viewport.width / 2;
		out.y = (scene.y - this.camera.cy) * this.camera.scale + this.viewport.height / 2;
		return out;
	}

	/**
	 * What is under screen (sx, sy): an artist point, a scene label, a scene
	 * disk, or nothing. Zoomed out, small points defer to their scene so a
	 * click opens the scene instead of a random dot.
	 */
	hitTest(sx: number, sy: number, slop = 4): HitResult {
		const hit = this.hit;
		hit.kind = 'none';
		hit.index = -1;
		const data = this.data;
		if (!data || !this.grid) return hit;
		// Scene labels are targets of their own.
		for (let n = 0; n < this.labels.length; n++) {
			const label = this.labels[n];
			if (label.kind !== 'scene') continue;
			const lx = (label.wx - this.camera.cx) * this.camera.scale + this.viewport.width / 2 + label.dx;
			const ly = (label.wy - this.camera.cy) * this.camera.scale + this.viewport.height / 2 + label.dy;
			if (sx >= lx - 4 && sx <= lx + label.width + 4 && sy >= ly - 2 && sy <= ly + label.height + 2) {
				hit.kind = 'scene';
				hit.index = label.index;
				return hit;
			}
		}
		const w = screenToWorld(this.camera, this.viewport, sx, sy, this.scratchPt);
		const k = this.zoomLevel();
		const growth = zoomGrowth(k);
		const scale = this.camera.scale;
		const zoomedOut = k < 1.4 && !this.mini;
		const minR = zoomedOut ? 2.6 : 0;
		const radius = (zoomedOut ? 1 : slop) / scale;
		const maxR = baseRadius(600) * growth;
		const baseR = this.baseR;
		const index = this.grid.nearest(
			w.x,
			w.y,
			radius,
			(i) => {
				const r = baseR[i] * growth;
				return r < minR ? -1e9 : r / scale;
			},
			maxR / scale
		);
		if (index >= 0) {
			hit.kind = 'artist';
			hit.index = index;
			return hit;
		}
		const scene = sceneAt(data.scenes, w.x, w.y, 1.08);
		if (scene >= 0) {
			hit.kind = 'scene';
			hit.index = scene;
		}
		return hit;
	}

	// ------------------------------------------------------------- frame loop

	requestDraw(): void {
		this.dirty = true;
		if (this.raf || this.destroyed) return;
		this.raf = requestAnimationFrame(this.tick);
	}

	private tick = (now: number): void => {
		this.raf = 0;
		if (this.destroyed) return;
		const dt = this.lastFrame ? Math.min(64, now - this.lastFrame) : 16;
		this.lastFrame = now;
		let animating = false;
		const data = this.data;

		if (data && this.flight) {
			const t = clamp((now - this.flight.start) / this.flight.plan.duration, 0, 1);
			this.flight.plan.at(easeInOutCubic(t), this.camera);
			if (t >= 1) this.flight = null;
			else animating = true;
		}
		if (data && this.glide) {
			const f = approach(dt, 55);
			const cam = this.camera;
			// Glide the scale; recompute the centre so the anchor stays put.
			const nextScale = cam.scale + (this.glide.scale - cam.scale) * f;
			zoomAround(cam, this.viewport, this.glideAnchor.x, this.glideAnchor.y, nextScale / cam.scale, this.minScale, this.maxScale, cam);
			if (Math.abs(this.glide.scale - cam.scale) / cam.scale < 0.002) {
				copyCamera(this.glide, cam);
				this.glide = null;
			} else animating = true;
			constrainCamera(cam, data.bounds, this.viewport);
		}
		if (data && this.fling.active) {
			this.camera.cx -= (this.fling.vx * dt) / this.camera.scale;
			this.camera.cy -= (this.fling.vy * dt) / this.camera.scale;
			const decay = Math.pow(0.5, dt / 140);
			this.fling.vx *= decay;
			this.fling.vy *= decay;
			constrainCamera(this.camera, data.bounds, this.viewport);
			if (Math.hypot(this.fling.vx, this.fling.vy) < 0.01) this.fling.active = false;
			else animating = true;
		}
		if (this.reveal < 1) {
			this.reveal = clamp((now - this.revealStart) / 900, 0, 1);
			if (this.reveal < 1) animating = true;
		}
		if (this.routeProgress < 1) {
			this.routeProgress = clamp((now - this.routeStart) / 1100, 0, 1);
			if (this.routeProgress < 1) animating = true;
		}
		for (let p = 0; p < MAX_PULSES; p++) {
			const pulse = this.pulses[p];
			if (pulse.index < 0) continue;
			if (now - pulse.start > PULSE_MS) pulse.index = -1;
			else animating = true;
		}

		const moved =
			this.camera.cx !== this.lastCam.cx || this.camera.cy !== this.lastCam.cy || this.camera.scale !== this.lastCam.scale;
		if (moved) {
			copyCamera(this.camera, this.lastCam);
			this.labelsStale = true;
		}
		// Re-plan labels at most ~11 times a second while moving, and once when
		// the camera settles, so motion never pays for placement every frame.
		if (data && this.labelsStale && (!animating || now - this.labelsAt > 90)) {
			this.computeLabels();
			this.labelsAt = now;
			this.labelsStale = false;
		}
		this.draw(now);
		this.dirty = false;
		if (moved && this.onCamera) this.onCamera(this.camera);
		if (animating || this.labelsStale) {
			this.raf = requestAnimationFrame(this.tick);
		} else {
			this.lastFrame = 0;
		}
	};

	private schedulePulse(): void {
		if (this.mini || this.reducedMotion || this.pulseTimer || this.destroyed) return;
		const data = this.data;
		if (!data || data.recent.length === 0) return;
		const delay = 3800 + Math.random() * 4200;
		this.pulseTimer = setTimeout(() => {
			this.pulseTimer = null;
			if (this.destroyed || this.reducedMotion || !this.data || this.data.recent.length === 0) return;
			const slot = this.pulses.find((p) => p.index < 0);
			if (slot && (typeof document === 'undefined' || document.visibilityState !== 'hidden')) {
				slot.index = this.data.recent[Math.floor(Math.random() * this.data.recent.length)];
				slot.start = performance.now();
				this.requestDraw();
			}
			this.schedulePulse();
		}, delay);
	}

	// ------------------------------------------------------------- labels

	/** Scene label box; the meta line (channel, artists, hours) only once you lean in. */
	private sceneBox(s: number, withMeta = this.sceneMetaShown(s)): { width: number; height: number; size: number; meta: string } {
		const slot = s * 2 + (withMeta ? 1 : 0);
		const cached = this.sceneBoxes[slot];
		if (cached) return cached;
		const data = this.data!;
		const scene = data.scenes[s];
		const maxVideos = data.scenes.reduce((m, sc) => Math.max(m, sc.videos), 0);
		const size = sceneFontSize(scene.videos, maxVideos, this.compact);
		const ctx = this.ctx;
		ctx.font = `italic 500 ${size}px ${SCENE_FONT}`;
		const nameW = ctx.measureText(scene.name).width;
		const meta =
			withMeta && !this.compact
				? `${scene.channel_no != null ? `CH ${scene.channel_no}   ` : ''}${formatCount(scene.artists)} ARTISTS   ${formatHours(scene.hours).toUpperCase()}`
				: '';
		ctx.font = `500 ${META_FONT_PX}px ${MONO_FONT}`;
		const metaW = meta ? ctx.measureText(meta).width : 0;
		const box = { width: Math.max(nameW, metaW), height: size * 1.1 + (meta ? META_FONT_PX + 5 : 0), size, meta };
		this.sceneBoxes[slot] = box;
		return box;
	}

	private sceneMetaShown(s: number): boolean {
		return this.zoomLevel() >= 1.45 || s === this.hoverScene || s === this.selectedScene;
	}

	private computeLabels(): void {
		const data = this.data;
		if (!data || !this.grid) return;
		const ctx = this.ctx;
		const growth = zoomGrowth(this.zoomLevel());
		if (this.mini) {
			// Mini map: just the pinned names and the scene of the current artist.
			this.labels = this.miniLabels(growth);
			return;
		}
		this.labels = planLabels({
			data,
			names: this.names,
			grid: this.grid,
			camera: this.camera,
			viewport: this.viewport,
			fitScale: this.fitScale,
			measureArtist: (i, text) => {
				if (this.artistWidth[i] < 0) {
					ctx.font = `600 ${ARTIST_FONT_PX}px ${BODY_FONT}`;
					this.artistWidth[i] = ctx.measureText(text).width;
				}
				return this.artistWidth[i];
			},
			measureScene: (s) => this.sceneBox(s),
			// Route stops wear a ring; their names start outside it.
			pointRadius: (i) => (this.routeFlag[i] ? Math.max(7, this.baseR[i] * growth + 5) : this.baseR[i] * growth),
			pinned: this.pinned,
			focusScene: this.selectedScene,
			compact: this.compact,
			artistLabelHeight: ARTIST_FONT_PX + 3,
		});
	}

	private miniLabels(growth: number): PlannedLabel[] {
		const data = this.data!;
		const out: PlannedLabel[] = [];
		const current = this.trailIdx && this.trailIdx.length ? this.trailIdx[this.trailIdx.length - 1] : this.selectedArtist;
		if (current < 0) return out;
		const name = this.names.get(data.ids[current]);
		if (name) {
			this.ctx.font = `600 ${ARTIST_FONT_PX}px ${BODY_FONT}`;
			const width = this.ctx.measureText(name).width;
			const r = this.baseR[current] * growth;
			const p = this.screenOf(current);
			const right = p.x + r + 4 + width < this.viewport.width - 4;
			out.push({
				kind: 'artist',
				index: current,
				wx: data.x[current],
				wy: data.y[current],
				dx: right ? r + 4 : -r - 4 - width,
				dy: -(ARTIST_FONT_PX + 3) / 2,
				width,
				height: ARTIST_FONT_PX + 3,
				alpha: 1,
				text: name,
			});
		}
		return out;
	}

	// ------------------------------------------------------------- drawing

	private draw(now: number): void {
		const ctx = this.ctx;
		const { width, height } = this.viewport;
		const dpr = this.dpr;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.globalCompositeOperation = 'source-over';
		ctx.globalAlpha = 1;

		// Sky.
		if (!this.bgGradient) {
			const g = ctx.createRadialGradient(width * 0.5, height * 0.42, 0, width * 0.5, height * 0.5, Math.hypot(width, height) * 0.62);
			g.addColorStop(0, STAGE.top);
			g.addColorStop(0.55, STAGE.mid);
			g.addColorStop(1, STAGE.edge);
			this.bgGradient = g;
		}
		ctx.fillStyle = this.bgGradient;
		ctx.fillRect(0, 0, width, height);
		this.drawStars();

		const data = this.data;
		if (!data) return;
		const cam = this.camera;
		const halfW = width / 2;
		const halfH = height / 2;
		const scale = cam.scale;
		const k = scale / this.fitScale;
		const reveal = this.reveal;

		this.drawGraticule(k, reveal);

		// Nebulae.
		const neb = this.nebula;
		if (neb) {
			const zoomFade = clamp(1.12 - Math.log2(Math.max(1, k)) * 0.13, 0.42, 1);
			ctx.globalCompositeOperation = 'lighter';
			ctx.globalAlpha = (this.look === 'place' ? 0.95 : 0.66) * zoomFade * reveal;
			ctx.imageSmoothingEnabled = true;
			ctx.imageSmoothingQuality = 'high';
			const x = (neb.x0 - cam.cx) * scale + halfW;
			const y = (neb.y0 - cam.cy) * scale + halfH;
			ctx.drawImage(neb.canvas, x, y, neb.span * scale, neb.span * scale);
			ctx.globalAlpha = 1;
			ctx.globalCompositeOperation = 'source-over';
		}

		this.drawTrail();
		this.drawRoute(true);

		const growth = zoomGrowth(k);
		const minX = cam.cx - (halfW + 40) / scale;
		const maxX = cam.cx + (halfW + 40) / scale;
		const minY = cam.cy - (halfH + 40) / scale;
		const maxY = cam.cy + (halfH + 40) / scale;
		const focus = this.selectedScene;
		const dimOthers = this.routeIdx !== null ? 0.4 : focus >= 0 ? 0.3 : 1;

		// Glows.
		ctx.globalCompositeOperation = 'lighter';
		for (let n = 0; n < this.glowList.length; n++) {
			const i = this.glowList[n];
			const wx = data.x[i];
			const wy = data.y[i];
			if (wx < minX || wx > maxX || wy < minY || wy > maxY) continue;
			const r = this.baseR[i] * growth;
			const g = r * 5 + 4;
			const sx = (wx - cam.cx) * scale + halfW;
			const sy = (wy - cam.cy) * scale + halfH;
			const dim = focus >= 0 && data.scene[i] !== focus ? dimOthers : this.routeIdx ? dimOthers : 1;
			ctx.globalAlpha = this.glowAlpha[n] * dim * reveal;
			ctx.drawImage(this.glowSprites[n], sx - g, sy - g, g * 2, g * 2);
		}
		ctx.globalAlpha = 1;
		ctx.globalCompositeOperation = 'source-over';

		// Cores, one path per bucket.
		const buckets = this.bucketColor.length;
		for (let b = 0; b < buckets; b++) {
			const start = this.bucketStart[b];
			const end = this.bucketStart[b + 1];
			if (start === end) continue;
			const s = this.bucketScene[b];
			const dim = (focus >= 0 && s !== focus) || this.routeIdx ? dimOthers : 1;
			ctx.globalAlpha = dim * reveal;
			ctx.fillStyle = this.bucketColor[b];
			ctx.beginPath();
			for (let n = start; n < end; n++) {
				const i = this.bucketOrder[n];
				const wx = data.x[i];
				const wy = data.y[i];
				if (wx < minX || wx > maxX || wy < minY || wy > maxY) continue;
				const r = this.baseR[i] * growth;
				const sx = (wx - cam.cx) * scale + halfW;
				const sy = (wy - cam.cy) * scale + halfH;
				if (r < 1.15) {
					ctx.rect(sx - r, sy - r, r * 2, r * 2);
				} else {
					ctx.moveTo(sx + r, sy);
					ctx.arc(sx, sy, r, 0, TAU);
				}
			}
			ctx.fill();
		}
		ctx.globalAlpha = 1;

		// Diffraction spikes on the brightest lit stars.
		if (this.sparkList.length && !this.compact) {
			ctx.globalCompositeOperation = 'lighter';
			ctx.strokeStyle = 'rgba(255, 246, 228, 0.5)';
			ctx.lineWidth = 0.7;
			ctx.globalAlpha = 0.55 * reveal * (this.routeIdx || focus >= 0 ? 0.5 : 1);
			ctx.beginPath();
			for (let n = 0; n < this.sparkList.length; n++) {
				const i = this.sparkList[n];
				const wx = data.x[i];
				const wy = data.y[i];
				if (wx < minX || wx > maxX || wy < minY || wy > maxY) continue;
				const sx = (wx - cam.cx) * scale + halfW;
				const sy = (wy - cam.cy) * scale + halfH;
				const len = this.baseR[i] * growth * 3.2 + 3;
				ctx.moveTo(sx - len, sy);
				ctx.lineTo(sx + len, sy);
				ctx.moveTo(sx, sy - len);
				ctx.lineTo(sx, sy + len);
			}
			ctx.stroke();
			ctx.globalAlpha = 1;
			ctx.globalCompositeOperation = 'source-over';
		}

		this.drawRoute(false);
		this.drawPulses(now, growth);
		this.drawRings(growth);
		this.drawLabels(reveal);
	}

	private drawStars(): void {
		const ctx = this.ctx;
		const { width, height } = this.viewport;
		const layers: [HTMLCanvasElement | null, number, number][] = STAR_LAYERS;
		layers[0][0] = this.farStars;
		layers[0][1] = 0.015;
		layers[0][2] = 512;
		layers[1][0] = this.nearStars;
		layers[1][1] = 0.045;
		layers[1][2] = 384;
		const cam = this.camera;
		for (let l = 0; l < 2; l++) {
			const tile = layers[l][0];
			if (!tile) continue;
			const factor = layers[l][1];
			const size = layers[l][2];
			const ox = -(((cam.cx * cam.scale * factor) % size) + size) % size;
			const oy = -(((cam.cy * cam.scale * factor) % size) + size) % size;
			for (let y = oy; y < height; y += size) {
				for (let x = ox; x < width; x += size) {
					ctx.drawImage(tile, x, y, size, size);
				}
			}
		}
	}

	/** Star chart furniture: faint rings and spokes, a degree bezel, the uncharted ring's name. */
	private drawGraticule(k: number, reveal: number): void {
		const data = this.data!;
		const ctx = this.ctx;
		const cam = this.camera;
		const scale = cam.scale;
		const c = data.size / 2;
		const sx = (c - cam.cx) * scale + this.viewport.width / 2;
		const sy = (c - cam.cy) * scale + this.viewport.height / 2;
		const outer = this.outerRadius();
		ctx.globalAlpha = reveal;
		ctx.lineWidth = 1;
		ctx.strokeStyle = STAGE.grid;
		ctx.beginPath();
		for (let ring = 1; ring <= 3; ring++) {
			const r = outer * (ring / 3.6) * scale;
			ctx.moveTo(sx + r, sy);
			ctx.arc(sx, sy, r, 0, TAU);
		}
		for (let a = 0; a < 12; a++) {
			const ang = (a / 12) * TAU;
			const r0 = outer * 0.1 * scale;
			const r1 = outer * 0.86 * scale;
			ctx.moveTo(sx + Math.cos(ang) * r0, sy + Math.sin(ang) * r0);
			ctx.lineTo(sx + Math.cos(ang) * r1, sy + Math.sin(ang) * r1);
		}
		ctx.stroke();

		// Bezel just outside the uncharted ring.
		const bezel = outer * scale + 34;
		ctx.strokeStyle = STAGE.gridStrong;
		ctx.beginPath();
		ctx.moveTo(sx + bezel, sy);
		ctx.arc(sx, sy, bezel, 0, TAU);
		const step = k > 2.5 ? 1 : 2;
		for (let d = 0; d < 360; d += step) {
			const ang = (d / 180) * Math.PI - Math.PI / 2;
			const len = d % 30 === 0 ? 9 : d % 10 === 0 ? 5 : 2.5;
			const cos = Math.cos(ang);
			const sin = Math.sin(ang);
			ctx.moveTo(sx + cos * bezel, sy + sin * bezel);
			ctx.lineTo(sx + cos * (bezel + len), sy + sin * (bezel + len));
		}
		ctx.stroke();

		if (!this.compact && !this.mini && bezel < Math.hypot(this.viewport.width, this.viewport.height) * 2.2) {
			ctx.fillStyle = STAGE.ringText;
			ctx.font = `500 ${META_FONT_PX}px ${MONO_FONT}`;
			ctx.textAlign = 'center';
			ctx.textBaseline = 'middle';
			for (let d = 0; d < 360; d += 30) {
				const ang = (d / 180) * Math.PI - Math.PI / 2;
				const r = bezel + 19;
				ctx.fillText(DEGREE_LABELS[d / 30], sx + Math.cos(ang) * r, sy + Math.sin(ang) * r);
			}
			// The ring's name, set along its arc at the top.
			if (data.lightCounts[0] > 0) this.drawArcText(UNCHARTED_TEXT, sx, sy, outer * scale + 16, -Math.PI / 2);
			ctx.textAlign = 'start';
			ctx.textBaseline = 'alphabetic';
		}
		ctx.globalAlpha = 1;
	}

	private outerRadius(): number {
		const data = this.data!;
		const c = data.size / 2;
		const b = data.bounds;
		return Math.max(c - b.minX, b.maxX - c, c - b.minY, b.maxY - c);
	}

	private drawArcText(text: string, cx: number, cy: number, radius: number, centerAngle: number): void {
		const ctx = this.ctx;
		ctx.font = `600 ${META_FONT_PX}px ${MONO_FONT}`;
		const spacing = 2.2;
		let total = 0;
		for (let n = 0; n < text.length; n++) total += ctx.measureText(text[n]).width + spacing;
		let angle = centerAngle - total / radius / 2;
		for (let n = 0; n < text.length; n++) {
			const w = ctx.measureText(text[n]).width + spacing;
			const a = angle + w / radius / 2;
			ctx.save();
			ctx.translate(cx + Math.cos(a) * radius, cy + Math.sin(a) * radius);
			ctx.rotate(a + Math.PI / 2);
			ctx.fillText(text[n], 0, 0);
			ctx.restore();
			angle += w / radius;
		}
	}

	private drawTrail(): void {
		const trail = this.trailIdx;
		const data = this.data!;
		if (!trail || trail.length < 2) return;
		const ctx = this.ctx;
		ctx.save();
		ctx.setLineDash(DASH_TRAIL);
		ctx.strokeStyle = STAGE.trail;
		ctx.lineWidth = this.mini ? 1.2 : 1.4;
		ctx.lineCap = 'round';
		ctx.beginPath();
		for (let n = 0; n < trail.length; n++) {
			const p = this.screenOf(trail[n]);
			if (n === 0) ctx.moveTo(p.x, p.y);
			else ctx.lineTo(p.x, p.y);
		}
		ctx.stroke();
		ctx.setLineDash(DASH_NONE);
		ctx.fillStyle = STAGE.trail;
		for (let n = 0; n < trail.length - 1; n++) {
			const p = this.screenOf(trail[n]);
			ctx.beginPath();
			ctx.arc(p.x, p.y, 2, 0, TAU);
			ctx.fill();
		}
		ctx.restore();
		void data;
	}

	/** The route: a glowing line drawn in over ~1 s, then numbered stops. */
	private drawRoute(under: boolean): void {
		const route = this.routeIdx;
		if (!route) return;
		const ctx = this.ctx;
		// Total length in screen px, to draw a partial line while it animates.
		let total = 0;
		for (let n = 1; n < route.length; n++) {
			const a = this.data!;
			total += Math.hypot(a.x[route[n]] - a.x[route[n - 1]], a.y[route[n]] - a.y[route[n - 1]]) * this.camera.scale;
		}
		const visible = total * easeInOutCubic(this.routeProgress);
		if (under) {
			ctx.save();
			ctx.globalCompositeOperation = 'lighter';
			ctx.lineCap = 'round';
			ctx.lineJoin = 'round';
			const passes: [number, number][] = ROUTE_PASSES;
			for (let pass = 0; pass < passes.length; pass++) {
				ctx.lineWidth = passes[pass][0];
				ctx.globalAlpha = passes[pass][1];
				ctx.strokeStyle = STAGE.route;
				this.tracePartial(route, visible);
				ctx.stroke();
			}
			ctx.restore();
			return;
		}
		// Stops on top of the points.
		const data = this.data!;
		let walked = 0;
		ctx.save();
		ctx.font = `600 ${META_FONT_PX}px ${MONO_FONT}`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		for (let n = 0; n < route.length; n++) {
			if (n > 0) walked += Math.hypot(data.x[route[n]] - data.x[route[n - 1]], data.y[route[n]] - data.y[route[n - 1]]) * this.camera.scale;
			if (walked > visible + 0.5) break;
			const p = this.screenOf(route[n]);
			const r = Math.max(7, this.pointRadius(route[n]) + 5);
			ctx.fillStyle = 'rgba(8, 10, 20, 0.86)';
			ctx.strokeStyle = STAGE.route;
			ctx.lineWidth = 1.5;
			ctx.beginPath();
			ctx.arc(p.x, p.y, r, 0, TAU);
			ctx.fill();
			ctx.stroke();
			ctx.fillStyle = STAGE.route;
			ctx.fillText(STOP_NUMBERS[Math.min(n, STOP_NUMBERS.length - 1)], p.x, p.y + 0.5);
		}
		ctx.restore();
	}

	private tracePartial(route: Int32Array, visible: number): void {
		const ctx = this.ctx;
		const data = this.data!;
		ctx.beginPath();
		let walked = 0;
		let p = this.screenOf(route[0]);
		let px = p.x;
		let py = p.y;
		ctx.moveTo(px, py);
		for (let n = 1; n < route.length; n++) {
			p = this.screenOf(route[n]);
			const seg = Math.hypot(p.x - px, p.y - py);
			if (walked + seg >= visible) {
				const t = seg > 0 ? (visible - walked) / seg : 1;
				ctx.lineTo(px + (p.x - px) * t, py + (p.y - py) * t);
				return;
			}
			ctx.lineTo(p.x, p.y);
			walked += seg;
			px = p.x;
			py = p.y;
		}
		void data;
	}

	private drawPulses(now: number, growth: number): void {
		const ctx = this.ctx;
		for (let n = 0; n < MAX_PULSES; n++) {
			const pulse = this.pulses[n];
			if (pulse.index < 0) continue;
			const t = clamp((now - pulse.start) / PULSE_MS, 0, 1);
			const p = this.screenOf(pulse.index);
			const r = this.baseR[pulse.index] * growth + 3 + t * 26;
			ctx.globalAlpha = (1 - t) * (1 - t) * 0.6;
			ctx.strokeStyle = 'rgba(255, 240, 214, 1)';
			ctx.lineWidth = 1.2;
			ctx.beginPath();
			ctx.arc(p.x, p.y, r, 0, TAU);
			ctx.stroke();
		}
		ctx.globalAlpha = 1;
	}

	private drawRings(growth: number): void {
		const ctx = this.ctx;
		const data = this.data!;
		if (this.hoverScene >= 0 && this.hover < 0 && this.selectedScene !== this.hoverScene && !this.mini) {
			// A hovered scene gets a soft lift, never a hard outline.
			const scene = data.scenes[this.hoverScene];
			const p = this.sceneScreen(this.hoverScene);
			const r = scene.r * this.camera.scale * 1.15;
			const g = ctx.createRadialGradient(p.x, p.y, r * 0.2, p.x, p.y, r);
			g.addColorStop(0, `hsla(${scene.hue}, 80%, 70%, 0.07)`);
			g.addColorStop(1, `hsla(${scene.hue}, 80%, 70%, 0)`);
			ctx.globalCompositeOperation = 'lighter';
			ctx.fillStyle = g;
			ctx.fillRect(p.x - r, p.y - r, r * 2, r * 2);
			ctx.globalCompositeOperation = 'source-over';
		}
		if (this.hover >= 0 && this.hover !== this.selectedArtist) {
			const p = this.screenOf(this.hover);
			const r = this.baseR[this.hover] * growth + 3.5;
			ctx.strokeStyle = 'rgba(255, 255, 255, 0.85)';
			ctx.lineWidth = 1.25;
			ctx.beginPath();
			ctx.arc(p.x, p.y, r, 0, TAU);
			ctx.stroke();
		}
		const current = this.mini && this.trailIdx && this.trailIdx.length ? this.trailIdx[this.trailIdx.length - 1] : this.selectedArtist;
		if (current >= 0) {
			const p = this.screenOf(current);
			const r = this.baseR[current] * growth + 5;
			ctx.globalCompositeOperation = 'lighter';
			ctx.strokeStyle = 'rgba(255, 236, 200, 0.28)';
			ctx.lineWidth = 6;
			ctx.beginPath();
			ctx.arc(p.x, p.y, r + 1, 0, TAU);
			ctx.stroke();
			ctx.globalCompositeOperation = 'source-over';
			ctx.strokeStyle = 'rgba(255, 244, 222, 0.98)';
			ctx.lineWidth = 1.6;
			ctx.beginPath();
			ctx.arc(p.x, p.y, r, 0, TAU);
			ctx.stroke();
		}
	}

	private drawLabels(reveal: number): void {
		const ctx = this.ctx;
		const data = this.data!;
		const cam = this.camera;
		const halfW = this.viewport.width / 2;
		const halfH = this.viewport.height / 2;
		ctx.textBaseline = 'top';
		ctx.lineJoin = 'round';
		for (let n = 0; n < this.labels.length; n++) {
			const label = this.labels[n];
			const x = (label.wx - cam.cx) * cam.scale + halfW + label.dx;
			const y = (label.wy - cam.cy) * cam.scale + halfH + label.dy;
			if (label.kind === 'scene') {
				const scene = data.scenes[label.index];
				const box = this.sceneBox(label.index, label.height > this.sceneBox(label.index, false).height + 1);
				const dimmed = this.selectedScene >= 0 && this.selectedScene !== label.index ? 0.45 : 1;
				const alpha = label.alpha * dimmed * reveal;
				ctx.globalAlpha = alpha;
				ctx.font = `italic 500 ${box.size}px ${SCENE_FONT}`;
				ctx.textAlign = 'center';
				const cx = x + box.width / 2;
				ctx.strokeStyle = STAGE.textShadow;
				ctx.lineWidth = this.compact ? 2.5 : 3.5;
				ctx.strokeText(scene.name, cx, y);
				ctx.fillStyle = sceneLabelColor(scene.hue);
				ctx.fillText(scene.name, cx, y);
				if (box.meta) {
					ctx.font = `500 ${META_FONT_PX}px ${MONO_FONT}`;
					ctx.globalAlpha = alpha * 0.8;
					ctx.lineWidth = 2.5;
					const my = y + box.size * 1.1 + 3;
					ctx.strokeText(box.meta, cx, my);
					ctx.fillStyle = STAGE.textDim;
					ctx.fillText(box.meta, cx, my);
				}
				ctx.textAlign = 'start';
			} else {
				const pinnedLabel = label.index === this.selectedArtist || label.index === this.hover;
				ctx.globalAlpha = label.alpha * reveal * (this.selectedScene >= 0 && data.scene[label.index] !== this.selectedScene ? 0.5 : 1);
				ctx.font = `600 ${ARTIST_FONT_PX}px ${BODY_FONT}`;
				ctx.strokeStyle = STAGE.textShadow;
				ctx.lineWidth = this.compact ? 2.5 : 3;
				ctx.strokeText(label.text, x, y + 1);
				ctx.fillStyle = pinnedLabel ? '#ffffff' : STAGE.text;
				ctx.fillText(label.text, x, y + 1);
			}
		}
		ctx.globalAlpha = 1;
		ctx.textBaseline = 'alphabetic';
	}
}

const STAR_LAYERS: [HTMLCanvasElement | null, number, number][] = [
	[null, 0, 512],
	[null, 0, 384],
];
const ROUTE_PASSES: [number, number][] = [
	[12, 0.08],
	[5, 0.22],
	[1.8, 0.95],
];
const DEGREE_LABELS = Array.from({ length: 12 }, (_, i) => String(i * 30).padStart(3, '0'));
const STOP_NUMBERS = Array.from({ length: 12 }, (_, i) => String(i + 1));
const UNCHARTED_TEXT = 'UNCHARTED  -  ARTISTS THE GRAPH HAS NOT LINKED YET';
