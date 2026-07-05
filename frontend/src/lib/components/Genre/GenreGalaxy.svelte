<script lang="ts">
	import { onMount } from 'svelte';
	import { innerWidth } from 'svelte/reactivity/window';
	import { clamp } from '$lib/utils/math';
	import {
		GALAXY_DEFAULT_SCALE,
		type Camera,
		type GalaxyEdge,
		type GalaxyNode,
		type GalaxyViewMode,
		type HeatParticle,
		type ZoomLevel
	} from './galaxy.types';

	type ArtistChipMap = Map<number, string[]>;
	type HoverCardPosition = { x: number; y: number; align: 'left' | 'right' };

	let {
		nodes = [],
		edges = [],
		selectedId = null,
		selectedSeedIds = [],
		focusNodeId = null,
		resetViewToken = 0,
		viewMode = 'map',
		labelsEnabled = true,
		autoDrift = false,
		artistChipMap = new Map<number, string[]>(),
		searchHighlightIds = new Set<number>(),
		onSelect = () => {},
		onToggleSeed = () => {},
		onMix = () => {},
		onZoomFamily = () => {},
		onEnterInterior = () => {}
	}: {
		nodes?: GalaxyNode[];
		edges?: GalaxyEdge[];
		selectedId?: number | null;
		selectedSeedIds?: number[];
		focusNodeId?: number | null;
		resetViewToken?: number;
		viewMode?: GalaxyViewMode;
		labelsEnabled?: boolean;
		autoDrift?: boolean;
		artistChipMap?: ArtistChipMap;
		searchHighlightIds?: Set<number>;
		onSelect?: (id: number | null) => void;
		onToggleSeed?: (id: number) => void;
		onMix?: (id: number) => void;
		onZoomFamily?: (familyId: number) => void;
		onEnterInterior?: (id: number) => void;
	} = $props();

	let wrapEl: HTMLDivElement | null = null;
	let canvasEl: HTMLCanvasElement | null = null;
	let width = $state(0);
	let height = $state(0);
	let dpr = $state(1);
	let hoveredNodeId = $state<number | null>(null);
	// hoverCardId trails hoveredNodeId by ~280ms so the hover card only appears
	// when the pointer parks on a node, not while sweeping across.
	let hoverCardId = $state<number | null>(null);
	let hoverCardTimer: ReturnType<typeof setTimeout> | null = null;
	const HOVER_INTENT_MS = 280;
	let activeFamilyId = $state<number | null>(null);
	let zoomLevel = $state<ZoomLevel>('galaxy');
	let isDragging = $state(false);
	let mixPillPosition = $state<{ x: number; y: number } | null>(null);
	let hoverCardPosition = $state<HoverCardPosition | null>(null);
	let camera = $state<Camera>({
		x: 0,
		y: 0,
		scale: GALAXY_DEFAULT_SCALE,
		targetX: 0,
		targetY: 0,
		targetScale: GALAXY_DEFAULT_SCALE
	});
	let particles = $state<HeatParticle[]>([]);

	let nodeById = $derived(new Map(nodes.map((node) => [node.id, node])));
	let isCompactViewport = $derived((innerWidth.current ?? 1200) <= 760);
	let hoveredNode = $derived(
		hoveredNodeId === null ? null : nodeById.get(hoveredNodeId) ?? null
	);
	let hoverCardNode = $derived(
		hoverCardId === null ? null : nodeById.get(hoverCardId) ?? null
	);

	// Schedule the hover card to appear after HOVER_INTENT_MS of stable hover.
	// Hide immediately on leave / drag.
	$effect(() => {
		const target = hoveredNodeId;
		if (target === null || isDragging) {
			if (hoverCardTimer) {
				clearTimeout(hoverCardTimer);
				hoverCardTimer = null;
			}
			hoverCardId = null;
			return;
		}
		if (target === hoverCardId) return;
		if (hoverCardTimer) clearTimeout(hoverCardTimer);
		hoverCardTimer = setTimeout(() => {
			hoverCardId = target;
			hoverCardTimer = null;
		}, HOVER_INTENT_MS);
	});

	// Vibe mode: energy color mapping
	function energyColor(energy: number | null): string {
		if (energy == null) return '';
		// Blue (220°) → Amber (45°) → Red (0°)
		const hue = 220 - energy * 220;
		return `hsl(${hue}, 70%, 50%)`;
	}

	// Vibe mode: danceability glow
	function danceGlowRadius(danceability: number | null, baseRadius: number): number {
		if (danceability == null) return baseRadius + 6;
		return baseRadius + 6 + danceability * 14;
	}
	let selectedSeedSet = $derived(new Set(selectedSeedIds));
	let selectedLineageSet = $derived.by(() => {
		const lineage = new Set<number>();
		if (selectedId === null) return lineage;
		let current = nodeById.get(selectedId) ?? null;
		while (current) {
			lineage.add(current.id);
			current = current.parentId === null ? null : nodeById.get(current.parentId) ?? null;
		}
		return lineage;
	});

	let bgCanvas: HTMLCanvasElement | null = null;
	let connCanvas: HTMLCanvasElement | null = null;
	let resizeObserver: ResizeObserver | null = null;
	let animationFrame = 0;
	let activePointerId: number | null = null;
	let dragStart:
		| {
				x: number;
				y: number;
				cameraX: number;
				cameraY: number;
		  }
		| null = null;
	let lastLayoutSignature = '';
	let lastCameraSnapshot = '';
	let lastFocusedExternalId: number | null = null;
	let lastResetViewToken = 0;
	let pendingConnectionRedraw = true;

	const MAX_PARTICLES = 120;
	const HOVER_CARD_CURSOR_CLEARANCE_X = 28;
	const HOVER_CARD_CURSOR_CLEARANCE_Y = 24;
	const HOVER_CARD_EDGE_MARGIN = 12;
	const HOVER_CARD_ESTIMATED_WIDTH = 260;
	const fontBody = '600 12px "Avenir Next", "Segoe UI", sans-serif';
	const fontDisplay = '600 13px "Iowan Old Style", Georgia, serif';

	function hexToRgba(hex: string, alpha: number): string {
		const normalized = hex.replace('#', '');
		if (normalized.length !== 6) {
			return `rgba(255, 255, 255, ${alpha})`;
		}
		const red = Number.parseInt(normalized.slice(0, 2), 16);
		const green = Number.parseInt(normalized.slice(2, 4), 16);
		const blue = Number.parseInt(normalized.slice(4, 6), 16);
		return `rgba(${red}, ${green}, ${blue}, ${alpha})`;
	}

	// Push a hex color toward black (amount < 0) or white (amount > 0) and emit
	// rgba. Used for node rims so they fall off to a darker shade of the node's
	// own hue instead of muddy navy.
	function shadeRgba(hex: string, amount: number, alpha: number): string {
		const normalized = hex.replace('#', '');
		if (normalized.length !== 6) return `rgba(255, 255, 255, ${alpha})`;
		let red = Number.parseInt(normalized.slice(0, 2), 16);
		let green = Number.parseInt(normalized.slice(2, 4), 16);
		let blue = Number.parseInt(normalized.slice(4, 6), 16);
		const target = amount < 0 ? 0 : 255;
		const p = Math.min(1, Math.abs(amount));
		red = Math.round((target - red) * p + red);
		green = Math.round((target - green) * p + green);
		blue = Math.round((target - blue) * p + blue);
		return `rgba(${red}, ${green}, ${blue}, ${alpha})`;
	}

	// --- Sprite cache --------------------------------------------------------
	// Building radial gradients per node per frame is what made the canvas both
	// fuzzy and slow. Node radius + color are stable, so each unique body and
	// glow is rendered ONCE to an offscreen sprite and blitted with drawImage.
	const BODY_GLOW_FACTOR = 1.4;
	const nodeSpriteCache = new Map<string, HTMLCanvasElement>();
	const glowSpriteCache = new Map<string, HTMLCanvasElement>();
	const edgeColorCache = new Map<number, string>();
	let colorScratch: CanvasRenderingContext2D | null = null;
	let spriteDpr = 0;

	function normalizeColor(color: string): string {
		// Canvas readback normalizes any CSS color (hsl strings from vibe mode,
		// hex, named) to #rrggbb, which shadeRgba/hexToRgba can parse.
		if (color.startsWith('#') && color.length === 7) return color;
		if (!colorScratch) {
			colorScratch = document.createElement('canvas').getContext('2d');
			if (!colorScratch) return '#ffffff';
		}
		colorScratch.fillStyle = color;
		return String(colorScratch.fillStyle);
	}

	function invalidateSprites() {
		nodeSpriteCache.clear();
		glowSpriteCache.clear();
	}

	function getNodeSprite(color: string, radius: number, jitter: number): HTMLCanvasElement | null {
		const hex = normalizeColor(color);
		const key = `${hex}|${Math.round(radius * 2)}|${Math.round(jitter * 20)}`;
		const cached = nodeSpriteCache.get(key);
		if (cached) return cached;

		const half = radius * BODY_GLOW_FACTOR;
		const size = Math.max(4, Math.ceil(half * 2 * spriteDpr));
		const sprite = document.createElement('canvas');
		sprite.width = size;
		sprite.height = size;
		const sctx = sprite.getContext('2d');
		if (!sctx) return null;
		sctx.scale(size / (half * 2), size / (half * 2));

		// Tight ambient glow hugging the body - subtle, not a haze.
		const glow = sctx.createRadialGradient(half, half, radius * 0.82, half, half, half);
		glow.addColorStop(0, hexToRgba(hex, 0.22));
		glow.addColorStop(1, hexToRgba(hex, 0));
		sctx.fillStyle = glow;
		sctx.beginPath();
		sctx.arc(half, half, half, 0, Math.PI * 2);
		sctx.fill();

		// Crisp solid body, gently lit toward the upper-left. Opaque to the edge
		// so the disc stays sharp - no feathering, no pearl, no dark rim.
		const body = sctx.createRadialGradient(
			half - radius * 0.2,
			half - radius * 0.24,
			radius * 0.1,
			half,
			half,
			radius
		);
		body.addColorStop(0, shadeRgba(hex, 0.24 + jitter, 1));
		body.addColorStop(0.62, hex);
		body.addColorStop(1, shadeRgba(hex, -0.14, 1));
		sctx.fillStyle = body;
		sctx.beginPath();
		sctx.arc(half, half, radius, 0, Math.PI * 2);
		sctx.fill();

		// Hairline lit rim so the edge reads crisp against the glow.
		const rimWidth = Math.max(0.75, radius * 0.045);
		sctx.lineWidth = rimWidth;
		sctx.strokeStyle = shadeRgba(hex, 0.38, 0.38);
		sctx.beginPath();
		sctx.arc(half, half, radius - rimWidth / 2, 0, Math.PI * 2);
		sctx.stroke();

		nodeSpriteCache.set(key, sprite);
		return sprite;
	}

	function getGlowSprite(color: string): HTMLCanvasElement | null {
		const hex = normalizeColor(color);
		const cached = glowSpriteCache.get(hex);
		if (cached) return cached;
		const size = 64;
		const sprite = document.createElement('canvas');
		sprite.width = size;
		sprite.height = size;
		const sctx = sprite.getContext('2d');
		if (!sctx) return null;
		const gradient = sctx.createRadialGradient(32, 32, 4, 32, 32, 32);
		gradient.addColorStop(0, hexToRgba(hex, 0.5));
		gradient.addColorStop(0.5, hexToRgba(hex, 0.14));
		gradient.addColorStop(1, hexToRgba(hex, 0));
		sctx.fillStyle = gradient;
		sctx.fillRect(0, 0, size, size);
		glowSpriteCache.set(hex, sprite);
		return sprite;
	}

	// --- Parallax starfield ---------------------------------------------------
	// The bg canvas is screen-fixed, so on its own the sky reads as a flat
	// poster. These two star layers live in (scaled) world space and shift with
	// the camera at different rates - pan, zoom, or drift and the depth shows.
	// ~185 stars tiled, trivial per-frame cost.
	type ParallaxStar = { x: number; y: number; size: number; alpha: number; tint: string; phase: number };
	const STAR_TILE = 1024;

	function makeStarLayer(
		count: number,
		seedStart: number,
		sizeMin: number,
		sizeVar: number,
		alphaMin: number,
		alphaVar: number
	): ParallaxStar[] {
		let seed = seedStart;
		const rnd = () => {
			seed = (seed * 1664525 + 1013904223) >>> 0;
			return seed / 4294967296;
		};
		const stars: ParallaxStar[] = [];
		for (let index = 0; index < count; index += 1) {
			const warmth = rnd();
			stars.push({
				x: rnd() * STAR_TILE,
				y: rnd() * STAR_TILE,
				size: sizeMin + rnd() * sizeVar,
				alpha: alphaMin + rnd() * alphaVar,
				tint:
					warmth > 0.82
						? 'rgb(196, 208, 255)'
						: warmth < 0.15
							? 'rgb(255, 224, 196)'
							: 'rgb(255, 255, 255)',
				phase: rnd() * Math.PI * 2
			});
		}
		return stars;
	}

	const starLayerFar = makeStarLayer(130, 7, 0.5, 0.7, 0.3, 0.34);
	const starLayerNear = makeStarLayer(55, 1234567, 0.9, 1.0, 0.4, 0.4);

	function drawParallaxStars(ctx: CanvasRenderingContext2D) {
		const now = performance.now();
		const layers = [
			{ stars: starLayerFar, factor: 0.16, twinkle: false },
			{ stars: starLayerNear, factor: 0.38, twinkle: true }
		];
		for (const layer of layers) {
			const offsetX = camera.x * camera.scale * layer.factor;
			const offsetY = camera.y * camera.scale * layer.factor;
			for (const star of layer.stars) {
				let sx = (star.x - offsetX) % STAR_TILE;
				let sy = (star.y - offsetY) % STAR_TILE;
				if (sx < 0) sx += STAR_TILE;
				if (sy < 0) sy += STAR_TILE;
				const alpha = layer.twinkle
					? star.alpha * (0.68 + 0.32 * Math.sin(now / 850 + star.phase))
					: star.alpha;
				ctx.fillStyle = star.tint;
				// Tile so the field is endless in every direction.
				for (let tx = sx - STAR_TILE; tx < width + 4; tx += STAR_TILE) {
					if (tx < -4) continue;
					for (let ty = sy - STAR_TILE; ty < height + 4; ty += STAR_TILE) {
						if (ty < -4) continue;
						ctx.globalAlpha = alpha;
						ctx.beginPath();
						ctx.arc(tx, ty, star.size, 0, Math.PI * 2);
						ctx.fill();
					}
				}
			}
		}
		ctx.globalAlpha = 1;
	}

	function edgeStrokeColor(edge: GalaxyEdge, source: GalaxyNode, target: GalaxyNode): string {
		const key = edge.sourceId * 1000000 + edge.targetId;
		const cached = edgeColorCache.get(key);
		if (cached) return cached;
		const a = normalizeColor(source.color).replace('#', '');
		const b = normalizeColor(target.color).replace('#', '');
		let mixed = 'rgb(136, 153, 204)';
		if (a.length === 6 && b.length === 6) {
			const red = Math.round((Number.parseInt(a.slice(0, 2), 16) + Number.parseInt(b.slice(0, 2), 16)) / 2);
			const green = Math.round((Number.parseInt(a.slice(2, 4), 16) + Number.parseInt(b.slice(2, 4), 16)) / 2);
			const blue = Math.round((Number.parseInt(a.slice(4, 6), 16) + Number.parseInt(b.slice(4, 6), 16)) / 2);
			mixed = `rgb(${red}, ${green}, ${blue})`;
		}
		edgeColorCache.set(key, mixed);
		return mixed;
	}

	function roundedRectPath(
		ctx: CanvasRenderingContext2D,
		x: number,
		y: number,
		width: number,
		height: number,
		radius: number
	) {
		const nextRadius = Math.min(radius, width / 2, height / 2);
		ctx.beginPath();
		ctx.moveTo(x + nextRadius, y);
		ctx.arcTo(x + width, y, x + width, y + height, nextRadius);
		ctx.arcTo(x + width, y + height, x, y + height, nextRadius);
		ctx.arcTo(x, y + height, x, y, nextRadius);
		ctx.arcTo(x, y, x + width, y, nextRadius);
		ctx.closePath();
	}

	function worldToScreen(x: number, y: number) {
		return {
			x: (x - camera.x) * camera.scale + width / 2,
			y: (y - camera.y) * camera.scale + height / 2
		};
	}

	function screenToWorld(x: number, y: number) {
		return {
			x: (x - width / 2) / camera.scale + camera.x,
			y: (y - height / 2) / camera.scale + camera.y
		};
	}

	function nodeIsVisible(node: GalaxyNode): boolean {
		const screen = worldToScreen(node.x, node.y);
		const margin = Math.max(60, node.radius * 2);
		return (
			screen.x >= -margin &&
			screen.x <= width + margin &&
			screen.y >= -margin &&
			screen.y <= height + margin
		);
	}

	function selectedLineageHas(nodeId: number): boolean {
		return selectedLineageSet.has(nodeId);
	}

	function nodeIsSeed(nodeId: number): boolean {
		return selectedSeedSet.has(nodeId);
	}

	function isRediscoverCandidate(node: GalaxyNode): boolean {
		return node.trackCount > 0 && node.listenCount === 0;
	}

	function nodeActivity(node: GalaxyNode): number {
		let activity: number;
		if (selectedId !== null) {
			const selectedNode = nodeById.get(selectedId);
			if (!selectedNode) {
				activity = 0.6;
			} else if (node.id === selectedNode.id) {
				activity = 1;
			} else if (selectedLineageHas(node.id)) {
				activity = 0.88;
			} else if (node.familyId === selectedNode.familyId) {
				activity = 0.7;
			} else {
				activity = 0.28;
			}
		} else if (activeFamilyId !== null) {
			activity = node.familyId === activeFamilyId ? 0.94 : 0.44;
		} else {
			activity = 0.86;
		}

		// Inline search dim: nodes outside the match lineage fade to make matches pop.
		if (searchHighlightIds.size > 0 && !searchHighlightIds.has(node.id)) {
			activity *= 0.22;
		}

		// Rediscover mode: candidates pop, well-played and empty nodes fade.
		if (viewMode === 'rediscover') {
			if (isRediscoverCandidate(node)) {
				activity = Math.max(activity, 0.95);
			} else if (node.depth > 0) {
				activity *= 0.32;
			}
		}

		return activity;
	}

	function edgeActivity(edge: GalaxyEdge): number {
		const source = nodeById.get(edge.sourceId);
		const target = nodeById.get(edge.targetId);
		if (!source || !target) return 0;

		let activity = Math.min(nodeActivity(source), nodeActivity(target));
		if (selectedId !== null) {
			if (edge.sourceId === selectedId || edge.targetId === selectedId) {
				activity *= 1.3;
			}
		}

		return clamp(activity, 0.16, 1.2);
	}

	function getNodeAtPoint(clientX: number, clientY: number): GalaxyNode | null {
		let best: GalaxyNode | null = null;
		let bestScore = Number.POSITIVE_INFINITY;

		for (const node of nodes) {
			if (!nodeIsVisible(node)) continue;
			const screen = worldToScreen(node.x, node.y);
			const dx = clientX - screen.x;
			const dy = clientY - screen.y;
			const hitRadius = node.radius + (node.depth === 0 ? 12 : 7);
			const distance = Math.hypot(dx, dy);
			if (distance > hitRadius) continue;
			const score = distance / hitRadius - node.depth * 0.03;
			if (score < bestScore) {
				best = node;
				bestScore = score;
			}
		}

		return best;
	}

	function fitToNodes(targetNodes: GalaxyNode[], paddingFactor = 0.9, initial = false) {
		if (targetNodes.length === 0 || width === 0 || height === 0) return;

		let minX = Number.POSITIVE_INFINITY;
		let minY = Number.POSITIVE_INFINITY;
		let maxX = Number.NEGATIVE_INFINITY;
		let maxY = Number.NEGATIVE_INFINITY;

		for (const node of targetNodes) {
			minX = Math.min(minX, node.x - node.radius * 2.4);
			minY = Math.min(minY, node.y - node.radius * 2.4);
			maxX = Math.max(maxX, node.x + node.radius * 2.4);
			maxY = Math.max(maxY, node.y + node.radius * 2.4);
		}

		const boundsWidth = Math.max(140, maxX - minX);
		const boundsHeight = Math.max(140, maxY - minY);
		const targetScale = clamp(
			Math.min((width * paddingFactor) / boundsWidth, (height * paddingFactor) / boundsHeight),
			0.3,
			8
		);
		const targetX = minX + boundsWidth / 2;
		const targetY = minY + boundsHeight / 2;

		camera.targetX = targetX;
		camera.targetY = targetY;
		camera.targetScale = targetScale;
		if (initial) {
			camera.x = targetX;
			camera.y = targetY;
			camera.scale = targetScale;
		}
		pendingConnectionRedraw = true;
	}

	function resetGalaxyView(initial = false) {
		activeFamilyId = null;
		zoomLevel = 'galaxy';
		fitToNodes(nodes, 0.8, initial);
		if (!initial && selectedId !== null) {
			onSelect(null);
		}
	}

	function zoomToFamily(familyId: number) {
		const familyNodes = nodes.filter((node) => node.familyId === familyId);
		if (familyNodes.length === 0) return;
		activeFamilyId = familyId;
		zoomLevel = selectedId === null ? 'cluster' : 'node';
		fitToNodes(familyNodes, 0.9);
		pendingConnectionRedraw = true;
	}

	function focusNode(node: GalaxyNode) {
		activeFamilyId = node.familyId;
		zoomLevel = 'node';
		camera.targetX = node.x;
		camera.targetY = node.y;
		camera.targetScale = clamp(node.depth === 0 ? 1.35 : node.depth === 1 ? 2.5 : 3.4, 0.3, 8);
		pendingConnectionRedraw = true;
	}

	function buildParticles(nextEdges: GalaxyEdge[]): HeatParticle[] {
		const particleLimit = isCompactViewport ? 54 : MAX_PARTICLES;
		// Particles are an "active listening" signal. Only spawn them on edges
		// where at least one endpoint actually has listen activity — otherwise
		// the canvas pulses on a library with no data, which is a lie.
		const parentEdges = nextEdges
			.map((edge, index) => ({ edge, index }))
			.filter(({ edge }) => {
				if (edge.type !== 'parent-child') return false;
				const source = nodeById.get(edge.sourceId);
				const target = nodeById.get(edge.targetId);
				return (source?.listenCount ?? 0) > 0 || (target?.listenCount ?? 0) > 0;
			})
			.sort((left, right) => right.edge.weight - left.edge.weight);

		const nextParticles: HeatParticle[] = [];
		for (const { edge, index } of parentEdges) {
			if (nextParticles.length >= particleLimit) break;
			const desiredCount = clamp(Math.round(1 + edge.weight * 2), 1, 3);
			for (let particleIndex = 0; particleIndex < desiredCount; particleIndex += 1) {
				if (nextParticles.length >= particleLimit) break;
				nextParticles.push({
					edgeIndex: index,
					t: ((index * 0.173) + particleIndex * 0.31) % 1,
					speed: 0.0024 + edge.weight * 0.0062,
					alpha: 0.34 + edge.weight * 0.44,
					size: 1.2 + edge.weight * 2.6
				});
			}
		}

		return nextParticles;
	}

	function advanceCamera() {
		const previous = { x: camera.x, y: camera.y, scale: camera.scale };
		const drifting = autoDrift && !isDragging && selectedId === null && activeFamilyId === null;
		const driftX = drifting ? Math.sin(Date.now() / 5400) * 18 : 0;
		const driftY = drifting ? Math.cos(Date.now() / 6800) * 14 : 0;
		const driftTargetX = camera.targetX + driftX;
		const driftTargetY = camera.targetY + driftY;

		camera.x += (driftTargetX - camera.x) * 0.08;
		camera.y += (driftTargetY - camera.y) * 0.08;
		camera.scale += (camera.targetScale - camera.scale) * 0.08;

		if (Math.abs(driftTargetX - camera.x) < 0.01) camera.x = driftTargetX;
		if (Math.abs(driftTargetY - camera.y) < 0.01) camera.y = driftTargetY;
		if (Math.abs(camera.targetScale - camera.scale) < 0.0005) camera.scale = camera.targetScale;

		const snapshot = `${camera.x.toFixed(2)}:${camera.y.toFixed(2)}:${camera.scale.toFixed(3)}`;
		if (snapshot !== lastCameraSnapshot) {
			lastCameraSnapshot = snapshot;
			pendingConnectionRedraw = true;
		}

		return (
			Math.abs(previous.x - camera.x) > 0.01 ||
			Math.abs(previous.y - camera.y) > 0.01 ||
			Math.abs(previous.scale - camera.scale) > 0.0005
		);
	}

	function updateParticles(dtFrames = 1) {
		for (const particle of particles) {
			const edge = edges[particle.edgeIndex];
			if (!edge) continue;
			// speed is tuned per 60Hz frame; dt-scale it so the idle frame gate
			// halves the draw rate without halving how fast particles travel.
			particle.t = (particle.t + particle.speed * dtFrames) % 1;
		}
	}

	function ensureLayerSizes() {
		if (!bgCanvas || !connCanvas || width === 0 || height === 0) return;

		const nextWidth = Math.max(1, Math.floor(width * dpr));
		const nextHeight = Math.max(1, Math.floor(height * dpr));
		for (const layer of [bgCanvas, connCanvas]) {
			if (layer.width !== nextWidth || layer.height !== nextHeight) {
				layer.width = nextWidth;
				layer.height = nextHeight;
			}
		}
	}

	function drawBackgroundLayer() {
		if (!bgCanvas || width === 0 || height === 0) return;
		const ctx = bgCanvas.getContext('2d');
		if (!ctx) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);

		// Deep space gradient
		const fill = ctx.createRadialGradient(width * 0.48, height * 0.48, 24, width * 0.5, height * 0.5, width * 0.86);
		fill.addColorStop(0, 'rgba(18, 20, 38, 0.99)');
		fill.addColorStop(0.38, 'rgba(10, 13, 27, 0.99)');
		fill.addColorStop(0.74, 'rgba(5, 8, 18, 1)');
		fill.addColorStop(1, 'rgba(2, 4, 10, 1)');
		ctx.fillStyle = fill;
		ctx.fillRect(0, 0, width, height);

		// Nebula clouds - saturated enough to actually read as a living sky.
		const nebulaA = ctx.createRadialGradient(width * 0.22, height * 0.28, 0, width * 0.22, height * 0.28, width * 0.34);
		nebulaA.addColorStop(0, 'rgba(88, 144, 255, 0.3)');
		nebulaA.addColorStop(0.5, 'rgba(124, 128, 255, 0.14)');
		nebulaA.addColorStop(1, 'rgba(124, 128, 255, 0)');
		ctx.fillStyle = nebulaA;
		ctx.fillRect(0, 0, width, height);

		const nebulaB = ctx.createRadialGradient(width * 0.76, height * 0.18, 0, width * 0.76, height * 0.18, width * 0.28);
		nebulaB.addColorStop(0, 'rgba(236, 180, 98, 0.18)');
		nebulaB.addColorStop(0.42, 'rgba(179, 123, 244, 0.14)');
		nebulaB.addColorStop(1, 'rgba(247, 37, 133, 0)');
		ctx.fillStyle = nebulaB;
		ctx.fillRect(0, 0, width, height);

		const nebulaC = ctx.createRadialGradient(width * 0.72, height * 0.8, 0, width * 0.72, height * 0.8, width * 0.36);
		nebulaC.addColorStop(0, 'rgba(6, 214, 160, 0.16)');
		nebulaC.addColorStop(0.46, 'rgba(59, 130, 246, 0.09)');
		nebulaC.addColorStop(1, 'rgba(6, 214, 160, 0)');
		ctx.fillStyle = nebulaC;
		ctx.fillRect(0, 0, width, height);

		const nebulaD = ctx.createRadialGradient(width * 0.12, height * 0.85, 0, width * 0.12, height * 0.85, width * 0.3);
		nebulaD.addColorStop(0, 'rgba(190, 96, 220, 0.14)');
		nebulaD.addColorStop(0.5, 'rgba(120, 80, 220, 0.07)');
		nebulaD.addColorStop(1, 'rgba(120, 80, 220, 0)');
		ctx.fillStyle = nebulaD;
		ctx.fillRect(0, 0, width, height);

		// Broad diagonal milky band across the middle - the thing that makes it
		// read as a galaxy instead of a dark room.
		ctx.save();
		ctx.translate(width * 0.52, height * 0.44);
		ctx.rotate(-0.34);
		ctx.scale(1.7, 0.5);
		const band = ctx.createRadialGradient(0, 0, 0, 0, 0, width * 0.55);
		band.addColorStop(0, 'rgba(168, 178, 255, 0.11)');
		band.addColorStop(0.55, 'rgba(130, 140, 230, 0.055)');
		band.addColorStop(1, 'rgba(130, 140, 230, 0)');
		ctx.fillStyle = band;
		ctx.fillRect(-width, -height, width * 2, height * 2);
		ctx.restore();

		// Stars: three layers for real depth instead of one flat scatter.
		// Dust (tiny + crisp + faint) reads as distant; a mid layer sits closer;
		// a handful of hero stars bloom with a soft halo and diffraction glint.
		let seed = 42;
		const random = () => {
			seed = (seed * 1664525 + 1013904223) >>> 0;
			return seed / 4294967296;
		};

		const starTint = (warmth: number, alpha: number) =>
			warmth > 0.82
				? `rgba(196, 208, 255, ${alpha})`
				: warmth < 0.15
					? `rgba(255, 224, 196, ${alpha})`
					: `rgba(255, 255, 255, ${alpha})`;

		const dustCount = isCompactViewport ? 280 : 520;
		for (let index = 0; index < dustCount; index += 1) {
			const x = random() * width;
			const y = random() * height;
			const size = 0.35 + random() * 0.65;
			const alpha = 0.2 + random() * 0.34;
			ctx.fillStyle = starTint(random(), alpha);
			ctx.beginPath();
			ctx.arc(x, y, size, 0, Math.PI * 2);
			ctx.fill();
		}

		const midCount = isCompactViewport ? 64 : 120;
		for (let index = 0; index < midCount; index += 1) {
			const x = random() * width;
			const y = random() * height;
			const size = 0.8 + random() * 0.8;
			const alpha = 0.38 + random() * 0.36;
			ctx.fillStyle = starTint(random(), alpha);
			ctx.beginPath();
			ctx.arc(x, y, size, 0, Math.PI * 2);
			ctx.fill();
		}

		const heroCount = isCompactViewport ? 12 : 22;
		for (let index = 0; index < heroCount; index += 1) {
			const x = random() * width;
			const y = random() * height;
			const core = 0.95 + random() * 0.9;
			const warmth = random();
			const bloom = ctx.createRadialGradient(x, y, 0, x, y, core * 5);
			bloom.addColorStop(0, starTint(warmth, 0.32));
			bloom.addColorStop(0.4, starTint(warmth, 0.08));
			bloom.addColorStop(1, 'rgba(255, 255, 255, 0)');
			ctx.fillStyle = bloom;
			ctx.beginPath();
			ctx.arc(x, y, core * 5, 0, Math.PI * 2);
			ctx.fill();

			ctx.fillStyle = starTint(warmth, 0.8);
			ctx.beginPath();
			ctx.arc(x, y, core, 0, Math.PI * 2);
			ctx.fill();

			if (random() > 0.74) {
				ctx.save();
				ctx.globalAlpha = 0.3;
				ctx.strokeStyle = starTint(warmth, 0.65);
				ctx.lineWidth = 0.55;
				const spike = core * 4;
				ctx.beginPath();
				ctx.moveTo(x - spike, y);
				ctx.lineTo(x + spike, y);
				ctx.moveTo(x, y - spike);
				ctx.lineTo(x, y + spike);
				ctx.stroke();
				ctx.restore();
			}
		}
	}

	function drawConnectionsLayer() {
		if (!connCanvas || width === 0 || height === 0) return;
		const ctx = connCanvas.getContext('2d');
		if (!ctx) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);

		// Draw taxonomy edges (full set, with gradients)
		for (const edge of edges) {
			const source = nodeById.get(edge.sourceId);
			const target = nodeById.get(edge.targetId);
			if (!source || !target) continue;

			const sourceScreen = worldToScreen(source.x, source.y);
			const targetScreen = worldToScreen(target.x, target.y);
			// Early viewport cull
			if (
				(sourceScreen.x < -120 && targetScreen.x < -120) ||
				(sourceScreen.x > width + 120 && targetScreen.x > width + 120) ||
				(sourceScreen.y < -120 && targetScreen.y < -120) ||
				(sourceScreen.y > height + 120 && targetScreen.y > height + 120)
			) {
				continue;
			}

			const activity = edgeActivity(edge);
			const isSelectedEdge = selectedId !== null && (edge.sourceId === selectedId || edge.targetId === selectedId);
			const emphasis = isSelectedEdge ? 1.28 : 1;
			// Vibe mode: fade edges when BPM diff > 60
			let bpmFade = 1;
			if (viewMode === 'vibe' && source.avgBpm != null && target.avgBpm != null) {
				const bpmDiff = Math.abs(source.avgBpm - target.avgBpm);
				if (bpmDiff > 60) {
					bpmFade = Math.max(0, 1 - (bpmDiff - 60) / 60);
				}
			}
			const opacity =
				(edge.type === 'parent-child'
					? 0.18 + edge.weight * 0.22
					: 0.06 + edge.weight * 0.1) *
				activity *
				emphasis *
				bpmFade;
			const lineWidth =
				(edge.type === 'parent-child'
					? 0.8 + edge.weight * 1.5
					: 0.6 + edge.weight * 0.9) *
				(0.72 + activity * 0.42) *
				emphasis;
			const dx = targetScreen.x - sourceScreen.x;
			const dy = targetScreen.y - sourceScreen.y;
			const distance = Math.max(1, Math.hypot(dx, dy));
			const normalX = -dy / distance;
			const normalY = dx / distance;
			const curve =
				edge.type === 'sibling'
					? Math.min(44, distance * 0.1)
					: Math.min(26, distance * 0.06);
			const curveSign = ((edge.sourceId + edge.targetId) & 1) === 0 ? 1 : -1;
			const controlX = (sourceScreen.x + targetScreen.x) * 0.5 + normalX * curve * curveSign;
			const controlY = (sourceScreen.y + targetScreen.y) * 0.5 + normalY * curve * curveSign;
			// Solid blended stroke from a per-edge cache. Per-redraw linear
			// gradients + shadowBlur were the main reason pan/zoom lagged.
			ctx.beginPath();
			ctx.globalAlpha = opacity;
			ctx.lineWidth = lineWidth;
			ctx.strokeStyle = edgeStrokeColor(edge, source, target);
			ctx.moveTo(sourceScreen.x, sourceScreen.y);
			ctx.quadraticCurveTo(controlX, controlY, targetScreen.x, targetScreen.y);
			ctx.stroke();
		}
		ctx.globalAlpha = 1;

		pendingConnectionRedraw = false;
	}

	function drawParticles(ctx: CanvasRenderingContext2D) {
		for (const particle of particles) {
			const edge = edges[particle.edgeIndex];
			if (!edge) continue;
			const source = nodeById.get(edge.sourceId);
			const target = nodeById.get(edge.targetId);
			if (!source || !target) continue;

			const sourceScreen = worldToScreen(source.x, source.y);
			const targetScreen = worldToScreen(target.x, target.y);
			const x = sourceScreen.x + (targetScreen.x - sourceScreen.x) * particle.t;
			const y = sourceScreen.y + (targetScreen.y - sourceScreen.y) * particle.t;
			if (x < -40 || x > width + 40 || y < -40 || y > height + 40) continue;

			const activity = edgeActivity(edge);
			if (activity < 0.2) continue;
			const isSelectedEdge = selectedId !== null && (edge.sourceId === selectedId || edge.targetId === selectedId);
			const edgeFade = particle.t < 0.15 ? particle.t / 0.15 : particle.t > 0.85 ? (1 - particle.t) / 0.15 : 1;
			const heatFactor = viewMode === 'heat' ? 0.72 + edge.weight * 0.8 : 0.54;
			// Skip shadow for performance
			ctx.globalAlpha = particle.alpha * edgeFade * activity * heatFactor * (isSelectedEdge ? 1.15 : 1);
			ctx.fillStyle = source.color;
			ctx.beginPath();
			ctx.arc(x, y, particle.size * (0.85 + activity * 0.25), 0, Math.PI * 2);
			ctx.fill();
		}
		ctx.globalAlpha = 1;
	}

	function drawFamilyFields(ctx: CanvasRenderingContext2D) {
		const rootNodes = nodes.filter((node) => node.depth === 0);
		for (const node of rootNodes) {
			if (!nodeIsVisible(node)) continue;
			const activity = nodeActivity(node);
			if (activity < 0.24) continue;

			const screen = worldToScreen(node.x, node.y);
			const fieldRadius = clamp(160 * camera.scale, 90, 320);
			const glow = ctx.createRadialGradient(
				screen.x,
				screen.y,
				0,
				screen.x,
				screen.y,
				fieldRadius
			);
			// Heat mode: amplify hot families and dim cold ones for clear at-a-glance contrast.
			const heatBoost = viewMode === 'heat' ? 0.45 + node.heatNorm * 2.4 : 0.78;
			glow.addColorStop(0, hexToRgba(node.color, 0.14 * activity * heatBoost));
			glow.addColorStop(0.42, hexToRgba(node.color, 0.07 * activity * heatBoost));
			glow.addColorStop(1, hexToRgba(node.color, 0));
			ctx.fillStyle = glow;
			ctx.beginPath();
			ctx.arc(screen.x, screen.y, fieldRadius, 0, Math.PI * 2);
			ctx.fill();
		}
	}

	function drawVignette(ctx: CanvasRenderingContext2D) {
		const vignette = ctx.createRadialGradient(
			width * 0.5,
			height * 0.5,
			Math.min(width, height) * 0.25,
			width * 0.5,
			height * 0.5,
			Math.max(width, height) * 0.68
		);
		vignette.addColorStop(0, 'rgba(0, 0, 0, 0)');
		vignette.addColorStop(1, 'rgba(4, 6, 12, 0.28)');
		ctx.save();
		ctx.fillStyle = vignette;
		ctx.fillRect(0, 0, width, height);
		ctx.restore();
	}

	function labelAlphaForNode(node: GalaxyNode): number {
		const inActiveFamily = activeFamilyId !== null && node.familyId === activeFamilyId;

		if (isCompactViewport) {
			if (selectedId === node.id) return 0.96;
			if (selectedLineageHas(node.id) && node.depth <= 1) return 0.8;
			return node.depth === 0 ? 0.72 : 0;
		}
		if (!labelsEnabled) {
			if (selectedId === node.id) return 0.96;
			if (node.depth === 0) return 0.74;
			return 0;
		}
		if (selectedId === node.id) return 0.96;
		if (inActiveFamily && node.depth === 0) return 0.94;
		if (inActiveFamily && node.depth === 1) return 0.86;
		if (inActiveFamily && selectedLineageHas(node.id)) return 0.82;
		if (inActiveFamily && node.depth === 2 && zoomLevel !== 'galaxy') {
			return clamp(0.62 + node.heatNorm * 0.16, 0.62, 0.78);
		}
		if (inActiveFamily && camera.scale > 1.35 && node.trackCount >= 20) {
			return clamp(0.32 + node.heatNorm * 0.2, 0.32, 0.56);
		}
		if (camera.scale < 0.8) return node.depth === 0 ? 0.92 : 0;
		if (camera.scale < 2) {
			if (node.depth > 1) return 0;
			return node.depth === 0 ? 0.94 : clamp((camera.scale - 0.8) / 1.2, 0.15, 0.88);
		}
		if (node.depth > 1) {
			if (activeFamilyId !== null && node.trackCount >= 25 && camera.scale > 2.8) {
				return clamp(0.34 + node.heatNorm * 0.22, 0.34, 0.58);
			}
			return 0;
		}
		return clamp(0.6 + node.heatNorm * 0.25, 0.6, 0.95);
	}

	function labelUsesChip(node: GalaxyNode): boolean {
		const inActiveFamily = activeFamilyId !== null && node.familyId === activeFamilyId;
		return node.depth <= 1 || (inActiveFamily && node.depth === 2 && zoomLevel !== 'galaxy');
	}

	function clampLabelRect(x: number, y: number, rectWidth: number, rectHeight: number) {
		const margin = 8;
		return {
			x: clamp(x, margin, Math.max(margin, width - rectWidth - margin)),
			y: clamp(y, margin, Math.max(margin, height - rectHeight - margin))
		};
	}

	function placeHoverCard(screen: { x: number; y: number }, nodeRadius: number): HoverCardPosition {
		const rightX = screen.x + nodeRadius + HOVER_CARD_CURSOR_CLEARANCE_X;
		const leftX = screen.x - nodeRadius - HOVER_CARD_CURSOR_CLEARANCE_X;
		const bottomY = clamp(
			screen.y - nodeRadius - HOVER_CARD_CURSOR_CLEARANCE_Y,
			HOVER_CARD_EDGE_MARGIN + 72,
			Math.max(HOVER_CARD_EDGE_MARGIN + 72, height - HOVER_CARD_EDGE_MARGIN)
		);

		if (rightX + HOVER_CARD_ESTIMATED_WIDTH + HOVER_CARD_EDGE_MARGIN <= width) {
			return {
				x: clamp(
					rightX,
					HOVER_CARD_EDGE_MARGIN,
					Math.max(HOVER_CARD_EDGE_MARGIN, width - HOVER_CARD_ESTIMATED_WIDTH - HOVER_CARD_EDGE_MARGIN)
				),
				y: bottomY,
				align: 'left'
			};
		}

		return {
			x: clamp(
				leftX,
				HOVER_CARD_ESTIMATED_WIDTH + HOVER_CARD_EDGE_MARGIN,
				Math.max(HOVER_CARD_ESTIMATED_WIDTH + HOVER_CARD_EDGE_MARGIN, width - HOVER_CARD_EDGE_MARGIN)
			),
			y: bottomY,
			align: 'right'
		};
	}

	function drawNodesAndLabels(ctx: CanvasRenderingContext2D) {
		const visibleNodes = nodes.filter(nodeIsVisible);

		// Pass 1: mode-emphasis glows, blitted from cached sprites. Map mode
		// skips this pass entirely - the body sprite carries its own tight glow,
		// and a second per-node halo is exactly the haze that made things fuzzy.
		if (viewMode !== 'map') {
			for (const node of visibleNodes) {
				const screen = worldToScreen(node.x, node.y);
				const radius = node.radius;
				const activity = nodeActivity(node);
				if (activity < 0.16) continue;

				// Vibe mode: energy-colored glow when DSP is available.
				// Unanalyzed nodes get NO halo so the data-coverage gap is honest.
				if (viewMode === 'vibe') {
					const eColor = energyColor(node.avgEnergy);
					if (!eColor) continue;
					const glowR = danceGlowRadius(node.avgDanceability, radius);
					const sprite = getGlowSprite(eColor);
					if (!sprite) continue;
					ctx.globalAlpha = clamp(0.55 * activity, 0.12, 0.6);
					ctx.drawImage(sprite, screen.x - glowR, screen.y - glowR, glowR * 2, glowR * 2);
					continue;
				}

				// Heat mode: cold nodes get tiny halos, hot nodes blaze.
				// Rediscover mode: candidates blaze, others fade.
				let heatExtra = 0;
				let haloAlphaScale = 1;
				if (viewMode === 'heat') {
					heatExtra = node.heatNorm * radius * 0.85;
					haloAlphaScale = 0.35 + node.heatNorm * 1.4;
				} else if (viewMode === 'rediscover') {
					if (isRediscoverCandidate(node)) {
						heatExtra = radius * 0.7;
						haloAlphaScale = 1.6;
					} else {
						haloAlphaScale = 0.25;
					}
				}
				const haloRadius = radius + 2 + radius * 0.4 + heatExtra;
				const sprite = getGlowSprite(node.color);
				if (!sprite) continue;
				ctx.globalAlpha = clamp(0.4 * activity * haloAlphaScale, 0, 0.85);
				ctx.drawImage(sprite, screen.x - haloRadius, screen.y - haloRadius, haloRadius * 2, haloRadius * 2);
			}
			ctx.globalAlpha = 1;
		}

		// Pass 2: solid sharp cores (no blur)
		const now = performance.now();
		for (const node of visibleNodes) {
			const screen = worldToScreen(node.x, node.y);
			let radius = node.radius;
			const activity = nodeActivity(node);
			if (activity < 0.16) continue;

			// Rediscover: gently pulse + grow candidates so they feel alive,
			// signalling the user to play them. The moment a play registers,
			// the next heat refresh drops them out of the candidate set and
			// they shrink back down — visible feedback on listening.
			if (viewMode === 'rediscover' && isRediscoverCandidate(node)) {
				const pulse = 1 + Math.sin(now / 600 + node.id * 0.7) * 0.08;
				radius = node.radius * 1.28 * pulse;
			}

			let baseColor = node.color;
			if (viewMode === 'vibe') {
				const eColor = energyColor(node.avgEnergy);
				if (eColor) {
					ctx.globalAlpha = activity;
					baseColor = eColor;
				} else {
					// No DSP coverage — render desaturated so Vibe is honest about its data.
					ctx.globalAlpha = activity * 0.55;
					baseColor = '#4a4d5e';
				}
			} else if (viewMode === 'heat') {
				// Cold nodes fade, hot nodes stay full bright.
				ctx.globalAlpha = activity * (0.5 + node.heatNorm * 0.6);
			} else if (viewMode === 'rediscover') {
				ctx.globalAlpha = activity;
				baseColor = isRediscoverCandidate(node) ? node.color : '#3a3d4e';
			} else {
				ctx.globalAlpha = activity;
			}
			const jitter = ((node.id * 37) % 20) / 100; // 0 .. 0.19, stable per node
			const sprite = getNodeSprite(baseColor, node.radius, jitter);
			if (sprite) {
				const half = radius * BODY_GLOW_FACTOR;
				ctx.drawImage(sprite, screen.x - half, screen.y - half, half * 2, half * 2);
			}
		}
		ctx.globalAlpha = 1;

		// Pass 3: rings and selection
		for (const node of visibleNodes) {
			const screen = worldToScreen(node.x, node.y);
			const radius = node.radius;
			const activity = nodeActivity(node);
			const nodeHeat = viewMode === 'heat' ? node.heatNorm : 0;

			if (node.depth === 0) {
				ctx.globalAlpha = clamp(0.18 + activity * 0.18, 0.16, 0.38);
				ctx.lineWidth = 1.2;
				ctx.strokeStyle = hexToRgba(node.color, 0.55);
				ctx.beginPath();
				ctx.arc(screen.x, screen.y, radius + 10 + nodeHeat * 8, 0, Math.PI * 2);
				ctx.stroke();
			}

			if (nodeIsSeed(node.id)) {
				ctx.globalAlpha = 0.76 * activity;
				ctx.setLineDash([3, 6]);
				ctx.lineWidth = 1.6;
				ctx.strokeStyle = 'rgba(255, 245, 220, 0.85)';
				ctx.beginPath();
				ctx.arc(screen.x, screen.y, radius + 8, 0, Math.PI * 2);
				ctx.stroke();
				ctx.setLineDash([]);
			}

			if (selectedId === node.id) {
				ctx.lineWidth = 2.2;
				ctx.strokeStyle = 'rgba(255, 255, 255, 0.92)';
				ctx.beginPath();
				ctx.arc(screen.x, screen.y, radius + 4, 0, Math.PI * 2);
				ctx.stroke();
			} else if (hoveredNodeId === node.id && !isDragging) {
				ctx.lineWidth = 1.6;
				ctx.strokeStyle = node.color;
				ctx.beginPath();
				ctx.arc(screen.x, screen.y, radius + 3, 0, Math.PI * 2);
				ctx.stroke();
			}
		}

		for (const node of visibleNodes) {
			if (hoveredNodeId === node.id && !isDragging) continue;
			const alpha = labelAlphaForNode(node);
			if (alpha <= 0) continue;
			const activity = nodeActivity(node);
			const activeFamilyLabel = activeFamilyId !== null && node.familyId === activeFamilyId;
			const labelActivity = activeFamilyLabel && labelUsesChip(node) ? Math.max(activity, 0.82) : activity;
			if (labelActivity < 0.22) continue;

			const screen = worldToScreen(node.x, node.y);
			const fontSize = node.depth === 0 ? 13 : node.depth === 1 ? 11.5 : 11;
			const label = node.depth === 0 ? node.name.toUpperCase() : node.name;
			ctx.save();
			ctx.globalAlpha = alpha * labelActivity;
			ctx.font = node.depth === 0 ? fontDisplay : fontBody.replace('12px', `${fontSize}px`);
			ctx.textAlign = 'center';
			ctx.textBaseline = 'top';

			if (labelUsesChip(node)) {
				const textWidth = ctx.measureText(label).width;
				const chipWidth = textWidth + (node.depth === 0 ? 18 : node.depth === 1 ? 14 : 12);
				const chipHeight = node.depth === 0 ? 22 : node.depth === 1 ? 19 : 18;
				const { x: chipX, y: chipY } = clampLabelRect(
					screen.x - chipWidth / 2,
					screen.y + node.radius + 8,
					chipWidth,
					chipHeight
				);
				roundedRectPath(ctx, chipX, chipY, chipWidth, chipHeight, 10);
				ctx.fillStyle = node.depth === 2 ? 'rgba(7, 9, 18, 0.9)' : 'rgba(8, 10, 18, 0.8)';
				ctx.fill();
				ctx.lineWidth = 1;
				ctx.strokeStyle = hexToRgba(node.color, node.depth === 0 ? 0.5 : node.depth === 1 ? 0.4 : 0.42);
				ctx.stroke();
				ctx.textBaseline = 'middle';
				ctx.fillStyle = node.depth === 2 ? 'rgba(248, 250, 255, 0.98)' : 'rgba(246, 248, 255, 0.96)';
				ctx.shadowBlur = node.depth === 2 ? 12 : 8;
				ctx.shadowColor = 'rgba(0, 0, 0, 0.45)';
				ctx.fillText(label, chipX + chipWidth / 2, chipY + chipHeight / 2);
			} else {
				ctx.fillStyle = 'rgba(255, 255, 255, 0.9)';
				ctx.shadowBlur = 10;
				ctx.shadowColor = 'rgba(0, 0, 0, 0.35)';
				ctx.fillText(label, screen.x, screen.y + node.radius + 8);
			}
			ctx.restore();
		}
	}

	function drawFrame() {
		if (!canvasEl || width === 0 || height === 0) return;
		const ctx = canvasEl.getContext('2d');
		if (!ctx || !bgCanvas || !connCanvas) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);
		ctx.drawImage(bgCanvas, 0, 0, width, height);
		drawParallaxStars(ctx);
		// Family haze fields only in heat mode, where the glow IS the data.
		// Everywhere else they wash out the starfield and blur the nodes.
		if (viewMode === 'heat') {
			drawFamilyFields(ctx);
		}

		if (pendingConnectionRedraw) {
			drawConnectionsLayer();
		}
		ctx.drawImage(connCanvas, 0, 0, width, height);

		drawParticles(ctx);
		drawNodesAndLabels(ctx);
		drawVignette(ctx);

		if (hoveredNode && !isDragging) {
			const screen = worldToScreen(hoveredNode.x, hoveredNode.y);
			mixPillPosition = { x: screen.x, y: screen.y - hoveredNode.radius - 18 };
			hoverCardPosition = placeHoverCard(screen, hoveredNode.radius);
		} else {
			mixPillPosition = null;
			hoverCardPosition = null;
		}
	}

	function handleCanvasClick(clientX: number, clientY: number, additive = false) {
		const node = getNodeAtPoint(clientX, clientY);
		if (!node) {
			hoveredNodeId = null;
			if (selectedId !== null) {
				zoomLevel = activeFamilyId === null ? 'galaxy' : 'cluster';
				onSelect(null);
				return;
			}
			if (activeFamilyId !== null) {
				resetGalaxyView();
			}
			return;
		}

		if (additive) {
			onToggleSeed(node.id);
			return;
		}

		if (zoomLevel === 'galaxy' && node.depth === 0) {
			activeFamilyId = node.familyId;
			zoomLevel = 'cluster';
			onSelect(null);
			onZoomFamily(node.familyId);
			zoomToFamily(node.familyId);
			return;
		}

		if (zoomLevel === 'galaxy' && node.depth > 0) {
			onZoomFamily(node.familyId);
		}

		focusNode(node);
		onSelect(node.id);
	}

	function handlePointerDown(event: PointerEvent) {
		if (!canvasEl) return;
		activePointerId = event.pointerId;
		if (isCompactViewport && event.pointerType !== 'mouse') {
			dragStart = null;
			isDragging = false;
			return;
		}
		dragStart = {
			x: event.offsetX,
			y: event.offsetY,
			cameraX: camera.x,
			cameraY: camera.y
		};
		isDragging = false;
		canvasEl.setPointerCapture(event.pointerId);
	}

	function handlePointerMove(event: PointerEvent) {
		if (!canvasEl) return;
		if (isCompactViewport && event.pointerType !== 'mouse') return;

		if (activePointerId === event.pointerId && dragStart) {
			const dx = event.offsetX - dragStart.x;
			const dy = event.offsetY - dragStart.y;
			if (!isDragging && Math.hypot(dx, dy) > 4) {
				isDragging = true;
			}
			if (isDragging) {
				camera.x = dragStart.cameraX - dx / camera.scale;
				camera.y = dragStart.cameraY - dy / camera.scale;
				camera.targetX = camera.x;
				camera.targetY = camera.y;
				pendingConnectionRedraw = true;
				hoveredNodeId = null;
				return;
			}
		}

		const nextHover = getNodeAtPoint(event.offsetX, event.offsetY);
		hoveredNodeId = nextHover?.id ?? null;
	}

	function finishPointer(event: PointerEvent) {
		if (!canvasEl || activePointerId !== event.pointerId) return;
		if (!(isCompactViewport && event.pointerType !== 'mouse')) {
			canvasEl.releasePointerCapture(event.pointerId);
		}
		const dragged = isDragging;
		activePointerId = null;
		dragStart = null;
		isDragging = false;

		if (!dragged) {
			handleCanvasClick(event.offsetX, event.offsetY, event.shiftKey);
		}
	}

	function handlePointerLeave() {
		if (activePointerId === null) {
			hoveredNodeId = null;
			mixPillPosition = null;
			hoverCardPosition = null;
		}
	}

	function handleWheel(event: WheelEvent) {
		if (event.ctrlKey || event.metaKey) return; // yield to global UI-zoom handler
		if (isCompactViewport) return;
		event.preventDefault();
		const pointBeforeZoom = screenToWorld(event.offsetX, event.offsetY);
		const zoomFactor = event.deltaY < 0 ? 1.12 : 0.9;
		const nextScale = clamp(camera.targetScale * zoomFactor, 0.3, 8);

		camera.targetScale = nextScale;
		camera.scale = nextScale;
		camera.targetX = pointBeforeZoom.x - (event.offsetX - width / 2) / nextScale;
		camera.targetY = pointBeforeZoom.y - (event.offsetY - height / 2) / nextScale;
		camera.x = camera.targetX;
		camera.y = camera.targetY;
		pendingConnectionRedraw = true;
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.key !== 'Escape') return;
		if (selectedId !== null) {
			zoomLevel = activeFamilyId === null ? 'galaxy' : 'cluster';
			onSelect(null);
			return;
		}
		if (activeFamilyId !== null) {
			resetGalaxyView();
		}
	}

	function resizeCanvas() {
		if (!wrapEl || !canvasEl) return;
		const rect = wrapEl.getBoundingClientRect();
		width = Math.max(1, Math.floor(rect.width));
		height = Math.max(1, Math.floor(rect.height));
		// Cap DPR to 2x to prevent excessive pixel counts on hi-res monitors
		dpr = Math.min(window.devicePixelRatio || 1, 2);
		if (dpr !== spriteDpr) {
			spriteDpr = dpr;
			invalidateSprites();
		}

		const nextWidth = Math.max(1, Math.floor(width * dpr));
		const nextHeight = Math.max(1, Math.floor(height * dpr));
		canvasEl.width = nextWidth;
		canvasEl.height = nextHeight;
		canvasEl.style.width = `${width}px`;
		canvasEl.style.height = `${height}px`;

		ensureLayerSizes();
		drawBackgroundLayer();
		pendingConnectionRedraw = true;
	}

	$effect(() => {
		const signature = nodes.map((node) => node.id).join(':');
		if (signature !== lastLayoutSignature) {
			lastLayoutSignature = signature;
			edgeColorCache.clear();
			particles = buildParticles(edges);
			if (width > 0 && height > 0 && nodes.length > 0) {
				resetGalaxyView(true);
			}
		} else {
			particles = buildParticles(edges);
			pendingConnectionRedraw = true;
		}
	});

	$effect(() => {
		if (resetViewToken === lastResetViewToken) return;
		lastResetViewToken = resetViewToken;
		resetGalaxyView();
	});

	$effect(() => {
		if (selectedId === null) {
			if (zoomLevel === 'node') {
				zoomLevel = activeFamilyId === null ? 'galaxy' : 'cluster';
			}
			return;
		}

		const node = nodeById.get(selectedId);
		if (!node) return;
		focusNode(node);
	});

	$effect(() => {
		if (focusNodeId === null) {
			lastFocusedExternalId = null;
			return;
		}
		if (focusNodeId === lastFocusedExternalId) return;
		lastFocusedExternalId = focusNodeId;
		const node = nodeById.get(focusNodeId);
		if (!node) return;
		if (node.depth === 0) {
			onZoomFamily(node.familyId);
			zoomToFamily(node.familyId);
			onSelect(node.id);
			return;
		}
		activeFamilyId = node.familyId;
		onZoomFamily(node.familyId);
		focusNode(node);
		onSelect(node.id);
	});

	onMount(() => {
		bgCanvas = document.createElement('canvas');
		connCanvas = document.createElement('canvas');
		resizeCanvas();

		if (wrapEl) {
			resizeObserver = new ResizeObserver(() => {
				resizeCanvas();
				pendingConnectionRedraw = true;
			});
			resizeObserver.observe(wrapEl);
		}

		window.addEventListener('keydown', handleKeydown);

		// Decorative full-window canvas at up to 2x DPR: full-rate redraw only
		// while the user is interacting (drag, hover, wheel). The idle drift +
		// particle motion runs at half rate with dt-scaled speeds, and a hidden
		// tab skips the work entirely.
		const IDLE_FRAME_MS = 1000 / 30;
		const POINTER_ACTIVE_MS = 1000;
		let lastPointerAt = 0;
		let lastTickAt = performance.now();
		const notePointer = () => {
			lastPointerAt = performance.now();
		};
		window.addEventListener('pointermove', notePointer, { passive: true });
		window.addEventListener('pointerdown', notePointer, { passive: true });
		window.addEventListener('wheel', notePointer, { passive: true });

		const tick = () => {
			animationFrame = window.requestAnimationFrame(tick);
			const now = performance.now();
			if (document.hidden) {
				lastTickAt = now;
				return;
			}
			const interacting = isDragging || now - lastPointerAt < POINTER_ACTIVE_MS;
			if (!interacting && now - lastTickAt < IDLE_FRAME_MS) return;
			const dtFrames = Math.min(4, (now - lastTickAt) / (1000 / 60));
			if (interacting) {
				lastTickAt = now;
			} else {
				// Grid-advance instead of stamping now: stamping quantizes a 30fps
				// target down to ~20fps against the 60Hz rAF grid.
				lastTickAt += IDLE_FRAME_MS;
				if (now - lastTickAt > IDLE_FRAME_MS) lastTickAt = now;
			}
			advanceCamera();
			updateParticles(dtFrames);
			drawFrame();
		};

		animationFrame = window.requestAnimationFrame(tick);

		return () => {
			window.cancelAnimationFrame(animationFrame);
			resizeObserver?.disconnect();
			window.removeEventListener('keydown', handleKeydown);
			window.removeEventListener('pointermove', notePointer);
			window.removeEventListener('pointerdown', notePointer);
			window.removeEventListener('wheel', notePointer);
			if (hoverCardTimer) {
				clearTimeout(hoverCardTimer);
				hoverCardTimer = null;
			}
		};
	});
