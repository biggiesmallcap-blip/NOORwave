import { describe, expect, test } from 'vitest';
import { creditFor, cutLabel, LOWER_THIRD_IN_S, LOWER_THIRD_OUTRO_S, lowerThirdPhase, splitTitle } from './lower_third';

describe('lower third timing', () => {
	test('in for the first six seconds of a video, then gone', () => {
		expect(LOWER_THIRD_IN_S).toBe(6);
		expect(lowerThirdPhase(0, 240)).toBe('intro');
		expect(lowerThirdPhase(5.9, 240)).toBe('intro');
		expect(lowerThirdPhase(6, 240)).toBe('hidden');
		expect(lowerThirdPhase(120, 240)).toBe('hidden');
	});

	test('back for the last ten seconds with what is up next', () => {
		expect(LOWER_THIRD_OUTRO_S).toBe(10);
		expect(lowerThirdPhase(229.9, 240)).toBe('hidden');
		expect(lowerThirdPhase(230, 240)).toBe('outro');
		expect(lowerThirdPhase(239.5, 240)).toBe('outro');
		expect(lowerThirdPhase(240, 240)).toBe('hidden');
	});

	test('a tune-in mid-video greets you where you landed', () => {
		expect(lowerThirdPhase(95, 240, 95)).toBe('intro');
		expect(lowerThirdPhase(100.9, 240, 95)).toBe('intro');
		expect(lowerThirdPhase(101, 240, 95)).toBe('hidden');
		// Not at 0:00 of that video, which we never saw.
		expect(lowerThirdPhase(2, 240, 95)).toBe('hidden');
	});

	test('a short video shows its credit first, then up next', () => {
		expect(lowerThirdPhase(3, 14)).toBe('intro');
		expect(lowerThirdPhase(7, 14)).toBe('outro');
	});

	test('no runtime: intro only; junk positions show nothing', () => {
		expect(lowerThirdPhase(3, 0)).toBe('intro');
		expect(lowerThirdPhase(300, 0)).toBe('hidden');
		expect(lowerThirdPhase(Number.NaN, 240)).toBe('hidden');
	});
});

describe('credits', () => {
	test('artist, song, tag and mono metadata', () => {
		const credit = creditFor({
			title: 'Swear It Again (Live at Croke Park)',
			artist_name: 'Westlife',
			duration_ms: 252_000,
			song_year: 1999,
			cut: 'live',
			why: 'Live at Croke Park, 2008',
		});
		expect(credit).toEqual({
			artist: 'Westlife',
			song: 'Swear It Again',
			tag: 'Live at Croke Park',
			year: 1999,
			cut: 'Live',
			runtime: '4:12',
			why: 'Live at Croke Park, 2008',
		});
	});

	test('falls back gracefully without the server extras', () => {
		const credit = creditFor({ title: 'Dynamite', artist_name: null, duration_ms: null, release_year: 3021 }, '  ');
		expect(credit.artist).toBe('Unknown artist');
		expect(credit.year).toBeNull();
		expect(credit.cut).toBeNull();
		expect(credit.runtime).toBe('');
		expect(credit.why).toBeNull();
	});

	test('title markers name the cut in several languages when the server does not', () => {
		expect(cutLabel({ title: 'Corazon (En Vivo)' })).toBe('Live');
		expect(cutLabel({ title: 'Garota (Ao Vivo)' })).toBe('Live');
		expect(cutLabel({ title: 'Song (Acoustic Session)' })).toBe('Session');
		expect(cutLabel({ title: 'Song (Lyric Video)' })).toBe('Lyric video');
		expect(cutLabel({ title: 'Song (Clip officiel)' })).toBe('Official video');
		expect(cutLabel({ title: 'Plain', type: 'Live' })).toBe('Live');
		expect(cutLabel({ title: 'Plain', cut: 'visualizer' })).toBe('Visualizer');
		expect(cutLabel({ title: 'Plain' })).toBeNull();
	});

	test('only a trailing bracket is a tag', () => {
		expect(splitTitle('Viva la Vida (Official Video)')).toEqual({ song: 'Viva la Vida', tag: 'Official Video' });
		expect(splitTitle('Wake Up [Live]')).toEqual({ song: 'Wake Up', tag: 'Live' });
		expect(splitTitle('(Untitled)')).toEqual({ song: '(Untitled)', tag: null });
		expect(splitTitle('Song (feat. X) Remix')).toEqual({ song: 'Song (feat. X) Remix', tag: null });
	});
});
