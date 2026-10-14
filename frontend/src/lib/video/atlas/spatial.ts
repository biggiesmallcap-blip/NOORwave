// A uniform grid over world space for hit testing 15k-60k points without a
// scan: points are bucketed once (counting sort into flat Int32Arrays), and
// a query only visits the cells its radius touches. No allocation per query.

export class SpatialGrid {
	readonly cellSize: number;
	readonly cols: number;
	readonly rows: number;
	readonly originX: number;
	readonly originY: number;
	/** cellStart[c]..cellStart[c + 1] indexes `items` for cell c. */
	private readonly cellStart: Int32Array;
	private readonly items: Int32Array;
	private readonly xs: ArrayLike<number>;
	private readonly ys: ArrayLike<number>;

	constructor(xs: ArrayLike<number>, ys: ArrayLike<number>, count: number, cellSize = 8) {
		this.xs = xs;
		this.ys = ys;
		this.cellSize = cellSize;
		let minX = Infinity;
		let minY = Infinity;
		let maxX = -Infinity;
		let maxY = -Infinity;
		for (let i = 0; i < count; i++) {
			if (xs[i] < minX) minX = xs[i];
			if (ys[i] < minY) minY = ys[i];
			if (xs[i] > maxX) maxX = xs[i];
			if (ys[i] > maxY) maxY = ys[i];
		}
		if (!Number.isFinite(minX)) {
			minX = 0;
			minY = 0;
			maxX = 0;
			maxY = 0;
		}
		this.originX = minX;
		this.originY = minY;
		this.cols = Math.max(1, Math.floor((maxX - minX) / cellSize) + 1);
		this.rows = Math.max(1, Math.floor((maxY - minY) / cellSize) + 1);
		const cells = this.cols * this.rows;
		const counts = new Int32Array(cells + 1);
		const cellOf = new Int32Array(count);
		for (let i = 0; i < count; i++) {
			const c = this.cellIndex(xs[i], ys[i]);
			cellOf[i] = c;
			counts[c + 1] += 1;
		}
		for (let c = 0; c < cells; c++) counts[c + 1] += counts[c];
		this.cellStart = counts;
		const cursor = counts.slice(0, cells);
		this.items = new Int32Array(count);
		for (let i = 0; i < count; i++) {
			this.items[cursor[cellOf[i]]++] = i;
		}
	}

	private cellIndex(x: number, y: number): number {
		const cx = Math.min(this.cols - 1, Math.max(0, Math.floor((x - this.originX) / this.cellSize)));
		const cy = Math.min(this.rows - 1, Math.max(0, Math.floor((y - this.originY) / this.cellSize)));
		return cy * this.cols + cx;
	}

	/**
	 * The point closest to (wx, wy) whose distance is at most
	 * `radius + extra(i)` (its own drawn radius in world units), or -1. Ties go
	 * to the larger point (it is drawn on top). `extra` may be null.
	 */
	nearest(wx: number, wy: number, radius: number, extra: ((index: number) => number) | null = null, maxExtra = 0): number {
		const reach = radius + maxExtra;
		const x0 = Math.max(0, Math.floor((wx - reach - this.originX) / this.cellSize));
		const x1 = Math.min(this.cols - 1, Math.floor((wx + reach - this.originX) / this.cellSize));
		const y0 = Math.max(0, Math.floor((wy - reach - this.originY) / this.cellSize));
		const y1 = Math.min(this.rows - 1, Math.floor((wy + reach - this.originY) / this.cellSize));
		let best = -1;
		let bestScore = Infinity;
		for (let cy = y0; cy <= y1; cy++) {
			for (let cx = x0; cx <= x1; cx++) {
				const c = cy * this.cols + cx;
				for (let k = this.cellStart[c]; k < this.cellStart[c + 1]; k++) {
					const i = this.items[k];
					const dx = this.xs[i] - wx;
					const dy = this.ys[i] - wy;
					const d = Math.sqrt(dx * dx + dy * dy);
					const own = extra ? extra(i) : 0;
					if (d > radius + own) continue;
					// Distance from the point's edge, so a big point wins near its rim.
					const score = d - own;
					if (score < bestScore) {
						bestScore = score;
						best = i;
					}
				}
			}
		}
		return best;
	}

	/** Calls `visit` for every point inside the rectangle (world units). */
	forEachIn(minX: number, minY: number, maxX: number, maxY: number, visit: (index: number) => void): void {
		const x0 = Math.max(0, Math.floor((minX - this.originX) / this.cellSize));
		const x1 = Math.min(this.cols - 1, Math.floor((maxX - this.originX) / this.cellSize));
		const y0 = Math.max(0, Math.floor((minY - this.originY) / this.cellSize));
		const y1 = Math.min(this.rows - 1, Math.floor((maxY - this.originY) / this.cellSize));
		for (let cy = y0; cy <= y1; cy++) {
			for (let cx = x0; cx <= x1; cx++) {
				const c = cy * this.cols + cx;
				for (let k = this.cellStart[c]; k < this.cellStart[c + 1]; k++) {
					const i = this.items[k];
					const x = this.xs[i];
					const y = this.ys[i];
					if (x >= minX && x <= maxX && y >= minY && y <= maxY) visit(i);
				}
			}
		}
	}
}

/** The scene whose disk contains (wx, wy), nearest centre first; -1 if none. */
export function sceneAt(
	scenes: readonly { x: number; y: number; r: number }[],
	wx: number,
	wy: number,
	grow = 1
): number {
	let best = -1;
	let bestRatio = Infinity;
	for (let i = 0; i < scenes.length; i++) {
		const s = scenes[i];
		const dx = s.x - wx;
		const dy = s.y - wy;
		const ratio = Math.sqrt(dx * dx + dy * dy) / Math.max(1e-6, s.r * grow);
		if (ratio <= 1 && ratio < bestRatio) {
			bestRatio = ratio;
			best = i;
		}
	}
	return best;
}
