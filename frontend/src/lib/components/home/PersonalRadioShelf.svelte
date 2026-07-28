<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { ApiError, type TidalMix } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { playTidalMix } from '$lib/stores/player';
	import { tidalStatus } from '$lib/stores/tidal';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';

	type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	// Position in the home stack; stagger only. See YourMixesShelf.
	let { index = 0 }: { index?: number } = $props();

	// Reactive, persisted query: hydrates the localStorage snapshot synchronously at
	// init so the shelf paints last-known stations with no skeleton, then revalidates
	// in the background. Replaces the in-memory-only cache wiped on every restart.
	const stationsQuery = cachedApi.tidalRadioStationsQuery();
	const seeded = stationsQuery.getSnapshot().data?.stations ?? [];

	let stations = $state<TidalMix[]>(seeded);
	let viewState = $state<State>(seeded.length > 0 ? 'ready' : 'loading');
	let refreshing = $state(false);
	let lastRefreshFailed = $state(false);
	let errorMsg = $state<string>('');

	// The subscription is the sole writer of stations/viewState. Svelte calls it
	// immediately with the current (hydrated) state, then on every revalidate.
	onMount(() => stationsQuery.subscribe((s) => {
		refreshing = s.refreshing;
		// 503 = TIDAL unreachable. Check before reading s.data (SWR keeps prior data
		// alongside the error). Keep cached stations through a transient cold-boot blip;
		// only show the connect prompt when there's nothing to fall back on. Never drop
		// the persisted copy here - a transient 503 would become a skeleton next launch.
		if (s.error instanceof ApiError && s.error.status === 503) {
			lastRefreshFailed = true;
			if (stations.length === 0) viewState = 'disconnected';
			return;
		}
		if (s.data) {
			lastRefreshFailed = false;
			stations = s.data.stations ?? [];
			if (stations.length > 0) viewState = 'ready';
			else if (!s.loading && !s.refreshing) viewState = 'empty';
			return;
		}
		if (s.error && !s.loading) {
			lastRefreshFailed = true;
			viewState = 'error';
			errorMsg = s.error instanceof Error ? s.error.message : 'Failed to load radio stations';
			return;
		}
		if (s.loading) viewState = 'loading';
	}));

	// Re-fetch on (re)connect: the cold-boot race (initial revalidate 503'd before
	// tokens rehydrated) and live connect from Settings. Skip when already showing
	// fresh stations, but force a refresh if the last attempt failed. untrack so the
	// effect only re-runs on tidalStatus changes.
	$effect(() => {
		if ($tidalStatus !== 'connected') return;
		const cur = untrack(() => viewState);
		const haveStations = untrack(() => stations).length > 0;
		const failed = untrack(() => lastRefreshFailed);
		if (cur !== 'ready' || !haveStations || failed) {
			void stationsQuery.refresh().catch(() => {});
		}
	});

	function retry() {
		errorMsg = '';
		viewState = stations.length > 0 ? 'ready' : 'loading';
		void stationsQuery.refresh().catch(() => {});
	}
</script>

{#snippet stationCard(station: TidalMix)}
	<button
		type="button"
		class="mix-card"
		title={station.sub_title ?? station.title}
		aria-label={`Play radio station ${station.title}`}
		onclick={() => playTidalMix(station.id)}
	>
		<div class="art-wrap">
			<ArtworkImage
				className="art"
				src={station.image_url}
				alt={station.title}
				size={320}
				tint={true}
				fallbackText="RAD"
				decorative={true}
			/>
			<PlayOverlay
				position="corner"
				size="sm"
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
{/snippet}

{#snippet stationRail(items: TidalMix[])}
	<MediaRail {items} card={stationCard} getKey={(s) => s.id} gap={14} fluid stagger />
{/snippet}

{#snippet skeletonCard()}
	<div class="mix-card skeleton">
		<div class="art-wrap"><div class="art skeleton-art"></div></div>
		<div class="meta">
			<div class="skeleton-line skeleton-line-title"></div>
			<div class="skeleton-line skeleton-line-sub"></div>
		</div>
	</div>
{/snippet}

{#snippet skeletonRail()}
	<MediaRail
		items={[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]}
		card={skeletonCard}
		getKey={(i) => i}
		gap={14}
		fluid
	/>
{/snippet}

<section class="discovery-section rise-in-shelf" data-section="personal-radio" style={`--rise-index: ${index}`}>
	<SectionHeader eyebrow="TIDAL" title="Personal Radio" variant="charts" level={2}>
		{#snippet actions()}
			{#if viewState === 'loading' || refreshing}
				<span class="loading-indicator">Loading…</span>
			{/if}
		{/snippet}
	</SectionHeader>

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
			<button class="inline-link" onclick={retry}>Retry</button>
		</p>
	{/if}
</section>

<style>
	.discovery-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.loading-indicator {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		font-style: italic;
	}

	/* Rail behaviour lives in MediaRail; this card only describes itself.
	   `min-width: 0` replaces the old explicit 180px lock - see the same note
	   in YourMixesShelf. */
	.mix-card {
		width: 100%;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 10px;
		background: none;
		border: 0;
		padding: 0;
		border-radius: var(--radius-md);
		text-align: left;
		transition: transform var(--motion-base);
		box-sizing: border-box;
		cursor: pointer;
		font: inherit;
		color: inherit;
	}
	.mix-card:hover {
		transform: translateY(-4px);
	}
	.mix-card:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 4px;
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
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--bg-raised);
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		transition: box-shadow var(--motion-base);
	}
	.mix-card:hover .art-wrap {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}
	.art-wrap :global(.art) {
		width: 100%;
		height: 100%;
	}
	.art-wrap :global(img.art) {
		display: block;
		object-fit: cover;
	}
	.art-wrap :global(.art.fallback) {
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.art-wrap :global(.art.fallback span) {
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-semibold);
		color: rgba(255, 255, 255, 0.92);
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
