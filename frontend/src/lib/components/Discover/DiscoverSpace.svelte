<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import type { DiscoverTrackNode, DiscoverArtistNode, DiscoverEdge, DiscoverViewMode } from './discover.types';
	import { applyForces } from './discoverBuilder';
	import { discoverSpace, addVisitedRegion } from '$lib/stores/discover_space';
	import { training } from '$lib/stores/training';

	let {
		nodes = [],
		artists = [],
		edges = [],
		mode = 'radio',
		currentTrackId = null,
		seedTrackId = null,
		isSeedLocked = false,
		onHover = (_node: DiscoverTrackNode | null) => {},
		onHoverPosition = (_node: DiscoverTrackNode | null, _x: number, _y: number) => {},
		onSelect = (_node: DiscoverTrackNode) => {},
		onNewNodes = (_nodes: DiscoverTrackNode[]) => {},
	}: {
		nodes?: DiscoverTrackNode[];
		artists?: DiscoverArtistNode[];
		edges?: DiscoverEdge[];
		mode?: DiscoverViewMode;
		currentTrackId?: number | null;
		seedTrackId?: number | null;
		isSeedLocked?: boolean;
		onHover?: (node: DiscoverTrackNode | null) => void;
		onHoverPosition?: (node: DiscoverTrackNode | null, x: number, y: number) => void;
		onSelect?: (node: DiscoverTrackNode) => void;
		onNewNodes?: (nodes: DiscoverTrackNode[]) => void;
	} = $props();

	let canvas: HTMLCanvasElement | null = null;
	let ctx: CanvasRenderingContext2D | null = null;
	let animId: number;
	let camera = $state({ x: 0, y: 0, zoom: 1 });
	let isDragging = $state(false);
	let dragStart = $state({ x: 0, y: 0 });
	let cameraStart = $state({ x: 0, y: 0 });
	let hoveredNode = $state<DiscoverTrackNode | null>(null);

	// ── Training animation state ─────────────────────────────────────────────
	let trainingNodes = $state<Set<number>>(new Set());
	let trainingProgress = $state<{ done: number; total: number; currentTitle: string | null }>({ done: 0, total: 0, currentTitle: null });
	let pulseNodes = $state<Set<number>>(new Set());
	let pulseStartTime = $state(0);

	// ── Edge drawing animation state ─────────────────────────────────────────
	let edgeDrawProgress = $state<Map<string, number>>(new Map()); // "from-to" -> 0..1

	// ── Hyperspace jump animation state ──────────────────────────────────────
	let hyperspacePhase = $state<'idle' | 'dim' | 'zoom_out' | 'warp' | 'zoom_in' | 'settle'>('idle');
	let hyperspaceStartTime = $state(0);
	let hyperspaceResults = $state<any[]>([]);

	// ── Visited regions (nebula halos) ───────────────────────────────────────
	let visitedRegions = $state<Map<string, { x: number; y: number; radius: number }>>(new Map());

	// Sync visitedRegions from store
	$effect(() => {
		let unsub = discoverSpace.subscribe((s) => {
			visitedRegions = new Map(s.visitedRegions);
		});
		return unsub;
	});

	// ── Training progress subscription ───────────────────────────────────────
	$effect(() => {
		let unsub = training.subscribe((t) => {
			if (t.isRunning && t.tracks_total > 0) {
				trainingProgress = {
					done: t.tracks_done,
					total: t.tracks_total,
					currentTitle: t.current_track_title
				};
				if (t.current_track_id != null) {
					trainingNodes.add(t.current_track_id);
					trainingNodes = new Set(trainingNodes); // trigger reactivity
				}
			}
			if (t.stage === 'complete' || (t.tracks_done >= t.tracks_total && t.tracks_total > 0)) {
				pulseNodes = new Set(nodes.map(n => n.track_id));
				pulseStartTime = Date.now();
				// Camera pull back
				animateCameraZoom(camera.zoom * 0.7, 500);
			}
		});
		return unsub;
	});

	// ── Camera animation helper ──────────────────────────────────────────────
	function animateCameraZoom(targetZoom: number, durationMs: number) {
		const startZoom = camera.zoom;
		const startTime = Date.now();

		function step() {
			const elapsed = Date.now() - startTime;
			const t = Math.min(1, elapsed / durationMs);
			const ease = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2; // easeInOutQuad
			camera.zoom = startZoom + (targetZoom - startZoom) * ease;
			if (t < 1) requestAnimationFrame(step);
		}
		step();
	}

	// ── Hyperspace search ────────────────────────────────────────────────────
	async function hyperspaceSearch(query: string) {
		hyperspacePhase = 'dim';
		hyperspaceStartTime = Date.now();

		// T+0: Dim existing nodes
		for (const node of nodes) {
			(node as any).targetOpacity = 0.3;
		}

		// T+100: Start zoom out
		setTimeout(() => {
			hyperspacePhase = 'zoom_out';
			animateCameraZoom(0.3, 200);
		}, 100);

		// T+300: Warp peak
		setTimeout(() => {
			hyperspacePhase = 'warp';
		}, 300);

		// T+600: Results arrive — snap camera, zoom in
		try {
			const client = await import('$lib/api/client');
			const apiBase = client.getApiBase();
			const response = await client.authFetch(`${apiBase}/api/discovery/space`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					mode: 'explore',
					prompt: query,
					limit: 60,
					include_artists: false,
				}),
			});
			if (response.ok) {
				const data = await response.json();
				hyperspaceResults = data.tracks ?? [];

				// Compute centroid
				let cx = 0, cy = 0, count = 0;
				for (const track of hyperspaceResults) {
					cx += track.x ?? 0;
					cy += track.y ?? 0;
					count++;
				}
				if (count > 0) { cx /= count; cy /= count; }

				camera.x = cx;
				camera.y = cy;
				hyperspacePhase = 'zoom_in';
				animateCameraZoom(1.0, 300);

				// T+700: New nodes materialize staggered — notify parent via callback
				setTimeout(() => {
					hyperspacePhase = 'settle';
					const incoming: DiscoverTrackNode[] = hyperspaceResults.map((track) => ({
						track_id: track.track_id,
						title: track.title,
						artist_name: track.artist_name,
						album_title: track.album_title,
						artwork_url: track.artwork_url,
						duration_ms: track.duration_ms,
						similarity_score: track.similarity_score ?? 0.5,
						energy: track.energy,
						danceability: track.danceability,
						bpm: track.bpm,
						key_signature: track.key_signature,
						camelot_key: track.camelot_key,
						is_in_library: track.is_in_library ?? false,
						source: track.source ?? 'tidal',
						x: track.x ?? cx + (Math.random() - 0.5) * 200,
						y: track.y ?? cy + (Math.random() - 0.5) * 200,
						vx: (cx - (track.x ?? cx)) * 0.1,
						vy: (cy - (track.y ?? cy)) * 0.1,
						radius: track.radius ?? 6,
						opacity: 0,
					}));
					// Mark edges for progressive drawing
					for (const node of incoming) {
						for (const edge of (data.edges ?? [])) {
							if (edge.from_id === node.track_id || edge.to_id === node.track_id) {
								edgeDrawProgress.set(`${edge.from_id}-${edge.to_id}`, 0);
							}
						}
					}
					edgeDrawProgress = new Map(edgeDrawProgress);
					// Push to parent store — no prop mutation
					onNewNodes(incoming);
				}, 100);

				// Store visited region
				addVisitedRegion(query, { x: cx, y: cy, radius: 300 });
			}
		} catch (e) {
			console.error('Hyperspace search failed:', e);
		}

		// T+1200: Full opacity restored
		setTimeout(() => {
			hyperspacePhase = 'idle';
			for (const node of nodes) {
				(node as any).targetOpacity = 1;
			}
		}, 1200);
	}

	// Expose hyperspaceSearch for parent components
	$effect(() => {
		// Make available on window for parent to call
		(window as any).__discoverSpaceHyperspaceSearch = hyperspaceSearch;
	});

	function energyColor(energy: number | null): string {
		if (energy == null) return 'hsl(220,20%,50%)';
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

		// ── Draw visited region halos (nebula) ───────────────────────────
		for (const [prompt, region] of visitedRegions) {
			const gradient = ctx.createRadialGradient(region.x, region.y, 0, region.x, region.y, region.radius);
			gradient.addColorStop(0, 'rgba(124,128,255,0.03)');
			gradient.addColorStop(0.5, 'rgba(124,128,255,0.01)');
			gradient.addColorStop(1, 'transparent');
			ctx.beginPath();
			ctx.arc(region.x, region.y, region.radius, 0, Math.PI * 2);
			ctx.fillStyle = gradient;
			ctx.fill();

			// Label
			ctx.fillStyle = 'rgba(255,255,255,0.1)';
			ctx.font = '10px sans-serif';
			ctx.fillText(prompt, region.x - ctx.measureText(prompt).width / 2, region.y + region.radius + 14);
		}

		// ── Draw edges ───────────────────────────────────────────────────
		for (const edge of edges) {
			const from = nodes.find(n => n.track_id === edge.from_id);
			const to = nodes.find(n => n.track_id === edge.to_id);
			if (!from || !to) continue;

			const edgeKey = `${edge.from_id}-${edge.to_id}`;
			const drawProg = edgeDrawProgress.get(edgeKey);

			// Progressive edge drawing for new edges
			if (drawProg != null && drawProg < 1) {
				const newProg = Math.min(1, drawProg + 0.05);
				edgeDrawProgress.set(edgeKey, newProg);

				const endX = from.x + (to.x - from.x) * newProg;
				const endY = from.y + (to.y - from.y) * newProg;
				ctx.beginPath();
				ctx.moveTo(from.x, from.y);
				ctx.lineTo(endX, endY);
			} else {
				ctx.beginPath();
				ctx.moveTo(from.x, from.y);
				ctx.lineTo(to.x, to.y);
			}

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

		// ── Draw track nodes ─────────────────────────────────────────────
		for (const node of nodes) {
			const color = energyColor(node.energy);

			// ── Training fade-in ─────────────────────────────────────
			let currentOpacity = node.opacity;
			let currentRadius = node.radius;

			if (trainingNodes.has(node.track_id)) {
				currentOpacity = Math.min(1, currentOpacity + 0.03); // fade in over ~600ms
				node.opacity = currentOpacity;
				currentRadius = 2 + (currentOpacity * (node.radius - 2)); // grow from 2 to final
			}

			// Hyperspace dim
			if (hyperspacePhase === 'dim' || hyperspacePhase === 'zoom_out' || hyperspacePhase === 'warp') {
				const targetOp = (node as any).targetOpacity ?? 1;
				if (targetOp < 1) {
					currentOpacity *= targetOp;
				}
			}

			// Glow
			if (node.danceability != null) {
				const glowRadius = currentRadius * (1 + node.danceability * 0.8);
				const gradient = ctx.createRadialGradient(node.x, node.y, currentRadius * 0.5, node.x, node.y, glowRadius);
				const energy = node.energy ?? 0.5;
				const hue = 220 - energy * 220;
				gradient.addColorStop(0, `hsla(${hue},70%,60%,0.25)`);
				gradient.addColorStop(1, `hsla(${hue},70%,60%,0)`);
				ctx.beginPath();
				ctx.arc(node.x, node.y, glowRadius, 0, Math.PI * 2);
				ctx.fillStyle = gradient;
				ctx.fill();
			}

			// ── Pulse effect for training completion ─────────────────
			if (pulseNodes.has(node.track_id)) {
				const elapsed = Date.now() - pulseStartTime;
				if (elapsed < 1000) {
					const pulseScale = 1 + 0.15 * Math.sin(elapsed / 200);
					ctx.save();
					ctx.translate(node.x, node.y);
					ctx.scale(pulseScale, pulseScale);
					ctx.translate(-node.x, -node.y);

					ctx.globalAlpha = currentOpacity;
					ctx.beginPath();
					ctx.arc(node.x, node.y, currentRadius, 0, Math.PI * 2);
					ctx.fillStyle = color;
					ctx.fill();
					ctx.strokeStyle = node.source === 'tidal' ? 'rgba(255,255,255,0.8)' : 'rgba(255,255,255,0.4)';
					ctx.lineWidth = node.source === 'tidal' ? 2 : 1;
					ctx.setLineDash(node.source === 'tidal' ? [] : [3, 3]);
					ctx.stroke();
					ctx.setLineDash([]);
					ctx.globalAlpha = 1;

					ctx.restore();
					continue; // skip normal draw for pulsed nodes
				} else {
					pulseNodes.delete(node.track_id);
					pulseNodes = new Set(pulseNodes);
				}
			}

			// Core circle
			ctx.globalAlpha = currentOpacity;
			ctx.beginPath();
			ctx.arc(node.x, node.y, currentRadius, 0, Math.PI * 2);
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
				ctx.arc(node.x, node.y, currentRadius + 4, 0, Math.PI * 2);
				ctx.strokeStyle = 'rgba(255,255,255,0.6)';
				ctx.lineWidth = 2;
				ctx.stroke();
			}
		}

		// ── Seed-distinct rendering ──────────────────────────────────────
		if (seedTrackId != null) {
			const seedNode = nodes.find(n => n.track_id === seedTrackId);
			if (seedNode) {
				const t = (Date.now() % 3000) / 3000;
				const seedRadius = seedNode.radius * 1.5;

				// Big purple halo
				const haloR = seedRadius * 2.4;
				const haloGrad = ctx.createRadialGradient(seedNode.x, seedNode.y, 0, seedNode.x, seedNode.y, haloR);
				haloGrad.addColorStop(0, 'rgba(91, 78, 248, 0.4)');
				haloGrad.addColorStop(0.5, 'rgba(91, 78, 248, 0.15)');
				haloGrad.addColorStop(1, 'transparent');
				ctx.beginPath();
				ctx.arc(seedNode.x, seedNode.y, haloR, 0, Math.PI * 2);
				ctx.fillStyle = haloGrad;
				ctx.fill();

				// Heartbeat ring (slower than currentTrackId pulse)
				const heartbeatR = seedRadius + 8 + Math.sin(t * Math.PI * 2) * 3;
				ctx.beginPath();
				ctx.arc(seedNode.x, seedNode.y, heartbeatR, 0, Math.PI * 2);
				ctx.strokeStyle = 'rgba(91, 78, 248, 0.7)';
				ctx.lineWidth = 2;
				ctx.stroke();

				// Bright filled core overriding the regular node
				ctx.beginPath();
				ctx.arc(seedNode.x, seedNode.y, seedRadius, 0, Math.PI * 2);
				const coreGrad = ctx.createRadialGradient(seedNode.x, seedNode.y, 0, seedNode.x, seedNode.y, seedRadius);
				coreGrad.addColorStop(0, '#ffffff');
				coreGrad.addColorStop(0.4, '#a89cff');
				coreGrad.addColorStop(1, '#5b4ef8');
				ctx.fillStyle = coreGrad;
				ctx.fill();
				ctx.strokeStyle = '#ffffff';
				ctx.lineWidth = 2;
				ctx.stroke();

				// Inline label
				ctx.save();
				ctx.fillStyle = '#ffffff';
				ctx.font = '600 13px system-ui, sans-serif';
				ctx.textAlign = 'center';
				ctx.textBaseline = 'top';
				ctx.fillText(seedNode.title, seedNode.x, seedNode.y + seedRadius + 8);
				if (seedNode.artist_name) {
					ctx.fillStyle = 'rgba(160, 160, 192, 0.9)';
					ctx.font = '11px system-ui, sans-serif';
					ctx.fillText(seedNode.artist_name, seedNode.x, seedNode.y + seedRadius + 24);
				}
				// Lock indicator
				if (isSeedLocked) {
					ctx.fillStyle = '#5b4ef8';
					ctx.font = '14px system-ui, sans-serif';
					ctx.textAlign = 'left';
					ctx.fillText('🔒', seedNode.x + seedRadius * 0.7, seedNode.y - seedRadius);
				}
				ctx.restore();
			}
		}

		// ── Currently playing node indicator ────────────────────────────
		if (currentTrackId != null) {
			const playingNode = nodes.find(n => n.track_id === currentTrackId);
			if (playingNode) {
				const t = (Date.now() % 2000) / 2000;
				const pulseR = playingNode.radius + 6 + Math.sin(t * Math.PI * 2) * 4;
				ctx.beginPath();
				ctx.arc(playingNode.x, playingNode.y, pulseR, 0, Math.PI * 2);
				ctx.strokeStyle = 'rgba(124,128,255,0.9)';
				ctx.lineWidth = 2;
				ctx.stroke();
				// Second outer ring fading out
				const outerR = playingNode.radius + 14 + Math.sin(t * Math.PI * 2) * 6;
				ctx.beginPath();
				ctx.arc(playingNode.x, playingNode.y, outerR, 0, Math.PI * 2);
				ctx.strokeStyle = `rgba(124,128,255,${0.3 - t * 0.3})`;
				ctx.lineWidth = 1;
				ctx.stroke();
			}
		}

		// ── Warp streak effect (hyperspace) ──────────────────────────────
		if (hyperspacePhase === 'warp') {
			const elapsed = Date.now() - hyperspaceStartTime;
			const intensity = Math.min(1, elapsed / 200);

			ctx.save();
			ctx.globalAlpha = intensity * 0.6;
			ctx.globalCompositeOperation = 'lighter';

			const numStreaks = 60;
			for (let i = 0; i < numStreaks; i++) {
				const angle = (i / numStreaks) * Math.PI * 2;
				const length = 100 + intensity * 400;
				const x1 = Math.cos(angle) * 20;
				const y1 = Math.sin(angle) * 20;
				const x2 = Math.cos(angle) * length;
				const y2 = Math.sin(angle) * length;

				const gradient = ctx.createLinearGradient(x1, y1, x2, y2);
				gradient.addColorStop(0, 'rgba(124,128,255,0.8)');
				gradient.addColorStop(1, 'transparent');

				ctx.beginPath();
				ctx.moveTo(x1, y1);
				ctx.lineTo(x2, y2);
				ctx.strokeStyle = gradient;
				ctx.lineWidth = 2 + intensity * 3;
				ctx.stroke();
			}

			ctx.restore();
		}

		ctx.restore();
	}

	function tick() {
		const allNodes = [...nodes, ...artists as (DiscoverTrackNode | DiscoverArtistNode)[]];
		applyForces(allNodes, edges, mode, 0.5);

		// Fade in (non-training nodes)
		for (const node of nodes) {
			if (!trainingNodes.has(node.track_id) && node.opacity < 1) {
				node.opacity = Math.min(1, node.opacity + 0.02);
			}
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
		onHoverPosition(found, e.clientX, e.clientY);
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

		// ResizeObserver fires when the element actually has layout dimensions,
		// avoiding the race condition where offsetWidth/Height are 0 in onMount.
		const ro = new ResizeObserver(() => {
			el.width = el.offsetWidth * devicePixelRatio;
			el.height = el.offsetHeight * devicePixelRatio;
			c.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
		});
		ro.observe(el);

		el.addEventListener('wheel', onWheel, { passive: false });
		el.addEventListener('mousedown', onMouseDown);
		window.addEventListener('mousemove', onMouseMove);
		window.addEventListener('mouseup', onMouseUp);
		el.addEventListener('click', onClick);

		animId = requestAnimationFrame(tick);

		return () => {
			ro.disconnect();
		};
	});

	onDestroy(() => {
		cancelAnimationFrame(animId);
	});
