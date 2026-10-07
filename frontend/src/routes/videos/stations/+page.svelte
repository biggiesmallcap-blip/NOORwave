<script lang="ts" module>
	import type { VideoStationsResponse as LineupResponse } from '$lib/api/client';
	// The last lineup, kept across visits so switching back to this tab
	// renders at once and refreshes quietly instead of popping in.
	let lastLineup: LineupResponse | null = null;
</script>

<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { api, type TidalSearchVideo, type VideoStationCard, type VideoStationsResponse } from '$lib/api/client';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import StationRow from '$lib/components/video/StationRow.svelte';
	import StationSpotlight from '$lib/components/video/StationSpotlight.svelte';
	import { SMALL_CATALOG, groupStations, numberStations, sceneFamilyLabel, stationMeta } from '$lib/components/video/stations';
	import { showToast } from '$lib/stores/toast';
	import { playVideoStation, videoSession, videoStationOnAir } from '$lib/stores/video_session';

	// The stations tab as a channel guide: one numbered row per station, each
	// with a filmstrip of what it would play. A frame starts the station from
	// that video.

	const POLL_MS = 3000;
	const POLL_LIMIT = 10;

	let data = $state<VideoStationsResponse | null>(lastLineup);
	let error = $state<string | null>(null);
	let starting = $state<string | null>(null);
	let timer: ReturnType<typeof setTimeout> | null = null;

	let grouped = $derived(groupStations(data?.stations ?? []));
	let numbers = $derived(numberStations(grouped.spotlight, grouped.rows));
	let empty = $derived(Boolean(data) && (data?.stations.length ?? 0) === 0);

	async function load(attempt = 0) {
		try {
			data = await api.getVideoStations();
			lastLineup = data;
			error = null;
		} catch {
			error = 'Could not load stations.';
			return;
		}
		if (data.building && attempt < POLL_LIMIT) {
			timer = setTimeout(() => void load(attempt + 1), POLL_MS);
		}
	}

	function isOnAir(card: VideoStationCard): boolean {
		return $videoSession.continuous && $videoStationOnAir === card.id;
	}

	async function start(card: VideoStationCard, startWith?: TidalSearchVideo) {
		if (starting) return;
		starting = card.id;
		try {
			if (!(await playVideoStation({ id: card.id, title: card.title }, { startWith }))) {
				showToast(`Nothing new to play in ${card.title}.`);
			}
		} catch {
			showToast('Could not start that station.', 'error');
		} finally {
			starting = null;
		}
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
	{#if error}
		<EmptyState title="Could not load stations" copy={error} />
	{:else if !data}
		<!-- First visit: row-shaped placeholders hold the page's shape, so
		     the lineup lands without a jump. -->
		<div class="placeholder-rows" aria-busy="true" aria-label="Loading stations">
			{#each Array(6) as _, index (index)}
				<div class="placeholder-row"><span></span><span></span></div>
			{/each}
		</div>
	{:else if empty && data.building}
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
			<StationSpotlight
				card={grouped.spotlight}
				number={numbers.get(grouped.spotlight.id) ?? '01'}
				busy={starting === grouped.spotlight.id}
				onAir={isOnAir(grouped.spotlight)}
				onplay={(card, startWith) => void start(card, startWith)}
			/>
		{/if}

		{#each grouped.rows as row (row.id)}
			<section class="group" aria-label={row.title}>
				<h3 class="group-label">{row.title}</h3>
				{#each row.stations as station, index (station.id)}
					{@const family = sceneFamilyLabel(row.stations, index)}
					{#if family}<h4 class="family-label">{family}</h4>{/if}
					<StationRow
						card={station}
						number={numbers.get(station.id) ?? ''}
						meta={stationMeta(station, row.stations)}
						busy={starting === station.id}
						onAir={isOnAir(station)}
						onplay={(card, startWith) => void start(card, startWith)}
					/>
				{/each}
			</section>
		{/each}

		{#if data.discovery_setting === 'off'}
			<p class="note">Video discovery is off, so stations won't grow. <a href="/settings?setting=video-discovery">Change it in Settings</a>.</p>
		{/if}
	{/if}
</div>

<style>
	/* Width, gutters and the bottom inset come from the video layout. */
	.stations-page {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: 18px;
	}
	.skeleton { padding: var(--space-4) 0; }

	.placeholder-rows {
		display: grid;
		gap: 2px;
	}
	.placeholder-row {
		display: grid;
		grid-template-columns: 220px minmax(0, 1fr);
		gap: 18px;
		padding: 10px 12px;
	}
	.placeholder-row span {
		height: 76px;
		border-radius: 8px;
		background: var(--bg-raised);
		animation: placeholder-pulse 1.4s ease-in-out infinite;
	}
	.placeholder-row span:first-child {
		height: 44px;
		align-self: center;
	}
	@keyframes placeholder-pulse {
		50% { opacity: 0.55; }
	}
	@media (max-width: 860px) {
		.placeholder-row { grid-template-columns: minmax(0, 1fr); }
	}

	/* A group is a small label over its channel rows. */
	.group {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: 2px;
		min-width: 0;
	}
	/* The section label from /search: small, uppercase, accent. */
	.group-label {
		margin: 6px 0 4px;
		padding: 0 12px;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 1.5px;
		color: var(--accent);
	}

	/* Explore's families: quieter than the group label above them. */
	.family-label {
		margin: 8px 0 2px;
		padding: 0 12px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
	}

	.note {
		padding: 0 12px;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}
</style>
