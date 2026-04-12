<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import type { DiscoverTrackNode, DiscoverArtistNode, DiscoverEdge, DiscoverViewMode } from './discover.types';
	import { applyForces } from './discoverBuilder';

	let {
		nodes = [],
		artists = [],
		edges = [],
		mode = 'radio',
		onHover = (_node: DiscoverTrackNode | null) => {},
		onSelect = (_node: DiscoverTrackNode) => {}
	}: {
		nodes?: DiscoverTrackNode[];
		artists?: DiscoverArtistNode[];
		edges?: DiscoverEdge[];
		mode?: DiscoverViewMode;
		onHover?: (node: DiscoverTrackNode | null) => void;
		onSelect?: (node: DiscoverTrackNode) => void;
	} = $props();

	let canvas: HTMLCanvasElement | null = null;
	let ctx: CanvasRenderingContext2D | null = null;
	let animId: number;
	let camera = $state({ x: 0, y: 0, zoom: 1 });
	let isDragging = $state(false);
	let dragStart = $state({ x: 0, y: 0 });
	let cameraStart = $state({ x: 0, y: 0 });
	let hoveredNode = $state<DiscoverTrackNode | null>(null);

	function energyColor(energy: number | null): string {
		if (energy == null) return '#666';
		const hue = 220 - energy * 220;
		return `hsl(${hue}, 70%, 55%)`;
	}

	function draw() {
		if (!ctx || !canvas) return;
		const w = canvas.width / devicePixelRatio;
		const h = canvas.height / devicePixelRatio;
		ctx.clearRect(0, 0, canvas.width, canvas.height);
		ctx.save();
		ctx.translate(w / 2, h / 2);
		ctx.scale(camera.zoom, camera.zoom);
		ctx.translate(-camera.x, -camera.y);

		// Draw edges
		for (const edge of edges) {
			const from = nodes.find(n => n.track_id === edge.from_id);
			const to = nodes.find(n => n.track_id === edge.to_id);
			if (!from || !to) continue;
			ctx.beginPath();
			ctx.moveTo(from.x, from.y);
			ctx.lineTo(to.x, to.y);
			switch (edge.type) {
				case 'bpm_match': ctx.strokeStyle = 'rgba(255,200,50,0.3)'; break;
				case 'harmonic': ctx.strokeStyle = 'rgba(150,100,255,0.3)'; break;
				case 'behavioural': ctx.strokeStyle = 'rgba(80,150,255,0.2)'; break;
				case 'sample': ctx.strokeStyle = 'rgba(255,100,50,0.4)'; break;
				default: ctx.strokeStyle = 'rgba(255,255,255,0.1)';
			}
			ctx.lineWidth = edge.weight * 3;
			ctx.stroke();
		}

		// Draw track nodes
		for (const node of nodes) {
			const color = energyColor(node.energy);

			// Glow
			if (node.danceability != null) {
				const glowRadius = node.radius * (1 + node.danceability * 0.8);
				const gradient = ctx.createRadialGradient(node.x, node.y, node.radius * 0.5, node.x, node.y, glowRadius);
				gradient.addColorStop(0, color + '40');
				gradient.addColorStop(1, 'transparent');
				ctx.beginPath();
				ctx.arc(node.x, node.y, glowRadius, 0, Math.PI * 2);
				ctx.fillStyle = gradient;
				ctx.fill();
			}

			// Core circle
			ctx.globalAlpha = node.opacity;
			ctx.beginPath();
			ctx.arc(node.x, node.y, node.radius, 0, Math.PI * 2);
			ctx.fillStyle = color;
			ctx.fill();
			ctx.strokeStyle = node.source === 'tidal' ? 'rgba(255,255,255,0.8)' : 'rgba(255,255,255,0.4)';
			ctx.lineWidth = node.source === 'tidal' ? 2 : 1;
			ctx.setLineDash(node.source === 'tidal' ? [] : [3, 3]);
			ctx.stroke();
			ctx.setLineDash([]);
			ctx.globalAlpha = 1;

			// Hover highlight
			if (hoveredNode?.track_id === node.track_id) {
				ctx.beginPath();
				ctx.arc(node.x, node.y, node.radius + 4, 0, Math.PI * 2);
				ctx.strokeStyle = 'rgba(255,255,255,0.6)';
				ctx.lineWidth = 2;
				ctx.stroke();
			}
		}

		ctx.restore();
	}

	function tick() {
		const allNodes = [...nodes, ...artists as (DiscoverTrackNode | DiscoverArtistNode)[]];
		applyForces(allNodes, edges, mode, 0.5);

		// Fade in
		for (const node of nodes) {
			if (node.opacity < 1) node.opacity = Math.min(1, node.opacity + 0.02);
		}

		draw();
		animId = requestAnimationFrame(tick);
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		camera.zoom *= e.deltaY > 0 ? 0.9 : 1.1;
		camera.zoom = Math.max(0.1, Math.min(5, camera.zoom));
	}

	function onMouseDown(e: MouseEvent) {
		isDragging = true;
		dragStart = { x: e.clientX, y: e.clientY };
		cameraStart = { x: camera.x, y: camera.y };
	}

	function onMouseMove(e: MouseEvent) {
		if (!canvas) return;
		const rect = canvas.getBoundingClientRect();
		const mx = (e.clientX - rect.left - canvas.offsetWidth / 2) / camera.zoom + camera.x;
		const my = (e.clientY - rect.top - canvas.offsetHeight / 2) / camera.zoom + camera.y;

		// Hover detection
		let found: DiscoverTrackNode | null = null;
		for (const node of nodes) {
			const dx = mx - node.x;
			const dy = my - node.y;
			if (dx * dx + dy * dy < node.radius * node.radius) {
				found = node;
				break;
			}
		}
		hoveredNode = found;
		onHover(found);
		if (canvas) canvas.style.cursor = found ? 'pointer' : 'grab';

		if (isDragging) {
			camera.x = cameraStart.x - (e.clientX - dragStart.x) / camera.zoom;
			camera.y = cameraStart.y - (e.clientY - dragStart.y) / camera.zoom;
		}
	}

	function onMouseUp() {
		isDragging = false;
	}

	function onClick() {
		if (hoveredNode) {
			onSelect(hoveredNode);
		}
	}

	onMount(() => {
		if (!canvas) return;
		ctx = canvas.getContext('2d');
		if (!ctx) return;

		const el = canvas;
		const c = ctx;

		const resize = () => {
			el.width = el.offsetWidth * devicePixelRatio;
			el.height = el.offsetHeight * devicePixelRatio;
			c.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
		};
		resize();
		window.addEventListener('resize', resize);

		el.addEventListener('wheel', onWheel, { passive: false });
		el.addEventListener('mousedown', onMouseDown);
		window.addEventListener('mousemove', onMouseMove);
		window.addEventListener('mouseup', onMouseUp);
		el.addEventListener('click', onClick);

		animId = requestAnimationFrame(tick);
	});

	onDestroy(() => {
		cancelAnimationFrame(animId);
	});
</script>

<canvas bind:this={canvas} class="discover-canvas"></canvas>

<style>
	.discover-canvas {
		width: 100%;
		height: 100%;
		cursor: grab;
	}
	.discover-canvas:active {
		cursor: grabbing;
	}
</style>
