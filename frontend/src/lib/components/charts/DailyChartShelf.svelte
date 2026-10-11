<script module lang="ts">
	// One provider-matrix refresh per app session, however often the page is
	// opened. The server only re-scrapes when asked, so without this a matrix
	// that already held data was never refreshed and "daily" charts went stale
	// for as long as the app stayed installed.
	let matrixRefreshStarted = false;
</script>

<script lang="ts">
	import {
		audienceLabel,
		isVideo,
		matrixHasData,
		matrixNeedsRefresh,
		movementLabel,
		needsMatch,
		regionHasMatrixData as regionHasMatrixDataIn,
	} from './chart_rules';
	import { onDestroy, onMount, untrack } from 'svelte';
	import {
		api,
		type ChartMatrixCell,
		type ChartMatrixResponse,
		type ChartSnapshotEntry,
		type ChartSnapshotResponse,
		type TidalPlayable,
		type TidalSearchTrack,
	} from '$lib/api/client';
	import { cachedApi, invalidateChartSnapshotCaches } from '$lib/cache/api_queries';
	import type { CachedQuery } from '$lib/cache/query';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { playTidalTrackNow, playTrackNow, reportPlayerError } from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import ChartMural, { type ChartMuralItem } from '$lib/components/charts/ChartMural.svelte';
	import { selectedChartRegion, selectedChartSource } from '$lib/stores/trending-prefs';
	import { composeTidalArtQuery, lazyTidalArt, peekTidalArt } from '$lib/actions/lazy-tidal-art';
	import { usableArtwork } from '$lib/utils/artwork';
	import {
		chartMatchKey,
		matchChartTrack,
		peekChartMatch,
		playableFromMatch,
	} from '$lib/components/charts/chart_tidal_match';

	const REGIONS = [
		{ value: 'global', label: 'Global' },
		{ value: 'US', label: 'US' },
		{ value: 'UK', label: 'UK' },
		{ value: 'AU', label: 'AU' },
		{ value: 'CA', label: 'CA' },
		{ value: 'NZ', label: 'NZ' },
	];

	const PERIOD = 'daily';
	const LIMIT = 20;
	const ROTATE_MS = 8000;

	type ChartItem = {
		artist: string;
		title: string;
		album?: string | null;
		entity_type: string;
		artwork_url: string | null;
		tidal_id: number | null;
		local_track_id?: number | null;
	};

	let matrix = $state<ChartMatrixResponse | null>(null);
	let matrixLoading = $state(true);
	let matrixError = $state<unknown>(null);
	let refreshingMatrix = $state(false);

	let data = $state<ChartSnapshotResponse | null>(null);
	// Which chart `data` belongs to; while another chart loads, the old one stays
	// on screen dimmed rather than blanking to a skeleton.
	let dataToken = $state('');
	let activeToken = $state('');
	let loading = $state(true);
	let refreshing = $state(false);
	let error = $state<unknown>(null);

	// TIDAL matches keyed by chartMatchKey(artist, title), shared across every
	// region and provider (and, via chart_tidal_match, across visits).
	let matches = $state<Record<string, TidalSearchTrack | null>>({});
	let resolving = $state<Record<string, boolean>>({});
	let lazyArt = $state<Record<string, string>>({});

	let currentEntryIndex = $state(0);
	let carouselPaused = $state(false);
	let muralAnchor = $state<HTMLElement>();

	let destroyed = false;
	let snapshotRefreshAttempted = false;
	let matrixQuery: CachedQuery<ChartMatrixResponse> | null = null;
	let snapshotQuery: CachedQuery<ChartSnapshotResponse> | null = null;

	let chartEntries = $derived(data?.entries ?? []);
	let currentEntry = $derived(chartEntries[currentEntryIndex] ?? chartEntries[0] ?? null);
	let showingOtherChart = $derived(chartEntries.length > 0 && dataToken !== activeToken);
	let providerOptions = $derived(
		(matrix?.providers ?? []).map((provider) => ({ value: provider.source_key, label: provider.label })),
	);
	let muralItems = $derived<ChartMuralItem[]>(
		chartEntries.map((entry) => ({
			id: String(entry.id),
			title: entry.title,
			subtitle: entrySubtitle(entry),
			artwork: itemArtwork(entry),
			fallbackText: fallbackText(entry.title),
			tileLabel: `Select ${entry.title}`,
			tileTitle: `${entry.rank}. ${entry.title} - ${entry.artist}`,
		})),
	);

	onMount(() => {
		matrixQuery = cachedApi.chartMatrixQuery();
		return matrixQuery.subscribe((state) => {
			if (state.data) matrix = state.data;
			matrixLoading = state.loading;
			matrixError = state.data ? null : state.error;
			if (state.error && !state.data) console.error('[daily-charts] matrix fetch failed', state.error);
			// Decide on a fresh answer only: a hydrated copy is revalidating already.
			if (state.data && !state.loading && !state.refreshing && matrixNeedsRefresh(state.data)) {
				void refreshMatrix();
			}
		});
	});

	onDestroy(() => {
		destroyed = true;
	});

	// One cached query per region + provider: revisits paint instantly and
	// revalidate in the background. The subscription is torn down with the
	// effect, so a late answer for an old chart can never land on the new one.
	$effect(() => {
		const region = $selectedChartRegion;
		const source = $selectedChartSource;
		const token = `${source}:${region}`;
		activeToken = token;
		const query = cachedApi.chartSnapshotQuery({ source, period: PERIOD, region, limit: LIMIT });
		snapshotQuery = query;
		return query.subscribe((state) => untrack(() => {
			if (state.data) {
				if (dataToken !== token) currentEntryIndex = 0;
				data = state.data;
				dataToken = token;
			}
			loading = state.loading;
			refreshing = state.refreshing;
			error = state.data ? null : state.error;
			if (state.error && !state.data) console.error('[daily-charts] snapshot fetch failed', state.error);
			const fresh = state.data && !state.loading && !state.refreshing;
			if (
				fresh &&
				!snapshotRefreshAttempted &&
				state.data!.entries.length > 0 &&
				state.data!.entries.length < Math.min(10, LIMIT)
			) {
				snapshotRefreshAttempted = true;
				void refreshMatrix();
			}
		}));
	});

	// A persisted provider that the matrix no longer offers would leave the
	// mural empty forever; fall back to the first one the server lists.
	$effect(() => {
		const providers = matrix?.providers ?? [];
		if (providers.length === 0) return;
		if (!providers.some((provider) => provider.source_key === $selectedChartSource)) {
			selectedChartSource.set(providers[0].source_key);
		}
	});

	$effect(() => {
		const entries = chartEntries;
		untrack(() => {
			for (const entry of entries) void resolveItem(entry);
		});
	});

	$effect(() => {
		if (currentEntryIndex >= chartEntries.length) currentEntryIndex = 0;
	});

	$effect(() => {
		if (chartEntries.length <= 1) return;
		const timer = setInterval(() => {
			if (!carouselPaused) jumpEntry(1);
		}, ROTATE_MS);
		return () => clearInterval(timer);
	});

	async function refreshMatrix(force = false) {
		if (matrixRefreshStarted && !force) return;
		matrixRefreshStarted = true;
		refreshingMatrix = true;
		try {
			await api.refreshChartMatrix();
			if (destroyed) return;
			invalidateChartSnapshotCaches();
			void snapshotQuery?.refresh().catch(() => undefined);
		} catch (e) {
			if (destroyed) return;
			console.error('[daily-charts] matrix refresh failed', e);
		} finally {
			if (!destroyed) refreshingMatrix = false;
		}
	}

	function retrySnapshot() {
		void snapshotQuery?.refresh().catch(() => undefined);
	}

	function retryMatrix() {
		void matrixQuery?.refresh().catch(() => undefined);
	}

	function pickRegion(region: string) {
		selectedChartRegion.set(region);
	}

	function pickProvider(source: string) {
		selectedChartSource.set(source);
	}

	function focusCell(region: string, source: string) {
		pickRegion(region);
		pickProvider(source);
		// The matrix sits below the mural; bring the chart that just changed into
		// view instead of swapping it somewhere off screen.
		const rect = muralAnchor?.getBoundingClientRect();
		if (rect && rect.top < 0) {
			const reduce = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
			muralAnchor?.scrollIntoView({ block: 'start', behavior: reduce ? 'auto' : 'smooth' });
		}
	}

	function regionHasMatrixData(region: string): boolean {
		return regionHasMatrixDataIn(matrix, region);
	}

	function regionLabel(region: string): string {
		return REGIONS.find((item) => item.value === region)?.label ?? region;
	}

	function selectedProviderLabel(): string {
		return (
			matrix?.providers.find((provider) => provider.source_key === $selectedChartSource)?.label ??
			'Spotify'
		);
	}

	function selectEntry(index: number) {
		if (chartEntries[index]) currentEntryIndex = index;
	}

	function jumpEntry(delta: number) {
		if (chartEntries.length === 0) return;
		currentEntryIndex = (currentEntryIndex + delta + chartEntries.length) % chartEntries.length;
	}

	// TIDAL matching

	async function resolveItem(item: ChartItem): Promise<TidalSearchTrack | null> {
		if (!needsMatch(item)) return null;
		const key = chartMatchKey(item.artist, item.title);
		if (key in matches) return matches[key];
		const known = peekChartMatch(key);
		if (known !== undefined) {
			matches[key] = known;
			return known;
		}
		resolving[key] = true;
		try {
			const hit = await matchChartTrack(item.artist, item.title);
			if (!destroyed) matches[key] = hit;
			return hit;
		} catch {
			// Breaker open or search failed: not a miss, so nothing is recorded
			// and the next visit (or Play) tries again.
			return null;
		} finally {
			if (!destroyed) resolving[key] = false;
		}
	}

	function directPlayable(item: ChartItem): TidalPlayable {
		return {
			tidal_id: item.tidal_id!,
			title: item.title,
			artist_name: item.artist,
			album_title: item.album ?? null,
			artwork_url: item.artwork_url,
			duration_ms: null,
		};
	}

	async function playableFor(item: ChartItem): Promise<TidalPlayable | null> {
		if (item.tidal_id) return directPlayable(item);
		const hit = await resolveItem(item);
		return hit ? playableFromMatch(hit, item.artwork_url) : null;
	}

	async function playItem(item: ChartItem) {
		if (isVideo(item)) return;
		if (item.local_track_id) {
			await playTrackNow(item.local_track_id);
			return;
		}
		const playable = await playableFor(item);
		if (!playable) {
			reportPlayerError("Couldn't find that chart entry on TIDAL.");
			return;
		}
		await playTidalTrackNow(playable);
	}

	async function openItemContext(e: MouseEvent, item: ChartItem) {
		e.preventDefault();
		e.stopPropagation();
		if (isVideo(item)) return;
		const playable = await playableFor(item);
		if (!playable) return;
		openContextMenu(e, buildTidalTrackMenu(playable), playable.title);
	}

	function matchFor(item: ChartItem): TidalSearchTrack | null | undefined {
		return matches[chartMatchKey(item.artist, item.title)];
	}

	function itemArtwork(item: ChartItem): string | null {
		const key = chartMatchKey(item.artist, item.title);
		return usableArtwork(
			item.artwork_url,
			matches[key]?.artwork_url,
			lazyArt[key],
			peekTidalArt(composeTidalArtQuery(item.artist, item.title)),
		);
	}

	function fallbackText(title: string): string {
		return (title.trim()[0] ?? 'N').toUpperCase();
	}

	function statusLabel(item: ChartItem): string {
		if (isVideo(item)) return 'Video chart';
		if (item.local_track_id) return 'In library';
		if (item.tidal_id) return 'TIDAL ready';
		const hit = matchFor(item);
		if (hit?.in_library) return 'In library';
		if (hit) return 'TIDAL ready';
		if (hit === null) return 'Not found on TIDAL';
		if (resolving[chartMatchKey(item.artist, item.title)]) return 'Finding on TIDAL';
		return '';
	}

	function entryMetric(entry: ChartSnapshotEntry): string {
		const parts = [audienceLabel(entry), movementLabel(entry.rank_delta)].filter(Boolean);
		return parts.length > 0 ? parts.join(' - ') : statusLabel(entry);
	}

	function entrySubtitle(entry: ChartSnapshotEntry): string {
		return `#${entry.rank} - ${entry.artist}`;
	}

	function actionLabel(item: ChartItem): string {
		if (isVideo(item)) return 'Video only';
		if (resolving[chartMatchKey(item.artist, item.title)]) return 'Finding...';
		return 'Play';
	}
