<script lang="ts">
	/**
	 * KpiCell — borderless cell for the KPI strip.
	 *
	 * Layout (per locked spec):
	 *   [LABEL (mono caps)        DELTA (mono, sign-tinted)]
	 *   [VALUE (Newsreader large)                          ]
	 *   [MiniSilhouette                                    ]
	 *
	 * Per-cell normalisation lives inside MiniSilhouette — each cell shows its own
	 * metric (ms vs count vs ratio), so silhouettes are not comparable across cells
	 * in magnitude, only in shape.
	 */

	import MiniSilhouette from '$lib/components/charts/MiniSilhouette.svelte';
	import { formatDelta } from '$lib/utils/format';

	interface Props {
		label: string;
		/** Pre-formatted value string — caller has already applied formatDuration / formatPercent / etc. */
		value: string;
		/** Raw current/previous numbers — used to compute the delta locally. */
		current?: number | null;
		previous?: number | null;
		/** Daily-series numbers feeding the silhouette underneath. */
		series?: number[];
		polarity?: 'positive' | 'inverse' | 'neutral';
		ariaLabel?: string;
	}

	let {
		label,
		value,
		current = null,
		previous = null,
		series = [],
		polarity = 'positive',
		ariaLabel,
	}: Props = $props();

	const delta = $derived(formatDelta(current, previous));
	const deltaTone = $derived.by(() => {
		if (delta.sign === 0 || polarity === 'neutral') return 'neutral';
		const oriented = polarity === 'inverse' ? -delta.sign : delta.sign;
		return oriented > 0 ? 'good' : 'bad';
	});
</script>

<div class="cell" aria-label={ariaLabel ?? label}>
	<span class="label">{label}</span>
	<div class="value-row">
		<span class="value">{value}</span>
		<span class="delta" data-tone={deltaTone}>{delta.text}</span>
	</div>
	<div class="silhouette">
		<MiniSilhouette values={series} ariaLabel={`${label} trend`} />
	</div>
</div>

<style>
	.cell {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding: var(--space-3) var(--space-4) 0;
		min-width: 0;
	}

	.label {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.value-row {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		min-width: 0;
	}

	.value {
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-medium);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
		line-height: var(--line-height-tight);
		letter-spacing: -0.01em;
		flex: 0 0 auto;
	}

	.delta {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		letter-spacing: 0.04em;
		color: var(--text-tertiary);
	}

	.delta[data-tone='good'] {
		color: var(--state-success);
	}

	.delta[data-tone='bad'] {
		color: var(--state-error);
	}

	.silhouette {
		margin-top: auto;
		min-height: 40px;
	}
</style>
