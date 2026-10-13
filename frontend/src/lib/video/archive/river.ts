// The era lens geometry: the decade facet as columns you click or drag across
// to pick a range of decades. Geometry only; EraBars.svelte owns the DOM.

export interface DecadeCount {
	decade: number;
	count: number;
}

/** Every decade from the first to the last one with songs, gaps filled with
 *  zero so the river reads as time, not as a list of buckets. A selected
 *  range outside the data still gets its decades, so it stays visible. */
export function decadeDomain(
	facet: readonly DecadeCount[],
	selected: { from: number | null; to: number | null } = { from: null, to: null }
): DecadeCount[] {
	const counts = new Map<number, number>();
	for (const { decade, count } of facet) {
		if (!Number.isFinite(decade) || count <= 0) continue;
		const key = Math.floor(decade / 10) * 10;
		counts.set(key, (counts.get(key) ?? 0) + count);
	}
	const keys = [...counts.keys()];
	if (selected.from != null) keys.push(Math.floor(selected.from / 10) * 10);
	if (selected.to != null) keys.push(Math.floor(selected.to / 10) * 10);
	if (keys.length === 0) return [];
	const first = Math.min(...keys);
	const last = Math.max(...keys);
	const out: DecadeCount[] = [];
	for (let decade = first; decade <= last; decade += 10) out.push({ decade, count: counts.get(decade) ?? 0 });
	return out;
}

/** Centre x of each decade's column in a river `width` wide. */
export function columnCenters(count: number, width: number): number[] {
	if (count <= 0) return [];
	const step = width / count;
	return Array.from({ length: count }, (_, i) => step * (i + 0.5));
}

/** The decade under x (clamped to the ends). */
export function decadeAt(domain: readonly DecadeCount[], width: number, x: number): number | null {
	if (domain.length === 0 || width <= 0) return null;
	const index = Math.min(domain.length - 1, Math.max(0, Math.floor((x / width) * domain.length)));
	return domain[index].decade;
}

/** A closed, smoothed river for the counts: symmetric about the middle line,
 *  wide where a decade has many songs. Square-root scaled so one huge decade
 *  (the 2010s) does not thin every other into a thread, with a hairline
 *  floor so empty decades still show where the river runs. */
export function riverPath(domain: readonly DecadeCount[], width: number, height: number): string {
	if (domain.length === 0 || width <= 0 || height <= 0) return '';
	const max = Math.max(...domain.map((d) => d.count), 1);
	const middle = height / 2;
	const floor = 0.75;
	const half = (count: number) => floor + (Math.sqrt(count) / Math.sqrt(max)) * (middle - floor - 1);
	const xs = columnCenters(domain.length, width);
	const points: [number, number][] = [[0, half(domain[0].count)]];
	domain.forEach((d, i) => points.push([xs[i], half(d.count)]));
	points.push([width, half(domain[domain.length - 1].count)]);

	const curve = (from: [number, number], to: [number, number], sign: 1 | -1) => {
		const mid = (from[0] + to[0]) / 2;
		const y0 = middle + sign * from[1];
		const y1 = middle + sign * to[1];
		return ` C${fmt(mid)} ${fmt(y0)} ${fmt(mid)} ${fmt(y1)} ${fmt(to[0])} ${fmt(y1)}`;
	};
	let path = `M0 ${fmt(middle - points[0][1])}`;
	for (let i = 1; i < points.length; i += 1) path += curve(points[i - 1], points[i], -1);
	path += ` L${fmt(width)} ${fmt(middle + points[points.length - 1][1])}`;
	for (let i = points.length - 1; i > 0; i -= 1) path += curve(points[i], points[i - 1], 1);
	return `${path} Z`;
}

function fmt(value: number): string {
	return String(Math.round(value * 10) / 10);
}

/** Two decades (either order) as an inclusive year range. */
export function decadeRange(a: number, b: number): { from: number; to: number } {
	const lo = Math.min(a, b);
	const hi = Math.max(a, b);
	return { from: lo, to: hi + 9 };
}

/** The selected span as fractions of the river's width, for the highlight. */
export function selectionSpan(
	domain: readonly DecadeCount[],
	from: number | null,
	to: number | null
): { start: number; end: number } | null {
	if (domain.length === 0 || (from == null && to == null)) return null;
	const first = domain[0].decade;
	const n = domain.length;
	const lo = from == null ? first : Math.floor(from / 10) * 10;
	const hi = to == null ? domain[n - 1].decade : Math.floor(to / 10) * 10;
	const start = Math.max(0, (lo - first) / 10) / n;
	const end = Math.min(n, (hi - first) / 10 + 1) / n;
	return end > start ? { start, end } : null;
}

/** Short decade label for the axis: 1980 -> "80s", 2000 -> "00s", but a
 *  century boundary or the ends keep four digits for orientation. */
export function decadeTick(decade: number, index: number, total: number): string {
	if (index === 0 || index === total - 1 || decade % 100 === 0) return `${decade}s`;
	return `${String(decade % 100).padStart(2, '0')}s`;
}
