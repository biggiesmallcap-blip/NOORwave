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
	let refreshAttempted = false;
	let snapshotRefreshAttempted = false;
	let currentEntryIndex = $state(0);
	let carouselPaused = $state(false);
	let carouselTimer: ReturnType<typeof setInterval> | undefined;

	let chartEntries = $derived(data?.entries ?? []);
	let currentEntry = $derived(chartEntries[currentEntryIndex] ?? chartEntries[0] ?? null);

	onMount(() => {
		void loadMatrix();
		void loadSnapshot(selectedRegion, selectedSource);
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

	async function playEntry(entry: ChartSnapshotEntry) {
		const hit = resolvedTracks[entry.id] ?? (await resolveEntry(entry));
		if (!hit) {
			playerError.set({ message: "Couldn't find that chart entry on TIDAL." });
			return;
		}
		await playTidalTrackNow({
			tidal_id: hit.tidal_id,
			title: hit.title,
			artist_name: hit.artist_name,
			album_title: hit.album_title,
			artwork_url: hit.artwork_url ?? entry.artwork_url,
			duration_ms: hit.duration_ms,
			artist_tidal_id: hit.artist_id,
			album_tidal_id: hit.album_tidal_id,
		});
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
	<div class="section-header">
		<div class="section-title-group">
			<p class="eyebrow">Charts - Provider matrix</p>
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
		<div
			class="chart-mural-card"
			onmouseenter={() => carouselPaused = true}
			onmouseleave={() => carouselPaused = false}
			role="region"
			aria-label={`${selectedProviderLabel()} ${selectedRegionLabel()} top ${chartEntries.length}`}
		>
			<div class="chart-mural-bg" aria-hidden="true">
				{#each chartEntries as entry (entry.id)}
					<button
						class="chart-mural-tile"
						class:chart-mural-tile--featured={currentEntry.id === entry.id}
						type="button"
						onclick={() => selectEntry(entry.id)}
						aria-label={`Select ${entry.title}`}
						title={`${entry.rank}. ${entry.title} - ${entry.artist}`}
					>
						<ArtworkImage
							src={entryArtwork(entry)}
							size={320}
							className="chart-mural-art"
							fallbackText={entryFallbackText(entry)}
							decorative
						/>
					</button>
				{/each}
			</div>
			<div class="chart-mural-shade"></div>
			<div class="chart-mural-content">
				<div class="chart-mural-meta">
					<span class="chart-mural-kind">
						{selectedProviderLabel()} top {chartEntries.length} - {selectedRegionLabel()}
					</span>
					<h3 class="chart-mural-title">{currentEntry.title}</h3>
					<p class="chart-mural-sub">{entrySubtitle(currentEntry)}</p>
					<div class="chart-mural-actions">
						<button class="btn btn-primary chart-mural-play" type="button" onclick={() => void playEntry(currentEntry)}>
							<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
								<path d="M3 2.5l10 5.5-10 5.5V2.5z"/>
							</svg>
							Play
						</button>
						<span>{entryMetric(currentEntry)}</span>
					</div>
				</div>
			</div>
			{#if chartEntries.length > 1}
				<button class="chart-nav chart-nav--prev" type="button" onclick={() => jumpEntry(-1)} aria-label="Previous chart entry">&lsaquo;</button>
				<button class="chart-nav chart-nav--next" type="button" onclick={() => jumpEntry(1)} aria-label="Next chart entry">&rsaquo;</button>
			{/if}
		</div>
	{:else if loading}
		<div class="chart-mural-loading">Loading chart mural</div>
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

	.chart-mural-card {
		position: relative;
		min-height: clamp(220px, 24vw, 360px);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--panel-bg);
	}

	.chart-mural-bg {
		position: absolute;
		inset: -7%;
		z-index: 0;
		display: grid;
		grid-template-columns: repeat(10, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
		background: linear-gradient(120deg, var(--panel-bg), color-mix(in srgb, var(--accent-soft) 24%, transparent));
	}

	.chart-mural-bg::after {
		content: '';
		position: absolute;
		inset: 0;
		background:
			radial-gradient(circle at 78% 42%, rgba(255,255,255,0.2), transparent 30%),
			linear-gradient(90deg, rgba(0,0,0,0.06), transparent 42%, rgba(0,0,0,0.02));
		pointer-events: none;
	}

	.chart-mural-tile {
		appearance: none;
		position: relative;
		min-width: 0;
		min-height: 0;
		padding: 0;
		border: 0;
		background: var(--bg-raised);
		color: var(--text-primary);
		cursor: pointer;
		overflow: hidden;
		opacity: 0.96;
		filter: saturate(1.18) brightness(1.16);
		transform: skewX(-7deg) scaleX(1.08);
		transform-origin: center;
		transition:
			filter var(--motion-fast),
			opacity var(--motion-fast),
			transform var(--motion-base),
			box-shadow var(--motion-base);
	}

	.chart-mural-tile::after {
		content: '';
		position: absolute;
		inset: 0;
		background: linear-gradient(90deg, rgba(0,0,0,0.18), transparent 48%, rgba(0,0,0,0.2));
		opacity: 0.18;
		pointer-events: none;
	}

	.chart-mural-tile:hover,
	.chart-mural-tile:focus-visible,
	.chart-mural-tile--featured {
		z-index: var(--z-raised);
		opacity: 1;
		filter: saturate(1.8) brightness(1.42);
		transform: skewX(-7deg) scaleX(1.08) scale(1.045);
		box-shadow:
			0 0 0 1px rgba(255,255,255,0.32),
			0 14px 30px rgba(0,0,0,0.32),
			0 0 24px color-mix(in srgb, var(--accent) 38%, transparent);
		outline: none;
	}

	:global(.chart-mural-art),
	:global(.chart-mural-art.fallback) {
		display: block;
		width: 100%;
		height: 100%;
	}

	:global(.chart-mural-art) {
		object-fit: cover;
		transform: skewX(7deg) scale(1.24);
		transition: transform var(--motion-base);
	}

	.chart-mural-tile:hover :global(.chart-mural-art),
	.chart-mural-tile:focus-visible :global(.chart-mural-art),
	.chart-mural-tile--featured :global(.chart-mural-art) {
		transform: skewX(7deg) scale(1.34);
	}

	:global(.chart-mural-art.fallback) {
		display: grid;
		place-items: center;
		background: linear-gradient(135deg, var(--bg-raised), color-mix(in srgb, var(--accent-soft) 28%, var(--bg-surface)));
		color: rgba(255,255,255,0.78);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
	}

	.chart-mural-shade {
		position: absolute;
		inset: 0;
		z-index: var(--z-base);
		background: linear-gradient(90deg, rgba(0,0,0,0.7) 0%, rgba(0,0,0,0.34) 42%, rgba(0,0,0,0.06) 78%, transparent 100%);
		pointer-events: none;
	}

	.chart-mural-content {
		position: relative;
		z-index: calc(var(--z-base) + 1);
		display: grid;
		align-items: center;
		min-height: inherit;
		padding: var(--space-5);
		pointer-events: none;
	}

	.chart-mural-meta {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		max-width: min(42rem, 58vw);
		text-shadow: 0 2px 18px rgba(0,0,0,0.62);
	}

	.chart-mural-kind {
		color: var(--accent);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0;
		text-transform: uppercase;
	}

	.chart-mural-title {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.chart-mural-sub {
		margin: 0 0 var(--space-2);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.chart-mural-actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		pointer-events: auto;
	}

	.chart-mural-actions span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.chart-mural-play {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.chart-nav {
		position: absolute;
		top: 50%;
		z-index: var(--z-raised);
		display: grid;
		place-items: center;
		width: clamp(32px, 3vw, 40px);
		aspect-ratio: 1 / 1;
		border: 1px solid var(--panel-border);
		border-radius: 50%;
		background: rgba(0,0,0,0.5);
		color: var(--text-primary);
		cursor: pointer;
		font-size: var(--font-size-xl);
		line-height: 1;
		opacity: 0;
		transform: translateY(-50%);
		transition: opacity var(--motion-fast), background var(--motion-fast);
	}

	.chart-mural-card:hover .chart-nav,
	.chart-nav:focus-visible {
		opacity: 1;
		outline: none;
	}

	.chart-nav:hover {
		background: rgba(0,0,0,0.75);
	}

	.chart-nav--prev {
		left: var(--space-3);
	}

	.chart-nav--next {
		right: var(--space-3);
	}

	.chart-mural-loading {
		display: grid;
		place-items: center;
		min-height: clamp(180px, 20vw, 280px);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		background: var(--panel-bg);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
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

	@media (max-width: 760px) {
		.chart-mural-bg {
			grid-template-columns: repeat(5, minmax(0, 1fr));
			grid-template-rows: repeat(4, minmax(0, 1fr));
		}

		.chart-mural-content {
			padding: var(--space-4);
		}

		.chart-mural-meta {
			max-width: 100%;
		}

		.chart-mural-title {
			font-size: var(--font-size-3xl);
		}
	}
</style>
