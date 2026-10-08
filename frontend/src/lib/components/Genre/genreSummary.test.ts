import { describe, expect, test } from 'vitest';
import type { Genre } from '$lib/api/client';
import { buildGenreSummary, cohortPhrase, type GenreSnapshot } from './genreSummary';
import { ROOT_FAMILY_COLORS } from './galaxy.types';

const g = (id: number, name: string, slug: string, parentId: number | null, trackCount: number, children: Genre[] = []): Genre => ({
	id,
	name,
	slug,
	parent_id: parentId,
	track_count: trackCount,
	children
});

const snapshot: GenreSnapshot = {
	genres: [
		g(1, 'Electronic', 'electronic', null, 30, [
			g(2, 'House', 'house', 1, 20, [g(3, 'Deep House', 'deep-house', 2, 12), g(4, 'Empty', 'empty', 2, 0)])
		])
	],
	heat: [{ genre_id: 2, genre_name: 'House', listen_count: 9, total_listened_ms: 600000 }],
	cohorts: [{ id: 'afternoon_drift', label: 'Afternoon Drift', icon: 'x', genre_ids: [2], listen_count: 9, total_listened_ms: 600000 }],
	evolution: [
		{ genre_id: 2, genre_name: 'House', period_start: '2026-09-07', listen_count: 4, total_listened_ms: 1 },
		{ genre_id: 2, genre_name: 'House', period_start: '2026-08-31', listen_count: 5, total_listened_ms: 1 }
	],
	metrics: [{ genre_id: 2, genre_name: 'House', avg_bpm: 124, avg_energy: 0.7, avg_danceability: 0.8, analyzed_count: 5 }]
};

describe('buildGenreSummary', () => {
	test('finds a nested genre with its lineage, family and stats', () => {
		const summary = buildGenreSummary(snapshot, 2)!;
		expect(summary.name).toBe('House');
		expect(summary.familyName).toBe('Electronic');
		expect(summary.lineage).toEqual([{ id: 1, name: 'Electronic' }]);
		expect(summary.trackCount).toBe(20);
		expect(summary.listenCount).toBe(9);
		expect(summary.avgBpm).toBe(124);
		expect(summary.evolutionHistory.map((point) => point.periodStart)).toEqual(['2026-08-31', '2026-09-07']);
	});

	test('lists only sub-genres that have tracks', () => {
		expect(buildGenreSummary(snapshot, 2)!.children).toEqual([{ id: 3, name: 'Deep House', trackCount: 12 }]);
	});

	test('roots keep the family colour', () => {
		expect(buildGenreSummary(snapshot, 1)!.color).toBe(ROOT_FAMILY_COLORS.electronic.color);
	});

	test('cohort reads as plain words', () => {
		expect(buildGenreSummary(snapshot, 2)!.cohort?.label).toBe('Mostly weekday afternoons');
		expect(cohortPhrase('other')).toBeNull();
	});

	test('unknown id gives null', () => {
		expect(buildGenreSummary(snapshot, 999)).toBeNull();
	});
});