</script>

<div class="galaxy-wrap" bind:this={wrapEl}>
	<canvas
		bind:this={canvasEl}
		class="galaxy-canvas"
		onpointerdown={handlePointerDown}
		onpointermove={handlePointerMove}
		onpointerup={finishPointer}
		onpointercancel={finishPointer}
		onpointerleave={handlePointerLeave}
		onwheel={handleWheel}
		ondblclick={(event) => {
			const node = getNodeAtPoint(event.offsetX, event.offsetY);
			if (node) {
				onEnterInterior(node.id);
			}
		}}
	></canvas>

	{#if hoveredNode && mixPillPosition && !isDragging}
		<button
			class="mix-pill"
			style={`transform: translate(${mixPillPosition.x}px, ${mixPillPosition.y}px) translate(-50%, -100%);`}
			onclick={(event) => {
				event.stopPropagation();
				onMix(hoveredNode.id);
			}}
		>
			▶ Mix
		</button>
	{/if}

	{#if hoverCardNode && hoverCardPosition && !isDragging && hoverCardId === hoveredNodeId}
		{@const hoverArtists = artistChipMap.get(hoverCardNode.id) ?? []}
		{@const hoverListenSec = Math.floor(hoverCardNode.totalListenedMs / 1000)}
		{@const hoverHours = Math.floor(hoverListenSec / 3600)}
		{@const hoverMinutes = Math.floor((hoverListenSec % 3600) / 60)}
		{@const hoverCardTransform = hoverCardPosition.align === 'right' ? 'translate(-100%, -100%)' : 'translate(0, -100%)'}
		<div
			class="hover-card"
			style={`transform: translate(${hoverCardPosition.x}px, ${hoverCardPosition.y}px) ${hoverCardTransform};`}
		>
			<div class="hover-head">
				<span class="hover-dot" style={`background: ${hoverCardNode.color}`}></span>
				<span class="hover-name">{hoverCardNode.name}</span>
			</div>
			<span class="hover-sub">
				{hoverCardNode.familyName} · {hoverCardNode.trackCount.toLocaleString()} tracks
				{#if hoverCardNode.totalListenedMs > 0}
					· {hoverHours > 0 ? `${hoverHours}h ${hoverMinutes}m` : `${hoverMinutes}m`}
				{/if}
			</span>
			{#if hoverArtists.length > 0}
				<span class="hover-meta">Top: {hoverArtists.slice(0, 2).join(', ')}</span>
			{/if}
			{#if viewMode === 'vibe' && (hoverCardNode.avgBpm != null || hoverCardNode.avgEnergy != null || hoverCardNode.avgDanceability != null)}
				<span class="hover-meta hover-vibe">
					{#if hoverCardNode.avgBpm != null}<span>{Math.round(hoverCardNode.avgBpm)} BPM</span>{/if}
					{#if hoverCardNode.avgEnergy != null}<span>E {hoverCardNode.avgEnergy.toFixed(2)}</span>{/if}
					{#if hoverCardNode.avgDanceability != null}<span>D {hoverCardNode.avgDanceability.toFixed(2)}</span>{/if}
				</span>
			{/if}
		</div>
	{/if}
</div>

<style>
	.galaxy-wrap {
		position: absolute;
		inset: 0;
		overflow: hidden;
		border-radius: var(--radius-lg);
		background:
			radial-gradient(circle at 16% 20%, color-mix(in srgb, var(--atlas-haze-a) 80%, transparent), transparent 36%),
			radial-gradient(circle at 84% 16%, color-mix(in srgb, var(--atlas-haze-b) 78%, transparent), transparent 30%),
			radial-gradient(circle at 76% 82%, color-mix(in srgb, var(--atlas-haze-c) 80%, transparent), transparent 34%),
			var(--atlas-bg);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 52%, transparent);
		box-shadow:
			0 18px 56px rgba(0, 0, 0, 0.42),
			inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 50%, transparent);
	}

	.galaxy-canvas {
		display: block;
		width: 100%;
		height: 100%;
		cursor: grab;
		touch-action: none;
	}

	.galaxy-canvas:active {
		cursor: grabbing;
	}

	.mix-pill {
		position: absolute;
		left: 0;
		top: 0;
		padding: 7px 14px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--accent-soft) 78%, var(--instrument-surface));
		color: var(--text-primary);
		border: 1px solid color-mix(in srgb, var(--accent-line) 88%, transparent);
		box-shadow:
			0 0 20px color-mix(in srgb, var(--accent-glow) 82%, transparent),
			inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 40%, transparent);
		backdrop-filter: var(--blur-overlay);
		-webkit-backdrop-filter: var(--blur-overlay);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		pointer-events: auto;
		z-index: 5;
		transition: transform var(--motion-fast), box-shadow var(--motion-fast), border-color var(--motion-fast);
	}

	.mix-pill:hover {
		box-shadow:
			0 0 26px color-mix(in srgb, var(--accent-glow) 94%, transparent),
			inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 56%, transparent);
		border-color: color-mix(in srgb, var(--accent-line) 100%, transparent);
	}

	@media (max-width: 760px) {
		.galaxy-wrap {
			border-radius: 26px;
			background: linear-gradient(180deg, rgba(13, 15, 24, 0.96), rgba(8, 10, 16, 0.98));
		}

		.galaxy-canvas {
			cursor: default;
			touch-action: pan-y pinch-zoom;
		}

		.mix-pill {
			display: none;
		}
	}

	.hover-card {
		position: absolute;
		left: 0;
		top: 0;
		pointer-events: none;
		z-index: 4;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 8px 12px;
		min-width: 160px;
		max-width: 260px;
		border-radius: var(--radius-sm);
		background: rgba(10, 10, 18, 0.92);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
		animation: hover-card-in 140ms ease-out both;
	}

	@keyframes hover-card-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.hover-head {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.hover-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex-shrink: 0;
	}

	.hover-name {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}

	.hover-sub,
	.hover-meta {
		font-size: var(--font-size-2xs);
		color: var(--signal-text);
		font-variant-numeric: tabular-nums;
	}

	.hover-vibe {
		display: inline-flex;
		gap: 8px;
	}
</style>
