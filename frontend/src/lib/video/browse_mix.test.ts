import { describe, expect, test } from 'vitest';
import type { TidalSearchVideo, VideoDiscoverSet } from '$lib/api/client';
import { buildBrowseMix } from './browse_mix';

function video(tidal_id: number, artist_name: string): TidalSearchVideo {
	return {
		tidal_id,
		artist_name,
		title: `Video ${tidal_id}`,
		duration_ms: null,
		artist_id: null,
		album_tidal_id: null,
		artwork_url: null,
		quality: null,
		explicit: null,
		type: 'Music Video'
	};
}

function shelf(slug: string, items: TidalSearchVideo[]): VideoDiscoverSet {
	return { slug, bucket_key: 'today', title: slug, blurb: '', items };
}

describe('browse mix', () => {
	test('brings linked artists forward and avoids duplicate clips and adjacent artists', () => {
		const mix = buildBrowseMix([
			shelf('daily-picks', [video(1, 'Elvis'), video(2, 'Elvis'), video(3, 'Willie')]),
			shelf('genre:pop', [video(1, 'Elvis'), video(4, 'Elvis'), video(5, 'Bob')]),
			shelf('one-step-out', [video(6, 'New Artist'), video(7, 'Another Artist')])
		]);
		expect(mix.slice(0, 3).map((item) => item.tidal_id)).toEqual([1, 3, 6]);
		expect(new Set(mix.map((item) => item.tidal_id)).size).toBe(mix.length);
		expect(mix.every((item, index) => index === 0 || item.artist_name !== mix[index - 1].artist_name)).toBe(true);
		expect(mix.filter((item) => item.artist_name === 'Elvis')).toHaveLength(2);
	});

	test('places unfamiliar linked artists through a longer familiar queue', () => {
		const familiar = Array.from({ length: 12 }, (_, index) => video(index + 1, `Known ${index + 1}`));
		const linked = Array.from({ length: 6 }, (_, index) => video(index + 101, `New ${index + 1}`));
		const mix = buildBrowseMix([shelf('daily-picks', familiar), shelf('one-step-out', linked)], 18);
		expect(mix).toHaveLength(18);
		for (let index = 0; index < mix.length; index += 1) {
			expect(mix[index].tidal_id >= 100).toBe(index % 3 === 2);
		}
	});
});
