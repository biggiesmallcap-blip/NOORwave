<script lang="ts" module>
	/**
	 * BPM tier → opacity table. Five bands, monochrome — the chart is intentionally
	 * single-hue so opacity carries the tempo signal without colour-coding genres.
	 *
	 * Exported so the legend, the SR-only summary, and any future consumer all read
	 * from one source. Adding/removing a tier here updates every consumer atomically.
	 */
	export const BPM_OPACITY_TIERS = [
		{ label: '< 90', max: 90, opacity: 0.18 },
		{ label: '90–110', max: 110, opacity: 0.3 },
		{ label: '110–130', max: 130, opacity: 0.42 },
		{ label: '130–150', max: 150, opacity: 0.55 },
		{ label: '> 150', max: Infinity, opacity: 0.7 },
	] as const;

	export function bpmOpacity(bpm: number): number {
		for (const tier of BPM_OPACITY_TIERS) {
			if (bpm < tier.max) return tier.opacity;
		}
		return BPM_OPACITY_TIERS[BPM_OPACITY_TIERS.length - 1].opacity;
	}

	export const SIZE_LEGEND = [
		{ label: 'low', t: 0.05 },
		{ label: 'medium', t: 0.5 },
		{ label: 'high', t: 1 },
	] as const;

	const DOT_MIN = 2.5;
	const DOT_MAX = 14;

	/**
	 * Dot radius scaled relative to the dataset's max listens — sqrt-area scaling so
	 * a track with 4× listens is 2× the area, not 2× the radius. Ensures a sparse DB
	 * (e.g. max=3 plays) still produces visually distinct dots; without this the absolute
	 * scaling collapsed every dot to the 2px floor.
	 */
	export function dotRadius(listens: number, datasetMax: number): number {
		if (datasetMax <= 0) return DOT_MIN;
		const t = Math.sqrt(Math.max(0, listens)) / Math.sqrt(datasetMax);
		return DOT_MIN + Math.min(1, t) * (DOT_MAX - DOT_MIN);
	}

	/** Same scale function for the legend dots — given a t in [0,1] return the radius. */
	export function legendRadius(t: number): number {
		return DOT_MIN + Math.max(0, Math.min(1, t)) * (DOT_MAX - DOT_MIN);
	}
</script>