</script>

<div class="canvas-wrap">
	<canvas bind:this={canvas} class="discover-canvas"></canvas>

	{#if trainingProgress.total > 0}
	  <div class="training-overlay">
	    <div class="training-strip">
	      <span class="track-title">{trainingProgress.currentTitle ?? 'Embedding...'}</span>
	      <div class="progress-bar">
	        <div class="progress-fill" style="width: {(trainingProgress.done / trainingProgress.total) * 100}%"></div>
	      </div>
	      <span class="progress-count">{trainingProgress.done} / {trainingProgress.total}</span>
	    </div>
	  </div>
	{/if}
</div>

<style>
	.canvas-wrap {
		position: relative;
		width: 100%;
		height: 100%;
	}

	.discover-canvas {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		cursor: grab;
		display: block;
	}
	.discover-canvas:active {
		cursor: grabbing;
	}

	.training-overlay {
		position: absolute;
		bottom: 24px;
		left: 50%;
		transform: translateX(-50%);
		z-index: 10;
	}
	.training-strip {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 20px;
		border-radius: 12px;
		backdrop-filter: blur(16px);
		background: rgba(255,255,255,0.05);
		border: 1px solid rgba(255,255,255,0.08);
		min-width: 320px;
	}
	.track-title {
		color: rgba(255,255,255,0.9);
		font-size: 13px;
		font-weight: 500;
		max-width: 180px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.progress-bar {
		flex: 1;
		height: 4px;
		background: rgba(255,255,255,0.1);
		border-radius: 2px;
		overflow: hidden;
	}
	.progress-fill {
		height: 100%;
		background: linear-gradient(90deg, rgba(124,128,255,0.6), rgba(124,128,255,1));
		border-radius: 2px;
		transition: width 0.15s ease-out;
	}
	.progress-count {
		color: rgba(255,255,255,0.5);
		font-size: 12px;
		font-variant-numeric: tabular-nums;
		min-width: 60px;
		text-align: right;
	}
</style>
