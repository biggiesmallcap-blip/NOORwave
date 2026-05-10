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

	function updateParticles() {
		for (const particle of particles) {
			const edge = edges[particle.edgeIndex];
			if (!edge) continue;
			particle.t = (particle.t + particle.speed) % 1;
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

		// Nebula clouds
		const nebulaA = ctx.createRadialGradient(width * 0.22, height * 0.28, 0, width * 0.22, height * 0.28, width * 0.32);
		nebulaA.addColorStop(0, 'rgba(88, 144, 255, 0.22)');
		nebulaA.addColorStop(0.5, 'rgba(124, 128, 255, 0.1)');
		nebulaA.addColorStop(1, 'rgba(124, 128, 255, 0)');
		ctx.fillStyle = nebulaA;
		ctx.fillRect(0, 0, width, height);

		const nebulaB = ctx.createRadialGradient(width * 0.76, height * 0.18, 0, width * 0.76, height * 0.18, width * 0.26);
		nebulaB.addColorStop(0, 'rgba(236, 180, 98, 0.14)');
		nebulaB.addColorStop(0.42, 'rgba(179, 123, 244, 0.11)');
		nebulaB.addColorStop(1, 'rgba(247, 37, 133, 0)');
		ctx.fillStyle = nebulaB;
		ctx.fillRect(0, 0, width, height);

		const nebulaC = ctx.createRadialGradient(width * 0.72, height * 0.8, 0, width * 0.72, height * 0.8, width * 0.34);
		nebulaC.addColorStop(0, 'rgba(6, 214, 160, 0.12)');
		nebulaC.addColorStop(0.46, 'rgba(59, 130, 246, 0.06)');
		nebulaC.addColorStop(1, 'rgba(6, 214, 160, 0)');
		ctx.fillStyle = nebulaC;
		ctx.fillRect(0, 0, width, height);

		drawNebulaVeins(ctx);

		// Subtle grid texture
		ctx.save();
		ctx.globalAlpha = 0.03;
		ctx.strokeStyle = '#8888cc';
		ctx.lineWidth = 0.5;
		const gridSize = 80 * dpr;
		for (let gx = 0; gx < width; gx += gridSize) {
			ctx.beginPath();
			ctx.moveTo(gx, 0);
			ctx.lineTo(gx, height);
			ctx.stroke();
		}
		for (let gy = 0; gy < height; gy += gridSize) {
			ctx.beginPath();
			ctx.moveTo(0, gy);
			ctx.lineTo(width, gy);
			ctx.stroke();
		}
		ctx.restore();

		// Stars — more and brighter
		let seed = 42;
		const random = () => {
			seed = (seed * 1664525 + 1013904223) >>> 0;
			return seed / 4294967296;
		};

		const starCount = isCompactViewport ? 220 : 430;
		for (let index = 0; index < starCount; index += 1) {
			const x = random() * width;
			const y = random() * height;
			const size = 0.35 + random() * 2.45;
			const alpha = 0.24 + random() * 0.68;
			// Some stars are slightly warm or cool
			const tint = random() > 0.85 ? `rgba(200, 210, 255, ${alpha})` :
			             random() > 0.85 ? `rgba(255, 230, 200, ${alpha})` :
			             `rgba(255, 255, 255, ${alpha})`;
			ctx.fillStyle = tint;
			ctx.beginPath();
			ctx.arc(x, y, size, 0, Math.PI * 2);
			ctx.fill();
		}
	}

	function drawNebulaVeins(ctx: CanvasRenderingContext2D) {
		ctx.save();
		ctx.globalCompositeOperation = 'screen';
		for (let index = 0; index < 18; index += 1) {
			const t = index / 17;
			const startX = width * (-0.08 + t * 1.16);
			const startY = height * (0.14 + Math.sin(index * 1.7) * 0.08);
			const endX = width * (0.18 + t * 0.95);
			const endY = height * (0.92 + Math.cos(index * 1.13) * 0.12);
			const controlX = width * (0.5 + Math.sin(index * 0.93) * 0.32);
			const controlY = height * (0.42 + Math.cos(index * 1.27) * 0.25);
			const gradient = ctx.createLinearGradient(startX, startY, endX, endY);
			gradient.addColorStop(0, 'rgba(88, 166, 255, 0)');
			gradient.addColorStop(0.44, index % 2 === 0 ? 'rgba(128, 118, 255, 0.075)' : 'rgba(6, 214, 160, 0.052)');
			gradient.addColorStop(1, 'rgba(236, 180, 98, 0)');
			ctx.strokeStyle = gradient;
			ctx.lineWidth = 0.7 + (index % 4) * 0.18;
			ctx.beginPath();
			ctx.moveTo(startX, startY);
			ctx.quadraticCurveTo(controlX, controlY, endX, endY);
			ctx.stroke();
		}
		ctx.restore();
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
			const gradient = ctx.createLinearGradient(sourceScreen.x, sourceScreen.y, targetScreen.x, targetScreen.y);
			gradient.addColorStop(0, hexToRgba(source.color, clamp(0.38 + edge.weight * 0.4, 0.35, 0.92)));
			gradient.addColorStop(1, hexToRgba(target.color, clamp(0.28 + edge.weight * 0.36, 0.28, 0.84)));

			ctx.save();
			ctx.beginPath();
			ctx.globalAlpha = opacity;
			ctx.lineWidth = lineWidth;
			ctx.strokeStyle = gradient;
			ctx.shadowBlur = edge.type === 'parent-child' ? 10 * edge.weight * (0.4 + activity * 0.8) : 0;
			ctx.shadowColor = source.glowColor;
			ctx.setLineDash([]);
			ctx.moveTo(sourceScreen.x, sourceScreen.y);
			ctx.quadraticCurveTo(controlX, controlY, targetScreen.x, targetScreen.y);
			ctx.stroke();
			ctx.restore();
		}

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
		vignette.addColorStop(1, 'rgba(4, 6, 12, 0.46)');
		ctx.save();
		ctx.fillStyle = vignette;
		ctx.fillRect(0, 0, width, height);
		ctx.restore();
	}

	function nodeFillGradient(
		ctx: CanvasRenderingContext2D,
		node: GalaxyNode,
		screen: { x: number; y: number },
		radius: number,
		baseColor: string
	): CanvasGradient {
		const gradient = ctx.createRadialGradient(
			screen.x - radius * 0.34,
			screen.y - radius * 0.42,
			radius * 0.08,
			screen.x,
			screen.y,
			radius * 1.12
		);
		gradient.addColorStop(0, 'rgba(255, 255, 255, 0.42)');
		gradient.addColorStop(0.22, baseColor);
		gradient.addColorStop(0.72, hexToRgba(node.color, 0.82));
		gradient.addColorStop(1, 'rgba(3, 5, 12, 0.86)');
		return gradient;
	}

	function drawNodeTexture(ctx: CanvasRenderingContext2D, node: GalaxyNode, screen: { x: number; y: number }, radius: number, activity: number) {
		if (radius < 8) return;
		ctx.save();
		ctx.globalAlpha = clamp(activity * (node.depth === 0 ? 0.22 : 0.14), 0.08, 0.24);
		ctx.lineWidth = 1;
		ctx.strokeStyle = 'rgba(255, 255, 255, 0.72)';
		const ringCount = node.depth === 0 ? 3 : 2;
		for (let index = 1; index <= ringCount; index += 1) {
			ctx.beginPath();
			ctx.arc(screen.x, screen.y, radius * (index / (ringCount + 1)), 0, Math.PI * 2);
			ctx.stroke();
		}
		ctx.globalAlpha = clamp(activity * 0.12, 0.05, 0.16);
		ctx.beginPath();
		ctx.moveTo(screen.x - radius * 0.62, screen.y + radius * 0.16);
		ctx.quadraticCurveTo(screen.x, screen.y - radius * 0.5, screen.x + radius * 0.66, screen.y - radius * 0.02);
		ctx.stroke();
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

		// Pass 1: glow halos (tight, outer only — no save/restore)
		for (const node of visibleNodes) {
			const screen = worldToScreen(node.x, node.y);
			const radius = node.radius;
			const activity = nodeActivity(node);
			if (activity < 0.16) continue;

			// Vibe mode: use energy color for glow when DSP is available.
			// Unanalyzed nodes get NO halo so the data-coverage gap is honest.
			if (viewMode === 'vibe') {
				const eColor = energyColor(node.avgEnergy);
				if (eColor) {
					const glowR = danceGlowRadius(node.avgDanceability, radius);
					const gradient = ctx.createRadialGradient(screen.x, screen.y, radius * 0.5, screen.x, screen.y, glowR);
					gradient.addColorStop(0, eColor.replace('50%)', '60%)').replace('hsl', 'hsla').replace(')', ', 0.42)'));
					gradient.addColorStop(1, 'rgba(0, 0, 0, 0)');
					ctx.fillStyle = gradient;
					ctx.beginPath();
					ctx.arc(screen.x, screen.y, glowR, 0, Math.PI * 2);
					ctx.fill();
				}
				continue;
			}

			// Heat mode: cold nodes get tiny halos, hot nodes blaze.
			// Rediscover mode: candidates blaze, others fade.
			// Map mode: even baseline halo for every node.
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
			const haloExtend = 2 + radius * 0.4 + heatExtra;
			const haloRadius = radius + haloExtend;
			const gradient = ctx.createRadialGradient(screen.x, screen.y, radius * 0.5, screen.x, screen.y, haloRadius);
			gradient.addColorStop(0, hexToRgba(node.color, 0.25 * activity * haloAlphaScale));
			gradient.addColorStop(1, 'rgba(0, 0, 0, 0)');
			ctx.fillStyle = gradient;
			ctx.beginPath();
			ctx.arc(screen.x, screen.y, haloRadius, 0, Math.PI * 2);
			ctx.fill();
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
			ctx.fillStyle = nodeFillGradient(ctx, node, screen, radius, baseColor);
			ctx.beginPath();
			ctx.arc(screen.x, screen.y, radius, 0, Math.PI * 2);
			ctx.fill();
			drawNodeTexture(ctx, node, screen, radius, activity);
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
		drawFamilyFields(ctx);

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

		const tick = () => {
			advanceCamera();
			updateParticles();
			drawFrame();
			animationFrame = window.requestAnimationFrame(tick);
		};

		animationFrame = window.requestAnimationFrame(tick);

		return () => {
			window.cancelAnimationFrame(animationFrame);
			resizeObserver?.disconnect();
			window.removeEventListener('keydown', handleKeydown);
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
