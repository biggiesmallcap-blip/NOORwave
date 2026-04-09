<script lang="ts">
	import { onMount } from 'svelte';
	import {
		api,
		getApiBase,
		type AnalyticsDashboard,
		type ListenHistoryEntry,
		type AnalyticsTopArtist
	} from '$lib/api/client';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import { wsConnected } from '$lib/api/ws';
	import { currentTrack, isPlaying, playbackQueue } from '$lib/stores/player';
	import { formatDuration } from '$lib/stores/library';

	let dashboard = $state<AnalyticsDashboard | null>(null);
	let runtime = $state<{ available: boolean; device_name: string | null }>({
		available: false,
		device_name: null
	});
	let status = $state<{ name: string; version: string; status: string } | null>(null);
	let tidalStatus = $state<'connected' | 'disconnected' | 'unknown'>('unknown');
	let loading = $state(true);
	let error = $state<string | null>(null);

	onMount(() => {
		void loadHome();
	});

	async function loadHome() {
		loading = true;
		error = null;

		try {
			const [statusData, dashboardData, runtimeData] = await Promise.all([
				api.getStatus(),
				api.getAnalyticsDashboard(6, 5, 14),
				api.getPlaybackRuntime()
			]);

			status = statusData;
			dashboard = dashboardData.dashboard;
			runtime = {
				available: runtimeData.available,
				device_name: runtimeData.runtime?.device_name ?? null
			};

			try {
				const tidalResponse = await fetch(`${getApiBase()}/api/tidal/status`);
				if (!tidalResponse.ok) throw new Error(`Server returned ${tidalResponse.status}`);
				const tidalData = await tidalResponse.json();
				tidalStatus = tidalData.connected ? 'connected' : 'disconnected';
			} catch {
				tidalStatus = 'unknown';
			}
		} catch {
			error = 'NOOR could not reach the local server on port 3333.';
		} finally {
			loading = false;
		}
	}

	function formatListenStamp(value: string) {
		return new Date(value).toLocaleString(undefined, {
			month: 'short',
			day: 'numeric',
			hour: 'numeric',
			minute: '2-digit'
		});
	}

	let recentListens = $derived<ListenHistoryEntry[]>(dashboard?.recent_listens ?? []);
	let topArtists = $derived<AnalyticsTopArtist[]>(dashboard?.top_artists ?? []);
	let genreCoverage = $derived(
		dashboard && dashboard.overview.tracks > 0
			? `${Math.round((dashboard.overview.tagged_tracks / dashboard.overview.tracks) * 100)}%`
			: '0%'
	);
</script>

<svelte:head>
	<title>NOOR</title>
</svelte:head>

