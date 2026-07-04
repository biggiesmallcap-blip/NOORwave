<script lang="ts">
	import { onMount } from 'svelte';
	import { cachedApi } from '$lib/cache/api_queries';
	import type { Track } from '$lib/api/client';
	import { currentTrack, isPlaying, playTracksInContext } from '$lib/stores/player';

	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import TrackRow from '$lib/components/TrackRow.svelte';

	const PAGE_SIZE = 50;

	let tracks = $state<Track[]>([]);
	let total = $state(0);
	let loading = $state(true);
	let loadingMore = $state(false);
	let error = $state<string | null>(null);

	let hasMore = $derived(tracks.length < total);

	async function load() {
		loading = true;
		error = null;
		try {
			const data = await cachedApi.getHistory(PAGE_SIZE, 0);
			tracks = data.tracks;
			total = data.total;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	async function loadMore() {
		if (loadingMore || !hasMore) return;
		loadingMore = true;
		try {
			const data = await cachedApi.getHistory(PAGE_SIZE, tracks.length);
			// Guard against a track sliding across page boundaries between requests
			// (a fresh listen can re-order the collapsed set) by dropping dupes.
			const seen = new Set(tracks.map((t) => t.id));
			tracks = [...tracks, ...data.tracks.filter((t) => !seen.has(t.id))];
			total = data.total;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loadingMore = false;
		}
	}

	async function playFrom(track: Track) {
		await playTracksInContext(
			tracks.map((t) => t.id),
			track.id,
		);
	}

	onMount(load);
</script>

<div class="history-page">
	<PageHeader title="Listening history" eyebrow="Recently played">
		{#snippet meta()}
			{#if total > 0}
				<span class="count">{total.toLocaleString()} tracks</span>
			{/if}
		{/snippet}
		{#snippet actions()}
			<button type="button" class="refresh" onclick={load} disabled={loading} aria-label="Refresh history"
				>↻</button
			>
		{/snippet}
	</PageHeader>

	{#if error}
		<EmptyState title="Couldn't load history" copy={error}>
			{#snippet actions()}
				<button type="button" class="refresh" onclick={load}>Retry</button>
			{/snippet}
		</EmptyState>
	{:else if loading}
		<p class="status">Loading history...</p>
	{:else if tracks.length === 0}
		<EmptyState title="Nothing played yet" copy="Play something and it'll show up here." />
	{:else}
		<ul class="track-list">
			{#each tracks as track (track.id)}
				<TrackRow
					{track}
					variant="art"
					isCurrent={$currentTrack?.id === track.id}
					isPlaying={$currentTrack?.id === track.id && $isPlaying}
					onRowClick={() => void playFrom(track)}
				/>
			{/each}
		</ul>

		{#if hasMore}
			<button type="button" class="load-more" onclick={loadMore} disabled={loadingMore}>
				{loadingMore ? 'Loading...' : 'Load more'}
			</button>
		{/if}
	{/if}
</div>

<style>
	.history-page {
		max-width: var(--content-width);
		margin: 0 auto;
		padding: var(--space-5) var(--space-5) var(--space-8);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

	.count {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
	}

	.track-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.status {
		color: var(--text-secondary);
		padding: var(--space-4) 0;
	}

	.refresh {
		all: unset;
		width: 34px;
		height: 34px;
		display: grid;
		place-items: center;
		border-radius: 999px;
		cursor: pointer;
		color: var(--text-secondary);
		background: var(--bg-surface);
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.refresh:hover:not(:disabled) {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.refresh:disabled {
		cursor: default;
		opacity: 0.5;
	}

	.load-more {
		all: unset;
		align-self: center;
		padding: 10px 20px;
		border-radius: 999px;
		cursor: pointer;
		color: var(--text-primary);
		background: var(--bg-surface);
		font-weight: var(--font-weight-semibold);
		transition: background var(--motion-fast);
	}

	.load-more:hover:not(:disabled) {
		background: var(--bg-hover);
	}

	.load-more:disabled {
		cursor: default;
		opacity: 0.6;
	}
</style>
