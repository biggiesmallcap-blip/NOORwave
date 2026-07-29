<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { goBack } from '$lib/navigation/back';
	import { cachedApi } from '$lib/cache/api_queries';
	import {
		type ProviderRecommendationItem,
		type ProviderRecommendationShelf,
	} from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { playChartTidalTrack } from '$lib/player/play_trending';
	import { playRecommendationAlbum, playRecommendationArtist } from '$lib/player/play_recommendations';
	import { playTrackNow } from '$lib/stores/player';
	import { recommendationEntity } from '$lib/components/home/recommendation_navigation';
	import {
		matchesRecommendationShelfSlug,
		openRecommendationItem,
		recommendationItemMenu,
		recommendationItemToTidalPlayable,
	} from '$lib/components/home/recommendation_menu';

	// The full set behind a Home shelf, as a grid.
	//
	// The shelf keeps a soft cap because a rail is for skimming; past a couple of
	// dozen cards, sideways scrolling stops being a way to see anything. A grid
	// shows fifty at once and needs no scrolling trick to do it.
	//
	// No new endpoint: /api/home/recommendations already returns every shelf in
	// full and is cached, so arriving here from Home is usually free.
	const slug = $derived(page.params.shelf ?? '');

	let shelf = $state<ProviderRecommendationShelf | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let resolving = $state<Record<string, boolean>>({});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		error = null;
		try {
			const res = await cachedApi.getHomeRecommendations();
			const found = (res.shelves ?? []).find((s) => matchesRecommendationShelfSlug(s, slug));
			if (!found) {
				error = 'That recommendation shelf is not available right now.';
			} else {
				shelf = found;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load recommendations.';
		} finally {
			loading = false;
		}
	}

	const entity = $derived(shelf ? recommendationEntity(shelf.items[0] ?? ({} as never)) : 'track');
	const isArtist = $derived(entity === 'artist');

	// Mirrors the shelf's own subtitles so the page reads as the same thing,
	// with the count appended since that is what this view adds.
	function subtitleFor(s: ProviderRecommendationShelf): string {
		const count = `${s.items.length} item${s.items.length === 1 ? '' : 's'}`;
		if (s.provider === 'lastfm' && s.entity_type === 'artist') {
			return `Artists similar to your Last.fm profile. ${count}.`;
		}
		if (s.provider === 'lastfm' && s.entity_type === 'album') {
			return `Albums from artists near your Last.fm taste. ${count}.`;
		}
		if (s.provider === 'lastfm') {
			return `Based on your Last.fm loved, recent, and top tracks. ${count}.`;
		}
		return `${count}.`;
	}

	function itemKey(item: ProviderRecommendationItem, index: number): string {
		return `${item.tidal_id ?? item.local_track_id ?? item.mbid ?? item.title}:${index}`;
	}

	function fallbackText(item: ProviderRecommendationItem): string {
		return (item.title.trim()[0] ?? 'N').toUpperCase();
	}

	// Same dispatch the shelf uses: tracks play, artists and albums open.
	async function activate(item: ProviderRecommendationItem) {
		const kind = recommendationEntity(item);
		const key = item.title;
		if (resolving[key]) return;
		resolving = { ...resolving, [key]: true };
		try {
			if (kind === 'artist') {
				await playRecommendationArtist(item);
			} else if (kind === 'album') {
				await playRecommendationAlbum(item);
			} else if (item.local_track_id) {
				await playTrackNow(item.local_track_id);
			} else {
				await playChartTidalTrack(recommendationItemToTidalPlayable(item));
			}
		} finally {
			resolving = { ...resolving, [key]: false };
		}
	}

	function openMenu(event: MouseEvent, item: ProviderRecommendationItem) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, recommendationItemMenu(item), item.title);
	}
</script>

<svelte:head>
	<title>{shelf?.title ? `${shelf.title} — NOOR` : 'Recommendations — NOOR'}</title>
</svelte:head>

<!-- Structure copied from TidalEditorialPage rather than invented: same page
     shell and padding, the same back link, the shared PageHeader, and
     `muted-line` for every non-ready state. A detail page reached from Home
     should be indistinguishable in chrome from /hires or /new-releases. -->
<div class="page" data-recommendation-shelf={slug}>
	<button class="back-link" type="button" onclick={() => goBack('/')}>&lt; Back</button>
	<PageHeader
		eyebrow="Connected profiles"
		title={shelf?.title ?? (loading ? 'Loading...' : 'Recommendations')}
		subtitle={shelf ? subtitleFor(shelf) : ''}
		variant="editorial"
	/>

	{#if loading}
		<p class="muted-line">Loading recommendations...</p>
	{:else if error}
		<p class="muted-line">{error} <button class="inline-link" onclick={() => void load()}>Retry</button></p>
	{:else if !shelf || shelf.items.length === 0}
		<p class="muted-line">Nothing in this shelf yet.</p>
	{:else}
		<div class="rec-grid" class:artists={isArtist}>
			{#each shelf.items as item, index (itemKey(item, index))}
				<button
					type="button"
					class="rec-tile rise-in-card"
					style={`--rise-index: ${index % 12}`}
					title={item.title}
					aria-label={item.title}
					onclick={() => void activate(item)}
					oncontextmenu={(e) => openMenu(e, item)}
				>
					<span class="rec-tile-art" class:round={isArtist}>
						<ArtworkImage
							className="rec-tile-img"
							src={item.artwork_url}
							alt={item.title}
							size={320}
							fadeIn={true}
							fallbackText={fallbackText(item)}
							decorative={true}
						/>
						<PlayOverlay position="corner" size="sm" />
					</span>
					<span class="rec-tile-title">{item.title}</span>
					{#if !isArtist && item.artist_name}
						<span class="rec-tile-sub">{item.artist_name}</span>
					{/if}
					{#if isArtist}
						<span class="rec-tile-sub">Artist</span>
					{/if}
				</button>
			{/each}
		</div>
	{/if}
</div>

<style>
	/* Same shell, padding and rhythm as TidalEditorialPage. */
	.page {
		max-width: var(--content-width);
		margin: 0 auto;
		padding: var(--space-5) var(--space-4) var(--space-7);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

	.back-link {
		align-self: flex-start;
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		cursor: pointer;
	}
	.back-link:hover,
	.back-link:focus-visible { color: var(--text-primary); }

	.muted-line {
		margin: 0;
		color: var(--text-secondary);
	}

	.inline-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: var(--accent-strong);
		cursor: pointer;
		text-decoration: underline;
	}

	/* Auto-fill rather than a fixed count: the point of this page is that
	   everything is visible at once, so the grid should use whatever width the
	   window gives it. */
	.rec-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: var(--space-3);
	}
	.rec-grid.artists {
		grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
	}

	.rec-tile {
		display: flex;
		flex-direction: column;
		gap: 6px;
		background: none;
		border: none;
		padding: 0;
		text-align: left;
		color: inherit;
		cursor: pointer;
		min-width: 0;
	}

	.rec-tile-art {
		position: relative;
		width: 100%;
		aspect-ratio: 1 / 1;
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--bg-raised);
		transition: box-shadow var(--motion-base);
	}
	.rec-tile-art.round { border-radius: 50%; }
	.rec-tile:hover .rec-tile-art { box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5); }
	.rec-tile:hover :global(.play-overlay) { opacity: 1; transform: translateY(0); }

	.rec-tile :global(.rec-tile-img),
	.rec-tile :global(img.rec-tile-img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.rec-tile-title, .rec-tile-sub {
		width: 100%;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: var(--line-height-snug);
	}
	.rec-tile-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}
	.rec-tile-sub {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
	}
</style>
