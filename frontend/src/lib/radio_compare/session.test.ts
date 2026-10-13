import { describe, expect, it } from 'vitest';
import type { RadioCompareArm, RadioCompareTrack } from '$lib/api/client';
import {
	addSeed,
	armName,
	emptySession,
	parseSession,
	removeSeed,
	setBest,
	setWorst,
	shuffled,
	stationLabel,
	summarize,
	toggleMisfit,
	trackKey,
	withArms,
} from './session';

function track(tidalId: number, localId: number | null = null): RadioCompareTrack {
	return {
		tidal_id: tidalId,
		title: `Track ${tidalId}`,
		artist_name: 'Artist',
		album_title: null,
		artwork_url: null,
		duration_ms: 200_000,
		local_id: localId,
	};
}

function arm(id: string, tidalIds: number[], note: string | null = null): RadioCompareArm {
	return { arm: id, tracks: tidalIds.map((t) => track(t)), note };
}

/** Deterministic stand-in for Math.random. */
function sequence(...values: number[]) {
	let i = 0;
	return () => values[i++ % values.length];
}

const seed = track(1000, 7);

describe('radio blind comparison session', () => {
	it('keys tracks by TIDAL id, falling back to the library id', () => {
		expect(trackKey(track(42))).toBe('t42');
		expect(trackKey(track(0, 9))).toBe('l9');
	});

	it('labels stations A, B, C, D', () => {
		expect([0, 1, 2, 3].map(stationLabel)).toEqual(['A', 'B', 'C', 'D']);
	});

	it('shuffles into a permutation of the input', () => {
		const out = shuffled(['current', 'lastfm', 'tidal'], sequence(0.9, 0.1));
		expect([...out].sort()).toEqual(['current', 'lastfm', 'tidal']);
		expect(shuffled(['a', 'b'], sequence(0.99))).toEqual(['a', 'b']);
		expect(shuffled(['a', 'b'], sequence(0))).toEqual(['b', 'a']);
	});

	it('adds a seed once and removes it', () => {
		let session = addSeed(emptySession(), seed);
		session = addSeed(session, seed);
		expect(session.entries).toHaveLength(1);
		expect(session.entries[0].arms).toBeNull();
		expect(removeSeed(session, 7).entries).toHaveLength(0);
	});

	it('hides which station is which behind a shuffled order that survives a reload', () => {
		let session = addSeed(emptySession(), seed);
		const arms = [arm('current', [1, 2]), arm('lastfm', [3]), arm('tidal', [4])];
		session = withArms(session, 7, arms, sequence(0, 0));
		const order = session.entries[0].order;
		expect([...order].sort()).toEqual(['current', 'lastfm', 'tidal']);
		expect(order).not.toEqual(['current', 'lastfm', 'tidal']);

		session = toggleMisfit(session, 7, 'lastfm', 't3');
		const reloaded = withArms(session, 7, arms, sequence(0.5));
		expect(reloaded.entries[0].order).toEqual(order);
		expect(reloaded.entries[0].misfits.lastfm).toEqual(['t3']);
	});

	it('toggles a misfit mark and picks one best station', () => {
		let session = withArms(addSeed(emptySession(), seed), 7, [arm('current', [1])]);
		session = toggleMisfit(session, 7, 'current', 't1');
		expect(session.entries[0].misfits.current).toEqual(['t1']);
		session = toggleMisfit(session, 7, 'current', 't1');
		expect(session.entries[0].misfits.current).toEqual([]);
		session = setBest(session, 7, 'current');
		expect(session.entries[0].best).toBe('current');
		expect(setBest(session, 7, 'current').entries[0].best).toBeNull();
	});

	it('picks one worst station, never the same as the best', () => {
		let session = withArms(addSeed(emptySession(), seed), 7, [arm('current', [1]), arm('tidal', [2])]);
		session = setBest(session, 7, 'current');
		session = setWorst(session, 7, 'tidal');
		expect(session.entries[0]).toMatchObject({ best: 'current', worst: 'tidal' });
		session = setWorst(session, 7, 'current');
		expect(session.entries[0]).toMatchObject({ best: null, worst: 'current' });
		session = setBest(session, 7, 'current');
		expect(session.entries[0]).toMatchObject({ best: 'current', worst: null });
		expect(setWorst(setWorst(session, 7, 'tidal'), 7, 'tidal').entries[0].worst).toBeNull();
	});

	it('counts worst picks per design', () => {
		let session = withArms(addSeed(emptySession(), seed), 7, [arm('current', [1]), arm('tidal', [2])]);
		session = setWorst(setBest(session, 7, 'current'), 7, 'tidal');
		const byArm = Object.fromEntries(summarize(session).map((s) => [s.arm, s]));
		expect(byArm.tidal.losses).toBe(1);
		expect(byArm.current.losses).toBe(0);
	});

	it('loads a session saved before worst picks existed', () => {
		const saved = JSON.parse(
			JSON.stringify(setBest(withArms(addSeed(emptySession(), seed), 7, [arm('current', [1])]), 7, 'current'))
		);
		delete saved.entries[0].worst;
		expect(parseSession(saved).entries[0].worst).toBeNull();
	});

	it('scores only judged seeds: fit is the share of tracks not marked', () => {
		let session = emptySession();
		session = addSeed(session, seed);
		session = addSeed(session, track(2000, 8));
		session = withArms(session, 7, [arm('current', [1, 2, 3, 4]), arm('tidal', [5, 6])]);
		session = withArms(session, 8, [arm('current', [7]), arm('tidal', [], 'No radio')]);
		session = toggleMisfit(session, 7, 'current', 't1');
		session = toggleMisfit(session, 7, 'tidal', 't5');
		session = toggleMisfit(session, 7, 'tidal', 't6');
		session = setBest(session, 7, 'current');
		// Seed 8 is loaded but not judged yet: it does not count.
		session = toggleMisfit(session, 8, 'current', 't7');

		const summary = Object.fromEntries(summarize(session).map((s) => [s.arm, s]));
		expect(summary.current).toMatchObject({ seeds: 1, tracks: 4, misfits: 1, wins: 1, losses: 0, empty: 0 });
		expect(summary.current.fit).toBeCloseTo(0.75);
		expect(summary.tidal).toMatchObject({ seeds: 1, tracks: 2, misfits: 2, wins: 0 });
		expect(summary.tidal.fit).toBe(0);
	});

	it('counts a judged seed with an empty list separately', () => {
		let session = withArms(addSeed(emptySession(), seed), 7, [
			arm('current', [1]),
			arm('tidal', [], 'No radio'),
		]);
		session = setBest(session, 7, 'current');
		const tidal = summarize(session).find((s) => s.arm === 'tidal')!;
		expect(tidal).toMatchObject({ seeds: 1, tracks: 0, empty: 1, fit: null });
	});

	it('reads back a saved session and refuses anything else', () => {
		const session = setBest(withArms(addSeed(emptySession(), seed), 7, [arm('current', [1])]), 7, 'current');
		expect(parseSession(JSON.parse(JSON.stringify(session)))).toEqual(session);
		expect(parseSession(null)).toEqual(emptySession());
		expect(parseSession({ version: 99, entries: [] })).toEqual(emptySession());
		expect(parseSession({ version: 1, entries: 'nope' })).toEqual(emptySession());
	});

	it('names each design for the reveal', () => {
		expect(armName('current')).toBe('Current station');
		expect(armName('tidal')).toBe('TIDAL radio');
		expect(armName('something-new')).toBe('something-new');
	});
});
