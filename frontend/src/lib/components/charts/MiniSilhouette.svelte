<script lang="ts">
	/**
	 * MiniSilhouette — KDE-smoothed silhouette for KPI cells.
	 *
	 * Per-cell normalisation (see plan): each cell shows a different metric
	 * (ms vs count vs ratio), so silhouettes are NOT comparable in magnitude — only
	 * in shape. Peaks reach 95% of cell height; baseline at 0.
	 *
	 * Gaussian-kernel smoothing in bin-widths (sigma prop). 0.6 is the default —
	 * angular enough that day-to-day variation reads, smooth enough that hour noise
	 * doesn't make the chart look like a seismograph.
	 */

	import { kde1d } from './ridge-kde';

	interface Props {
		values: number[];
		/** viewBox width — actual rendered width comes from CSS (100%). */
		width?: number;
		/** viewBox height — actual rendered height in px lives in CSS. */
		height?: number;
		/** KDE bandwidth in bin-widths. 0 disables smoothing (raw straight lines). */
		sigma?: number;
		ariaLabel?: string;
	}

	let {
		values,
		width = 240,
		height = 40,
		sigma = 0.35,
		ariaLabel = 'metric trend',
	}: Props = $props();

	const PAD = 1.2; // breathing room so the stroke doesn't clip at edges
	const SAMPLES = 96; // smoothed-curve sample count

	const smoothed = $derived(
		sigma <= 0 || values.length < 3 ? values : kde1d(values, sigma, SAMPLES),
	);
	const max = $derived(Math.max(0, ...smoothed));
	const isFlat = $derived(max === 0 || smoothed.length < 2);

	function pointFor(i: number, val: number, total: number): [number, number] {
		const x = total === 1 ? width / 2 : (i / (total - 1)) * width;
		const yNorm = isFlat ? 0 : val / max;
		const peakBudget = height - PAD * 2;
		const y = height - PAD - yNorm * peakBudget * 0.95;
		return [x, y];
	}

	const path = $derived.by(() => {
		if (smoothed.length === 0) {
			return `M0,${height - PAD} L${width},${height - PAD} Z`;
		}
		if (smoothed.length === 1) {
			const [, y] = pointFor(0, smoothed[0], 1);
			return `M0,${height - PAD} L0,${y} L${width},${y} L${width},${height - PAD} Z`;
		}
		const pts = smoothed.map((v, i) => pointFor(i, v, smoothed.length));
		let d = `M${pts[0][0]},${height - PAD} L${pts[0][0]},${pts[0][1].toFixed(1)}`;
		for (let i = 1; i < pts.length; i++) {
			d += ` L${pts[i][0].toFixed(1)},${pts[i][1].toFixed(1)}`;
		}
		d += ` L${pts[pts.length - 1][0]},${height - PAD} Z`;
		return d;
	});
</script>

<svg
	class="mini-silhouette"
	viewBox="0 0 {width} {height}"
	preserveAspectRatio="none"
	aria-label={ariaLabel}
	role="img"
>
	<path d={path} class:flat={isFlat} />
</svg>

<style>
	.mini-silhouette {
		display: block;
		width: 100%;
		height: 40px;
		overflow: visible;
	}

	.mini-silhouette path {
		fill: var(--text-primary);
		fill-opacity: 0.34;
		stroke: var(--text-primary);
		stroke-width: 1.1;
		stroke-linejoin: miter; /* sharp corners — angular peaks */
		stroke-miterlimit: 6;
		stroke-linecap: butt;
		vector-effect: non-scaling-stroke;
	}

	.mini-silhouette path.flat {
		fill: transparent;
		stroke-opacity: 0.4;
	}
</style>
