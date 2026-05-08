<script lang="ts">
	/**
	 * Tempo ridges — stacked BPM histograms.
	 *
	 * Each row = one time bucket (day / week / month, decided server-side per the
	 * Granularity rule). The ridge inside a row is a smoothed BPM histogram across
	 * the [60, 200) range in 4-BPM steps. Per-row normalisation makes every active
	 * row reach the same peak height — the SHAPE differs, height matches.
	 *
	 * Right sidecar shows window-wide MEDIAN / MODE / SPREAD / COVERAGE. The
	 * MEDIAN and MODE rows include a tiny inline histogram of the full-window
	 * BPM distribution; MODE marks the argmax bucket with a vertical tick.
	 *
	 * Spec: C:\Users\Felix\.claude\plans\lets-revision-analytics-stats-crystalline-melody.md
	 */

	import type { TempoView, BpmBucket } from '$lib/api/client';
	import { formatBpm, formatCount, formatDate } from '$lib/utils/format';
	import { kde1d, ridgePath, rowMax } from './ridge-kde';

	interface Props {
		tempo: TempoView;
		/** KDE bandwidth in bin-widths. Default 0.6 matches ListenRidgeline. */
		sigma?: number;
	}

	let { tempo, sigma = 0.6 }: Props = $props();

	const KDE_SAMPLES = 144; // 4 samples per bin (36 × 4)
	const ROW_AMP_FACTOR = 4.0;
	const TOP_HEADROOM_FACTOR = ROW_AMP_FACTOR + 0.2;
	/**
	 * Tempo reads as a banner — short and wide, not a portrait card. With ~30 rows
	 * at 8px each plus padding the chart lands ~280px tall, ~3.5:1 aspect at full width.
	 * For sparse data (1–4 rows) it floors at 200 so a single-row case still has visual
	 * presence rather than collapsing to a thin sliver.
	 */
	const ROW_HEIGHT_DEFAULT = 8;

	// BPM tick axis values (every 20 BPM from 60 to 180; 200 is the open right edge).
	const BPM_TICKS = [60, 80, 100, 120, 140, 160, 180];

	let chartEl: SVGSVGElement | undefined = $state(undefined);
	let chartWidth = $state(900);
	let hover = $state<{ rowIdx: number; bucketIdx: number } | null>(null);

	const rows = $derived(tempo.rows);
	const ROW_COUNT = $derived(rows.length);
	const granularity = $derived(rows[0]?.granularity ?? 'day');

	const CHART_HEIGHT = $derived(
		Math.max(200, Math.min(320, ROW_COUNT * ROW_HEIGHT_DEFAULT + 80)),
	);

	const PADDING = { top: 16, bottom: 38, left: 78, right: 16 };
	const PLOT_HEIGHT = $derived(CHART_HEIGHT - PADDING.top - PADDING.bottom);

	const ROW_SPACING = $derived.by(() => {
		// Unified formula. For ROW_COUNT === 1 this gives rowSpacing = PLOT_HEIGHT / 4.2,
		// placing the single row's baseline at the chart bottom with the peak rising into
		// the upper 80% of the canvas. Works for any row count without a special case.
		if (ROW_COUNT === 0) return 24;
		return PLOT_HEIGHT / Math.max(1, ROW_COUNT - 1 + TOP_HEADROOM_FACTOR);
	});

	const TOP_HEADROOM = $derived(ROW_SPACING * TOP_HEADROOM_FACTOR);

	/**
	 * AMP formula. Tempo's row counts are usually modest (4–13 weekly, 4–12 monthly),
	 * so we use the dense formula down to 4 rows. For 1–3 rows we still cap at row-spacing
	 * to avoid the topmost peak shooting past the chart top.
	 */
	const AMP = $derived.by(() => {
		if (ROW_COUNT >= 4) return ROW_SPACING * ROW_AMP_FACTOR;
		return Math.min(PLOT_HEIGHT * 0.55, ROW_SPACING * 0.85);
	});

	// Each row's bucket array → KDE-smoothed density.
	const smoothed = $derived(
		rows.map((r) => kde1d(r.buckets.map((b) => b.listens), sigma, KDE_SAMPLES)),
	);
	const rowMaxes = $derived(smoothed.map(rowMax));

	// Window-wide histogram for the sidecar silhouettes.
	const windowHistogram = $derived.by((): BpmBucket[] => {
		if (rows.length === 0) return [];
		const totals = new Array<number>(rows[0].buckets.length).fill(0);
		const buckets = rows[0].buckets.map((b) => b.bucket);
		for (const r of rows) {
			for (let i = 0; i < r.buckets.length; i++) {
				totals[i] += r.buckets[i].listens;
			}
		}
		return buckets.map((bucket, i) => ({ bucket, listens: totals[i] }));
	});

	const histMax = $derived(windowHistogram.reduce((m, b) => (b.listens > m ? b.listens : m), 0));

	// Mode bucket index — argmax across the window histogram.
	const modeBucketIdx = $derived.by(() => {
		let best = -1;
		let bestVal = 0;
		for (let i = 0; i < windowHistogram.length; i++) {
			if (windowHistogram[i].listens > bestVal) {
				bestVal = windowHistogram[i].listens;
				best = i;
			}
		}
		return best;
	});

	// Hover plumbing.
	$effect(() => {
		if (!chartEl) return;
		const ro = new ResizeObserver((entries) => {
			for (const entry of entries) chartWidth = Math.round(entry.contentRect.width);
		});
		ro.observe(chartEl);
		return () => ro.disconnect();
	});

	function pixelXToBucketIdx(px: number, plotWidth: number, bucketCount: number): number {
		const idx = (px / plotWidth) * (bucketCount - 1);
		return Math.max(0, Math.min(bucketCount - 1, Math.round(idx)));
	}

	function handlePointerMove(e: PointerEvent) {
		if (!chartEl) return;
		const rect = chartEl.getBoundingClientRect();
		const px = e.clientX - rect.left - PADDING.left;
		const py = e.clientY - rect.top - PADDING.top;
		const plotWidth = chartWidth - PADDING.left - PADDING.right;
		if (px < 0 || px > plotWidth || py < 0 || py > PLOT_HEIGHT) {
			hover = null;
			return;
		}
		const rowIdx = Math.max(
			0,
			Math.min(ROW_COUNT - 1, Math.round((py - TOP_HEADROOM) / ROW_SPACING)),
		);
		const bucketIdx = pixelXToBucketIdx(px, plotWidth, rows[0]?.buckets.length ?? 36);
		hover = { rowIdx, bucketIdx };
	}

	function handlePointerLeave() {
		hover = null;
	}

	const hoverBpm = $derived.by(() => {
		if (!hover) return null;
		return rows[hover.rowIdx]?.buckets[hover.bucketIdx]?.bucket ?? null;
	});

	const hoverListens = $derived.by(() => {
		if (!hover) return 0;
		return rows[hover.rowIdx]?.buckets[hover.bucketIdx]?.listens ?? 0;
	});

	const hoverRowLabel = $derived.by(() => {
		if (!hover) return null;
		const row = rows[hover.rowIdx];
		if (!row) return null;
		return formatDate(row.label, granularity, { window: rows });
	});

	function bucketAxisX(bucket: number, plotWidth: number): number {
		const { min, max } = tempo.bucket_axis;
		return ((bucket - min) / (max - min)) * plotWidth;
	}
