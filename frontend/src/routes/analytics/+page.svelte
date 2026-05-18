<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type AnalyticsSignals } from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import { debounce } from '$lib/utils/debounce';

	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';

	import TimeRangePills, { type TimeRange } from '$lib/components/analytics/TimeRangePills.svelte';
	import KpiStrip from '$lib/components/analytics/KpiStrip.svelte';
	import RankList from '$lib/components/analytics/RankList.svelte';
	import CohortTable from '$lib/components/analytics/CohortTable.svelte';
	import AudioProfileStrip from '$lib/components/analytics/AudioProfileStrip.svelte';
	import ListenRidgeline from '$lib/components/charts/ListenRidgeline.svelte';
	import TempoRidges from '$lib/components/charts/TempoRidges.svelte';
	import SonicField from '$lib/components/charts/SonicField.svelte';

	const RANGE_TO_DAYS: Record<TimeRange, number> = {
		'24h': 1,
		'7d': 7,
		'14d': 14,
		'30d': 30,
		all: 36500,
	};

	let range = $state<TimeRange>('30d');
	let signals = $state<AnalyticsSignals | null>(null);
	let initialLoading = $state(true);
	let windowChanging = $state(false);
	let refreshing = $state(false);
	let error = $state<string | null>(null);

	const days = $derived(RANGE_TO_DAYS[range]);

	// Non-reactive guard: prevents $effect from double-fetching on first render.
	let initialized = false;

	$effect(() => {
		const d = days; // reactive subscription — re-runs when range changes
		if (!initialized) return;
		windowChanging = true;
		error = null;
		api
			.getAnalyticsSignals(d)
			.then((s) => {
				signals = s;
			})
			.catch((e: unknown) => {
				error = e instanceof Error ? e.message : 'Failed to load analytics.';
			})
			.finally(() => {
				windowChanging = false;
			});
	});

	async function fetchSignals() {
		error = null;
		try {
			signals = await api.getAnalyticsSignals(days);
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load analytics.';
		} finally {
			initialLoading = false;
			refreshing = false;
		}
	}

	function refresh() {
		refreshing = true;
		debouncedWsRefresh.cancel();
		void fetchSignals();
	}

	const debouncedWsRefresh = debounce(() => void fetchSignals(), 1500);

	onMount(() => {
		initialized = true;
		void fetchSignals();

		const unsub = wsMessages.subscribe((msgs) => {
			const latest = msgs.at(-1);
			if (!latest) return;
			if (latest.type === 'listen_history_updated' || latest.type === 'library_synced') {
				debouncedWsRefresh();
			}
		});

		return () => {
			unsub();
			debouncedWsRefresh.cancel();
		};
	});

	// ── Visibility helpers (per plan empty-state table) ──────────────────────

	const showTempo = $derived(
		signals !== null && signals.tempo.coverage.analyzed > 0,
	);
	const showSonic = $derived(signals !== null && signals.sonic_field.total > 0);
	const showArtists = $derived(signals !== null && signals.top_artists.length > 0);
	const showTracks = $derived(signals !== null && signals.top_tracks.length > 0);
	const showRanks = $derived(showArtists || showTracks);
	const showGenres = $derived(signals !== null && signals.top_genres.length > 0);
	const showCohorts = $derived(signals !== null && signals.cohorts.some((c) => c.tracks > 0));
	const showAudioProfile = $derived(
		signals !== null && signals.audio_profile.coverage.analyzed > 0,
	);
	const ridgelineWindowLabel = $derived.by(() => {
		if (!signals) return null;
		const cap = signals.window.display_caps.ridgeline_days;
		if (!cap || cap >= signals.window.days) return null;
		return cap === 365 ? 'recent year' : `recent ${cap} days`;
	});
</script>

