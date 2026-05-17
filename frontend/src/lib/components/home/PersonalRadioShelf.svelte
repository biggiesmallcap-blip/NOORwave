<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { api, ApiError, type TidalMix } from '$lib/api/client';
	import { playTidalMix } from '$lib/stores/player';
	import { tidalStatus } from '$lib/stores/tidal';
	import {
		getCachedRadioStations,
		putCachedRadioStations,
		clearCachedRadioStations
	} from '$lib/stores/tidal-radio-cache';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	const cachedOnMount = getCachedRadioStations();
	let stations = $state<TidalMix[]>(cachedOnMount ?? []);
	let viewState = $state<State>(
		cachedOnMount && cachedOnMount.length > 0 ? 'ready' : 'loading'
	);
	let errorMsg = $state<string>('');

	onMount(() => {
		if (cachedOnMount && cachedOnMount.length > 0) return;
		void load();
	});

	// Untrack viewState so the effect only re-fires on tidalStatus transitions.
	// Reading viewState directly would create a fetch loop when load() ends in
	// disconnected/empty/error state.
	$effect(() => {
		if ($tidalStatus !== 'connected') return;
		const cur = untrack(() => viewState);
		if (cur !== 'loading' && cur !== 'ready') {
			void load();
		}
	});

	async function load() {
		viewState = 'loading';
		errorMsg = '';
		try {
			const data = await api.getTidalRadioStations();
			stations = data.stations ?? [];
			if (stations.length > 0) putCachedRadioStations(stations);
			viewState = stations.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (e instanceof ApiError && e.status === 503) {
				clearCachedRadioStations();
				viewState = 'disconnected';
			} else {
				viewState = 'error';
				errorMsg = e instanceof Error ? e.message : 'Failed to load radio stations';
			}
		}
	}
</script>

{#snippet stationRail(items: TidalMix[])}
	<div class="mix-rail" use:wheelToHorizontal>
		{#each items as station (station.id)}
			<button
				type="button"
				class="mix-card"
				title={station.sub_title ?? station.title}
				aria-label={`Play radio station ${station.title}`}
				onclick={() => playTidalMix(station.id)}
			>
				<div class="art-wrap">
					{#if station.image_url}
						<div class="art" style="background-image: url('{station.image_url}')"></div>
					{:else}
						<div class="art fallback">📻</div>
					{/if}
					<PlayOverlay
						position="center"
						size="md"
						label={`Play radio station ${station.title}`}
					/>
				</div>
				<div class="meta">
					<h3 class="title">{station.title}</h3>
					{#if station.sub_title}
						<p class="artist">{station.sub_title}</p>
					{/if}
				</div>
			</button>
		{/each}
	</div>
{/snippet}

{#snippet skeletonRail()}
	<div class="mix-rail" use:wheelToHorizontal>
		{#each [0, 1, 2, 3, 4, 5] as i (i)}
			<div class="mix-card skeleton">
				<div class="art-wrap"><div class="art skeleton-art"></div></div>
				<div class="meta">
					<div class="skeleton-line skeleton-line-title"></div>
					<div class="skeleton-line skeleton-line-sub"></div>
				</div>
			</div>
		{/each}
	</div>
{/snippet}

<section class="discovery-section" data-section="personal-radio">
	<div class="section-header">
		<div class="section-title-group">
			<p class="eyebrow">TIDAL</p>
			<h2>Personal Radio</h2>
		</div>
		{#if viewState === 'loading'}
			<span class="loading-indicator">Loading…</span>
		{/if}
	</div>

	{#if viewState === 'loading'}
		{@render skeletonRail()}
	{:else if viewState === 'ready'}
		{@render stationRail(stations)}
	{:else if viewState === 'empty'}
		<p class="muted-line">No personal radio stations found — save some in TIDAL to see them here.</p>
	{:else if viewState === 'disconnected'}
		<p class="muted-line">
			Connect TIDAL to see your personal radio stations.
			<a class="inline-link" href="/settings#sources-tidal">Open settings</a>
		</p>
	{:else if viewState === 'error'}
		<p class="muted-line">
			Couldn't load radio stations{errorMsg ? `: ${errorMsg}` : '.'}
			<button class="inline-link" onclick={load}>Retry</button>
		</p>
	{/if}
</section>

<style>
	.discovery-section {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.section-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
	}
	.section-title-group {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.section-title-group h2 {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		margin: 0;
	}
	.loading-indicator {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		font-style: italic;
	}

	.mix-rail {
		display: flex;
		gap: 14px;
		overflow-x: auto;
		padding-bottom: 8px;
		scroll-snap-type: x mandatory;
		mask-image: linear-gradient(
			to right,
			transparent 0,
			black 16px,
			black calc(100% - 32px),
			transparent 100%
		);
		-webkit-mask-image: linear-gradient(
			to right,
			transparent 0,
			black 16px,
			black calc(100% - 32px),
			transparent 100%
		);
	}
	.mix-rail::-webkit-scrollbar { height: 6px; }
	.mix-rail::-webkit-scrollbar-track {
		background: var(--bg-surface);
		border-radius: 3px;
	}
	.mix-rail::-webkit-scrollbar-thumb {
		background: var(--border-subtle);
		border-radius: 3px;
	}
	.mix-rail::-webkit-scrollbar-thumb:hover {
		background: var(--text-muted);
	}

	.mix-card {
		flex: 0 0 180px;
		width: 180px;
		min-width: 180px;
		max-width: 180px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		background: none;
		border: 1px solid transparent;
		padding: 8px;
		border-radius: 12px;
		text-align: left;
		scroll-snap-align: start;
		transition: background var(--motion-base), border-color var(--motion-base);
		box-sizing: border-box;
		cursor: pointer;
		font: inherit;
		color: inherit;
	}
	.mix-card:hover,
	.mix-card:focus-visible {
		background: rgba(255, 255, 255, 0.04);
		border-color: rgba(255, 255, 255, 0.08);
		outline: none;
	}
	.mix-card:focus-visible {
		border-color: var(--accent-line, rgba(125, 200, 175, 0.6));
	}

	.mix-card:hover :global(.play-overlay),
	.mix-card:focus-visible :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	.art-wrap {
		position: relative;
		aspect-ratio: 1 / 1;
		width: 100%;
		border-radius: 8px;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.04);
	}
	.art {
		width: 100%;
		height: 100%;
		background-size: cover;
		background-position: center;
		transition: transform var(--motion-base);
	}
	.mix-card:hover .art {
		transform: scale(1.05);
	}
	.art.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-4xl);
		color: rgba(255, 255, 255, 0.55);
	}

	.meta {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}
	.title {
		margin: 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary, #fff);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: var(--line-height-snug);
	}
	.artist {
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-secondary, rgba(255, 255, 255, 0.6));
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.skeleton-art {
		background: linear-gradient(
			110deg,
			rgba(255, 255, 255, 0.04) 30%,
			rgba(255, 255, 255, 0.08) 50%,
			rgba(255, 255, 255, 0.04) 70%
		);
		background-size: 200% 100%;
		animation: shimmer 1.4s linear infinite;
	}
	.skeleton-line {
		height: 0.7rem;
		border-radius: 4px;
		background: rgba(255, 255, 255, 0.08);
	}
	.skeleton-line-title { width: 75%; }
	.skeleton-line-sub   { width: 50%; }
	@keyframes shimmer {
		0%   { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}

	.muted-line {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary, rgba(255, 255, 255, 0.6));
	}
	.inline-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: var(--accent-line, #7dc8af);
		cursor: pointer;
		text-decoration: underline;
		text-underline-offset: 2px;
		margin-left: 6px;
	}
</style>
