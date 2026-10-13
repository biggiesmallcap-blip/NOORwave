import { describe, expect, test } from 'vitest';
import type { ExploreVideo, VideoCut, VideoVersion } from '$lib/api/video_explore_types';
import { concertLength, concertQueue, cutChipLabel, distinctCuts, orderVersions, runtimeLabel, versionCaption, versionsQueue } from './versions';
import { decadeAt, decadeDomain, decadeRange, decadeTick, riverPath, selectionSpan } from './river';

function video(id: number, durationS = 240): ExploreVideo {
	return {
		tidal_id: id,
		title: `Swear It Again ${id}`,
		duration_ms: durationS * 1000,
		artist_id: 5,
		artist_name: 'Westlife',
		album_tidal_id: null,
		artwork_url: null,
		quality: null,
		explicit: null,
		type: 'video',
	};
}

function version(id: number, cut: VideoCut, year: number | null, best = false, durationS = 240): VideoVersion {
	return { video: video(id, durationS), cut, cut_year: year, label: `${cut} ${year}`, concert: null, best };
}

describe('version ordering', () => {
	test('best first, then cut, then oldest year, then longer, then id', () => {
		const ordered = orderVersions([
			version(1, 'live', 2016),
			version(2, 'lyric', 2010),
			version(3, 'live', 2001),
			version(4, 'official', 2015, true),
			version(5, 'live', null),
			version(6, 'official', 1999),
			version(7, 'live', 2001, false, 300),
		]);
		expect(ordered.map((v) => v.video.tidal_id)).toEqual([4, 6, 7, 3, 1, 5, 2]);
	});

	test('does not mutate the server list', () => {
		const input = [version(2, 'live', 2001), version(1, 'official', 2000, true)];
		orderVersions(input);
		expect(input.map((v) => v.video.tidal_id)).toEqual([2, 1]);
	});

	test('captions add the cut and year only when the label lacks them', () => {
		expect(versionCaption({ label: 'Official video', cut: 'official', cut_year: 1999 }, 'Official')).toBe('Official video, 1999');
		expect(versionCaption({ label: 'Live at Croke Park 2006', cut: 'live', cut_year: 2006 }, 'Live')).toBe('Live at Croke Park 2006');
		expect(versionCaption({ label: 'The Farewell Tour', cut: 'live', cut_year: 2012 }, 'Live')).toBe('The Farewell Tour, Live, 2012');
		expect(versionCaption({ label: '', cut: 'lyric', cut_year: null }, 'Lyric')).toBe('Lyric');
	});

	test('the cuts line names one of each other kind, never the kind on', () => {
		const others = [version(1, 'official', 2000), version(2, 'live', 2001), version(3, 'live', 2006), version(4, 'lyric', null)];
		expect(distinctCuts(others, 'official').map((v) => v.video.tidal_id)).toEqual([2, 4]);
		expect(distinctCuts(others, null).map((v) => v.video.tidal_id)).toEqual([1, 2, 4]);
		expect(cutChipLabel({ cut_year: 2006 }, 'Live')).toBe('Live, 2006');
		expect(cutChipLabel({ cut_year: null }, 'Lyric')).toBe('Lyric');
	});

	test('a picked version leads its queue, the rest keep list order', () => {
		const list = [version(1, 'official', 2000, true), version(2, 'live', 2001), version(3, 'live', 2002)];
		expect(versionsQueue(list, 2).map((v) => v.tidal_id)).toEqual([2, 1, 3]);
	});
});

describe('concert play order', () => {
	test('plays whole and in setlist order from the top', () => {
		const { queue, index } = concertQueue({ videos: [video(30), video(10), video(20)] });
		expect(queue.map((v) => v.tidal_id)).toEqual([30, 10, 20]);
		expect(index).toBe(0);
	});

	test('starting from a song keeps the whole setlist queued', () => {
		const { queue, index } = concertQueue({ videos: [video(30), video(10), video(20)] }, 10);
		expect(queue.map((v) => v.tidal_id)).toEqual([30, 10, 20]);
		expect(index).toBe(1);
	});

	test('a repeated video plays once; an unknown start starts at the top', () => {
		const { queue, index } = concertQueue({ videos: [video(1), video(2), video(1)] }, 99);
		expect(queue.map((v) => v.tidal_id)).toEqual([1, 2]);
		expect(index).toBe(0);
	});

	test('lengths read naturally', () => {
		expect(runtimeLabel(247_000)).toBe('4:07');
		expect(runtimeLabel(4_324_000)).toBe('1:12:04');
		expect(runtimeLabel(null)).toBe('');
		expect(concertLength(48 * 60)).toBe('48 min');
		expect(concertLength(112 * 60)).toBe('1 hr 52 min');
	});
});

describe('era river', () => {
	const facet = [
		{ decade: 1990, count: 400 },
		{ decade: 1970, count: 100 },
		{ decade: 2010, count: 1600 },
	];

	test('fills the gaps between the first and last decade', () => {
		expect(decadeDomain(facet).map((d) => [d.decade, d.count])).toEqual([
			[1970, 100], [1980, 0], [1990, 400], [2000, 0], [2010, 1600],
		]);
	});

	test('a selection outside the data stays on the river', () => {
		expect(decadeDomain(facet, { from: 1950, to: 1959 })[0].decade).toBe(1950);
	});

	test('maps x to a decade and decades to a year range', () => {
		const domain = decadeDomain(facet);
		expect(decadeAt(domain, 500, 0)).toBe(1970);
		expect(decadeAt(domain, 500, 250)).toBe(1990);
		expect(decadeAt(domain, 500, 9999)).toBe(2010);
		expect(decadeRange(2000, 1980)).toEqual({ from: 1980, to: 2009 });
	});

	test('the highlight spans the selected decades', () => {
		const domain = decadeDomain(facet);
		expect(selectionSpan(domain, 1980, 1999)).toEqual({ start: 0.2, end: 0.6 });
		expect(selectionSpan(domain, null, null)).toBeNull();
	});

	test('draws a closed path and short ticks', () => {
		const path = riverPath(decadeDomain(facet), 500, 60);
		expect(path.startsWith('M0 ')).toBe(true);
		expect(path).toContain(' L500 ');
		expect(path.endsWith('Z')).toBe(true);
		expect(riverPath([], 500, 60)).toBe('');
		expect(decadeTick(1980, 1, 5)).toBe('80s');
		expect(decadeTick(2000, 3, 5)).toBe('2000s');
		expect(decadeTick(1970, 0, 5)).toBe('1970s');
	});
});
