<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { ApiError, type TidalMix } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { playTidalMix } from '$lib/stores/player';
	import { tidalStatus } from '$lib/stores/tidal';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	// Position in the home stack, used only to stagger the entrance so shelves
	// that resolve together cascade instead of landing as one slab. This shelf
	// owns two sections, so the video rail sits one slot further down.
	let { index = 0 }: { index?: number } = $props();

	// Reactive, persisted query: hydrates the localStorage snapshot synchronously at
	// init so the shelf paints last-known mixes with no skeleton, then revalidates in
	// the background. Replaces the old in-memory-only cache that was wiped on restart.
	const mixesQuery = cachedApi.tidalMixesQuery();
	const seeded = mixesQuery.getSnapshot().data?.mixes ?? [];

	let mixes = $state<TidalMix[]>(seeded);
	let viewState = $state<State>(seeded.length > 0 ? 'ready' : 'loading');
	let refreshing = $state(false);
	let lastRefreshFailed = $state(false);
	let errorMsg = $state<string>('');

	let audioMixes = $derived(mixes.filter((m) => !isMixVideo(m)));
	let videoMixes = $derived(mixes.filter((m) => isMixVideo(m)));

	// The subscription is the sole writer of mixes/viewState. Svelte calls it
	// immediately with the current (hydrated) state, then on every revalidate.
	onMount(() => mixesQuery.subscribe((s) => {
		refreshing = s.refreshing;
		// 503 = TIDAL unreachable/disconnected. Check this BEFORE reading s.data: the
		// SWR error shape keeps the prior data alongside the error. Keep cached mixes
		// visible through a transient cold-boot blip (tokens not yet rehydrated); only
		// show the connect prompt when there's nothing to fall back on. Flag it so a
		// later connect transition forces a fresh fetch. Never drop the persisted copy
		// here - that would turn a transient 503 into a skeleton on the next launch.
		if (s.error instanceof ApiError && s.error.status === 503) {
			lastRefreshFailed = true;
			if (mixes.length === 0) viewState = 'disconnected';
			return;
		}
		if (s.data) {
			lastRefreshFailed = false;
			mixes = s.data.mixes ?? [];
			if (mixes.length > 0) viewState = 'ready';
			else if (!s.loading && !s.refreshing) viewState = 'empty';
			return;
		}
		if (s.error && !s.loading) {
			lastRefreshFailed = true;
			viewState = 'error';
			errorMsg = s.error instanceof Error ? s.error.message : 'Failed to load mixes';
			return;
		}
		if (s.loading) viewState = 'loading';
	}));

	// Re-fetch when TIDAL transitions to connected: the cold-boot race (initial
	// revalidate 503'd before tokens rehydrated) and live connect from Settings. Skip
	// when we're already showing fresh mixes (the init-time revalidate covers that),
	// but force a refresh if the last attempt failed. untrack so the effect only
	// re-runs on tidalStatus changes - reading viewState/mixes would loop.
	$effect(() => {
		if ($tidalStatus !== 'connected') return;
		const cur = untrack(() => viewState);
		const haveMixes = untrack(() => mixes).length > 0;
		const failed = untrack(() => lastRefreshFailed);
		if (cur !== 'ready' || !haveMixes || failed) {
			void mixesQuery.refresh().catch(() => {});
		}
	});

	function retry() {
		errorMsg = '';
		viewState = mixes.length > 0 ? 'ready' : 'loading';
		void mixesQuery.refresh().catch(() => {});
	}

	// Belt-and-suspenders: trust the server field but also check title/mix_type
	// client-side in case the binary is stale or Tidal changes their field names.
	function isMixVideo(mix: TidalMix): boolean {
		if (mix.is_video_mix) return true;
		const title = mix.title.toLowerCase();
		const mixType = mix.mix_type?.toLowerCase() ?? '';
		return title.includes('video mix') || mixType.includes('video');
	}

	function playMix(mix: TidalMix) {
		if (isMixVideo(mix)) {
			void goto(`/videos?mixId=${encodeURIComponent(mix.id)}&play=1`);
			return;
		}
		void playTidalMix(mix.id);
	}

