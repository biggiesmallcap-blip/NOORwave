<script lang="ts">
	/**
	 * KpiStrip — borderless 4-cell row.
	 *
	 * Cells: Listened · Sessions · Completion · Skip rate.
	 * Each cell is its own grid cell; 1px vertical dividers separate them.
	 * Below 1100px collapses to 2x2; below 720px to 1x4 stacked.
	 *
	 * Per the locked spec the strip is borderless — sits between two glass cards
	 * (hero above, tempo below) and inherits the page background.
	 */

	import KpiCell from './KpiCell.svelte';
	import type { SignalsKpis } from '$lib/api/client';
	import { formatCount, formatDuration, formatPercent } from '$lib/utils/format';

	interface Props {
		kpis: SignalsKpis;
	}

	let { kpis }: Props = $props();

	// Daily series for each cell — same backing data, different field per metric.
	// Listened / Sessions: zero days are real troughs, keep them.
	// Completion / Skip: zero-listen days have no ratio (NaN territory). Filter them
	// out so the silhouette reflects active-day variation only — and so the two
	// series are honest visual inverses (skip = 1 - completion).
	const listenedMsSeries = $derived(kpis.daily.map((d) => d.listened_ms));
	const listensSeries = $derived(kpis.daily.map((d) => d.listens));
	const activeDays = $derived(kpis.daily.filter((d) => d.listens > 0));
	const completionSeries = $derived(activeDays.map((d) => d.completed / d.listens));
	const skipSeries = $derived(activeDays.map((d) => 1 - d.completed / d.listens));
</script>

<div class="strip" role="group" aria-label="Listening summary">
	<KpiCell
		label="Listened"
		value={formatDuration(kpis.listened_ms.current)}
		current={kpis.listened_ms.current}
		previous={kpis.listened_ms.previous}
		series={listenedMsSeries}
	/>
	<KpiCell
		label="Sessions"
		value={formatCount(kpis.sessions.current)}
		current={kpis.sessions.current}
		previous={kpis.sessions.previous}
		series={listensSeries}
	/>
	<KpiCell
		label="Completion"
		value={formatPercent(kpis.completion.current, { decimals: 0 })}
		current={kpis.completion.current}
		previous={kpis.completion.previous}
		series={completionSeries}
	/>
	<KpiCell
		label="Skip rate"
		value={formatPercent(kpis.skip_rate.current, { decimals: 0 })}
		current={kpis.skip_rate.current}
		previous={kpis.skip_rate.previous}
		series={skipSeries}
	/>
</div>

<style>
	/* Apply the .glass surface to the whole strip; internal 1px dividers between
	   cells via grid `gap` painted with a thin border colour. The :global() override
	   makes each cell's background transparent so the parent glass shows through. */
	.strip {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		width: 100%;
		max-width: 1220px;
		margin: 0 auto;
		gap: 1px;
		background: var(--border-subtle);
		border-radius: var(--radius);
		overflow: hidden;
		/* glass treatment baked in so the strip feels native to the rest of the page */
		backdrop-filter: blur(16px);
		-webkit-backdrop-filter: blur(16px);
		border: 1px solid color-mix(in srgb, var(--panel-border) 70%, var(--instrument-border));
		box-shadow:
			inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 52%, transparent),
			var(--panel-shadow);
	}

	.strip > :global(*) {
		background: linear-gradient(
				180deg,
				color-mix(in srgb, var(--instrument-surface) 74%, transparent),
				color-mix(in srgb, var(--instrument-surface-strong) 88%, transparent)
			),
			var(--panel-bg);
	}

	@media (max-width: 1100px) {
		.strip {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}

	@media (max-width: 720px) {
		.strip {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