<div class="analytics-tree" class:dim={windowChanging}>
	<PageHeader title="Library analytics" eyebrow="Analytics">
		{#snippet actions()}
			<TimeRangePills bind:value={range} />
			<button
				type="button"
				class="refresh"
				class:spinning={refreshing}
				onclick={refresh}
				disabled={refreshing}
				aria-label="Refresh analytics"
			>↻</button>
		{/snippet}
	</PageHeader>

	{#if initialLoading}
		<!-- ── Initial load skeletons ─────────────────────────────────── -->
		<div class="skeleton-hero glass"></div>
		<div class="skeleton-strip glass"></div>
		<div class="skeleton-chart glass"></div>
		<div class="skeleton-chart glass"></div>
		<div class="skeleton-duo">
			<div class="skeleton-list glass"></div>
			<div class="skeleton-list glass"></div>
		</div>
	{:else if error}
		<EmptyState title="Couldn't load analytics" copy={error}>
			{#snippet actions()}
				<button type="button" class="refresh" onclick={refresh}>Retry</button>
			{/snippet}
		</EmptyState>
	{:else if signals}
		<!-- ── Hero — Listening Pulse ───────────────────────────────────────── -->
		<div class="section glass hero-card">
			<ListenRidgeline
				rows={signals.ridgeline}
				heroStats={signals.kpis.hero_stats}
				mode="hero"
				windowLabel={ridgelineWindowLabel}
			/>
		</div>

		<!-- ── KPI strip ────────────────────────────────────────────────────── -->
		<KpiStrip kpis={signals.kpis} />

		<!-- ── Tempo ridges ─────────────────────────────────────────────────── -->
		{#if showTempo}
			<div class="section glass">
				<TempoRidges tempo={signals.tempo} />
			</div>
		{/if}

		<!-- ── Sonic field ──────────────────────────────────────────────────── -->
		{#if showSonic}
			<div class="section glass">
				<SonicField field={signals.sonic_field} />
			</div>
		{/if}

		<!-- ── Ranks: top artists + top tracks ─────────────────────────────── -->
		{#if showRanks}
			<div class="duo">
				{#if showTracks}
					<RankList kind="track" items={signals.top_tracks} title="Top tracks" />
				{/if}
				{#if showArtists}
					<RankList kind="artist" items={signals.top_artists} title="Top artists" />
				{/if}
			</div>
		{/if}

		<!-- ── Genres ───────────────────────────────────────────────────────── -->
		{#if showGenres}
			<RankList kind="genre" items={signals.top_genres} title="Genres" limit={6} />
		{/if}

		<!-- ── Cohorts ──────────────────────────────────────────────────────── -->
		{#if showCohorts}
			<CohortTable cohorts={signals.cohorts} />
		{/if}

		<!-- ── Audio profile ────────────────────────────────────────────────── -->
		{#if showAudioProfile}
			<AudioProfileStrip profile={signals.audio_profile} />
		{/if}
	{/if}
</div>

<style>
	.analytics-tree {
		max-width: var(--content-width);
		margin: 0 auto;
		padding: var(--space-5) var(--space-5) var(--space-8);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

	/* Dim existing content on window change (don't flash skeleton). */
	.analytics-tree.dim {
		opacity: 0.55;
		pointer-events: none;
		transition: opacity 150ms ease;
	}

	.section {
		padding: var(--space-4);
	}

	.hero-card {
		padding: var(--space-5);
	}

	/* ── Initial load skeletons ─────────────────────── */

	.skeleton-hero {
		height: 460px;
		border-radius: var(--radius);
	}

	.skeleton-strip {
		height: 110px;
		border-radius: var(--radius);
	}

	.skeleton-chart {
		height: 380px;
		border-radius: var(--radius);
	}

	.skeleton-duo {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-4);
	}

	.skeleton-list {
		height: 280px;
		border-radius: var(--radius);
	}

	/* ── Ranks two-column layout ────────────────────── */

	.duo {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-4);
	}

	@media (max-width: 900px) {
		.duo {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	/* ── Refresh button ─────────────────────────────── */

	.refresh {
		font-family: var(--font-mono);
		font-size: var(--font-size-sm);
		background: transparent;
		border: 1px solid var(--input-border);
		color: var(--text-secondary);
		width: 32px;
		height: 32px;
		border-radius: var(--radius-xs);
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}

	.refresh:hover:not(:disabled) {
		color: var(--text-primary);
		border-color: var(--border-strong);
	}

	.refresh:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.refresh.spinning {
		animation: spin 0.7s linear infinite;
	}

	@keyframes spin {
		to { transform: rotate(360deg); }
	}

	/* ── Reduced motion ─────────────────────────────── */

	@media (prefers-reduced-motion: reduce) {
		.analytics-tree,
		.analytics-tree.dim {
			transition: none;
		}

		.analytics-tree :global(*),
		.analytics-tree :global(::before),
		.analytics-tree :global(::after) {
			transition: none !important;
			animation: none !important;
		}
	}
</style>
