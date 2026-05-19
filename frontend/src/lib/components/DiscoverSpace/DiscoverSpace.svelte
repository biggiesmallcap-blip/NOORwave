<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { discoverSpaceStore } from './discover_space_store';
	import { applyForces, kineticEnergy, findNodeNear } from './discover_space_physics';
	import {
		worldToCanvas,
		drawBackground,
		drawOrbitRings,
		drawVisitedRegions,
		drawGenreNebulae,
		drawEdges,
		drawNodes,
		drawSeedNode,
		drawPlayingNode,
		drawLabels,
		drawRadioRoute,
		drawSelectionRipple,
		drawWarpStreaks,
		invalidateNebulaCache,
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
	let tick = 0;         // animation clock (drives halo breathing, playing pulse)
	// Tracked separately from the main RAF so warp / camera animations get
	// cancelled on unmount instead of running their closures past teardown.
	let auxRafId = 0;
	let alive = true;

	// Camera
	let camera: Camera = { x: 0, y: 0, zoom: 0.7 };
	const ZOOM_MIN = 0.25;
	const ZOOM_MAX = 3.0;

	// Interaction state
	let isPanning = false;
	let panStart = { x: 0, y: 0, cx: 0, cy: 0 };
	let hoveredNode: DiscoverTrackNode | null = $state(null);
	let selectedNode: DiscoverTrackNode | null = $state(null);

	// Physics settling — stops mutating node positions once kinetic energy drops
	let simulationSettled = false;
	let lastNodeCount = 0;

	// Selection ripple (one-shot ring on node click)
	let rippleNode: DiscoverTrackNode | null = null;
	let rippleStartTick = 0;
	const RIPPLE_TICKS = 45; // ≈ 750ms at 60fps

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
					if (!alive) { res(); return; }
					const elapsed = performance.now() - warpStart;
					warpProgress = Math.min(1, elapsed / 400);
					if (warpProgress < 1) auxRafId = requestAnimationFrame(warpTick);
					else res();
				}
				auxRafId = requestAnimationFrame(warpTick);
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
				if (!alive) { res(); return; }
				const t = Math.min(1, (performance.now() - startTime) / durationMs);
				const ease = t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t;
				if (target.x != null) camera.x = start.x + (target.x - start.x) * ease;
				if (target.y != null) camera.y = start.y + (target.y - start.y) * ease;
				if (target.zoom != null) camera.zoom = start.zoom + (target.zoom - start.zoom) * ease;
				if (t < 1) auxRafId = requestAnimationFrame(step);
				else res();
			}
			auxRafId = requestAnimationFrame(step);
		});
	}

	// ── Main RAF loop ─────────────────────────────────────────────────────────

	function loop() {
		rafId = requestAnimationFrame(loop);
		if (!canvas) return;

		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		// Use CSS pixel dimensions — ctx.scale(dpr, dpr) makes the drawing space CSS pixels,
		// so w/h must be CSS sizes (not physical/DPR-scaled) for worldToCanvas to be correct.
		const dpr = window.devicePixelRatio || 1;
		const w = canvas.width / dpr;
		const h = canvas.height / dpr;

		const state = $discoverSpaceStore;
		const nodes = state.nodes;
		const edges = state.edges;
		const lens = state.lens;
		const route = state.radioRoute;
		const regions = state.visitedRegions;

		// Restart physics when node set changes (new load, search, route nodes)
		if (nodes.length !== lastNodeCount) {
			lastNodeCount = nodes.length;
			simulationSettled = false;
		}

		// Physics: run until kinetic energy drops below threshold, then stop
		if (!isWarping) {
			if (!simulationSettled) {
				applyForces(nodes, edges, { genreLensActive: lens === 'genre', prefersReducedMotion });
				// Centroids are cached by node-set identity; positions move while
				// settling so we drop the cache each tick. Once settled, the
				// cache stays valid for free.
				invalidateNebulaCache();
				if (prefersReducedMotion || kineticEnergy(nodes) < 0.002) {
					simulationSettled = true;
					for (const n of nodes) { n.vx = 0; n.vy = 0; }
				}
			}
			tick++; // animation clock always advances for halo/pulse animations
		}

		// Build lookups
		const nodeMap  = new Map(nodes.map((n) => [n.trackId, n]));
		const seedNode = seedTrackId != null ? nodeMap.get(seedTrackId) : null;
		const seedNodes = nodes.filter((n) => n.isSeed);
		const playingNode = currentTrackId != null ? nodeMap.get(currentTrackId) : null;

		const routeTrackIds = new Set(route.map((s) => s.trackId));
		const hoveredId  = hoveredNode?.trackId  ?? null;
		const selectedId = selectedNode?.trackId ?? null;

		// Connected-to-hovered set for focus-opacity dimming
		const connectedIds = new Set<number>();
		if (hoveredId !== null) {
			for (const edge of edges) {
				if (edge.fromTrackId === hoveredId) connectedIds.add(edge.toTrackId);
				if (edge.toTrackId   === hoveredId) connectedIds.add(edge.fromTrackId);
			}
		}

		// ── Draw ──────────────────────────────────────────────────────────────
		drawBackground(ctx, w, h, prefersReducedMotion);
		drawVisitedRegions(ctx, regions, camera, w, h);
		// Orbit rings behind everything — visual guide for distance tiers
		if (seedNode || seedNodes.length > 0) drawOrbitRings(ctx, camera, w, h);
		drawGenreNebulae(ctx, nodes, camera, w, h, lens);
		drawEdges(ctx, edges, nodeMap, camera, w, h, camera.zoom, seedTrackId, hoveredId, selectedId, routeTrackIds);
		drawNodes(ctx, nodes, camera, w, h, lens, hoveredId, selectedId, connectedIds, tick, prefersReducedMotion, routeTrackIds);

		if (playingNode && !playingNode.isSeed) {
			drawPlayingNode(ctx, playingNode, camera, w, h, tick, prefersReducedMotion);
		}
		if (seedNode) {
			drawSeedNode(ctx, seedNode, camera, w, h, isLocked, tick, prefersReducedMotion);
		}
		for (const blendSeed of seedNodes) {
			if (seedNode && blendSeed.trackId === seedNode.trackId) continue;
			drawSeedNode(ctx, blendSeed, camera, w, h, isLocked, tick, prefersReducedMotion);
		}

		// Selection ripple (one-shot, 750ms)
		if (rippleNode) {
			const progress = (tick - rippleStartTick) / RIPPLE_TICKS;
			if (progress < 1) {
				const [rx, ry] = worldToCanvas(rippleNode.x, rippleNode.y, camera, w, h);
				drawSelectionRipple(ctx, rx, ry, Math.max(3, rippleNode.radius * camera.zoom), progress);
			} else {
				rippleNode = null;
			}
		}

		drawLabels(ctx, nodes, camera, w, h, hoveredId, selectedId, camera.zoom);
		drawRadioRoute(ctx, route, nodeMap, camera, w, h, camera.zoom);

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
		// Use CSS pixel coordinates throughout — canvas.width is DPR-scaled physical pixels,
		// but ctx.scale(dpr, dpr) means drawing happens in CSS-pixel space.
		const dpr = window.devicePixelRatio || 1;
		const cssW = canvas!.width / dpr;
		const cssH = canvas!.height / dpr;
		return {
			x: (ex - rect.left - cssW / 2) / camera.zoom + camera.x,
			y: (ey - rect.top - cssH / 2) / camera.zoom + camera.y,
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
		if (hit?.trackId !== hoveredNode?.trackId) {
			hoveredNode = hit;
			canvas.style.cursor = hit ? 'pointer' : 'grab';
			// Pass viewport coordinates — hover card uses position:fixed and needs viewport origin
			onHoverNode?.(hit, e.clientX, e.clientY);
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
				// One-shot selection ripple
				rippleNode = hit;
				rippleStartTick = tick;
			} else {
				selectedNode = null;
			}
		}
		isPanning = false;
	}

	function onWheel(e: WheelEvent) {
		if (e.ctrlKey || e.metaKey) return; // yield to global UI-zoom handler
		e.preventDefault();
		const factor = e.deltaY < 0 ? 1.1 : 0.9;
		camera.zoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, camera.zoom * factor));
	}

	// ── Resize observer ───────────────────────────────────────────────────────

	let resizeObserver: ResizeObserver | null = null;
	let dprCleanup: (() => void) | null = null;

	function applyCanvasSize(width: number, height: number) {
		if (!canvas) return;
		const dpr = window.devicePixelRatio || 1;
		canvas.width = width * dpr;
		canvas.height = height * dpr;
		const ctx = canvas.getContext('2d');
		if (ctx) ctx.scale(dpr, dpr);
	}

	onMount(() => {
		if (!canvas) return;

		resizeObserver = new ResizeObserver((entries) => {
			const entry = entries[0];
			if (!entry || !canvas) return;
			const { width, height } = entry.contentRect;
			applyCanvasSize(width, height);
		});
		resizeObserver.observe(canvas.parentElement!);

		// Initial size
		const rect = canvas.parentElement!.getBoundingClientRect();
		applyCanvasSize(rect.width, rect.height);

		// Re-size when moving between monitors with different DPR (Windows doesn't
		// always fire a layout resize when DPI changes, so we watch for it directly).
		function watchDpr() {
			const mq = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
			function onChange() {
				const r = canvas!.parentElement!.getBoundingClientRect();
				applyCanvasSize(r.width, r.height);
				mq.removeEventListener('change', onChange);
				watchDpr();
			}
			mq.addEventListener('change', onChange);
			dprCleanup = () => mq.removeEventListener('change', onChange);
		}
		watchDpr();

		// Expose hyperspace search to page via window (same pattern as existing /discover)
		(window as any).__discoverSpaceHyperspaceSearch = runHyperspaceSearch;

		loop();
	});

	onDestroy(() => {
		alive = false;
		cancelAnimationFrame(rafId);
		cancelAnimationFrame(auxRafId);
		resizeObserver?.disconnect();
		dprCleanup?.();
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
		font-size: var(--font-size-xs);
		font-family: var(--font-mono);
		color: rgba(200, 200, 220, 0.8);
		pointer-events: none;
		z-index: 100;
	}
</style>
