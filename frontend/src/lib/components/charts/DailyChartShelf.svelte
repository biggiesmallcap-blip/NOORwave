<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import {
		api,
		type ChartMatrixCell,
		type ChartMatrixResponse,
		type ChartSnapshotEntry,
		type ChartSnapshotResponse,
		type TidalPlayable,
		type TidalSearchTrack,
	} from '$lib/api/client';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import { playTidalTrackNow, playerError } from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import ChartMural, { type ChartMuralItem } from '$lib/components/charts/ChartMural.svelte';
	import { tidalSearchTrackToPlayable } from '$lib/utils/track';
	import { gatedTidalSearch } from '$lib/actions/lazy-tidal-art';

	const REGIONS = [
		{ code: 'global', label: 'Global' },
		{ code: 'US', label: 'US' },
		{ code: 'UK', label: 'UK' },
		{ code: 'AU', label: 'AU' },
		{ code: 'CA', label: 'CA' },
		{ code: 'NZ', label: 'NZ' },
	];

	const PERIOD = 'daily';
	const LIMIT = 20;
	const ROTATE_MS = 8000;

	let selectedRegion = $state('global');
	let selectedSource = $state('spotify_daily');
	let matrix = $state<ChartMatrixResponse | null>(null);
	let data = $state<ChartSnapshotResponse | null>(null);
	let resolvedTracks = $state<Record<number, TidalSearchTrack | null>>({});
	let resolvingEntries = $state<Record<number, boolean>>({});
	let matrixLoading = $state(true);
	let loading = $state(true);
	let refreshingMatrix = $state(false);
	let matrixError = $state(false);
	let error = $state(false);
	let requestToken = 0;
	let destroyed = false;
	let refreshAttempted = false;
	let snapshotRefreshAttempted = false;
	let currentEntryIndex = $state(0);
	let carouselPaused = $state(false);
	let carouselTimer: ReturnType<typeof setInterval> | undefined;

	let chartEntries = $derived(data?.entries ?? []);
	let currentEntry = $derived(chartEntries[currentEntryIndex] ?? chartEntries[0] ?? null);
	let muralItems = $derived<ChartMuralItem[]>(
		chartEntries.map((entry) => ({
			id: String(entry.id),
			title: entry.title,
			subtitle: entrySubtitle(entry),
			artwork: entryArtwork(entry),
			fallbackText: entryFallbackText(entry),
			tileLabel: `Select ${entry.title}`,
			tileTitle: `${entry.rank}. ${entry.title} - ${entry.artist}`,
		})),
	);

	onMount(() => {
		void loadMatrix();
		void loadSnapshot(selectedRegion, selectedSource);
	});

	onDestroy(() => {
		destroyed = true;
		requestToken += 1;
		stopCarousel();
	});

	$effect(() => {
		if (chartEntries.length === 0) return;
		void resolveVisibleEntries(chartEntries);
	});

	$effect(() => {
		if (currentEntryIndex >= chartEntries.length) currentEntryIndex = 0;
	});

	$effect(() => {
		stopCarousel();
		if (chartEntries.length <= 1) return;
		carouselTimer = setInterval(() => {
			if (!carouselPaused) jumpEntry(1);
		}, ROTATE_MS);
		return stopCarousel;
	});

	async function loadMatrix() {
		matrixLoading = true;
		matrixError = false;
		try {
			const next = await api.getChartMatrix();
			if (destroyed) return;
			matrix = next;
			if (!refreshAttempted && !matrixHasData(next)) {
				refreshAttempted = true;
				await refreshMatrix();
			}
		} catch (e) {
			if (destroyed) return;
			console.error('[daily-charts] matrix fetch failed', e);
			matrix = null;
			matrixError = true;
		} finally {
			if (!destroyed) matrixLoading = false;
		}
	}

	async function refreshMatrix() {
		refreshingMatrix = true;
		try {
			await api.refreshChartMatrix();
			if (destroyed) return;
			matrix = await api.getChartMatrix();
			if (destroyed) return;
			void loadSnapshot(selectedRegion, selectedSource);
		} catch (e) {
			if (destroyed) return;
			console.error('[daily-charts] matrix refresh failed', e);
		} finally {
			if (!destroyed) refreshingMatrix = false;
		}
	}

	async function loadSnapshot(region: string, source: string) {
		const token = ++requestToken;
		loading = true;
		error = false;
		try {
			const next = await api.getChartSnapshot({
				source,
				period: PERIOD,
				region,
				limit: LIMIT,
			});
			if (destroyed || token !== requestToken) return;
			data = next;
			currentEntryIndex = 0;
			if (
				!snapshotRefreshAttempted &&
				!refreshingMatrix &&
				next.entries.length > 0 &&
				next.entries.length < Math.min(10, LIMIT)
			) {
				snapshotRefreshAttempted = true;
				void refreshMatrix();
			}
		} catch (e) {
			if (destroyed || token !== requestToken) return;
			console.error('[daily-charts] snapshot fetch failed', e);
			data = null;
			error = true;
		} finally {
			if (!destroyed && token === requestToken) loading = false;
		}
	}

	function pickRegion(region: string) {
		if (region === selectedRegion) return;
		selectedRegion = region;
		void loadSnapshot(region, selectedSource);
	}

	function pickProvider(region: string, source: string) {
		selectedRegion = region;
		selectedSource = source;
		void loadSnapshot(region, source);
	}

	function cellMetric(cell: ChartMatrixCell): string {
		if (cell.streams != null) return `${cell.streams.toLocaleString()} streams`;
		if (cell.views != null) return `${cell.views.toLocaleString()} views`;
		if (cell.points != null) return `${cell.points.toLocaleString()} pts`;
		if (resolvedTracks[cell.entry_id]?.in_library) return 'In library';
		if (resolvedTracks[cell.entry_id]) return 'TIDAL ready';
		if (resolvingEntries[cell.entry_id]) return 'Resolving';
		return 'Tap to resolve';
	}

	function matrixHasData(next: ChartMatrixResponse | null): boolean {
		return Boolean(next?.rows.some((row) =>
			next.providers.some((provider) => Boolean(row.cells[provider.source_key])),
		));
	}

	function regionHasMatrixData(region: string): boolean {
		const row = matrix?.rows.find((item) => item.region === region);
		return Boolean(
			row && matrix?.providers.some((provider) => Boolean(row.cells[provider.source_key])),
		);
	}

	function selectedRegionLabel(): string {
		return REGIONS.find((region) => region.code === selectedRegion)?.label ?? selectedRegion;
	}

	function selectedProviderLabel(): string {
		return (
			matrix?.providers.find((provider) => provider.source_key === selectedSource)?.label ??
			'Spotify'
		);
	}

	function stopCarousel() {
		if (carouselTimer) clearInterval(carouselTimer);
		carouselTimer = undefined;
	}

	function selectEntry(entryId: number) {
		const nextIndex = chartEntries.findIndex((entry) => entry.id === entryId);
		if (nextIndex >= 0) currentEntryIndex = nextIndex;
	}

	function jumpEntry(delta: number) {
		if (chartEntries.length === 0) return;
		currentEntryIndex = (currentEntryIndex + delta + chartEntries.length) % chartEntries.length;
	}

	function rankDeltaLabel(delta: number | null): string {
		if (delta == null || delta === 0) return 'Steady';
		if (delta < 0) return `Up ${Math.abs(delta)}`;
		return `Down ${delta}`;
	}

	async function resolveVisibleEntries(entries: ChartSnapshotEntry[]) {
		await Promise.all(entries.slice(0, LIMIT).map((entry) => resolveEntry(entry)));
	}

	async function resolveEntry(entry: ChartSnapshotEntry): Promise<TidalSearchTrack | null> {
		return resolveChartItem(entry.id, entry.artist, entry.title, entry.entity_type, entry.tidal_id);
	}

	async function resolveChartItem(
		entryId: number,
		artist: string,
		title: string,
		entityType: string,
		tidalId: number | null,
	): Promise<TidalSearchTrack | null> {
		if (tidalId || entityType === 'video') return null;
		if (entryId in resolvedTracks || resolvingEntries[entryId]) {
			return resolvedTracks[entryId] ?? null;
		}

		resolvingEntries = { ...resolvingEntries, [entryId]: true };
		try {
			const query = [artist, title].filter(Boolean).join(' ');
			// Shared gate, not a bare searchTidal: this shelf resolves its whole
			// visible page at once, and an uncapped fan-out here would blow past
			// the in-flight limit every other surface respects and trip TIDAL's
			// rejections for all of them.
			const results = await gatedTidalSearch(query, 1);
			if (destroyed) return null;
			const hit = results?.tracks[0] ?? null;
			resolvedTracks = { ...resolvedTracks, [entryId]: hit };
			return hit;
		} catch (e) {
			if (destroyed) return null;
			console.error('[daily-charts] tidal resolve failed', e);
			resolvedTracks = { ...resolvedTracks, [entryId]: null };
			return null;
		} finally {
			if (!destroyed) resolvingEntries = { ...resolvingEntries, [entryId]: false };
		}
	}

	async function playEntry(entry: ChartSnapshotEntry) {
		const hit = resolvedTracks[entry.id] ?? (await resolveEntry(entry));
		if (!hit) {
			playerError.set({ message: "Couldn't find that chart entry on TIDAL." });
			return;
		}
		await playTidalTrackNow(playableFromHit(hit, entry.artwork_url));
	}

	function playableFromHit(hit: TidalSearchTrack, fallbackArtwork: string | null): TidalPlayable {
		const playable = tidalSearchTrackToPlayable(hit);
		return {
			...playable,
			artwork_url: playable.artwork_url ?? fallbackArtwork,
		};
	}

	async function openEntryContext(e: MouseEvent, entry: ChartSnapshotEntry) {
		e.preventDefault();
		e.stopPropagation();
		const hit = resolvedTracks[entry.id] ?? (await resolveEntry(entry));
		if (!hit) return;
		const playable = playableFromHit(hit, entry.artwork_url);
		openContextMenu(e, buildTidalTrackMenu(playable), playable.title);
	}

	async function openMatrixCellContext(e: MouseEvent, cell: ChartMatrixCell) {
		e.preventDefault();
		e.stopPropagation();
		const hit =
			resolvedTracks[cell.entry_id] ??
			(await resolveChartItem(cell.entry_id, cell.artist, cell.title, cell.entity_type, cell.tidal_id));
		if (!hit) return;
		const playable = playableFromHit(hit, cell.artwork_url);
		openContextMenu(e, buildTidalTrackMenu(playable), playable.title);
	}

	function entryArtwork(entry: ChartSnapshotEntry): string | null {
		return resolvedTracks[entry.id]?.artwork_url ?? entry.artwork_url;
	}

	function entryFallbackText(entry: ChartSnapshotEntry): string {
		return (entry.title.trim()[0] ?? 'N').toUpperCase();
	}

	function entryStatusLabel(entry: ChartSnapshotEntry): string {
		if (entry.tidal_id || entry.resolution_status === 'tidal') return 'TIDAL ready';
		if (resolvedTracks[entry.id]?.in_library) return 'In library';
		if (resolvedTracks[entry.id]) return 'TIDAL ready';
		if (resolvingEntries[entry.id]) return 'Resolving';
		return entry.resolution_status;
	}

	function entryMetric(entry: ChartSnapshotEntry): string {
		if (entry.streams != null) return `${entry.streams.toLocaleString()} streams`;
		if (entry.views != null) return `${entry.views.toLocaleString()} views`;
		if (entry.points != null) return `${entry.points.toLocaleString()} pts`;
		return entryStatusLabel(entry);
	}

	function entrySubtitle(entry: ChartSnapshotEntry): string {
		return `#${entry.rank} ${rankDeltaLabel(entry.rank_delta)} - ${entry.artist}`;
	}
