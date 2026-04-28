<script lang="ts">
	import { onMount } from 'svelte';
	import type { Unsubscriber } from 'svelte/store';
	import type { Snapshot } from './$types';
	import { api, type AnalyticsDashboard } from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { playTrackNow } from '$lib/stores/player';

	let dashboard = $state<AnalyticsDashboard | null>(null);
	let loading = $state(true);

	// Phase 5B — back/forward state via SvelteKit snapshot.
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: typeof window !== 'undefined' ? window.scrollY : 0 }),
		restore: (saved) => {
			requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }));
		}
	};
	let refreshing = $state(false);
	let error = $state<string | null>(null);
	let refreshedAt = $state<string | null>(null);
	let wsUnsubscribe: Unsubscriber | null = null;

	onMount(() => {
		wsUnsubscribe = wsMessages.subscribe((messages) => {
			const latest = messages.at(-1);
			if (!latest) return;
			if (latest.type === 'listen_history_updated' || latest.type === 'library_synced') {
				void refreshAnalytics();
			}
		});
		void refreshAnalytics();

		return () => {
			wsUnsubscribe?.();
		};
	});

	function formatCount(value: number): string {
		return value.toLocaleString();
	}

	function formatPercent(value: number): string {
		return `${Math.round(value * 100)}%`;
	}

	function formatDuration(value: number): string {
		if (!value || value <= 0) return '0m';
		const minutes = Math.floor(value / 60000);
		const hours = Math.floor(minutes / 60);
		if (hours > 0) return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`;
		return `${minutes}m`;
	}

	function formatListenStamp(value: string): string {
		const date = new Date(value);
		if (Number.isNaN(date.getTime())) return value;
		return date.toLocaleString([], {
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	async function refreshAnalytics() {
		if (!loading) refreshing = true;
		error = null;
		try {
			const response = await api.getAnalyticsDashboard(12, 8, 14);
			dashboard = response.dashboard;
			refreshedAt = new Date().toLocaleTimeString([], {
				hour: '2-digit',
				minute: '2-digit'
			});
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
		} finally {
			loading = false;
			refreshing = false;
		}
	}

	let overview = $derived(dashboard?.overview ?? null);
	let behavior = $derived(dashboard?.behavior ?? null);
</script>

<svelte:head>
	<title>Analytics | NOOR</title>
</svelte:head>

<div class="page-shell analytics-page animate-in">
	<PageHeader
		eyebrow="Analytics"
		title="Listening patterns, without the dashboard noise."
		subtitle="A clean read on recent sessions, top entities, and the taste signals shaping future discovery."
	>
		{#snippet actions()}
			<button class="btn btn-glass" onclick={refreshAnalytics} disabled={loading || refreshing}>
				{loading || refreshing ? 'Refreshing…' : 'Refresh'}
			</button>
		{/snippet}
		{#snippet meta()}
			{#if refreshedAt}
				<span class="eyebrow meta-time">Updated {refreshedAt}</span>
			{/if}
		{/snippet}
	</PageHeader>

	{#if error}
		<EmptyState title="Analytics could not load" copy={error}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={refreshAnalytics} disabled={loading || refreshing}>Retry</button>
			{/snippet}
		</EmptyState>
	{:else if loading && !dashboard}
		<EmptyState title="Loading analytics" copy="Pulling recent listening history and taste signals." />
	{:else if dashboard && overview && behavior}
		<section class="stat-grid">
			<MetricPair label="Tracks" value={formatCount(overview.tracks)} copy="Current library rows." />
			<MetricPair label="Listen rows" value={formatCount(behavior.total_listens)} copy="Stored listening sessions." />
			<MetricPair label="Completion" value={formatPercent(behavior.completion_rate)} copy="Sessions played through." />
			<MetricPair label="Listened time" value={formatDuration(behavior.total_listened_ms)} copy="Across the current reporting window." />
		</section>

		<section class="panel-grid">
			<section class="glass-panel panel">
				<SectionHeader eyebrow="History" title="Recent listens" subtitle="The latest sessions recorded by the playback pipeline." />
				{#if dashboard.recent_listens.length === 0}
					<EmptyState title="No listens yet" copy="Start playback and this history will begin to fill in." />
				{:else}
					<div class="stack scroll-list">
						{#each dashboard.recent_listens as listen}
							<div
								class="list-card interactive"
								role="button"
								tabindex="0"
								onclick={() => void playTrackNow(listen.track_id)}
								onkeydown={(e) => e.key === 'Enter' && void playTrackNow(listen.track_id)}
								oncontextmenu={(e) => openContextMenu(e, buildTrackMenu({ id: listen.track_id, title: listen.track_title, artist_name: listen.artist_name, album_title: listen.album_title }), listen.track_title)}
							>
								<div class="list-card-play" aria-hidden="true">▶</div>
								<div class="list-card-info">
									<h4>{listen.track_title}</h4>
									<p>{listen.artist_name ?? 'Unknown artist'}{listen.album_title ? ` · ${listen.album_title}` : ''}</p>
								</div>
								<div class="list-card-side">
									<span>{formatDuration(listen.duration_listened_ms)}</span>
									<span>{formatListenStamp(listen.started_at)}</span>
								</div>
							</div>
						{/each}
					</div>
				{/if}
			</section>

			<section class="glass-panel panel">
				<SectionHeader eyebrow="Behavior" title="Playback posture" subtitle="How the last two weeks have actually been used." />
				<div class="stack compact">
					<div class="behavior-row">
						<span>Average listen</span>
						<strong>{formatDuration(behavior.average_listen_ms)}</strong>
					</div>
					<div class="behavior-row">
						<span>Repeat-heavy tracks</span>
						<strong>{formatCount(behavior.repeat_track_count)}</strong>
					</div>
					<div class="behavior-row">
						<span>Active days</span>
						<strong>{formatCount(behavior.active_days)}</strong>
					</div>
					<div class="behavior-row">
						<span>Unique tracks reached</span>
						<strong>{formatCount(behavior.unique_tracks)}</strong>
					</div>
				</div>
			</section>
		</section>

		<section class="panel-grid">
			<section class="glass-panel panel">
				<SectionHeader eyebrow="Artists" title="Top artists" subtitle="Who has dominated the room most recently." />
				<div class="stack">
					{#each dashboard.top_artists as artist}
						<div class="list-card">
							<div>
								<h4>{artist.artist_name}</h4>
								<p>{formatCount(artist.unique_tracks)} unique tracks</p>
							</div>
							<div class="list-card-side">
								<strong>{formatCount(artist.listens)}</strong>
							</div>
						</div>
					{/each}
				</div>
			</section>

			<section class="glass-panel panel">
				<SectionHeader eyebrow="Tracks" title="Top tracks" subtitle="The cuts coming back around most often." />
				<div class="stack">
					{#each dashboard.top_tracks as track}
						<div class="list-card">
							<div>
								<h4>{track.title}</h4>
								<p>{track.artist_name ?? 'Unknown artist'}</p>
							</div>
							<div class="list-card-side">
								<strong>{formatCount(track.listens)}</strong>
							</div>
						</div>
					{/each}
				</div>
			</section>
		</section>

		<section class="glass-panel panel">
			<SectionHeader eyebrow="Taste" title="Genre profile" subtitle="What your recent history is already telling discovery." />
			<div class="genre-row">
				{#each dashboard.top_genres as genre}
					<span class="genre-pill">{genre.genre_name} · {formatCount(genre.listens)}</span>
				{/each}
			</div>
		</section>
	{/if}
</div>

<style>
	.panel {
		padding: 22px;
		display: flex;
		flex-direction: column;
		gap: 18px;
	}

	.meta-time {
		text-transform: none;
		letter-spacing: 0.04em;
	}

	.stack {
		display: flex;
		flex-direction: column;
	}

	.compact .behavior-row {
		padding: 8px 0;
	}

	.scroll-list {
		max-height: 420px;
		overflow-y: auto;
		overflow-x: hidden;
		scrollbar-width: thin;
		scrollbar-color: rgba(255, 255, 255, 0.12) transparent;
	}

	.list-card,
	.behavior-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 11px 0;
		border-bottom: 1px solid rgba(255, 255, 255, 0.06);
	}

	.list-card:last-child,
	.behavior-row:last-child {
		border-bottom: none;
		padding-bottom: 0;
	}

	.list-card.interactive {
		cursor: pointer;
		border-radius: 6px;
		transition: background 80ms ease;
	}

	.list-card.interactive:hover {
		background: rgba(255, 255, 255, 0.05);
	}

	.list-card.interactive:hover .list-card-play {
		opacity: 1;
	}

	.list-card-play {
		opacity: 0;
		width: 16px;
		flex-shrink: 0;
		font-size: 0.65rem;
		color: var(--accent-strong, #6366f1);
		transition: opacity 100ms ease;
		text-align: center;
	}

	.list-card-info {
		flex: 1;
		min-width: 0;
	}

	.list-card-info h4,
	.list-card > div:not(.list-card-play):not(.list-card-side) h4 {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-weight: 600;
	}

	.list-card-info p,
	.list-card > div:not(.list-card-play):not(.list-card-side) p {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-secondary);
	}

	.list-card-side {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 4px;
		flex-shrink: 0;
	}

	.list-card h4,
	.behavior-row strong {
		font-weight: 600;
	}

	.list-card p,
	.list-card-side span,
	.behavior-row span {
		color: var(--text-secondary);
	}

	.genre-row {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.genre-pill {
		padding: 8px 11px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-secondary);
	}

	@media (max-width: 760px) {
		.list-card,
		.behavior-row {
			flex-direction: column;
			align-items: flex-start;
		}

		.list-card-side {
			align-items: flex-start;
		}
	}
</style>
