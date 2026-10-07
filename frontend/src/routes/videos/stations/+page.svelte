<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { api, type VideoStationCard, type VideoStationsResponse } from '$lib/api/client';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import StationCard from '$lib/components/video/StationCard.svelte';
	import VideoNavigation from '$lib/components/video/VideoNavigation.svelte';
	import { SMALL_CATALOG, groupStations, spotlightArtistId } from '$lib/components/video/stations';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { showToast } from '$lib/stores/toast';
	import { playVideoStation } from '$lib/stores/video_session';

	const POLL_MS = 3000;
	const POLL_LIMIT = 10;

	let data = $state<VideoStationsResponse | null>(null);
	let error = $state<string | null>(null);
	let starting = $state<string | null>(null);
	let timer: ReturnType<typeof setTimeout> | null = null;

	let grouped = $derived(groupStations(data?.stations ?? []));
	let empty = $derived(Boolean(data) && (data?.stations.length ?? 0) === 0);

	async function load(attempt = 0) {
		try {
			data = await api.getVideoStations();
			error = null;
		} catch {
			error = 'Could not load stations.';
			return;
		}
		if (data.building && attempt < POLL_LIMIT) {
			timer = setTimeout(() => void load(attempt + 1), POLL_MS);
		}
	}

	async function start(card: VideoStationCard) {
		if (starting) return;
		starting = card.id;
		try {
			if (!(await playVideoStation({ id: card.id, title: card.title }))) {
				showToast(`Nothing new to play in ${card.title}.`);
			}
		} catch {
			showToast('Could not start that station.', 'error');
		} finally {
			starting = null;
		}
	}

	function artistMenu(event: MouseEvent, card: VideoStationCard) {
		const tidalId = spotlightArtistId(card);
		if (tidalId == null) return;
		event.preventDefault();
		openContextMenu(event, buildArtistMenu({ tidal_id: tidalId, name: card.title, in_library: false }, { isLocal: false }), card.title);
	}

	onMount(() => void load());
	onDestroy(() => {
		if (timer) clearTimeout(timer);
	});
</script>

<svelte:head>
	<title>Video stations - NOOR</title>
</svelte:head>

<div class="page">
	<header class="stations-header">
		<VideoNavigation current="stations" />
	</header>

	{#if error}
		<EmptyState title="Could not load stations" copy={error} />
	{:else if !data || (empty && data.building)}
		<div class="skeleton"><Skeleton rows={3} label="Building your stations" /></div>
	{:else if empty && data.catalog_videos < SMALL_CATALOG}
		<EmptyState
			title="Stations need a bigger video catalog"
			copy="Video discovery finds music videos in the background. Stations appear once a few hundred are indexed."
		>
			{#snippet actions()}
				<a class="btn btn-glass" href="/settings?setting=video-discovery">Video discovery settings</a>
			{/snippet}
		</EmptyState>
	{:else if empty}
		<EmptyState title="No stations today" copy="Nothing has enough videos you haven't seen yet. Check back tomorrow." />
	{:else}
		{#if grouped.spotlight}
			{@const spot = grouped.spotlight}
			<section class="spotlight" aria-label="Today's spotlight">
				<button type="button" class="spotlight-art" aria-label={`Play ${spot.title} station`} onclick={() => void start(spot)}>
					<ArtworkImage src={spot.preview[0]?.artwork_url ?? null} size={640} fallbackText="VID" decorative={true} fadeIn={true} />
				</button>
				<div class="spotlight-copy">
					<span class="eyebrow">Today's spotlight</span>
					<button type="button" class="spotlight-name" oncontextmenu={(event) => artistMenu(event, spot)} onclick={() => void start(spot)}>{spot.title}</button>
					<span class="subtitle">{spot.subtitle}</span>
					<button type="button" class="btn btn-primary" disabled={starting === spot.id} onclick={() => void start(spot)}>Play station</button>
				</div>
			</section>
		{/if}

		{#each grouped.rows as row (row.id)}
			<section class="row" aria-label={row.title}>
				<h2>{row.title}</h2>
				<div class="grid">
					{#each row.stations as card (card.id)}
						<StationCard {card} busy={starting === card.id} onplay={(c) => void start(c)} />
					{/each}
				</div>
			</section>
		{/each}

		{#if data.discovery_setting === 'off'}
			<p class="note">Video discovery is off, so stations won't grow. <a href="/settings?setting=video-discovery">Change it in Settings</a>.</p>
		{/if}
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
		padding: var(--space-5) var(--space-5) var(--space-7);
		width: 100%;
		max-width: var(--content-width);
		margin: 0 auto;
	}
	.stations-header { padding: 0 4px; }
	.skeleton { padding: var(--space-4) 0; }
	.spotlight {
		display: grid;
		grid-template-columns: minmax(0, 360px) minmax(0, 1fr);
		gap: var(--space-5);
		align-items: center;
	}
	.spotlight-art {
		padding: 0;
		border: 0;
		background: transparent;
		border-radius: 12px;
		overflow: hidden;
		aspect-ratio: 16 / 9;
		cursor: pointer;
	}
	.spotlight-art :global(img) { width: 100%; height: 100%; object-fit: cover; }
	.spotlight-copy { display: grid; gap: 6px; justify-items: start; }
	.eyebrow { font-size: var(--font-size-sm); color: var(--text-secondary); }
	.spotlight-name {
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		text-align: left;
	}
	.subtitle { color: var(--text-secondary); }
	.row { display: grid; gap: var(--space-3); }
	.row h2 { margin: 0; font-size: var(--font-size-lg); color: var(--text-primary); }
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: var(--space-4);
	}
	.note { color: var(--text-secondary); font-size: var(--font-size-sm); }
	@media (max-width: 720px) {
		.spotlight { grid-template-columns: 1fr; }
	}
</style>
