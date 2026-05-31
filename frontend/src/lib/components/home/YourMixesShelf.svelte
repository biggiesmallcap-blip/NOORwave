<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { ApiError, type TidalMix } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { playTidalMix } from '$lib/stores/player';
	import { tidalStatus } from '$lib/stores/tidal';
	import { getCachedMixes, putCachedMixes, clearCachedMixes } from '$lib/stores/tidal-mixes-cache';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	const cachedOnMount = getCachedMixes();
	let mixes = $state<TidalMix[]>(cachedOnMount ?? []);
	let viewState = $state<State>(cachedOnMount && cachedOnMount.length > 0 ? 'ready' : 'loading');
	let errorMsg = $state<string>('');

	let audioMixes = $derived(mixes.filter((m) => !isMixVideo(m)));
	let videoMixes = $derived(mixes.filter((m) => isMixVideo(m)));

	// Skip the network on remount when we have warm cached mixes — the shelf
	// renders synchronously from cache and stays static for the 6h TTL window.
	onMount(() => {
		if (cachedOnMount && cachedOnMount.length > 0) return;
		void load();
	});

	// Re-fetch when TIDAL transitions to connected. Covers two cases:
	//   1. Cold-boot race — shelf mounted and 503'd before tidal_status had
	//      rehydrated tokens from disk; without this it stayed "disconnected"
	//      until the user navigated away and back.
	//   2. Live connect — user opens Settings, connects TIDAL, returns home;
	//      the shelf updates in place instead of needing a refresh.
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
			const data = await cachedApi.getTidalMixes();
			mixes = data.mixes ?? [];
			if (mixes.length > 0) putCachedMixes(mixes);
			viewState = mixes.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (e instanceof ApiError && e.status === 503) {
				clearCachedMixes();
				viewState = 'disconnected';
			} else {
				viewState = 'error';
				errorMsg = e instanceof Error ? e.message : 'Failed to load mixes';
			}
		}
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
					{#if mix.image_url}
						<div class="art" style="background-image: url('{mix.image_url}')"></div>
					{:else}
						<div class="art fallback">♫</div>
					{/if}
					<PlayOverlay
						position="center"
						size="md"
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

<section class="discovery-section" data-section="your-mixes">
	<div class="section-header">
		<div class="section-title-group">
			<p class="eyebrow">TIDAL</p>
			<h2>Music Mixes</h2>
		</div>
		{#if viewState === 'loading'}
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
			<button class="inline-link" onclick={load}>Retry</button>
		</p>
	{/if}
</section>

{#if viewState === 'ready' && videoMixes.length > 0}
	<section class="discovery-section" data-section="your-video-mixes">
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
	.video-badge {
		position: absolute;
		right: 8px;
		bottom: 8px;
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
