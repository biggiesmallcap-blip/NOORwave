/**
 * Shared 1D KDE + closed-shape ridge path generation.
 *
 * Used by `ListenRidgeline.svelte` (24 hourly bins) and `TempoRidges.svelte`
 * (36 BPM buckets). Sigma is in BIN-WIDTH UNITS, so the same numerical value
 * works for both — the caller knows what "one bin" represents in its domain.
 *
 * The closed-shape path is the Joy-Division occlusion trick: each ridge is
 * filled with the page background colour and stroked with text-primary, so
 * later-drawn rows occlude earlier ones. Render top-first, bottom-last.
 */

/**
 * Gaussian kernel-density estimate over a discrete series.
 *
 *   kde1d([0,3,3,0,0,3,...], 0.6, 144)
 *
 * Returns `samples` smoothed densities along the closed interval [0, n-1] in
 * bin coordinates. `sigma` is in bin-widths.
 */
export function kde1d(values: number[], sigma: number, samples: number): number[] {
	const out = new Array<number>(samples);
	const n = values.length;
	const denom = 2 * sigma * sigma;
	for (let i = 0; i < samples; i++) {
		const x = (i / (samples - 1)) * (n - 1);
		let sum = 0;
		for (let b = 0; b < n; b++) {
			const d = x - b;
			sum += values[b] * Math.exp(-(d * d) / denom);
		}
		out[i] = sum;
	}
	return out;
}

/**
 * Closed-shape ridge path. The fill goes from baseY back to baseY along the
 * bottom edge so the shape can be filled with the background colour for the
 * occlusion trick. Empty rows (rowMax === 0) render as a flat baseline.
 */
export function ridgePath(
	density: number[],
	baseY: number,
	amp: number,
	width: number,
	rowMax: number,
): string {
	if (rowMax === 0) {
		return `M0,${baseY} L${width},${baseY} Z`;
	}
	const n = density.length;
	let d = `M0,${baseY}`;
	for (let i = 0; i < n; i++) {
		const x = (i / (n - 1)) * width;
		const y = baseY - amp * (density[i] / rowMax);
		d += ` L${x.toFixed(1)},${y.toFixed(1)}`;
	}
	d += ` L${width},${baseY} Z`;
	return d;
}

/**
 * Per-row maximum of an array of smoothed densities. Returns 0 when every
 * value is 0 (empty row); callers use that to render a flat baseline.
 */
export function rowMax(density: number[]): number {
	let m = 0;
	for (const v of density) if (v > m) m = v;
	return m;
}
