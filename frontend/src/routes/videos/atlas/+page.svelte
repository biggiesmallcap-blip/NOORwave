<script lang="ts">
	// The Atlas tab: every artist with music videos on one map, grouped into
	// scenes. The last map renders at once and refreshes quietly (light changes
	// with every watch); while the first map is still being built the stage
	// says so and checks again every 10 s.
	import { onDestroy, onMount } from 'svelte';
	import { page } from '$app/state';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import AtlasView from '$lib/video/atlas/AtlasView.svelte';
	import { cachedAtlas, loadAtlas, type LoadedAtlas } from '$lib/video/atlas/atlas_data';
	import { parseAtlasParams } from '$lib/video/atlas/links';

	const POLL_MS = 10_000;

	// Raw: the map is replaced whole, never edited, and a deep proxy over
	// 15k artists would tax every read the renderer makes.
	let loaded = $state.raw<LoadedAtlas | null>(cachedAtlas());
	let building = $state(false);
	let error = $state<unknown>(null);
	let slow = $state(false);
	let timer: ReturnType<typeof setTimeout> | null = null;
	let slowTimer: ReturnType<typeof setTimeout> | null = null;
	let destroyed = false;

	// Read once: later replaceState calls from the map must not re-focus it.
	const focus = parseAtlasParams(page.url.searchParams);

	async function load() {
		if (timer) clearTimeout(timer);
		timer = null;
		error = null;
		try {
			const next = await loadAtlas(true);
			if (destroyed) return;
			building = next.data.building;
			if (next.data.count > 0) loaded = next;
			// Keep looking until there is a map to draw: a build can be
			// between steps (scenes written, no artist placed yet).
			if (building || next.data.count === 0) timer = setTimeout(() => void load(), POLL_MS);
		} catch (err) {
			if (destroyed) return;
			if (!loaded) error = err;
		}
	}

	onMount(() => {
		// The calm loading copy only appears if the map takes a moment.
		slowTimer = setTimeout(() => (slow = true), 150);
		void load();
	});

	onDestroy(() => {
		destroyed = true;
		if (timer) clearTimeout(timer);
		if (slowTimer) clearTimeout(slowTimer);
	});
</script>

<svelte:head>
	<title>Atlas - NOOR</title>
</svelte:head>

<div class="atlas-page">
	{#if loaded}
		<AtlasView {loaded} {focus} rebuilding={building} />
	{:else if error}
		<ErrorState title="Could not load the Atlas" {error} onretry={() => void load()} />
	{:else}
		<div class="waiting" class:building role="status">
			<div class="sky" aria-hidden="true"></div>
			{#if building}
				<div class="copy">
					<h2>The map is being drawn</h2>
					<p>NOOR is grouping every artist with videos into scenes and laying them out. It checks again every ten seconds and appears here on its own.</p>
				</div>
			{:else if slow}
				<div class="copy quiet">
					<p>Charting the sky...</p>
				</div>
			{/if}
		</div>
	{/if}
</div>

<style>
	/* Width, gutters and the bottom inset come from the video layout. */
	.atlas-page {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		min-width: 0;
	}

	.waiting {
		position: relative;
		display: grid;
		place-items: center;
		height: max(440px, calc(100dvh - 230px - max(var(--bottom-player-height, 0px), 44px)));
		border-radius: var(--radius-lg);
		overflow: hidden;
		background: radial-gradient(ellipse at 50% 42%, #0a0e1c 0%, #05060c 55%, #020306 100%);
		color: rgba(236, 239, 247, 0.92);
	}

	/* A slow drift of faint stars while there is nothing to draw yet. */
	.sky {
		position: absolute;
		inset: -20%;
		background-image:
			radial-gradient(1px 1px at 12% 22%, rgba(255, 255, 255, 0.55), transparent 60%),
			radial-gradient(1px 1px at 68% 38%, rgba(255, 255, 255, 0.45), transparent 60%),
			radial-gradient(1.5px 1.5px at 40% 74%, rgba(255, 236, 210, 0.6), transparent 60%),
			radial-gradient(1px 1px at 84% 82%, rgba(210, 225, 255, 0.5), transparent 60%),
			radial-gradient(1px 1px at 24% 58%, rgba(255, 255, 255, 0.35), transparent 60%),
			radial-gradient(1px 1px at 92% 12%, rgba(255, 255, 255, 0.4), transparent 60%);
		background-size: 340px 340px;
		opacity: 0.7;
		animation: drift 60s linear infinite;
	}

	@keyframes drift {
		to {
			transform: translate3d(-340px, -120px, 0);
		}
	}

	.copy {
		position: relative;
		display: grid;
		gap: var(--space-2);
		max-width: 46ch;
		padding: var(--space-5);
		text-align: center;
	}

	.copy h2 {
		margin: 0;
		font-family: var(--font-display);
		font-style: italic;
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}

	.copy p {
		margin: 0;
		color: rgba(222, 226, 240, 0.72);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
	}

	.copy.quiet p {
		font-family: var(--font-display);
		font-style: italic;
		font-size: var(--font-size-md);
	}

	@media (prefers-reduced-motion: reduce) {
		.sky {
			animation: none;
		}
	}

	:global([data-motion='reduce']) .sky {
		animation: none;
	}
</style>
