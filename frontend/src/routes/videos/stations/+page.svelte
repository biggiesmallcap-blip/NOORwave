<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { api, type VideoStationCard, type VideoStationsResponse } from '$lib/api/client';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import StationCard from '$lib/components/video/StationCard.svelte';
	import VideoBackLink from '$lib/components/video/VideoBackLink.svelte';
	import VideoNavigation from '$lib/components/video/VideoNavigation.svelte';
	import { SMALL_CATALOG, groupStations, previewArtists, spotlightArtistId, stationMeta } from '$lib/components/video/stations';
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

<div class="stations-page">
	<VideoBackLink current="stations" />
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
			{@const lead = spot.preview.find((video) => video.artwork_url)?.artwork_url ?? null}
			{@const artists = previewArtists(spot, 4)}
			<section class="spotlight" aria-label="Today's spotlight">
				<div class="spotlight-backdrop" aria-hidden="true">
					<ArtworkImage src={lead} size={320} fallbackText="" decorative={true} fadeIn={true} />
				</div>
				<button type="button" class="spotlight-art" aria-label={`Play ${spot.title} station`} onclick={() => void start(spot)}>
					<ArtworkImage src={lead} size={1080} fallbackText="VID" decorative={true} fadeIn={true} />
				</button>
				<div class="spotlight-copy">
					<span class="eyebrow">Today's spotlight</span>
					<button type="button" class="spotlight-name" oncontextmenu={(event) => artistMenu(event, spot)} onclick={() => void start(spot)}>{spot.title}</button>
					<span class="spotlight-sub">{spot.subtitle}</span>
					{#if artists.length > 1}
						<span class="spotlight-with">With {artists.slice(1).join(', ')}</span>
					{/if}
					<div class="spotlight-actions">
						<button type="button" class="btn btn-primary" disabled={starting === spot.id} onclick={() => void start(spot)}>
							{starting === spot.id ? 'Starting...' : 'Play station'}
						</button>
					</div>
				</div>
			</section>
		{/if}

		{#each grouped.rows as row (row.id)}
			<section class="row" aria-label={row.title}>
				<header class="row-head">
					<h2>{row.title}</h2>
					<span class="row-count">{row.stations.length} {row.stations.length === 1 ? 'station' : 'stations'}</span>
				</header>
				<MediaRail items={row.stations} getKey={(station) => station.id} ariaLabel={row.title}>
					{#snippet card(station)}
						<div class="rail-card">
							<StationCard
								card={station}
								meta={stationMeta(station, row.stations)}
								busy={starting === station.id}
								onplay={(c) => void start(c)}
							/>
						</div>
					{/snippet}
				</MediaRail>
			</section>
		{/each}

		{#if data.discovery_setting === 'off'}
			<p class="note">Video discovery is off, so stations won't grow. <a href="/settings?setting=video-discovery">Change it in Settings</a>.</p>
		{/if}
	{/if}
</div>

<style>
	/* Same frame as /videos: content width, and a minmax(0, 1fr) column so
	   rails scroll instead of widening the page. */
	.stations-page {
		width: min(100%, var(--content-width));
		margin: 0 auto;
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: 28px;
		padding: 0 4px max(var(--bottom-player-height, 0px), 44px, var(--safe-bottom));
	}
	.stations-header {
		padding-top: var(--space-2);
	}
	.skeleton { padding: var(--space-4) 0; }

	/* Spotlight: a wide hero over a blurred wash of its own artwork. */
	.spotlight {
		position: relative;
		display: grid;
		grid-template-columns: minmax(0, 1.15fr) minmax(0, 1fr);
		gap: clamp(20px, 3vw, 40px);
		align-items: center;
		padding: clamp(16px, 2.4vw, 28px);
		border: 1px solid var(--border-subtle);
		border-radius: 16px;
		overflow: hidden;
		isolation: isolate;
	}
	.spotlight-backdrop {
		position: absolute;
		inset: 0;
		z-index: -1;
		opacity: 0.3;
		filter: blur(48px) saturate(1.3);
		transform: scale(1.2);
		pointer-events: none;
	}
	.spotlight-backdrop :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.spotlight-art {
		padding: 0;
		border: 0;
		background: var(--bg-raised);
		border-radius: 12px;
		overflow: hidden;
		aspect-ratio: 16 / 9;
		cursor: pointer;
		box-shadow: 0 18px 40px rgba(0, 0, 0, 0.35);
		transition: transform var(--motion-fast);
	}
	.spotlight-art:hover,
	.spotlight-art:focus-visible { transform: translateY(-2px); }
	.spotlight-art:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
	.spotlight-art :global(img) { width: 100%; height: 100%; object-fit: cover; }
	.spotlight-copy {
		display: grid;
		gap: 8px;
		justify-items: start;
		min-width: 0;
	}
	/* The section label from /search: small, uppercase, accent. */
	.eyebrow {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 1.5px;
		color: var(--accent);
	}
	.spotlight-name {
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-align: left;
		cursor: pointer;
		overflow-wrap: anywhere;
	}
	.spotlight-name:hover { text-decoration: underline; text-underline-offset: 4px; }
	.spotlight-sub { color: var(--text-secondary); }
	.spotlight-with {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}
	.spotlight-actions { margin-top: var(--space-2); }

	/* Rows: the shelf heading used on /videos, cards on a horizontal rail. */
	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: 10px;
		min-width: 0;
	}
	.row-head {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		padding: 0 2px;
	}
	.row-head h2 {
		margin: 0;
		font-size: var(--font-size-lg);
		color: var(--text-primary);
	}
	.row-count {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}
	.rail-card {
		flex: 0 0 auto;
		width: clamp(220px, 21vw, 300px);
		scroll-snap-align: start;
	}

	.note { color: var(--text-secondary); font-size: var(--font-size-sm); }

	@media (max-width: 860px) {
		.spotlight { grid-template-columns: 1fr; }
		.spotlight-name { font-size: var(--font-size-2xl); }
		.rail-card { width: 64vw; }
	}
</style>