<div class="page-shell home-page animate-in">
	<section class="page-header">
		<p class="eyebrow">NOOR</p>
		<h1>Your music, fully in command.</h1>
	</section>

	{#if loading}
		<EmptyState title="Loading dashboard" copy="Pulling listening trends, runtime state, and library totals." />
	{:else if error}
		<EmptyState title="NOOR is offline" copy={error}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={loadHome}>Try again</button>
			{/snippet}
		</EmptyState>
	{:else if dashboard}
		<section class="stat-strip">
			<MetricPair label="Tracks" value={dashboard.overview.tracks.toLocaleString()} />
			<MetricPair label="Albums" value={dashboard.overview.albums.toLocaleString()} />
			<MetricPair label="Artists" value={dashboard.overview.artists.toLocaleString()} />
			<MetricPair label="Genre coverage" value={genreCoverage} />
		</section>

		{#if $currentTrack}
			<section class="glass-panel now-panel">
				<div class="now-left">
					{#if $currentTrack.artwork_url}
						<img class="now-art" src={$currentTrack.artwork_url} alt="" />
					{:else}
						<div class="now-art placeholder">♫</div>
					{/if}
					<div class="now-meta">
						<p class="eyebrow">{$isPlaying ? 'Now playing' : 'Paused'}</p>
						<h3>{$currentTrack.title}</h3>
						<span>{$currentTrack.artist_name ?? 'Unknown artist'}</span>
					</div>
				</div>
				{#if $playbackQueue.length > 1}
					{@const upNext = $playbackQueue.filter(q => q.track.id !== $currentTrack?.id).slice(0, 3)}
					{#if upNext.length > 0}
						<div class="up-next">
							<p class="eyebrow">Up next</p>
							{#each upNext as item (item.id)}
								<div class="up-next-row">
									<span class="up-next-title">{item.track.title}</span>
									<span class="up-next-artist">{item.track.artist_name ?? ''}</span>
									<span class="up-next-dur">{formatDuration(item.track.duration_ms)}</span>
								</div>
							{/each}
						</div>
					{/if}
				{/if}
			</section>
		{/if}

		<section class="panel-grid activity-grid">
			<section class="glass-panel list-panel">
				<div class="section-head">
					<p class="eyebrow">Listening now</p>
					<h2>Recently played</h2>
				</div>

				{#if recentListens.length > 0}
					<div class="list-stack">
						{#each recentListens as entry (entry.id)}
							<div class="listen-row">
								{#if entry.artwork_url}
									<img class="listen-art" src={entry.artwork_url} alt="" />
								{:else}
									<div class="listen-art placeholder">♫</div>
								{/if}

								<div class="listen-meta">
									<p>{entry.track_title}</p>
									<span>{entry.artist_name ?? 'Unknown artist'}</span>
								</div>

								<span class="listen-stamp">{formatListenStamp(entry.started_at)}</span>
							</div>
						{/each}
					</div>
				{:else}
					<EmptyState title="No listens yet" copy="Start playback and the recent timeline will populate here." />
				{/if}
			</section>

			<section class="glass-panel list-panel">
				<div class="section-head">
					<p class="eyebrow">Last 14 days</p>
					<h2>This fortnight</h2>
				</div>

				{#if topArtists.length > 0}
					<div class="list-stack">
						{#each topArtists as artist (artist.artist_id)}
							<div class="artist-row">
								<div class="artist-meta">
									<p>{artist.artist_name}</p>
									<span>{artist.unique_tracks} unique tracks</span>
								</div>
								<strong>{artist.listens} listens</strong>
							</div>
						{/each}
					</div>
				{:else}
					<EmptyState title="No artist trends yet" copy="Give NOOR a little listening history and this fortnight view will start to read clearly." />
				{/if}
			</section>
		</section>

		<section class="glass-panel landscape-panel">
			<div class="section-head">
				<p class="eyebrow">Listening landscape</p>
				<h2>Genre landscape</h2>
			</div>

			{#if dashboard.top_genres.length > 0}
				<div class="genre-row">
					{#each dashboard.top_genres as genre}
						<span class="genre-pill">{genre.genre_name} · {genre.listens}</span>
					{/each}
				</div>
			{:else}
				<EmptyState title="Genre signals are still thin" copy="Run MusicBrainz enrichment to unlock a richer landscape view." />
			{/if}
		</section>

		<section class="system-row">
			<StateBadge label={status ? `Server v${status.version}` : 'Server unknown'} tone={status ? 'success' : 'muted'} />
			<StateBadge
				label={tidalStatus === 'connected' ? 'TIDAL connected' : tidalStatus === 'disconnected' ? 'TIDAL offline' : 'TIDAL unknown'}
				tone={tidalStatus === 'connected' ? 'active' : tidalStatus === 'disconnected' ? 'muted' : 'warning'}
			/>
			<StateBadge label={$wsConnected ? 'WS live' : 'WS offline'} tone={$wsConnected ? 'success' : 'muted'} />
			<StateBadge
				label={runtime.available ? (runtime.device_name ?? 'Runtime active') : 'Runtime idle'}
				tone={runtime.available ? 'active' : 'muted'}
			/>
		</section>
	{/if}
</div>

<style>
	.home-page {
		gap: var(--space-5);
	}

	.page-header {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding-top: 4px;
	}

	.page-header h1 {
		max-width: 12ch;
	}

	.stat-strip {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: var(--space-3);
	}

	.now-panel {
		padding: 16px 20px;
		display: flex;
		align-items: center;
		gap: 24px;
		flex-wrap: wrap;
	}

	.now-left {
		display: flex;
		align-items: center;
		gap: 14px;
		flex: 1;
		min-width: 200px;
	}

	.now-art {
		width: 52px;
		height: 52px;
		border-radius: 8px;
		object-fit: cover;
		flex-shrink: 0;
	}

	.now-art.placeholder {
		background: var(--accent-soft);
		display: grid;
		place-items: center;
		color: var(--accent-strong);
		font-size: 1.2rem;
	}

	.now-meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.now-meta h3 {
		font-size: 0.95rem;
		font-weight: 700;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		margin: 0;
	}

	.now-meta span {
		font-size: 0.8rem;
		color: var(--text-muted);
	}

	.up-next {
		display: flex;
		flex-direction: column;
		gap: 6px;
		flex: 1;
		min-width: 180px;
	}

	.up-next .eyebrow {
		margin-bottom: 2px;
	}

	.up-next-row {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 0.8rem;
	}

	.up-next-title {
		flex: 1;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		color: var(--text-primary);
	}

	.up-next-artist {
		color: var(--text-muted);
		flex-shrink: 0;
		max-width: 120px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.up-next-dur {
		color: var(--text-muted);
		flex-shrink: 0;
	}

	.activity-grid {
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
	}

	.list-panel,
	.landscape-panel {
		padding: 20px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.section-head {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.section-head h2 {
		font-size: 1.05rem;
	}

	.list-stack {
		display: flex;
		flex-direction: column;
	}

	.listen-row,
	.artist-row {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 0;
		border-bottom: 1px solid var(--border-subtle);
	}

	.listen-row:last-child,
	.artist-row:last-child {
		border-bottom: none;
	}

	.listen-art {
		width: 32px;
		height: 32px;
		border-radius: 9px;
		object-fit: cover;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		flex-shrink: 0;
	}

	.listen-art.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		font-size: 0.9rem;
	}

	.listen-meta,
	.artist-meta {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
	}

	.listen-meta p,
	.listen-meta span,
	.artist-meta p,
	.artist-meta span {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.listen-meta p,
	.artist-meta p {
		font-weight: 600;
	}

	.listen-meta span,
	.artist-meta span,
	.listen-stamp {
		color: var(--text-secondary);
		font-size: 0.78rem;
	}

	.listen-stamp,
	.artist-row strong {
		flex-shrink: 0;
	}

	.artist-row strong {
		font-size: 0.85rem;
		font-weight: 600;
	}

	.genre-row {
		display: flex;
		flex-wrap: wrap;
		gap: 10px;
	}

	.genre-pill {
		padding: 5px 12px;
		border-radius: 99px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		font-size: 0.78rem;
		color: var(--text-secondary);
	}

	.system-row {
		display: flex;
		flex-wrap: wrap;
		gap: 10px;
	}

	@media (max-width: 1180px) {
		.stat-strip,
		.activity-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}

	@media (max-width: 640px) {
		.stat-strip,
		.activity-grid {
			grid-template-columns: 1fr;
		}

		.listen-row,
		.artist-row {
			align-items: flex-start;
		}

		.listen-row {
			flex-wrap: wrap;
		}

		.listen-stamp {
			width: 100%;
			padding-left: 42px;
		}
	}
</style>