</script>

<section class="daily-chart-shelf">
	<SectionHeader eyebrow="Charts - Provider matrix" title="Market pulse" variant="charts" level={2}>
		{#snippet actions()}
		<div class="region-tabs" role="tablist" aria-label="Daily chart region">
			{#each REGIONS as region (region.code)}
				<button
					type="button"
					class="chip"
					class:active={region.code === selectedRegion}
					role="tab"
					aria-selected={region.code === selectedRegion}
					onclick={() => pickRegion(region.code)}
				>
					{region.label}
				</button>
			{/each}
		</div>
		{/snippet}
	</SectionHeader>

	{#if matrix?.providers.length}
		<div class="source-tabs" role="tablist" aria-label="Daily chart provider">
			{#each matrix.providers as provider (provider.source_key)}
				<button
					type="button"
					class="source-chip"
					class:active={provider.source_key === selectedSource}
					role="tab"
					aria-selected={provider.source_key === selectedSource}
					onclick={() => pickProvider(selectedRegion, provider.source_key)}
				>
					{provider.label}
				</button>
			{/each}
		</div>
	{/if}

	{#if chartEntries.length > 0 && currentEntry}
		<ChartMural
			items={muralItems}
			currentIndex={currentEntryIndex}
			ariaLabel={`${selectedProviderLabel()} ${selectedRegionLabel()} top ${chartEntries.length}`}
			kindLabel={`${selectedProviderLabel()} top ${chartEntries.length} - ${selectedRegionLabel()}`}
			title={currentEntry.title}
			subtitle={entrySubtitle(currentEntry)}
			metric={entryMetric(currentEntry)}
			actionLabel="Play"
			loading={loading && chartEntries.length === 0}
			loadingLabel="Loading chart mural"
			onSelect={(index) => {
				const entry = chartEntries[index];
				if (entry) selectEntry(entry.id);
			}}
			onJump={jumpEntry}
			onPlay={() => playEntry(currentEntry)}
			onCardContext={(event) => currentEntry && openEntryContext(event, currentEntry)}
			onItemContext={(event, index) => {
				const entry = chartEntries[index];
				if (entry) void openEntryContext(event, entry);
			}}
			onPauseChange={(paused) => carouselPaused = paused}
		/>
	{:else if loading}
		<ChartMural
			items={[]}
			currentIndex={0}
			ariaLabel="Loading market pulse"
			kindLabel=""
			title=""
			subtitle=""
			loading
			loadingLabel="Loading chart mural"
		/>
	{:else if error}
		<EmptyState
			title="Daily chart unavailable"
			copy="Restart the NOOR server if this update just landed."
		/>
	{:else if !matrixLoading}
		<EmptyState
			title={`No ${selectedProviderLabel()} top list for ${selectedRegionLabel()}`}
			copy="Try another provider or region."
		/>
	{/if}

	{#if matrix?.providers.length}
		<div class="matrix-shell" aria-label="Market pulse provider matrix">
			<div class="matrix-heading">
				<div>
					<p>Provider comparison</p>
					<h3>All markets</h3>
				</div>
				<span>{selectedRegionLabel()} focus</span>
			</div>
			<div class="matrix-grid">
				<div class="matrix-head region-head">Region</div>
				{#each matrix.providers as provider (provider.source_key)}
					<div class="matrix-head">{provider.label}</div>
				{/each}
				{#each matrix.rows as row (row.region)}
					<button
						type="button"
						class="matrix-region"
						class:active={row.region === selectedRegion}
						onclick={() => pickRegion(row.region)}
					>
						{row.region === 'global' ? 'Global' : row.region}
					</button>
					{#each matrix.providers as provider (provider.source_key)}
						{@const cell = row.cells[provider.source_key]}
						<button
							type="button"
							class="matrix-cell"
							class:filled={Boolean(cell)}
							class:active={Boolean(cell) && row.region === selectedRegion && provider.source_key === selectedSource}
							onclick={() => {
								if (cell) pickProvider(row.region, provider.source_key);
								else pickRegion(row.region);
							}}
							oncontextmenu={(e) => {
								if (cell) void openMatrixCellContext(e, cell);
							}}
							aria-label={`${provider.label} ${row.region}`}
						>
							{#if cell}
								<strong>{cell.title}</strong>
								<span>{cell.artist}</span>
								<small>{cellMetric(cell)}</small>
							{:else}
								<span>No data</span>
							{/if}
						</button>
					{/each}
				{/each}
			</div>
		</div>
	{:else if matrixLoading}
		<div class="provider-strip" aria-label="Loading chart providers">
			{#each Array.from({ length: 6 }) as _, i (i)}
				<span class="provider-skeleton">{refreshingMatrix ? 'Refreshing' : 'Loading'}</span>
			{/each}
		</div>
	{:else if matrixError}
		<EmptyState
			title="Market matrix unavailable"
			copy="Restart the NOOR server if this update just landed."
		/>
	{/if}

	{#if !matrixLoading && !matrixError && !regionHasMatrixData(selectedRegion)}
		<EmptyState
			title={matrixHasData(matrix)
				? `No provider leaders for ${selectedRegionLabel()}`
				: 'No market snapshot yet'}
			copy={matrixHasData(matrix)
				? 'This region has no provider leaders yet. Global data is available above.'
				: 'NOOR tried to refresh the provider matrix, but there is no stored chart data yet.'}
		/>
	{/if}
</section>

<style>
	.daily-chart-shelf {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.region-tabs {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		padding: var(--space-1);
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		background: var(--panel-bg);
		overflow-x: auto;
		max-width: 100%;
	}

	.provider-strip {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(112px, 100%), 1fr));
		gap: var(--space-1);
	}

	.provider-strip span {
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		background: var(--panel-bg);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		padding: var(--space-2) var(--space-3);
		text-align: center;
		white-space: nowrap;
	}

	.provider-skeleton {
		opacity: 0.55;
	}

	.source-tabs {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		overflow-x: auto;
		padding-bottom: var(--space-1);
	}

	.source-chip {
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		background: var(--panel-bg);
		color: var(--text-muted);
		cursor: pointer;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		padding: var(--space-2) var(--space-3);
		white-space: nowrap;
		transition: background var(--motion-base), border-color var(--motion-base), color var(--motion-base);
	}

	.source-chip:hover,
	.source-chip:focus-visible,
	.source-chip.active {
		background: var(--bg-hover);
		border-color: var(--accent-line);
		color: var(--text-primary);
		outline: none;
	}

	.matrix-shell {
		display: grid;
		gap: var(--space-2);
		overflow-x: auto;
		padding-bottom: var(--space-1);
	}

	.matrix-heading {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: var(--gap-sm);
		min-width: 920px;
	}

	.matrix-heading p,
	.matrix-heading h3 {
		margin: 0;
	}

	.matrix-heading p {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-muted);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	.matrix-heading h3 {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
	}

	.matrix-heading span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.matrix-grid {
		display: grid;
		grid-template-columns: clamp(72px, 8vw, 96px) repeat(6, minmax(136px, 1fr));
		gap: var(--space-1);
		min-width: 920px;
	}

	.matrix-head,
	.matrix-region,
	.matrix-cell {
		border: 1px solid var(--panel-border);
		background: var(--panel-bg);
		color: var(--text-secondary);
		border-radius: var(--radius-xs);
	}

	.matrix-head {
		padding: var(--space-2) var(--space-3);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		line-height: 1;
		text-align: left;
	}

	.region-head {
		color: var(--text-muted);
	}

	.matrix-region,
	.matrix-cell {
		min-height: clamp(58px, 6vw, 74px);
		padding: var(--space-2) var(--space-3);
		cursor: pointer;
		transition: background var(--motion-base), border-color var(--motion-base), color var(--motion-base);
	}

	.matrix-region {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-bold);
		text-align: left;
	}

	.matrix-region.active,
	.matrix-region:hover,
	.matrix-cell:hover,
	.matrix-cell:focus-visible,
	.matrix-region:focus-visible,
	.matrix-cell.active {
		background: var(--bg-hover);
		border-color: var(--accent-line);
		color: var(--text-primary);
		outline: none;
	}

	.matrix-cell {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		justify-content: center;
		gap: var(--space-1);
		text-align: left;
		min-width: 0;
	}

	.matrix-cell strong,
	.matrix-cell span,
	.matrix-cell small {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.matrix-cell strong {
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
	}

	.matrix-cell span {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	.matrix-cell small {
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
		line-height: 1;
	}

	.matrix-cell:not(.filled) {
		color: var(--text-muted);
		opacity: 0.72;
	}

	.chip {
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		background: var(--panel-bg);
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		padding: var(--space-2) var(--space-3);
		cursor: pointer;
		white-space: nowrap;
		transition: background var(--motion-base), border-color var(--motion-base), color var(--motion-base);
	}

	.chip:hover,
	.chip:focus-visible {
		background: var(--bg-hover);
		border-color: var(--accent-line);
		color: var(--text-primary);
		outline: none;
	}

	.chip.active {
		background: var(--bg-hover);
		border-color: var(--accent-line);
		color: var(--text-primary);
	}

</style>
