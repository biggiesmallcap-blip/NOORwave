// Rules behind the Daily chart shelf: whether the provider matrix has data or
// needs a refresh, which entries need a TIDAL match, and how an entry's metric
// line reads. Kept out of the component so they can be tested by behaviour.

import type { ChartMatrixResponse } from '$lib/api/client';

export type ChartItemKind = {
	entity_type: string;
	tidal_id: number | null;
	local_track_id?: number | null;
};

/** Today as the YYYY-MM-DD the server stamps chart dates with. */
export function todayUtc(now: Date = new Date()): string {
	return now.toISOString().slice(0, 10);
}

/** The newest chart date across every cell of the matrix. */
export function latestChartDate(matrix: ChartMatrixResponse): string | null {
	let latest: string | null = null;
	for (const row of matrix.rows) {
		for (const cell of Object.values(row.cells)) {
			if (cell && (!latest || cell.chart_date > latest)) latest = cell.chart_date;
		}
	}
	return latest;
}

export function matrixHasData(matrix: ChartMatrixResponse | null): boolean {
	return Boolean(
		matrix?.rows.some((row) =>
			matrix.providers.some((provider) => Boolean(row.cells[provider.source_key])),
		),
	);
}

export function regionHasMatrixData(matrix: ChartMatrixResponse | null, region: string): boolean {
	const row = matrix?.rows.find((item) => item.region === region);
	return Boolean(row && matrix?.providers.some((provider) => Boolean(row.cells[provider.source_key])));
}

/** An empty matrix, or one whose newest chart is older than today, is refreshed. */
export function matrixNeedsRefresh(matrix: ChartMatrixResponse, today: string = todayUtc()): boolean {
	if (!matrixHasData(matrix)) return true;
	const latest = latestChartDate(matrix);
	return latest !== null && latest < today;
}

export function isVideo(item: ChartItemKind): boolean {
	return item.entity_type === 'video';
}

/** Entries with no TIDAL id and no library row must be matched by search before playing. */
export function needsMatch(item: ChartItemKind): boolean {
	return !item.tidal_id && !item.local_track_id && !isVideo(item);
}

export function audienceLabel(item: {
	streams: number | null;
	views: number | null;
	points: number | null;
}): string {
	if (item.streams != null) return `${item.streams.toLocaleString()} streams`;
	if (item.views != null) return `${item.views.toLocaleString()} views`;
	if (item.points != null) return `${item.points.toLocaleString()} pts`;
	return '';
}

/** A negative delta means the entry climbed (rank number went down). */
export function movementLabel(delta: number | null): string {
	if (delta == null) return '';
	if (delta === 0) return 'Steady';
	if (delta < 0) return `Up ${Math.abs(delta)}`;
	return `Down ${delta}`;
}