<script lang="ts">
	/**
	 * Sonic Field — every track plotted by what it does to a room.
	 *
	 *   x = Energy (low → high)
	 *   y = Danceability (low at bottom → high at top)
	 *   size = listens (sqrt-scaled for a friendly visual range)
	 *   opacity = BPM tier (5 bands, see BPM_OPACITY_TIERS module export)
	 *
	 * Quadrant labels (Newsreader italic, very faint):
	 *   top-left   = contemplative   (high D, low E)
	 *   top-right  = euphoric        (high D, high E)
	 *   bottom-left  = melancholy    (low D, low E)
	 *   bottom-right = aggressive    (low D, high E)
	 *
	 * Hover: dot gains a 1.5px ring at full opacity; all other dots dim to 30% of
	 * their tier opacity. Click → playTrackNow. Right-click → buildTrackMenu.
	 *
	 * Spec: C:\Users\Felix\.claude\plans\lets-revision-analytics-stats-crystalline-melody.md
	 */

	import type { SonicView, SonicTrack } from '$lib/api/client';
	import { formatBpm, formatCount } from '$lib/utils/format';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu, type MenuTrack } from '$lib/player/track_menu';
	import { playTrackNow } from '$lib/stores/player';

	interface Props {
		field: SonicView;
		/**
		 * Override hook — used by the dev preview route to log instead of triggering
		 * playback / menus. In production the component wires playTrackNow + buildTrackMenu
		 * directly per the universal-context-menus rule.
		 */
		onplay?: (trackId: number) => void;
		oncontext?: (event: MouseEvent, trackId: number) => void;
	}

	let { field, onplay, oncontext }: Props = $props();

	function toMenuTrack(t: SonicTrack): MenuTrack {
		return {
			id: t.track_id,
			title: t.title,
			artist_name: t.artist_name,
			album_title: t.album,
		};
	}

	const VIEWBOX_W = 1000;
	const VIEWBOX_H = 560;
	const PADDING = { top: 24, bottom: 48, left: 56, right: 36 };
	const PLOT_W = VIEWBOX_W - PADDING.left - PADDING.right;
	const PLOT_H = VIEWBOX_H - PADDING.top - PADDING.bottom;
	const ZOOM_MIN_W = 80; // ~12.5x max zoom
	const ZOOM_FACTOR = 1.18;
	const DRAG_THRESHOLD = 4; // px

	/** Unique clip-path id — prevents collisions if multiple instances exist. */
	const clipId = `sonic-clip-${Math.floor(Math.random() * 1e9)}`;

	let chartEl: SVGSVGElement | undefined = $state(undefined);
	let hoveredId: number | null = $state(null);

	/** Current visible window over the data-space (SVG coords). Full extent = no zoom. */
	let view = $state({ x: 0, y: 0, w: VIEWBOX_W, h: VIEWBOX_H });
	const isZoomed = $derived(view.w < VIEWBOX_W - 0.5 || view.h < VIEWBOX_H - 0.5);

	/** Transform applied to the data layer so axis/legend stay static. */
	const dataTransform = $derived(
		`scale(${VIEWBOX_W / view.w},${VIEWBOX_H / view.h}) translate(${-view.x},${-view.y})`,
	);

	// ── Drag panning ──────────────────────────────────────────────────────────
	let dragStart: { x: number; y: number; view: typeof view } | null = $state(null);
	let dragDist = 0; // total px moved in the current drag — NOT reactive (checked synchronously)

	function handleMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		if (!chartEl) return;
		dragStart = { x: e.clientX, y: e.clientY, view: { ...view } };
		dragDist = 0;
	}

	function handleMouseMove(e: MouseEvent) {
		if (!dragStart || !chartEl) return;
		const totalDist = Math.hypot(e.clientX - dragStart.x, e.clientY - dragStart.y);
		dragDist = totalDist;
		if (totalDist < DRAG_THRESHOLD) return;
		const rect = chartEl.getBoundingClientRect();
		const dx = ((e.clientX - dragStart.x) / rect.width) * dragStart.view.w;
		const dy = ((e.clientY - dragStart.y) / rect.height) * dragStart.view.h;
		const newX = Math.max(0, Math.min(VIEWBOX_W - dragStart.view.w, dragStart.view.x - dx));
		const newY = Math.max(0, Math.min(VIEWBOX_H - dragStart.view.h, dragStart.view.y - dy));
		view = { ...view, x: newX, y: newY };
	}

	function handleMouseUp() {
		dragStart = null;
	}

	function handleWheel(e: WheelEvent) {
		if (!chartEl) return;
		e.preventDefault();
		const rect = chartEl.getBoundingClientRect();
		const mxNorm = (e.clientX - rect.left) / rect.width; // 0..1 along chart width
		const myNorm = (e.clientY - rect.top) / rect.height;
		const factor = e.deltaY > 0 ? ZOOM_FACTOR : 1 / ZOOM_FACTOR;

		// Anchor: data point under the cursor stays under the cursor.
		const cursorDataX = view.x + mxNorm * view.w;
		const cursorDataY = view.y + myNorm * view.h;
		const newW = Math.max(ZOOM_MIN_W, Math.min(VIEWBOX_W, view.w * factor));
		const newH = newW * (VIEWBOX_H / VIEWBOX_W); // preserve aspect ratio
		const newX = Math.max(0, Math.min(VIEWBOX_W - newW, cursorDataX - mxNorm * newW));
		const newY = Math.max(0, Math.min(VIEWBOX_H - newH, cursorDataY - myNorm * newH));
		view = { x: newX, y: newY, w: newW, h: newH };
	}

	function resetZoom() {
		view = { x: 0, y: 0, w: VIEWBOX_W, h: VIEWBOX_H };
	}

	const tracks = $derived(field.tracks);

	const datasetMax = $derived(tracks.reduce((m, t) => (t.listens > m ? t.listens : m), 0));

	function cx(e: number): number {
		return PADDING.left + e * PLOT_W;
	}
	function cy(d: number): number {
		return PADDING.top + (1 - d) * PLOT_H;
	}

	function handleClick(track: SonicTrack) {
		if (dragDist > DRAG_THRESHOLD) return; // suppress click when drag occurred
		if (onplay) {
			onplay(track.track_id);
		} else {
			void playTrackNow(track.track_id);
		}
	}

	function handleContext(event: MouseEvent, track: SonicTrack) {
		event.preventDefault();
		if (oncontext) {
			oncontext(event, track.track_id);
			return;
		}
		const items = buildTrackMenu(toMenuTrack(track));
		openContextMenu(event, items, track.title);
	}

	const hoveredTrack = $derived.by(() =>
		hoveredId === null ? null : tracks.find((t) => t.track_id === hoveredId) ?? null,
	);
