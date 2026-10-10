import { describe, expect, test } from 'vitest';
import type { ChartMatrixCell, ChartMatrixResponse } from '$lib/api/client';
import {
	audienceLabel,
	latestChartDate,
	matrixHasData,
	matrixNeedsRefresh,
	movementLabel,
	needsMatch,
	regionHasMatrixData,
	todayUtc,
} from './chart_rules';

const cell = (chart_date: string) => ({ chart_date }) as ChartMatrixCell;

function matrix(cells: Record<string, Record<string, string | null>>): ChartMatrixResponse {
	return {
		region_group: 'core',
		period: 'daily',
		providers: [
			{ source_key: 'spotify', label: 'Spotify' },
			{ source_key: 'apple', label: 'Apple Music' },
		],
		rows: Object.entries(cells).map(([region, byProvider]) => ({
			region,
			cells: Object.fromEntries(
				Object.entries(byProvider).map(([k, d]) => [k, d ? cell(d) : null]),
			),
		})),
	};
}

describe('matrix freshness', () => {
	test('an empty matrix needs a refresh', () => {
		expect(matrixHasData(matrix({ GLOBAL: { spotify: null } }))).toBe(false);
		expect(matrixNeedsRefresh(matrix({ GLOBAL: { spotify: null } }), '2026-10-11')).toBe(true);
	});
	test('a matrix behind today needs a refresh; one from today does not', () => {
		const m = matrix({ GLOBAL: { spotify: '2026-10-10', apple: '2026-10-11' }, US: { spotify: '2026-10-09' } });
		expect(latestChartDate(m)).toBe('2026-10-11');
		expect(matrixNeedsRefresh(m, '2026-10-11')).toBe(false);
		expect(matrixNeedsRefresh(m, '2026-10-12')).toBe(true);
	});
	test('region data is per row', () => {
		const m = matrix({ GLOBAL: { spotify: '2026-10-11' }, US: { spotify: null } });
		expect(regionHasMatrixData(m, 'GLOBAL')).toBe(true);
		expect(regionHasMatrixData(m, 'US')).toBe(false);
		expect(regionHasMatrixData(m, 'JP')).toBe(false);
	});
	test('todayUtc uses the UTC date', () => {
		expect(todayUtc(new Date('2026-10-11T23:30:00Z'))).toBe('2026-10-11');
	});
});

describe('entry rules', () => {
	test('only unresolved non-video entries need a TIDAL match', () => {
		expect(needsMatch({ entity_type: 'track', tidal_id: null, local_track_id: null })).toBe(true);
		expect(needsMatch({ entity_type: 'track', tidal_id: 5 })).toBe(false);
		expect(needsMatch({ entity_type: 'track', tidal_id: null, local_track_id: 7 })).toBe(false);
		expect(needsMatch({ entity_type: 'video', tidal_id: null })).toBe(false);
	});
	test('audience prefers streams, then views, then points', () => {
		expect(audienceLabel({ streams: 1200, views: 5, points: 1 })).toBe(`${(1200).toLocaleString()} streams`);
		expect(audienceLabel({ streams: null, views: 9, points: 1 })).toBe('9 views');
		expect(audienceLabel({ streams: null, views: null, points: null })).toBe('');
	});
	test('a negative rank delta reads as climbing', () => {
		expect(movementLabel(-3)).toBe('Up 3');
		expect(movementLabel(2)).toBe('Down 2');
		expect(movementLabel(0)).toBe('Steady');
		expect(movementLabel(null)).toBe('');
	});
});
