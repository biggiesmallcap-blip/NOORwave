<script lang="ts">
	import { onMount } from 'svelte';
	import { api, ApiError, type TidalMix } from '$lib/api/client';
	import { playTidalMix } from '$lib/stores/player';

	type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	let mixes = $state<TidalMix[]>([]);
	let viewState = $state<State>('loading');
	let errorMsg = $state<string>('');

	onMount(load);

	async function load() {
		viewState = 'loading';
		errorMsg = '';
		try {
			const data = await api.getTidalMixes();
			mixes = data.mixes ?? [];
			viewState = mixes.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (e instanceof ApiError && e.status === 503) {
				viewState = 'disconnected';
			} else {
				viewState = 'error';
				errorMsg = e instanceof Error ? e.message : 'Failed to load mixes';
			}
		}
	}

	// Translate vertical wheel scroll to horizontal scroll over the rail.
	// Wheel events only fire on the hovered element, so this naturally
	// activates only when the cursor is over the rail (per the user spec:
	// "wheel should only be usable after a hover on card").
	//
	// Only preventDefault when there's actually room to scroll horizontally
	// in the wheel direction — otherwise the page keeps scrolling vertically
	// at the edges, so the rail doesn't trap the user mid-page.
	function wheelToHorizontal(node: HTMLElement) {
		const onWheel = (e: WheelEvent) => {
			// Trackpads / native horizontal scroll devices already supply
			// deltaX; only intervene when the user is genuinely scrolling
			// vertically (deltaY dominant).
			if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;

			const max = node.scrollWidth - node.clientWidth;
			if (max <= 0) return; // Nothing to scroll.

			const goingRight = e.deltaY > 0;
			const atStart = node.scrollLeft <= 0;
			const atEnd = node.scrollLeft >= max - 1;
			if ((goingRight && atEnd) || (!goingRight && atStart)) return;

			e.preventDefault();
			node.scrollLeft += e.deltaY;
		};
		node.addEventListener('wheel', onWheel, { passive: false });
		return {
			destroy() {
				node.removeEventListener('wheel', onWheel);
			},
		};
	}
</script>

<section class="discovery-section" data-section="your-mixes">
	<div class="section-header">
		<div class="section-title-group">
			<p class="eyebrow">TIDAL</p>
			<h2>Your Mixes</h2>
		</div>
		{#if viewState === 'loading'}
			<span class="loading-indicator">Loading…</span>
		{/if}
	</div>

	{#if viewState === 'loading'}
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
	{:else if viewState === 'ready'}
		<div class="mix-rail" use:wheelToHorizontal>
			{#each mixes as mix (mix.id)}
				<button
					type="button"
					class="mix-card"
					title={mix.sub_title ?? mix.title}
					aria-label={`Play ${mix.title}`}
					onclick={() => void playTidalMix(mix.id)}
				>
					<div class="art-wrap">
						{#if mix.image_url}
							<div
								class="art"
								style="background-image: url('{mix.image_url}')"
							></div>
						{:else}
							<div class="art fallback">♫</div>
						{/if}
						<div class="play-overlay" aria-hidden="true">▶</div>
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
		font-size: 1.15rem;
		font-weight: 700;
		margin: 0;
	}
	.loading-indicator {
		font-size: 0.78rem;
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
		transition: background 140ms ease, border-color 140ms ease;
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

	.play-overlay {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: linear-gradient(180deg, rgba(0, 0, 0, 0.05) 0%, rgba(0, 0, 0, 0.55) 100%);
		opacity: 0;
		color: #fff;
		font-size: 28px;
		transition: opacity 160ms ease;
	}
	.mix-card:hover .play-overlay,
	.mix-card:focus-visible .play-overlay {
		opacity: 1;
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
		transition: transform 220ms ease;
	}
	.mix-card:hover .art {
		transform: scale(1.05);
	}
	.art.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 48px;
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
		font-size: 13.5px;
		font-weight: 600;
		color: var(--text-primary, #fff);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: 1.3;
	}
	.artist {
		margin: 0;
		font-size: 12px;
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
		font-size: 13px;
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
