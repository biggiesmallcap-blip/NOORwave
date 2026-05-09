<script lang="ts">
	/**
	 * Listening pulse — Joy-Division-style stacked ridges.
	 *
	 * Two modes:
	 *   "hero"  — large, with title block + caption + left stat spine. Used at the top of /analytics.
	 *   "solo"  — just the ridges. Used in narrower contexts.
	 *
	 * Two spine variants (hero only):
	 *   "default"     — Peak Hour · Rhythm · Night · Morning (4 stats)
	 *   "single-day"  — Peak Hour · Longest session · Listens · Tracks  (used at days <= 1)
	 *
	 * Spec: C:\Users\Felix\.claude\plans\lets-revision-analytics-stats-crystalline-melody.md
	 *
	 * The occlusion trick: each ridge is a closed `<path>` filled with the page background,
	 * stroked with text-primary. Rows render top-first so later rows occlude earlier ones —
	 * that's the entire JD effect. fill="none" produces a wireframe and loses the mountains.
	 */

	import type { RidgeRow, HeroStats } from '$lib/api/client';
	import { formatHour, formatDate, formatCount, formatPercent, formatDuration } from '$lib/utils/format';
	import { kde1d, ridgePath, rowMax } from './ridge-kde';
	import { onMount } from 'svelte';

	/**
	 * KDE bandwidth in hours. Locked at 0.6 for both daily and weekly modes — the
	 * weekly aggregation already cleans the noise floor (each weekly row is the sum
	 * of 7 daily arrays, so individual hour spikes get averaged out), and a tighter
	 * sigma there made the peaks feel jagged. 0.6 reads consistently across windows.
	 *
	 * The two constants stay separate so future tuning can diverge them per-mode if
	 * the data character changes (e.g. monthly aggregation past 730 days).
	 *
	 * Locked after visual tuning at /analytics/preview/ridgeline. Per-row normalisation
	 * gives every active row equal vertical weight regardless of absolute volume —
	 * what differs is the SHAPE of when listening happened.
	 */
	const RIDGE_SIGMA_DAILY = 0.6;
	const RIDGE_SIGMA_WEEKLY = 0.6;
	const WEEKLY_GRANULARITY_THRESHOLD = 90;

	/**
	 * SPINE_SIGMA is the KDE bandwidth used to compute the spine "Peak hour"
	 * stat — independent from the chart's slider so the number reflects where
	 * the user's listening *clusters* even when the chart is rendered sharp.
	 * 1.0 hour merges adjacent-hour clusters (9-10am or 12-13pm collapse to a
	 * single peak) without bleeding distant hours together.
	 */
	const SPINE_SIGMA = 1.0;

	// User-tunable chart smoothing slider (hero mode only, when no external
	// sigma prop is provided — the dev preview route still drives sigma
	// externally and overrides the slider). Persisted per-user.
	const SLIDER_KEY = 'noor:analytics:listening-pulse:sigma';
	const SLIDER_MIN = 0.3;
	const SLIDER_MAX = 1.5;
	const SLIDER_STEP = 0.05;
	const SLIDER_DEFAULT = RIDGE_SIGMA_DAILY;

	interface Props {
		rows: RidgeRow[];
		heroStats?: HeroStats | null;
		mode: 'hero' | 'solo';
		variant?: 'default' | 'single-day';
		/** KDE bandwidth — visually tuned. Set via the preview route in dev. */
		sigma?: number;
		/** Backend-supplied amplitude divisor (P95 across all per-row hours). */
		ridgeAmpMax?: number | null;
	}

	let {
		rows,
		heroStats = null,
		mode,
		variant = 'default',
		sigma,
		ridgeAmpMax = null,
	}: Props = $props();

	/**
	 * Past 90 daily rows we aggregate every 7 days into a single "week" row so the chart
	 * stays legible. A 6-month window collapses 180 daily rows into ~26 weekly rows; a year
	 * collapses 365 → ~52. Each weekly row's hourly array is the element-wise sum of the
	 * 7 daily arrays it contains, so the shape reflects the typical week's hourly density.
	 */
	const granularity = $derived<'day' | 'week'>(
		rows.length >= WEEKLY_GRANULARITY_THRESHOLD ? 'week' : 'day',
	);

	const displayRows = $derived.by(() => {
		if (granularity === 'day') return rows;
		const out: RidgeRow[] = [];
		for (let i = 0; i < rows.length; i += 7) {
			const chunk = rows.slice(i, i + 7);
			if (chunk.length === 0) continue;
			const summed = new Array<number>(24).fill(0);
			for (const r of chunk) {
				for (let h = 0; h < 24; h++) summed[h] += r.hourly[h];
			}
			out.push({ date: chunk[0].date, hourly: summed });
		}
		return out;
	});

	let userSigma = $state(SLIDER_DEFAULT);
	onMount(() => {
		const stored = localStorage.getItem(SLIDER_KEY);
		if (stored) {
			const parsed = Number(stored);
			if (Number.isFinite(parsed) && parsed >= SLIDER_MIN && parsed <= SLIDER_MAX) {
				userSigma = parsed;
			}
		}
	});
	$effect(() => {
		// Only persist when the slider is the live source of sigma (i.e. no
		// external sigma prop). Avoids overwriting the user's value when the
		// preview route briefly mounts the same component with a forced sigma.
		if (sigma === undefined) localStorage.setItem(SLIDER_KEY, String(userSigma));
	});

	const effectiveSigma = $derived(
		sigma
			?? (mode === 'hero' && granularity === 'day' ? userSigma : undefined)
			?? (granularity === 'week' ? RIDGE_SIGMA_WEEKLY : RIDGE_SIGMA_DAILY),
	);

	const KDE_SAMPLES = 144; // 6 samples per hour bin (24 × 6 = 144)
	/**
	 * amp = rowSpacing * ROW_AMP_FACTOR controls how far peaks travel above their row.
	 * Joy Division's cover has peaks reaching ~6–8 row-spacings up — heavy mountain stack.
	 * 4.0 gives that overlap without the topmost peaks colliding with the title block.
	 * Higher = denser overlap; lower = each row reads as a thin strip.
	 */
	const ROW_AMP_FACTOR = 4.0;
	/**
	 * Tighter row packing reads more like Unknown Pleasures (which has ~80 rows
	 * crammed into a square). At 12px per row, 30 rows takes 360 + headroom; 90 rows
	 * takes 1080 + headroom. The chart-height clamp below trims long windows.
	 */
	const ROW_HEIGHT_DEFAULT = 12;

	let chartEl: SVGSVGElement | undefined = $state(undefined);
	let chartWidth = $state(900);
	let hover = $state<{ rowIdx: number; hour: number } | null>(null);

	const ROW_COUNT = $derived(displayRows.length);
	const CHART_HEIGHT = $derived(
		mode === 'hero'
			? Math.max(420, Math.min(640, ROW_COUNT * ROW_HEIGHT_DEFAULT + 120))
			: Math.max(220, ROW_COUNT * 10 + 48),
	);

	/**
	 * Reserve space at the top of the plot frame for the first row's full peak.
	 * `TOP_HEADROOM_FACTOR > ROW_AMP_FACTOR` so even the tallest peak fits below
	 * the timeline axis labels (small clearance margin).
	 */
	const TOP_HEADROOM_FACTOR = ROW_AMP_FACTOR + 0.2;

	/**
	 * Solve for ROW_SPACING such that:
	 *   topHeadroom + (rowCount - 1) * rowSpacing == plotHeight
	 *   topHeadroom = TOP_HEADROOM_FACTOR * rowSpacing
	 * → rowSpacing = plotHeight / (rowCount - 1 + TOP_HEADROOM_FACTOR)
	 */
	const ROW_SPACING = $derived.by(() => {
		const plotHeight = CHART_HEIGHT - 80; // PADDING.top(56) + PADDING.bottom(40) - axis label height
		// Unified formula. ROW_COUNT === 1 falls out as plotHeight / TOP_HEADROOM_FACTOR — the
		// single row sits at the bottom with the peak rising into the upper 80% of the canvas.
		if (ROW_COUNT === 0) return 24;
		return plotHeight / Math.max(1, ROW_COUNT - 1 + TOP_HEADROOM_FACTOR);
	});

	const TOP_HEADROOM = $derived(ROW_SPACING * TOP_HEADROOM_FACTOR);

	/**
	 * AMP controls the peak height of each ridge.
	 *
	 * Dense (12+ rows): amp = rowSpacing * factor → JD-style mountain stacking with
	 * 5–6 row-spacings of overlap. Per-row normalisation means every active row
	 * reaches this height regardless of absolute volume.
	 *
	 * Sparse (3–11 rows): rowSpacing balloons; capping at rowSpacing * 0.85 keeps the
	 * topmost ridge from clipping past the chart top.
	 */
	const AMP = $derived.by(() => {
		const plotHeight = CHART_HEIGHT - 80;
		if (ROW_COUNT >= 12) {
			return ROW_SPACING * ROW_AMP_FACTOR;
		}
		return Math.min(plotHeight * 0.4, ROW_SPACING * 0.85);
	});

	/**
	 * Per-row normalisation — the JD pulsar trick.
	 *
	 * Each row's smoothed density is scaled to its own max so every active row reaches
	 * the same amp ceiling. Days with one big listening burst and days with steady
	 * activity both produce equally tall ridges; the SHAPE differs, the height matches.
	 *
	 * Honest about empty days: rows with zero listens render as a flat baseline (no peak).
	 *
	 * The backend's `ridge_amp_max` (P95 global) is now unused for amplitude scaling but
	 * kept on the response for any future global view.
	 */
	const smoothed = $derived(
		displayRows.map((r) => kde1d(r.hourly, effectiveSigma, KDE_SAMPLES)),
	);
	const rowMaxes = $derived(smoothed.map(rowMax));

	/**
	 * Visual peak hour — argmax of a KDE-smoothed aggregate over every visible
	 * row. Decoupled from the chart's `effectiveSigma` and pinned to SPINE_SIGMA
	 * so the spine stat reflects "where listening clusters" even when the user
	 * dials the chart sigma down to render sharp peaks.
	 *
	 * Granularity-invariant: weekly buckets already sum 7 days per row, so
	 * summing across rows gives the same per-hour totals as daily granularity
	 * would have produced.
	 *
	 * Known limitation: kde1d is a flat-bin Gaussian (not circular), so a true
	 * ~23:00 peak gets dragged ~1 hour earlier. Acceptable — visual ≈ number
	 * is the explicit goal here.
	 */
	const aggregateHourly = $derived.by(() => {
		const out = new Array<number>(24).fill(0);
		for (const r of displayRows) for (let h = 0; h < 24; h++) out[h] += r.hourly[h];
		return out;
	});
	const aggregateTotal = $derived(aggregateHourly.reduce((a, b) => a + b, 0));
	const visualPeakHour = $derived.by<number | null>(() => {
		if (aggregateTotal === 0) return null;
		const density = kde1d(aggregateHourly, SPINE_SIGMA, KDE_SAMPLES);
		let best = 0;
		for (let i = 1; i < density.length; i++) {
			if (density[i] > density[best]) best = i;
		}
		return Math.round((best / (KDE_SAMPLES - 1)) * 23);
	});
	const peakHour = $derived(visualPeakHour ?? heroStats?.peak_hour ?? null);
	const peakHint = 'Hour your listening clusters around (smoothed at a fixed bandwidth, independent of the chart slider).';

	const TIMELINE_TICKS = [0, 6, 12, 18, 24];

	// Layout — hero mode reserves a left column for the spine.
	const SPINE_WIDTH = $derived(mode === 'hero' ? 140 : 0);
	const PADDING = { top: 56, bottom: 40, left: SPINE_WIDTH + 16, right: 24 };
	const PLOT_HEIGHT = $derived(CHART_HEIGHT - PADDING.top - PADDING.bottom);

	// Convert mouse x in plot coords to an hour 0..24 (continuous).
	function pixelXToHour(px: number, plotWidth: number): number {
		return Math.max(0, Math.min(24, (px / plotWidth) * 24));
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
		// Map mouse y to nearest row baseline. Row i's baseline sits at TOP_HEADROOM + i * ROW_SPACING.
		const rowIdx = Math.max(
			0,
			Math.min(ROW_COUNT - 1, Math.round((py - TOP_HEADROOM) / ROW_SPACING)),
		);
		const hourFloat = pixelXToHour(px, plotWidth);
		const hour = Math.max(0, Math.min(23, Math.round(hourFloat)));
		hover = { rowIdx, hour };
	}

	function handlePointerLeave() {
		hover = null;
	}

	// Resize observer keeps chartWidth in sync with the SVG bbox.
	$effect(() => {
		if (!chartEl) return;
		const ro = new ResizeObserver((entries) => {
			for (const entry of entries) {
				chartWidth = Math.round(entry.contentRect.width);
			}
		});
		ro.observe(chartEl);
		return () => ro.disconnect();
	});

	const hoverDate = $derived.by(() => {
		if (!hover) return null;
		const row = displayRows[hover.rowIdx];
		if (!row) return null;
		if (granularity === 'week') {
			// Each weekly row's `date` is the first day of the week.
			return `Week of ${formatDate(row.date, 'day')}`;
		}
		return formatDate(row.date, 'day-tooltip');
	});

	const hoverListens = $derived.by(() => {
		if (!hover) return null;
		return displayRows[hover.rowIdx]?.hourly[hover.hour] ?? 0;
	});
