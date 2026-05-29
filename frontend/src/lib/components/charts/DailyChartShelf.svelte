<script lang="ts">
	import { onMount } from 'svelte';
	import {
		api,
		type ChartMatrixCell,
		type ChartMatrixResponse,
		type ChartSnapshotEntry,
		type ChartSnapshotResponse,
		type TidalSearchTrack,
	} from '$lib/api/client';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { playTidalTrackNow, playerError } from '$lib/stores/player';

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

	let selectedRegion = $state('global');
	let selectedSource = $state('spotify_daily');
	let matrix = $state<ChartMatrixResponse | null>(null);
	let data = $state<ChartSnapshotResponse | null>(null);
	let resolvedTracks = $state<Record<number, TidalSearchTrack | null>>({});
	let resolvingEntries = $state<Record<number, boolean>>({});
	let loading = $state(true);
	let matrixLoading = $state(true);
	let refreshingMatrix = $state(false);
	let error = $state(false);
	let matrixError = $state(false);
	let requestToken = 0;
	let refreshAttempted = false;

	onMount(() => {
		void loadMatrix();
		void loadSnapshot(selectedRegion, selectedSource);
	});

	$effect(() => {
		const cells = selectedRegionCells();
		if (cells.length === 0) return;
		void resolveVisibleCells(cells);
	});

	$effect(() => {
		const entries = data?.entries ?? [];
		if (entries.length === 0) return;
		void resolveVisibleEntries(entries);
	});

	async function loadMatrix() {
		matrixLoading = true;
		matrixError = false;
		try {
			const next = await api.getChartMatrix();
			matrix = next;
			if (!refreshAttempted && !matrixHasData(next)) {
				refreshAttempted = true;
				await refreshMatrix();
			}
		} catch (e) {
			console.error('[daily-charts] matrix fetch failed', e);
			matrix = null;
			matrixError = true;
		} finally {
			matrixLoading = false;
		}
	}

	async function refreshMatrix() {
		refreshingMatrix = true;
		try {
			await api.refreshChartMatrix();
			matrix = await api.getChartMatrix();
			void loadSnapshot(selectedRegion, selectedSource);
		} catch (e) {
			console.error('[daily-charts] matrix refresh failed', e);
		} finally {
			refreshingMatrix = false;
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
			if (token !== requestToken) return;
			data = next;
		} catch (e) {
			console.error('[daily-charts] snapshot fetch failed', e);
			if (token !== requestToken) return;
			data = null;
			error = true;
		} finally {
			if (token === requestToken) loading = false;
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

	function rankDeltaLabel(delta: number | null): string {
		if (delta == null || delta === 0) return 'steady';
		return delta > 0 ? `up ${delta}` : `down ${Math.abs(delta)}`;
	}

	function formatMetric(entry: ChartSnapshotEntry): string {
		if (entry.streams != null) return `${entry.streams.toLocaleString()} streams`;
		if (entry.views != null) return `${entry.views.toLocaleString()} views`;
		if (entry.points != null) return `${entry.points.toLocaleString()} pts`;
		return entryStatusLabel(entry);
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

	function selectedRegionCells(): ChartMatrixCell[] {
		const row = matrix?.rows.find((item) => item.region === selectedRegion);
		if (!row || !matrix) return [];
		return matrix.providers
			.map((provider) => row.cells[provider.source_key])
			.filter((cell): cell is ChartMatrixCell => Boolean(cell));
	}

	function selectedProviderLabel(): string {
		return (
			matrix?.providers.find((provider) => provider.source_key === selectedSource)?.label ??
			'Spotify'
		);
	}

	async function resolveVisibleCells(cells: ChartMatrixCell[]) {
		await Promise.all(cells.slice(0, 6).map((cell) => resolveCell(cell)));
	}

	async function resolveVisibleEntries(entries: ChartSnapshotEntry[]) {
		await Promise.all(entries.slice(0, 12).map((entry) => resolveEntry(entry)));
	}

	async function resolveCell(cell: ChartMatrixCell): Promise<TidalSearchTrack | null> {
		return resolveChartItem(cell.entry_id, cell.artist, cell.title, cell.entity_type, cell.tidal_id);
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
			const results = await api.searchTidal(query, 1);
			const hit = results.tracks[0] ?? null;
			resolvedTracks = { ...resolvedTracks, [entryId]: hit };
			return hit;
		} catch (e) {
			console.error('[daily-charts] tidal resolve failed', e);
			resolvedTracks = { ...resolvedTracks, [entryId]: null };
			return null;
		} finally {
			resolvingEntries = { ...resolvingEntries, [entryId]: false };
		}
	}

	async function playCell(cell: ChartMatrixCell) {
		const hit = resolvedTracks[cell.entry_id] ?? (await resolveCell(cell));
		if (!hit) {
			playerError.set({ message: "Couldn't find that chart entry on TIDAL." });
			return;
		}
		await playTidalTrackNow({
			tidal_id: hit.tidal_id,
			title: hit.title,
			artist_name: hit.artist_name,
			album_title: hit.album_title,
			artwork_url: hit.artwork_url ?? cell.artwork_url,
			duration_ms: hit.duration_ms,
			artist_tidal_id: hit.artist_id,
			album_tidal_id: hit.album_tidal_id,
		});
	}

	function cellArtwork(cell: ChartMatrixCell): string | null {
		return resolvedTracks[cell.entry_id]?.artwork_url ?? cell.artwork_url;
	}

	function entryArtwork(entry: ChartSnapshotEntry): string | null {
		return resolvedTracks[entry.id]?.artwork_url ?? entry.artwork_url;
	}

	function entryStatusLabel(entry: ChartSnapshotEntry): string {
		if (entry.tidal_id || entry.resolution_status === 'tidal') return 'TIDAL ready';
		if (resolvedTracks[entry.id]?.in_library) return 'In library';
		if (resolvedTracks[entry.id]) return 'TIDAL ready';
		if (resolvingEntries[entry.id]) return 'Resolving';
		return entry.resolution_status;
	}

	function cellFallbackText(cell: ChartMatrixCell): string {
		return (cell.title.trim()[0] ?? 'N').toUpperCase();
	}

	function fallbackText(entry: ChartSnapshotEntry): string {
		return (entry.title.trim()[0] ?? 'N').toUpperCase();
	}
</script>

<section class="daily-chart-shelf">
	<div class="section-header">
		<div class="section-title-group">
			<p class="eyebrow">Charts · Provider matrix</p>
			<h2>Market pulse</h2>
		</div>
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
	</div>

	{#if matrix?.providers.length}
		<div class="region-pulse" aria-label={`${selectedRegionLabel()} provider leaders`}>
			<div class="region-pulse-copy">
				<p>{selectedRegionLabel()} chart leaders</p>
				<h3>{selectedProviderLabel()}</h3>
			</div>
			<div class="provider-card-row">
				{#each matrix.providers as provider (provider.source_key)}
					{@const row = matrix.rows.find((item) => item.region === selectedRegion)}
					{@const cell = row?.cells[provider.source_key] ?? null}
					<button
						type="button"
						class="provider-card"
						class:active={selectedSource === provider.source_key}
						class:empty={!cell}
						onclick={() => {
							if (cell) {
								pickProvider(selectedRegion, provider.source_key);
								void playCell(cell);
							}
						}}
						aria-label={`${provider.label} ${selectedRegionLabel()} leader`}
					>
						<span class="provider-name">{provider.label}</span>
						{#if cell}
							<div class="provider-art">
								<ArtworkImage
									src={cellArtwork(cell)}
									size={320}
									className="provider-card-art"
									fallbackText={cellFallbackText(cell)}
									decorative
								/>
							</div>
							<strong>{cell.title}</strong>
							<span>{cell.artist}</span>
							<small>{cellMetric(cell)}</small>
						{:else}
							<div class="provider-art empty-art"></div>
							<strong>No data</strong>
							<span>{selectedRegionLabel()}</span>
							<small>Provider missing</small>
						{/if}
					</button>
				{/each}
			</div>
		</div>

		<div class="matrix-shell" aria-label="Market pulse provider matrix">
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

	{#if data?.snapshot && data.entries.length > 0}
		<div class="chart-list" aria-label="Daily chart entries">
			<div class="list-heading">
				<p>{selectedRegionLabel()} · {selectedProviderLabel()}</p>
				<h3>Provider snapshot</h3>
			</div>
			{#each data.entries as entry (entry.id)}
				<div class="chart-row">
					<div class="rank">
						<span>{entry.rank}</span>
						<small>{rankDeltaLabel(entry.rank_delta)}</small>
					</div>
					<div class="art-cell">
						<ArtworkImage
							src={entryArtwork(entry)}
							size={320}
							className="daily-chart-art"
							fallbackText={fallbackText(entry)}
							decorative
						/>
					</div>
					<div class="track-meta">
						<strong>{entry.title}</strong>
						<span>{entry.artist}</span>
					</div>
					<div class="metric">{formatMetric(entry)}</div>
					<div class="status" data-status={entry.resolution_status}>
						{entryStatusLabel(entry)}
					</div>
				</div>
			{/each}
		</div>
	{:else if loading}
		<div class="chart-list" aria-label="Loading daily chart">
			{#each Array.from({ length: 6 }) as _, i (i)}
				<div class="chart-row skeleton" aria-hidden="true"></div>
			{/each}
		</div>
	{:else if error}
		<EmptyState
			title="Daily snapshots unavailable"
			copy="Restart the NOOR server if this update just landed."
		/>
	{:else}
		<EmptyState
			title={matrixHasData(matrix)
				? `No Spotify daily list for ${selectedRegionLabel()}`
				: 'No market snapshot yet'}
			copy={matrixHasData(matrix)
				? regionHasMatrixData(selectedRegion)
					? `This region has provider leaders, but no ${selectedProviderLabel()} detail list yet.`
					: 'This region has no provider snapshot yet. Global data is available above.'
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

	.section-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--gap-sm);
		flex-wrap: wrap;
	}

	.section-title-group {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}

	.eyebrow {
		margin: 0;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0;
		color: var(--service-spotify);
	}

	h2 {
		margin: 0;
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
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

	.region-pulse {
		display: grid;
		gap: var(--space-3);
	}

	.region-pulse-copy {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: var(--gap-sm);
	}

	.region-pulse-copy p,
	.region-pulse-copy h3 {
		margin: 0;
	}

	.region-pulse-copy p {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-secondary);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	.region-pulse-copy h3 {
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
	}

	.provider-card-row {
		display: grid;
		grid-template-columns: repeat(6, minmax(132px, 1fr));
		gap: var(--space-2);
		overflow-x: auto;
		padding-bottom: var(--space-1);
	}

	.provider-card {
		position: relative;
		display: grid;
		grid-template-rows: auto auto auto auto auto;
		align-content: start;
		gap: var(--space-1);
		min-width: 132px;
		min-height: clamp(210px, 19vw, 260px);
		padding: var(--space-2);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		background: var(--panel-bg);
		color: var(--text-secondary);
		text-align: left;
		cursor: pointer;
		transition: background var(--motion-base), border-color var(--motion-base), color var(--motion-base);
	}

	.provider-card:hover,
	.provider-card:focus-visible,
	.provider-card.active {
		border-color: var(--accent-line);
		background: var(--bg-hover);
		color: var(--text-primary);
		outline: none;
	}

	.provider-card.empty {
		cursor: default;
		opacity: 0.62;
	}

	.provider-name {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		color: var(--service-spotify);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	.provider-art {
		width: 100%;
		aspect-ratio: 1 / 1;
		border-radius: var(--radius-xs);
		overflow: hidden;
		background: var(--bg-raised);
	}

	:global(.provider-card-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	:global(.provider-card-art.fallback),
	.empty-art {
		display: grid;
		place-items: center;
		background: linear-gradient(135deg, var(--bg-raised), var(--panel-bg));
		color: var(--text-muted);
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-bold);
	}

	.provider-card strong,
	.provider-card span,
	.provider-card small {
		min-width: 0;
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.provider-card strong {
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
	}

	.provider-card span:not(.provider-name) {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	.provider-card small {
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
	}

	.matrix-shell {
		overflow-x: auto;
		padding-bottom: var(--space-1);
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
		border: 0;
		border-radius: 999px;
		background: transparent;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		padding: var(--space-2) var(--space-3);
		cursor: pointer;
		white-space: nowrap;
		transition: background var(--motion-base), color var(--motion-base);
	}

	.chip:hover,
	.chip:focus-visible {
		color: var(--text-primary);
		outline: none;
	}

	.chip.active {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.chart-list {
		display: grid;
		gap: var(--space-1);
	}

	.list-heading {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--gap-sm);
		padding-top: var(--space-1);
	}

	.list-heading p,
	.list-heading h3 {
		margin: 0;
	}

	.list-heading p {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-muted);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	.list-heading h3 {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
	}

	.chart-row {
		display: grid;
		grid-template-columns: clamp(44px, 5vw, 64px) clamp(42px, 4vw, 52px) minmax(0, 1fr) auto auto;
		align-items: center;
		gap: var(--space-3);
		min-height: clamp(56px, 6vw, 68px);
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		background: var(--panel-bg);
	}

	.rank {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		line-height: var(--line-height-tight);
	}

	.rank span {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
	}

	.rank small {
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	.art-cell {
		width: clamp(42px, 4vw, 52px);
		aspect-ratio: 1 / 1;
		border-radius: var(--radius-xs);
		overflow: hidden;
		background: var(--bg-raised);
	}

	:global(.daily-chart-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	:global(.daily-chart-art.fallback) {
		display: grid;
		place-items: center;
		color: var(--text-muted);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.track-meta {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}

	.track-meta strong,
	.track-meta span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.track-meta strong {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
	}

	.track-meta span {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	.metric,
	.status {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-secondary);
		white-space: nowrap;
	}

	.status {
		border-radius: 999px;
		padding: var(--space-1) var(--space-2);
		background: var(--bg-hover);
		color: var(--text-muted);
		text-transform: capitalize;
	}

	.status[data-status='local'],
	.status[data-status='tidal'] {
		color: var(--text-primary);
	}

	.skeleton {
		min-height: clamp(56px, 6vw, 68px);
		background: linear-gradient(90deg, var(--panel-bg), var(--bg-hover), var(--panel-bg));
		background-size: 200% 100%;
		animation: pulse 1.2s linear infinite;
	}

	@keyframes pulse {
		from { background-position: 200% 0; }
		to { background-position: -200% 0; }
	}

	@media (max-width: 760px) {
		.provider-card-row {
			grid-template-columns: repeat(6, minmax(118px, 38vw));
		}

		.chart-row {
			grid-template-columns: clamp(36px, 10vw, 48px) clamp(42px, 12vw, 52px) minmax(0, 1fr);
		}

		.metric,
		.status {
			grid-column: 3;
		}
	}
</style>