</script>

<section class="daily-chart-shelf">
	<SectionHeader title="Market pulse" subtitle="Daily leaders across stores and streaming services" variant="charts" level={2}>
		{#snippet actions()}
			<Segmented
				options={REGIONS}
				value={$selectedChartRegion}
				label="Daily chart region"
				onchange={pickRegion}
			/>
		{/snippet}
	</SectionHeader>

	<!-- Always rendered: holds its height while the provider list loads, so the
	     mural below never jumps. -->
	<div class="source-row">
		{#if providerOptions.length > 0}
			<Segmented
				options={providerOptions}
				value={$selectedChartSource}
				label="Daily chart provider"
				onchange={pickProvider}
			/>
		{/if}
	</div>

	<div class="mural-anchor" bind:this={muralAnchor}>
		{#if error && (chartEntries.length === 0 || showingOtherChart)}
			<ErrorState title="Couldn't load this chart" {error} onretry={retrySnapshot} />
		{:else if chartEntries.length > 0 && currentEntry}
			<ChartMural
				items={muralItems}
				currentIndex={currentEntryIndex}
				ariaLabel={`${selectedProviderLabel()} ${regionLabel($selectedChartRegion)} top ${chartEntries.length}`}
				title={currentEntry.title}
				subtitle={entrySubtitle(currentEntry)}
				metric={entryMetric(currentEntry)}
				actionLabel={actionLabel(currentEntry)}
				actionDisabled={isVideo(currentEntry) || matchFor(currentEntry) === null}
				refreshing={refreshing || refreshingMatrix}
				stale={showingOtherChart && loading}
				onSelect={selectEntry}
				onJump={jumpEntry}
				onPlay={() => currentEntry && playItem(currentEntry)}
				onItemActivate={(index) => {
					const entry = chartEntries[index];
					if (entry) void playItem(entry);
				}}
				onCardContext={(event) => currentEntry && openItemContext(event, currentEntry)}
				onItemContext={(event, index) => {
					const entry = chartEntries[index];
					if (entry) void openItemContext(event, entry);
				}}
				onPauseChange={(paused) => carouselPaused = paused}
			/>
		{:else if loading || refreshingMatrix}
			<ChartMural items={[]} ariaLabel="Loading market pulse" title="" subtitle="" loading loadingLabel="Loading chart" />
		{:else}
			<EmptyState
				title={`No ${selectedProviderLabel()} top list for ${regionLabel($selectedChartRegion)}`}
				copy="Try another provider or region."
			/>
		{/if}
	</div>

	{#if matrix?.providers.length}
		<div class="matrix-shell" role="region" aria-label="Market pulse provider matrix">
			<h3 class="t-label matrix-label">All markets</h3>
			<div class="matrix-scroll">
				<div class="matrix-grid" role="table" style:--providers={matrix.providers.length}>
					<div class="matrix-row matrix-head-row" role="row">
						<span class="matrix-head" role="columnheader">Region</span>
						{#each matrix.providers as provider (provider.source_key)}
							<span class="matrix-head" role="columnheader">{provider.label}</span>
						{/each}
					</div>
					{#each matrix.rows as row (row.region)}
						<div class="matrix-row" role="row" class:selected={row.region === $selectedChartRegion}>
							<div class="matrix-region-cell" role="rowheader">
								<button type="button" class="matrix-region" onclick={() => pickRegion(row.region)}>
									{regionLabel(row.region)}
								</button>
							</div>
							{#each matrix.providers as provider (provider.source_key)}
								{@const cell = row.cells[provider.source_key]}
								{#if cell}
									{@const key = chartMatchKey(cell.artist, cell.title)}
									<!-- The menu is a mouse shortcut; keyboard users reach the
									     same song through the two buttons inside. -->
									<!-- svelte-ignore a11y_interactive_supports_focus -->
									<div
										class="matrix-cell"
										role="cell"
										class:active={row.region === $selectedChartRegion && provider.source_key === $selectedChartSource}
										oncontextmenu={(e) => void openItemContext(e, cell)}
									>
										<button
											type="button"
											class="cell-art"
											disabled={isVideo(cell)}
											onclick={() => void playItem(cell)}
											aria-label={`Play ${cell.title} by ${cell.artist}`}
											title={isVideo(cell) ? 'Video chart' : `Play ${cell.title}`}
											use:lazyTidalArt={{
												enabled: !itemArtwork(cell) && !isVideo(cell),
												query: { artist: cell.artist, title: cell.title },
												onResolve: (url) => (lazyArt[key] = url),
											}}
										>
											<ArtworkImage
												src={itemArtwork(cell)}
												size={160}
												className="matrix-cell-art"
												fallbackText={fallbackText(cell.title)}
												tint
												decorative
											/>
											{#if !isVideo(cell)}
												<svg class="cell-play" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 2.5l9.5 5.5L4 13.5V2.5z" /></svg>
											{/if}
										</button>
										<button
											type="button"
											class="cell-text"
											onclick={() => focusCell(row.region, provider.source_key)}
											aria-label={`Show the ${provider.label} ${regionLabel(row.region)} chart`}
										>
											<strong class="t-row-title">{cell.title}</strong>
											<span class="t-meta">{cell.artist}</span>
											{#if audienceLabel(cell)}
												<small>{audienceLabel(cell)}</small>
											{/if}
										</button>
									</div>
								{:else}
									<div class="matrix-cell empty" role="cell">No data</div>
								{/if}
							{/each}
						</div>
					{/each}
				</div>
			</div>
		</div>
	{:else if matrixLoading}
		<div class="matrix-skeleton" role="status" aria-label="Loading chart providers">
			{#each Array.from({ length: 4 }) as _, i (i)}
				<span style:--idx={i}></span>
			{/each}
		</div>
	{:else if matrixError}
		<ErrorState title="Couldn't load the market matrix" error={matrixError} onretry={retryMatrix} />
	{/if}

	{#if !matrixLoading && !matrixError && !regionHasMatrixData($selectedChartRegion)}
		<EmptyState
			title={matrixHasData(matrix)
				? `No provider leaders for ${regionLabel($selectedChartRegion)}`
				: 'No market snapshot yet'}
			copy={matrixHasData(matrix)
				? 'This region has no provider leaders yet. Global data is available above.'
				: 'NOOR tried to refresh the provider matrix, but there is no stored chart data yet.'}
		>
			{#snippet actions()}
				{#if !matrixHasData(matrix)}
					<button type="button" class="btn btn-secondary" disabled={refreshingMatrix} onclick={() => refreshMatrix(true)}>
						{refreshingMatrix ? 'Refreshing...' : 'Refresh charts'}
					</button>
				{/if}
			{/snippet}
		</EmptyState>
	{/if}
</section>

<style>
	.daily-chart-shelf {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.source-row {
		display: flex;
		min-height: calc(var(--control-h) + 6px);
		overflow-x: auto;
		scrollbar-width: none;
	}

	.source-row::-webkit-scrollbar {
		display: none;
	}

	.mural-anchor {
		scroll-margin-top: var(--space-5);
	}

	.matrix-shell {
		display: grid;
		gap: var(--group-gap);
		margin-top: var(--space-5);
	}

	.matrix-label {
		margin: 0;
	}

	.matrix-scroll {
		overflow-x: auto;
		padding-bottom: var(--space-1);
	}

	.matrix-grid {
		display: grid;
		grid-template-columns: minmax(64px, 88px) repeat(var(--providers, 6), minmax(156px, 1fr));
		row-gap: var(--row-gap);
		min-width: calc(88px + var(--providers, 6) * 160px);
	}

	/* Rows are subgrids so every column lines up with its header. */
	.matrix-row {
		display: grid;
		grid-column: 1 / -1;
		grid-template-columns: subgrid;
		column-gap: var(--space-1);
		border-radius: var(--radius-sm);
	}

	.matrix-head-row {
		border-bottom: 1px solid var(--border-subtle);
		border-radius: 0;
		padding-bottom: var(--space-2);
		margin-bottom: var(--space-1);
	}

	.matrix-head {
		padding: 0 var(--space-2);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--text-tertiary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.matrix-row.selected {
		background: var(--bg-surface);
	}

	.matrix-region-cell {
		display: flex;
		min-width: 0;
	}

	.matrix-region {
		display: flex;
		flex: 1;
		align-items: center;
		padding: 0 var(--space-2);
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		text-align: left;
		cursor: pointer;
		transition: color var(--motion-fast);
	}

	.matrix-region:hover,
	.matrix-row.selected .matrix-region {
		color: var(--text-primary);
	}

	.matrix-cell {
		display: grid;
		grid-template-columns: 40px minmax(0, 1fr);
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
		min-height: 56px;
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-sm);
		transition: background var(--motion-fast);
	}

	.matrix-cell:hover {
		background: var(--bg-hover);
	}

	.matrix-cell.active {
		background: var(--accent-soft);
	}

	.matrix-cell.empty {
		display: flex;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.cell-art {
		position: relative;
		width: 40px;
		height: 40px;
		padding: 0;
		border: 0;
		border-radius: var(--radius-xs);
		overflow: hidden;
		background: var(--bg-raised);
		cursor: pointer;
	}

	.cell-art:disabled {
		cursor: default;
	}

	:global(.matrix-cell-art),
	:global(.matrix-cell-art.fallback) {
		display: grid;
		place-items: center;
		width: 100%;
		height: 100%;
		object-fit: cover;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-bold);
	}

	.cell-play {
		position: absolute;
		inset: 0;
		margin: auto;
		width: 16px;
		height: 16px;
		fill: #fff;
		opacity: 0;
		filter: drop-shadow(0 1px 3px rgba(0, 0, 0, 0.6));
		transition: opacity var(--motion-fast);
		pointer-events: none;
	}

	.cell-art::after {
		content: '';
		position: absolute;
		inset: 0;
		background: rgba(8, 8, 12, 0.45);
		opacity: 0;
		transition: opacity var(--motion-fast);
		pointer-events: none;
	}

	.cell-art:not(:disabled):hover::after,
	.cell-art:not(:disabled):focus-visible::after,
	.cell-art:not(:disabled):hover .cell-play,
	.cell-art:not(:disabled):focus-visible .cell-play {
		opacity: 1;
	}

	.cell-text {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 1px;
		min-width: 0;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		text-align: left;
		cursor: pointer;
	}

	.cell-text strong,
	.cell-text span {
		max-width: 100%;
	}

	.cell-text small {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-variant-numeric: tabular-nums;
	}

	.cell-art:focus-visible,
	.cell-text:focus-visible,
	.matrix-region:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.matrix-skeleton {
		display: grid;
		gap: var(--row-gap);
		margin-top: var(--space-5);
		animation: matrix-skeleton-in var(--motion-fast) 150ms both;
	}

	.matrix-skeleton span {
		height: 56px;
		border-radius: var(--radius-sm);
		background: linear-gradient(90deg, var(--bg-surface) 0%, var(--bg-hover) 50%, var(--bg-surface) 100%);
		background-size: 200% 100%;
		animation: matrix-shimmer 1.4s ease-in-out infinite;
		animation-delay: calc(var(--idx) * 80ms);
		opacity: 0.7;
	}

	@keyframes matrix-skeleton-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	@keyframes matrix-shimmer {
		0% { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}

	@media (prefers-reduced-motion: reduce) {
		.matrix-skeleton span {
			animation: none;
		}
	}
</style>