</script>

<div class="ridgeline" data-mode={mode}>
	{#if mode === 'hero'}
		<aside class="spine">
			{#if variant === 'single-day'}
				<div class="stat" title={peakHint}>
					<div class="stat-value">{formatHour(peakHour)}</div>
					<div class="stat-label">Peak hour</div>
				</div>
				<div class="stat">
					<div class="stat-value">{formatDuration(heroStats?.longest_session_ms ?? null)}</div>
					<div class="stat-label">Longest session</div>
				</div>
				<div class="stat">
					<div class="stat-value">{formatCount(rowsTotalListens(rows))}</div>
					<div class="stat-label">Listens</div>
				</div>
				<div class="stat">
					<div class="stat-value">{formatCount(heroStats?.distinct_tracks ?? null)}</div>
					<div class="stat-label">Tracks</div>
				</div>
			{:else}
				<div class="stat" title={peakHint}>
					<div class="stat-value">{formatHour(peakHour)}</div>
					<div class="stat-label">Peak hour</div>
				</div>
				<div class="stat">
					<div class="stat-value">{heroStats?.rhythm ?? '--'}</div>
					<div class="stat-label">Rhythm</div>
				</div>
				<div class="stat">
					<div class="stat-value">{formatPercent(heroStats?.night_share ?? null, { decimals: 0 })}</div>
					<div class="stat-label">Night</div>
				</div>
				<div class="stat">
					<div class="stat-value">{formatPercent(heroStats?.morning_share ?? null, { decimals: 0 })}</div>
					<div class="stat-label">Morning</div>
				</div>
			{/if}
		</aside>
	{/if}

	<div class="chart-wrap">
		{#if mode === 'hero'}
			<div class="title-row" aria-hidden="true">
				<span class="title">Listening pulse</span>
			</div>
		{/if}

		<svg
			bind:this={chartEl}
			class="chart"
			viewBox="0 0 {chartWidth} {CHART_HEIGHT}"
			preserveAspectRatio="none"
			onpointermove={handlePointerMove}
			onpointerleave={handlePointerLeave}
			role="img"
			aria-label="Listening density across hours of the day, one ridge per day"
		>
			<!-- Timeline axis at the top -->
			<g class="axis" aria-hidden="true">
				{#each TIMELINE_TICKS as tick}
					{@const x = PADDING.left + (tick / 24) * (chartWidth - PADDING.left - PADDING.right)}
					<text {x} y={PADDING.top - 16} text-anchor="middle">{formatHour(tick === 24 ? 0 : tick).replace('00:00', tick === 24 ? '24:00' : '00:00')}</text>
					<line {x} x1={x} x2={x} y1={PADDING.top - 8} y2={PADDING.top + PLOT_HEIGHT} class="axis-tick" />
				{/each}
			</g>

			<!-- Ridges, top-first so later rows occlude earlier ones (the JD trick). -->
			<g class="ridges" transform="translate({PADDING.left} {PADDING.top})">
				{#each smoothed as density, i (displayRows[i]?.date ?? i)}
					{@const baseY = TOP_HEADROOM + i * ROW_SPACING}
					{@const isEmpty = rowMaxes[i] === 0}
					<path
						class:empty={isEmpty}
						d={ridgePath(density, baseY, AMP, chartWidth - PADDING.left - PADDING.right, rowMaxes[i])}
					/>
				{/each}

				<!-- Hover guide -->
				{#if hover}
					{@const plotWidth = chartWidth - PADDING.left - PADDING.right}
					{@const guideX = (hover.hour / 24) * plotWidth}
					<line class="hover-guide" x1={guideX} x2={guideX} y1={0} y2={PLOT_HEIGHT} />
				{/if}
			</g>
		</svg>

		{#if hover && hoverDate}
			<div class="tooltip" role="status" aria-live="polite">
				{hoverDate} · {formatHour(hover.hour)} · {formatCount(hoverListens)} {hoverListens === 1 ? 'listen' : 'listens'}
			</div>
		{/if}

		{#if mode === 'hero'}
			<p class="caption">
				Your listening clusters around <em>{formatHour(peakHour)}</em>.
			</p>
			{#if sigma === undefined && granularity === 'day'}
				<div class="sigma-row">
					<label class="sigma-control" title="KDE bandwidth for the chart — wider smooths spikes into clusters, narrower preserves sharp single-hour peaks. The spine stat uses a fixed bandwidth and is unaffected.">
						<span class="sigma-label">Smoothing</span>
						<input
							type="range"
							min={SLIDER_MIN}
							max={SLIDER_MAX}
							step={SLIDER_STEP}
							bind:value={userSigma}
							aria-label="Chart smoothing bandwidth"
						/>
						<span class="sigma-value">{userSigma.toFixed(2)}</span>
					</label>
				</div>
			{/if}
		{/if}
	</div>
</div>

<!-- SR-only summary for assistive tech -->
<table class="sr-only" aria-label="Listening pulse summary">
	<caption>
		Listening clusters around {formatHour(peakHour)};
		Rhythm {heroStats?.rhythm ?? '--'};
		Night {formatPercent(heroStats?.night_share ?? null, { decimals: 0 })};
		Morning {formatPercent(heroStats?.morning_share ?? null, { decimals: 0 })}.
	</caption>
	<thead>
		<tr>
			<th scope="col">Date</th>
			{#each Array.from({ length: 24 }, (_, h) => h) as h (h)}
				<th scope="col">{formatHour(h)}</th>
			{/each}
		</tr>
	</thead>
	<tbody>
		{#each displayRows as row (row.date)}
			<tr>
				<th scope="row">
					{granularity === 'week' ? `Wk of ${formatDate(row.date, 'day')}` : formatDate(row.date, 'day-tooltip')}
				</th>
				{#each row.hourly as listens, hourIdx (hourIdx)}
					<td>{listens}</td>
				{/each}
			</tr>
		{/each}
	</tbody>
</table>

<script module lang="ts">
	function rowsTotalListens(rows: RidgeRow[]): number {
		let n = 0;
		for (const r of rows) for (const h of r.hourly) n += h;
		return n;
	}
</script>

<style>
	.ridgeline {
		display: grid;
		/* Cap the chart at 1080px so it stays the right proportions on wide displays.
		   Without this, 4K viewports stretch the chart horizontally — chartWidth grows
		   but CHART_HEIGHT stays fixed, so peaks read as smaller relative to the wider
		   canvas. With the cap, the spine sits on the left and the chart on the right,
		   total max ~1220px, centred within the card on big displays. */
		grid-template-columns: auto minmax(0, 1080px);
		gap: 0;
		width: 100%;
		max-width: 1220px;
		margin: 0 auto;
		align-items: stretch;
	}

	.ridgeline[data-mode='solo'] {
		grid-template-columns: minmax(0, 1080px);
	}

	.spine {
		display: flex;
		flex-direction: column;
		justify-content: space-between;
		padding: var(--space-5) var(--space-5) var(--space-5) var(--space-4);
		min-width: 132px;
		align-self: stretch;
	}

	.stat {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.stat-value {
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-medium);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
		line-height: var(--line-height-tight);
		letter-spacing: -0.01em;
	}

	.stat-label {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.chart-wrap {
		position: relative;
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.title-row {
		position: absolute;
		top: var(--space-3);
		left: var(--space-4);
		z-index: 2;
		pointer-events: none;
	}

	.title {
		font-family: var(--font-display);
		font-style: italic;
		font-size: var(--font-size-lg);
		color: var(--text-primary);
	}

	.sigma-row {
		display: flex;
		justify-content: center;
		margin-top: var(--space-2);
	}

	.sigma-control {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		color: var(--text-tertiary);
		letter-spacing: 0.08em;
	}

	.sigma-label {
		text-transform: uppercase;
	}

	.sigma-control input[type='range'] {
		/* Slider width scales modestly with viewport — wide enough for usable
		   precision on a 720 px-floor window, narrow enough not to dominate
		   the chart's footer rhythm at 4K. */
		width: clamp(112px, 10vw, 160px);
		accent-color: var(--text-secondary);
		cursor: pointer;
	}

	.sigma-value {
		color: var(--text-secondary);
		min-width: 2.5em;
		text-align: right;
	}

	.chart {
		width: 100%;
		height: auto;
		display: block;
	}

	.axis text {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		letter-spacing: 0.08em;
		fill: var(--text-tertiary);
	}

	.axis-tick {
		stroke: var(--border-subtle);
		stroke-width: 1;
		stroke-dasharray: 1 3;
	}

	.ridges path {
		fill: var(--bg-base);
		stroke: var(--text-primary);
		stroke-width: 1.2;
		stroke-linejoin: round;
		stroke-linecap: round;
		vector-effect: non-scaling-stroke;
		transition: stroke var(--motion-fast);
	}

	/* Ghost rows — days with zero listens. They hold their slot in the timeline
	   so the chart's vertical extent reflects the full window, but recede so the
	   active days carry the visual weight. */
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

	.tooltip {
		position: absolute;
		top: 8px;
		right: 16px;
		padding: 6px 10px;
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-xs);
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		color: var(--text-secondary);
		pointer-events: none;
		white-space: nowrap;
	}

	.caption {
		margin: var(--space-4) 0 var(--space-2) 0;
		text-align: center;
		font-family: var(--font-display);
		font-style: italic;
		font-size: var(--font-size-md);
		color: var(--text-secondary);
	}

	.caption em {
		color: var(--text-primary);
		font-style: italic;
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

	/* Mobile: spine collapses under the ridges as a 2x2 grid. */
	@media (max-width: 720px) {
		.ridgeline {
			grid-template-columns: 1fr;
		}

		.spine {
			grid-row: 2;
			display: grid;
			grid-template-columns: 1fr 1fr;
			gap: var(--space-4);
			padding-top: var(--space-4);
		}
	}

	/* Reduced motion — kill every transition and animation in this tree. */
	@media (prefers-reduced-motion: reduce) {
		.ridgeline :where(*, *::before, *::after) {
			transition: none !important;
			animation: none !important;
		}
	}
</style>
