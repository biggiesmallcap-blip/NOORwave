<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { discoverSpaceStore } from './discover_space_store';
	import { applyForces, findNodeNear } from './discover_space_physics';
	import {
		drawBackground,
		drawVisitedRegions,
		drawGenreNebulae,
		drawEdges,
		drawNodes,
		drawSeedNode,
		drawPlayingNode,
		drawLabels,
		drawRadioRoute,
		drawWarpStreaks,
	} from './discover_space_renderer';
	import type { DiscoverTrackNode, Camera } from './discover_space_types';
	import { addVisitedRegion } from './discover_space_store';

	interface Props {
		currentTrackId: number | null;
		seedTrackId: number | null;
		isLocked: boolean;
		onHoverNode?: (node: DiscoverTrackNode | null, x: number, y: number) => void;
		onSelectNode?: (node: DiscoverTrackNode) => void;
		onNewNodes?: (nodes: DiscoverTrackNode[]) => void;
	}
	let {
		currentTrackId,
		seedTrackId,
		isLocked,
		onHoverNode,
		onSelectNode,
	}: Props = $props();

	// Debug overlay (dev + query param)
	const debugEnabled = import.meta.env.DEV &&
		typeof window !== 'undefined' &&
		new URLSearchParams(window.location.search).has('debug_discoverspace');

	let canvas: HTMLCanvasElement | undefined = $state();
	let rafId = 0;
	let tick = 0;

	// Camera
	let camera: Camera = { x: 0, y: 0, zoom: 0.7 };
	const ZOOM_MIN = 0.25;
	const ZOOM_MAX = 3.0;

	// Interaction state
	let isPanning = false;
	let panStart = { x: 0, y: 0, cx: 0, cy: 0 };
	let hoveredNode: DiscoverTrackNode | null = $state(null);
	let selectedNode: DiscoverTrackNode | null = $state(null);

	// Warp animation state
	let warpProgress = 0;
	let isWarping = false;

	// Reduced motion
	const prefersReducedMotion = typeof window !== 'undefined' &&
		window.matchMedia('(prefers-reduced-motion: reduce)').matches;

	// FPS tracking (debug)
	let fpsFrames: number[] = [];
	let fps = $state(60);

	// ── Hyperspace search API (exposed to parent via window) ──────────────────
	export function hyperspaceSearch(query: string): Promise<void> {
		return runHyperspaceSearch(query);
	}

	async function runHyperspaceSearch(query: string): Promise<void> {
		if (isWarping) return;

		// Save outgoing viewport as visited region BEFORE zoom-out
		const label = seedTrackId ? `seed:${seedTrackId}` : 'home';
		addVisitedRegion(label, { x: camera.x, y: camera.y, radius: 300 / camera.zoom });

		if (!prefersReducedMotion) {
			// T+0: dim existing nodes
			isWarping = true;

			// T+100: zoom out
			await wait(100);
			animateCamera({ zoom: camera.zoom * 0.4 }, 300);

			// T+300: warp streaks
			await wait(300);
			warpProgress = 0;
			const warpStart = performance.now();
			await new Promise<void>((res) => {
				function warpTick() {
					const elapsed = performance.now() - warpStart;
					warpProgress = Math.min(1, elapsed / 400);
					if (warpProgress < 1) requestAnimationFrame(warpTick);
					else res();
				}
				requestAnimationFrame(warpTick);
			});

			// T+700: results arrive (fetched by parent via store)
			await wait(100);
			warpProgress = 0;
			isWarping = false;

			// Snap camera to new centroid
			const nodes = $discoverSpaceStore.nodes;
			if (nodes.length > 0) {
				const cx = nodes.reduce((s, n) => s + n.x, 0) / nodes.length;
				const cy = nodes.reduce((s, n) => s + n.y, 0) / nodes.length;
				animateCamera({ x: cx, y: cy, zoom: 0.7 }, 400);
			}
		}
	}

	function wait(ms: number): Promise<void> {
		return new Promise((res) => setTimeout(res, ms));
	}

	function animateCamera(target: Partial<Camera>, durationMs: number): Promise<void> {
		const start = { ...camera };
		const startTime = performance.now();
		return new Promise((res) => {
			function step() {
				const t = Math.min(1, (performance.now() - startTime) / durationMs);
				const ease = t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t;
				if (target.x != null) camera.x = start.x + (target.x - start.x) * ease;
				if (target.y != null) camera.y = start.y + (target.y - start.y) * ease;
				if (target.zoom != null) camera.zoom = start.zoom + (target.zoom - start.zoom) * ease;
				if (t < 1) requestAnimationFrame(step);
				else res();
			}
			requestAnimationFrame(step);
		});
	}

	// ── Main RAF loop ─────────────────────────────────────────────────────────

	function loop() {
		rafId = requestAnimationFrame(loop);
		if (!canvas) return;

		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const w = canvas.width;
		const h = canvas.height;

		const state = $discoverSpaceStore;
		const nodes = state.nodes;
		const edges = state.edges;
		const lens = state.lens;
		const route = state.radioRoute;
		const regions = state.visitedRegions;

		// Physics tick (skip when warping for visual consistency)
		if (!isWarping) {
			applyForces(nodes, edges, tick, {
				genreLensActive: lens === 'genre',
				prefersReducedMotion,
			});
			tick++;
		}

		// Build node map for O(1) lookup
		const nodeMap = new Map(nodes.map((n) => [n.trackId, n]));
		const seedNode = seedTrackId != null ? nodeMap.get(seedTrackId) : null;
		const playingNode = currentTrackId != null ? nodeMap.get(currentTrackId) : null;

		// Build route track ID set for edge filtering
		const routeTrackIds = new Set(route.map((s) => s.trackId));
		const hoveredId  = hoveredNode?.trackId  ?? null;
		const selectedId = selectedNode?.trackId ?? null;

		// ── Draw ──────────────────────────────────────────────────────────────
		drawBackground(ctx, w, h, prefersReducedMotion);
		drawVisitedRegions(ctx, regions, camera, w, h);
		drawGenreNebulae(ctx, nodes, camera, w, h, lens);
		drawEdges(ctx, edges, nodeMap, camera, w, h, camera.zoom, lens, seedTrackId, hoveredId, selectedId, routeTrackIds);
		drawNodes(ctx, nodes, camera, w, h, lens, hoveredId, selectedId, tick, prefersReducedMotion);

		if (playingNode && !playingNode.isSeed) {
			drawPlayingNode(ctx, playingNode, camera, w, h, tick, prefersReducedMotion);
		}
		if (seedNode) {
			drawSeedNode(ctx, seedNode, camera, w, h, isLocked, tick, prefersReducedMotion);
		}

		drawLabels(ctx, nodes, camera, w, h, hoveredId, selectedId, camera.zoom);
		drawRadioRoute(ctx, route, nodeMap, camera, w, h, camera.zoom, tick, prefersReducedMotion);

		if (!prefersReducedMotion) {
			drawWarpStreaks(ctx, warpProgress, w, h);
		}

		// FPS tracking
		if (debugEnabled) {
			const now = performance.now();
			fpsFrames.push(now);
			fpsFrames = fpsFrames.filter((t) => now - t < 1000);
			fps = fpsFrames.length;
		}
	}

	// ── Pointer events ────────────────────────────────────────────────────────

	function canvasToWorld(ex: number, ey: number): { x: number; y: number } {
		const rect = canvas!.getBoundingClientRect();
		const px = (ex - rect.left) * (canvas!.width / rect.width);
		const py = (ey - rect.top) * (canvas!.height / rect.height);
		return {
			x: (px - canvas!.width / 2) / camera.zoom + camera.x,
			y: (py - canvas!.height / 2) / camera.zoom + camera.y,
		};
	}

	function onPointerDown(e: PointerEvent) {
		if (e.button !== 0) return;
		isPanning = true;
		panStart = { x: e.clientX, y: e.clientY, cx: camera.x, cy: camera.y };
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
	}

	function onPointerMove(e: PointerEvent) {
		if (!canvas) return;
		const world = canvasToWorld(e.clientX, e.clientY);
		const hit = findNodeNear($discoverSpaceStore.nodes, world.x, world.y);
		if (hit !== hoveredNode) {
			hoveredNode = hit;
			canvas.style.cursor = hit ? 'pointer' : 'grab';
			const rect = canvas.getBoundingClientRect();
			onHoverNode?.(hit, e.clientX - rect.left, e.clientY - rect.top);
		}

		if (!isPanning) return;
		const dx = (e.clientX - panStart.x) / camera.zoom;
		const dy = (e.clientY - panStart.y) / camera.zoom;
		camera.x = panStart.cx - dx;
		camera.y = panStart.cy - dy;
	}

	function onPointerUp(e: PointerEvent) {
		if (isPanning && Math.abs(e.clientX - panStart.x) < 4 && Math.abs(e.clientY - panStart.y) < 4) {
			// It was a click, not a drag
			const world = canvasToWorld(e.clientX, e.clientY);
			const hit = findNodeNear($discoverSpaceStore.nodes, world.x, world.y);
			if (hit) {
				selectedNode = hit;
				onSelectNode?.(hit);
			} else {
				selectedNode = null;
			}
		}
		isPanning = false;
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		const factor = e.deltaY < 0 ? 1.1 : 0.9;
		camera.zoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, camera.zoom * factor));
	}

	// ── Resize observer ───────────────────────────────────────────────────────

	let resizeObserver: ResizeObserver | null = null;

	onMount(() => {
		if (!canvas) return;

		const dpr = window.devicePixelRatio || 1;
		resizeObserver = new ResizeObserver((entries) => {
			const entry = entries[0];
			if (!entry || !canvas) return;
			const { width, height } = entry.contentRect;
			canvas.width = width * dpr;
			canvas.height = height * dpr;
			const ctx = canvas.getContext('2d');
			if (ctx) ctx.scale(dpr, dpr);
		});
		resizeObserver.observe(canvas.parentElement!);

		// Initial size
		const rect = canvas.parentElement!.getBoundingClientRect();
		canvas.width = rect.width * dpr;
		canvas.height = rect.height * dpr;
		const ctx = canvas.getContext('2d');
		if (ctx) ctx.scale(dpr, dpr);

		// Expose hyperspace search to page via window (same pattern as existing /discover)
		(window as any).__discoverSpaceHyperspaceSearch = runHyperspaceSearch;

		loop();
	});

	onDestroy(() => {
		cancelAnimationFrame(rafId);
		resizeObserver?.disconnect();
		delete (window as any).__discoverSpaceHyperspaceSearch;
	});
