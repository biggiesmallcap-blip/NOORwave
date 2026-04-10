<script lang="ts">
	import { onMount } from 'svelte';
	import { innerWidth } from 'svelte/reactivity/window';
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

	let {
		nodes = [],
		edges = [],
		selectedId = null,
		selectedSeedIds = [],
		focusNodeId = null,
		resetViewToken = 0,
		viewMode = 'map',
		labelsEnabled = true,
		heatEnabled = true,
		autoDrift = false,
		artistChipMap = new Map<number, string[]>(),
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
		heatEnabled?: boolean;
		autoDrift?: boolean;
		artistChipMap?: ArtistChipMap;
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
	let activeFamilyId = $state<number | null>(null);
	let zoomLevel = $state<ZoomLevel>('galaxy');
	let isDragging = $state(false);
	let mixPillPosition = $state<{ x: number; y: number } | null>(null);
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
	const fontBody = '600 12px "Avenir Next", "Segoe UI", sans-serif';
	const fontDisplay = '600 13px "Iowan Old Style", Georgia, serif';

	function clamp(value: number, min: number, max: number): number {
		return Math.min(max, Math.max(min, value));
	}

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

	function nodeActivity(node: GalaxyNode): number {
		const inSelectedLineage = selectedLineageHas(node.id);
		if (selectedId !== null) {
			const selectedNode = nodeById.get(selectedId);
			if (!selectedNode) return 0.6;
			if (viewMode === 'paths') {
				if (node.id === selectedNode.id) return 1;
				if (inSelectedLineage) return 0.96;
				if (node.familyId === selectedNode.familyId) return 0.42;
				return 0.14;
			}
			if (node.id === selectedNode.id) return 1;
			if (node.familyId === selectedNode.familyId) return 0.76;
			return 0.28;
		}

		if (activeFamilyId !== null) {
			return node.familyId === activeFamilyId ? 0.94 : 0.44;
		}

		return 0.86;
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
		const parentEdges = nextEdges
			.map((edge, index) => ({ edge, index }))
			.filter(({ edge }) => edge.type === 'parent-child')
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

		const fill = ctx.createRadialGradient(width * 0.5, height * 0.5, 40, width * 0.5, height * 0.5, width * 0.78);
		fill.addColorStop(0, 'rgba(17, 19, 34, 0.98)');
		fill.addColorStop(0.55, 'rgba(10, 12, 22, 0.98)');
		fill.addColorStop(1, 'rgba(6, 8, 15, 1)');
		ctx.fillStyle = fill;
		ctx.fillRect(0, 0, width, height);

		const nebulaA = ctx.createRadialGradient(width * 0.22, height * 0.28, 0, width * 0.22, height * 0.28, width * 0.32);
		nebulaA.addColorStop(0, 'rgba(124, 128, 255, 0.16)');
		nebulaA.addColorStop(1, 'rgba(124, 128, 255, 0)');
		ctx.fillStyle = nebulaA;
		ctx.fillRect(0, 0, width, height);

		const nebulaB = ctx.createRadialGradient(width * 0.76, height * 0.18, 0, width * 0.76, height * 0.18, width * 0.26);
		nebulaB.addColorStop(0, 'rgba(179, 123, 244, 0.12)');
		nebulaB.addColorStop(1, 'rgba(247, 37, 133, 0)');
		ctx.fillStyle = nebulaB;
		ctx.fillRect(0, 0, width, height);

		const nebulaC = ctx.createRadialGradient(width * 0.72, height * 0.8, 0, width * 0.72, height * 0.8, width * 0.34);
		nebulaC.addColorStop(0, 'rgba(6, 214, 160, 0.08)');
		nebulaC.addColorStop(1, 'rgba(6, 214, 160, 0)');
		ctx.fillStyle = nebulaC;
		ctx.fillRect(0, 0, width, height);

		let seed = 42;
		const random = () => {
			seed = (seed * 1664525 + 1013904223) >>> 0;
			return seed / 4294967296;
		};

		const starCount = isCompactViewport ? 120 : 200;
		for (let index = 0; index < starCount; index += 1) {
			const x = random() * width;
			const y = random() * height;
			const size = 0.4 + random() * 1.9;
			const alpha = 0.2 + random() * 0.65;
			ctx.beginPath();
			ctx.fillStyle = `rgba(255, 255, 255, ${alpha})`;
			ctx.arc(x, y, size, 0, Math.PI * 2);
			ctx.fill();
		}
	}

	function drawConnectionsLayer() {
		if (!connCanvas || width === 0 || height === 0) return;
		const ctx = connCanvas.getContext('2d');
		if (!ctx) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);

		for (const edge of edges) {
			const source = nodeById.get(edge.sourceId);
			const target = nodeById.get(edge.targetId);
			if (!source || !target) continue;

			const sourceScreen = worldToScreen(source.x, source.y);
			const targetScreen = worldToScreen(target.x, target.y);
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

			// Co-listening edges: dashed, lower opacity, silver-tinted
			const isCoListening = edge.type === 'co-listening';
			const baseOpacity = isCoListening
				? 0.06 + edge.weight * 0.18
				: edge.type === 'parent-child'
					? 0.18 + edge.weight * 0.22
					: 0.06 + edge.weight * 0.1;
			const opacity = baseOpacity * activity * emphasis;
			const baseWidth = isCoListening
				? 0.5 + edge.weight * 1.2
				: edge.type === 'parent-child'
					? 0.8 + edge.weight * 1.5
					: 0.6 + edge.weight * 0.9;
			const lineWidth = baseWidth * (0.72 + activity * 0.42) * emphasis;
			const dx = targetScreen.x - sourceScreen.x;
			const dy = targetScreen.y - sourceScreen.y;
			const distance = Math.max(1, Math.hypot(dx, dy));
			const normalX = -dy / distance;
			const normalY = dx / distance;
			const curve =
				edge.type === 'sibling'
					? Math.min(44, distance * 0.1)
					: isCoListening
						? Math.min(60, distance * 0.14) // more arc for co-listening bridges
						: Math.min(26, distance * 0.06);
			const curveSign = ((edge.sourceId + edge.targetId) & 1) === 0 ? 1 : -1;
			const controlX = (sourceScreen.x + targetScreen.x) * 0.5 + normalX * curve * curveSign;
			const controlY = (sourceScreen.y + targetScreen.y) * 0.5 + normalY * curve * curveSign;

			const gradient = ctx.createLinearGradient(sourceScreen.x, sourceScreen.y, targetScreen.x, targetScreen.y);
			if (isCoListening) {
				// Silver/white bridges for co-listening
				const bridgeAlpha = clamp(0.3 + edge.weight * 0.5, 0.3, 0.7);
				gradient.addColorStop(0, `rgba(200, 210, 240, ${bridgeAlpha * 0.7})`);
				gradient.addColorStop(0.5, `rgba(220, 225, 255, ${bridgeAlpha})`);
				gradient.addColorStop(1, `rgba(200, 210, 240, ${bridgeAlpha * 0.7})`);
			} else {
				gradient.addColorStop(0, hexToRgba(source.color, clamp(0.38 + edge.weight * 0.4, 0.35, 0.92)));
				gradient.addColorStop(1, hexToRgba(target.color, clamp(0.28 + edge.weight * 0.36, 0.28, 0.84)));
			}

			ctx.save();
			ctx.beginPath();
			ctx.globalAlpha = opacity;
			ctx.lineWidth = lineWidth;
			ctx.strokeStyle = gradient;
			if (isCoListening) {
				ctx.setLineDash([4, 6]);
			}
			ctx.shadowBlur = isCoListening ? 6 * edge.weight * activity : edge.type === 'parent-child' ? 10 * edge.weight * (0.4 + activity * 0.8) : 0;
			ctx.shadowColor = isCoListening ? 'rgba(180, 190, 240, 0.3)' : source.glowColor;
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
			ctx.save();
			const heatFactor = heatEnabled || viewMode === 'heat' ? 0.72 + edge.weight * 0.8 : 0.54;
			ctx.globalAlpha = particle.alpha * edgeFade * activity * heatFactor * (isSelectedEdge ? 1.15 : 1);
			ctx.fillStyle = source.color;
			ctx.shadowBlur = 12;
			ctx.shadowColor = source.glowColor;
			ctx.beginPath();
			ctx.arc(x, y, particle.size * (0.85 + activity * 0.25), 0, Math.PI * 2);
			ctx.fill();
			ctx.restore();
		}
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
			const heatBoost = heatEnabled || viewMode === 'heat' ? 0.75 + node.heatNorm * 1.35 : 0.78;
			const warmTint = viewMode === 'heat' && node.heatNorm > 0.45 ? 'rgba(255, 168, 98, 0.12)' : null;
			glow.addColorStop(0, hexToRgba(node.color, 0.14 * activity * heatBoost));
			glow.addColorStop(0.42, hexToRgba(node.color, 0.07 * activity * heatBoost));
			glow.addColorStop(1, hexToRgba(node.color, 0));
			ctx.save();
			ctx.globalAlpha = clamp(0.5 + node.heatNorm * 0.5, 0.45, 1);
			ctx.fillStyle = glow;
			ctx.beginPath();
			ctx.arc(screen.x, screen.y, fieldRadius, 0, Math.PI * 2);
			ctx.fill();
			if (warmTint) {
				ctx.fillStyle = warmTint;
				ctx.fill();
			}
			ctx.restore();
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

	function drawOrbitGuides(ctx: CanvasRenderingContext2D) {
		if (isCompactViewport) return;
		if (viewMode !== 'constellations' && viewMode !== 'mood') return;
		const roots = nodes.filter((node) => node.depth === 0);
		for (const root of roots) {
			if (!nodeIsVisible(root)) continue;
			const activity = nodeActivity(root);
			const screen = worldToScreen(root.x, root.y);
			const orbitRadii = [72, 118, 162];
			ctx.save();
			ctx.globalAlpha = 0.12 * activity;
			ctx.setLineDash(viewMode === 'mood' ? [4, 12] : [2, 9]);
			ctx.lineWidth = 1;
			ctx.strokeStyle = hexToRgba(root.color, 0.46);
			for (const orbitRadius of orbitRadii) {
				ctx.beginPath();
				ctx.arc(screen.x, screen.y, orbitRadius * camera.scale, 0, Math.PI * 2);
				ctx.stroke();
			}
			ctx.restore();
		}
	}

	function drawMoodOverlay(ctx: CanvasRenderingContext2D) {
		if (viewMode !== 'mood') return;
		const gradient = ctx.createLinearGradient(0, height, width, 0);
		gradient.addColorStop(0, 'rgba(255, 145, 84, 0.08)');
		gradient.addColorStop(0.5, 'rgba(111, 136, 255, 0.04)');
		gradient.addColorStop(1, 'rgba(129, 255, 226, 0.08)');
		ctx.save();
		ctx.fillStyle = gradient;
		ctx.fillRect(0, 0, width, height);
		ctx.restore();
	}

	function labelAlphaForNode(node: GalaxyNode): number {
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
		if (camera.scale < 0.8) return node.depth === 0 ? 0.92 : 0;
		if (camera.scale < 2) {
			if (node.depth > 1) return 0;
			return node.depth === 0 ? 0.94 : clamp((camera.scale - 0.8) / 1.2, 0.15, 0.88);
		}
		return clamp(0.6 + node.heatNorm * 0.25, 0.6, 0.95);
	}

	function drawNodesAndLabels(ctx: CanvasRenderingContext2D) {
		const visibleNodes = nodes.filter(nodeIsVisible);
		const pulse = 0.65 + Math.sin(Date.now() / 480) * 0.35;

		for (const node of visibleNodes) {
			const screen = worldToScreen(node.x, node.y);
			const radius = node.radius;
			const activity = nodeActivity(node);
			if (activity < 0.16) continue;
			const nodeHeat = heatEnabled || viewMode === 'heat' ? node.heatNorm : 0;

			ctx.save();
			ctx.globalAlpha = activity;
			const gradient = ctx.createRadialGradient(screen.x, screen.y, 0, screen.x, screen.y, radius * 2.4);
			gradient.addColorStop(0, node.color);
			gradient.addColorStop(0.42, `${node.color}b3`);
			gradient.addColorStop(1, 'rgba(0, 0, 0, 0)');
			ctx.fillStyle = gradient;
			ctx.shadowBlur = radius * (1.8 + nodeHeat * 4.4) * (0.84 + activity * 0.42);
			ctx.shadowColor = node.glowColor;
			ctx.beginPath();
			ctx.arc(screen.x, screen.y, radius, 0, Math.PI * 2);
			ctx.fill();
			ctx.restore();

			if (node.depth === 0) {
				ctx.save();
				ctx.globalAlpha = clamp(0.18 + activity * 0.18, 0.16, 0.38);
				ctx.lineWidth = 1.2;
				ctx.strokeStyle = hexToRgba(node.color, 0.55);
				ctx.beginPath();
				ctx.arc(screen.x, screen.y, radius + 10 + nodeHeat * 8, 0, Math.PI * 2);
				ctx.stroke();
				ctx.restore();
			}

			if (nodeIsSeed(node.id)) {
				ctx.save();
				ctx.globalAlpha = 0.76 * activity;
				ctx.setLineDash([3, 6]);
				ctx.lineWidth = 1.6;
				ctx.strokeStyle = 'rgba(255, 245, 220, 0.85)';
				ctx.beginPath();
				ctx.arc(screen.x, screen.y, radius + 8, 0, Math.PI * 2);
				ctx.stroke();
				ctx.restore();
			}

			if (selectedId === node.id) {
				ctx.save();
				ctx.beginPath();
				ctx.lineWidth = 2.2;
				ctx.strokeStyle = 'rgba(255, 255, 255, 0.92)';
				ctx.shadowBlur = 18 * pulse;
				ctx.shadowColor = node.glowColor;
				ctx.arc(screen.x, screen.y, radius + 4, 0, Math.PI * 2);
				ctx.stroke();
				ctx.restore();
			} else if (hoveredNodeId === node.id && !isDragging) {
				ctx.save();
				ctx.beginPath();
				ctx.lineWidth = 1.6;
				ctx.strokeStyle = node.color;
				ctx.arc(screen.x, screen.y, radius + 3, 0, Math.PI * 2);
				ctx.stroke();
				ctx.restore();
			}
		}

		for (const node of visibleNodes) {
			const alpha = labelAlphaForNode(node);
			if (alpha <= 0) continue;
			const activity = nodeActivity(node);
			if (activity < 0.22) continue;

			const screen = worldToScreen(node.x, node.y);
			const fontSize = node.depth === 0 ? 13 : node.depth === 1 ? 11.5 : 10;
			const label = node.depth === 0 ? node.name.toUpperCase() : node.name;
			ctx.save();
			ctx.globalAlpha = alpha * activity;
			ctx.font = node.depth === 0 ? fontDisplay : fontBody.replace('12px', `${fontSize}px`);
			ctx.textAlign = 'center';
			ctx.textBaseline = 'top';

			if (node.depth <= 1) {
				const textWidth = ctx.measureText(label).width;
				const chipWidth = textWidth + (node.depth === 0 ? 18 : 14);
				const chipHeight = node.depth === 0 ? 22 : 19;
				const chipX = screen.x - chipWidth / 2;
				const chipY = screen.y + node.radius + 8;
				roundedRectPath(ctx, chipX, chipY, chipWidth, chipHeight, 10);
				ctx.fillStyle = 'rgba(8, 10, 18, 0.74)';
				ctx.fill();
				ctx.lineWidth = 1;
				ctx.strokeStyle = hexToRgba(node.color, node.depth === 0 ? 0.4 : 0.28);
				ctx.stroke();
				ctx.textBaseline = 'middle';
				ctx.fillStyle = 'rgba(244, 247, 255, 0.95)';
				ctx.shadowBlur = 8;
				ctx.shadowColor = 'rgba(0, 0, 0, 0.25)';
				ctx.fillText(label, screen.x, chipY + chipHeight / 2);

				// Cohort indicator dot below the label chip
				if (node.cohortId) {
					const dotY = chipY + chipHeight + 6;
					const dotRadius = 3;
					ctx.globalAlpha = alpha * activity * 0.7;
					ctx.beginPath();
					ctx.arc(screen.x, dotY, dotRadius, 0, Math.PI * 2);
					ctx.fillStyle = 'rgba(255, 220, 160, 0.85)';
					ctx.shadowBlur = 6;
					ctx.shadowColor = 'rgba(255, 200, 120, 0.4)';
					ctx.fill();
				}
			} else {
				ctx.fillStyle = 'rgba(255, 255, 255, 0.9)';
				ctx.shadowBlur = 10;
				ctx.shadowColor = 'rgba(0, 0, 0, 0.35)';
				ctx.fillText(label, screen.x, screen.y + node.radius + 8);
			}
			ctx.restore();
		}
	}

	function drawArtistChips(ctx: CanvasRenderingContext2D) {
		if (isCompactViewport || activeFamilyId === null || camera.scale < 1.25) return;

		const familyNodes = nodes.filter(
			(node) => node.familyId === activeFamilyId && node.depth === 1 && artistChipMap.has(node.id)
		);

		for (const node of familyNodes) {
			if (!nodeIsVisible(node)) continue;
			const artists = artistChipMap.get(node.id) ?? [];
			if (artists.length === 0) continue;
			const screen = worldToScreen(node.x, node.y);
			const chipX = screen.x + node.radius + 18;
			const chipWidth = 142;
			const chipHeight = 22;
			const totalHeight = artists.length * chipHeight + (artists.length - 1) * 6;
			const startY = screen.y - totalHeight / 2;

			artists.slice(0, 3).forEach((artist, index) => {
				const chipY = startY + index * (chipHeight + 6);
				ctx.save();
				ctx.globalAlpha = 0.92;
				roundedRectPath(ctx, chipX, chipY, chipWidth, chipHeight, 11);
				ctx.fillStyle = 'rgba(8, 9, 18, 0.82)';
				ctx.fill();
				ctx.lineWidth = 1;
				ctx.strokeStyle = 'rgba(255, 255, 255, 0.08)';
				ctx.stroke();
				ctx.font = '600 10px "Avenir Next", "Segoe UI", sans-serif';
				ctx.fillStyle = 'rgba(255, 255, 255, 0.9)';
				ctx.textAlign = 'left';
				ctx.textBaseline = 'middle';
				ctx.fillText(artist, chipX + 10, chipY + chipHeight / 2);
				ctx.restore();
			});
		}
	}

	function drawFrame() {
		if (!canvasEl || width === 0 || height === 0) return;
		const ctx = canvasEl.getContext('2d');
		if (!ctx || !bgCanvas || !connCanvas) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);
		ctx.drawImage(bgCanvas, 0, 0, width, height);
		drawMoodOverlay(ctx);
		drawFamilyFields(ctx);
		drawOrbitGuides(ctx);

		if (pendingConnectionRedraw) {
			drawConnectionsLayer();
		}
		ctx.drawImage(connCanvas, 0, 0, width, height);

		drawParticles(ctx);
		drawNodesAndLabels(ctx);
		drawArtistChips(ctx);
		drawVignette(ctx);

		if (hoveredNode && !isDragging) {
			const screen = worldToScreen(hoveredNode.x, hoveredNode.y);
			mixPillPosition = { x: screen.x, y: screen.y - hoveredNode.radius - 18 };
		} else {
			mixPillPosition = null;
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
		}
	}

	function handleWheel(event: WheelEvent) {
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
		dpr = window.devicePixelRatio || 1;

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
</div>

<style>
	.galaxy-wrap {
		position: absolute;
		inset: 0;
		overflow: hidden;
		border-radius: 24px;
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
		backdrop-filter: blur(12px);
		-webkit-backdrop-filter: blur(12px);
		font-size: 0.75rem;
		font-weight: 700;
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
</style>
