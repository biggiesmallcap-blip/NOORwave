import { describe, expect, test } from 'vitest';
import type { AtlasScene, VideoCensus } from '$lib/api/video_explore_types';
import { censusFigures, coverageFigures, cutShares, sceneCoverage, seenLabel, steerAge } from './logbook';

const census: VideoCensus = {
	building: false,
	built_at: '2026-10-14T00:00:00Z',
	videos: 162349,
	hours: 11556.4,
	programmable: 150590,
	artists: 15214,
	charted_artists: 10034,
	uncharted_artists: 5180,
	scenes: 83,
	channel_scenes: 40,
	songs: 130734,
	multi_cut_songs: 20335,
	concerts: 513,
	cuts: { official: 60000, live: 22000, session: 4000, lyric: 9000, visualizer: 3000, remix: 1000, dance: 500, interview: 800, trailer: 300, clip: 900, longform: 0 },
	genre_coverage: 0.86,
	era_coverage: 0.624,
	watched_videos: 1204,
	lit_artists: 312,
	stages: [],
	scene_rows: [],
};

function scene(id: number, videos: number, seen: number): AtlasScene {
	return { id, name: `Scene ${id}`, exemplars: [], tags: [], hue: id * 10, x: 0, y: 0, r: 1, videos, artists: 1, hours: 1, channel_no: null, lit_artists: 0, seen_videos: seen };
}

describe('logbook census', () => {
	test('reads as plain numbers', () => {
		const figures = censusFigures(census);
		expect(figures.map((f) => [f.label, f.value])).toEqual([
			['Videos', '162,349'],
			['Hours', '11,556'],
			['Songs', '130,734'],
			['Scenes', '83'],
			['Concerts', '513'],
			['Artists', '15,214'],
		]);
		expect(figures[2].note).toBe('20,335 with more than one cut');
	});

	test('coverage as percentages', () => {
		expect(coverageFigures(census).map((f) => f.value)).toEqual(['62%', '86%', '66%']);
	});

	test('cuts biggest first, empty cuts left out', () => {
		const shares = cutShares(census);
		expect(shares[0]).toMatchObject({ cut: 'official', share: 1 });
		expect(shares.map((s) => s.cut)).not.toContain('longform');
	});
});

describe('scene coverage', () => {
	test('visited scenes first by share, then the rest by size; seen is capped', () => {
		const rows = sceneCoverage([scene(1, 100, 0), scene(2, 50, 10), scene(3, 1000, 20), scene(4, 300, 0), scene(5, 10, 99)]);
		expect(rows.map((r) => r.id)).toEqual([5, 2, 3, 4, 1]);
		expect(rows[0]).toMatchObject({ seen: 10, videos: 10, ratio: 1 });
		expect(seenLabel(rows[2])).toBe('seen 20 of 1,000');
	});
});

describe('channel steering', () => {
	test('a press reads as how long ago it was', () => {
		const now = Date.UTC(2026, 9, 14, 12);
		const at = (daysAgo: number) => (now - daysAgo * 86_400_000) / 1000;
		expect(steerAge(at(0), now)).toBe('today');
		expect(steerAge(at(1), now)).toBe('yesterday');
		expect(steerAge(at(5), now)).toBe('5 days ago');
		expect(steerAge(at(20), now)).toBe('2 weeks ago');
		// A clock that runs behind never reads as the future.
		expect(steerAge(at(-1), now)).toBe('today');
	});
});
