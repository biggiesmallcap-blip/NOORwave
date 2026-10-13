<script lang="ts">
	// A small crop of the Atlas around the artist on stage, with the short
	// trail of artists the listener came through. Clicking it opens the full
	// Atlas centred there (`/videos/atlas?artist=...&trail=...`). For the watch
	// page; it draws once per change and never animates.
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { cachedAtlas, loadAtlas, type LoadedAtlas } from './atlas_data';
	import { atlasUrl } from './links';
	import { AtlasRenderer } from './renderer';

	let {
		artistId,
		trail = [],
		look = 'place',
	}: {
		/** TIDAL id of the artist on stage. */
		artistId: number | null;
		/** Recent artist ids, oldest first (the current one may be last). */
		trail?: number[];
		look?: 'light' | 'place';
	} = $props();

	let canvasEl = $state<HTMLCanvasElement>();
	let loaded = $state<LoadedAtlas | null>(cachedAtlas());
	let renderer: AtlasRenderer | null = null;
	let observer: ResizeObserver | null = null;

	let index = $derived(loaded && artistId != null ? (loaded.data.indexById.get(artistId) ?? -1) : -1);
	let sceneName = $derived.by(() => {
		if (!loaded || index < 0) return null;
		const s = loaded.data.scene[index];
		return s >= 0 ? loaded.data.scenes[s].name : 'Uncharted';
	});
	let fullTrail = $derived.by(() => {
		const ids = trail.filter((id) => id !== artistId);
		if (artistId != null) ids.push(artistId);
		return ids.slice(-8);
	});

	function frame() {
		if (!renderer || !loaded) return;
		const data = loaded.data;
		const indices = fullTrail.map((id) => data.indexById.get(id) ?? -1).filter((i) => i >= 0);
		renderer.setTrail(indices.length ? indices : null);
		renderer.setSelection(index, -1);
		if (index < 0) {
			renderer.setCamera({ cx: (data.bounds.minX + data.bounds.maxX) / 2, cy: (data.bounds.minY + data.bounds.maxY) / 2, scale: renderer.fitScale });
			return;
		}
		// Frame the trail, but never wider than a neighbourhood.
		let minX = data.x[index];
		let maxX = minX;
		let minY = data.y[index];
		let maxY = minY;
		for (const i of indices) {
			minX = Math.min(minX, data.x[i]);
			maxX = Math.max(maxX, data.x[i]);
			minY = Math.min(minY, data.y[i]);
			maxY = Math.max(maxY, data.y[i]);
		}
		const { width, height } = renderer.viewport;
		const span = Math.max(maxX - minX, (maxY - minY) * (width / Math.max(1, height)), 40);
		const scale = Math.min(renderer.fitScale * 7, Math.max(renderer.fitScale * 2.2, (width * 0.7) / span));
		renderer.setCamera({ cx: (minX + maxX) / 2, cy: (minY + maxY) / 2, scale });
	}

	function open() {
		if (artistId == null) return void goto(atlasUrl());
		void goto(atlasUrl({ artist: artistId, trail: fullTrail.filter((id) => id !== artistId) }));
	}

	onMount(() => {
		if (!canvasEl) return;
		try {
			renderer = new AtlasRenderer(canvasEl, { mini: true });
			renderer.setReducedMotion(true);
		} catch {
			return;
		}
		const rect = canvasEl.getBoundingClientRect();
		renderer.resize(rect.width, rect.height, window.devicePixelRatio || 1);
		renderer.setLook(look);
		observer = new ResizeObserver(() => {
			if (!canvasEl || !renderer) return;
			const r = canvasEl.getBoundingClientRect();
			renderer.resize(r.width, r.height, window.devicePixelRatio || 1);
			frame();
		});
		observer.observe(canvasEl);
		void loadAtlas()
			.then((result) => {
				if (result.data.count > 0) loaded = result;
			})
			.catch(() => {});
	});

	onDestroy(() => {
		observer?.disconnect();
		renderer?.destroy();
		renderer = null;
	});

	// Data arrival and artist changes redraw the crop.
	$effect(() => {
		const current = loaded;
		void index;
		void fullTrail;
		if (!renderer || !current) return;
		if (renderer.data !== current.data) renderer.setData(current.data, current.names);
		frame();
	});

	$effect(() => {
		renderer?.setLook(look);
	});
</script>

<button type="button" class="minimap" class:empty={!loaded} onclick={open} aria-label={sceneName ? `Open the Atlas at ${sceneName}` : 'Open the Atlas'}>
	<canvas bind:this={canvasEl} aria-hidden="true"></canvas>
	<span class="caption">
		<span class="where">{sceneName ?? 'Atlas'}</span>
		<span class="go">Open Atlas</span>
	</span>
</button>

<style>
	.minimap {
		position: relative;
		display: block;
		width: 100%;
		aspect-ratio: 16 / 9;
		min-height: 120px;
		padding: 0;
		border-radius: var(--radius-md);
		overflow: hidden;
		background: #04050a;
		text-align: left;
		cursor: pointer;
	}

	canvas {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		display: block;
		transition: transform var(--motion-base);
	}

	.minimap:hover canvas {
		transform: scale(1.03);
	}

	.minimap:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.caption {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-2);
		padding: var(--space-4) var(--space-3) var(--space-2);
		background: linear-gradient(transparent, rgba(3, 4, 9, 0.85));
		color: rgba(240, 242, 250, 0.95);
	}

	.where {
		font-family: var(--font-display);
		font-style: italic;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.go {
		flex: none;
		color: rgba(222, 226, 240, 0.7);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
	}

	@media (prefers-reduced-motion: reduce) {
		canvas {
			transition: none;
		}
	}
</style>
