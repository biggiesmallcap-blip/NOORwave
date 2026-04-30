<script lang="ts">
	import { onMount } from 'svelte';
	import type { Unsubscriber } from 'svelte/store';
	import type { Snapshot } from './$types';
	import {
		api,
		type AnalyticsDashboard,
		type AudioFeaturesStats,
		type GenreAudioMetrics,
		type GenreCohort,
		type GenreEvolutionPoint,
		type GenreHeat
	} from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { playTrackNow } from '$lib/stores/player';

	let dashboard = $state<AnalyticsDashboard | null>(null);
	let audioStats = $state<AudioFeaturesStats | null>(null);
	let genreHeat = $state<GenreHeat[]>([]);
	let genreCohorts = $state<GenreCohort[]>([]);
	let genreEvolution = $state<GenreEvolutionPoint[]>([]);
	let genreMetrics = $state<GenreAudioMetrics[]>([]);
	let loading = $state(true);
	let refreshing = $state(false);
	let error = $state<string | null>(null);
	let refreshedAt = $state<string | null>(null);
	let wsUnsubscribe: Unsubscriber | null = null;

	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: typeof window !== 'undefined' ? window.scrollY : 0 }),
		restore: (saved) => {
			requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }));
		}
	};

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

	function formatDay(value: string): string {
		const date = new Date(value);
		if (Number.isNaN(date.getTime())) return value;
		return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
	}

	function clampPercent(value: number): number {
		if (!Number.isFinite(value)) return 0;
		return Math.max(0, Math.min(100, value));
	}

	async function refreshAnalytics() {
		if (!loading) refreshing = true;
		error = null;
		try {
			const [
				dashboardResponse,
				statsResponse,
				heatResponse,
				cohortResponse,
				evolutionResponse,
				metricsResponse
			] = await Promise.all([
				api.getAnalyticsDashboard(18, 10, 30),
				api.getAudioFeaturesStats().catch(() => null),
				api.getGenreHeat(90).catch(() => null),
				api.getGenreCohorts(90).catch(() => null),
				api.getGenreEvolution(90).catch(() => null),
				api.getGenreAudioMetrics().catch(() => null)
			]);
			dashboard = dashboardResponse.dashboard;
			audioStats = statsResponse?.stats ?? null;
			genreHeat = heatResponse?.heat ?? [];
			genreCohorts = cohortResponse?.cohorts ?? [];
			genreEvolution = evolutionResponse?.evolution ?? [];
			genreMetrics = metricsResponse?.metrics ?? [];
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
	let maxActivity = $derived(Math.max(1, ...(dashboard?.activity ?? []).map((point) => point.listens)));
	let totalLibraryMinutes = $derived(
		dashboard?.overview && behavior
			? Math.round((behavior.total_listened_ms || 0) / 60000)
			: 0
	);
	let favoriteRatio = $derived(
		overview && overview.tracks > 0 ? overview.favorite_tracks / overview.tracks : 0
	);
	let taggedRatio = $derived(
		overview && overview.tracks > 0 ? overview.tagged_tracks / overview.tracks : 0
	);
	let hottestGenres = $derived(
		genreHeat
			.slice()
			.sort((a, b) => b.listen_count - a.listen_count)
			.slice(0, 8)
	);
	let maxGenreHeat = $derived(Math.max(1, ...hottestGenres.map((genre) => genre.listen_count)));
	let topAudioGenres = $derived(
		genreMetrics
			.filter((genre) => genre.analyzed_count > 0)
			.sort((a, b) => (b.avg_energy ?? 0) - (a.avg_energy ?? 0))
			.slice(0, 6)
	);
	let latestEvolution = $derived(
		genreEvolution
			.slice()
			.sort((a, b) => b.period_start.localeCompare(a.period_start))
			.slice(0, 10)
	);
	let cohortTotal = $derived(
		genreCohorts.reduce((sum, cohort) => sum + cohort.listen_count, 0)
	);
	let insightCards = $derived.by(() => {
		if (!dashboard || !behavior || !overview) return [];
		const topGenre = dashboard.top_genres[0];
		const topArtist = dashboard.top_artists[0];
		return [
			{
				label: 'Listening posture',
				value: behavior.completion_rate >= 0.72 ? 'Deep sessions' : 'Browsing mode',
				copy: `${formatPercent(behavior.completion_rate)} completion across ${formatCount(behavior.total_listens)} listens.`
			},
			{
				label: 'Taste gravity',
				value: topGenre?.genre_name ?? 'No genre yet',
				copy: topGenre ? `${formatCount(topGenre.listens)} listens in the leading genre.` : 'Genre tags will sharpen this once listening history grows.'
			},
			{
				label: 'Collection signal',
				value: topArtist?.artist_name ?? 'No artist yet',
				copy: topArtist ? `${formatCount(topArtist.unique_tracks)} unique tracks reached.` : `${formatPercent(taggedRatio)} of the library is tagged.`
			}
		];
	});
</script>

<svelte:head>
	<title>Analytics | NOOR</title>
</svelte:head>

<div class="page-shell analytics-page animate-in">
	<PageHeader
		eyebrow="Analytics"
		title="A living map of the library."
		subtitle="Recent listening, genre gravity, DSP coverage, and the signals discovery is already learning from."
	>
		{#snippet actions()}
			<button class="btn btn-glass" onclick={refreshAnalytics} disabled={loading || refreshing}>
				{loading || refreshing ? 'Refreshing...' : 'Refresh'}
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
		<EmptyState title="Loading analytics" copy="Pulling listening history, audio features, and genre heat." />
	{:else if dashboard && overview && behavior}
		<section class="analytics-hero glass-panel">
			<div class="hero-copy">
				<p class="eyebrow">30 day listening pulse</p>
				<h2>{formatDuration(behavior.total_listened_ms)} listened</h2>
				<p>
					{formatCount(behavior.unique_tracks)} unique tracks reached over {formatCount(behavior.active_days)}
					active days, with {formatPercent(behavior.completion_rate)} completion.
				</p>
				<div class="hero-badges">
					<StateBadge label={`${formatPercent(favoriteRatio)} favorites`} tone="active" compact={true} />
					<StateBadge label={`${formatPercent(taggedRatio)} tagged`} tone="success" compact={true} />
					<StateBadge label={`${audioStats?.total_analyzed?.toLocaleString() ?? 0} DSP analyzed`} tone="default" compact={true} />
				</div>
			</div>

			<div class="activity-panel">
				<div class="activity-header">
					<span>Daily listens</span>
					<strong>{formatCount(dashboard.activity.reduce((sum, point) => sum + point.listens, 0))}</strong>
				</div>
				<div class="activity-bars" aria-label="Daily listening activity">
					{#each dashboard.activity as point}
						<div class="activity-bar" title={`${formatDay(point.day)}: ${point.listens} listens`}>
							<i style={`height:${clampPercent((point.listens / maxActivity) * 100)}%`}></i>
						</div>
					{/each}
				</div>
			</div>
		</section>

		<section class="stat-grid">
			<MetricPair label="Tracks" value={formatCount(overview.tracks)} copy={`${formatCount(overview.albums)} albums, ${formatCount(overview.artists)} artists.`} />
			<MetricPair label="Listen rows" value={formatCount(behavior.total_listens)} copy="Stored playback sessions." />
			<MetricPair label="Completion" value={formatPercent(behavior.completion_rate)} copy={`${formatCount(behavior.skipped_listens)} sessions ended early.`} />
			<MetricPair label="Avg listen" value={formatDuration(behavior.average_listen_ms)} copy={`${formatCount(behavior.repeat_track_count)} repeat-heavy tracks.`} />
		</section>

		<section class="insight-grid">
			{#each insightCards as insight}
				<div class="glass-panel insight-card">
					<span>{insight.label}</span>
					<strong>{insight.value}</strong>
					<p>{insight.copy}</p>
				</div>
			{/each}
		</section>

		<section class="dashboard-grid">
			<section class="glass-panel panel recent-panel">
				<SectionHeader eyebrow="History" title="Recent listens" subtitle="The latest sessions recorded by playback." />
				{#if dashboard.recent_listens.length === 0}
					<EmptyState title="No listens yet" copy="Start playback and this history will begin to fill in." />
				{:else}
					<div class="stack scroll-list">
						{#each dashboard.recent_listens as listen}
							<div
								class="track-card interactive"
								role="button"
								tabindex="0"
								onclick={() => void playTrackNow(listen.track_id)}
								onkeydown={(e) => e.key === 'Enter' && void playTrackNow(listen.track_id)}
								oncontextmenu={(e) => openContextMenu(e, buildTrackMenu({ id: listen.track_id, title: listen.track_title, artist_name: listen.artist_name, album_title: listen.album_title }), listen.track_title)}
							>
								{#if listen.artwork_url}
									<img src={listen.artwork_url} alt="" />
								{:else}
									<div class="art-placeholder">♪</div>
								{/if}
								<div class="track-info">
									<h4>{listen.track_title}</h4>
									<p>{listen.artist_name ?? 'Unknown artist'}{listen.album_title ? ` / ${listen.album_title}` : ''}</p>
								</div>
								<div class="track-side">
									<strong>{formatDuration(listen.duration_listened_ms)}</strong>
									<span>{formatListenStamp(listen.started_at)}</span>
								</div>
							</div>
						{/each}
					</div>
				{/if}
			</section>

			<section class="glass-panel panel">
				<SectionHeader eyebrow="Heat" title="Genre pressure" subtitle="Recent listening weight across mapped genres." />
				<div class="heat-list">
					{#each hottestGenres as genre}
						<div class="heat-row">
							<div>
								<strong>{genre.genre_name}</strong>
								<span>{formatDuration(genre.total_listened_ms)}</span>
							</div>
							<div class="heat-meter">
								<i style={`width:${clampPercent((genre.listen_count / maxGenreHeat) * 100)}%`}></i>
							</div>
							<b>{formatCount(genre.listen_count)}</b>
						</div>
					{/each}
					{#if hottestGenres.length === 0}
						<EmptyState title="No genre heat yet" copy="Genre listening heat appears once tagged tracks are played." />
					{/if}
				</div>
			</section>
		</section>

		<section class="dashboard-grid">
			<section class="glass-panel panel">
				<SectionHeader eyebrow="Artists" title="Top artists" subtitle="Who has dominated the room." />
				<div class="rank-list">
					{#each dashboard.top_artists as artist, i}
						<div class="rank-row">
							<span>{i + 1}</span>
							<div>
								<h4>{artist.artist_name}</h4>
								<p>{formatCount(artist.unique_tracks)} unique tracks / {formatDuration(artist.total_listened_ms)}</p>
							</div>
							<strong>{formatCount(artist.listens)}</strong>
						</div>
					{/each}
				</div>
			</section>

			<section class="glass-panel panel">
				<SectionHeader eyebrow="Tracks" title="Top tracks" subtitle="The tracks with the strongest return signal." />
				<div class="rank-list">
					{#each dashboard.top_tracks as track, i}
						<div
							class="rank-row interactive"
							role="button"
							tabindex="0"
							onclick={() => void playTrackNow(track.track_id)}
							onkeydown={(e) => e.key === 'Enter' && void playTrackNow(track.track_id)}
							oncontextmenu={(e) => {
								e.preventDefault();
								openContextMenu(e, buildTrackMenu({ id: track.track_id, title: track.title, artist_name: track.artist_name, album_title: track.album_title }), track.title);
							}}
						>
							<span>{i + 1}</span>
							<div>
								<h4>{track.title}</h4>
								<p>{track.artist_name ?? 'Unknown artist'} / {formatDuration(track.total_listened_ms)}</p>
							</div>
							<div class="rank-actions">
								<strong>{formatCount(track.listens)}</strong>
								<button
									type="button"
									class="list-card-menu"
									title="More options"
									aria-label="More options"
									onclick={(e) => {
										e.stopPropagation();
										openMenuAtElement(e.currentTarget as HTMLElement, buildTrackMenu({ id: track.track_id, title: track.title, artist_name: track.artist_name, album_title: track.album_title }), track.title);
									}}
								>...</button>
							</div>
						</div>
					{/each}
				</div>
			</section>
		</section>

		<section class="signal-grid">
			<section class="glass-panel panel">
				<SectionHeader eyebrow="Audio intelligence" title="DSP profile" subtitle="The analyzed tempo, key, and energy layer." />
				<div class="dsp-grid">
					<div>
						<span>Analyzed tracks</span>
						<strong>{audioStats?.total_analyzed?.toLocaleString() ?? '0'}</strong>
					</div>
					<div>
						<span>Average BPM</span>
						<strong>{audioStats?.avg_bpm?.toFixed(1) ?? '--'}</strong>
					</div>
					<div>
						<span>Top key</span>
						<strong>{audioStats?.top_key ?? '--'}</strong>
					</div>
					<div>
						<span>Energy</span>
						<strong>{audioStats?.avg_energy != null ? formatPercent(audioStats.avg_energy) : '--'}</strong>
					</div>
				</div>
				<div class="key-row">
					{#each Object.entries(audioStats?.key_distribution ?? {}).slice(0, 10) as [key, count]}
						<span>{key} / {formatCount(count)}</span>
					{/each}
				</div>
			</section>

			<section class="glass-panel panel">
				<SectionHeader eyebrow="Cohorts" title="Taste families" subtitle="Genre cohorts over the last 90 days." />
				<div class="cohort-list">
					{#each genreCohorts.slice(0, 7) as cohort}
						<div class="cohort-row">
							<span>{cohort.icon}</span>
							<div>
								<strong>{cohort.label}</strong>
								<div class="heat-meter">
									<i style={`width:${clampPercent((cohort.listen_count / Math.max(1, cohortTotal)) * 100)}%`}></i>
								</div>
							</div>
							<b>{formatCount(cohort.listen_count)}</b>
						</div>
					{/each}
					{#if genreCohorts.length === 0}
						<EmptyState title="No cohorts yet" copy="Cohorts appear after mapped genre listens accumulate." />
					{/if}
				</div>
			</section>
		</section>

		<section class="glass-panel panel">
			<SectionHeader eyebrow="Taste" title="Genre and audio signals" subtitle="The most useful raw signals for discovery and automix." />
			<div class="genre-signal-layout">
				<div class="genre-row">
					{#each dashboard.top_genres as genre}
						<span class="genre-pill">{genre.genre_name} / {formatCount(genre.listens)}</span>
					{/each}
				</div>
				<div class="audio-genre-grid">
					{#each topAudioGenres as genre}
						<div>
							<strong>{genre.genre_name}</strong>
							<span>{genre.avg_bpm?.toFixed(0) ?? '--'} BPM / {genre.avg_energy != null ? formatPercent(genre.avg_energy) : '--'} energy / {formatCount(genre.analyzed_count)} analyzed</span>
						</div>
					{/each}
				</div>
			</div>
		</section>

		{#if latestEvolution.length > 0}
			<section class="glass-panel panel">
				<SectionHeader eyebrow="Trajectory" title="Latest genre movement" subtitle="Recent period points from the genre evolution endpoint." />
				<div class="evolution-strip">
					{#each latestEvolution as point}
						<div>
							<strong>{point.genre_name}</strong>
							<span>{formatDay(point.period_start)} / {formatCount(point.listen_count)} listens</span>
						</div>
					{/each}
				</div>
			</section>
		{/if}
	{/if}
</div>

<style>
	.analytics-page {
		gap: var(--space-5);
	}

	.meta-time {
		text-transform: none;
		letter-spacing: 0.04em;
	}

	.analytics-hero {
		display: grid;
		grid-template-columns: minmax(0, 0.95fr) minmax(340px, 1.05fr);
		gap: 24px;
		padding: 24px;
		align-items: stretch;
	}

	.hero-copy {
		display: grid;
		align-content: center;
		gap: 12px;
	}

	.hero-copy h2 {
		font-size: clamp(2rem, 4vw, 4rem);
	}

	.hero-copy p:not(.eyebrow) {
		color: var(--text-secondary);
		max-width: 58ch;
	}

	.hero-badges {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.activity-panel {
		display: grid;
		grid-template-rows: auto 1fr;
		gap: 14px;
		min-height: 230px;
		padding: 16px;
		border-radius: 14px;
		background: rgba(255, 255, 255, 0.026);
		border: 1px solid var(--border-subtle);
	}

	.activity-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		color: var(--text-secondary);
	}

	.activity-header strong {
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: 1.5rem;
	}

	.activity-bars {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(8px, 1fr));
		align-items: end;
		gap: 5px;
		min-height: 150px;
	}

	.activity-bar {
		height: 100%;
		display: flex;
		align-items: end;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.05);
		overflow: hidden;
	}

	.activity-bar i,
	.heat-meter i {
		display: block;
		width: 100%;
		min-height: 3px;
		border-radius: inherit;
		background: linear-gradient(180deg, var(--accent-strong), var(--state-success));
	}

	.insight-grid,
	.dashboard-grid,
	.signal-grid {
		display: grid;
		gap: var(--space-4);
	}

	.insight-grid {
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}

	.dashboard-grid {
		grid-template-columns: minmax(0, 1.12fr) minmax(320px, 0.88fr);
	}

	.signal-grid {
		grid-template-columns: repeat(2, minmax(0, 1fr));
	}

	.insight-card,
	.panel {
		padding: 20px;
	}

	.insight-card {
		display: grid;
		gap: 8px;
	}

	.insight-card span,
	.dsp-grid span {
		color: var(--text-tertiary);
		font-size: 0.72rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.insight-card strong {
		font-family: var(--font-display);
		font-size: 1.7rem;
		line-height: 1.05;
	}

	.insight-card p {
		color: var(--text-secondary);
	}

	.panel {
		display: flex;
		flex-direction: column;
		gap: 18px;
		min-width: 0;
	}

	.stack,
	.rank-list,
	.heat-list,
	.cohort-list {
		display: grid;
		gap: 6px;
	}

	.scroll-list {
		max-height: 520px;
		overflow-y: auto;
		overflow-x: hidden;
		scrollbar-width: thin;
		scrollbar-color: rgba(255, 255, 255, 0.12) transparent;
	}

	.track-card {
		display: grid;
		grid-template-columns: 48px minmax(0, 1fr) auto;
		align-items: center;
		gap: 12px;
		padding: 9px;
		border-radius: 10px;
		border: 1px solid transparent;
	}

	.track-card img,
	.art-placeholder {
		width: 48px;
		height: 48px;
		border-radius: 8px;
		object-fit: cover;
		background: rgba(255, 255, 255, 0.05);
	}

	.art-placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		border: 1px solid var(--border-subtle);
	}

	.track-info,
	.rank-row > div,
	.audio-genre-grid div,
	.evolution-strip div {
		min-width: 0;
	}

	.track-info h4,
	.track-info p,
	.rank-row h4,
	.rank-row p {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.track-info p,
	.track-side span,
	.rank-row p,
	.heat-row span,
	.audio-genre-grid span,
	.evolution-strip span {
		color: var(--text-secondary);
	}

	.track-side {
		display: grid;
		justify-items: end;
		gap: 3px;
	}

	.interactive {
		cursor: pointer;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}

	.interactive:hover {
		background: rgba(255, 255, 255, 0.045);
		border-color: var(--border-subtle);
	}

	.heat-row,
	.cohort-row,
	.rank-row {
		display: grid;
		align-items: center;
		gap: 12px;
		padding: 10px 0;
		border-bottom: 1px solid var(--border-subtle);
	}

	.heat-row {
		grid-template-columns: minmax(0, 1fr) minmax(120px, 0.8fr) 44px;
	}

	.cohort-row {
		grid-template-columns: 28px minmax(0, 1fr) 54px;
	}

	.rank-row {
		grid-template-columns: 28px minmax(0, 1fr) auto;
	}

	.heat-row:last-child,
	.cohort-row:last-child,
	.rank-row:last-child {
		border-bottom: none;
	}

	.rank-row > span {
		font-family: var(--font-mono);
		color: var(--text-tertiary);
		font-size: 0.78rem;
	}

	.rank-actions {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.list-card-menu {
		width: 28px;
		height: 28px;
		border-radius: 999px;
		color: var(--text-tertiary);
		opacity: 0;
	}

	.interactive:hover .list-card-menu,
	.interactive:focus-within .list-card-menu {
		opacity: 1;
	}

	.list-card-menu:hover {
		background: rgba(255, 255, 255, 0.06);
		color: var(--text-primary);
	}

	.heat-meter {
		height: 7px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.07);
		overflow: hidden;
	}

	.heat-meter i {
		height: 100%;
		background: linear-gradient(90deg, var(--accent), var(--state-success));
	}

	.dsp-grid {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 10px;
	}

	.dsp-grid div,
	.audio-genre-grid div,
	.evolution-strip div {
		display: grid;
		gap: 6px;
		padding: 12px;
		border-radius: 10px;
		background: rgba(255, 255, 255, 0.026);
		border: 1px solid var(--border-subtle);
	}

	.dsp-grid strong {
		font-family: var(--font-display);
		font-size: 1.6rem;
	}

	.key-row,
	.genre-row,
	.evolution-strip {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.key-row span,
	.genre-pill {
		padding: 7px 10px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: 0.78rem;
	}

	.genre-signal-layout {
		display: grid;
		gap: 16px;
	}

	.audio-genre-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 10px;
	}

	.evolution-strip {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
	}

	@media (max-width: 1100px) {
		.analytics-hero,
		.dashboard-grid,
		.signal-grid,
		.insight-grid {
			grid-template-columns: 1fr;
		}

		.audio-genre-grid,
		.dsp-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}

	@media (max-width: 640px) {
		.track-card,
		.heat-row,
		.cohort-row,
		.rank-row {
			grid-template-columns: 1fr;
		}

		.track-side {
			justify-items: start;
		}

		.audio-genre-grid,
		.dsp-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