</script>

<div class="sonic">
	<header class="header">
		<span class="eyebrow">Sonic field</span>
		<h2><span class="title">Energy &times; Danceability</span> <span class="counter">{formatCount(field.total)} tracks</span></h2>
	</header>

	<div class="chart-grid">
		<svg
			bind:this={chartEl}
			class="chart"
			class:zoomed={isZoomed}
			class:dragging={dragStart !== null && dragDist >= DRAG_THRESHOLD}
			viewBox="0 0 {VIEWBOX_W} {VIEWBOX_H}"
			preserveAspectRatio="xMidYMid meet"
			onwheel={handleWheel}
			onmousedown={handleMouseDown}
			onmousemove={handleMouseMove}
			onmouseup={handleMouseUp}
			onmouseleave={handleMouseUp}
			role="img"
			aria-label="Energy by danceability scatter, {field.total} tracks"
		>
			<defs>
				<!-- Clips the data layer to the padded plot area so dots don't bleed into axes. -->
				<clipPath id={clipId}>
					<rect x={PADDING.left} y={PADDING.top} width={PLOT_W} height={PLOT_H} />
				</clipPath>
			</defs>

			<!-- Static axis labels and names — outside the data transform so they don't pan. -->
			<g class="axes" aria-hidden="true">
				<!-- X axis -->
				<text x={PADDING.left} y={VIEWBOX_H - 18} text-anchor="start">0</text>
				<text x={PADDING.left + PLOT_W / 2} y={VIEWBOX_H - 18} text-anchor="middle">0.5</text>
				<text x={PADDING.left + PLOT_W} y={VIEWBOX_H - 18} text-anchor="end">1.0</text>
				<text x={PADDING.left + PLOT_W / 2} y={VIEWBOX_H - 4} text-anchor="middle" class="axis-name">ENERGY</text>
				<!-- Y axis -->
				<text x={PADDING.left - 8} y={PADDING.top + 4} text-anchor="end">1.0</text>
				<text x={PADDING.left - 8} y={PADDING.top + PLOT_H / 2 + 3} text-anchor="end">0.5</text>
				<text x={PADDING.left - 8} y={PADDING.top + PLOT_H} text-anchor="end">0</text>
				<text
					x={12}
					y={PADDING.top + PLOT_H / 2}
					text-anchor="middle"
					class="axis-name"
					transform="rotate(-90 12 {PADDING.top + PLOT_H / 2})"
				>
					DANCEABILITY
				</text>
			</g>

			<!-- Data layer: clipped to plot area, then scaled/translated for zoom+pan.
			     The outer <g> provides the clip; the inner <g> provides the transform.
			     This keeps the clip-path in SVG-root coordinates (not affected by transform). -->
			<g clip-path="url(#{clipId})">
				<g class="data-layer" transform={dataTransform}>
					<!-- Crosshair at 0.5 / 0.5 — follows data space. -->
					<g class="crosshair" aria-hidden="true">
						<line x1={cx(0.5)} x2={cx(0.5)} y1={PADDING.top} y2={PADDING.top + PLOT_H} />
						<line y1={cy(0.5)} y2={cy(0.5)} x1={PADDING.left} x2={PADDING.left + PLOT_W} />
					</g>

					<!-- Quadrant labels — follow data space, fade when zoomed into a single quadrant. -->
					<g class="quadrants" aria-hidden="true">
						<text x={cx(0.04)} y={cy(0.96)} text-anchor="start">contemplative</text>
						<text x={cx(0.96)} y={cy(0.96)} text-anchor="end">euphoric</text>
						<text x={cx(0.04)} y={cy(0.04)} text-anchor="start" dominant-baseline="hanging">melancholy</text>
						<text x={cx(0.96)} y={cy(0.04)} text-anchor="end" dominant-baseline="hanging">aggressive</text>
					</g>

					<!-- Dots — largest first so smaller ones aren't fully occluded. -->
					<g class="dots" class:has-hover={hoveredId !== null}>
						{#each [...tracks].sort((a, b) => b.listens - a.listens) as track (track.track_id)}
							{@const isHover = hoveredId === track.track_id}
							<circle
								cx={cx(track.e)}
								cy={cy(track.d)}
								r={dotRadius(track.listens, datasetMax)}
								fill="var(--text-primary)"
								fill-opacity={bpmOpacity(track.bpm)}
								class:hover={isHover}
								onmouseenter={() => (hoveredId = track.track_id)}
								onmouseleave={() => (hoveredId = null)}
								onclick={() => handleClick(track)}
								oncontextmenu={(e) => handleContext(e, track)}
								role="button"
								tabindex="-1"
								aria-label={`${track.title} by ${track.artist_name ?? 'Unknown artist'}, ${track.listens} ${track.listens === 1 ? 'play' : 'plays'}`}
							/>
						{/each}
					</g>
				</g>
			</g>
		</svg>

		<!-- Right legend stack -->
		<aside class="legend">
			<div class="legend-block">
				<div class="legend-header">BPM tier (opacity)</div>
				{#each BPM_OPACITY_TIERS as tier (tier.label)}
					<div class="legend-row">
						<span class="legend-dot" style="opacity: {tier.opacity}"></span>
						<span class="legend-label">{tier.label}</span>
					</div>
				{/each}
			</div>

			<div class="legend-block">
				<div class="legend-header">Listens (size)</div>
				{#each SIZE_LEGEND as item (item.label)}
					<div class="legend-row">
						<span class="legend-dot size-marker" style="--size: {legendRadius(item.t) * 2}px"></span>
						<span class="legend-label">{item.label}</span>
					</div>
				{/each}
			</div>
		</aside>
	</div>

	{#if isZoomed}
		<button type="button" class="reset-zoom" onclick={resetZoom} aria-label="Reset zoom">
			Reset
		</button>
	{/if}

	{#if hoveredTrack}
		<div class="tooltip" role="status" aria-live="polite">
			<strong>{hoveredTrack.title}</strong>
			<span class="dim">·</span>
			<span>{hoveredTrack.artist_name ?? 'Unknown artist'}</span>
			<span class="dim">·</span>
			<span class="mono">{formatBpm(hoveredTrack.bpm, { decimals: 0, suffix: 'BPM' })}</span>
			<span class="dim">·</span>
			<span class="mono">{formatCount(hoveredTrack.listens)} {hoveredTrack.listens === 1 ? 'play' : 'plays'}</span>
		</div>
	{/if}
</div>

<!-- SR-only summary: top 20 by listens -->
<table class="sr-only" aria-label="Sonic field summary, top 20 tracks by listens">
	<thead>
		<tr>
			<th scope="col">Title</th><th scope="col">Artist</th>
			<th scope="col">Energy</th><th scope="col">Danceability</th>
			<th scope="col">BPM</th><th scope="col">Listens</th>
		</tr>
	</thead>
	<tbody>
		{#each [...tracks].sort((a, b) => b.listens - a.listens).slice(0, 20) as t (t.track_id)}
			<tr>
				<td>{t.title}</td>
				<td>{t.artist_name ?? 'Unknown'}</td>
				<td>{t.e.toFixed(2)}</td>
				<td>{t.d.toFixed(2)}</td>
				<td>{Math.round(t.bpm)}</td>
				<td>{t.listens}</td>
			</tr>
		{/each}
	</tbody>
</table>

<style>
	.sonic {
		width: 100%;
		max-width: 1280px;
		margin: 0 auto;
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		position: relative;
	}

	.header {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.eyebrow {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.header h2 {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		margin: 0;
		font-weight: var(--font-weight-medium);
		font-size: var(--font-size-xl);
	}

	.title {
		font-family: var(--font-display);
		font-style: normal;
		font-weight: var(--font-weight-medium);
		color: var(--text-primary);
	}

	.counter {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		letter-spacing: 0.04em;
	}

	.chart-grid {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 180px;
		gap: var(--space-4);
		align-items: stretch;
	}

	.chart {
		width: 100%;
		height: auto;
		display: block;
	}

	.chart.zoomed {
		cursor: grab;
	}

	.chart.zoomed.dragging {
		cursor: grabbing;
	}

	.reset-zoom {
		position: absolute;
		top: 8px;
		right: 200px; /* clear the legend on desktop */
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.1em;
		padding: 5px 12px;
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		border-radius: var(--radius-xs);
		cursor: pointer;
		z-index: 1;
	}

	.reset-zoom:hover {
		color: var(--text-primary);
		border-color: var(--border-strong);
	}

	.crosshair line {
		stroke: var(--text-primary);
		stroke-opacity: 0.08;
		stroke-width: 1;
		stroke-dasharray: 2 4;
		vector-effect: non-scaling-stroke;
	}

	.quadrants text {
		font-family: var(--font-display);
		font-style: italic;
		font-size: var(--font-size-xl);
		fill: var(--text-primary);
		fill-opacity: 0.14;
		pointer-events: none;
	}

	.axes text {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		letter-spacing: 0.04em;
		fill: var(--text-tertiary);
	}

	.axes text.axis-name {
		font-size: var(--font-size-2xs);
		letter-spacing: 0.18em;
		fill: var(--text-muted);
	}

	.dots circle {
		cursor: pointer;
		stroke: var(--text-primary);
		stroke-opacity: 0.28;
		stroke-width: 0.5;
		vector-effect: non-scaling-stroke;
		transition: fill-opacity var(--motion-fast), stroke-opacity var(--motion-fast);
	}

	/* Hover state: highlight the hovered dot, dim everyone else. */
	.dots.has-hover circle {
		fill-opacity: 0.18 !important;
		stroke-opacity: 0.18;
	}

	.dots.has-hover circle.hover {
		fill-opacity: 1 !important;
		stroke-opacity: 1;
		stroke-width: 1.5;
	}

	.legend {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-3) 0 var(--space-3) var(--space-4);
		border-left: 1px solid var(--border-subtle);
	}

	.legend-block {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.legend-header {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
		margin-bottom: 4px;
	}

	.legend-row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	.legend-dot {
		display: inline-block;
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--text-primary);
		flex: 0 0 auto;
	}

	.legend-dot.size-marker {
		width: var(--size);
		height: var(--size);
		opacity: 0.6;
	}

	.tooltip {
		position: absolute;
		bottom: 8px;
		left: 0;
		right: 0;
		display: flex;
		justify-content: center;
		align-items: center;
		gap: var(--space-2);
		padding: 8px 12px;
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-xs);
		font-family: var(--font-body);
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		pointer-events: none;
		max-width: max-content;
		margin: 0 auto;
	}

	.tooltip strong {
		color: var(--text-primary);
		font-weight: var(--font-weight-medium);
	}

	.tooltip .dim {
		color: var(--text-tertiary);
	}

	.tooltip .mono {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
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
		.chart-grid {
			grid-template-columns: 1fr;
		}
		.legend {
			flex-direction: row;
			border-left: none;
			border-top: 1px solid var(--border-subtle);
			padding: var(--space-3) 0 0 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.sonic :where(*, *::before, *::after) {
			transition: none !important;
			animation: none !important;
		}
	}
</style>