</script>

<canvas
	bind:this={canvas}
	class="discover-canvas"
	onpointerdown={onPointerDown}
	onpointermove={onPointerMove}
	onpointerup={onPointerUp}
	onwheel={onWheel}
></canvas>

{#if debugEnabled}
	<div class="debug-overlay" aria-hidden="true">
		<span>Nodes: {$discoverSpaceStore.nodes.length}</span>
		<span>Edges: {$discoverSpaceStore.edges.length}</span>
		<span>FPS: {fps}</span>
		<span>Zoom: {camera.zoom.toFixed(2)}</span>
		<span>Lens: {$discoverSpaceStore.lens}</span>
		{#if hoveredNode}<span>Hovered: {hoveredNode.trackId}</span>{/if}
		{#if selectedNode}<span>Selected: {selectedNode.trackId}</span>{/if}
		<span>Route: {$discoverSpaceStore.radioRoute.length} steps</span>
		{#if $discoverSpaceStore.lastDiagnostics}
			<span>Avg conf: {$discoverSpaceStore.lastDiagnostics.avg_confidence.toFixed(2)}</span>
			<span>Pruned: {$discoverSpaceStore.lastDiagnostics.pruned_node_count} nodes</span>
		{/if}
	</div>
{/if}

<style>
	.discover-canvas {
		width: 100%;
		height: 100%;
		display: block;
		cursor: grab;
		touch-action: none;
	}
	.discover-canvas:active {
		cursor: grabbing;
	}

	.debug-overlay {
		position: absolute;
		top: 8px;
		right: 8px;
		background: rgba(0, 0, 0, 0.7);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 6px;
		padding: 8px 12px;
		display: flex;
		flex-direction: column;
		gap: 2px;
		font-size: 0.72rem;
		font-family: monospace;
		color: rgba(200, 200, 220, 0.8);
		pointer-events: none;
		z-index: 100;
	}
</style>
