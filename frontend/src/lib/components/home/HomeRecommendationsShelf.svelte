<script lang="ts">
	import { onMount } from 'svelte';
	import { ApiError, api, type ProviderRecommendationShelf } from '$lib/api/client';
	import { playTrackNow } from '$lib/stores/player';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';

	type State = 'hidden' | 'loading' | 'ready' | 'empty' | 'error';

	let shelves = $state<ProviderRecommendationShelf[]>([]);
	let viewState = $state<State>('hidden');
	let errorMsg = $state('');
	let failedArtwork = $state<Record<string, boolean>>({});

	onMount(() => {
		void load();
	});

	async function load() {
		errorMsg = '';
		try {
			const [lastfm, listenbrainz] = await Promise.allSettled([
				api.getLastfmStatus(),
				api.getListenBrainzStatus()
			]);
			const lastfmConnected = lastfm.status === 'fulfilled' && Boolean(lastfm.value.scrobbling);
			const listenbrainzConnected = listenbrainz.status === 'fulfilled' && Boolean(listenbrainz.value.scrobbling);
			if (!lastfmConnected && !listenbrainzConnected) {
				viewState = 'hidden';
				return;
			}

			viewState = 'loading';
			const response = await api.getHomeRecommendations();
			shelves = response.shelves ?? [];
			viewState = shelves.some((shelf) => shelf.items.length > 0) ? 'ready' : 'empty';
		} catch (err) {
			if (err instanceof ApiError && err.status === 404) {
				viewState = 'hidden';
				return;
			}
			viewState = 'error';
			errorMsg = err instanceof Error ? err.message : 'Recommendations could not be loaded.';
		}
	}

	function artworkFor(url: string | null): string | null {
		return upscaleTidalArtwork(url, 320);
	}

	function markArtworkFailed(key: string) {
		failedArtwork = { ...failedArtwork, [key]: true };
	}
</script>

{#if viewState === 'hidden'}
	<!-- Hidden until a profile integration is connected. -->
{:else if viewState === 'loading'}
	<section class="discovery-section" data-section="provider-recommendations">
		<div class="section-header">
			<div class="section-title-group">
				<p class="eyebrow">Connected profiles</p>
				<h2>Recommendations</h2>
			</div>
			<span class="loading-indicator">Loading...</span>
		</div>
		<div class="recommendation-rail" use:wheelToHorizontal>
			{#each [0, 1, 2, 3, 4, 5] as i (i)}
				<div class="recommendation-card skeleton">
					<div class="art skeleton-art"></div>
					<div class="meta">
						<div class="skeleton-line title-line"></div>
						<div class="skeleton-line artist-line"></div>
					</div>
				</div>
			{/each}
		</div>
	</section>
{:else if viewState === 'ready'}
	{#each shelves as shelf (shelf.provider)}
		<section class="discovery-section" data-section={`provider-recommendations-${shelf.provider}`}>
			<div class="section-header">
				<div class="section-title-group">
					<p class="eyebrow">{shelf.provider}</p>
					<h2>{shelf.title}</h2>
				</div>
				{#if shelf.status === 'error'}
					<button type="button" class="inline-link" onclick={load}>Retry</button>
				{/if}
			</div>

			{#if shelf.items.length > 0}
				<div class="recommendation-rail" use:wheelToHorizontal>
					{#each shelf.items as item (`${shelf.provider}-${item.local_track_id}`)}
						<button
							type="button"
							class="recommendation-card"
							aria-label={`Play ${item.title}`}
							onclick={() => void playTrackNow(item.local_track_id)}
						>
							<div class="art-wrap">
								{#if artworkFor(item.artwork_url) && !failedArtwork[`${shelf.provider}-${item.local_track_id}`]}
									<img
										class="art"
										src={artworkFor(item.artwork_url) ?? ''}
										alt=""
										onerror={() => markArtworkFailed(`${shelf.provider}-${item.local_track_id}`)}
									/>
								{:else}
									<div class="art fallback">{item.title.slice(0, 1).toUpperCase()}</div>
								{/if}
								<PlayOverlay position="center" size="md" label={`Play ${item.title}`} />
							</div>
							<div class="meta">
								<h3>{item.title}</h3>
								<p>{item.artist_name ?? 'Unknown artist'}</p>
								<span>{item.reason}</span>
							</div>
						</button>
					{/each}
				</div>
			{:else}
				<p class="muted-line">{shelf.message ?? 'No playable recommendations yet.'}</p>
			{/if}
		</section>
	{/each}
{:else if viewState === 'empty'}
	<section class="discovery-section" data-section="provider-recommendations-empty">
		<div class="section-header">
			<div class="section-title-group">
				<p class="eyebrow">Connected profiles</p>
				<h2>Recommendations</h2>
			</div>
		</div>
		<p class="muted-line">
			Connected profiles have no playable recommendations yet.
			<a class="inline-link" href="/settings?category=account">Open account settings</a>
		</p>
	</section>
{:else if viewState === 'error'}
	<section class="discovery-section" data-section="provider-recommendations-error">
		<div class="section-header">
			<div class="section-title-group">
				<p class="eyebrow">Connected profiles</p>
				<h2>Recommendations</h2>
			</div>
		</div>
		<p class="muted-line">
			{errorMsg}
			<button type="button" class="inline-link" onclick={load}>Retry</button>
		</p>
	</section>
{/if}

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

	.loading-indicator,
	.muted-line,
	.inline-link {
		font-size: var(--font-size-xs);
	}

	.loading-indicator,
	.muted-line {
		color: var(--text-muted);
	}

	.inline-link {
		border: 0;
		background: transparent;
		color: var(--accent);
		padding: 0;
		font: inherit;
		cursor: pointer;
	}

	.recommendation-rail {
		display: flex;
		gap: 14px;
		overflow-x: auto;
		padding-bottom: 8px;
		scroll-snap-type: x mandatory;
	}

	.recommendation-card {
		flex: 0 0 180px;
		width: 180px;
		min-width: 180px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		border: 1px solid transparent;
		border-radius: 8px;
		background: transparent;
		padding: 8px;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
		scroll-snap-align: start;
		transition: background var(--motion-base), border-color var(--motion-base);
	}

	.recommendation-card:hover,
	.recommendation-card:focus-visible {
		background: rgba(255, 255, 255, 0.04);
		border-color: var(--border-subtle);
	}

	.art-wrap {
		position: relative;
		aspect-ratio: 1;
		width: 100%;
		overflow: hidden;
		border-radius: 8px;
		background: var(--bg-elevated);
	}

	.art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.fallback {
		display: grid;
		place-items: center;
		color: var(--text-secondary);
		font-weight: var(--font-weight-bold);
		background: linear-gradient(135deg, rgba(255, 255, 255, 0.08), rgba(255, 255, 255, 0.02));
	}

	.meta {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.meta h3,
	.meta p,
	.meta span {
		margin: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.meta h3 {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}

	.meta p,
	.meta span {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
	}

	.skeleton {
		pointer-events: none;
	}

	.skeleton-art,
	.skeleton-line {
		border-radius: 8px;
		background: linear-gradient(90deg, rgba(255, 255, 255, 0.05), rgba(255, 255, 255, 0.12), rgba(255, 255, 255, 0.05));
		background-size: 200% 100%;
		animation: shimmer 1.4s infinite linear;
	}

	.title-line {
		width: 80%;
		height: 14px;
	}

	.artist-line {
		width: 60%;
		height: 12px;
	}

	@keyframes shimmer {
		from { background-position: 200% 0; }
		to { background-position: -200% 0; }
	}
</style>