</script>

<div class="tempo">
	<div class="chart-wrap">
		<svg
			bind:this={chartEl}
			class="chart"
			viewBox="0 0 {chartWidth} {CHART_HEIGHT}"
			preserveAspectRatio="none"
			onpointermove={handlePointerMove}
			onpointerleave={handlePointerLeave}
			role="img"
			aria-label="Tempo distribution across the window, one ridge per {granularity}"
		>
			<!-- Row labels (left gutter) — sparse cadence: every 7th for daily, every row otherwise. -->
			<g class="row-labels" transform="translate({PADDING.left - 8} {PADDING.top})">
				{#each rows as row, i (row.label)}
					{@const baseY = TOP_HEADROOM + i * ROW_SPACING}
					{@const labelEvery = granularity === 'day' ? 7 : 1}
					{#if i % labelEvery === 0}
						<text x={0} y={baseY + 3} text-anchor="end">
							{formatDate(row.label, granularity, { window: rows })}
						</text>
					{/if}
				{/each}
			</g>

			<!-- Ridges -->
			<g class="ridges" transform="translate({PADDING.left} {PADDING.top})">
				{#each smoothed as density, i (rows[i]?.label ?? i)}
					{@const baseY = TOP_HEADROOM + i * ROW_SPACING}
					{@const isEmpty = rowMaxes[i] === 0}
					<path
						class:empty={isEmpty}
						d={ridgePath(density, baseY, AMP, chartWidth - PADDING.left - PADDING.right, rowMaxes[i])}
					/>
				{/each}

				{#if hover}
					{@const plotWidth = chartWidth - PADDING.left - PADDING.right}
					{@const guideX = (hover.bucketIdx / (rows[0]?.buckets.length ?? 36) - 1) * plotWidth + plotWidth}
					<line
						class="hover-guide"
						x1={hover.bucketIdx / ((rows[0]?.buckets.length ?? 36) - 1) * plotWidth}
						x2={hover.bucketIdx / ((rows[0]?.buckets.length ?? 36) - 1) * plotWidth}
						y1={0}
						y2={PLOT_HEIGHT}
					/>
				{/if}
			</g>

			<!-- BPM tick axis at the bottom -->
			<g class="axis" transform="translate({PADDING.left} {PADDING.top + PLOT_HEIGHT + 4})">
				{#each BPM_TICKS as tick (tick)}
					{@const x = bucketAxisX(tick, chartWidth - PADDING.left - PADDING.right)}
					<line {x} x1={x} x2={x} y1={-PLOT_HEIGHT - 4} y2={-PLOT_HEIGHT + 2} class="axis-tick" />
					<text {x} y={18} text-anchor="middle">{tick}</text>
				{/each}
				<text
					x={(chartWidth - PADDING.left - PADDING.right) / 2}
					y={36}
					text-anchor="middle"
					class="axis-unit"
				>
					BPM
				</text>
			</g>
		</svg>

		{#if hover && hoverRowLabel && hoverBpm !== null}
			<div class="tooltip" role="status" aria-live="polite">
				{hoverRowLabel} · {formatBpm(hoverBpm, { decimals: 0, suffix: 'BPM' })} · {formatCount(hoverListens)}
				{hoverListens === 1 ? 'play' : 'plays'}
			</div>
		{/if}
	</div>

	<aside class="sidecar">
		<div class="stat">
			<div class="stat-label">Median</div>
			<div class="stat-row">
				<div class="stat-value">{formatBpm(tempo.stats.median, { decimals: 0, suffix: 'BPM' })}</div>
				<svg class="silhouette" viewBox="0 0 80 24" preserveAspectRatio="none" aria-hidden="true">
					{#if histMax > 0 && windowHistogram.length > 1}
						{#each windowHistogram as b, i (b.bucket)}
							{@const x = (i / (windowHistogram.length - 1)) * 80}
							{@const h = (b.listens / histMax) * 22}
							{#if h > 0}
								<rect x={x - 0.7} y={22 - h} width={1.4} height={h} />
							{/if}
						{/each}
					{/if}
				</svg>
			</div>
		</div>
		<div class="stat">
			<div class="stat-label">Mode</div>
			<div class="stat-row">
				<div class="stat-value">{formatBpm(tempo.stats.mode, { decimals: 0, suffix: 'BPM' })}</div>
				<svg class="silhouette" viewBox="0 0 80 24" preserveAspectRatio="none" aria-hidden="true">
					{#if histMax > 0 && windowHistogram.length > 1}
						{#each windowHistogram as b, i (b.bucket)}
							{@const x = (i / (windowHistogram.length - 1)) * 80}
							{@const h = (b.listens / histMax) * 22}
							{@const isMode = i === modeBucketIdx}
							{#if h > 0}
								<rect class:mode={isMode} x={x - 0.7} y={22 - h} width={1.4} height={h} />
							{/if}
						{/each}
					{/if}
				</svg>
			</div>
		</div>
		<div class="stat">
			<div class="stat-label">Spread</div>
			<div class="stat-value">{formatBpm(tempo.stats.sigma, { decimals: 1, suffix: '' })}</div>
		</div>
		<div class="stat">
			<div class="stat-label">Coverage</div>
			<div class="stat-value coverage">
				{tempo.coverage.analyzed === 0 && tempo.coverage.total_listened === 0
					? '--'
					: `${formatCount(tempo.coverage.analyzed)} / ${formatCount(tempo.coverage.total_listened)}`}
			</div>
		</div>
	</aside>
</div>

<!-- SR-only summary -->
<table class="sr-only" aria-label="Tempo ridges summary">
	<caption>
		Median {formatBpm(tempo.stats.median, { decimals: 0, suffix: 'BPM' })};
		Mode {formatBpm(tempo.stats.mode, { decimals: 0, suffix: 'BPM' })};
		Spread {formatBpm(tempo.stats.sigma, { decimals: 1, suffix: '' })};
		Coverage {tempo.coverage.analyzed} of {tempo.coverage.total_listened} listened tracks analysed.
	</caption>
	<thead>
		<tr>
			<th scope="col">Row</th>
			<th scope="col">Top bucket BPM</th>
			<th scope="col">Top bucket listens</th>
		</tr>
	</thead>
	<tbody>
		{#each rows as row (row.label)}
			{@const top = row.buckets.reduce((acc, b) => (b.listens > acc.listens ? b : acc), row.buckets[0] ?? { bucket: 0, listens: 0 })}
			<tr>
				<th scope="row">{formatDate(row.label, granularity, { window: rows })}</th>
				<td>{top.bucket}</td>
				<td>{top.listens}</td>
			</tr>
		{/each}
	</tbody>
</table>

<style>
	.tempo {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 200px;
		gap: var(--space-4);
		width: 100%;
		max-width: 1220px;
		margin: 0 auto;
		align-items: center;
	}

	.chart-wrap {
		position: relative;
		min-width: 0;
	}

	.chart {
		width: 100%;
		height: auto;
		display: block;
	}

	.row-labels text {
		font-family: var(--font-mono);
		font-size: 0.6rem;
		letter-spacing: 0.04em;
		fill: var(--text-tertiary);
	}

	.ridges path {
		fill: var(--bg-base);
		stroke: var(--text-primary);
		stroke-width: 1.2;
		stroke-linejoin: round;
		stroke-linecap: round;
		vector-effect: non-scaling-stroke;
	}

	.ridges path.empty {
		stroke: var(--text-primary);
		stroke-opacity: 0.32;
		stroke-width: 0.8;
		stroke-dasharray: 1 2;
		fill: transparent;
	}

	.hover-guide {
		stroke: var(--text-primary);
		stroke-width: 1;
		stroke-dasharray: 2 3;
		opacity: 0.4;
		pointer-events: none;
	}

	.axis text {
		font-family: var(--font-mono);
		font-size: 0.62rem;
		letter-spacing: 0.08em;
		fill: var(--text-tertiary);
	}

	.axis .axis-unit {
		font-size: 0.58rem;
		letter-spacing: 0.18em;
		fill: var(--text-muted);
	}

	.axis-tick {
		stroke: var(--border-subtle);
		stroke-width: 1;
		stroke-dasharray: 1 3;
	}

	.tooltip {
		position: absolute;
		top: 8px;
		right: 16px;
		padding: 6px 10px;
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-xs);
		font-family: var(--font-mono);
		font-size: 0.68rem;
		color: var(--text-secondary);
		pointer-events: none;
		white-space: nowrap;
	}

	.sidecar {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-2) 0 var(--space-2) var(--space-4);
		min-width: 180px;
		border-left: 1px solid var(--border-subtle);
	}

	.stat {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	.stat-label {
		font-family: var(--font-mono);
		font-size: 0.62rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.stat-row {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.stat-value {
		font-family: var(--font-display);
		font-size: 1.25rem;
		font-weight: 500;
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
		line-height: 1;
	}

	.stat-value.coverage {
		font-size: 0.95rem;
	}

	.silhouette {
		width: 76px;
		height: 22px;
		flex: 0 0 auto;
	}

	.silhouette rect {
		fill: var(--text-primary);
		fill-opacity: 0.62;
	}

	.silhouette rect.mode {
		fill: var(--accent-strong);
		fill-opacity: 1;
	}

	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	@media (max-width: 720px) {
		.tempo {
			grid-template-columns: 1fr;
		}
		.sidecar {
			flex-direction: row;
			flex-wrap: wrap;
			gap: var(--space-3) var(--space-5);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.tempo :where(*, *::before, *::after) {
			transition: none !important;
			animation: none !important;
		}
	}
</style>