</script>

{#snippet mixRail(items: TidalMix[])}
	<div class="mix-rail" use:wheelToHorizontal>
		{#each items as mix (mix.id)}
			<button
				type="button"
				class="mix-card"
				title={mix.sub_title ?? mix.title}
				aria-label={`${isMixVideo(mix) ? 'Play video mix' : 'Play mix'} ${mix.title}`}
				onclick={() => playMix(mix)}
			>
				<div class="art-wrap">
					<ArtworkImage
						className="art"
						src={mix.image_url}
						alt={mix.title}
						size={320}
						tint={true}
						fallbackText="MIX"
						decorative={true}
					/>
					<PlayOverlay
						position="corner"
						size="sm"
						label={`${isMixVideo(mix) ? 'Play video mix' : 'Play mix'} ${mix.title}`}
					/>
					{#if isMixVideo(mix)}
						<span class="video-badge">Video</span>
					{/if}
				</div>
				<div class="meta">
					<h3 class="title">{mix.title}</h3>
					{#if mix.sub_title}
						<p class="artist">{mix.sub_title}</p>
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

<section class="discovery-section rise-in-shelf" data-section="your-mixes" style={`--rise-index: ${index}`}>
	<div class="section-header">
		<div class="section-title-group">
			<p class="eyebrow">TIDAL</p>
			<h2>Music Mixes</h2>
		</div>
		{#if viewState === 'loading' || refreshing}
			<span class="loading-indicator">Loading…</span>
		{/if}
	</div>

	{#if viewState === 'loading'}
		{@render skeletonRail()}
	{:else if viewState === 'ready' && audioMixes.length > 0}
		{@render mixRail(audioMixes)}
	{:else if viewState === 'empty'}
		<p class="muted-line">TIDAL hasn't built mixes for you yet — keep listening.</p>
	{:else if viewState === 'disconnected'}
		<p class="muted-line">
			Connect TIDAL to see your personal mixes.
			<a class="inline-link" href="/settings#sources-tidal">Open settings</a>
		</p>
	{:else if viewState === 'error'}
		<p class="muted-line">
			Couldn't load your mixes{errorMsg ? `: ${errorMsg}` : '.'}
			<button class="inline-link" onclick={retry}>Retry</button>
		</p>
	{/if}
</section>

{#if viewState === 'ready' && videoMixes.length > 0}
	<section class="discovery-section rise-in-shelf" data-section="your-video-mixes" style={`--rise-index: ${index + 1}`}>
		<div class="section-header">
			<div class="section-title-group">
				<p class="eyebrow">TIDAL</p>
				<h2>Video Mixes</h2>
			</div>
		</div>
		{@render mixRail(videoMixes)}
	</section>
{/if}

<style>
	/* Mirrors the Trending shelf's visual language: borderless cards,
	   transparent background, hover-zoom on artwork, ellipsised meta.
	   Adapted to a horizontal rail (per the plan's carousel requirement)
	   instead of Trending's auto-fill grid. */

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

	/* Horizontal rail */
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

	/* Card — transparent, borderless, hover-lift via background only.
	   Width is locked with explicit min/max because flex-basis alone is
	   negotiable when one item has a large intrinsic image (the Daily
	   Discovery artwork was ballooning the first card before this). */
	.mix-card {
		flex: 0 0 180px;
		width: 180px;
		min-width: 180px;
		max-width: 180px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		background: none;
		border: 0;
		padding: 0;
		border-radius: var(--radius-md);
		text-align: left;
		scroll-snap-align: start;
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
	.video-badge {
		position: absolute;
		right: 8px;
		top: 8px;
		padding: 3px 7px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.62);
		color: #fff;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.04em;
		text-transform: uppercase;
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

	/* Skeleton — same shape, shimmering art block */
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

	/* Inline empty / error / disconnected states — no boxed tile, just a
	   muted line that sits in the rail's place. */
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
